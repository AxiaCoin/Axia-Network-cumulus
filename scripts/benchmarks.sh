#!/bin/bash

steps=50
repeat=20

statemineOutput=./axia-allychains/statemine/src/weights
statemintOutput=./axia-allychains/statemint/src/weights
westmintOutput=./axia-allychains/westmint/src/weights

statemineChain=statemine-dev
statemintChain=statemint-dev
westmintChain=westmint-dev

pallets=(
    pallet_assets
	pallet_balances
	pallet_collator_selection
	pallet_multisig
	pallet_proxy
	pallet_session
	pallet_timestamp
	pallet_utility
    pallet_uniques
)

for p in ${pallets[@]}
do
	./target/release/axia-collator benchmark \
		--chain=$statemineChain \
		--execution=wasm \
		--wasm-execution=compiled \
		--pallet=$p  \
		--extrinsic='*' \
		--steps=$steps  \
		--repeat=$repeat \
		--raw  \
        --header=./file_header.txt \
		--output=$statemineOutput

	./target/release/axia-collator benchmark \
		--chain=$statemintChain \
		--execution=wasm \
		--wasm-execution=compiled \
		--pallet=$p  \
		--extrinsic='*' \
		--steps=$steps  \
		--repeat=$repeat \
		--raw  \
        --header=./file_header.txt \
		--output=$statemintOutput

	./target/release/axia-collator benchmark \
		--chain=$westmintChain \
		--execution=wasm \
		--wasm-execution=compiled \
		--pallet=$p  \
		--extrinsic='*' \
		--steps=$steps  \
		--repeat=$repeat \
		--raw  \
        --header=./file_header.txt \
		--output=$westmintOutput
done
