// Copyright (C) 2019-2023 Aleo Systems Inc.
// This file is part of the snarkOS library.

// The snarkOS library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The snarkOS library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the snarkOS library. If not, see <https://www.gnu.org/licenses/>.

use crate::{Outbound, Peer, Router};
use snarkos_node_messages::{
    ChallengeRequest,
    ChallengeResponse,
    Data,
    Disconnect,
    DisconnectReason,
    Message,
    MessageCodec,
    MessageTrait,
};
use snarkos_node_tcp::{protocols::Handshake, Connection, ConnectionSide};
use snarkvm::prelude::{error, Address, Header, Network};

use anyhow::{bail, Result};
use futures::SinkExt;
use rand::{rngs::OsRng, Rng};
use std::{io, net::SocketAddr};
use tokio::net::TcpStream;
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;

#[async_trait]
pub trait ExtendedHandshake<N: Network>: Handshake + Outbound<N> {
    /* User implemented methods. */

    fn genesis_header(&self) -> io::Result<Header<N>>;

    async fn custom_handshake<'a>(
        &'a self,
        _conn_addr: SocketAddr,
        peer: Peer<N>,
        framed: Framed<&'a mut TcpStream, MessageCodec<N>>,
    ) -> io::Result<(Peer<N>, Framed<&'a mut TcpStream, MessageCodec<N>>)> {
        Ok((peer, framed))
    }

    /* Provided implementations. */

    async fn extended_handshake<'a>(
        &'a self,
        connection: &'a mut Connection,
    ) -> io::Result<(Peer<N>, Framed<&'a mut TcpStream, MessageCodec<N>>)> {
        let conn_addr = connection.addr();
        let conn_side = connection.side();
        match self.extended_handshake_inner(connection).await {
            // If the handshake is succesful, insert the peer and its connection address.
            Ok((peer, framed)) => {
                self.router().insert_connected_peer(peer.clone(), conn_addr);

                // Log the success.
                let base_msg = format!("Successfully shook hands with peer '{}' (listening addr)", peer.ip());
                match conn_side {
                    // Peer is the initiator.
                    ConnectionSide::Initiator => info!("{base_msg} on '{conn_addr}' (connection addr)",),

                    // Peer is the responder.
                    ConnectionSide::Responder => info!("{base_msg}"),
                }

                Ok((peer, framed))
            }

            // If the handshake isn't succesful, clear the peer from the connecting collection.
            Err(e) => {
                self.router().connecting_peers.lock().remove(&conn_addr);
                Err(e)
            }
        }
    }

    async fn extended_handshake_inner<'a>(
        &'a self,
        connection: &'a mut Connection,
    ) -> io::Result<(Peer<N>, Framed<&'a mut TcpStream, MessageCodec<N>>)> {
        let conn_addr = connection.addr();
        let conn_side = connection.side();
        let stream = self.borrow_stream(connection);
        let genesis_header = self.genesis_header()?;

        let (peer, framed) = self.router().handshake(conn_addr, stream, conn_side, genesis_header).await?;
        let (peer, framed) = self.custom_handshake(conn_addr, peer, framed).await?;

        Ok((peer, framed))
    }
}

