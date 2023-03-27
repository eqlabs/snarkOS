# Bullshark testing

## Pre-Generating Transactions

To be able to inject the tranasctions into a running network, we save the genesis block used for generation to a file and
the block with the deployment transaction in another file. The transactions are generated and saved in yet another file.

When the network starts up (--dev mode only), it can read the genesis block (all validators) and the deployment transaction block (only the first validator) and passes this to the next round
after this new block is propagated, the pre-generated transactions are now valid in this VM.

We can then send them with another program to the workers.

These 3 files are re-usable now (but you do need to restart the network to test again). So you only have to eat the cost of generating the test transactions once.

## Testing

You can pre-generate these 3 files:

`cargo test --package snarkos-node-consensus --lib -- tests::pre_generate --exact --nocapture --ignored`

or use a pre-generated one.

Right now it only generates 100 transactions, you can change this to how much you like by changing the `NUM` const in `node/consensus/src/tests.rs`.

Then start the network using

```
cargo +stable run -- start --nodisplay --verbosity 0 --genesis /var/tmp/genesis-<num tx>.bin --program /var/tmp/program-<num tx>.bin --dev <id> --validator
```

This passes --genesis /var/tmp/genesis-<num tx>.bin --program /var/tmp/program-<num-tx>.bin arguments to snarkOS - these arguments only do something in --dev mode.

When you see it has propagated the deployment block (you should see "Advanced to block 1" in all validators), send the transactions using

```
cargo test --package snarkos-node-consensus --lib -- tests::read_pre_generated_transactions --exact --nocapture --ignored
```
