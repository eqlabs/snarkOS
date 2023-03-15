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

use super::{state::TestBftExecutionState, validation::TestTransactionValidator};

pub struct TestBftConsensus {
    pub primary_id: u8,
    pub primary_keypair: BLS12381KeyPair,
    pub network_keypair: NetworkKeyPair,
    pub worker_keypair: NetworkKeyPair,
    pub parameters: Parameters,
    pub primary_store: NodeStorage,
    pub worker_store: NodeStorage,
    pub committee: Arc<ArcSwap<Committee>>,
    pub worker_cache: Arc<ArcSwap<WorkerCache>>,
}

#[allow(dead_code)]
pub struct RunningConsensusInstance {
    primary_id: u8,
    primary_node: PrimaryNode,
    worker_node: WorkerNode,
}

impl TestBftConsensus {
    pub async fn start(self) -> Result<RunningConsensusInstance> {
        let primary_pub = self.primary_keypair.public().clone();
        let primary = PrimaryNode::new(self.parameters.clone(), true);
        let bft_execution_state = TestBftExecutionState::default();

        primary
            .start(
                self.primary_keypair,
                self.network_keypair,
                self.committee.clone(),
                self.worker_cache.clone(),
                &self.primary_store,
                Arc::new(bft_execution_state),
            )
            .await?;

        let worker = WorkerNode::new(0, self.parameters.clone());
        worker
            .start(
                primary_pub,
                self.worker_keypair,
                self.committee.clone(),
                self.worker_cache,
                &self.worker_store,
                TestTransactionValidator::default(),
            )
            .await?;

        let instance =
            RunningConsensusInstance { primary_id: self.primary_id, primary_node: primary, worker_node: worker };

        Ok(instance)
    }
}
