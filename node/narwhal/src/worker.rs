// Copyright (C) 2019-2023 Aleo Systems Inc.
// This file is part of the snarkOS library.

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:
// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    event::{Event, TransmissionRequest, TransmissionResponse},
    helpers::{fmt_id, Pending, Ready, Storage, WorkerReceiver},
    Ledger,
    ProposedBatch,
    Transport,
    MAX_BATCH_DELAY,
    MAX_TRANSMISSIONS_PER_BATCH,
    MAX_WORKERS,
    WORKER_PING_INTERVAL,
};
use snarkvm::{
    console::prelude::*,
    ledger::narwhal::{Data, Transmission, TransmissionID},
    prelude::{
        block::Transaction,
        coinbase::{ProverSolution, PuzzleCommitment},
    },
};

use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use parking_lot::Mutex;
use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

#[derive(Clone)]
pub struct Worker<N: Network> {
    /// The worker ID.
    id: u8,
    /// The gateway.
    gateway: Arc<dyn Transport<N>>,
    /// The storage.
    storage: Storage<N>,
    /// The ledger service.
    ledger: Ledger<N>,
    /// The proposed batch.
    proposed_batch: Arc<ProposedBatch<N>>,
    /// The ready queue.
    ready: Ready<N>,
    /// The pending transmissions queue.
    pending: Arc<Pending<TransmissionID<N>, Transmission<N>>>,
    /// The spawned handles.
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl<N: Network> Worker<N> {
    /// Initializes a new worker instance.
    pub fn new(
        id: u8,
        gateway: Arc<dyn Transport<N>>,
        storage: Storage<N>,
        ledger: Ledger<N>,
        proposed_batch: Arc<ProposedBatch<N>>,
    ) -> Result<Self> {
        // Ensure the worker ID is valid.
        ensure!(id < MAX_WORKERS, "Invalid worker ID '{id}'");
        // Return the worker.
        Ok(Self {
            id,
            gateway,
            storage: storage.clone(),
            ledger,
            proposed_batch,
            ready: Ready::new(storage),
            pending: Default::default(),
            handles: Default::default(),
        })
    }

    /// Run the worker instance.
    pub fn run(&self, receiver: WorkerReceiver<N>) {
        info!("Starting worker instance {} of the memory pool...", self.id);
        // Start the worker handlers.
        self.start_handlers(receiver);
    }

    /// Returns the worker ID.
    pub const fn id(&self) -> u8 {
        self.id
    }
}

impl<N: Network> Worker<N> {
    /// Returns the number of transmissions in the ready queue.
    pub fn num_transmissions(&self) -> usize {
        self.ready.num_transmissions()
    }

    /// Returns the number of ratifications in the ready queue.
    pub fn num_ratifications(&self) -> usize {
        self.ready.num_ratifications()
    }

    /// Returns the number of solutions in the ready queue.
    pub fn num_solutions(&self) -> usize {
        self.ready.num_solutions()
    }

    /// Returns the number of transactions in the ready queue.
    pub fn num_transactions(&self) -> usize {
        self.ready.num_transactions()
    }
}

impl<N: Network> Worker<N> {
    /// Returns the transmission IDs in the ready queue.
    pub fn transmission_ids(&self) -> IndexSet<TransmissionID<N>> {
        self.ready.transmission_ids()
    }

    /// Returns the transmissions in the ready queue.
    pub fn transmissions(&self) -> IndexMap<TransmissionID<N>, Transmission<N>> {
        self.ready.transmissions()
    }

    /// Returns the solutions in the ready queue.
    pub fn solutions(&self) -> impl '_ + Iterator<Item = (PuzzleCommitment<N>, Data<ProverSolution<N>>)> {
        self.ready.solutions()
    }

    /// Returns the transactions in the ready queue.
    pub fn transactions(&self) -> impl '_ + Iterator<Item = (N::TransactionID, Data<Transaction<N>>)> {
        self.ready.transactions()
    }
}

