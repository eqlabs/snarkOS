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

use super::*;

impl<N: Network, C: ConsensusStorage<N>, R: Routing<N>> Rest<N, C, R> {
    // GET /testnet3/latest/height
    pub(crate) async fn latest_height(State(rest): State<Rest<N, C, R>>) -> Json<u32> {
        Json(rest.ledger.latest_height())
    }

    // GET /testnet3/latest/hash
    pub(crate) async fn latest_hash(State(rest): State<Rest<N, C, R>>) -> Json<N::BlockHash> {
        Json(rest.ledger.latest_hash())
    }

    // GET /testnet3/latest/block
    pub(crate) async fn latest_block(State(rest): State<Rest<N, C, R>>) -> Json<Block<N>> {
        Json(rest.ledger.latest_block())
    }

    // GET /testnet3/latest/stateRoot
    pub(crate) async fn latest_state_root(State(rest): State<Rest<N, C, R>>) -> Json<N::StateRoot> {
        Json(rest.ledger.latest_state_root())
    }

    // GET /testnet3/block/{height}
    // GET /testnet3/block/{blockHash}
    pub(crate) async fn get_block(
        State(rest): State<Rest<N, C, R>>,
        Path(height_or_hash): Path<String>,
    ) -> Result<Json<Block<N>>, RestError> {
        // Manually parse the height or the height or the hash, axum doesn't support different types
        // for the same path param.
        let block = if let Ok(height) = height_or_hash.parse::<u32>() {
            rest.ledger.get_block(height)?
        } else {
            let hash = height_or_hash
                .parse::<N::BlockHash>()
                .map_err(|_| RestError("invalid input, it is neither a block height nor a block hash".to_string()))?;

            rest.ledger.get_block_by_hash(&hash)?
        };

        Ok(Json(block))
    }

