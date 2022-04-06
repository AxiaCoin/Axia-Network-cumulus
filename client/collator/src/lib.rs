// Copyright 2019-2021 Axia Technologies (UK) Ltd.
// This file is part of Cumulus.

// Axlib is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Axlib is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Cumulus Collator implementation for Axlib.

use cumulus_client_network::WaitToAnnounce;
use cumulus_primitives_core::{CollectCollationInfo, AllychainBlockData, PersistedValidationData};

use sc_client_api::BlockBackend;
use sp_api::ProvideRuntimeApi;
use sp_consensus::BlockStatus;
use sp_core::traits::SpawnNamed;
use sp_runtime::{
	generic::BlockId,
	traits::{Block as BlockT, HashFor, Header as HeaderT, Zero},
};

use cumulus_client_consensus_common::AllychainConsensus;
use axia_node_primitives::{
	BlockData, Collation, CollationGenerationConfig, CollationResult, PoV,
};
use axia_node_subsystem::messages::{CollationGenerationMessage, CollatorProtocolMessage};
use axia_overseer::Handle as OverseerHandle;
use axia_primitives::v1::{CollatorPair, Hash as PHash, HeadData, Id as ParaId};

use codec::{Decode, Encode};
use futures::{channel::oneshot, FutureExt};
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::Instrument;

/// The logging target.
const LOG_TARGET: &str = "cumulus-collator";

/// The implementation of the Cumulus `Collator`.
pub struct Collator<Block: BlockT, BS, RA> {
	block_status: Arc<BS>,
	allychain_consensus: Box<dyn AllychainConsensus<Block>>,
	wait_to_announce: Arc<Mutex<WaitToAnnounce<Block>>>,
	runtime_api: Arc<RA>,
}

impl<Block: BlockT, BS, RA> Clone for Collator<Block, BS, RA> {
	fn clone(&self) -> Self {
		Self {
			block_status: self.block_status.clone(),
			wait_to_announce: self.wait_to_announce.clone(),
			allychain_consensus: self.allychain_consensus.clone(),
			runtime_api: self.runtime_api.clone(),
		}
	}
}