impl<N: Network> Worker<N> {
    /// Returns `true` if the transmission ID exists in the ready queue, proposed batch, storage, or ledger.
    pub fn contains_transmission(&self, transmission_id: impl Into<TransmissionID<N>>) -> bool {
        let transmission_id = transmission_id.into();
        // Check if the transmission ID exists in the ready queue, proposed batch, storage, or ledger.
        self.ready.contains(transmission_id)
            || self.proposed_batch.read().as_ref().map_or(false, |p| p.contains_transmission(transmission_id))
            || self.storage.contains_transmission(transmission_id)
            || self.ledger.contains_transmission(&transmission_id).unwrap_or(false)
    }

    /// Returns the transmission if it exists in the ready queue, proposed batch, storage.
    ///
    /// Note: We explicitly forbid retrieving a transmission from the ledger, as transmissions
    /// in the ledger are not guaranteed to be invalid for the current batch.
    pub fn get_transmission(&self, transmission_id: TransmissionID<N>) -> Option<Transmission<N>> {
        // Check if the transmission ID exists in the ready queue.
        if let Some(transmission) = self.ready.get(transmission_id) {
            return Some(transmission);
        }
        // Check if the transmission ID exists in storage.
        if let Some(transmission) = self.storage.get_transmission(transmission_id) {
            return Some(transmission);
        }
        // Check if the transmission ID exists in the proposed batch.
        if let Some(transmission) =
            self.proposed_batch.read().as_ref().and_then(|p| p.get_transmission(transmission_id))
        {
            return Some(transmission.clone());
        }
        None
    }

    /// Returns the transmissions if it exists in the worker, or requests it from the specified peer.
    pub async fn get_or_fetch_transmission(
        &self,
        peer_ip: SocketAddr,
        transmission_id: TransmissionID<N>,
    ) -> Result<(TransmissionID<N>, Transmission<N>)> {
        // Attempt to get the transmission from the worker.
        if let Some(transmission) = self.get_transmission(transmission_id) {
            return Ok((transmission_id, transmission));
        }
        // Send a transmission request to the peer.
        let (candidate_id, transmission) = self.send_transmission_request(peer_ip, transmission_id).await?;
        // Ensure the transmission ID matches.
        ensure!(candidate_id == transmission_id, "Invalid transmission ID");
        // Return the transmission.
        Ok((transmission_id, transmission))
    }

    /// Removes the specified number of transmissions from the ready queue, and returns them.
    pub(crate) async fn take_candidates(
        &self,
        num_transmissions: usize,
    ) -> impl Iterator<Item = (TransmissionID<N>, Transmission<N>)> {
        // Iterate through the ready transmissions, and determine which should be retained.
        let keep = futures::stream::iter(self.ready.transmissions())
            .filter_map(|(id, transmission)| async move {
                // Check if the transmission has been stored already.
                if self.storage.contains_transmission(id) {
                    return None;
                }
                // Check if the proposed batch already contains the transmission.
                if let Some(proposed_batch) = self.proposed_batch.read().as_ref() {
                    if proposed_batch.contains_transmission(id) {
                        return None;
                    }
                }
                // Check if the ledger already contains the transmission.
                if self.ledger.contains_transmission(&id).unwrap_or(false) {
                    return None;
                }
                // If the transmission is a solution, ensure the solution is still valid.
                if let (TransmissionID::Solution(commitment), Transmission::Solution(solution)) = (id, transmission) {
                    // Check if the solution is still valid.
                    if self.ledger.check_solution_basic(commitment, solution).await.is_err() {
                        return None;
                    }
                }
                Some(id)
            })
            .collect::<IndexSet<_>>()
            .await;

        // Retain the transmissions that are not in the storage or ledger.
        self.ready.retain(|id, _| keep.contains(id));
        // Remove the specified number of transmissions from the ready queue.
        self.ready.take(num_transmissions).into_iter()
    }

    /// Reinserts the specified transmission into the ready queue.
    pub(crate) fn reinsert(&self, transmission_id: TransmissionID<N>, transmission: Transmission<N>) -> bool {
        // Check if the transmission ID exists.
        if !self.contains_transmission(transmission_id) {
            // Insert the transmission into the ready queue.
            return self.ready.insert(transmission_id, transmission);
        }
        false
    }
}

