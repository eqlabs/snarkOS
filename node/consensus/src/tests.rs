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

use std::{net::SocketAddr, time::Duration};

use crate::ConsensusMemory;
use snarkos_account::Account;
use snarkos_node::Validator;
use snarkos_node_ledger::{Ledger, RecordsFilter};
use snarkos_node_messages::{Data, Message, UnconfirmedTransaction};
use snarkvm::{
    console::{
        account::{Address, PrivateKey, ViewKey},
        network::{prelude::*, Testnet3},
        program::{Entry, Identifier, Literal, Plaintext, Value},
    },
    prelude::TestRng,
    synthesizer::{
        block::{Block, Transaction, Transactions},
        program::Program,
        store::ConsensusStore,
        vm::VM,
    },
};

use indexmap::IndexMap;
use narwhal_types::TransactionProto;
use rand::prelude::IteratorRandom;
use tokio::sync::mpsc;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_test::traced_test;

type CurrentNetwork = Testnet3;

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use crate::Consensus;
    use snarkvm::{
        console::{account::PrivateKey, network::Testnet3, program::Value},
        prelude::TestRng,
        synthesizer::{Block, ConsensusMemory},
    };

    use once_cell::sync::OnceCell;

    type CurrentNetwork = Testnet3;
    pub(crate) type CurrentLedger = Ledger<CurrentNetwork, ConsensusMemory<CurrentNetwork>>;
    pub(crate) type CurrentConsensus = Consensus<CurrentNetwork, ConsensusMemory<CurrentNetwork>>;

    pub(crate) fn sample_vm() -> VM<CurrentNetwork, ConsensusMemory<CurrentNetwork>> {
        VM::from(ConsensusStore::open(None).unwrap()).unwrap()
    }

    pub(crate) fn sample_genesis_private_key(rng: &mut TestRng) -> PrivateKey<CurrentNetwork> {
        static INSTANCE: OnceCell<PrivateKey<CurrentNetwork>> = OnceCell::new();
        *INSTANCE.get_or_init(|| {
            // Initialize a new caller.
            PrivateKey::<CurrentNetwork>::new(rng).unwrap()
        })
    }

    #[allow(dead_code)]
    pub(crate) fn sample_genesis_block(rng: &mut TestRng) -> Block<CurrentNetwork> {
        static INSTANCE: OnceCell<Block<CurrentNetwork>> = OnceCell::new();
        INSTANCE
            .get_or_init(|| {
                // Initialize the VM.
                let vm = crate::tests::test_helpers::sample_vm();
                // Initialize a new caller.
                let caller_private_key = PrivateKey::<CurrentNetwork>::new(rng).unwrap();
                // Return the block.
                Block::genesis(&vm, &caller_private_key, rng).unwrap()
            })
            .clone()
    }

    pub(crate) fn sample_genesis_block_with_private_key(
        rng: &mut TestRng,
        private_key: PrivateKey<CurrentNetwork>,
    ) -> Block<CurrentNetwork> {
        static INSTANCE: OnceCell<Block<CurrentNetwork>> = OnceCell::new();
        INSTANCE
            .get_or_init(|| {
                // Initialize the VM.
                let vm = crate::tests::test_helpers::sample_vm();
                // Return the block.
                Block::genesis(&vm, &private_key, rng).unwrap()
            })
            .clone()
    }

    pub(crate) fn sample_genesis_consensus(rng: &mut TestRng) -> CurrentConsensus {
        // Sample the genesis private key.
        let private_key = sample_genesis_private_key(rng);
        // Sample the genesis block.
        let genesis = sample_genesis_block_with_private_key(rng, private_key);

        // Initialize the ledger with the genesis block and the associated private key.
        let ledger = CurrentLedger::load(genesis.clone(), None).unwrap();
        assert_eq!(0, ledger.latest_height());
        assert_eq!(genesis.hash(), ledger.latest_hash());
        assert_eq!(genesis.round(), ledger.latest_round());
        assert_eq!(genesis, ledger.get_block(0).unwrap());

        CurrentConsensus::new(ledger, true).unwrap()
    }

    pub(crate) fn sample_program() -> Program<CurrentNetwork> {
        static INSTANCE: OnceCell<Program<CurrentNetwork>> = OnceCell::new();
        INSTANCE
            .get_or_init(|| {
                // Initialize a new program.
                Program::<CurrentNetwork>::from_str(
                    r"
program testing.aleo;

struct message:
    amount as u128;

record token:
    owner as address.private;
    gates as u64.private;
    amount as u64.private;

function compute:
    input r0 as message.private;
    input r1 as message.public;
    input r2 as message.private;
    input r3 as token.record;
    add r0.amount r1.amount into r4;
    cast r3.owner r3.gates r3.amount into r5 as token.record;
    output r4 as u128.public;
    output r5 as token.record;",
                )
                .unwrap()
            })
            .clone()
    }

    pub(crate) fn sample_deployment_transaction(rng: &mut TestRng) -> Transaction<CurrentNetwork> {
        static INSTANCE: OnceCell<Transaction<CurrentNetwork>> = OnceCell::new();
        INSTANCE
            .get_or_init(|| {
                // Initialize the program.
                let program = sample_program();

                // Initialize a new caller.
                let caller_private_key = crate::tests::test_helpers::sample_genesis_private_key(rng);
                let caller_view_key = ViewKey::try_from(&caller_private_key).unwrap();

                // Initialize the consensus.
                let consensus = crate::tests::test_helpers::sample_genesis_consensus(rng);

                // Fetch the unspent records.
                let microcredits = Identifier::from_str("microcredits").unwrap();
                let records = consensus
                    .ledger
                    .find_records(&caller_view_key, RecordsFilter::SlowUnspent(caller_private_key))
                    .unwrap()
                    .filter(|(_, record)| {
                        // TODO (raychu86): Find cleaner approach and check that the record is associated with the `credits.aleo` program
                        match record.data().get(&microcredits) {
                            Some(Entry::Private(Plaintext::Literal(Literal::U64(amount), _))) => !amount.is_zero(),
                            _ => false,
                        }
                    })
                    .collect::<indexmap::IndexMap<_, _>>();
                trace!("Unspent Records:\n{:#?}", records);

                // Prepare the additional fee.
                let credits = records.values().next().unwrap().clone();
                let additional_fee = (credits, 6466000);

                // Deploy.
                let transaction = Transaction::deploy(
                    consensus.ledger.vm(),
                    &caller_private_key,
                    &program,
                    additional_fee,
                    None,
                    rng,
                )
                .unwrap();
                // Verify.
                assert!(consensus.ledger.vm().verify_transaction(&transaction));
                // Return the transaction.
                transaction
            })
            .clone()
    }

    pub(crate) fn sample_execution_transaction(rng: &mut TestRng) -> Transaction<CurrentNetwork> {
        static INSTANCE: OnceCell<Transaction<CurrentNetwork>> = OnceCell::new();
        INSTANCE
            .get_or_init(|| {
                // Initialize a new caller.
                let caller_private_key = crate::tests::test_helpers::sample_genesis_private_key(rng);
                let caller_view_key = ViewKey::try_from(&caller_private_key).unwrap();
                let address = Address::try_from(&caller_private_key).unwrap();

                // Initialize the consensus.
                let consensus = crate::tests::test_helpers::sample_genesis_consensus(rng);

                // Fetch the unspent records.
                let microcredits = Identifier::from_str("microcredits").unwrap();
                let records = consensus
                    .ledger
                    .find_records(&caller_view_key, RecordsFilter::SlowUnspent(caller_private_key))
                    .unwrap()
                    .filter(|(_, record)| {
                        // TODO (raychu86): Find cleaner approach and check that the record is associated with the `credits.aleo` program
                        match record.data().get(&microcredits) {
                            Some(Entry::Private(Plaintext::Literal(Literal::U64(amount), _))) => !amount.is_zero(),
                            _ => false,
                        }
                    })
                    .collect::<indexmap::IndexMap<_, _>>();
                trace!("Unspent Records:\n{:#?}", records);
                // Select a record to spend.
                let record = records.values().next().unwrap().clone();

                // Retrieve the VM.
                let vm = consensus.ledger.vm();

                // Prepare the inputs.
                let inputs = [
                    Value::<CurrentNetwork>::from_str(&address.to_string()).unwrap(),
                    Value::<CurrentNetwork>::from_str("1u64").unwrap(),
                ]
                .into_iter();

                // Authorize.
                let authorization = vm.authorize(&caller_private_key, "credits.aleo", "mint", inputs, rng).unwrap();
                assert_eq!(authorization.len(), 1);

                // Execute the fee.
                let fee = Transaction::execute_fee(vm, &caller_private_key, record, 3000, None, rng).unwrap();

                // Execute.
                let transaction = Transaction::execute_authorization(vm, authorization, Some(fee), None, rng).unwrap();
                // Verify.
                assert!(vm.verify_transaction(&transaction));
                // Return the transaction.
                transaction
            })
            .clone()
    }

    pub(crate) fn start_logger(default_level: LevelFilter) {
        let filter = match EnvFilter::try_from_default_env() {
            Ok(filter) => filter
                .add_directive("anemo=off".parse().unwrap())
                .add_directive("tokio_util=off".parse().unwrap())
                .add_directive("narwhal_config=off".parse().unwrap())
                .add_directive("narwhal_consensus=off".parse().unwrap())
                .add_directive("narwhal_executor=off".parse().unwrap())
                .add_directive("narwhal_network=off".parse().unwrap())
                .add_directive("narwhal_primary=off".parse().unwrap())
                .add_directive("narwhal_worker=off".parse().unwrap()),
            _ => EnvFilter::default()
                .add_directive(default_level.into())
                .add_directive("anemo=off".parse().unwrap())
                .add_directive("tokio_util=off".parse().unwrap())
                .add_directive("narwhal_config=off".parse().unwrap())
                .add_directive("narwhal_consensus=off".parse().unwrap())
                .add_directive("narwhal_executor=off".parse().unwrap())
                .add_directive("narwhal_network=off".parse().unwrap())
                .add_directive("narwhal_primary=off".parse().unwrap())
                .add_directive("narwhal_worker=off".parse().unwrap()),
        };

        tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();
    }
}

