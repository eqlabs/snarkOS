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

use super::*;

use snarkos_node_bft_consensus::{batched_transactions, sort_transactions};
use snarkos_node_messages::{
    BlockRequest,
    BlockResponse,
    ConsensusId,
    Data,
    DataBlocks,
    DisconnectReason,
    Message,
    MessageCodec,
    NewBlock,
    Ping,
    Pong,
    UnconfirmedTransaction,
};
use snarkos_node_router::{ExtendedHandshake, Peer};
use snarkos_node_tcp::{Connection, ConnectionSide, Tcp};
use snarkvm::prelude::{error, EpochChallenge, Network, Transaction};

use bytes::BytesMut;
use fastcrypto::{
    traits::{Signer, ToFromBytes},
    Verifier,
};
use futures_util::sink::SinkExt;
use std::{collections::HashSet, io, net::SocketAddr, time::Duration};
use tokio::net::TcpStream;
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;

impl<N: Network, C: ConsensusStorage<N>> P2P for Validator<N, C> {
    /// Returns a reference to the TCP instance.
    fn tcp(&self) -> &Tcp {
        self.router.tcp()
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Handshake for Validator<N, C>
where
    Self: ExtendedHandshake<N>,
{
    /// Performs the handshake protocol.
    async fn perform_handshake(&self, mut connection: Connection) -> io::Result<Connection> {
        // Perform the handshake.
        let (peer, mut framed) = self.extended_handshake(&mut connection).await?;

        // TODO: perhaps this can be moved somewhere else in future? It is technically not part of
        // the handshake.

        // Retrieve the block locators.
        let block_locators = match crate::helpers::get_block_locators(&self.ledger) {
            Ok(block_locators) => Some(block_locators),
            Err(e) => {
                error!("Failed to get block locators: {e}");
                return Err(error(format!("Failed to get block locators: {e}")));
            }
        };

        // Send the first `Ping` message to the peer.
        let message = Message::Ping(Ping::new(self.node_type(), block_locators));
        trace!("Sending '{}' to '{}'", message.name(), peer.ip());
        framed.send(message).await?;

        Ok(connection)
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> ExtendedHandshake<N> for Validator<N, C> {
    fn genesis_header(&self) -> io::Result<Header<N>> {
        self.ledger.get_header(0).map_err(|e| error(e.to_string()))
    }

    async fn handshake_extension<'a>(
        &'a self,
        peer_addr: SocketAddr,
        peer: Peer<N>,
        mut framed: Framed<&'a mut TcpStream, MessageCodec<N>>,
    ) -> io::Result<(Peer<N>, Framed<&'a mut TcpStream, MessageCodec<N>>)> {
        if peer.node_type() != NodeType::Validator {
            return Ok((peer, framed));
        }

        // Establish quorum with other validators:
        //
        // 1. Sign and send the node's pub key.
        // 2. Receive and verify peer's signed pub key.
        // 3. Insert into connected_committee_members.
        // 4. If quorum threshold is reached, start the bft.

        // 1.
        // BFT must be set here.
        // TODO: we should probably use something else than the public key, potentially interactive, since this could
        // be copied and reused by a malicious validator.
        let public_key = self.primary_keypair.public();
        let signature = self.primary_keypair.sign(public_key.as_bytes());

        let message = Message::ConsensusId(Box::new(ConsensusId { public_key: public_key.clone(), signature }));
        framed.send(message).await?;

        // 2.
        let consensus_id = match framed.try_next().await? {
            Some(Message::ConsensusId(data)) => data,
            _ => return Err(error(format!("'{peer_addr}' did not send a 'ConsensusId' message"))),
        };

        // Check the advertised public key exists in the committee.
        if !self.committee.keys().contains(&&consensus_id.public_key) {
            return Err(error(format!("'{peer_addr}' is not part of the committee")));
        }

        // Check the signature.
        // TODO: again, the signed message should probably be something we send to the peer, not
        // their public key.
        if consensus_id.public_key.verify(consensus_id.public_key.as_bytes(), &consensus_id.signature).is_err() {
            return Err(error(format!("'{peer_addr}' couldn't verify their identity")));
        }

        // 3.
        // Track the committee member.
        // TODO: in future we could error here if it already exists in the collection but that
        // logic is probably best implemented when dynamic committees are being considered.
        self.router.connected_committee_members.write().insert(peer.ip(), consensus_id.public_key);

        // 4.
        // If quorum is reached, start the consensus but only if it hasn't already been started.
        let connected_stake =
            self.router.connected_committee_members.read().values().map(|pk| self.committee.stake(pk)).sum::<u64>();
        if connected_stake >= self.committee.quorum_threshold() && self.bft.get().is_none() {
            self.start_bft().await.unwrap()
        }

        Ok((peer, framed))
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Disconnect for Validator<N, C> {
    /// Any extra operations to be performed during a disconnect.
    async fn handle_disconnect(&self, peer_addr: SocketAddr) {
        if let Some(peer_ip) = self.router.resolve_to_listener(&peer_addr) {
            self.router.remove_connected_peer(peer_ip);
            self.router.connected_committee_members.write().remove(&peer_ip);
        }
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Writing for Validator<N, C> {
    type Codec = MessageCodec<N>;
    type Message = Message<N>;

    /// Creates an [`Encoder`] used to write the outbound messages to the target stream.
    /// The `side` parameter indicates the connection side **from the node's perspective**.
    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Reading for Validator<N, C> {
    type Codec = MessageCodec<N>;
    type Message = Message<N>;

    /// Creates a [`Decoder`] used to interpret messages from the network.
    /// The `side` param indicates the connection side **from the node's perspective**.
    fn codec(&self, _peer_addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    /// Processes a message received from the network.
    async fn process_message(&self, peer_addr: SocketAddr, message: Self::Message) -> io::Result<()> {
        // Process the message. Disconnect if the peer violated the protocol.
        if let Err(error) = self.inbound(peer_addr, message).await {
            if let Some(peer_ip) = self.router().resolve_to_listener(&peer_addr) {
                warn!("Disconnecting from '{peer_ip}' - {error}");
                self.send(peer_ip, Message::Disconnect(DisconnectReason::ProtocolViolation.into()));
                // Disconnect from this peer.
                self.router().disconnect(peer_ip);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Routing<N> for Validator<N, C> {}

impl<N: Network, C: ConsensusStorage<N>> Heartbeat<N> for Validator<N, C> {
    /// The maximum number of peers permitted to maintain connections with.
    const MAXIMUM_NUMBER_OF_PEERS: usize = 1_000;
}

impl<N: Network, C: ConsensusStorage<N>> Outbound<N> for Validator<N, C> {
    /// Returns a reference to the router.
    fn router(&self) -> &Router<N> {
        &self.router
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Inbound<N> for Validator<N, C> {
    /// Retrieves the blocks within the block request range, and returns the block response to the peer.
    fn block_request(&self, peer_ip: SocketAddr, message: BlockRequest) -> bool {
        let BlockRequest { start_height, end_height } = &message;

        // Retrieve the blocks within the requested range.
        let blocks = match self.ledger.get_blocks(*start_height..*end_height) {
            Ok(blocks) => Data::Object(DataBlocks(blocks)),
            Err(error) => {
                error!("Failed to retrieve blocks {start_height} to {end_height} from the ledger - {error}");
                return false;
            }
        };
        // Send the `BlockResponse` message to the peer.
        self.send(peer_ip, Message::BlockResponse(BlockResponse { request: message, blocks }));
        true
    }

    /// Handles a `BlockResponse` message.
    fn block_response(&self, peer_ip: SocketAddr, blocks: Vec<Block<N>>) -> bool {
        // Insert the candidate blocks into the sync pool.
        for block in blocks {
            if let Err(error) = self.router().sync().insert_block_response(peer_ip, block) {
                warn!("{error}");
                return false;
            }
        }

        // Tries to advance with blocks from the sync pool.
        self.advance_with_sync_blocks();
        true
    }

    /// Handles a `NewBlock` message.
    fn new_block(&self, peer_ip: SocketAddr, block: Block<N>, serialized: NewBlock<N>) -> bool {
        // A failed check doesn't necessarily mean the block is malformed, so return true here.
        if self.consensus.check_next_block(&block).is_err() {
            return true;
        }

        // If the previous consensus output is available, check the order of transactions.
        if let Some(last_consensus_output) = self.bft().state.last_output.lock().clone() {
            let mut expected_txs = batched_transactions(&last_consensus_output)
                .map(|bytes| {
                    // Safe; it's our own consensus output, so we already processed this tx with the TransactionValidator.
                    // Also, it's fast to deserialize, because we only process the ID and keep the actual tx as a blob.
                    // This, of course, assumes that only the ID is used for sorting.
                    let message = Message::<N>::deserialize(BytesMut::from(&bytes[..])).unwrap();

                    let unconfirmed_tx = if let Message::UnconfirmedTransaction(tx) = message {
                        tx
                    } else {
                        // TransactionValidator ensures that the Message is an UnconfirmedTransaction.
                        unreachable!();
                    };

                    unconfirmed_tx.transaction_id
                })
                .collect::<HashSet<_>>();

            // Remove the ids that are not present in the block (presumably dropped due to ledger rejection).
            let block_txs = block.transaction_ids().copied().collect::<HashSet<_>>();
            for id in &expected_txs.clone() {
                if !block_txs.contains(id) {
                    expected_txs.remove(id);
                }
            }

            // Sort the txs according to shared logic.
            let mut expected_txs = expected_txs.into_iter().collect::<Vec<_>>();
            sort_transactions::<N>(&mut expected_txs);

            if block.transaction_ids().zip(&expected_txs).any(|(id1, id2)| id1 != id2) {
                error!("[NewBlock] Invalid order of transactions");
                return false;
            }
        }

        // Attempt to add the block to the ledger.
        if let Err(err) = self.consensus.advance_to_next_block(&block) {
            error!("[NewBlock] {err}");
            return false;
        }

        // TODO: perform more elaborate propagation
        self.propagate(Message::NewBlock(serialized), &[peer_ip]);

        true
    }

    /// Sleeps for a period and then sends a `Ping` message to the peer.
    fn pong(&self, peer_ip: SocketAddr, _message: Pong) -> bool {
        // Spawn an asynchronous task for the `Ping` request.
        let self_clone = self.clone();
        tokio::spawn(async move {
            // Sleep for the preset time before sending a `Ping` request.
            tokio::time::sleep(Duration::from_secs(Self::PING_SLEEP_IN_SECS)).await;
            // Check that the peer is still connected.
            if self_clone.router().is_connected(&peer_ip) {
                // Retrieve the block locators.
                match crate::helpers::get_block_locators(&self_clone.ledger) {
                    // Send a `Ping` message to the peer.
                    Ok(block_locators) => self_clone.send_ping(peer_ip, Some(block_locators)),
                    Err(e) => error!("Failed to get block locators: {e}"),
                }
            }
        });
        true
    }

    /// Retrieves the latest epoch challenge and latest block header, and returns the puzzle response to the peer.
    fn puzzle_request(&self, peer_ip: SocketAddr) -> bool {
        // Retrieve the latest epoch challenge.
        let epoch_challenge = match self.ledger.latest_epoch_challenge() {
            Ok(epoch_challenge) => epoch_challenge,
            Err(error) => {
                error!("Failed to prepare a puzzle request for '{peer_ip}': {error}");
                return false;
            }
        };
        // Retrieve the latest block header.
        let block_header = Data::Object(self.ledger.latest_header());
        // Send the `PuzzleResponse` message to the peer.
        self.send(peer_ip, Message::PuzzleResponse(PuzzleResponse { epoch_challenge, block_header }));
        true
    }

    /// Disconnects on receipt of a `PuzzleResponse` message.
    fn puzzle_response(&self, peer_ip: SocketAddr, _epoch_challenge: EpochChallenge<N>, _header: Header<N>) -> bool {
        debug!("Disconnecting '{peer_ip}' for the following reason - {:?}", DisconnectReason::ProtocolViolation);
        false
    }

    /// Propagates the unconfirmed solution to all connected beacons and validators.
    async fn unconfirmed_solution(
        &self,
        peer_ip: SocketAddr,
        serialized: UnconfirmedSolution<N>,
        solution: ProverSolution<N>,
    ) -> bool {
        // Add the unconfirmed solution to the memory pool.
        if let Err(error) = self.consensus.add_unconfirmed_solution(&solution) {
            trace!("[UnconfirmedSolution] {error}");
            return true; // Maintain the connection.
        }
        let message = Message::UnconfirmedSolution(serialized);
        // Propagate the "UnconfirmedSolution" to the connected beacons.
        self.propagate_to_beacons(message.clone(), &[peer_ip]);
        // Propagate the "UnconfirmedSolution" to the connected validators.
        self.propagate_to_validators(message, &[peer_ip]);
        true
    }

    /// Handles an `UnconfirmedTransaction` message.
    fn unconfirmed_transaction(
        &self,
        peer_ip: SocketAddr,
        serialized: UnconfirmedTransaction<N>,
        _transaction: Transaction<N>,
    ) -> bool {
        let message = Message::UnconfirmedTransaction(serialized);
        // Propagate the "UnconfirmedTransaction" to the connected beacons.
        self.propagate_to_beacons(message.clone(), &[peer_ip]);
        // Propagate the "UnconfirmedTransaction" to the connected validators.
        self.propagate_to_validators(message, &[peer_ip]);
        true
    }
}
