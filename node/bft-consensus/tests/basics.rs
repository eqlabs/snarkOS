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

use common::{start_logger, CommitteeSetup, PrimarySetup, TestBftConsensus};

#[tokio::test]
async fn foo() {
    start_logger(LevelFilter::DEBUG);

    let mut rng = thread_rng();

    let primaries = vec![
        PrimarySetup::new(1, &mut rng),
        PrimarySetup::new(1, &mut rng),
        PrimarySetup::new(1, &mut rng),
        PrimarySetup::new(1, &mut rng),
    ];
    let mut committee = CommitteeSetup::new(primaries, 0);

    let inert_consensus_instances = committee.generate_consensus_instances();
    let mut running_consensus_instances = Vec::with_capacity(4); // TODO: make adjustable
    for instance in inert_consensus_instances {
        let running_instance = instance.start().await.unwrap();
        running_consensus_instances.push(running_instance);
    }

    std::future::pending().await

    // TODO: extend to something useful and rename
}
