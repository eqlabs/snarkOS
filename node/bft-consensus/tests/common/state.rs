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

use std::{collections::HashMap, fmt};

use async_trait::async_trait;
use narwhal_executor::ExecutionState;
use narwhal_types::ConsensusOutput;
use parking_lot::Mutex;
use rand::prelude::{IteratorRandom, Rng, SliceRandom};
use tracing::*;

use super::transaction::*;

pub type Address = String;
pub type Amount = u64;

pub struct TestBftExecutionState {
    pub balances: Mutex<HashMap<Address, Amount>>,
}

impl Clone for TestBftExecutionState {
    fn clone(&self) -> Self {
        Self { balances: Mutex::new(self.balances.lock().clone()) }
    }
}

impl PartialEq for TestBftExecutionState {
    fn eq(&self, other: &Self) -> bool {
        *self.balances.lock() == *other.balances.lock()
    }
}

impl Eq for TestBftExecutionState {}

impl fmt::Debug for TestBftExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &*self.balances.lock())
    }
}

impl Default for TestBftExecutionState {
    fn default() -> Self {
        let mut balances = HashMap::new();
        balances.insert("Alice".into(), 1_000_000);
        balances.insert("Bob".into(), 2_000_000);
        balances.insert("Chad".into(), 3_000_000);
        let balances = Mutex::new(balances);

        Self { balances }
    }
}

impl TestBftExecutionState {
    pub fn generate_random_transfers<T: Rng>(&self, num_transfers: usize, rng: &mut T) -> Vec<Transaction> {
        let balances = self.balances.lock();

        let mut transfers = Vec::with_capacity(num_transfers);
        for _ in 0..num_transfers {
            let mut sides = balances.keys().cloned().choose_multiple(rng, 2);
            sides.shuffle(rng);
            let amount = rng.gen_range(1..=MAX_TRANSFER_AMOUNT);

            let transfer = Transfer { from: sides.pop().unwrap(), to: sides.pop().unwrap(), amount };
            transfers.push(Transaction::Transfer(transfer));
        }

        transfers
    }

    fn process_transactions(&self, transactions: Vec<Transaction>) {
        let mut balances = self.balances.lock();

        for transaction in transactions {
            match transaction {
                Transaction::Transfer(Transfer { from, to, amount }) => {
                    if amount > MAX_TRANSFER_AMOUNT {
                        continue;
                    }

                    if !balances.contains_key(&from) || !balances.contains_key(&to) {
                        continue;
                    }

                    if let Some(from_balance) = balances.get_mut(&from) {
                        if amount > *from_balance {
                            continue;
                        } else {
                            *from_balance -= amount;
                        }
                    }

                    if let Some(to_balance) = balances.get_mut(&to) {
                        *to_balance += amount;
                    }
                }
            }
        }
    }
}

#[async_trait]
impl ExecutionState for TestBftExecutionState {
    async fn handle_consensus_output(&self, consensus_output: ConsensusOutput) {
        if consensus_output.batches.is_empty() {
            info!("There are no batches to process.");
            return;
        }

        let mut transactions = Vec::new();
        for batch in consensus_output.batches {
            for batch in batch.1 {
                for transaction in batch.transactions {
                    let transaction: Transaction = bincode::deserialize(&transaction).unwrap();
                    transactions.push(transaction);
                }
            }
        }

        self.process_transactions(transactions);
    }

    async fn last_executed_sub_dag_index(&self) -> u64 {
        0
    }
}
