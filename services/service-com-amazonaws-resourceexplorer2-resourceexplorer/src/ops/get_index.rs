// SPDX-License-Identifier: CC0-1.0

//! @op GetIndex
//!
//! IMPLEMENTED: reports the index of the Region the request was signed for — its ARN,
//! type, lifecycle state, timestamps and tags — out of `crate::state`, and the modelled
//! `ResourceNotFoundException` (HTTP 404) for a Region that holds no index.
//!
//! This is the operation a client polls to learn when a freshly created index is usable,
//! so it reports the index in WHATEVER state the store holds it in, including the
//! non-terminal `CREATING` and `DELETING`: a caller waiting for `ACTIVE` has to be able
//! to see the state it is waiting out.
//!
//! Reporting: the only thing this handler writes is the index lifecycle transition the
//! clock had already made due ([`crate::state::ComAmazonawsResourceexplorer2ResourceexplorerState::settle`]),
//! which was owed at this instant whether or not anybody called. Polling therefore never
//! moves an index on by itself.
//!
//! `ReplicatingFrom`/`ReplicatingTo` are computed from the account's other indexes: an
//! `AGGREGATOR` index reports the Regions that replicate into it, a `LOCAL` index the
//! Region of the account's aggregator index, and neither member is present when there is
//! nothing to report. `UpdateIndexType` is the operation that promotes an index to
//! `AGGREGATOR`, so that is the call that makes either form observable.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this operation
//! because its input is `smithy.api#Unit` (recorded in `src/ops/SKIPPED_STUBS.md`), so it
//! had no handler file and no entry in the service's `operations(...)` allow-list and was
//! answered by the framework's unroutable-operation path. `build.rs` derives that
//! allow-list from the `//! @op <Name>` header of every `src/ops/*.rs`, so this file is
//! what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back; never
//! construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! the SigV4 credential scope this handler reads the Region from lives there.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetIndex`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-resourceexplorer2-resourceexplorer/smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json")
//! ```
//!
//! Retrieves details about the Amazon Web Services Resource Explorer index in the Amazon
//! Web Services Region in which you invoked the operation.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `Arn` (String) — The Amazon resource name (ARN) of the index.
//! * `Type` (IndexType) — The type of the index in this Region.
//! * `State` (IndexState) — The current state of the index in this Amazon Web Services
//!   Region.
//! * `ReplicatingFrom` (RegionList) — This response value is present only if this index is
//!   `Type=AGGREGATOR`. A list of the Amazon Web Services Regions that replicate their
//!   content to the index in this Region.
//! * `ReplicatingTo` (RegionList) — This response value is present only if this index is
//!   `Type=LOCAL`. The Amazon Web Services Region that contains the aggregator index, if
//!   one exists.
//! * `CreatedAt` (Timestamp, date-time) — The date and time when the index was originally
//!   created.
//! * `LastUpdatedAt` (Timestamp, date-time) — The date and time when the index was last
//!   updated.
//! * `Tags` (TagMap) — Tag key and value pairs that are attached to the index.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The credentials that you used to call this
//!   operation don't have the minimum required permissions.
//! * `InternalServerException` (HTTP 500) — The request failed because of internal service
//!   error. Try your request again later.
//! * `ResourceNotFoundException` (HTTP 404) — You specified a resource that doesn't exist.
//!   Check the ID or ARN that you used to identity the resource, and try again.
//! * `ThrottlingException` (HTTP 429) — The request failed because you exceeded a rate limit
//!   for this operation. For more information, see Quotas for Resource Explorer.
//! * `ValidationException` (HTTP 400) — You provided an invalid value for one of the
//!   operation's parameters. Check the syntax for the operation, and try again.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetIndexError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetIndexError` alias below.
#[smithy_macros::operation_error("com.amazonaws.resourceexplorer2#GetIndex")]
#[derive(Debug)]
pub enum GetIndexErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_index::GetIndexError`).
pub type GetIndexError = crate::scaffolded::Scaffolded<GetIndexErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetIndexErrorModeled> for GetIndexError {
    fn from(modeled: GetIndexErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ReplicatingFrom` and `ReplicatingTo` of `index`, computed from the other indexes
/// the account holds.
///
/// Each member is present only for one index type, as the model documents: an
/// `AGGREGATOR` index reports the Regions whose indexes replicate INTO it, and a `LOCAL`
/// index reports the Region of the account's aggregator index, "if one exists" — so the
/// member is absent, rather than an empty list, when there is nothing to report.
///
/// An index being torn down is not a replication peer: it is on its way out, so reporting
/// it would name a Region whose content is about to stop being replicated at all. That is
/// the same reason `live_index` and `listable_indexes` leave it out.
fn replication(
    store: &crate::state::ComAmazonawsResourceexplorer2ResourceexplorerState,
    index: &crate::state::IndexEntry,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let local = index.index_type == crate::state::INDEX_TYPE_LOCAL;
    let mut regions: Vec<String> = store
        .indexes
        .values()
        .filter(|other| other.state != crate::state::IndexLifecycle::Deleting)
        .filter(|other| {
            if local {
                // The aggregator index, wherever it is: the Region a local index
                // replicates its content to.
                other.index_type != crate::state::INDEX_TYPE_LOCAL
            } else {
                // Every other Region's index replicates into this aggregator.
                other.region != index.region
            }
        })
        .map(|other| other.region.clone())
        .collect();
    regions.sort();
    let reported = (!regions.is_empty()).then_some(regions);
    if local {
        (None, reported)
    } else {
        (reported, None)
    }
}

/// The `GetIndex` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetIndex<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetIndex<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetIndex<Eng>
{
    type Input = GetIndexInput;
    type Output = GetIndexOutput;
    type Error = GetIndexError;

    const NAME: &'static str = "GetIndex";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_INDEX_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_INDEX_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it: which
        // index this reports is decided by the Region the request was signed for, which
        // is the Region `CreateIndex` keyed it under.
        let _ = &input;
        let Some(store) = crate::state::service_state(&*self.engine) else {
            return Err(GetIndexErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the Resource Explorer store is unavailable".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.settle(now);

        // Not `live_index`: an index being torn down is still an index a caller can read,
        // and `DELETING` is one of the states the model's `IndexState` names.
        let Some(index) = guard.indexes.get(&region) else {
            return Err(GetIndexErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // The crate's wording for this modelled refusal, keyed on what the
                    // request named: this operation names a Region rather than an ARN,
                    // because the index of "the Region in which you invoke the operation"
                    // is what it reports.
                    message: Some(format!("Index not found in Region {region}")),
                },
            )
            .into());
        };
        let (replicating_from, replicating_to) = replication(&guard, index);
        Ok(GetIndexOutput {
            arn: Some(index.arn.clone()),
            r#type: Some(index.index_type.clone()),
            state: Some(index.state.as_wire().to_string()),
            replicating_from,
            replicating_to,
            created_at: Some(::aws_smithy_types::DateTime::from(index.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(index.last_updated_at)),
            // An index always HAS a tag map; one that carries no tags has an empty one.
            tags: Some(index.tags.clone()),
        })
    }
}
