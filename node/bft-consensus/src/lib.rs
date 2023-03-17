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

mod setup;
mod state;
mod validation;

use setup::*;
use state::*;
use validation::*;

use anyhow::Result;
use arc_swap::ArcSwap;
use fastcrypto::{bls12381::min_sig::BLS12381KeyPair, traits::KeyPair};
use narwhal_config::{Committee, Import, Parameters, WorkerCache};
use narwhal_crypto::NetworkKeyPair;
use narwhal_node::{primary_node::PrimaryNode, worker_node::WorkerNode, NodeStorage};
use std::sync::Arc;
use tracing::*;

use snarkos_node_consensus::Consensus as AleoConsensus;
use snarkos_node_router::Router;
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

impl<N: Network, C: ConsensusStorage<N>> BftConsensus<N, C> {
    pub fn new(aleo_consensus: AleoConsensus<N, C>, aleo_router: Router<N>, dev: Option<u16>) -> Result<Self> {
        // Offset here as the beacon is started on 0 and validators have their keys counted from 0
        // currently.
        let id = dev.expect("only dev mode is supported currently") - 1;
        let primary_key_file = format!("{}/committee/.primary-{id}-key.json", env!("CARGO_MANIFEST_DIR"));
        let primary_keypair =
            read_authority_keypair_from_file(primary_key_file).expect("Failed to load the node's primary keypair");
        let primary_network_key_file =
            format!("{}/committee/.primary-{id}-network-key.json", env!("CARGO_MANIFEST_DIR"));
        let network_keypair = read_network_keypair_from_file(primary_network_key_file)
            .expect("Failed to load the node's primary network keypair");
        let worker_key_file = format!("{}/committee/.worker-{id}-key.json", env!("CARGO_MANIFEST_DIR"));
        let worker_keypair =
            read_network_keypair_from_file(worker_key_file).expect("Failed to load the node's worker keypair");
        debug!("creating task {}", id);
        // Read the committee, workers and node's keypair from file.
        let committee_file = format!("{}/committee/.committee.json", env!("CARGO_MANIFEST_DIR"));
        let committee = Arc::new(ArcSwap::from_pointee(
            Committee::import(&committee_file).expect("Failed to load the committee information"),
        ));
        let workers_file = format!("{}/committee/.workers.json", env!("CARGO_MANIFEST_DIR"));
        let worker_cache = Arc::new(ArcSwap::from_pointee(
            WorkerCache::import(&workers_file).expect("Failed to load the worker information"),
        ));

        // Load default parameters if none are specified.
        let filename = format!("{}/committee/.parameters.json", env!("CARGO_MANIFEST_DIR"));
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
