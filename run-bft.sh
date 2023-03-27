#!/usr/bin/env bash

trap func exit
# Declare the function
function func() {
	kill $(jobs -p)
	echo "Done"
}

# Start other validators without metrics
for i in 0 1 2 3; do
	# Enable metrics for first validator only
	extra_args=""
	if [[ "$i" -eq 0 ]]; then
		extra_args=" --metrics"
	fi
	NUM=100
	VALIDATOR_COMMAND="cargo +stable run -- start$extra_args --nodisplay --verbosity 0 --genesis /var/tmp/genesis-${NUM}.bin --program /var/tmp/program-${NUM}.bin --dev ${i} --validator"
	echo "starting validator as $i, check logs at ./validator-$i.log"
	echo "command: $VALIDATOR_COMMAND"
	$VALIDATOR_COMMAND '' >validator-$i.log 2>&1 &
done

echo "All running, press Ctrl-C to stop"
wait
