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

use std::sync::Arc;

use anyhow::Result;
use arc_swap::ArcSwap;
use fastcrypto::{bls12381::min_sig::BLS12381KeyPair, traits::KeyPair};
use narwhal_config::{Committee, Parameters, WorkerCache};
use narwhal_crypto::NetworkKeyPair;
use narwhal_node::{primary_node::PrimaryNode, worker_node::WorkerNode, NodeStorage};

use super::{TestBftExecutionState, TestTransactionValidator};

pub struct InertConsensusInstance {
    pub primary_keypair: BLS12381KeyPair,
    pub network_keypair: NetworkKeyPair,
    pub worker_keypairs: Vec<NetworkKeyPair>,
    pub parameters: Parameters,
    pub primary_store: NodeStorage,
    pub worker_stores: Vec<NodeStorage>,
    pub committee: Arc<ArcSwap<Committee>>,
    pub worker_cache: Arc<ArcSwap<WorkerCache>>,
    pub state: TestBftExecutionState,
}

impl InertConsensusInstance {
    pub async fn start(self) -> Result<RunningConsensusInstance> {
        let primary_pub = self.primary_keypair.public().clone();
        let primary_node = PrimaryNode::new(self.parameters.clone(), true);
        let state = Arc::new(self.state);

        // Start the primary.
        primary_node
            .start(
                self.primary_keypair,
                self.network_keypair,
                self.committee.clone(),
                self.worker_cache.clone(),
                &self.primary_store,
                Arc::clone(&state),
            )
            .await?;

        // Start the workers associated with the primary.
        let num_workers = self.worker_keypairs.len();
        let mut worker_nodes = Vec::with_capacity(num_workers);
        for (worker_id, worker_keypair) in self.worker_keypairs.into_iter().enumerate() {
            let worker = WorkerNode::new(worker_id as u32, self.parameters.clone());
            worker
                .start(
                    primary_pub.clone(),
                    worker_keypair,
                    self.committee.clone(),
                    self.worker_cache.clone(),
                    &self.worker_stores[worker_id],
                    TestTransactionValidator::default(),
                )
                .await?;

            worker_nodes.push(worker);
        }

        let instance = RunningConsensusInstance { primary_node, worker_nodes, state };

        Ok(instance)
    }
}

pub struct RunningConsensusInstance {
    pub primary_node: PrimaryNode,
    pub worker_nodes: Vec<WorkerNode>,
    pub state: Arc<TestBftExecutionState>,
}
