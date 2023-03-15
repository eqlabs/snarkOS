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

use rand::thread_rng;
use tracing_subscriber::filter::LevelFilter;

mod common;

use common::{start_logger, CommitteeSetup, PrimarySetup};

#[tokio::test(flavor = "multi_thread")]
async fn foo() {
    // Start the logger.
    start_logger(LevelFilter::DEBUG);

    // Prepare a source of randomness.
    let mut rng = thread_rng();

    // Configure the primary-related variables.
    const NUM_PRIMARIES: usize = 4;
    const WORKERS_PER_PRIMARY: u32 = 1;
    const PRIMARY_STAKE: u64 = 1;

    // Generate the committee setup.
    let mut primaries = Vec::with_capacity(NUM_PRIMARIES);
    for _ in 0..NUM_PRIMARIES {
        let primary = PrimarySetup::new(PRIMARY_STAKE, WORKERS_PER_PRIMARY, &mut rng);
        primaries.push(primary);
    }
    let mut committee = CommitteeSetup::new(primaries, 0);

    // Create and start the preconfigured consensus instances.
    let inert_consensus_instances = committee.generate_consensus_instances();
    let mut running_consensus_instances = Vec::with_capacity(NUM_PRIMARIES);
    for instance in inert_consensus_instances {
        let running_instance = instance.start().await.unwrap();
        running_consensus_instances.push(running_instance);
    }

    // TODO: extend to something useful and rename the test
    std::future::pending().await
}
