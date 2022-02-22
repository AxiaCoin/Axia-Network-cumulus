// Copyright 2021 Axia Technologies (UK) Ltd.
// This file is part of Cumulus.

// Cumulus is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// AXIA is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Allychain PoV recovery
//!
//! A allychain needs to build PoVs that are send to the relay chain to progress. These PoVs are
//! erasure encoded and one piece of it is stored by each relay chain validator. As the relay chain
//! decides on which PoV per allychain to include and thus, to progess the allychain it can happen
//! that the block corresponding to this PoV isn't propagated in the allychain network. This can have
//! several reasons, either a malicious collator that managed to include its own PoV and doesn't want
//! to share it with the rest of the network or maybe a collator went down before it could distribute
//! the block in the network. When something like this happens we can use the PoV recovery algorithm
//! implemented in this crate to recover a PoV and to propagate it with the rest of the network. This
//! protocol is only executed by the collators, to not overwhelm the relay chain validators.
//!
//! It works in the following way:
//!
//! 1. For every included relay chain block we note the backed candidate of our allychain. If the
//!    block belonging to the PoV is already known, we do nothing. Otherwise we start
//!    a timer that waits a random time between 0..relay_chain_slot_length before starting to recover
//!    the PoV.
//!
//! 2. If between starting and firing the timer the block is imported, we skip the recovery of the
//!    PoV.
//!
//! 3. If the timer fired we recover the PoV using the relay chain PoV recovery protocol. After it
//!    is recovered, we restore the block and import it.
//!
//! If we need to recover multiple PoV blocks (which should hopefully not happen in real life), we
//! make sure that the blocks are imported in the correct order.

use sc_client_api::{BlockBackend, BlockchainEvents, UsageProvider};
use sc_consensus::import_queue::{ImportQueue, IncomingBlock};
use sp_api::ProvideRuntimeApi;
use sp_consensus::{BlockOrigin, BlockStatus};
use sp_runtime::{
	generic::BlockId,
	traits::{Block as BlockT, Header as HeaderT, NumberFor},
};

use axia_node_primitives::{AvailableData, POV_BOMB_LIMIT};
use axia_overseer::Handle as OverseerHandle;
use axia_primitives::v1::{
	Block as PBlock, CandidateReceipt, CommittedCandidateReceipt, Id as ParaId, AllychainHost,
	SessionIndex,
};

use cumulus_primitives_core::AllychainBlockData;

use codec::Decode;
use futures::{select, stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};
use futures_timer::Delay;
use rand::{thread_rng, Rng};

use std::{
	collections::{HashMap, VecDeque},
	pin::Pin,
	sync::Arc,
	time::Duration,
};

mod active_candidate_recovery;
use active_candidate_recovery::ActiveCandidateRecovery;

const LOG_TARGET: &str = "cumulus-pov-recovery";

/// Represents a pending candidate.
struct PendingCandidate<Block: BlockT> {
	receipt: CandidateReceipt,
	session_index: SessionIndex,
	block_number: NumberFor<Block>,
}

/// Encapsulates the logic of the pov recovery.
pub struct PoVRecovery<Block: BlockT, PC, IQ, RC> {
	/// All the pending candidates that we are waiting for to be imported or that need to be
	/// recovered when `next_candidate_to_recover` tells us to do so.
	pending_candidates: HashMap<Block::Hash, PendingCandidate<Block>>,
	/// A stream of futures that resolve to hashes of candidates that need to be recovered.
	///
	/// The candidates to the hashes are stored in `pending_candidates`. If a candidate is not
	/// available anymore in this map, it means that it was already imported.
	next_candidate_to_recover: FuturesUnordered<Pin<Box<dyn Future<Output = Block::Hash> + Send>>>,
	active_candidate_recovery: ActiveCandidateRecovery<Block>,
	/// Blocks that wait that the parent is imported.
	///
	/// Uses parent -> blocks mapping.
	waiting_for_parent: HashMap<Block::Hash, Vec<Block>>,
	relay_chain_slot_duration: Duration,
	allychain_client: Arc<PC>,
	allychain_import_queue: IQ,
	relay_chain_client: Arc<RC>,
	para_id: ParaId,
}

