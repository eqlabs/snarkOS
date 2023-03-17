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

use anyhow::anyhow;
use fastcrypto::{
    bls12381::min_sig::BLS12381KeyPair,
    ed25519::Ed25519KeyPair,
    encoding::{Base64, Encoding},
    traits::{EncodeDecodeBase64, ToFromBytes},
};
use std::path::PathBuf;

use aleo_std::aleo_dir;

fn base_path(dev: Option<u16>) -> PathBuf {
    // Retrieve the starting directory.
    match dev.is_some() {
        // In development mode, the ledger is stored in the root directory of the repository.
        true => match std::env::current_dir() {
            Ok(current_dir) => current_dir,
            _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        },
        // In production mode, the ledger is stored in the `~/.aleo/` directory.
        false => aleo_dir(),
    }
}

pub(crate) fn primary_dir(network: u16, dev: Option<u16>) -> PathBuf {
    let mut path = base_path(dev);

    // Construct the path to the ledger in storage.
    //
    // Prod: `~/.aleo/storage/bft-{network}/primary`
    // Dev: `path/to/repo/.bft-{network}/primary-{id}`
    match dev {
        Some(id) => {
            path.push(format!(".bft-{network}"));
            path.push(format!("primary-{id}"));
        }

        None => {
            path.push("storage");
            path.push(format!("bft-{network}"));
            path.push("primary");
        }
    }

    path
}

pub(crate) fn worker_dir(network: u16, worker_id: u32, dev: Option<u16>) -> PathBuf {
    // Retrieve the starting directory.
    let mut path = base_path(dev);

    // Construct the path to the ledger in storage.
    //
    // Prod: `~/.aleo/storage/bft-{network}/worker-{worker_id}`
    // Dev: `path/to/repo/.bft-{network}/worker-{primary_id}-{worker_id}`
    match dev {
        Some(primary_id) => {
            path.push(format!(".bft-{network}"));
            path.push(format!("worker-{primary_id}-{worker_id}"));
        }

        None => {
            path.push("storage");
            path.push(format!("bft-{network}"));
            path.push(format!("worker-{worker_id}"));
        }
    }

    path
}

pub(crate) fn read_network_keypair_from_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Ed25519KeyPair> {
    let contents = std::fs::read_to_string(path)?;
    let bytes = Base64::decode(contents.as_str()).map_err(|e| anyhow!("{}", e.to_string()))?;
    Ed25519KeyPair::from_bytes(bytes.get(1..).unwrap()).map_err(|e| anyhow!(e))
}

pub(crate) fn read_authority_keypair_from_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<BLS12381KeyPair> {
    let contents = std::fs::read_to_string(path)?;
    BLS12381KeyPair::decode_base64(contents.as_str().trim()).map_err(|e| anyhow!(e))
}
