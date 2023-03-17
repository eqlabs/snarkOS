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

use anyhow::{anyhow, bail, Result};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::BytesMut;
use fastcrypto::{
    bls12381::min_sig::{BLS12381KeyPair, BLS12381PublicKey},
    ed25519::Ed25519KeyPair,
    encoding::{Base64, Encoding},
    traits::{EncodeDecodeBase64, KeyPair, ToFromBytes},
};
use narwhal_config::{Committee, Import, Parameters, WorkerCache};
use narwhal_crypto::NetworkKeyPair;
use narwhal_executor::ExecutionState;
use narwhal_node::{primary_node::PrimaryNode, worker_node::WorkerNode, NodeStorage};
use narwhal_types::{Batch, ConsensusOutput};
use std::{path::PathBuf, sync::Arc};
use tracing::*;

use aleo_std::aleo_dir;
use snarkos_node_consensus::Consensus as AleoConsensus;
use snarkos_node_messages::{Data, Message, NewBlock};
use snarkos_node_router::Router;
use snarkos_node_tcp::protocols::Writing;
use snarkvm::prelude::{ConsensusStorage, Network};

pub struct BftConsensus<N: Network, C: ConsensusStorage<N>> {
    // TODO(nkls): remove this
    id: u32,
    primary_keypair: BLS12381KeyPair,
    network_keypair: NetworkKeyPair,
    worker_keypair: NetworkKeyPair,
    parameters: Parameters,
    p_store: NodeStorage,
    w_store: NodeStorage,
    committee: Arc<ArcSwap<Committee>>,
    worker_cache: Arc<ArcSwap<WorkerCache>>,
    aleo_consensus: AleoConsensus<N, C>,
    aleo_router: Router<N>,
}

fn base_path(dev: Option<u16>) -> PathBuf {
    // Retrieve the starting directory.
    match dev.is_some() {
        // In development mode, the ledger is stored in the root directory of the repository.
        true => match std::env::current_dir() {
            Ok(current_dir) => current_dir,
            _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        },
        // In production mode, the ledger is stored in the `~/.aleo/` directory.
        false => aleo_dir(),
    }
}

fn primary_dir(network: u16, dev: Option<u16>) -> PathBuf {
    let mut path = base_path(dev);

    // Construct the path to the ledger in storage.
    //
    // Prod: `~/.aleo/storage/bft-{network}/primary`
    // Dev: `path/to/repo/.bft-{network}/primary-{id}`
    match dev {
        Some(id) => {
            path.push(format!(".bft-{network}"));
            path.push(format!("primary-{id}"));
        }

        None => {
            path.push("storage");
            path.push(format!("bft-{network}"));
            path.push("primary");
        }
    }

    path
}

fn worker_dir(network: u16, worker_id: u32, dev: Option<u16>) -> PathBuf {
    // Retrieve the starting directory.
    let mut path = base_path(dev);

    // Construct the path to the ledger in storage.
    //
    // Prod: `~/.aleo/storage/bft-{network}/worker-{worker_id}`
    // Dev: `path/to/repo/.bft-{network}/worker-{primary_id}-{worker_id}`
    match dev {
        Some(primary_id) => {
            path.push(format!(".bft-{network}"));
            path.push(format!("worker-{primary_id}-{worker_id}"));
        }

        None => {
            path.push("storage");
            path.push(format!("bft-{network}"));
            path.push(format!("worker-{worker_id}"));
        }
    }

    path
}

impl<N: Network, C: ConsensusStorage<N>> BftConsensus<N, C> {
    pub fn new(aleo_consensus: AleoConsensus<N, C>, aleo_router: Router<N>, dev: Option<u16>) -> Result<Self> {
        // Offset here as the beacon is started on 0 and validators have their keys counted from 0
        // currently.
        let id = dev.expect("only dev mode is supported currently") - 1;
        let primary_key_file = format!("{}/.primary-{id}-key.json", env!("CARGO_MANIFEST_DIR"));
        let primary_keypair =
            read_authority_keypair_from_file(primary_key_file).expect("Failed to load the node's primary keypair");
        let primary_network_key_file = format!("{}/.primary-{id}-network-key.json", env!("CARGO_MANIFEST_DIR"));
        let network_keypair = read_network_keypair_from_file(primary_network_key_file)
            .expect("Failed to load the node's primary network keypair");
        let worker_key_file = format!("{}/.worker-{id}-key.json", env!("CARGO_MANIFEST_DIR"));
        let worker_keypair =
            read_network_keypair_from_file(worker_key_file).expect("Failed to load the node's worker keypair");
        debug!("creating task {}", id);
        // Read the committee, workers and node's keypair from file.
        let committee_file = format!("{}/.committee.json", env!("CARGO_MANIFEST_DIR"));
        let committee = Arc::new(ArcSwap::from_pointee(
            Committee::import(&committee_file).expect("Failed to load the committee information"),
        ));
        let workers_file = format!("{}/.workers.json", env!("CARGO_MANIFEST_DIR"));
        let worker_cache = Arc::new(ArcSwap::from_pointee(
            WorkerCache::import(&workers_file).expect("Failed to load the worker information"),
        ));

        // Load default parameters if none are specified.
        let filename = format!("{}/.parameters.json", env!("CARGO_MANIFEST_DIR"));
        let parameters = Parameters::import(&filename).expect("Failed to load the node's parameters");

        // Make the data store.
        let p_store_path = primary_dir(N::ID, dev);
        let p_store = NodeStorage::reopen(p_store_path);
        let w_store_path = worker_dir(N::ID, 0, dev);
        let w_store = NodeStorage::reopen(w_store_path);
        Ok(Self {
            id: id.into(),
            primary_keypair,
            network_keypair,
            worker_keypair,
            parameters,
            p_store,
            w_store,
            committee,
            worker_cache,
            aleo_consensus,
            aleo_router,
        })
    }

