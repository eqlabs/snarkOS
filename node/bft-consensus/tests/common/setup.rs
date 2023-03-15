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

use std::{collections::BTreeMap, sync::Arc};

use arc_swap::ArcSwap;
use fastcrypto::{bls12381::min_sig::BLS12381KeyPair, traits::KeyPair};
use multiaddr::Multiaddr;
use narwhal_config::{Authority, Committee, Parameters, WorkerCache, WorkerIndex, WorkerInfo};
use narwhal_crypto::NetworkKeyPair;
use narwhal_node::NodeStorage;
use rand::prelude::ThreadRng;
use tempfile::TempDir;

use crate::TestBftConsensus;

pub struct PrimarySetup {
    stake: u64,
    address: Multiaddr,
    keypair: BLS12381KeyPair,
    network_keypair: NetworkKeyPair,
    worker: WorkerSetup, // TODO: extend to multiple workers
}

impl PrimarySetup {
    pub fn new(stake: u64, rng: &mut ThreadRng) -> Self {
        Self {
            stake,
            address: "/ip4/127.0.0.1/udp/0".parse().unwrap(),
            keypair: BLS12381KeyPair::generate(rng),
            network_keypair: NetworkKeyPair::generate(rng),
            worker: WorkerSetup::new(rng),
        }
    }
}

pub struct WorkerSetup {
    address: Multiaddr,
    tx_address: Multiaddr,
    network_keypair: NetworkKeyPair,
}

impl WorkerSetup {
    fn new(rng: &mut ThreadRng) -> Self {
        Self {
            address: "/ip4/127.0.0.1/udp/0".parse().unwrap(),
            tx_address: "/ip4/127.0.0.1/tcp/0/http".parse().unwrap(),
            network_keypair: NetworkKeyPair::generate(rng),
        }
    }
}

pub struct CommitteeSetup {
    primaries: Vec<PrimarySetup>,
    epoch: u64,
    storage_dir: TempDir,
}

impl CommitteeSetup {
    pub fn new(primaries: Vec<PrimarySetup>, epoch: u64) -> Self {
        Self { primaries, epoch, storage_dir: TempDir::new().unwrap() }
    }

    pub fn generate_consensus_instances(&mut self) -> Vec<TestBftConsensus> {
        // Generate the Parameters.
        // TODO: tweak them further for test purposes?
        let mut parameters = Parameters::default();

        // These tweaks are necessary in order to avoid "address already in use" errors.
        parameters.network_admin_server.primary_network_admin_server_port = 0;
        parameters.network_admin_server.worker_network_admin_server_base_port = 0;

        // Generate the Committee.
        let mut authorities = BTreeMap::default();
        for primary in &self.primaries {
            let authority = Authority {
                stake: primary.stake,
                primary_address: primary.address.clone(),
                network_key: primary.network_keypair.public().clone(),
            };

            authorities.insert(primary.keypair.public().clone(), authority);
        }
        let committee = Arc::new(ArcSwap::from_pointee(Committee { authorities, epoch: self.epoch }));

        // Generate the WorkerCache.
        let mut workers = BTreeMap::default();
        // TODO: extend to multiple workers
        for primary in &self.primaries {
            let worker_info = WorkerInfo {
                name: primary.worker.network_keypair.public().clone(),
                transactions: primary.worker.tx_address.clone(),
                worker_address: primary.worker.address.clone(),
            };

            let mut worker_index = BTreeMap::default();
            worker_index.insert(0, worker_info);
            let worker_index = WorkerIndex(worker_index);

            workers.insert(primary.keypair.public().clone(), worker_index);
        }
        let worker_cache = Arc::new(ArcSwap::from_pointee(WorkerCache { epoch: self.epoch, workers }));

        // Create the consensus objects.
        let mut consensus_objects = Vec::with_capacity(self.primaries.len());
        for (primary_id, primary) in self.primaries.drain(..).enumerate() {
            // Prepare the storage.
            let base_path = self.storage_dir.path();

            let mut primary_store_path = base_path.to_owned();
            primary_store_path.push(format!("primary-{primary_id}"));
            let primary_store = NodeStorage::reopen(primary_store_path);

            let worker_id = 0; // TODO: extend to multiple workers
            let mut worker_store_path = base_path.to_owned();
            worker_store_path.push(format!("worker-{primary_id}-{worker_id}"));
            let worker_store = NodeStorage::reopen(worker_store_path);

            let consensus = TestBftConsensus {
                primary_id: primary_id as u8,
                primary_keypair: primary.keypair,
                network_keypair: primary.network_keypair,
                worker_keypair: primary.worker.network_keypair,
                parameters: parameters.clone(),
                primary_store,
                worker_store,
                committee: Arc::clone(&committee),
                worker_cache: Arc::clone(&worker_cache),
            };

            consensus_objects.push(consensus);
        }

        consensus_objects
    }
}
