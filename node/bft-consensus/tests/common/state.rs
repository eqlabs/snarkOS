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

use async_trait::async_trait;
use narwhal_executor::ExecutionState;
use narwhal_types::ConsensusOutput;

// TODO: come up with some useful state to alter and test via consensus
#[derive(Default)]
pub struct TestBftExecutionState;

#[async_trait]
impl ExecutionState for TestBftExecutionState {
    async fn handle_consensus_output(&self, _consensus_output: ConsensusOutput) {}

    async fn last_executed_sub_dag_index(&self) -> u64 {
        0
    }
}