#[test]
fn test_validators() {
    // Initialize an RNG.
    let rng = &mut TestRng::default();

    // Sample the private key, view key, and address.
    let private_key = PrivateKey::<CurrentNetwork>::new(rng).unwrap();
    let view_key = ViewKey::try_from(private_key).unwrap();
    let address = Address::try_from(&view_key).unwrap();

    // Initialize the VM.
    let vm = crate::tests::test_helpers::sample_vm();

    // Create a genesis block.
    let genesis = Block::genesis(&vm, &private_key, rng).unwrap();

    // Initialize the validators.
    let validators: IndexMap<Address<_>, ()> = [(address, ())].into_iter().collect();

    // Ensure the block is signed by an authorized validator.
    let signer = genesis.signature().to_address();
    if !validators.contains_key(&signer) {
        let validator = validators.iter().next().unwrap().0;
        eprintln!("{} {} {} {}", *validator, signer, *validator == signer, validators.contains_key(&signer));
        eprintln!(
            "Block {} ({}) is signed by an unauthorized validator ({})",
            genesis.height(),
            genesis.hash(),
            signer
        );
    }
    assert!(validators.contains_key(&signer));
}

#[test]
#[traced_test]
fn test_ledger_deploy() {
    let rng = &mut TestRng::default();

    // Sample the genesis private key.
    let private_key = crate::tests::test_helpers::sample_genesis_private_key(rng);
    // Sample the genesis consensus.
    let consensus = test_helpers::sample_genesis_consensus(rng);

    // Add a transaction to the memory pool.
    let transaction = crate::tests::test_helpers::sample_deployment_transaction(rng);
    consensus.add_unconfirmed_transaction(transaction.clone()).unwrap();

    // Propose the next block.
    let next_block = consensus.propose_next_block(&private_key, rng).unwrap();

    // Ensure the block is a valid next block.
    consensus.check_next_block(&next_block).unwrap();

    // Construct a next block.
    consensus.advance_to_next_block(&next_block).unwrap();
    assert_eq!(consensus.ledger.latest_height(), 1);
    assert_eq!(consensus.ledger.latest_hash(), next_block.hash());
    assert!(consensus.ledger.contains_transaction_id(&transaction.id()).unwrap());
    assert!(transaction.input_ids().count() > 0);
    assert!(consensus.ledger.contains_input_id(transaction.input_ids().next().unwrap()).unwrap());

    // Ensure that the VM can't re-deploy the same program.
    assert!(consensus.ledger.vm().finalize(&Transactions::from(&[transaction.clone()])).is_err());
    // Ensure that the ledger deems the same transaction invalid.
    assert!(consensus.check_transaction_basic(&transaction).is_err());
    // Ensure that the ledger cannot add the same transaction.
    assert!(consensus.add_unconfirmed_transaction(transaction).is_err());
}

