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

/// A macro unwrapping the expected handshake message or returning an error for unexpected messages.
#[macro_export]
macro_rules! expect_message {
    ($msg_ty:path, $framed:expr, $peer_addr:expr) => {
        match $framed.try_next().await? {
            // Received the expected message, proceed.
            Some($msg_ty(data)) => {
                trace!("Received '{}' from '{}'", data.name(), $peer_addr);
                data
            }
            // Received a disconnect message, abort.
            Some(Message::Disconnect(reason)) => {
                return Err(error(format!("'{}' disconnected: {reason:?}", $peer_addr)))
            }
            // Received an unexpected message, abort.
            Some(ty) => {
                return Err(error(format!(
                    "'{}' did not follow the handshake protocol: received {:?} instead of {}",
                    $peer_addr,
                    ty.name(),
                    stringify!($msg_ty),
                )))
            }
            // Received nothing.
            None => {
                return Err(error(format!("'{}' disconnected before sending {:?}", $peer_addr, stringify!($msg_ty),)))
            }
        }
    };
}

/// A macro for cutting a handshake short if message verification fails.
#[macro_export]
macro_rules! handle_verification {
    ($result:expr, $framed:expr, $peer_addr:expr) => {
        if let Some(reason) = $result {
            trace!("Sending 'Disconnect' to '{}'", $peer_addr);
            $framed.send(Message::Disconnect(Disconnect { reason: reason.clone() })).await?;
            return Err(error(format!("Dropped '{}' for reason: {reason:?}", $peer_addr)));
        }
    };
}

/// A trait that enables wrapping custom handshake logic within the router logic.
///
/// This keeps peer collections nicely encapsulated with nicer error handling.
#[async_trait]
pub trait ExtendedHandshake<N: Network>: Handshake + Outbound<N> {
    /* User implemented methods. */

    fn genesis_header(&self) -> io::Result<Header<N>>;

    async fn handshake_extension<'a>(
        &'a self,
        _peer_addr: SocketAddr,
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
        let peer_addr = connection.addr();
        let conn_side = connection.side();
        match self.extended_handshake_inner(connection).await {
            // In case of success, conclude the extended handshake.
            Ok((peer, mut framed)) => {
                // Registed the peer in the list of connected peers.
                self.router().insert_connected_peer(peer.clone(), peer_addr);

                // Log the success.
                let base_msg = format!("Successfully shook hands with peer '{}' (listening addr)", peer.ip());
                match conn_side {
                    ConnectionSide::Initiator => info!("{base_msg} on '{peer_addr}' (connection addr)",),
                    ConnectionSide::Responder => info!("{base_msg}"),
                }

                Ok((peer, framed))
            }

            // In case of an error, perform applicable cleanups.
            Err(e) => {
                self.router().connecting_peers.lock().remove(&peer_addr);
                Err(e)
            }
        }
    }

    async fn extended_handshake_inner<'a>(
        &'a self,
        connection: &'a mut Connection,
    ) -> io::Result<(Peer<N>, Framed<&'a mut TcpStream, MessageCodec<N>>)> {
        let peer_addr = connection.addr();
        let conn_side = connection.side();
        let stream = self.borrow_stream(connection);
        let genesis_header = self.genesis_header()?;

        let (peer, framed) = self.router().handshake(peer_addr, stream, conn_side, genesis_header).await?;
        let (peer, framed) = self.handshake_extension(peer_addr, peer, framed).await?;

        Ok((peer, framed))
    }
}

