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

use snarkvm::console::{prelude::*, types::Address};

use indexmap::IndexMap;

#[derive(Clone, Debug)]
pub struct Committee<N: Network> {
    /// The current round number.
    round: u64,
    /// A map of `address` to `stake`.
    members: IndexMap<Address<N>, u64>,
}

impl<N: Network> Committee<N> {
    /// Initializes a new `Committee` instance.
    pub fn new(round: u64, members: IndexMap<Address<N>, u64>) -> Result<Self> {
        // Ensure the round is nonzero.
        ensure!(round > 0, "Round must be nonzero");
        // Ensure there are at least 4 members.
        ensure!(members.len() >= 4, "Committee must have at least 4 members");
        // Return the new committee.
        Ok(Self { round, members })
    }

    /// Returns a new `Committee` instance for the next round.
    /// TODO (howardwu): Add arguments for members (and stake) 1) to be added, 2) to be updated, and 3) to be removed.
    pub fn to_next_round(&self) -> Result<Self> {
        // Increment the round number.
        let Some(round) = self.round.checked_add(1) else {
            bail!("Overflow when incrementing round number in committee");
        };
        // Return the new committee.
        Ok(Self { round, members: self.members.clone() })
    }
}

impl<N: Network> Committee<N> {
    /// Returns the current round number.
    pub fn round(&self) -> u64 {
        self.round
    }

    /// Returns the committee members alongside their stake.
    pub fn members(&self) -> &IndexMap<Address<N>, u64> {
        &self.members
    }

    /// Returns the number of validators in the committee.
    pub fn committee_size(&self) -> usize {
        self.members.len()
    }

    /// Returns `true` if the given address is in the committee.
    pub fn is_committee_member(&self, address: Address<N>) -> bool {
        self.members.contains_key(&address)
    }

    /// Returns the amount of stake for the given address.
    pub fn get_stake(&self, address: Address<N>) -> u64 {
        self.members.get(&address).copied().unwrap_or_default()
    }

    /// Returns the amount of stake required to reach the availability threshold `(f + 1)`.
    pub fn availability_threshold(&self) -> Result<u64> {
        // Assuming `N = 3f + 1 + k`, where `0 <= k < 3`,
        // then `(N + 2) / 3 = f + 1 + k/3 = f + 1`.
        Ok(self.total_stake()?.saturating_add(2) / 3)
    }

    /// Returns the amount of stake required to reach a quorum threshold `(2f + 1)`.
    pub fn quorum_threshold(&self) -> Result<u64> {
        // Assuming `N = 3f + 1 + k`, where `0 <= k < 3`,
        // then `(2N + 3) / 3 = 2f + 1 + (2k + 2)/3 = 2f + 1 + k = N - f`.
        Ok(self.total_stake()?.saturating_mul(2) / 3 + 1)
    }

    /// Returns the total amount of stake in the committee `(3f + 1)`.
    pub fn total_stake(&self) -> Result<u64> {
        // Compute the total power of the committee.
        let mut power = 0u64;
        for stake in self.members.values() {
            // Accumulate the stake, checking for overflow.
            power = match power.checked_add(*stake) {
                Some(power) => power,
                None => bail!("Failed to calculate total stake - overflow detected"),
            };
        }
        Ok(power)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ops::Range;

    use indexmap::IndexMap;
    use proptest::{collection::vec, prelude::*};
    use snarkos_account::Account;

    type CurrentNetwork = snarkvm::prelude::Testnet3;

    const MIN_MEMBERS: usize = 4;
    const MAX_MEMBERS: usize = 500;

    const MIN_STAKE: u64 = u64::MIN;
    const MAX_STAKE: u64 = u64::MAX;

    // Generates a random address.
    fn arbitrary_address() -> impl Strategy<Value = Address<CurrentNetwork>> {
        any::<u64>().prop_map(|seed| {
            let mut rng = TestRng::fixed(seed);
            let account = Account::<CurrentNetwork>::new(&mut rng).unwrap();
            account.address() // assuming `address()` method exists on `Account`
        })
    }

    // Generates a random address and stake.
    fn arbitrary_member() -> impl Strategy<Value = (Address<CurrentNetwork>, u64)> {
        (arbitrary_address(), MIN_STAKE..MAX_STAKE)
    }

    // Generates a random map of addresses to stakes.
    fn arbitrary_members(range: Range<usize>) -> impl Strategy<Value = IndexMap<Address<CurrentNetwork>, u64>> {
        vec(arbitrary_member(), range).prop_map(|vec| vec.into_iter().collect())
    }

    proptest! {
        #[test]
        fn test_new_round_and_members_conditions(round in 1u64.., members in arbitrary_members(MIN_MEMBERS..MAX_MEMBERS)) {
            let result = Committee::new(round, members);
            assert!(result.is_ok(), "New committee creation failed with valid input parameters");
        }
    }

    proptest! {
        #[test]
        fn test_new_round_zero(members in arbitrary_members(MIN_MEMBERS..MAX_MEMBERS)) {
            let result = Committee::new(0, members);
            assert!(result.is_err(), "New committee creation should fail with zero round");
        }
    }

    proptest! {
        #[test]
        fn test_new_members_less_than_4(round in 1u64.., members in arbitrary_members(0..4)) {
            let result = Committee::new(round, members);
            assert!(result.is_err(), "New committee creation should fail with less than 4 members");
        }
    }
}
