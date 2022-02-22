# Cumulus Overview

This document provides high-level documentation for Cumulus.

## Runtime

Each Axlib blockchain provides a runtime. The runtime is the state transition function of the
blockchain. Cumulus provides interfaces and extensions to convert a Axlib FRAME runtime into a
Allychain runtime. AXIA expects each runtime exposes an interface for validating a
Allychain's state transition and also provides interfaces for the Allychain to send and receive
messages of other Allychains.

To convert a Axlib runtime into a Allychain runtime, the following code needs to be added to the
runtime:
```rust
cumulus_pallet_allychain_system::register_validate_block!(Block, Executive);
```

This macro call expects the `Block` and `Executive` type. It generates the `validate_block` function
that is expected by AXIA to validate the state transition.

When compiling a runtime that uses Cumulus, a WASM binary is generated that contains the *full* code
of the Allychain runtime plus the `validate_block` functionality. This binary is required to
register a Allychain on the relay chain.

When the Allychain validator calls the `validate_block` function, it passes the PoVBlock (See [Block
building](#block-building) for more information) and the parent header of the Allychain that is
stored on the relay chain. From the PoVBlock witness data, Cumulus reconstructs the partial trie.
This partial trie is used as storage while executing the block. Cumulus also redirects all storage
related host functions to use the witness data storage. After the setup is done, Cumulus calls
`execute_block` with the transactions and the header stored in the PoVBlock. On success, the new
Allychain header is returned as part of the `validate_block` result.

## Node

Allychains support light-clients, full nodes, and authority nodes. Authority nodes are called
Collators in the AXIA ecosystem. Cumulus provides the consensus implementation for a
Allychain and the block production logic.

The Allychain consensus will follow the relay chain to get notified about which Allychain blocks are
included in the relay-chain and which are finalized. Each block that is built by a Collator is sent
to a validator that is assigned to the particular Allychain. Cumulus provides the block production
logic that notifies each Collator of the Allychain to build a Allychain block. The
notification is triggered on a relay-chain block import by the Collator. This means that every
Collator of the Allychain can send a block to the Allychain validators. For more sophisticated
authoring logic, the Allychain will be able to use Aura, BABE, etc. (Not supported at the moment)

A Allychain Collator will join the Allychain network and the relay-chain network. The Allychain
network will be used to gossip Allychain blocks and to gossip transactions. Collators will only
gossip blocks to the Allychain network that have a high chance of being included in the relay
chain. To prove that a block is probably going to be included, the Collator will send along side
the notification the so-called candidate message. This candidate message is issued by a Allychain
validator after approving a block. This proof of possible inclusion prevents spamming other collators
of the network with useless blocks.
The Collator joins the relay-chain network for two reasons. First, the Collator uses it to send the
Allychain blocks to the Allychain validators. Secondly, the Collator participates as light/full-node
of the relay chain to be informed of new relay-chain blocks. This information will be used for the
consensus and the block production logic.

## Block Building

AXIA requires that a Allychain block is transmitted in a fixed format. These blocks sent by a
Allychain to the Allychain validators are called proof-of-validity blocks (PoVBlock). Such a
PoVBlock contains the header and the transactions of the Allychain as opaque blobs (`Vec<u8>`). They
are opaque, because AXIA can not and should not support all kinds of possible Allychain block
formats. Besides the header and the transactions, it also contains the witness data and the outgoing
messages.

A Allychain validator needs to validate a given PoVBlock, but without requiring the full state of
the Allychain. To still make it possible to validate the Allychain block, the PoVBlock contains the
witness data. The witness data is a proof that is collected while building the block. The proof will
contain all trie nodes that are read during the block production. Cumulus uses the witness data to
reconstruct a partial trie and uses this a storage when executing the block.

The outgoing messages are also collected at block production. These are messages from the Allychain
the block is built for to other Allychains or to the relay chain itself.

## Runtime Upgrade

Every Axlib blockchain supports runtime upgrades. Runtime upgrades enable a blockchain to update
its state transition function without requiring any client update. Such a runtime upgrade is applied
by a special transaction in a Axlib runtime. AXIA and Cumulus provide support for these
runtime upgrades, but updating a Allychain runtime is not as easy as updating a standalone
blockchain runtime. In a standalone blockchain, the special transaction needs to be included in a
block and the runtime is updated.

A Allychain will follow the same paradigm, but the relay chain needs to be informed before
the update. Cumulus will provide functionality to notify the relay chain about the runtime update. The
update will not be enacted directly; instead it takes `X` relay blocks (a value that is configured
by the relay chain) before the relay chain allows the update to be applied. The first Allychain
block that will be included after `X` relay chain blocks needs to apply the upgrade.
If the update is applied before the waiting period is finished, the relay chain will reject the
Allychain block for inclusion. The Cumulus runtime pallet will provide the functionality to
register the runtime upgrade and will also make sure that the update is applied at the correct block.

After updating the Allychain runtime, a Allychain needs to wait a certain amount of time `Y`
(configured by the relay chain) before another update can be applied.

The WASM blob update not only contains the Allychain runtime, but also the `validate_block`
function provided by Cumulus. So, updating a Allychain runtime on the relay chain involves a
complete update of the validation WASM blob.
