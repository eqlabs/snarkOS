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

use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use config::{Committee, Import, Parameters, WorkerCache};
use crypto::NetworkKeyPair;
use executor::ExecutionState;
use eyre::Context;
use fastcrypto::{
    bls12381::min_sig::BLS12381KeyPair,
    ed25519::Ed25519KeyPair,
    encoding::{Base64, Encoding},
    traits::{EncodeDecodeBase64, KeyPair, ToFromBytes},
};
use mysten_metrics::RegistryService;
use node::{primary_node::PrimaryNode, worker_node::WorkerNode, NodeStorage};
use prometheus::Registry;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info};
use types::ConsensusOutput;
use worker::TrivialTransactionValidator;

pub struct BftConsensus {
    id: u32,
    primary_keypair: BLS12381KeyPair,
    network_keypair: NetworkKeyPair,
    worker_keypair: NetworkKeyPair,
    parameters: Parameters,
    p_store: NodeStorage,
    w_store: NodeStorage,
    committee: Arc<ArcSwap<Committee>>,
    worker_cache: Arc<ArcSwap<WorkerCache>>,
}

#[derive(Error, Debug)]
pub enum BftError {
    #[error("Error in BFT: {0}")]
    EyreReport(String),
}

impl BftConsensus {
    pub fn new(id: u32) -> Result<Self> {
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
                Arc::new(MyExecutionState::new(self.id)),
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
                TrivialTransactionValidator::default(), // TODO: we probably want to do better than just accepting
                None,
            )
            .await?;
        info!("created worker id {}", self.id);

        Ok((primary, worker))
    }
}

pub struct MyExecutionState {
    id: u32,
}

impl MyExecutionState {
    pub(crate) fn new(id: u32) -> Self {
        Self { id }
    }
}

#[async_trait]
impl ExecutionState for MyExecutionState {
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
            // TODO: get the output to snarkOS
            // self.node.save_consensus(consensus_output).await;
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
