#!/bin/bash
# boilerplate Bash safety nets
set -euo pipefail
IFS=$'\n\t'

COMMITTEE_SIZE=${COMMITTEE_SIZE:-4}
NETWORK_OPTIONS="devnet"
VOLUME_OPTIONS="--volume ./docker.${COMMITTEE_SIZE}-peer.network.txt:/simple_node/config/docker.peer-map.txt:z,ro"
IMAGE_NAME="${IMAGE_NAME:-my-simple-node}"

podman network create --driver bridge --ignore --subnet 172.16.0.0/16 --gateway 172.16.0.1 devnet

PEER_HOST_OPTIONS="--add-host peer0:172.16.0.2"

# Loop through N from 1 to COMMITTEE_SIZE to create etc/hosts mappings (things we do to avoid setting up DNS)
for ((i=1; i<COMMITTEE_SIZE; i++))
do
    peer_name="peer$i"
    # 172.16.0.1 is reserved for the gateway, let's skip that
    ip_address="172.16.0.$((i+2))"
    PEER_HOST_OPTIONS+=" --add-host ${peer_name}:${ip_address}"
done

# Loop through the COMMITTEE_SIZE peers and create containers
for ((i=0; i<COMMITTEE_SIZE; i++))
do
    peer_name="peer$i"
    # 172.16.0.1 is reserved for the gateway, let's skip that
    ip_address="172.16.0.$((i+2))"
    command="podman create --network devnet $PEER_HOST_OPTIONS $VOLUME_OPTIONS --ip=$ip_address --hostname=$peer_name --name=$peer_name $IMAGE_NAME"
    container_id=$(eval "$command")
    echo "Created $peer_name @ $ip_address -> $container_id"
done
