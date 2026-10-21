// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-datapipeline-datapipeline service.
//!
//! The pipeline control plane lives here: every pipeline a caller created, the
//! `uniqueId` claims that make `CreatePipeline` idempotent, and the tombstones that
//! let a deleted pipeline id keep answering `PipelineDeletedException` instead of
//! silently becoming unknown. Handlers compute every response from this store; the
//! wire shapes themselves (`PipelineObject`, `Tag`, …) are reused as the stored
//! representation so a definition read back by `GetPipelineDefinition` is byte-for-byte
//! what `PutPipelineDefinition` committed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{ParameterObject, ParameterValue, PipelineObject, Tag};

/// A pipeline's `@pipelineState` metadata field.
///
/// A freshly created pipeline is `PENDING`; it only leaves that state through
/// `ActivatePipeline` / `DeactivatePipeline`, which are still scaffolded (HTTP 501), so
/// nothing in this crate yet constructs the other variants. They are modelled here
/// because the state machine is a property of the resource, not of the operation that
/// moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    /// Created, with no activated definition.
    Pending,
    /// Activated: the committed definition is scheduled to run.
    Scheduled,
    /// `DeactivatePipeline` was called and is still draining active runs.
    Deactivating,
    /// Deactivated.
    Inactive,
}

impl PipelineState {
    /// The value this state takes in the `@pipelineState` field of a
    /// `PipelineDescription` (see the `DescribePipelines` response example in the
    /// Smithy model's documentation, which shows `"@pipelineState": "SCHEDULED"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Scheduled => "SCHEDULED",
            Self::Deactivating => "DEACTIVATING",
            Self::Inactive => "INACTIVE",
        }
    }
}

/// One committed pipeline definition — exactly what `PutPipelineDefinition` stored and
/// what `GetPipelineDefinition` reads back.
#[derive(Debug, Clone, Default)]
pub struct PipelineDefinition {
    pub pipeline_objects: Vec<PipelineObject>,
    pub parameter_objects: Vec<ParameterObject>,
    pub parameter_values: Vec<ParameterValue>,
}

/// A persisted pipeline.
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub pipeline_id: String,
    pub name: String,
    pub unique_id: String,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    pub state: PipelineState,
    pub created_at: SystemTime,
    /// Monotonic creation order. `ListPipelines` pages on this rather than on an offset
    /// into the live set, so deleting a pipeline mid-pagination cannot make the next page
    /// skip or repeat a pipeline.
    pub seq: u64,
    /// How many definitions have been committed to this pipeline. Zero until the first
    /// `PutPipelineDefinition`, and the `@version` a `PipelineDescription` reports.
    pub definition_version: u64,
    pub definition: Option<PipelineDefinition>,
}

impl Pipeline {
    /// The `@version` metadata field: how many definitions have been committed to this
    /// pipeline, so it is 1 after the first `PutPipelineDefinition` (the
    /// `DescribePipelines` example in the model shows `"@version": "1"` for a pipeline
    /// that has a definition) and 0 for a pipeline that has none.
    #[must_use]
    pub fn reported_version(&self) -> String {
        self.definition_version.to_string()
    }
}

/// Which pipeline a `uniqueId` is already claimed by.
///
/// `CreatePipeline`'s documented contract is keyed on the `uniqueId`, not on the pair:
/// the same `(name, uniqueId)` returns the previously assigned `pipelineId`, while the
/// same `uniqueId` under a different name is refused. Both decisions need the name the
/// claim was made under, so it is stored beside the id.
#[derive(Debug, Clone)]
pub struct UniqueIdClaim {
    pub name: String,
    pub pipeline_id: String,
}

/// Why a pipeline id did not resolve to a live pipeline.
///
/// Handlers map this onto whichever of their own modelled errors applies, so the
/// resolution rule lives in one place while each operation keeps the error set its model
/// declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineLookupError {
    /// No pipeline with this id was ever created here.
    Unknown,
    /// The id named a pipeline that has since been deleted (a tombstone).
    Deleted,
}

impl PipelineLookupError {
    /// Wording for the modelled error this lookup failure becomes. The HTTP status and
    /// the error code are what a client branches on; this text only has to name the
    /// pipeline the caller asked for.
    #[must_use]
    pub fn message(self, pipeline_id: &str) -> String {
        match self {
            Self::Unknown => format!("The specified pipeline was not found: {pipeline_id}"),
            Self::Deleted => format!("The specified pipeline has been deleted: {pipeline_id}"),
        }
    }
}