impl<N: Network> Worker<N> {
    /// Handles the incoming transmission ID from a worker ping event.
    async fn process_transmission_id_from_ping(
        &self,
        peer_ip: SocketAddr,
        transmission_id: TransmissionID<N>,
    ) -> Result<()> {
        // Check if the transmission ID exists.
        if self.contains_transmission(transmission_id) {
            return Ok(());
        }
        // If the ready queue is full, then skip this transmission.
        // Note: We must prioritize the unconfirmed solutions and unconfirmed transactions, not transmissions.
        if self.ready.num_transmissions() > MAX_TRANSMISSIONS_PER_BATCH {
            return Ok(());
        }
        trace!("Worker {} - Found a new transmission ID '{}' from peer '{peer_ip}'", self.id, fmt_id(transmission_id));
        // Send an transmission request to the peer.
        let (candidate_id, transmission) = self.send_transmission_request(peer_ip, transmission_id).await?;
        // Ensure the transmission ID matches.
        ensure!(candidate_id == transmission_id, "Invalid transmission ID");
        // Insert the transmission into the ready queue.
        self.process_transmission_from_peer(peer_ip, transmission_id, transmission);
        Ok(())
    }

    /// Handles the incoming transmission from a peer.
    pub(crate) fn process_transmission_from_peer(
        &self,
        peer_ip: SocketAddr,
        transmission_id: TransmissionID<N>,
        transmission: Transmission<N>,
    ) {
        // Check if the transmission ID exists.
        if !self.contains_transmission(transmission_id) {
            // Insert the transmission into the ready queue.
            self.ready.insert(transmission_id, transmission);
            trace!("Worker {} - Added transmission '{}' from peer '{peer_ip}'", self.id, fmt_id(transmission_id));
        }
    }

    /// Handles the incoming unconfirmed solution.
    /// Note: This method assumes the incoming solution is valid and does not exist in the ledger.
    pub(crate) async fn process_unconfirmed_solution(
        &self,
        puzzle_commitment: PuzzleCommitment<N>,
        prover_solution: Data<ProverSolution<N>>,
    ) -> Result<()> {
        // Construct the transmission.
        let transmission = Transmission::Solution(prover_solution.clone());
        // Remove the puzzle commitment from the pending queue.
        self.pending.remove(puzzle_commitment, Some(&transmission));

        // Check if the solution exists.
        if self.contains_transmission(puzzle_commitment) {
            bail!("Solution '{}' already exists.", fmt_id(puzzle_commitment));
        }
        // Check that the solution is well-formed and unique.
        if let Err(e) = self.ledger.check_solution_basic(puzzle_commitment, prover_solution).await {
            bail!("Invalid unconfirmed solution '{}': {e}", fmt_id(puzzle_commitment));
        }
        // Adds the prover solution to the ready queue.
        self.ready.insert(puzzle_commitment, transmission);
        trace!("Worker {} - Added unconfirmed solution '{}'", self.id, fmt_id(puzzle_commitment));
        Ok(())
    }

    /// Handles the incoming unconfirmed transaction.
    pub(crate) async fn process_unconfirmed_transaction(
        &self,
        transaction_id: N::TransactionID,
        transaction: Data<Transaction<N>>,
    ) -> Result<()> {
        // Construct the transmission.
        let transmission = Transmission::Transaction(transaction.clone());
        // Remove the transaction from the pending queue.
        self.pending.remove(&transaction_id, Some(&transmission));

        // Check if the transaction ID exists.
        if self.contains_transmission(&transaction_id) {
            bail!("Transaction '{}' already exists.", fmt_id(transaction_id));
        }
        // Check that the transaction is well-formed and unique.
        if let Err(e) = self.ledger.check_transaction_basic(transaction_id, transaction).await {
            bail!("Invalid unconfirmed transaction '{}': {e}", fmt_id(transaction_id));
        }
        // Adds the transaction to the ready queue.
        self.ready.insert(&transaction_id, transmission);
        trace!("Worker {} - Added unconfirmed transaction '{}'", self.id, fmt_id(transaction_id));
        Ok(())
    }
}

