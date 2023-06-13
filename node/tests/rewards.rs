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

use std::{collections::HashMap, marker::PhantomData, time::SystemTime};

type Address = u64;

#[derive(Copy, Clone, Debug, PartialEq)]
struct Validator {
    stake: u64,
    address: Address,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Tx {
    fee_total: u64,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Proof {
    target: u64,
    prover: Address,
}

#[derive(Clone, Debug)]
struct Block<T> {
    height: u64,
    validators: Vec<Validator>,
    leader: Validator,
    ts: u64,
    // target_total: u64,
    // min_target_proof: u64,
    txs: Vec<Tx>,
    proofs: Vec<Proof>,
    phantom: PhantomData<T>,
}

const FIXED_POINT_DECIMALS: u64 = 10000;

trait NetworkConstants {
    const SUPPLY_GENESIS: u64 = 1_000_000_000_000_000;
    const ANCHOR_TIME: u64 = 20;
    fn new(validators: Vec<Validator>, leader: Validator) -> Self;
    fn height_year1() -> u64 {
        365 * 24 * 3600 / Self::ANCHOR_TIME
    }
    fn height_year10() -> u64 {
        Self::height_year1() * 10
    }

    fn reward_anchor() -> u64 {
        (2 * Self::SUPPLY_GENESIS) / (Self::height_year10() * (Self::height_year10() + 1))
    }

    fn reward_staking() -> u64 {
        25 * (Self::SUPPLY_GENESIS / Self::height_year1()) / 1000
    }
    fn genesis_block(self) -> Block<Self>
    where
        Self: Sized;
}

struct DefaultConstants {
    genesis: Block<Self>,
}

impl NetworkConstants for DefaultConstants {
    fn new(validators: Vec<Validator>, leader: Validator) -> DefaultConstants {
        match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(ts) => DefaultConstants {
                genesis: Block {
                    height: 0,
                    validators,
                    leader,
                    ts: ts.as_secs(),
                    txs: vec![],
                    proofs: vec![],
                    phantom: Default::default(),
                },
            },
            Err(_) => panic!("System time is messed up"),
        }
    }

    fn genesis_block(self) -> Block<Self> {
        self.genesis
    }
}

struct Rewards {
    total: u64,
    provers: HashMap<Address, u64>,
    stakers: HashMap<Address, u64>,
    leader: (Address, u64),
}

impl<T: NetworkConstants> Block<T> {
    fn n(&self) -> u64 {
        (self.validators.len()) as u64
    }

    fn factor(&self, genesis_timestamp: u64) -> u32 {
        ((self.ts - genesis_timestamp - (self.height * T::ANCHOR_TIME)) / (self.n() * T::ANCHOR_TIME)) as u32
    }

    fn reward_leading(&self) -> u64 {
        self.txs.iter().map(|tx| &tx.fee_total).sum()
    }

    fn prover_share(&self, prover: Address) -> u64 {
        let mut prover_sum = 0;
        let mut total_sum = 0;
        for proof in &self.proofs {
            if proof.prover == prover {
                prover_sum += &proof.target;
            }
            total_sum += &proof.target;
        }
        prover_sum * FIXED_POINT_DECIMALS / total_sum
    }

    fn staker_share(&self, staker_address: Address) -> u64 {
        let total_sum: u64 = self.validators.iter().map(|v| v.stake).sum();
        if let Some(staker) = self.validators.iter().find(|v| v.address == staker_address) {
            staker.stake * FIXED_POINT_DECIMALS / total_sum
        } else {
            0
        }
    }

    fn reward_proving(&self, genesis: &Block<T>) -> u64 {
        // to convert the algorithm into integers, we do the following
        // reward_{proving} = max(0, height_{year10} - block_i.height) * reward_{anchor} * 2^{-factor_i}
        //                  = (multiplier * reward_{anchor}) / 2^{factor_i}
        let multiplier = T::height_year10().saturating_sub(self.height);
        // let inverse_factor = (self.factor(genesis.ts) as i64).neg();
        // let factor = (2_f32).powi(inverse_factor as i32) as f64;
        // (multiplier * (T::reward_anchor() as f64) * factor).round() as u64
        (multiplier * T::reward_anchor()) / (2_u64.pow(self.factor(genesis.ts)))
    }

    fn compute_rewards(self, genesis: Block<T>) -> Rewards {
        // let's collect all mintable rewards into this reward struct
        let mut rewards =
            Rewards { total: 0, provers: HashMap::new(), stakers: HashMap::new(), leader: (self.leader.address, 0) };
        // compute prover rewards
        let proving_reward_total = self.reward_proving(&genesis);
        // TODO this ignores that block might contain multiple proofs from the same prover
        for proof in &self.proofs {
            let share = self.prover_share(proof.prover);

            let prover_reward = (&proving_reward_total / 2) * share / FIXED_POINT_DECIMALS;
            rewards.total += &prover_reward;
            rewards.provers.insert(proof.prover, prover_reward);
        }

        // compute staker rewards
        let staking_reward_total = T::reward_staking();
        for validator in &self.validators {
            let share = self.staker_share(validator.address);

            let staking_reward = ((&proving_reward_total / 2) + staking_reward_total) * share / FIXED_POINT_DECIMALS;
            rewards.total += &staking_reward;
            rewards.stakers.insert(validator.address, staking_reward);
        }

        // finally compute leader reward
        let leader_address = self.leader.address;
        let leader_reward = self.reward_leading();
        rewards.total += &leader_reward;
        rewards.leader = (leader_address, leader_reward);

        rewards
    }
}

#[cfg(test)]
mod tests {
    use crate::{Block, DefaultConstants, NetworkConstants, Validator};
    use proptest::prelude::*;

    #[test]
    fn test_smoke() {
        let leader: Validator = Validator { stake: 1, address: 1 };
        let network = DefaultConstants::new(vec![leader], leader);
        let genesis_block = network.genesis_block();
        let block: Block<DefaultConstants> = Block {
            height: 1,
            validators: vec![leader],
            leader,
            ts: &genesis_block.ts + 100,
            txs: vec![],
            proofs: vec![],
            phantom: Default::default(),
        };

        let rewards = block.compute_rewards(genesis_block);

        // no provers so no prover rewards
        assert_eq!(rewards.provers.len(), 0);
        assert_eq!(rewards.stakers.len(), 1);
        if let Some(reward) = rewards.stakers.get(&rewards.leader.0) {
            // TODO: write the expected value in equation form
            assert_eq!(*reward, 19796894);
            // no fees, so no leader rewards
            assert_eq!(rewards.leader.1, 0);
            assert_eq!(rewards.total, 19796894);
        } else {
            panic!();
        }
    }

    fn arbitrary_validator(max_stake: u64) -> impl Strategy<Value = Validator> {
        (any::<u64>(), 1..max_stake).prop_map(|(address, stake)| Validator { address, stake })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 100,
            failure_persistence: None,
            .. ProptestConfig::default()
        })]


        #[test]
        fn default_constants(validators in proptest::collection::vec(arbitrary_validator(1000), 1..4)) {
            let leader = validators[0];
            let network = DefaultConstants::new(validators.clone(), leader);
            assert_eq!(network.genesis.leader, leader);
            assert_eq!(network.genesis.validators, validators);
            assert_eq!(network.genesis.height, 0);
        }
    }
}