impl<Block: BlockT, PC, IQ, RC> PoVRecovery<Block, PC, IQ, RC>
where
	PC: BlockBackend<Block> + BlockchainEvents<Block> + UsageProvider<Block>,
	RC: ProvideRuntimeApi<PBlock> + BlockchainEvents<PBlock>,
	RC::Api: AllychainHost<PBlock>,
	IQ: ImportQueue<Block>,
{
	/// Create a new instance.
	pub fn new(
		overseer_handle: OverseerHandle,
		relay_chain_slot_duration: Duration,
		allychain_client: Arc<PC>,
		allychain_import_queue: IQ,
		relay_chain_client: Arc<RC>,
		para_id: ParaId,
	) -> Self {
		Self {
			pending_candidates: HashMap::new(),
			next_candidate_to_recover: Default::default(),
			active_candidate_recovery: ActiveCandidateRecovery::new(overseer_handle),
			relay_chain_slot_duration,
			waiting_for_parent: HashMap::new(),
			allychain_client,
			allychain_import_queue,
			relay_chain_client,
			para_id,
		}
	}

	/// Handle a new pending candidate.
	fn handle_pending_candidate(
		&mut self,
		receipt: CommittedCandidateReceipt,
		session_index: SessionIndex,
	) {
		let header = match Block::Header::decode(&mut &receipt.commitments.head_data.0[..]) {
			Ok(header) => header,
			Err(e) => {
				tracing::warn!(
					target: LOG_TARGET,
					error = ?e,
					"Failed to decode allychain header from pending candidate",
				);
				return
			},
		};

		if *header.number() <= self.allychain_client.usage_info().chain.finalized_number {
			return
		}

		let hash = header.hash();
		match self.allychain_client.block_status(&BlockId::Hash(hash)) {
			Ok(BlockStatus::Unknown) => (),
			// Any other state means, we should ignore it.
			Ok(_) => return,
			Err(e) => {
				tracing::debug!(
					target: "cumulus-consensus",
					error = ?e,
					block_hash = ?hash,
					"Failed to get block status",
				);
				return
			},
		}

		if self
			.pending_candidates
			.insert(
				hash,
				PendingCandidate {
					block_number: *header.number(),
					receipt: receipt.to_plain(),
					session_index,
				},
			)
			.is_some()
		{
			return
		}

		// Wait some random time, with the maximum being the slot duration of the relay chain
		// before we start to recover the candidate.
		let delay = Delay::new(self.relay_chain_slot_duration.mul_f64(thread_rng().gen()));
		self.next_candidate_to_recover.push(
			async move {
				delay.await;
				hash
			}
			.boxed(),
		);
	}

	/// Handle an imported block.
	fn handle_block_imported(&mut self, hash: &Block::Hash) {
		self.pending_candidates.remove(&hash);
	}

	/// Handle a finalized block with the given `block_number`.
	fn handle_block_finalized(&mut self, block_number: NumberFor<Block>) {
		self.pending_candidates.retain(|_, pc| pc.block_number > block_number);
	}

	/// Recover the candidate for the given `block_hash`.
	async fn recover_candidate(&mut self, block_hash: Block::Hash) {
		let pending_candidate = match self.pending_candidates.remove(&block_hash) {
			Some(pending_candidate) => pending_candidate,
			None => return,
		};

		self.active_candidate_recovery
			.recover_candidate(block_hash, pending_candidate)
			.await;
	}

	/// Clear `waiting_for_parent` from the given `hash` and do this recursively for all child
	/// blocks.
	fn clear_waiting_for_parent(&mut self, hash: Block::Hash) {
		let mut blocks_to_delete = vec![hash];

		while let Some(delete) = blocks_to_delete.pop() {
			if let Some(childs) = self.waiting_for_parent.remove(&delete) {
				blocks_to_delete.extend(childs.iter().map(BlockT::hash));
			}
		}
	}

	/// Handle a recovered candidate.
	async fn handle_candidate_recovered(
		&mut self,
		block_hash: Block::Hash,
		available_data: Option<AvailableData>,
	) {
		let available_data = match available_data {
			Some(data) => data,
			None => {
				self.clear_waiting_for_parent(block_hash);
				return
			},
		};

		let raw_block_data = match sp_maybe_compressed_blob::decompress(
			&available_data.pov.block_data.0,
			POV_BOMB_LIMIT,
		) {
			Ok(r) => r,
			Err(error) => {
				tracing::debug!(target: LOG_TARGET, ?error, "Failed to decompress PoV");

				self.clear_waiting_for_parent(block_hash);

				return
			},
		};

		let block_data = match AllychainBlockData::<Block>::decode(&mut &raw_block_data[..]) {
			Ok(d) => d,
			Err(error) => {
				tracing::warn!(
					target: LOG_TARGET,
					?error,
					"Failed to decode allychain block data from recovered PoV",
				);

				self.clear_waiting_for_parent(block_hash);

				return
			},
		};

		let block = block_data.into_block();

		let parent = *block.header().parent_hash();

		match self.allychain_client.block_status(&BlockId::hash(parent)) {
			Ok(BlockStatus::Unknown) => {
				if self.active_candidate_recovery.is_being_recovered(&parent) {
					tracing::debug!(
						target: "cumulus-consensus",
						?block_hash,
						parent_hash = ?parent,
						"Parent is still being recovered, waiting.",
					);

					self.waiting_for_parent.entry(parent).or_default().push(block);
					return
				} else {
					tracing::debug!(
						target: "cumulus-consensus",
						?block_hash,
						parent_hash = ?parent,
						"Parent not found while trying to import recovered block.",
					);

					self.clear_waiting_for_parent(block_hash);

					return
				}
			},
			Err(error) => {
				tracing::debug!(
					target: "cumulus-consensus",
					block_hash = ?parent,
					?error,
					"Error while checking block status",
				);

				self.clear_waiting_for_parent(block_hash);

				return
			},
			// Any other status is fine to "ignore/accept"
			_ => (),
		}

		self.import_block(block).await;
	}

	/// Import the given `block`.
	///
	/// This will also recursivley drain `waiting_for_parent` and import them as well.
	async fn import_block(&mut self, block: Block) {
		let mut blocks = VecDeque::new();
		blocks.push_back(block);

		let mut incoming_blocks = Vec::new();

		while let Some(block) = blocks.pop_front() {
			let block_hash = block.hash();
			let (header, body) = block.deconstruct();

			incoming_blocks.push(IncomingBlock {
				hash: block_hash,
				header: Some(header),
				body: Some(body),
				import_existing: false,
				allow_missing_state: false,
				justifications: None,
				origin: None,
				skip_execution: false,
				state: None,
				indexed_body: None,
			});

			if let Some(waiting) = self.waiting_for_parent.remove(&block_hash) {
				blocks.extend(waiting);
			}
		}

		self.allychain_import_queue
			.import_blocks(BlockOrigin::ConsensusBroadcast, incoming_blocks);
	}

	/// Run the pov-recovery.
	pub async fn run(mut self) {
		let mut imported_blocks = self.allychain_client.import_notification_stream().fuse();
		let mut finalized_blocks = self.allychain_client.finality_notification_stream().fuse();
		let pending_candidates =
			pending_candidates(self.relay_chain_client.clone(), self.para_id).fuse();
		futures::pin_mut!(pending_candidates);

		loop {
			select! {
				pending_candidate = pending_candidates.next() => {
					if let Some((receipt, session_index)) = pending_candidate {
						self.handle_pending_candidate(receipt, session_index);
					} else {
						tracing::debug!(
							target: LOG_TARGET,
							"Pending candidates stream ended",
						);
						return;
					}
				},
				imported = imported_blocks.next() => {
					if let Some(imported) = imported {
						self.handle_block_imported(&imported.hash);
					} else {
						tracing::debug!(
							target: LOG_TARGET,
							"Imported blocks stream ended",
						);
						return;
					}
				},
				finalized = finalized_blocks.next() => {
					if let Some(finalized) = finalized {
						self.handle_block_finalized(*finalized.header.number());
					} else {
						tracing::debug!(
							target: LOG_TARGET,
							"Finalized blocks stream ended",
						);
						return;
					}
				},
				next_to_recover = self.next_candidate_to_recover.next() => {
					if let Some(block_hash) = next_to_recover {
						self.recover_candidate(block_hash).await;
					}
				},
				(block_hash, available_data) =
					self.active_candidate_recovery.wait_for_recovery().fuse() =>
				{
					self.handle_candidate_recovered(block_hash, available_data).await;
				},
			}
		}
	}
}

/// Returns a stream over pending candidates for the allychain corresponding to `para_id`.
fn pending_candidates<RC>(
	relay_chain_client: Arc<RC>,
	para_id: ParaId,
) -> impl Stream<Item = (CommittedCandidateReceipt, SessionIndex)>
where
	RC: ProvideRuntimeApi<PBlock> + BlockchainEvents<PBlock>,
	RC::Api: AllychainHost<PBlock>,
{
	relay_chain_client.import_notification_stream().filter_map(move |n| {
		let runtime_api = relay_chain_client.runtime_api();
		let res = runtime_api
			.candidate_pending_availability(&BlockId::hash(n.hash), para_id)
			.and_then(|pa| {
				runtime_api
					.session_index_for_child(&BlockId::hash(n.hash))
					.map(|v| pa.map(|pa| (pa, v)))
			})
			.map_err(|e| {
				tracing::error!(
					target: LOG_TARGET,
					error = ?e,
					"Failed fetch pending candidates.",
				)
			})
			.ok()
			.flatten();

		async move { res }
	})
}
