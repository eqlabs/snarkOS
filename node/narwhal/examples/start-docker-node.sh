#!/bin/bash
# boilerplate Bash safety nets
set -euo pipefail
IFS=$'\n\t'

# Assumptions: container is run with hostname "peer<n>" and either DNS or static IP that doesn't change in the network
# between restarts
HOST_NAME=$(hostname)

# Verify peer is in form of "peer<n>" where n is the node-id
if ! [[ "$HOST_NAME" =~ ^peer[0-9]+$ ]]; then
    echo "Error: invalid peer ${HOST_NAME} (valid is 'peer<n>' where n is the node-id), fix networking"
    exit 1
fi

# Extract the <n> from "peer<n>", convert it into a number
NODE_ID=$((10#${HOST_NAME:4}))

# Read the nodeid/address map file
# - the idea is to provide this file as a volume to the container, so that different devnets don't require new images
PEER_FILE_PATH="config/docker.peer-map.txt"
if ! [[ -e "$PEER_FILE_PATH" ]]; then
   echo "Error: Peer file does not exist at ${PEER_FILE_PATH}, fix volume"
   exit 1
fi

# Compute the size of network
NUM_NODES=$(cat $PEER_FILE_PATH | wc -l)

echo "Running: ./simple_node --mode bft --id $NODE_ID --num-nodes $NUM_NODES --peers $PEER_FILE_PATH"
./simple_node --mode bft --id "$NODE_ID" --num-nodes "$NUM_NODES" --peers "$PEER_FILE_PATH"
