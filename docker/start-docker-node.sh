#!/bin/bash
# boilerplate Bash safety nets
set -euo pipefail
IFS=$'\n\t'

ARGS="$@"

# Run the jemalloc-enabled executable if JEMALLOC env var is set
EXECUTABLE="snarkos_node"
if [[ -n ${JEMALLOC:-} ]]; then
  EXECUTABLE="snarkos_node"
fi

# Run the simple_node with heaptrack if HEAPTRACK env var is set
if [[ -n ${HEAPTRACK:-} ]]; then
    command="/usr/bin/heaptrack ./$EXECUTABLE $ARGS"
    echo "Running: $command"
    eval "$command"
else
    command="./$EXECUTABLE $ARGS"
    echo "Running: $command"
    eval "$command"
fi