impl<N: Network> Router<N> {
    /// Executes the handshake protocol.
    pub async fn handshake<'a>(
        &'a self,
        peer_addr: SocketAddr,
        stream: &'a mut TcpStream,
        peer_side: ConnectionSide,
        genesis_header: Header<N>,
    ) -> io::Result<(Peer<N>, Framed<&mut TcpStream, MessageCodec<N>>)> {
        // Perform the handshake.
        if peer_side == ConnectionSide::Responder {
            debug!("Connecting to {peer_addr}...");
            self.handshake_inner_initiator(peer_addr, stream, genesis_header).await
        } else {
            debug!("Received a connection request from '{peer_addr}'");
            self.handshake_inner_responder(peer_addr, stream, genesis_header).await
        }
    }

    /// The connection initiator side of the handshake.
    async fn handshake_inner_initiator<'a>(
        &'a self,
        peer_addr: SocketAddr,
        stream: &'a mut TcpStream,
        genesis_header: Header<N>,
    ) -> io::Result<(Peer<N>, Framed<&mut TcpStream, MessageCodec<N>>)> {
        // Construct the stream.
        let mut framed = Framed::new(stream, MessageCodec::<N>::handshake());

        /* Step 1: Send the challenge request. */

        // Initialize an RNG.
        let rng = &mut OsRng;
        // Sample a random nonce.
        let our_nonce = rng.gen();

        // Send a challenge request to the peer.
        let our_request = ChallengeRequest::new(self.local_ip().port(), self.node_type, self.address(), our_nonce);
        trace!("Sending '{}' to '{peer_addr}'", our_request.name());
        framed.send(Message::ChallengeRequest(our_request)).await?;

        /* Step 2: Receive the peer's challenge response followed by the challenge request. */

        // Listen for the challenge response message.
        let peer_response = expect_message!(Message::ChallengeResponse, framed, peer_addr);

        // Listen for the challenge request message.
        let peer_request = expect_message!(Message::ChallengeRequest, framed, peer_addr);

        // Verify the challenge response. If a disconnect reason was returned, send the disconnect message and abort.
        handle_verification!(
            self.verify_challenge_response(peer_addr, peer_request.address, peer_response, genesis_header, our_nonce)
                .await,
            framed,
            peer_addr
        );

        // Verify the challenge request. If a disconnect reason was returned, send the disconnect message and abort.
        handle_verification!(self.verify_challenge_request(peer_addr, &peer_request), framed, peer_addr);

        /* Step 3: Send the challenge response. */

        // Sign the counterparty nonce.
        let our_signature = self
            .account
            .sign_bytes(&peer_request.nonce.to_le_bytes(), rng)
            .map_err(|_| error(format!("Failed to sign the challenge request nonce from '{peer_addr}'")))?;

        // Send the challenge response.
        let our_response = ChallengeResponse { genesis_header, signature: Data::Object(our_signature) };
        trace!("Sending '{}' to '{peer_addr}'", our_response.name());
        framed.send(Message::ChallengeResponse(our_response)).await?;

        /* Step 4: Construct the peer. */

        // Note: adding the peer to the router will need to be done from the node-specific
        // handshake implementations for now.
        let peer = Peer::new(peer_addr, &peer_request);

        Ok((peer, framed))
    }

    /// The connection responder side of the handshake.
    async fn handshake_inner_responder<'a>(
        &'a self,
        peer_addr: SocketAddr,
        stream: &'a mut TcpStream,
        genesis_header: Header<N>,
    ) -> io::Result<(Peer<N>, Framed<&mut TcpStream, MessageCodec<N>>)> {
        // Construct the stream.
        let mut framed = Framed::new(stream, MessageCodec::<N>::handshake());

        /* Step 1: Receive the challenge request. */

        // Listen for the challenge request message.
        let peer_request = expect_message!(Message::ChallengeRequest, framed, peer_addr);

        // Obtain the peer's listening address.
        let peer_ip = SocketAddr::new(peer_addr.ip(), peer_request.listener_port);

        // Knowing the peer's listening address, ensure it is allowed to connect.
        if let Err(forbidden_message) = self.ensure_peer_is_allowed(peer_ip) {
            return Err(error(format!("{forbidden_message}")));
        }

        // Verify the challenge request. If a disconnect reason was returned, send the disconnect message and abort.
        handle_verification!(self.verify_challenge_request(peer_addr, &peer_request), framed, peer_addr);

        /* Step 2: Send the challenge response followed by own challenge request. */

        // Initialize an RNG.
        let rng = &mut OsRng;

        // Sign the counterparty nonce.
        let our_signature = self
            .account
            .sign_bytes(&peer_request.nonce.to_le_bytes(), rng)
            .map_err(|_| error(format!("Failed to sign the challenge request nonce from '{peer_addr}'")))?;

        // Sample a random nonce.
        let our_nonce = rng.gen();

        // Send the challenge response.
        let our_response = ChallengeResponse { genesis_header, signature: Data::Object(our_signature) };
        trace!("Sending '{}' to '{peer_addr}'", our_response.name());
        framed.send(Message::ChallengeResponse(our_response)).await?;

        // Send the challenge request.
        let our_request = ChallengeRequest::new(self.local_ip().port(), self.node_type, self.address(), our_nonce);
        trace!("Sending '{}' to '{peer_addr}'", our_request.name());
        framed.send(Message::ChallengeRequest(our_request)).await?;

        /* Step 3: Receive the challenge response. */

        // Listen for the challenge response message.
        let peer_response = expect_message!(Message::ChallengeResponse, framed, peer_addr);

        // Verify the challenge response. If a disconnect reason was returned, send the disconnect message and abort.
        handle_verification!(
            self.verify_challenge_response(peer_addr, peer_request.address, peer_response, genesis_header, our_nonce)
                .await,
            framed,
            peer_addr
        );

        /* Step 4: Construct the peer. */

        // Note: adding the peer to the router will need to be done from the node-specific
        // handshake implementations for now.
        let peer = Peer::new(peer_ip, &peer_request);

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