#[test]
#[traced_test]
fn test_ledger_execute() {
    let rng = &mut TestRng::default();

    // Sample the genesis private key.
    let private_key = crate::tests::test_helpers::sample_genesis_private_key(rng);
    // Sample the genesis consensus.
    let consensus = test_helpers::sample_genesis_consensus(rng);

    // Add a transaction to the memory pool.
    let transaction = crate::tests::test_helpers::sample_execution_transaction(rng);
    consensus.add_unconfirmed_transaction(transaction.clone()).unwrap();

    // Propose the next block.
    let next_block = consensus.propose_next_block(&private_key, rng).unwrap();

    // Ensure the block is a valid next block.
    consensus.check_next_block(&next_block).unwrap();

    // Construct a next block.
    consensus.advance_to_next_block(&next_block).unwrap();
    assert_eq!(consensus.ledger.latest_height(), 1);
    assert_eq!(consensus.ledger.latest_hash(), next_block.hash());

    // Ensure that the ledger deems the same transaction invalid.
    assert!(consensus.check_transaction_basic(&transaction).is_err());
    // Ensure that the ledger cannot add the same transaction.
    assert!(consensus.add_unconfirmed_transaction(transaction).is_err());
}

#[test]
#[traced_test]
fn test_ledger_execute_many() {
    let rng = &mut TestRng::default();

    // Sample the genesis private key, view key, and address.
    let private_key = crate::tests::test_helpers::sample_genesis_private_key(rng);
    let view_key = ViewKey::try_from(private_key).unwrap();

    // Sample the genesis consensus.
    let consensus = crate::tests::test_helpers::sample_genesis_consensus(rng);

    // Track the number of starting records.
    let mut num_starting_records = 4;

    for height in 1..5 {
        // Fetch the unspent records.
        let microcredits = Identifier::from_str("microcredits").unwrap();
        let records: Vec<_> = consensus
            .ledger
            .find_records(&view_key, RecordsFilter::Unspent)
            .unwrap()
            .filter(|(_, record)| {
                // TODO (raychu86): Find cleaner approach and check that the record is associated with the `credits.aleo` program
                match record.data().get(&microcredits) {
                    Some(Entry::Private(Plaintext::Literal(Literal::U64(amount), _))) => !amount.is_zero(),
                    _ => false,
                }
            })
            .collect();
        assert_eq!(records.len(), num_starting_records);

        for ((_, record), (_, fee_record)) in records.iter().tuples() {
            // Prepare the inputs.
            let amount = match record.data().get(&Identifier::from_str("microcredits").unwrap()).unwrap() {
                Entry::Private(Plaintext::Literal(Literal::<CurrentNetwork>::U64(amount), _)) => amount,
                _ => unreachable!(),
            };
            let inputs = [Value::Record(record.clone()), Value::from_str(&format!("{}u64", **amount / 2)).unwrap()];
            // Create a new transaction.
            let transaction = Transaction::execute(
                consensus.ledger.vm(),
                &private_key,
                ("credits.aleo", "split"),
                inputs.iter(),
                Some((fee_record.clone(), 3000u64)),
                None,
                rng,
            )
            .unwrap();
            // Add the transaction to the memory pool.
            consensus.add_unconfirmed_transaction(transaction).unwrap();
        }
        assert_eq!(consensus.memory_pool().num_unconfirmed_transactions(), num_starting_records / 2);

        // Update the number of starting records
        num_starting_records = num_starting_records * 3 / 2;

        // Propose the next block.
        let next_block = consensus.propose_next_block(&private_key, rng).unwrap();

        // Ensure the block is a valid next block.
        consensus.check_next_block(&next_block).unwrap();
        // Construct a next block.
        consensus.advance_to_next_block(&next_block).unwrap();
        assert_eq!(consensus.ledger.latest_height(), height);
        assert_eq!(consensus.ledger.latest_hash(), next_block.hash());
    }
}

