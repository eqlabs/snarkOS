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

#![forbid(unsafe_code)]

#[macro_use]
extern crate tracing;

mod helpers;
pub use helpers::*;

mod routes;
pub use routes::*;

mod axum_routes;
use axum_routes::*;

use snarkos_node_consensus::Consensus;
use snarkos_node_ledger::Ledger;
use snarkos_node_messages::{Data, Message, NodeType, UnconfirmedTransaction};
use snarkos_node_router::{Router, Routing};
use snarkvm::{
    console::{account::Address, program::ProgramID, types::Field},
    prelude::{cfg_into_iter, Block, Network, StatePath, Transactions},
    synthesizer::{ConsensusStorage, Program, Transaction},
};

use anyhow::Result;
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{Method, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use http::header::{HeaderName, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::SocketAddr, str::FromStr, sync::Arc};
use tokio::task::JoinHandle;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
// use warp::{reject, reply, Filter, Rejection, Reply};

/// A REST API server for the ledger.
#[derive(Clone)]
pub struct Rest<N: Network, C: ConsensusStorage<N>, R: Routing<N>> {
    /// The consensus module.
    consensus: Option<Consensus<N, C>>,
    /// The ledger.
    ledger: Ledger<N, C>,
    /// The node (routing).
    routing: Arc<R>,
    /// The server handles.
    handles: Vec<Arc<JoinHandle<()>>>,
}

impl<N: Network, C: 'static + ConsensusStorage<N>, R: Routing<N>> Rest<N, C, R> {
    /// Initializes a new instance of the server.
    pub fn start(
        rest_ip: SocketAddr,
        consensus: Option<Consensus<N, C>>,
        ledger: Ledger<N, C>,
        routing: Arc<R>,
    ) -> Result<Self> {
        // Initialize the server.
        let mut server = Self { consensus, ledger, routing, handles: vec![] };
        // Spawn the server.
        server.spawn_server(rest_ip);
        // Return the server.
        Ok(server)
    }
}

impl<N: Network, C: ConsensusStorage<N>, R: Routing<N>> Rest<N, C, R> {
    /// Returns the ledger.
    pub const fn ledger(&self) -> &Ledger<N, C> {
        &self.ledger
    }

    /// Returns the handles.
    pub const fn handles(&self) -> &Vec<Arc<JoinHandle<()>>> {
        &self.handles
    }
}

impl<N: Network, C: ConsensusStorage<N>, R: Routing<N>> Rest<N, C, R> {
    fn spawn_server(&mut self, rest_ip: SocketAddr) {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([CONTENT_TYPE]);

        let router = {
            axum::Router::new()

            .route("/testnet3/latest/height", get(latest_height))
            .route("/testnet3/latest/hash", get(latest_hash))
            .route("/testnet3/latest/block", get(latest_block))
            .route("/testnet3/latest/stateRoot", get(latest_state_root))

            .route("/testnet3/block/:height_or_hash", get(get_block))
            // The path param here is actually only the height, but the name must match the route
            // above, otherwise there'll be a conflict at runtime.
            .route("/testnet3/block/:height_or_hash/transactions", get(get_block_transactions))

            .route("/testnet3/blocks", get(get_blocks))
            .route("/testnet3/height/:hash", get(get_height))
            .route("/testnet3/memoryPool/transactions", get(get_memory_pool_transactions))
            .route("/testnet3/program/:id", get(get_program))
            .route("/testnet3/statePath/:commitment", get(get_state_path_for_commitment))
            .route("/testnet3/beacons", get(get_beacons))
            .route("/testnet3/node/address", get(get_node_address))

            .route("/testnet3/peers/count", get(get_peers_count))
            .route("/testnet3/peers/all", get(get_peers_all))
            .route("/testnet3/peers/all/metrics", get(get_peers_all_metrics))


            .route("/testnet3/find/blockHash/:tx_id", get(find_block_hash))
            .route("/testnet3/find/transactionID/deployment/:program_id", get(find_transaction_id_from_program_id))
            .route("/testnet3/find/transactionID/:transition_id", get(find_transaction_id_from_transition_id))
            .route("/testnet3/find/transitionID/:input_or_output_id", get(find_transition_id))

            .route("/testnet3/transaction/:id", get(get_transaction))
            .route("/testnet3/transaction/broadcast", post(transaction_broadcast))

            // Pass in `Rest` to make things convenient.
            .with_state(self.clone())

            // TODO(nkls): add JWT auth.
            .layer(TraceLayer::new_for_http())
            .layer(cors)
            // Cap body size at 10MB
            .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
            .layer(middleware::from_fn(auth_middleware))
        };

        self.handles.push(Arc::new(tokio::spawn(async move {
            axum::Server::bind(&rest_ip).serve(router.into_make_service()).await.expect("couldn't start rest server");
        })))
    }
}

struct RestError(String);

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Something went wrong: {}", self.0)).into_response()
    }
}

impl From<anyhow::Error> for RestError {
    fn from(err: anyhow::Error) -> Self {
        Self(err.to_string())
    }
}

// impl<N: Network, C: 'static + ConsensusStorage<N>, R: Routing<N>> Rest<N, C, R> {
//  /// Initializes the server.
//  fn spawn_warp_server(&mut self, rest_ip: SocketAddr) {
//      let cors = warp::cors()
//          .allow_any_origin()
//          .allow_header(HeaderName::from_static("content-type"))
//          .allow_methods(vec!["GET", "POST", "OPTIONS"]);

//      // Initialize the routes.
//      let routes = self.routes();

//      // Add custom logging for each request.
//      let custom_log = warp::log::custom(|info| match info.remote_addr() {
//          Some(addr) => debug!("Received '{} {}' from '{addr}' ({})", info.method(), info.path(), info.status()),
//          None => debug!("Received '{} {}' ({})", info.method(), info.path(), info.status()),
//      });

//      // Spawn the server.
//      self.handles.push(Arc::new(tokio::spawn(async move {
//          // Start the server.
//          warp::serve(routes.with(cors).with(custom_log)).run(rest_ip).await
//      })))
//  }
// }
