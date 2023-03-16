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
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU16, Ordering},
        Arc,
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use fastcrypto::{bls12381::min_sig::BLS12381KeyPair, traits::KeyPair};
use multiaddr::{Multiaddr, Protocol};
use narwhal_config::{Authority, Committee, Parameters, WorkerCache, WorkerIndex, WorkerInfo};
use narwhal_crypto::NetworkKeyPair;
use narwhal_node::NodeStorage;
use narwhal_types::TransactionsClient;
use rand::prelude::ThreadRng;
use tempfile::TempDir;
use tonic::transport::Channel;
use tracing::*;

use crate::common::{InertConsensusInstance, TestBftExecutionState};

// The non-registered port range for primaries (27 slots).
const PRIMARY_FIRST_PORT: u16 = 1030;
const PRIMARY_LAST_PORT: u16 = 1057;

// The non-registered network port range for workers (27 slots).
const WORKER_FIRST_PORT_NET: u16 = 1242;
const WORKER_LAST_PORT_NET: u16 = 1269;

// The non-registered transaction port range for workers (53 slots).
const WORKER_FIRST_PORT_TX: u16 = 1360;
const WORKER_LAST_PORT_TX: u16 = 1413;

static PRIMARY_PORT_OFFSET: AtomicU16 = AtomicU16::new(0);
static WORKER_PORT_OFFSET_NET: AtomicU16 = AtomicU16::new(0);
static WORKER_PORT_OFFSET_TX: AtomicU16 = AtomicU16::new(0);

pub struct PrimarySetup {
    stake: u64,
    address: Multiaddr,
    keypair: BLS12381KeyPair,
    network_keypair: NetworkKeyPair,
    workers: Vec<WorkerSetup>,
}

impl PrimarySetup {
    pub fn new(stake: u64, num_workers: u32, rng: &mut ThreadRng) -> Self {
        if num_workers > 1 {
            panic!(
                "Running multiple workers on a single machine is currently unsupported;\
                    the bullshark-bft crate would need to be adjusted for that feature."
            );
        }

        let workers = (0..num_workers).map(|_| WorkerSetup::new(rng)).collect();

        let primary_port = PRIMARY_FIRST_PORT + PRIMARY_PORT_OFFSET.fetch_add(1, Ordering::SeqCst);
        if primary_port > PRIMARY_LAST_PORT {
            warn!("Primary port is running into registered range ({primary_port}).");
        }

        Self {
            stake,
            address: format!("/ip4/127.0.0.1/udp/{primary_port}").parse().unwrap(),
            keypair: BLS12381KeyPair::generate(rng),
            network_keypair: NetworkKeyPair::generate(rng),
            workers,
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
        let worker_port_net = WORKER_FIRST_PORT_NET + WORKER_PORT_OFFSET_NET.fetch_add(1, Ordering::SeqCst);
        if worker_port_net > WORKER_LAST_PORT_NET {
            warn!("Worker network port is running into registered range ({worker_port_net}).");
        }

        let worker_port_tx = WORKER_FIRST_PORT_TX + WORKER_PORT_OFFSET_TX.fetch_add(1, Ordering::SeqCst);
        if worker_port_tx > WORKER_LAST_PORT_TX {
            warn!("Worker transaction port is running into registered range ({worker_port_tx}).");
        }

        Self {
            address: format!("/ip4/127.0.0.1/udp/{worker_port_net}").parse().unwrap(),
            tx_address: format!("/ip4/127.0.0.1/tcp/{worker_port_tx}/http").parse().unwrap(),
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

    pub fn tx_clients(&self) -> Vec<TransactionsClient<Channel>> {
        let mut clients = Vec::with_capacity(self.primaries.iter().map(|p| p.workers.len()).sum());
        for primary in &self.primaries {
            for worker in &primary.workers {
                let tx_port = if let Protocol::Tcp(port) =
                    worker.tx_address.into_iter().find(|protocol| matches!(protocol, Protocol::Tcp(_))).unwrap()
                {
                    port
                } else {
                    unreachable!()
                };
                let tx_addr = format!("http://127.0.0.1:{tx_port}").into_bytes();
                let channel = Channel::from_shared(tx_addr).unwrap().connect_lazy();
                let client = TransactionsClient::new(channel);
                clients.push(client);
            }
        }
        clients
    }

    pub fn generate_consensus_instances(&mut self, state: TestBftExecutionState) -> Vec<InertConsensusInstance> {
        // Generate the Parameters.
        // TODO: tweak them further for test purposes?
        let mut parameters = Parameters::default();

        // These tweaks are necessary in order to avoid "address already in use" errors.
        parameters.network_admin_server.primary_network_admin_server_port = 0;
        parameters.network_admin_server.worker_network_admin_server_base_port = 0;

        // Tweaks that make log inspection a bit more practical etc.
        parameters.gc_depth = 100;
        parameters.max_header_num_of_batches = 50;
        parameters.min_header_delay = Duration::from_millis(500);
        parameters.max_header_delay = Duration::from_secs(2);

        debug!("Using the following consensus parameters: {:#?}", parameters);

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
        for primary in &self.primaries {
            let mut worker_index = BTreeMap::default();
            for (worker_id, worker) in primary.workers.iter().enumerate() {
                let worker_info = WorkerInfo {
                    name: worker.network_keypair.public().clone(),
                    transactions: worker.tx_address.clone(),
                    worker_address: worker.address.clone(),
                };

                worker_index.insert(worker_id as u32, worker_info);
            }
            let worker_index = WorkerIndex(worker_index);
            workers.insert(primary.keypair.public().clone(), worker_index);
        }
        let worker_cache = Arc::new(ArcSwap::from_pointee(WorkerCache { epoch: self.epoch, workers }));

        // Create the consensus objects.
        let mut consensus_objects = Vec::with_capacity(self.primaries.len());
        for (primary_id, primary) in self.primaries.drain(..).enumerate() {
            // Prepare the temporary folder for storage.
            let base_path = self.storage_dir.path();

            // Create the primary storage instance.
            let mut primary_store_path = base_path.to_owned();
            primary_store_path.push(format!("primary-{primary_id}"));
            let primary_store = NodeStorage::reopen(primary_store_path);

            // Create the worker storage instance(s).
            let mut worker_stores = Vec::with_capacity(primary.workers.len());
            for worker_id in 0..primary.workers.len() {
                let mut worker_store_path = base_path.to_owned();
                worker_store_path.push(format!("worker-{primary_id}-{worker_id}"));
                let worker_store = NodeStorage::reopen(worker_store_path);
                worker_stores.push(worker_store);
            }

            // Create the full consensus instance.
            let consensus = InertConsensusInstance {
                primary_keypair: primary.keypair,
                network_keypair: primary.network_keypair,
                worker_keypairs: primary.workers.into_iter().map(|w| w.network_keypair).collect(),
                parameters: parameters.clone(),
                primary_store,
                worker_stores,
                committee: Arc::clone(&committee),
                worker_cache: Arc::clone(&worker_cache),
                state: state.clone(),
            };

            consensus_objects.push(consensus);
        }

        consensus_objects
    }
}