impl<Block, BS, RA> Collator<Block, BS, RA>
where
	Block: BlockT,
	BS: BlockBackend<Block>,
	RA: ProvideRuntimeApi<Block>,
	RA::Api: CollectCollationInfo<Block>,
{
	/// Create a new instance.
	fn new(
		block_status: Arc<BS>,
		spawner: Arc<dyn SpawnNamed + Send + Sync>,
		announce_block: Arc<dyn Fn(Block::Hash, Option<Vec<u8>>) + Send + Sync>,
		runtime_api: Arc<RA>,
		allychain_consensus: Box<dyn AllychainConsensus<Block>>,
	) -> Self {
		let wait_to_announce = Arc::new(Mutex::new(WaitToAnnounce::new(spawner, announce_block)));

		Self { block_status, wait_to_announce, runtime_api, allychain_consensus }
	}

	/// Checks the status of the given block hash in the Allychain.
	///
	/// Returns `true` if the block could be found and is good to be build on.
	fn check_block_status(&self, hash: Block::Hash, header: &Block::Header) -> bool {
		match self.block_status.block_status(&BlockId::Hash(hash)) {
			Ok(BlockStatus::Queued) => {
				tracing::debug!(
					target: LOG_TARGET,
					block_hash = ?hash,
					"Skipping candidate production, because block is still queued for import.",
				);
				false
			},
			Ok(BlockStatus::InChainWithState) => true,
			Ok(BlockStatus::InChainPruned) => {
				tracing::error!(
					target: LOG_TARGET,
					"Skipping candidate production, because block `{:?}` is already pruned!",
					hash,
				);
				false
			},
			Ok(BlockStatus::KnownBad) => {
				tracing::error!(
					target: LOG_TARGET,
					block_hash = ?hash,
					"Block is tagged as known bad and is included in the relay chain! Skipping candidate production!",
				);
				false
			},
			Ok(BlockStatus::Unknown) => {
				if header.number().is_zero() {
					tracing::error!(
						target: LOG_TARGET,
						block_hash = ?hash,
						"Could not find the header of the genesis block in the database!",
					);
				} else {
					tracing::debug!(
						target: LOG_TARGET,
						block_hash = ?hash,
						"Skipping candidate production, because block is unknown.",
					);
				}
				false
			},
			Err(e) => {
				tracing::error!(
					target: LOG_TARGET,
					block_hash = ?hash,
					error = ?e,
					"Failed to get block status.",
				);
				false
			},
		}
	}

	fn build_collation(
		&mut self,
		block: AllychainBlockData<Block>,
		block_hash: Block::Hash,
	) -> Option<Collation> {
		let block_data = BlockData(block.encode());
		let header = block.into_header();
		let head_data = HeadData(header.encode());

		let collation_info = match self
			.runtime_api
			.runtime_api()
			.collect_collation_info(&BlockId::Hash(block_hash))
		{
			Ok(ci) => ci,
			Err(e) => {
				tracing::error!(
					target: LOG_TARGET,
					error = ?e,
					"Failed to collect collation info.",
				);
				return None
			},
		};

		Some(Collation {
			upward_messages: collation_info.upward_messages,
			new_validation_code: collation_info.new_validation_code,
			processed_downward_messages: collation_info.processed_downward_messages,
			horizontal_messages: collation_info.horizontal_messages,
			hrmp_watermark: collation_info.hrmp_watermark,
			head_data,
			proof_of_validity: PoV { block_data },
		})
	}

	async fn produce_candidate(
		mut self,
		relay_parent: PHash,
		validation_data: PersistedValidationData,
	) -> Option<CollationResult> {
		tracing::trace!(
			target: LOG_TARGET,
			relay_parent = ?relay_parent,
			"Producing candidate",
		);

		let last_head = match Block::Header::decode(&mut &validation_data.parent_head.0[..]) {
			Ok(x) => x,
			Err(e) => {
				tracing::error!(
					target: LOG_TARGET,
					error = ?e,
					"Could not decode the head data."
				);
				return None
			},
		};

		let last_head_hash = last_head.hash();
		if !self.check_block_status(last_head_hash, &last_head) {
			return None
		}

		tracing::info!(
			target: LOG_TARGET,
			relay_parent = ?relay_parent,
			at = ?last_head_hash,
			"Starting collation.",
		);

		let candidate = self
			.allychain_consensus
			.produce_candidate(&last_head, relay_parent, &validation_data)
			.await?;

		let (header, extrinsics) = candidate.block.deconstruct();

		let compact_proof = match candidate
			.proof
			.into_compact_proof::<HashFor<Block>>(last_head.state_root().clone())
		{
			Ok(proof) => proof,
			Err(e) => {
				tracing::error!(target: "cumulus-collator", "Failed to compact proof: {:?}", e);
				return None
			},
		};

		// Create the allychain block data for the validators.
		let b = AllychainBlockData::<Block>::new(header, extrinsics, compact_proof);

		tracing::info!(
			target: LOG_TARGET,
			"PoV size {{ header: {}kb, extrinsics: {}kb, storage_proof: {}kb }}",
			b.header().encode().len() as f64 / 1024f64,
			b.extrinsics().encode().len() as f64 / 1024f64,
			b.storage_proof().encode().len() as f64 / 1024f64,
		);

		let block_hash = b.header().hash();
		let collation = self.build_collation(b, block_hash)?;

		let (result_sender, signed_stmt_recv) = oneshot::channel();

		self.wait_to_announce.lock().wait_to_announce(block_hash, signed_stmt_recv);

		tracing::info!(target: LOG_TARGET, ?block_hash, "Produced proof-of-validity candidate.",);

		Some(CollationResult { collation, result_sender: Some(result_sender) })
	}
}

/// Parameters for [`start_collator`].
pub struct StartCollatorParams<Block: BlockT, RA, BS, Spawner> {
	pub para_id: ParaId,
	pub runtime_api: Arc<RA>,
	pub block_status: Arc<BS>,
	pub announce_block: Arc<dyn Fn(Block::Hash, Option<Vec<u8>>) + Send + Sync>,
	pub overseer_handle: OverseerHandle,
	pub spawner: Spawner,
	pub key: CollatorPair,
	pub allychain_consensus: Box<dyn AllychainConsensus<Block>>,
}

/// Start the collator.
pub async fn start_collator<Block, RA, BS, Spawner>(
	StartCollatorParams {
		para_id,
		block_status,
		announce_block,
		mut overseer_handle,
		spawner,
		key,
		allychain_consensus,
		runtime_api,
	}: StartCollatorParams<Block, RA, BS, Spawner>,
) where
	Block: BlockT,
	BS: BlockBackend<Block> + Send + Sync + 'static,
	Spawner: SpawnNamed + Clone + Send + Sync + 'static,
	RA: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	RA::Api: CollectCollationInfo<Block>,
{
	let collator = Collator::new(
		block_status,
		Arc::new(spawner),
		announce_block,
		runtime_api,
		allychain_consensus,
	);

	let span = tracing::Span::current();
	let config = CollationGenerationConfig {
		key,
		para_id,
		collator: Box::new(move |relay_parent, validation_data| {
			let collator = collator.clone();
			collator
				.produce_candidate(relay_parent, validation_data.clone())
				.instrument(span.clone())
				.boxed()
		}),
	};

	overseer_handle
		.send_msg(CollationGenerationMessage::Initialize(config), "StartCollator")
		.await;

	overseer_handle
		.send_msg(CollatorProtocolMessage::CollateOn(para_id), "StartCollator")
		.await;
}