#[test]
#[traced_test]
fn test_proof_target() {
    let rng = &mut TestRng::default();

    // Sample the genesis private key and address.
    let private_key = crate::tests::test_helpers::sample_genesis_private_key(rng);
    let address = Address::try_from(&private_key).unwrap();

    // Sample the genesis consensus.
    let consensus = crate::tests::test_helpers::sample_genesis_consensus(rng);

    // Fetch the proof target and epoch challenge for the block.
    let proof_target = consensus.ledger.latest_proof_target();
    let epoch_challenge = consensus.ledger.latest_epoch_challenge().unwrap();

    for _ in 0..100 {
        // Generate a prover solution.
        let prover_solution = consensus.coinbase_puzzle.prove(&epoch_challenge, address, rng.gen(), None).unwrap();

        // Check that the prover solution meets the proof target requirement.
        if prover_solution.to_target().unwrap() >= proof_target {
            assert!(consensus.add_unconfirmed_solution(&prover_solution).is_ok())
        } else {
            assert!(consensus.add_unconfirmed_solution(&prover_solution).is_err())
        }

        // Generate a prover solution with a minimum proof target.
        let prover_solution = consensus.coinbase_puzzle.prove(&epoch_challenge, address, rng.gen(), Some(proof_target));

        // Check that the prover solution meets the proof target requirement.
        if let Ok(prover_solution) = prover_solution {
            assert!(prover_solution.to_target().unwrap() >= proof_target);
            assert!(consensus.add_unconfirmed_solution(&prover_solution).is_ok())
        }
    }
}

