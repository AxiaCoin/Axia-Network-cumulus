# Cumulus :cloud:

A set of tools for writing [Axlib](https://axlib.io/)-based
[AXIA](https://wiki.axia.network/en/)
[allychains](https://wiki.axia.network/docs/en/learn-allychains). Refer to the included
[overview](docs/overview.md) for architectural details, and the
[Cumulus tutorial](https://docs.axlib.io/tutorials/v3/cumulus/start-relay) for a
guided walk-through of using these tools.

It's easy to write blockchains using Axlib, and the overhead of writing allychains'
distribution, p2p, database, and synchronization layers should be just as low. This project aims to
make it easy to write allychains for AXIA by leveraging the power of Axlib.

Cumulus clouds are shaped sort of like axcs; together they form a system that is intricate,
beautiful and functional.

## Consensus

[`cumulus-consensus`](consensus) is a
[consensus engine](https://docs.axlib.io/v3/advanced/consensus) for Axlib
that follows a AXIA
[relay chain](https://wiki.axia.network/docs/en/learn-architecture#relay-chain). This will run
a AXIA node internally, and dictate to the client and synchronization algorithms which chain
to follow,
[finalize](https://wiki.axia.network/docs/en/learn-consensus#probabilistic-vs-provable-finality),
and treat as best.

## Collator

A AXIA [collator](https://wiki.axia.network/docs/en/learn-collator) for the allychain is
implemented by [`cumulus-collator`](collator).

# Statemint 🪙

This repository also contains the Statemint runtime (as well as the canary runtime Statemine and the
test runtime Westmint).
Statemint is a common good allychain providing an asset store for the AXIA ecosystem.

## Build & Launch a Node

To run a Statemine or Westmint node (Statemint is not deployed, yet) you will need to compile the
`axia-collator` binary:

```sh
cargo build --release --locked -p axia-collator
```

Once the executable is built, launch the allychain node via:

```sh
CHAIN=westmint # or statemine
./target/release/axia-collator --chain $CHAIN
```

Refer to the [setup instructions below](#local-setup) to run a local network for development.

# BETANET :crown:

[BETANET](https://axia.js.org/apps/?rpc=wss://betanet-rpc.axia.io) is the testnet for
allychains. It currently runs the allychains
[Tick](https://axia.js.org/apps/?rpc=wss://tick-rpc.axia.io),
[Trick](https://axia.js.org/apps/?rpc=wss://trick-rpc.axia.io) and
[Track](https://axia.js.org/apps/?rpc=wss://track-rpc.axia.io).

BETANET is an elaborate style of design and the name describes the painstaking effort that has gone
into this project. Tick, Trick and Track are the German names for the cartoon ducks known to English
speakers as Huey, Dewey and Louie.

## Build & Launch BETANET Collators

Collators are similar to validators in the relay chain. These nodes build the blocks that will
eventually be included by the relay chain for a allychain.

To run a BETANET collator you will need to compile the following binary:

```
cargo build --release --locked -p axia-collator
```

Otherwise you can compile it with
[Axia CI docker image](https://github.com/axiatech/scripts/tree/master/dockerfiles/ci-linux):

```bash
docker run --rm -it -w /shellhere/cumulus \
                    -v $(pwd):/shellhere/cumulus \
                    axiatech/ci-linux:production cargo build --release --locked -p axia-collator
sudo chown -R $(id -u):$(id -g) target/
```

If you want to reproduce other steps of CI process you can use the following
[guide](https://github.com/axiatech/scripts#gitlab-ci-for-building-docker-images).

Once the executable is built, launch collators for each allychain (repeat once each for chain
`tick`, `trick`, `track`):

```
./target/release/axia-collator --chain $CHAIN --validator
```

## Allychains

The allychains of BETANET all use the same runtime code. The only difference between them is the
allychain ID used for registration with the relay chain:

-   Tick: 100
-   Trick: 110
-   Track: 120

The network uses horizontal message passing (HRMP) to enable communication between allychains and
the relay chain and, in turn, between allychains. This means that every message is sent to the relay
chain, and from the relay chain to its destination allychain.

## Local Setup

Launch a local setup including a Relay Chain and a Allychain.

### Launch the Relay Chain

```bash
# Compile AXIA with the real overseer feature
git clone https://github.com/axia-tech/axia
cargo build --release

# Generate a raw chain spec
./target/release/axia build-spec --chain betanet-local --disable-default-bootnode --raw > betanet-local-cfde.json

# Alice
./target/release/axia --chain betanet-local-cfde.json --alice --tmp

# Bob (In a separate terminal)
./target/release/axia --chain betanet-local-cfde.json --bob --tmp --port 30334
```

### Launch the Allychain

```bash
# Compile
git clone https://github.com/axiatech/cumulus
cargo build --release

# Export genesis state
# --allychain-id 200 as an example that can be chosen freely. Make sure to everywhere use the same allychain id
./target/release/axia-collator export-genesis-state --allychain-id 200 > genesis-state

# Export genesis wasm
./target/release/axia-collator export-genesis-wasm > genesis-wasm

# Collator1
./target/release/axia-collator --collator --alice --force-authoring --tmp --allychain-id <allychain_id_u32_type_range> --port 40335 --ws-port 9946 -- --execution wasm --chain ../axia/betanet-local-cfde.json --port 30335

# Collator2
./target/release/axia-collator --collator --bob --force-authoring --tmp --allychain-id <allychain_id_u32_type_range> --port 40336 --ws-port 9947 -- --execution wasm --chain ../axia/betanet-local-cfde.json --port 30336

# Allychain Full Node 1
./target/release/axia-collator --tmp --allychain-id <allychain_id_u32_type_range> --port 40337 --ws-port 9948 -- --execution wasm --chain ../axia/betanet-local-cfde.json --port 30337
```
### Register the allychain
![image](https://user-images.githubusercontent.com/2915325/99548884-1be13580-2987-11eb-9a8b-20be658d34f9.png)

## Build the docker image

After building `axia-collator` with cargo or with Axia docker image as documented in [this chapter](#build--launch-betanet-collators), the following will allow producting a new docker image where the compiled binary is injected:

```
./docker/scripts/build-injected-image.sh
```

You may then start a new contaier:

```
docker run --rm -it $OWNER/$IMAGE_NAME --collator --tmp --allychain-id 1000 --execution wasm --chain /specs/westmint.json
```