    // GET /testnet3/blocks?start={start_height}&end={end_height}
    pub(crate) async fn get_blocks(
        State(rest): State<Rest<N, C, R>>,
        Query((start_height, end_height)): Query<(u32, u32)>,
    ) -> Result<Json<Vec<Block<N>>>, RestError> {
        const MAX_BLOCK_RANGE: u32 = 50;

        // Ensure the end height is greater than the start height.
        if start_height > end_height {
            return Err(RestError("Invalid block range".to_string()));
        }

        // Ensure the block range is bounded.
        if end_height - start_height > MAX_BLOCK_RANGE {
            return Err(RestError(format!(
                "Cannot request more than {MAX_BLOCK_RANGE} blocks per call (requested {})",
                end_height - start_height
            )));
        }

        let blocks = cfg_into_iter!((start_height..end_height))
            .map(|height| rest.ledger.get_block(height))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Json(blocks))
    }

    // GET /testnet3/height/{blockHash}
    pub(crate) async fn get_height(
        State(rest): State<Rest<N, C, R>>,
        Path(hash): Path<N::BlockHash>,
    ) -> Result<Json<u32>, RestError> {
        Ok(Json(rest.ledger.get_height(&hash)?))
    }

    // GET /testnet3/block/{height}/transactions
    pub(crate) async fn get_block_transactions(
        State(rest): State<Rest<N, C, R>>,
        Path(height): Path<u32>,
    ) -> Result<Json<Transactions<N>>, RestError> {
        Ok(Json(rest.ledger.get_transactions(height)?))
    }

    // GET /testnet3/transaction/{transactionID}
    pub(crate) async fn get_transaction(
        State(rest): State<Rest<N, C, R>>,
        Path(tx_id): Path<N::TransactionID>,
    ) -> Result<Json<Transaction<N>>, RestError> {
        Ok(Json(rest.ledger.get_transaction(tx_id)?))
    }

    // GET /testnet3/memoryPool/transactions
    pub(crate) async fn get_memory_pool_transactions(
        State(rest): State<Rest<N, C, R>>,
    ) -> Result<Json<Vec<Transaction<N>>>, RestError> {
        match rest.consensus {
            Some(consensus) => Ok(Json(consensus.memory_pool().unconfirmed_transactions())),
            None => Err(RestError("route isn't available for this node type".to_string())),
        }
    }

    // GET /testnet3/program/{programID}
    pub(crate) async fn get_program(
        State(rest): State<Rest<N, C, R>>,
        Path(id): Path<ProgramID<N>>,
    ) -> Result<Json<Program<N>>, RestError> {
        let program = if id == ProgramID::<N>::from_str("creadits.aleo")? {
            Program::<N>::credits()?
        } else {
            rest.ledger.get_program(id)?
        };

        Ok(Json(program))
    }

    // GET /testnet3/statePath/{commitment}
    pub(crate) async fn get_state_path_for_commitment(
        State(rest): State<Rest<N, C, R>>,
        Path(commitment): Path<Field<N>>,
    ) -> Result<Json<StatePath<N>>, RestError> {
        Ok(Json(rest.ledger.get_state_path_for_commitment(&commitment)?))
    }

    // GET /testnet3/beacons
    pub(crate) async fn get_beacons(State(rest): State<Rest<N, C, R>>) -> Result<Json<Vec<Address<N>>>, RestError> {
        match rest.consensus {
            Some(consensus) => Ok(Json(consensus.beacons().keys().copied().collect())),
            None => Err(RestError("route isn't available for this node type".to_string())),
        }
    }

    // GET /testnet3/peers/count
    pub(crate) async fn get_peers_count(State(rest): State<Rest<N, C, R>>) -> Json<usize> {
        Json(rest.routing.router().number_of_connected_peers())
    }

    // GET /testnet3/peers/all
    pub(crate) async fn get_peers_all(State(rest): State<Rest<N, C, R>>) -> Json<Vec<SocketAddr>> {
        Json(rest.routing.router().connected_peers())
    }

    // GET /testnet3/peers/all/metrics
    pub(crate) async fn get_peers_all_metrics(State(rest): State<Rest<N, C, R>>) -> Json<Vec<(SocketAddr, NodeType)>> {
        Json(rest.routing.router().connected_metrics())
    }

    // GET /testnet3/node/address
    pub(crate) async fn get_node_address(State(rest): State<Rest<N, C, R>>) -> Json<Address<N>> {
        Json(rest.routing.router().address())
    }

    // GET /testnet3/find/blockHash/{transactionID}
    pub(crate) async fn find_block_hash(
        State(rest): State<Rest<N, C, R>>,
        Path(tx_id): Path<N::TransactionID>,
    ) -> Result<Json<Option<N::BlockHash>>, RestError> {
        Ok(Json(rest.ledger.find_block_hash(&tx_id)?))
    }

    // GET /testnet3/find/transactionID/deployment/{programID}
    pub(crate) async fn find_transaction_id_from_program_id(
        State(rest): State<Rest<N, C, R>>,
        Path(program_id): Path<ProgramID<N>>,
    ) -> Result<Json<Option<N::TransactionID>>, RestError> {
        Ok(Json(rest.ledger.find_transaction_id_from_program_id(&program_id)?))
    }

    // GET /testnet3/find/transactionID/{transitionID}
    pub(crate) async fn find_transaction_id_from_transition_id(
        State(rest): State<Rest<N, C, R>>,
        Path(transition_id): Path<N::TransitionID>,
    ) -> Result<Json<Option<N::TransactionID>>, RestError> {
        Ok(Json(rest.ledger.find_transaction_id_from_transition_id(&transition_id)?))
    }

    // GET /testnet3/find/transitionID/{inputOrOutputID}
    pub(crate) async fn find_transition_id(
        State(rest): State<Rest<N, C, R>>,
        Path(input_or_output_id): Path<Field<N>>,
    ) -> Result<Json<N::TransitionID>, RestError> {
        Ok(Json(rest.ledger.find_transition_id(&input_or_output_id)?))
    }

    // POST /testnet3/transaction/broadcast
    pub(crate) async fn transaction_broadcast(
        State(rest): State<Rest<N, C, R>>,
        Json(tx): Json<Transaction<N>>,
    ) -> Result<Json<N::TransactionID>, RestError> {
        // If the consensus module is enabled, add the unconfirmed transaction to the memory pool.
        if let Some(consensus) = rest.consensus {
            // Add the unconfirmed transaction to the memory pool.
            consensus.add_unconfirmed_transaction(tx.clone())?;
        }

        // Prepare the unconfirmed transaction message.
        let tx_id = tx.id();
        let message = Message::UnconfirmedTransaction(UnconfirmedTransaction {
            transaction_id: tx_id,
            transaction: Data::Object(tx),
        });

        // Broadcast the transaction.
        rest.routing.propagate(message, vec![]);

        Ok(Json(tx_id))
    }
}