/// Per-service state for com-amazonaws-datapipeline-datapipeline. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatapipelineDatapipelineState {
    /// Live pipelines, keyed by the `pipelineId` this emulator assigned.
    pub pipelines: HashMap<String, Pipeline>,
    /// `uniqueId` -> the pipeline it is claimed by, for `CreatePipeline` idempotency.
    pub unique_ids: HashMap<String, UniqueIdClaim>,
    /// Ids of pipelines that were deleted, so they answer `PipelineDeletedException`
    /// rather than `PipelineNotFoundException`. Bounded by [`MAX_TOMBSTONES`].
    pub deleted: HashSet<String>,
    /// The order [`Self::deleted`] was filled in, so the oldest tombstone is the one
    /// evicted once the set is full.
    pub deleted_order: VecDeque<String>,
    /// The creation counter both the next `pipelineId` and the next `seq` come from.
    pub next_pipeline_seq: u64,
}

/// How many deleted pipeline ids are remembered.
///
/// The tombstone set is what lets a deleted id answer `PipelineDeletedException`, but it
/// grows once per `DeletePipeline` and nothing ever releases an entry, so a create/delete
/// loop would grow the store without limit. It is therefore capped, with the oldest
/// tombstone evicted first: an evicted id answers `PipelineNotFoundException` instead,
/// which is the same HTTP 400 refusal naming an id that no longer resolves.
pub const MAX_TOMBSTONES: usize = 1024;

/// How many pipelines may exist at once.
///
/// Source: the AWS Data Pipeline service limits ("Number of pipelines: 100" per account).
/// `CreatePipeline` refuses past this, which is also what bounds `unique_ids`, since a
/// claim exists only while the pipeline that took it does.
pub const MAX_PIPELINES: usize = 100;

impl ComAmazonawsDatapipelineDatapipelineState {
    /// Assign the next `pipelineId` and its creation sequence number.
    ///
    /// The caller is expected to insert a [`Pipeline`] under the returned id while still
    /// holding the state lock; the counter has already advanced, so two concurrent
    /// creates can never be handed the same id.
    pub fn mint_pipeline_id(&mut self) -> (String, u64) {
        self.next_pipeline_seq += 1;
        let seq = self.next_pipeline_seq;
        (mint_pipeline_id(seq), seq)
    }

    /// Resolve a live pipeline, distinguishing "never existed" from "deleted".
    pub fn pipeline(&self, pipeline_id: &str) -> Result<&Pipeline, PipelineLookupError> {
        match self.pipelines.get(pipeline_id) {
            Some(pipeline) => Ok(pipeline),
            None if self.deleted.contains(pipeline_id) => Err(PipelineLookupError::Deleted),
            None => Err(PipelineLookupError::Unknown),
        }
    }

    /// [`Self::pipeline`] for a handler that mutates the record it resolved.
    pub fn pipeline_mut(
        &mut self,
        pipeline_id: &str,
    ) -> Result<&mut Pipeline, PipelineLookupError> {
        if let Some(pipeline) = self.pipelines.get_mut(pipeline_id) {
            return Ok(pipeline);
        }
        if self.deleted.contains(pipeline_id) {
            return Err(PipelineLookupError::Deleted);
        }
        Err(PipelineLookupError::Unknown)
    }

    /// Delete a pipeline: drop the record and its definition, tombstone the id, and
    /// release the `uniqueId` it claimed so the same `(name, uniqueId)` can be created
    /// again (the claim only holds while a pipeline with that name and unique id
    /// exists).
    pub fn remove_pipeline(&mut self, pipeline_id: &str) -> Result<Pipeline, PipelineLookupError> {
        match self.pipelines.remove(pipeline_id) {
            Some(pipeline) => {
                self.unique_ids
                    .retain(|_, claim| claim.pipeline_id != pipeline_id);
                self.tombstone(pipeline_id);
                Ok(pipeline)
            }
            None if self.deleted.contains(pipeline_id) => Err(PipelineLookupError::Deleted),
            None => Err(PipelineLookupError::Unknown),
        }
    }

    /// Remember `pipeline_id` as deleted, evicting the oldest tombstones so the set never
    /// holds more than [`MAX_TOMBSTONES`] ids.
    fn tombstone(&mut self, pipeline_id: &str) {
        if self.deleted.insert(pipeline_id.to_string()) {
            self.deleted_order.push_back(pipeline_id.to_string());
        }
        while self.deleted_order.len() > MAX_TOMBSTONES {
            if let Some(evicted) = self.deleted_order.pop_front() {
                self.deleted.remove(&evicted);
            }
        }
    }

    /// Every live pipeline in creation order — the order `ListPipelines` pages in.
    pub fn pipelines_in_creation_order(&self) -> Vec<&Pipeline> {
        let mut ordered: Vec<&Pipeline> = self.pipelines.values().collect();
        ordered.sort_by_key(|pipeline| pipeline.seq);
        ordered
    }
}

/// How many characters follow the `df-` prefix of a pipeline id.
///
/// AWS assigns ids of the form `df-06372391ZG65EXAMPLE` (the `CreatePipeline` and
/// `DescribePipelines` examples in this service's Smithy model documentation), i.e. the
/// prefix plus 19 upper-case alphanumeric characters.
const PIPELINE_ID_SUFFIX_LEN: usize = 19;

