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

use common::{generate_consensus_instances, TestBftExecutionState};
use snarkos_node_bft_consensus::setup::{CommitteeSetup, PrimarySetup};

// Makes sure that all the primaries have identical state after
// having processed a range of transactions using the consensus.
#[tokio::test(flavor = "multi_thread")]
async fn verify_state_coherence() {
    // Configure the primary-related variables.
    const NUM_PRIMARIES: usize = 5;
    const PRIMARY_STAKE: u64 = 1;

    // Configure the transactions.
    const NUM_TRANSACTIONS: usize = 100;

    // Prepare a source of randomness for key generation.
    let mut rng = thread_rng();

    // Generate the committee setup.
    let mut primaries = Vec::with_capacity(NUM_PRIMARIES);
    for _ in 0..NUM_PRIMARIES {
        let primary = PrimarySetup::new(None, PRIMARY_STAKE, vec![], &mut rng);
        primaries.push(primary);
    }
    let committee = CommitteeSetup::new(primaries, 0);

    // Prepare the initial state.
    let state = TestBftExecutionState::default();

    // Create the preconfigured consensus instances.
    let inert_consensus_instances = generate_consensus_instances(committee, state.clone());

    // Start the consensus instances.
    let mut running_consensus_instances = Vec::with_capacity(NUM_PRIMARIES);
    for instance in inert_consensus_instances {
        let running_instance = instance.start().await.unwrap();
        running_consensus_instances.push(running_instance);
    }

    // Create transaction clients; any instance can be used to do that.
    let mut tx_clients = running_consensus_instances[0].spawn_tx_clients();

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

// Ensures that a 4-member committee can survive a single member failure,
// and that it ceases to function with a single additional failure.
#[tokio::test(flavor = "multi_thread")]
async fn primary_failures() {
    // Configure the primary-related variables.
    const NUM_PRIMARIES: usize = 4;
    const WORKERS_PER_PRIMARY: u32 = 1;
    const PRIMARY_STAKE: u64 = 1;

    // Configure the transactions.
    const NUM_TRANSACTIONS: usize = 30;

    // Prepare a source of randomness for key generation.
    let mut rng = thread_rng();

    // Generate the committee setup.
    let mut primaries = Vec::with_capacity(NUM_PRIMARIES);
    for _ in 0..NUM_PRIMARIES {
        let primary = PrimarySetup::new(None, PRIMARY_STAKE, vec![], &mut rng);
        primaries.push(primary);
    }
    let committee = CommitteeSetup::new(primaries, 0);

    // Prepare the initial state.
    let state = TestBftExecutionState::default();

    // Create the preconfigured consensus instances.
    let inert_consensus_instances = generate_consensus_instances(committee, state.clone());

    // Start the consensus instances.
    let mut running_consensus_instances = Vec::with_capacity(NUM_PRIMARIES);
    for instance in inert_consensus_instances {
        let running_instance = instance.start().await.unwrap();
        running_consensus_instances.push(running_instance);
    }

    // Create transaction clients; any instance can be used to do that.
    let mut tx_clients = running_consensus_instances[0].spawn_tx_clients();

    // Use a deterministic Rng for transaction generation.
    let mut rng = TestRng::default();

    // Generate random transactions.
    let transfers = state.generate_random_transfers(NUM_TRANSACTIONS, &mut rng);

    // Send a third of the transactions to the workers.
    for transfer in &transfers[..10] {
        let transaction: Bytes = bincode::serialize(&transfer).unwrap().into();
        let tx = TransactionProto { transaction };

        // Submit the transaction to the chosen workers.
        for tx_client in &mut tx_clients {
            tx_client.submit_transaction(tx.clone()).await.unwrap();
        }
    }

    // Wait for a while to allow the transfers to be processed.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Save the current balances.
    let balances1 = running_consensus_instances.iter().map(|rci| rci.state.balances.lock().clone()).collect::<Vec<_>>();

    // Kill one of the consensus instances and shut down the corresponding transaction client.
    let instance_idx = rng.gen_range(0..NUM_PRIMARIES);
    let instance = running_consensus_instances.remove(instance_idx);

    instance.primary_node.shutdown().await;
    drop(instance);
    tx_clients.remove(instance_idx);

    // Send another third of the transactions to the workers.
    for transfer in &transfers[10..20] {
        let transaction: Bytes = bincode::serialize(&transfer).unwrap().into();
        let tx = TransactionProto { transaction };

        // Submit the transaction to the chosen workers.
        for tx_client in &mut tx_clients {
            tx_client.submit_transaction(tx.clone()).await.unwrap();
        }
    }

    // Wait for a while to allow the transfers to be processed.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Save the current balances.
    let balances2 = running_consensus_instances.iter().map(|rci| rci.state.balances.lock().clone()).collect::<Vec<_>>();

    // First check: the balances should have changed, as a single missing primary shouldn't break the consensus.
    assert_ne!(balances1[..NUM_PRIMARIES - 1], balances2);

    // Kill another one of the primaries and shut down the corresponding transaction client.
    let instance_idx = rng.gen_range(0..NUM_PRIMARIES - 1);
    let instance = running_consensus_instances.remove(instance_idx);
    // FIXME: this shouldn't need to happen in a separate task, but the await hangs otherwise.
    tokio::spawn(async move {
        instance.primary_node.shutdown().await;
    });
    tx_clients.remove(instance_idx);

    // Send another third of the transactions to the workers.
    for transfer in &transfers[20..] {
        let transaction: Bytes = bincode::serialize(&transfer).unwrap().into();
        let tx = TransactionProto { transaction };

        // Submit the transaction to the chosen workers.
        for tx_client in &mut tx_clients {
            tx_client.submit_transaction(tx.clone()).await.unwrap();
        }
    }

    // Wait for a while to allow the transfers to be processed.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Save the current balances.
    let balances3 = running_consensus_instances.iter().map(|rci| rci.state.balances.lock().clone()).collect::<Vec<_>>();

    // Final check: the balances should NOT have changed, as another missing primary should break the consensus.
    assert_eq!(balances2[..NUM_PRIMARIES - 2], balances3);
}
