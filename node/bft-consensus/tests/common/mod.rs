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

mod objects;
mod setup;
mod state;
mod transaction;
mod validation;

pub use objects::*;
pub use setup::*;
pub use state::*;
pub use transaction::*;
pub use validation::*;

use tracing_subscriber::filter::{EnvFilter, LevelFilter};

pub fn start_logger(default_level: LevelFilter) {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter
            .add_directive("anemo=off".parse().unwrap())
            .add_directive("rustls=off".parse().unwrap())
            .add_directive("tokio_util=off".parse().unwrap())
            .add_directive("typed_store=off".parse().unwrap()),
        _ => EnvFilter::default()
            .add_directive(default_level.into())
            .add_directive("anemo=off".parse().unwrap())
            .add_directive("rustls=off".parse().unwrap())
            .add_directive("tokio_util=off".parse().unwrap())
            .add_directive("typed_store=off".parse().unwrap()),
    };

    tracing_subscriber::fmt().with_env_filter(filter).with_target(true).init();
}
