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

use std::time::Duration;

use bytes::Bytes;
use narwhal_types::TransactionProto;
use rand::prelude::{thread_rng, IteratorRandom, Rng};
use snarkvm::prelude::TestRng;

mod common;

use common::{CommitteeSetup, PrimarySetup, TestBftExecutionState};

#[tokio::test(flavor = "multi_thread")]
async fn verify_state_coherence() {
    // Configure the primary-related variables.
    const NUM_PRIMARIES: usize = 5;
    const WORKERS_PER_PRIMARY: u32 = 1;
    const PRIMARY_STAKE: u64 = 1;

    // Configure the transactions.
    const NUM_TRANSACTIONS: usize = 100;

    // Prepare a source of randomness for key generation.
    let mut rng = thread_rng();

    // Generate the committee setup.
    let mut primaries = Vec::with_capacity(NUM_PRIMARIES);
    for _ in 0..NUM_PRIMARIES {
        let primary = PrimarySetup::new(PRIMARY_STAKE, WORKERS_PER_PRIMARY, &mut rng);
        primaries.push(primary);
    }
    let mut committee = CommitteeSetup::new(primaries, 0);

    // Create transaction clients.
    let mut tx_clients = committee.tx_clients();

    // Prepare the initial state.
    let state = TestBftExecutionState::default();

    // Create and start the preconfigured consensus instances.
    let inert_consensus_instances = committee.generate_consensus_instances(state.clone());
    let mut running_consensus_instances = Vec::with_capacity(NUM_PRIMARIES);
    for instance in inert_consensus_instances {
        let running_instance = instance.start().await.unwrap();
        running_consensus_instances.push(running_instance);
    }

    // Use a deterministic Rng for transaction generation.
    let mut rng = TestRng::default();

    // Generate random transactions.
    let transfers = state.generate_random_transfers(NUM_TRANSACTIONS, &mut rng);

    // Send the transactions to a random number of BFT workers at a time.
    for transfer in transfers {
        // Randomize the number of worker recipients.
        let n_recipients: usize = rng.gen_range(1..=tx_clients.len());

        let transaction: Bytes = bincode::serialize(&transfer).unwrap().into();
        let tx = TransactionProto { transaction };

        // Submit the transaction to the chosen workers.
        for tx_client in tx_clients.iter_mut().choose_multiple(&mut rng, n_recipients) {
            tx_client.submit_transaction(tx.clone()).await.unwrap();
        }
    }

    // Wait for a while to allow the transfers to be processed.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check that all the states match.
    let first_state = &running_consensus_instances[0].state;
    for state in running_consensus_instances.iter().skip(1).map(|rci| &rci.state) {
        assert_eq!(first_state, state);
    }
}