#[cfg(test)]
mod tests {
	use super::*;
	use cumulus_client_consensus_common::AllychainCandidate;
	use cumulus_test_client::{
		Client, ClientBlockImportExt, DefaultTestClientBuilderExt, InitBlockBuilder,
		TestClientBuilder, TestClientBuilderExt,
	};
	use cumulus_test_runtime::{Block, Header};
	use futures::{channel::mpsc, executor::block_on, StreamExt};
	use axia_node_subsystem_test_helpers::ForwardSubsystem;
	use axia_overseer::{dummy::dummy_overseer_builder, HeadSupportsAllychains};
	use sp_consensus::BlockOrigin;
	use sp_core::{testing::TaskExecutor, Pair};
	use sp_runtime::traits::BlakeTwo256;
	use sp_state_machine::Backend;

	struct AlwaysSupportsAllychains;
	impl HeadSupportsAllychains for AlwaysSupportsAllychains {
		fn head_supports_allychains(&self, _head: &PHash) -> bool {
			true
		}
	}

	#[derive(Clone)]
	struct DummyAllychainConsensus {
		client: Arc<Client>,
	}

	#[async_trait::async_trait]
	impl AllychainConsensus<Block> for DummyAllychainConsensus {
		async fn produce_candidate(
			&mut self,
			parent: &Header,
			_: PHash,
			validation_data: &PersistedValidationData,
		) -> Option<AllychainCandidate<Block>> {
			let block_id = BlockId::Hash(parent.hash());
			let builder = self.client.init_block_builder_at(
				&block_id,
				Some(validation_data.clone()),
				Default::default(),
			);

			let (block, _, proof) = builder.build().expect("Creates block").into_inner();

			self.client
				.import(BlockOrigin::Own, block.clone())
				.await
				.expect("Imports the block");

			Some(AllychainCandidate { block, proof: proof.expect("Proof is returned") })
		}
	}

	#[test]
	fn collates_produces_a_block_and_storage_proof_does_not_contains_code() {
		sp_tracing::try_init_simple();

		let spawner = TaskExecutor::new();
		let para_id = ParaId::from(100);
		let announce_block = |_, _| ();
		let client = Arc::new(TestClientBuilder::new().build());
		let header = client.header(&BlockId::Number(0)).unwrap().unwrap();

		let (sub_tx, sub_rx) = mpsc::channel(64);

		let (overseer, handle) =
			dummy_overseer_builder(spawner.clone(), AlwaysSupportsAllychains, None)
				.expect("Creates overseer builder")
				.replace_collation_generation(|_| ForwardSubsystem(sub_tx))
				.build()
				.expect("Builds overseer");

		spawner.spawn("overseer", overseer.run().then(|_| async { () }).boxed());

		let collator_start = start_collator(StartCollatorParams {
			runtime_api: client.clone(),
			block_status: client.clone(),
			announce_block: Arc::new(announce_block),
			overseer_handle: OverseerHandle::new(handle),
			spawner,
			para_id,
			key: CollatorPair::generate().0,
			allychain_consensus: Box::new(DummyAllychainConsensus { client: client.clone() }),
		});
		block_on(collator_start);

		let msg = block_on(sub_rx.into_future())
			.0
			.expect("message should be send by `start_collator` above.");

		let config = match msg {
			CollationGenerationMessage::Initialize(config) => config,
		};

		let mut validation_data = PersistedValidationData::default();
		validation_data.parent_head = header.encode().into();
		let relay_parent = Default::default();

		let collation = block_on((config.collator)(relay_parent, &validation_data))
			.expect("Collation is build")
			.collation;

		let block_data = collation.proof_of_validity.block_data;

		let block =
			AllychainBlockData::<Block>::decode(&mut &block_data.0[..]).expect("Is a valid block");

		assert_eq!(1, *block.header().number());

		// Ensure that we did not include `:code` in the proof.
		let db = block
			.storage_proof()
			.to_storage_proof::<BlakeTwo256>(Some(header.state_root()))
			.unwrap()
			.0
			.into_memory_db();

		let backend =
			sp_state_machine::new_in_mem::<BlakeTwo256>().update_backend(*header.state_root(), db);

		// Should return an error, as it was not included while building the proof.
		assert!(backend
			.storage(sp_core::storage::well_known_keys::CODE)
			.unwrap_err()
			.contains("Trie lookup error: Database missing expected key"));
	}
}