#[test]
#[traced_test]
fn test_coinbase_target() {
    let rng = &mut TestRng::default();

    // Sample the genesis private key and address.
    let private_key = crate::tests::test_helpers::sample_genesis_private_key(rng);
    let address = Address::try_from(&private_key).unwrap();

    // Sample the genesis consensus.
    let consensus = test_helpers::sample_genesis_consensus(rng);

    // Add a transaction to the memory pool.
    let transaction = crate::tests::test_helpers::sample_execution_transaction(rng);
    consensus.add_unconfirmed_transaction(transaction).unwrap();

    // Ensure that the ledger can't create a block that satisfies the coinbase target.
    let proposed_block = consensus.propose_next_block(&private_key, rng).unwrap();
    // Ensure the block does not contain a coinbase solution.
    assert!(proposed_block.coinbase().is_none());

    // Check that the ledger won't generate a block for a cumulative target that does not meet the requirements.
    let mut cumulative_target = 0u128;
    let epoch_challenge = consensus.ledger.latest_epoch_challenge().unwrap();

    while cumulative_target < consensus.ledger.latest_coinbase_target() as u128 {
        // Generate a prover solution.
        let prover_solution = match consensus.coinbase_puzzle.prove(
            &epoch_challenge,
            address,
            rng.gen(),
            Some(consensus.ledger.latest_proof_target()),
        ) {
            Ok(prover_solution) => prover_solution,
            Err(_) => continue,
        };

        // Try to add the prover solution to the memory pool.
        if consensus.add_unconfirmed_solution(&prover_solution).is_ok() {
            // Add to the cumulative target if the prover solution is valid.
            cumulative_target += prover_solution.to_target().unwrap() as u128;
        }
    }

    // Ensure that the ledger can create a block that satisfies the coinbase target.
    let proposed_block = consensus.propose_next_block(&private_key, rng).unwrap();
    // Ensure the block contains a coinbase solution.
    assert!(proposed_block.coinbase().is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "This test is intended to be run on-demand and in isolation."]
async fn test_bullshark_full() {
    // Start the logger.
    test_helpers::start_logger(LevelFilter::INFO);

    // TODO: introduce a Ctrl-C signal handler that will delete the temporary databases.

    // The number of validators to run.
    // TODO: support a different number than 4.
    const N_VALIDATORS: u16 = 4;

    // The randomly-seeded source of deterministic randomness.
    let mut rng = TestRng::default();

    // Sample the genesis private key.
    let genesis_private_key = test_helpers::sample_genesis_private_key(&mut rng);
    let genesis_view_key = ViewKey::try_from(&genesis_private_key).unwrap();
    let genesis_address = Address::try_from(&genesis_private_key).unwrap();

    // Sample the genesis block.
    let genesis = test_helpers::sample_genesis_block_with_private_key(&mut rng, genesis_private_key);

    // Collect the validator addresses.
    let mut validator_addrs = vec![];
    for i in 0..N_VALIDATORS {
        let addr: SocketAddr = format!("127.0.0.1:{}", 4130 + i).parse().unwrap();
        validator_addrs.push(addr);
    }

    // Start and collect the validator nodes.
    let mut validators = vec![];
    for (i, addr) in validator_addrs.iter().copied().enumerate() {
        info!("Staring validator {i} at {addr}.");

        let account = Account::<CurrentNetwork>::new(&mut rng).unwrap();
        let other_addrs = validator_addrs.iter().copied().filter(|&a| a != addr).collect::<Vec<_>>();
        let validator = Validator::<CurrentNetwork, ConsensusMemory<CurrentNetwork>>::new(
            addr,
            None,
            account,
            &other_addrs,    // the other validators are trusted peers
            genesis.clone(), // use a common genesis block
            None,
            Some(i as u16),
            i == 0, // enable metrics only for the first validator
        )
        .await
        .unwrap();
        validators.push(validator);

        info!("Validator {i} is ready.");
    }

    // Wait until the validators are connected to one another.
    // TODO: validators should do this automatically until quorum is reached
    loop {
        info!("Waiting for the validator mesh...");

        let mut mesh_ready = true;

        for validator in &validators {
            if validator.router().number_of_connected_peers() != N_VALIDATORS as usize - 1 {
                mesh_ready = false;
                break;
            }
        }

        if mesh_ready {
            info!("The validator mesh is ready.");
            break;
        } else {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    // Prepare the setup related to the BFT workers.
    let mut tx_clients = validators[0].bft().spawn_tx_clients();

    info!("Preparing a block that will allow the production of transactions.");

    // Initialize the consensus to generate transactions.
    let ledger = test_helpers::CurrentLedger::load(genesis, None).unwrap();
    let consensus = test_helpers::CurrentConsensus::new(ledger, true).unwrap();

    // Initialize a new program. This program is a simple program with a function `test` that does not require any
    // input records. This means you can sample as many execution transactions as you want without needing
    // to locate any owned records to spend.
    let program = Program::<CurrentNetwork>::from_str(
        r"
program simple.aleo;

function hello:
    input r0 as u32.private;
    input r1 as u32.private;
    add r0 r1 into r2;
    output r2 as u32.private;
",
    )
    .unwrap();

    // Fetch the unspent records.
    let microcredits = Identifier::from_str("microcredits").unwrap();
    let records: Vec<_> = consensus
        .ledger
        .find_records(&genesis_view_key, RecordsFilter::Unspent)
        .unwrap()
        .filter(|(_, record)| match record.data().get(&microcredits) {
            Some(Entry::Private(Plaintext::Literal(Literal::U64(amount), _))) => !amount.is_zero(),
            _ => false,
        })
        .collect();
    assert_eq!(records.len(), 4);

    let fee = 4000000;
    let (_, record) = records
        .iter()
        .find(|(_, r)| match r.data().get(&microcredits) {
            Some(Entry::Private(Plaintext::Literal(Literal::U64(amount), _))) => **amount >= fee,
            _ => false,
        })
        .unwrap();

    // Create a deployment transaction for the above program.
    let deployment_transaction = Transaction::deploy(
        consensus.ledger.vm(),
        &genesis_private_key,
        &program,
        (record.clone(), fee),
        None,
        &mut rng,
    )
    .unwrap();

    // Add the transaction to the memory pool.
    consensus.add_unconfirmed_transaction(deployment_transaction).unwrap();
    assert_eq!(consensus.memory_pool().num_unconfirmed_transactions(), 1);

    // Propose the next block.
    let next_block = consensus.propose_next_block(&genesis_private_key, &mut rng).unwrap();

    // Ensure the block is a valid next block.
    consensus.check_next_block(&next_block).unwrap();
    // Construct a next block.
    consensus.advance_to_next_block(&next_block).unwrap();

    // Introduce the block to all the validators.
    for validator in &validators {
        validator.consensus().check_next_block(&next_block).unwrap();
        validator.consensus().advance_to_next_block(&next_block).unwrap();
    }

    info!("Done; the validators are now ready to process transactions.");

    // From this point on, once the deployment transaction has been included in a block,
    // all executions of the `test` function in `sample.program` will be valid for any subsequent block.

    // Use a channel to be able to process transactions as they are created.
    let (tx_sender, mut tx_receiver) = mpsc::unbounded_channel();

    // Generate execution transactions in the background.
    tokio::task::spawn_blocking(move || {
        // TODO (raychu86): Update this bandaid workaround.
        //  Currently the `mint` function can be called without restriction if the recipient is an authorized `beacon`.
        //  Consensus rules will change later when staking and proper coinbase rewards are integrated, which will invalidate this approach.
        //  Note: A more proper way to approach this is to create `split` transactions and then start generating increasingly larger numbers of
        //  transactions, once more and more records are available to you in subsequent blocks.

        // Create inputs for the `credits.aleo/mint` call.
        let inputs = [Value::from_str(&genesis_address.to_string()).unwrap(), Value::from_str("1u64").unwrap()];

        for i in 0.. {
            let transaction = Transaction::execute(
                consensus.ledger.vm(),
                &genesis_private_key,
                ("credits.aleo", "mint"),
                inputs.iter(),
                None,
                None,
                &mut rng,
            )
            .unwrap();

            info!("Created transaction {} ({}/inf).", transaction.id(), i + 1);

            tx_sender.send(transaction).unwrap();
        }
    });

    // Note: These transactions do not have conflicting state, so they can be added in any order. However,
    // this means we can't test for conflicts or double spends using these transactions.

    // Create a new test rng for worker and delay randomization (the other one was moved to the transaction
    // creation task). This one doesn't need to be deterministic, it's just fast and readily available.
    let mut rng = TestRng::default();

    // Send the transactions to a random number of BFT workers.
    while let Some(transaction) = tx_receiver.recv().await {
        // Randomize the number of worker recipients.
        let n_recipients: usize = rng.gen_range(1..=4);

        info!("Sending transaction {} to {} workers.", transaction.id(), n_recipients);

        let message = Message::UnconfirmedTransaction(UnconfirmedTransaction {
            transaction_id: transaction.id(),
            transaction: Data::Object(transaction),
        });
        let mut bytes: Vec<u8> = Vec::new();
        message.serialize(&mut bytes).unwrap();
        let payload = bytes::Bytes::from(bytes);
        let tx = TransactionProto { transaction: payload };

        // Submit the transaction to the chosen workers.
        for tx_client in tx_clients.iter_mut().choose_multiple(&mut rng, n_recipients) {
            tx_client.submit_transaction(tx.clone()).await.unwrap();
        }

        // Wait for a random amount of time before processing further transactions.
        let delay: u64 = rng.gen_range(0..2_000);
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }

    // Wait indefinitely.
    std::future::pending::<()>().await;
}
