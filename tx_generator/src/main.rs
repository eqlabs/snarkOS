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

use std::{env, net::IpAddr, time::Duration};

use multiaddr::Protocol;
use narwhal_config::{Import, WorkerCache};
use narwhal_types::{TransactionProto, TransactionsClient};
use rand::prelude::IteratorRandom;
use snarkos_node_bft_consensus::setup::workspace_dir;
use snarkos_node_consensus::Consensus;
use snarkos_node_messages::{Data, Message, UnconfirmedTransaction};
use snarkvm::{
    console::{
        account::{Address, PrivateKey, ViewKey},
        network::{prelude::*, Testnet3},
        program::{Entry, Identifier, Literal, Plaintext, Value},
    },
    prelude::{Ledger, RecordsFilter, TestRng},
    synthesizer::{
        block::{Block, Transaction},
        program::Program,
        store::helpers::rocksdb::ConsensusDB,
    },
};
use tikv_jemallocator::Jemalloc;
use tokio::sync::mpsc;
use tonic::transport::Channel;
use tracing::*;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

type CurrentNetwork = Testnet3;
type CurrentLedger = Ledger<CurrentNetwork, ConsensusDB<CurrentNetwork>>;
type CurrentConsensus = Consensus<CurrentNetwork, ConsensusDB<CurrentNetwork>>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Simple runtime arguments.
    const EXPECTED_ARGS: [&str; 2] = ["create_ledger", "create_txs"];
    let mut args = env::args();
    if args.len() != 3 {
        panic!("Invalid runtime arguments! Expected 2, found {}", args.len() - 1);
    }
    args.next(); // Skip the binary name.

    // Retrieve the command.
    let arg = args.next().unwrap();

    // Retrieve the private key.
    let private_key = args.next().unwrap();

    // Prepare an Rng.
    let mut rng = TestRng::default();

    info!("Preparing an instance of consensus that can generate transactions.");

    // Initialize the beacon private key.
    let genesis_private_key = PrivateKey::<CurrentNetwork>::from_str(&private_key).unwrap();
    let genesis_view_key = ViewKey::try_from(&genesis_private_key).unwrap();
    let genesis_address = Address::try_from(&genesis_private_key).unwrap();
    // Initialize the genesis block.
    let genesis = Block::from_bytes_le(Testnet3::genesis_bytes()).unwrap();

    // Initialize the consensus to generate transactions.
    let ledger = CurrentLedger::load(genesis, None).unwrap();
    let consensus = CurrentConsensus::new(ledger, false).unwrap();

    // Create the initial block or start producing transactions.
    if arg == EXPECTED_ARGS[0] {
        info!("Preparing a block that will allow the production of transactions.");

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

        info!("The ledger containing a block facilitating test transactions is ready!");
    } else if arg == EXPECTED_ARGS[1] {
        // Read the workers file.
        let base_path = format!("{}/node/bft-consensus/committee/", workspace_dir());
        let workers_file = format!("{base_path}.workers.json");
        let worker_cache = WorkerCache::import(&workers_file).expect("Failed to load the worker information");

        // Start up gRPC tx sender channels.
        let mut tx_clients = spawn_tx_clients(worker_cache);

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
                if tx_client.submit_transaction(tx.clone()).await.is_err() {
                    warn!("Couldn't deliver a transaction to one of the workers");
                }
            }

            // Wait for a random amount of time before processing further transactions.
            let delay: u64 = rng.gen_range(0..2_000);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        // Wait indefinitely.
        std::future::pending::<()>().await;
    } else {
        panic!("Invalid runtime argument! Options: {:?}", EXPECTED_ARGS);
    }
}

fn spawn_tx_clients(worker_cache: WorkerCache) -> Vec<TransactionsClient<Channel>> {
    let mut tx_uris = Vec::with_capacity(worker_cache.workers.values().map(|worker_index| worker_index.0.len()).sum());
    for worker_set in worker_cache.workers.values() {
        for worker_info in worker_set.0.values() {
            // Construct an address usable by the tonic channel based on the worker's tx Multiaddr.
            let mut tx_ip = None;
            let mut tx_port = None;
            for component in &worker_info.transactions {
                match component {
                    Protocol::Ip4(ip) => tx_ip = Some(IpAddr::V4(ip)),
                    Protocol::Ip6(ip) => tx_ip = Some(IpAddr::V6(ip)),
                    Protocol::Tcp(port) => tx_port = Some(port),
                    _ => {} // TODO: do we expect other combinations?
                }
            }
            // TODO: these may be known in advance, but shouldn't be trusted when we switch to a dynamic committee
            let tx_ip = tx_ip.unwrap();
            let tx_port = tx_port.unwrap();

            let tx_uri = format!("http://{tx_ip}:{tx_port}");
            tx_uris.push(tx_uri);
        }
    }

    // Sort the channel URIs by port for greater determinism in local tests.
    tx_uris.sort_unstable();

    // Create tx channels.
    tx_uris
        .into_iter()
        .map(|uri| {
            let channel = Channel::from_shared(uri).unwrap().connect_lazy();
            TransactionsClient::new(channel)
        })
        .collect()
}