impl<N: Network> Router<N> {
    /// A helper that facilitates some extra error handling in `Router::handshake`.
    pub async fn handshake<'a>(
        &'a self,
        conn_addr: SocketAddr,
        stream: &'a mut TcpStream,
        peer_side: ConnectionSide,
        genesis_header: Header<N>,
    ) -> io::Result<(Peer<N>, Framed<&mut TcpStream, MessageCodec<N>>)> {
        // If this is an inbound connection, we log it, but don't know the listening address yet.
        // Otherwise, we can immediately register the listening address.
        let mut peer_ip = if peer_side == ConnectionSide::Initiator {
            debug!("Received a connection request from '{conn_addr}'");
            None
        } else {
            Some(conn_addr)
        };

        // Construct the stream.
        let mut framed = Framed::new(stream, MessageCodec::<N>::default());

        /* Step 1: Send the challenge request. */

        // Initialize an RNG.
        let rng = &mut OsRng;
        // Sample a random nonce.
        let nonce_a = rng.gen();

        // Send a challenge request to the peer.
        let message_a = Message::<N>::ChallengeRequest(ChallengeRequest {
            version: Message::<N>::VERSION,
            listener_port: self.local_ip().port(),
            node_type: self.node_type,
            address: self.address(),
            nonce: nonce_a,
        });
        trace!("Sending '{}-A' to '{conn_addr}'", message_a.name());
        framed.send(message_a).await?;

        /* Step 2: Receive the challenge request. */

        // Listen for the challenge request message.
        let request_b = match framed.try_next().await? {
            // Received the challenge request message, proceed.
            Some(Message::ChallengeRequest(data)) => data,
            // Received a disconnect message, abort.
            Some(Message::Disconnect(reason)) => return Err(error(format!("'{conn_addr}' disconnected: {reason:?}"))),
            // Received an unexpected message, abort.
            _ => return Err(error(format!("'{conn_addr}' did not send a challenge request"))),
        };
        trace!("Received '{}-B' from '{conn_addr}'", request_b.name());

        // Obtain the peer's listening address if it's an inbound connection.
        if peer_ip.is_none() {
            peer_ip = Some(SocketAddr::new(conn_addr.ip(), request_b.listener_port));
        }

        // This value is now guaranteed to be present, so it can be unwrapped.
        let peer_ip = peer_ip.unwrap();

        // Knowing the peer's listening address, ensure it is allowed to connect.
        if peer_side == ConnectionSide::Initiator {
            if let Err(forbidden_message) = self.ensure_peer_is_allowed(peer_ip) {
                return Err(error(format!("{forbidden_message}")));
            }
        }

        // Verify the challenge request. If a disconnect reason was returned, send the disconnect message and abort.
        if let Some(reason) = self.verify_challenge_request(conn_addr, &request_b) {
            trace!("Sending 'Disconnect' to '{conn_addr}'");
            framed.send(Message::Disconnect(Disconnect { reason: reason.clone() })).await?;
            return Err(error(format!("Dropped '{conn_addr}' for reason: {reason:?}")));
        }

        /* Step 3: Send the challenge response. */

        // Sign the counterparty nonce.
        let signature_b = self
            .account
            .sign_bytes(&request_b.nonce.to_le_bytes(), rng)
            .map_err(|_| error(format!("Failed to sign the challenge request nonce from '{conn_addr}'")))?;

        // Send the challenge response.
        let message_b =
            Message::ChallengeResponse(ChallengeResponse { genesis_header, signature: Data::Object(signature_b) });
        trace!("Sending '{}-B' to '{conn_addr}'", message_b.name());
        framed.send(message_b).await?;

        /* Step 4: Receive the challenge response. */

        // Listen for the challenge response message.
        let response_a = match framed.try_next().await? {
            // Received the challenge response message, proceed.
            Some(Message::ChallengeResponse(data)) => data,
            // Received a disconnect message, abort.
            Some(Message::Disconnect(reason)) => return Err(error(format!("'{conn_addr}' disconnected: {reason:?}"))),
            // Received an unexpected message, abort.
            _ => return Err(error(format!("'{conn_addr}' did not send a challenge response"))),
        };
        trace!("Received '{}-A' from '{conn_addr}'", response_a.name());

        // Verify the challenge response. If a disconnect reason was returned, send the disconnect message and abort.
        if let Some(reason) =
            self.verify_challenge_response(conn_addr, request_b.address, response_a, genesis_header, nonce_a).await
        {
            trace!("Sending 'Disconnect' to '{conn_addr}'");
            framed.send(Message::Disconnect(Disconnect { reason: reason.clone() })).await?;
            return Err(error(format!("Dropped '{conn_addr}' for reason: {reason:?}")));
        }

        /* Step 5: Construct the peer. */

        // Note: adding the peer to the router will need to be done from the node-specific
        // handshake implementations for now.

        // Construct the peer.
        let peer_address = request_b.address;
        let peer_type = request_b.node_type;
        let peer_version = request_b.version;

        let peer = Peer::new(peer_ip, peer_address, peer_type, peer_version);

        Ok((peer, framed))
    }

    /// Ensure the peer is allowed to connect.
    fn ensure_peer_is_allowed(&self, peer_ip: SocketAddr) -> Result<()> {
        // Ensure the peer IP is not this node.
        if self.is_local_ip(&peer_ip) {
            bail!("Dropping connection request from '{peer_ip}' (attempted to self-connect)")
        }
        // Ensure the node is not already connecting to this peer.
        if !self.connecting_peers.lock().insert(peer_ip) {
            bail!("Dropping connection request from '{peer_ip}' (already shaking hands as the initiator)")
        }
        // Ensure the node is not already connected to this peer.
        if self.is_connected(&peer_ip) {
            bail!("Dropping connection request from '{peer_ip}' (already connected)")
        }
        // Ensure the peer is not restricted.
        if self.is_restricted(&peer_ip) {
            bail!("Dropping connection request from '{peer_ip}' (restricted)")
        }
        // Ensure the peer is not spamming connection attempts.
        if !peer_ip.ip().is_loopback() {
            // Add this connection attempt and retrieve the number of attempts.
            let num_attempts = self.cache.insert_inbound_connection(peer_ip.ip(), Self::RADIO_SILENCE_IN_SECS as i64);
            // Ensure the connecting peer has not surpassed the connection attempt limit.
            if num_attempts > Self::MAXIMUM_CONNECTION_FAILURES {
                // Restrict the peer.
                self.insert_restricted_peer(peer_ip);
                bail!("Dropping connection request from '{peer_ip}' (tried {num_attempts} times)")
            }
        }
        Ok(())
    }

    /// Verifies the given challenge request. Returns a disconnect reason if the request is invalid.
    fn verify_challenge_request(
        &self,
        peer_addr: SocketAddr,
        message: &ChallengeRequest<N>,
    ) -> Option<DisconnectReason> {
        // Retrieve the components of the challenge request.
        let &ChallengeRequest { version, listener_port: _, node_type, address, nonce: _ } = message;

        // Ensure the message protocol version is not outdated.
        if version < Message::<N>::VERSION {
            warn!("Dropping '{peer_addr}' on version {version} (outdated)");
            return Some(DisconnectReason::OutdatedClientVersion);
        }

        // TODO (howardwu): Remove this after Phase 2.
        if !self.is_dev
            && node_type.is_beacon()
            && address.to_string() != "aleo1q6qstg8q8shwqf5m6q5fcenuwsdqsvp4hhsgfnx5chzjm3secyzqt9mxm8"
        {
            warn!("Dropping '{peer_addr}' for an invalid {node_type}");
            return Some(DisconnectReason::ProtocolViolation);
        }

        None
    }

    /// Verifies the given challenge response. Returns a disconnect reason if the response is invalid.
    async fn verify_challenge_response(
        &self,
        peer_addr: SocketAddr,
        peer_address: Address<N>,
        response: ChallengeResponse<N>,
        expected_genesis_header: Header<N>,
        expected_nonce: u64,
    ) -> Option<DisconnectReason> {
        // Retrieve the components of the challenge response.
        let ChallengeResponse { genesis_header, signature } = response;

        // Verify the challenge response, by checking that the block header matches.
        if genesis_header != expected_genesis_header {
            warn!("Handshake with '{peer_addr}' failed (incorrect block header)");
            return Some(DisconnectReason::InvalidChallengeResponse);
        }

        // Perform the deferred non-blocking deserialization of the signature.
        let signature = match signature.deserialize().await {
            Ok(signature) => signature,
            Err(_) => {
                warn!("Handshake with '{peer_addr}' failed (cannot deserialize the signature)");
                return Some(DisconnectReason::InvalidChallengeResponse);
            }
        };

        // Verify the signature.
        if !signature.verify_bytes(&peer_address, &expected_nonce.to_le_bytes()) {
            warn!("Handshake with '{peer_addr}' failed (invalid signature)");
            return Some(DisconnectReason::InvalidChallengeResponse);
        }

        None
    }
}