    /// Start the primary and worker node
    /// only 1 worker is spawned ATM
    /// caller must call `wait().await` on primary and worker
    pub async fn start(self) -> Result<(PrimaryNode, WorkerNode)> {
        let primary_pub = self.primary_keypair.public().clone();
        let primary = PrimaryNode::new(self.parameters.clone(), true);
        let bft_execution_state =
            BftExecutionState::new(primary_pub.clone(), self.aleo_router.clone(), self.aleo_consensus.clone());

        primary
            .start(
                self.primary_keypair,
                self.network_keypair,
                self.committee.clone(),
                self.worker_cache.clone(),
                &self.p_store,
                Arc::new(bft_execution_state),
            )
            .await?;

        info!("Created a primary with id {} and public key {}", self.id, primary_pub);

        let worker = WorkerNode::new(0, self.parameters.clone());
        let worker_pub = self.worker_keypair.public().clone();
        worker
            .start(
                primary_pub,
                self.worker_keypair,
                self.committee.clone(),
                self.worker_cache,
                &self.w_store,
                TransactionValidator(self.aleo_consensus),
            )
            .await?;
        info!("Created a worker with id 0 and public key {}", worker_pub);

        Ok((primary, worker))
    }
}

pub struct BftExecutionState<N: Network, C: ConsensusStorage<N>> {
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

pub fn read_network_keypair_from_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Ed25519KeyPair> {
    let contents = std::fs::read_to_string(path)?;
    let bytes = Base64::decode(contents.as_str()).map_err(|e| anyhow!("{}", e.to_string()))?;
    Ed25519KeyPair::from_bytes(bytes.get(1..).unwrap()).map_err(|e| anyhow!(e))
}

pub fn read_authority_keypair_from_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<BLS12381KeyPair> {
    let contents = std::fs::read_to_string(path)?;
    BLS12381KeyPair::decode_base64(contents.as_str().trim()).map_err(|e| anyhow!(e))
}

#[derive(Clone)]
struct TransactionValidator<N: Network, C: ConsensusStorage<N>>(AleoConsensus<N, C>);

impl<N: Network, C: ConsensusStorage<N>> narwhal_worker::TransactionValidator for TransactionValidator<N, C> {
    type Error = anyhow::Error;

    /// Determines if a transaction valid for the worker to consider putting in a batch
    fn validate(&self, transaction: &[u8]) -> Result<(), Self::Error> {
        let bytes = BytesMut::from(transaction);
        let message = Message::<N>::deserialize(bytes)?;

        let unconfirmed_transaction = if let Message::UnconfirmedTransaction(unconfirmed_transaction) = message {
            unconfirmed_transaction
        } else {
            bail!("[UnconfirmedTransaction] Expected Message::UnconfirmedTransaction, got {:?}", message.name());
        };

        let transaction = match unconfirmed_transaction.transaction.deserialize_blocking() {
            Ok(transaction) => transaction,
            Err(error) => bail!("[UnconfirmedTransaction] {error}"),
        };

        if let Err(err) = self.0.check_transaction_basic(&transaction) {
            error!("Failed to validate a transaction: {err}");
            return Err(err);
        }

        Ok(())
    }

    /// Determines if this batch can be voted on
    fn validate_batch(&self, batch: &Batch) -> Result<(), Self::Error> {
        // TODO: once the beacon is no longer the source of the transactions, batch validation
        // might need to be disabled to avoid double validation.
        for transaction in &batch.transactions {
            self.validate(transaction)?;
        }

        Ok(())
    }
}
