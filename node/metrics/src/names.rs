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

pub const GAUGE_NAMES: [&str; 8] = [
    blocks::HEIGHT,
    consensus::COMMITTED_CERTIFICATES,
    consensus::LAST_COMMITTED_ROUND,
    network::NETWORK_PEERS,
    peers::CANDIDATE,
    peers::CONNECTED,
    peers::RESTRICTED,
    primary::CURRENT_ROUND,
];
pub const COUNTER_NAMES: [&str; 1] = [consensus::LEADERS_ELECTED];
pub const HISTOGRAM_NAMES: [&str; 3] =
    [consensus::CERTIFICATE_COMMIT_LATENCY, consensus::COMMIT_ROUNDS_LATENCY, subscribers::CERTIFICATE_LATENCY];

pub mod blocks {
    pub const HEIGHT: &str = "snarkos_blocks_height_total";
}

pub mod peers {
    pub const CONNECTED: &str = "snarkos_peers_connected_total";
    pub const CANDIDATE: &str = "snarkos_peers_candidate_total";
    pub const RESTRICTED: &str = "snarkos_peers_restricted_total";
}

pub mod consensus {
    pub const COMMITTED_CERTIFICATES: &str = "snarkos_consensus_committed_certificates_total";
    pub const CERTIFICATE_COMMIT_LATENCY: &str = "snarkos_consensus_certificate_commit_latency_secs";
    pub const COMMIT_ROUNDS_LATENCY: &str = "snarkos_consensus_commit_rounds_latency_secs";
    pub const LEADERS_ELECTED: &str = "snarkos_consensus_leaders_elected_total";
    pub const LAST_COMMITTED_ROUND: &str = "snarkos_consensus_last_committed_round";
}

pub mod network {
    pub const NETWORK_PEERS: &str = "snarkos_network_peers_connected_total";
    pub const NETWORK_PEER_CONNECTED: &str = "snarkos_network_peer_connected";

    pub mod labels {
        pub const PEER_ID: &str = "peer_id";
    }
}

pub mod subscribers {
    pub const CERTIFICATE_LATENCY: &str = "snarkos_subscribers_certificate_latency_secs";
}

pub mod primary {
    pub const CURRENT_ROUND: &str = "snarkos_primary_current_round";
}