/// The `pipelineId` for creation number `seq`.
///
/// `seq` is run through a bijective 64-bit mix before it is rendered, so consecutive
/// creates do not produce ids that read as consecutive counters, while the mapping stays
/// injective — two different sequence numbers can never collide on one id.
fn mint_pipeline_id(seq: u64) -> String {
    let mixed = seq
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x1656_67B1_9E37_79F9);
    let suffix = base36_upper(mixed);
    let width = PIPELINE_ID_SUFFIX_LEN;
    format!("df-{suffix:0>width$}")
}

/// `value` in base 36 using the digits `0-9A-Z`.
fn base36_upper(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if value == 0 {
        return "0".to_string();
    }
    let mut rendered = Vec::new();
    while value > 0 {
        rendered.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    rendered.reverse();
    String::from_utf8(rendered).expect("base 36 digits are ASCII")
}

/// A `SystemTime` in the form Data Pipeline's `@creationTime` metadata field takes:
/// `2012-12-13T01:24:06`, i.e. ISO-8601 UTC to the second with no zone suffix (the
/// `DescribePipelines` response example in this service's Smithy model documentation).
#[must_use]
pub fn format_creation_time(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_secs()).ok())
        .unwrap_or_default();
    let rendered = aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // `Format::DateTime` renders RFC-3339 (`…T01:24:06Z`); this field carries no zone
    // suffix, and whole seconds in means no fractional part to strip.
    rendered.strip_suffix('Z').unwrap_or(&rendered).to_string()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatapipelineDatapipelineState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatapipelineDatapipelineState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minted ids carry the documented `df-` prefix and width, and never repeat.
    #[test]
    fn minted_ids_are_well_formed_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for seq in 1..=1000u64 {
            let id = mint_pipeline_id(seq);
            assert!(
                id.starts_with("df-"),
                "seq {seq}: expected a df- prefix, got {id:?}"
            );
            assert_eq!(
                id.len(),
                3 + PIPELINE_ID_SUFFIX_LEN,
                "seq {seq}: expected df- plus {PIPELINE_ID_SUFFIX_LEN} characters, got {id:?}"
            );
            assert!(
                id[3..]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()),
                "seq {seq}: expected upper-case alphanumerics, got {id:?}"
            );
            assert!(seen.insert(id.clone()), "seq {seq}: duplicate id {id:?}");
        }
    }

    /// `@creationTime` is ISO-8601 to the second with no zone suffix.
    #[test]
    fn creation_time_is_rendered_without_a_zone_suffix() {
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_355_362_246);
        let rendered = format_creation_time(at);
        assert_eq!(
            rendered, "2012-12-13T01:30:46",
            "expected ISO-8601 seconds with no zone suffix, got {rendered:?}"
        );
    }

    /// The tombstone set is bounded: a create/delete loop evicts the oldest ids rather
    /// than growing the store without limit, and the newest tombstones survive.
    #[test]
    fn tombstones_are_bounded_and_evict_the_oldest_first() {
        let mut state = ComAmazonawsDatapipelineDatapipelineState::default();
        let mut ids = Vec::new();
        for index in 0..MAX_TOMBSTONES + 10 {
            let (pipeline_id, seq) = state.mint_pipeline_id();
            state.pipelines.insert(
                pipeline_id.clone(),
                Pipeline {
                    pipeline_id: pipeline_id.clone(),
                    name: format!("p{index}"),
                    unique_id: format!("u{index}"),
                    description: None,
                    tags: Vec::new(),
                    state: PipelineState::Pending,
                    created_at: SystemTime::UNIX_EPOCH,
                    seq,
                    definition_version: 0,
                    definition: None,
                },
            );
            state
                .remove_pipeline(&pipeline_id)
                .expect("the pipeline was just inserted");
            ids.push(pipeline_id);
        }

        assert_eq!(
            state.deleted.len(),
            MAX_TOMBSTONES,
            "expected the tombstone set capped at {MAX_TOMBSTONES}, got {}",
            state.deleted.len()
        );
        assert_eq!(
            state.deleted_order.len(),
            MAX_TOMBSTONES,
            "expected the eviction queue capped at {MAX_TOMBSTONES}, got {}",
            state.deleted_order.len()
        );
        let oldest = &ids[0];
        assert_eq!(
            state.pipeline(oldest).err(),
            Some(PipelineLookupError::Unknown),
            "the oldest tombstone must have been evicted, got {:?}",
            state.pipeline(oldest).err()
        );
        let newest = ids.last().expect("ids is not empty");
        assert_eq!(
            state.pipeline(newest).err(),
            Some(PipelineLookupError::Deleted),
            "the newest tombstone must be retained, got {:?}",
            state.pipeline(newest).err()
        );
    }
}
