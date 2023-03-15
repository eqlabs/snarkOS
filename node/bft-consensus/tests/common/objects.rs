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

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use arc_swap::ArcSwap;
use fastcrypto::{bls12381::min_sig::BLS12381KeyPair, traits::KeyPair};
use narwhal_config::{Committee, Import, Parameters, WorkerCache};
use narwhal_crypto::NetworkKeyPair;
use narwhal_node::{primary_node::PrimaryNode, worker_node::WorkerNode, NodeStorage};
use tempfile::TempDir;

use snarkos_node_bft_consensus::{read_authority_keypair_from_file, read_network_keypair_from_file};

use super::{state::TestBftExecutionState, validation::TestTransactionValidator};

pub struct TestBftConsensus {
    primary_id: u8,
    primary_keypair: BLS12381KeyPair,
    network_keypair: NetworkKeyPair,
    worker_keypair: NetworkKeyPair,
    parameters: Parameters,
    primary_store: NodeStorage,
    worker_store: NodeStorage,
    committee: Arc<ArcSwap<Committee>>,
    worker_cache: Arc<ArcSwap<WorkerCache>>,
    storage_dir: TempDir,
}

#[allow(dead_code)]
pub struct Member {
    primary_id: u8,
    primary_node: PrimaryNode,
    worker_node: WorkerNode,
    storage_dir: TempDir,
}

fn primary_dir(base_path: &Path, primary_id: u8) -> PathBuf {
    let mut path = base_path.to_owned();

    path.push(".bft");
    path.push(format!("primary-{primary_id}"));

    path
}

fn worker_dir(base_path: &Path, primary_id: u8, worker_id: u8) -> PathBuf {
    let mut path = base_path.to_owned();

    path.push(".bft");
    path.push(format!("worker-{primary_id}-{worker_id}"));

    path
}

impl TestBftConsensus {
    pub fn new(primary_id: u8) -> Result<Self> {
        let primary_key_file = format!("{}/.primary-{primary_id}-key.json", env!("CARGO_MANIFEST_DIR"));
        let primary_keypair = read_authority_keypair_from_file(primary_key_file).unwrap();
        let primary_network_key_file = format!("{}/.primary-{primary_id}-network-key.json", env!("CARGO_MANIFEST_DIR"));
        let network_keypair = read_network_keypair_from_file(primary_network_key_file).unwrap();
        let worker_key_file = format!("{}/.worker-{primary_id}-key.json", env!("CARGO_MANIFEST_DIR"));
        let worker_keypair = read_network_keypair_from_file(worker_key_file).unwrap();
        let committee_file = format!("{}/.committee.json", env!("CARGO_MANIFEST_DIR"));
        let committee = Arc::new(ArcSwap::from_pointee(Committee::import(&committee_file).unwrap()));
        let workers_file = format!("{}/.workers.json", env!("CARGO_MANIFEST_DIR"));
        let worker_cache = Arc::new(ArcSwap::from_pointee(WorkerCache::import(&workers_file).unwrap()));

        let filename = format!("{}/.parameters.json", env!("CARGO_MANIFEST_DIR"));
        let parameters = Parameters::import(&filename).unwrap();

        let storage_dir = TempDir::new().unwrap();
        let base_path = storage_dir.path();

        let primary_store_path = primary_dir(base_path, primary_id);
        let primary_store = NodeStorage::reopen(primary_store_path);
        let worker_store_path = worker_dir(base_path, primary_id, 0);
        let worker_store = NodeStorage::reopen(worker_store_path);

        Ok(Self {
            primary_id,
            primary_keypair,
            network_keypair,
            worker_keypair,
            parameters,
            primary_store,
            worker_store,
            committee,
            worker_cache,
            storage_dir,
        })
    }

    pub async fn start(self) -> Result<Member> {
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

        let member = Member {
            primary_id: self.primary_id,
            primary_node: primary,
            worker_node: worker,
            storage_dir: self.storage_dir,
        };

        Ok(member)
    }
}
