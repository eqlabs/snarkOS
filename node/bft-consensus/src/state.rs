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

use anyhow::bail;
use async_trait::async_trait;
use bytes::BytesMut;
use fastcrypto::bls12381::min_sig::BLS12381PublicKey;
use narwhal_executor::ExecutionState;
use narwhal_types::ConsensusOutput;
use tracing::*;

use snarkos_node_consensus::Consensus as AleoConsensus;
use snarkos_node_messages::{Data, Message, NewBlock};
use snarkos_node_router::Router;
use snarkos_node_tcp::protocols::Writing;
use snarkvm::prelude::{ConsensusStorage, Network};

pub(crate) struct BftExecutionState<N: Network, C: ConsensusStorage<N>> {
    primary_pub: BLS12381PublicKey,
    router: Router<N>,
    consensus: AleoConsensus<N, C>,
}

impl<N: Network, C: ConsensusStorage<N>> BftExecutionState<N, C> {
    pub(crate) fn new(primary_pub: BLS12381PublicKey, router: Router<N>, consensus: AleoConsensus<N, C>) -> Self {
        Self { primary_pub, router, consensus }
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> ExecutionState for BftExecutionState<N, C> {
    /// Receive the consensus result with the ordered transactions in `ConsensusOutupt`
    async fn handle_consensus_output(&self, consensus_output: ConsensusOutput) {
        let leader = &consensus_output.sub_dag.leader.header.author;
        let mut leader_id = leader.to_string();
        leader_id.truncate(8);

        let mut validator_id = self.primary_pub.to_string();
        validator_id.truncate(8);

        info!(
            "Consensus (id: {}) output for round {}: {} batches, leader: {}",
            validator_id,
            consensus_output.sub_dag.leader.header.round,
            consensus_output.sub_dag.num_batches(),
            leader_id,
        );

        if consensus_output.batches.is_empty() {
            info!("There are no batches to process; not attempting to create a block.");
        } else {
            if self.primary_pub != *leader {
                info!("I'm not the current leader (id: {}), yielding block production.", validator_id);
                return;
            } else {
                info!("I'm the current leader (id: {}); producing a block.", validator_id);
            }

            let consensus = self.consensus.clone();
            let private_key = *self.router.private_key();
            let next_block = tokio::task::spawn_blocking(move || {
                // Collect all the transactions contained in the agreed upon batches.
                let mut transactions = Vec::new();
                for batch in consensus_output.batches {
                    for batch in batch.1 {
                        for transaction in batch.transactions {
                            let bytes = BytesMut::from(&transaction[..]);
                            // TransactionValidator ensures that the Message can be deserialized.
                            let message = Message::<N>::deserialize(bytes).unwrap();

                            let unconfirmed_transaction =
                                if let Message::UnconfirmedTransaction(unconfirmed_transaction) = message {
                                    unconfirmed_transaction
                                } else {
                                    // TransactionValidator ensures that the Message is an UnconfirmedTransaction.
                                    unreachable!();
                                };

                            // TransactionValidator ensures that the Message can be deserialized.
                            let transaction = unconfirmed_transaction.transaction.deserialize_blocking().unwrap();

                            transactions.push(transaction);
                        }
                    }
                }

                // Attempt to add the batched transactions to the Aleo mempool.
                let mut num_valid_txs = 0;
                for transaction in transactions {
                    // Skip invalid transactions.
                    if consensus.add_unconfirmed_transaction(transaction).is_ok() {
                        num_valid_txs += 1;
                    }
                }

                // Return early if there are no valid transactions.
                if num_valid_txs == 0 {
                    debug!("No valid transactions in ConsensusOutput; not producing a block.");
                    return Ok(None);
                }

                // Propose a new block.
                let next_block = match consensus.propose_next_block(&private_key, &mut rand::thread_rng()) {
                    Ok(block) => block,
                    Err(error) => bail!("Failed to propose the next block: {error}"),
                };

                // Ensure the block is a valid next block.
                if let Err(error) = consensus.check_next_block(&next_block) {
                    // Clear the memory pool of all solutions and transactions.
                    consensus.clear_memory_pool();
                    bail!("Proposed an invalid block: {error}");
                }

                // Advance to the next block.
                match consensus.advance_to_next_block(&next_block) {
                    Ok(()) => {
                        // Log the next block.
                        match serde_json::to_string_pretty(&next_block.header()) {
                            Ok(header) => info!("Block {}: {header}", next_block.height()),
                            Err(error) => info!("Block {}: (serde failed: {error})", next_block.height()),
                        }
                    }
                    Err(error) => {
                        // Clear the memory pool of all solutions and transactions.
                        consensus.clear_memory_pool();
                        bail!("Failed to advance to the next block: {error}");
                    }
                }

                info!("Produced a block with {num_valid_txs} transactions.");

                Ok(Some(next_block))
            })
            .await;

            let next_block = match next_block.map_err(|err| err.into()) {
                Ok(Ok(Some(block))) => block,
                Ok(Ok(None)) => return,
                Ok(Err(error)) | Err(error) => {
                    error!("Failed to produce a new block: {error}");
                    return;
                }
            };

            let next_block_round = next_block.round();
            let next_block_height = next_block.height();
            let next_block_hash = next_block.hash();

            // Serialize the block ahead of time to not do it for each peer.
            let serialized_block = match Data::Object(next_block).serialize().await {
                Ok(serialized_block) => Data::Buffer(serialized_block),
                Err(error) => unreachable!("Failed to serialize own block: {error}"),
            };

            // Prepare the block to be sent to all peers.
            let message = Message::<N>::NewBlock(NewBlock::new(
                next_block_round,
                next_block_height,
                next_block_hash,
                serialized_block,
            ));

            // Broadcast the new block.
            self.router.broadcast(message).unwrap();
        }
    }

    async fn last_executed_sub_dag_index(&self) -> u64 {
        // TODO: this seems like a potential optimization, but shouldn't be needed
        0
    }
}