impl<N: Network> Worker<N> {
    /// Starts the worker handlers.
    fn start_handlers(&self, receiver: WorkerReceiver<N>) {
        let WorkerReceiver { mut rx_worker_ping, mut rx_transmission_request, mut rx_transmission_response } = receiver;

        // Broadcast a ping event periodically.
        let self_ = self.clone();
        self.spawn(async move {
            loop {
                // Broadcast the ping event.
                self_.broadcast_ping();
                // Wait for the next interval.
                tokio::time::sleep(Duration::from_millis(WORKER_PING_INTERVAL)).await;
            }
        });

        // Process the ping events.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((peer_ip, transmission_id)) = rx_worker_ping.recv().await {
                if let Err(e) = self_.process_transmission_id_from_ping(peer_ip, transmission_id).await {
                    warn!(
                        "Worker {} failed to fetch missing transmission '{}' from peer '{peer_ip}': {e}",
                        self_.id,
                        fmt_id(transmission_id)
                    );
                }
            }
        });

        // Process the transmission requests.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((peer_ip, transmission_request)) = rx_transmission_request.recv().await {
                self_.send_transmission_response(peer_ip, transmission_request);
            }
        });

        // Process the transmission responses.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((peer_ip, transmission_response)) = rx_transmission_response.recv().await {
                // Process the transmission response.
                self_.finish_transmission_request(peer_ip, transmission_response);
            }
        });
    }

    /// Broadcasts a ping event.
    fn broadcast_ping(&self) {
        // Broadcast the ping event.
        self.gateway.broadcast(Event::WorkerPing(
            self.ready.transmission_ids().into_iter().take(MAX_TRANSMISSIONS_PER_BATCH).collect::<IndexSet<_>>().into(),
        ));
    }

    /// Sends an transmission request to the specified peer.
    async fn send_transmission_request(
        &self,
        peer_ip: SocketAddr,
        transmission_id: TransmissionID<N>,
    ) -> Result<(TransmissionID<N>, Transmission<N>)> {
        // Initialize a oneshot channel.
        let (callback_sender, callback_receiver) = oneshot::channel();
        // Insert the transmission ID into the pending queue.
        self.pending.insert(transmission_id, peer_ip, Some(callback_sender));
        // Send the transmission request to the peer.
        self.gateway.send(peer_ip, Event::TransmissionRequest(transmission_id.into()));
        // Wait for the transmission to be fetched.
        match timeout(Duration::from_millis(MAX_BATCH_DELAY), callback_receiver).await {
            // If the transmission was fetched, return it.
            Ok(result) => Ok((transmission_id, result?)),
            // If the transmission was not fetched, return an error.
            Err(e) => bail!("Unable to fetch transmission - (timeout) {e}"),
        }
    }

    /// Handles the incoming transmission response.
    /// This method ensures the transmission response is well-formed and matches the transmission ID.
    fn finish_transmission_request(&self, peer_ip: SocketAddr, response: TransmissionResponse<N>) {
        let TransmissionResponse { transmission_id, transmission } = response;
        // Check if the peer IP exists in the pending queue for the given transmission ID.
        let exists = self.pending.get(transmission_id).unwrap_or_default().contains(&peer_ip);
        // If the peer IP exists, finish the pending request.
        if exists {
            // TODO: Validate the transmission.
            // TODO (howardwu): Deserialize the transmission, and ensure it matches the transmission ID.
            //  Note: This is difficult for testing and example purposes, since those transmissions are fake.
            // Remove the transmission ID from the pending queue.
            self.pending.remove(transmission_id, Some(&transmission));
        }
    }

    /// Sends the requested transmission to the specified peer.
    fn send_transmission_response(&self, peer_ip: SocketAddr, request: TransmissionRequest<N>) {
        let TransmissionRequest { transmission_id } = request;
        // Attempt to retrieve the transmission.
        if let Some(transmission) = self.get_transmission(transmission_id) {
            // Send the transmission response to the peer.
            self.gateway.send(peer_ip, Event::TransmissionResponse((transmission_id, transmission).into()));
        }
    }

    /// Spawns a task with the given future; it should only be used for long-running tasks.
    fn spawn<T: Future<Output = ()> + Send + 'static>(&self, future: T) {
        self.handles.lock().push(tokio::spawn(future));
    }

    /// Shuts down the worker.
    pub(crate) fn shut_down(&self) {
        trace!("Shutting down worker {}...", self.id);
        // Abort the tasks.
        self.handles.lock().iter().for_each(|handle| handle.abort());
    }
}

