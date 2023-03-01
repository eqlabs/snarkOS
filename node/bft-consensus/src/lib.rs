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
use eyre::Context;
use fastcrypto::{
    bls12381::min_sig::BLS12381KeyPair,
    ed25519::Ed25519KeyPair,
    encoding::{Base64, Encoding},
    traits::{EncodeDecodeBase64, KeyPair, ToFromBytes},
};
use mysten_metrics::RegistryService;
use narwhal_config::{Committee, Import, Parameters, WorkerCache};
use narwhal_crypto::NetworkKeyPair;
use narwhal_executor::ExecutionState;
use narwhal_node::{primary_node::PrimaryNode, worker_node::WorkerNode, NodeStorage};
use narwhal_types::{Batch, ConsensusOutput};
use prometheus::Registry;
use std::sync::Arc;
use thiserror::Error;
use tracing::*;

use snarkos_account::Account;
use snarkos_node_consensus::Consensus as AleoConsensus;
use snarkos_node_messages::Message;
use snarkvm::prelude::{ConsensusStorage, Network};

pub struct BftConsensus<N: Network, C: ConsensusStorage<N>> {
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
    aleo_account: Account<N>,
}

#[derive(Error, Debug)]
pub enum BftError {
    #[error("Error in BFT: {0}")]
    EyreReport(String),
}

impl<N: Network, C: ConsensusStorage<N>> BftConsensus<N, C> {
    pub fn new(id: u32, aleo_account: Account<N>, aleo_consensus: AleoConsensus<N, C>) -> Result<Self> {
        let primary_key_file = format!(".primary-{id}-key.json");
        let primary_keypair =
            read_authority_keypair_from_file(primary_key_file).expect("Failed to load the node's primary keypair");
        let primary_network_key_file = format!(".primary-{id}-network-key.json");
        let network_keypair = read_network_keypair_from_file(primary_network_key_file)
            .expect("Failed to load the node's primary network keypair");
        let worker_key_file = format!(".worker-{id}-key.json");
        let worker_keypair =
            read_network_keypair_from_file(worker_key_file).expect("Failed to load the node's worker keypair");
        debug!("creating task {}", id);
        // Read the committee, workers and node's keypair from file.
        let committee_file = ".committee.json";
        let committee = Arc::new(ArcSwap::from_pointee(
            Committee::import(committee_file)
                .context("Failed to load the committee information")
                .map_err(|e| BftError::EyreReport(e.to_string()))?,
        ));
        let workers_file = ".workers.json";
        let worker_cache = Arc::new(ArcSwap::from_pointee(
            WorkerCache::import(workers_file)
                .context("Failed to load the worker information")
                .map_err(|e| BftError::EyreReport(e.to_string()))?,
        ));

        // Load default parameters if none are specified.
        let filename = ".parameters.json";
        let parameters = Parameters::import(filename)
            .context("Failed to load the node's parameters")
            .map_err(|e| BftError::EyreReport(e.to_string()))?;

        // Make the data store.
        let store_path = format!(".db-{id}-key.json");
        let p_store = NodeStorage::reopen(store_path);
        let store_path = format!(".db-{id}-0-key.json");
        let w_store = NodeStorage::reopen(store_path);
        Ok(Self {
            id,
            primary_keypair,
            network_keypair,
            worker_keypair,
            parameters,
            p_store,
            w_store,
            committee,
            worker_cache,
            aleo_consensus,
            aleo_account,
        })
    }

    /// Start the primary and worker node
    /// only 1 worker is spawned ATM
    /// caller must call `wait().await` on primary and worker
    pub async fn start(self) -> Result<(PrimaryNode, WorkerNode)> {
        let primary_pub = self.primary_keypair.public().clone();
        let primary = PrimaryNode::new(self.parameters.clone(), true, RegistryService::new(Registry::new()));
        primary
            .start(
                self.primary_keypair,
                self.network_keypair,
                self.committee.clone(),
                self.worker_cache.clone(),
                &self.p_store,
                Arc::new(MyExecutionState::new(self.id, self.aleo_account, self.aleo_consensus.clone())),
            )
            .await?;

        info!("created primary id {}", self.id);

        let worker = WorkerNode::new(0, self.parameters.clone(), RegistryService::new(Registry::new()));
        worker
            .start(
                primary_pub,
                self.worker_keypair,
                self.committee.clone(),
                self.worker_cache,
                &self.w_store,
                TransactionValidator(self.aleo_consensus),
                None,
            )
            .await?;
        info!("created worker id {}", self.id);

        Ok((primary, worker))
    }
}

pub struct MyExecutionState<N: Network, C: ConsensusStorage<N>> {
    id: u32,
    account: Account<N>,
    consensus: AleoConsensus<N, C>,
}

impl<N: Network, C: ConsensusStorage<N>> MyExecutionState<N, C> {
    pub(crate) fn new(id: u32, account: Account<N>, consensus: AleoConsensus<N, C>) -> Self {
        Self { id, account, consensus }
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> ExecutionState for MyExecutionState<N, C> {
    /// Receive the consensus result with the ordered transactions in `ConsensusOutupt`
    async fn handle_consensus_output(&self, consensus_output: ConsensusOutput) {
        if !consensus_output.batches.is_empty() {
            info!(
                "Node {} consensus output for round {}: {:?} batches, leader: {:?}",
                self.id,
                consensus_output.sub_dag.leader.header.round,
                consensus_output.batches.len(),
                consensus_output.sub_dag.leader.header.author,
            );

            /*
            TODO: the following generally works, but there are some open points
                1. currently, only blocks produced by the beacon are considered valid
                2. can the Aleo mempool have a different set of transactions than ones accepted by the consensus?
                3. we can't fail here, i.e. the checks by the TransactionValidator must be final
                4. every validator can create a block, but should they? the downstream can create it on its own too

            let consensus = self.consensus.clone();
            let account = self.account.clone();
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
                let next_block = match consensus.propose_next_block(account.private_key(), &mut rand::thread_rng()) {
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

            info!(
                "Produced a block with the following txs: {:?}",
                next_block.transactions().iter().map(|tx| tx.id()).collect::<Vec<_>>()
            );
            */
        }
    }

    async fn last_executed_sub_dag_index(&self) -> u64 {
        info!("Node {} last_executed_sub_dag_index() called", self.id);
        // TODO: get this info from storage somewhere
        // self.node.last_executed_sub_dag_index().await
        0
    }
}

fn read_network_keypair_from_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Ed25519KeyPair> {
    let contents = std::fs::read_to_string(path)?;
    let bytes = Base64::decode(contents.as_str()).map_err(|e| anyhow!("{}", e.to_string()))?;
    Ed25519KeyPair::from_bytes(bytes.get(1..).unwrap()).map_err(|e| anyhow!(e))
}

fn read_authority_keypair_from_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<BLS12381KeyPair> {
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
        let message = Message::deserialize(bytes)?;

        let unconfirmed_transaction = if let Message::UnconfirmedTransaction(unconfirmed_transaction) = message {
            unconfirmed_transaction
        } else {
            bail!("[UnconfirmedTransaction] Expected Message::UnconfirmedTransaction, got {:?}", message.name());
        };

        let transaction = match unconfirmed_transaction.transaction.deserialize_blocking() {
            Ok(transaction) => transaction,
            Err(error) => bail!("[UnconfirmedTransaction] {error}"),
        };

        self.0.check_transaction_basic(&transaction)?;

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