#[cfg(test)]
mod prop_tests {

    use super::*;

    use crate::Gateway;
    use snarkos_node_narwhal_ledger_service::MockLedgerService;
    use test_strategy::proptest;

    type CurrentNetwork = snarkvm::prelude::Testnet3;

    #[proptest]
    fn worker_initialization(
        #[strategy(0..MAX_WORKERS)] id: u8,
        gateway: Gateway<CurrentNetwork>,
        storage: Storage<CurrentNetwork>,
    ) {
        let ledger: Ledger<CurrentNetwork> = Arc::new(MockLedgerService::new());
        let worker = Worker::new(id, Arc::new(gateway), storage, ledger, Default::default()).unwrap();
        assert_eq!(worker.id(), id);
    }

    #[proptest]
    fn invalid_worker_id(
        #[strategy(MAX_WORKERS..)] id: u8,
        gateway: Gateway<CurrentNetwork>,
        storage: Storage<CurrentNetwork>,
    ) {
        let ledger: Ledger<CurrentNetwork> = Arc::new(MockLedgerService::new());
        let worker = Worker::new(id, Arc::new(gateway), storage, ledger, Default::default());
        // TODO once Worker implements Debug, simplify this with `unwrap_err`
        if let Err(error) = worker {
            assert_eq!(error.to_string(), format!("Invalid worker ID '{}'", id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bytes::Bytes;
    use mockall::mock;
    use snarkos_node_narwhal_ledger_service::LedgerService;
    use snarkvm::prelude::{narwhal::TransmissionID, Field, Network, Result};

    type CurrentNetwork = snarkvm::prelude::Testnet3;

    mock! {
        Gateway<N: Network> {}
        impl<N:Network> Transport<N> for Gateway<N> {
            fn broadcast(&self, event: Event<N>);
            fn send(&self, peer_ip: SocketAddr, event: Event<N>);
        }
    }

    mock! {
        Ledger<N: Network> {}
        #[async_trait]
        impl<N: Network> LedgerService<N> for Ledger<N> {
            fn contains_certificate(&self, certificate_id: &Field<N>) -> Result<bool>;
            fn contains_transmission(&self, transmission_id: &TransmissionID<N>) -> Result<bool>;
            async fn check_solution_basic(
                &self,
                puzzle_commitment: PuzzleCommitment<N>,
                solution: Data<ProverSolution<N>>,
            ) -> Result<()>;
            async fn check_transaction_basic(
                &self,
                transaction_id: N::TransactionID,
                transaction: Data<Transaction<N>>,
            ) -> Result<()>;
        }
    }

    #[tokio::test]
    async fn test_process_transmission() {
        let rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkos_node_narwhal_committee::test_helpers::sample_committee(rng);
        // Initialize the storage.
        let storage = Storage::<CurrentNetwork>::new(committee.clone(), 1);
        // Setup the mock gateway and ledger.
        let gateway = MockGateway::default();
        let mut mock_ledger = MockLedger::default();
        mock_ledger.expect_contains_transmission().returning(|_| Ok(false));
        mock_ledger.expect_check_solution_basic().returning(|_, _| Ok(()));
        let ledger: Ledger<CurrentNetwork> = Arc::new(mock_ledger);

        // Create the Worker.
        let worker = Worker::new(1, Arc::new(gateway), storage, ledger, Default::default()).unwrap();
        let data = |rng: &mut TestRng| Data::Buffer(Bytes::from((0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>()));
        let transmission_id = TransmissionID::Solution(PuzzleCommitment::from_g1_affine(rng.gen()));
        let peer_ip = SocketAddr::from(([127, 0, 0, 1], 1234));
        let transmission = Transmission::Solution(data(rng));

        // Process the transmission.
        worker.process_transmission_from_peer(peer_ip, transmission_id, transmission.clone());
        assert!(worker.contains_transmission(transmission_id));
        assert!(worker.ready.contains(transmission_id));
        assert_eq!(worker.get_transmission(transmission_id), Some(transmission));
        // Take the transmission from the ready set.
        let transmission: Vec<_> = worker.take_candidates(1).await.collect();
        assert_eq!(transmission.len(), 1);
        assert!(!worker.ready.contains(transmission_id));
    }

    #[tokio::test]
    async fn test_send_transmission() {
        let rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkos_node_narwhal_committee::test_helpers::sample_committee(rng);
        // Initialize the storage.
        let storage = Storage::<CurrentNetwork>::new(committee.clone(), 1);
        // Setup the mock gateway and ledger.
        let mut gateway = MockGateway::default();
        gateway.expect_send().returning(|_, _| ());
        let ledger: Ledger<CurrentNetwork> = Arc::new(MockLedger::new());

        // Create the Worker.
        let worker = Worker::new(1, Arc::new(gateway), storage, ledger, Default::default()).unwrap();
        let transmission_id = TransmissionID::Solution(PuzzleCommitment::from_g1_affine(rng.gen()));
        let worker_ = worker.clone();
        let peer_ip = SocketAddr::from(([127, 0, 0, 1], 1234));
        let _ = worker_.send_transmission_request(peer_ip, transmission_id).await;
        assert!(worker.pending.contains(transmission_id));
        let peer_ip = SocketAddr::from(([127, 0, 0, 1], 1234));
        // Fake the transmission response.
        worker.finish_transmission_request(peer_ip, TransmissionResponse {
            transmission_id,
            transmission: Transmission::Solution(Data::Buffer(Bytes::from(vec![0; 512]))),
        });
        // Check the transmission was removed from the pending set.
        assert!(!worker.pending.contains(transmission_id));
    }

    #[tokio::test]
    async fn test_process_solution_ok() {
        let rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkos_node_narwhal_committee::test_helpers::sample_committee(rng);
        // Initialize the storage.
        let storage = Storage::<CurrentNetwork>::new(committee.clone(), 1);
        // Setup the mock gateway and ledger.
        let mut gateway = MockGateway::default();
        gateway.expect_send().returning(|_, _| ());
        let mut mock_ledger = MockLedger::default();
        mock_ledger.expect_contains_transmission().returning(|_| Ok(false));
        mock_ledger.expect_check_solution_basic().returning(|_, _| Ok(()));
        let ledger: Ledger<CurrentNetwork> = Arc::new(mock_ledger);

        // Create the Worker.
        let worker = Worker::new(1, Arc::new(gateway), storage, ledger, Default::default()).unwrap();
        let puzzle = PuzzleCommitment::from_g1_affine(rng.gen());
        let transmission_id = TransmissionID::Solution(puzzle);
        let worker_ = worker.clone();
        let peer_ip = SocketAddr::from(([127, 0, 0, 1], 1234));
        let _ = worker_.send_transmission_request(peer_ip, transmission_id).await;
        assert!(worker.pending.contains(transmission_id));
        let result = worker
            .process_unconfirmed_solution(
                puzzle,
                Data::Buffer(Bytes::from((0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>())),
            )
            .await;
        assert!(result.is_ok());
        assert!(!worker.pending.contains(transmission_id));
        assert!(worker.ready.contains(puzzle));
    }

    #[tokio::test]
    async fn test_process_solution_nok() {
        let rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkos_node_narwhal_committee::test_helpers::sample_committee(rng);
        // Initialize the storage.
        let storage = Storage::<CurrentNetwork>::new(committee.clone(), 1);
        // Setup the mock gateway and ledger.
        let mut gateway = MockGateway::default();
        gateway.expect_send().returning(|_, _| ());
        let mut mock_ledger = MockLedger::default();
        mock_ledger.expect_contains_transmission().returning(|_| Ok(false));
        mock_ledger.expect_check_solution_basic().returning(|_, _| Err(anyhow!("")));
        let ledger: Ledger<CurrentNetwork> = Arc::new(mock_ledger);

        // Create the Worker.
        let worker = Worker::new(1, Arc::new(gateway), storage, ledger, Default::default()).unwrap();
        let puzzle = PuzzleCommitment::from_g1_affine(rng.gen());
        let transmission_id = TransmissionID::Solution(puzzle);
        let worker_ = worker.clone();
        let peer_ip = SocketAddr::from(([127, 0, 0, 1], 1234));
        let _ = worker_.send_transmission_request(peer_ip, transmission_id).await;
        assert!(worker.pending.contains(transmission_id));
        let result = worker
            .process_unconfirmed_solution(
                puzzle,
                Data::Buffer(Bytes::from((0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>())),
            )
            .await;
        assert!(result.is_err());
        assert!(!worker.pending.contains(puzzle));
        assert!(!worker.ready.contains(puzzle));
    }

    #[tokio::test]
    async fn test_process_transaction_ok() {
        let mut rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkos_node_narwhal_committee::test_helpers::sample_committee(rng);
        // Initialize the storage.
        let storage = Storage::<CurrentNetwork>::new(committee.clone(), 1);
        // Setup the mock gateway and ledger.
        let mut gateway = MockGateway::default();
        gateway.expect_send().returning(|_, _| ());
        let mut mock_ledger = MockLedger::default();
        mock_ledger.expect_contains_transmission().returning(|_| Ok(false));
        mock_ledger.expect_check_transaction_basic().returning(|_, _| Ok(()));
        let ledger: Ledger<CurrentNetwork> = Arc::new(mock_ledger);

        // Create the Worker.
        let worker = Worker::new(1, Arc::new(gateway), storage, ledger, Default::default()).unwrap();
        let transaction_id: <CurrentNetwork as Network>::TransactionID = Field::<CurrentNetwork>::rand(&mut rng).into();
        let transmission_id = TransmissionID::Transaction(transaction_id);
        let worker_ = worker.clone();
        let peer_ip = SocketAddr::from(([127, 0, 0, 1], 1234));
        let _ = worker_.send_transmission_request(peer_ip, transmission_id).await;
        assert!(worker.pending.contains(transmission_id));
        let result = worker
            .process_unconfirmed_transaction(
                transaction_id,
                Data::Buffer(Bytes::from((0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>())),
            )
            .await;
        assert!(result.is_ok());
        assert!(!worker.pending.contains(transmission_id));
        assert!(worker.ready.contains(transmission_id));
    }

    #[tokio::test]
    async fn test_process_transaction_nok() {
        let mut rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkos_node_narwhal_committee::test_helpers::sample_committee(rng);
        // Initialize the storage.
        let storage = Storage::<CurrentNetwork>::new(committee.clone(), 1);
        // Setup the mock gateway and ledger.
        let mut gateway = MockGateway::default();
        gateway.expect_send().returning(|_, _| ());
        let mut mock_ledger = MockLedger::default();
        mock_ledger.expect_contains_transmission().returning(|_| Ok(false));
        mock_ledger.expect_check_transaction_basic().returning(|_, _| Err(anyhow!("")));
        let ledger: Ledger<CurrentNetwork> = Arc::new(mock_ledger);

        // Create the Worker.
        let worker = Worker::new(1, Arc::new(gateway), storage, ledger, Default::default()).unwrap();
        let transaction_id: <CurrentNetwork as Network>::TransactionID = Field::<CurrentNetwork>::rand(&mut rng).into();
        let transmission_id = TransmissionID::Transaction(transaction_id);
        let worker_ = worker.clone();
        let peer_ip = SocketAddr::from(([127, 0, 0, 1], 1234));
        let _ = worker_.send_transmission_request(peer_ip, transmission_id).await;
        assert!(worker.pending.contains(transmission_id));
        let result = worker
            .process_unconfirmed_transaction(
                transaction_id,
                Data::Buffer(Bytes::from((0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>())),
            )
            .await;
        assert!(result.is_err());
        assert!(!worker.pending.contains(transmission_id));
        assert!(!worker.ready.contains(transmission_id));
    }
}
