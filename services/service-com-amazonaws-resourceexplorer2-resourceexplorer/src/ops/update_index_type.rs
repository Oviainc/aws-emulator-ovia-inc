// SPDX-License-Identifier: CC0-1.0

//! @op UpdateIndexType
//!
//! IMPLEMENTED: switches the stored type of the index its `Arn` names between the two
//! values the model's `IndexType` admits, reports the index `UPDATING` until the clock
//! settles it back to `ACTIVE`, and refuses a promotion the account's one-aggregator
//! rule does not allow.
//!
//! The rules, from the operation's own documentation:
//!
//! * the index is resolved by ARN — a Resource Explorer ARN carries its own Region — and
//!   an ARN this account holds no index for is the modelled `ResourceNotFoundException`
//!   (HTTP 404);
//! * "You can have only one aggregator index per Amazon Web Services account. Before you
//!   can promote a different index to be the aggregator index for the account, you must
//!   first demote the existing aggregator index to type LOCAL" — so promoting while
//!   another Region's index is the aggregator is the modelled `ConflictException`
//!   (HTTP 409), which is exactly what that error documents;
//! * "After you demote an aggregator index to a local index, you must wait 24 hours
//!   before you can promote another index to be the new aggregator index for the
//!   account" — the wait is
//!   [`crate::state::AGGREGATOR_PROMOTION_WAIT`], and promoting another index inside it
//!   is refused with the same modelled conflict;
//! * the switch is asynchronous: "Turning on replication from all other Regions is
//!   performed by asynchronous background tasks… the Status response of that operation
//!   changes from UPDATING to ACTIVE". The index is therefore stored `UPDATING` with the
//!   instant its settling is due, and `GetIndex` is what a caller polls for `ACTIVE`.
//!
//! A request that asks for the type the index already has changes nothing and reports the
//! index as it stands: there is no replication to start or stop, so no background task is
//! owed and the stored `LastUpdatedAt` is not moved on by asking.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateIndexType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-resourceexplorer2-resourceexplorer/smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json")
//! ```
//!
//! Changes the type of the index from one of the following types to the other. For more
//! information about indexes and the role they perform in Amazon Web Services Resource
//! Explorer, see Turning on cross-Region search by creating an aggregator index in the
//! Amazon Web Services Resource Explorer User Guide. AGGREGATOR index type The index
//! contains information about resources from all Amazon Web Services Regions in the Amazon
//! Web Services account in which you've created a Resource Explorer index. Resource
//! information from all other Regions is replicated to this Region's index. When you change
//! the index type to AGGREGATOR, Resource Explorer turns on replication of all discovered
//! resource information from the other Amazon Web Services Regions in your account to this
//! index. You can then, from this Region only, perform resource search queries that span
//! all Amazon Web Services Regions in the Amazon Web Services account. Turning on
//! replication from all other Regions is performed by asynchronous background tasks. You
//! can check the status of the asynchronous tasks by using the GetIndex operation. When the
//! asynchronous tasks complete, the Status response of that operation changes from UPDATING
//! to ACTIVE. After that, you can start to see results from other Amazon Web Services
//! Regions in query results. However, it can take several hours for replication from all
//! other Regions to complete. You can have only one aggregator index per Amazon Web
//! Services account. Before you can promote a different index to be the aggregator index
//! for the account, you must first demote the existing aggregator index to type LOCAL.
//! LOCAL index type The index contains information about resources in only the Amazon Web
//! Services Region in which the index exists. If an aggregator index in another Region
//! exists, then information in this local index is replicated to the aggregator index. When
//! you change the index type to LOCAL, Resource Explorer turns off the replication of
//! resource information from all other Amazon Web Services Regions in the Amazon Web
//! Services account to this Region. The aggregator index remains in the UPDATING state
//! until all replication with other Regions successfully stops. You can check the status of
//! the asynchronous task by using the GetIndex operation. When Resource Explorer
//! successfully stops all replication with other Regions, the Status response of that
//! operation changes from UPDATING to ACTIVE. Separately, the resource information from
//! other Regions that was previously stored in the index is deleted within 30 days by
//! another background task. Until that asynchronous task completes, some results from other
//! Regions can continue to appear in search results. After you demote an aggregator index
//! to a local index, you must wait 24 hours before you can promote another index to be the
//! new aggregator index for the account.
//!
//! ## Input
//!
//! * `Arn` (String) **required** — The Amazon resource name (ARN) of the index that you want
//!   to update.
//! * `Type` (IndexType, one of: AGGREGATOR | LOCAL) **required** — The type of the index. To
//!   understand the difference between LOCAL and AGGREGATOR, see Turning on cross-Region
//!   search in the Amazon Web Services Resource Explorer User Guide.
//!
//! ## Output
//!
//! * `Arn` (String) — The Amazon resource name (ARN) of the index that you updated.
//! * `LastUpdatedAt` (Timestamp) — The date and timestamp when the index was last updated.
//! * `State` (IndexState, one of: ACTIVE | CREATING | DELETED | DELETING | UPDATING) —
//!   Indicates the state of the request to update the index. This operation is asynchronous.
//!   Call the GetIndex operation to check for changes.
//! * `Type` (IndexType, one of: AGGREGATOR | LOCAL) — Specifies the type of the specified
//!   index after the operation completes.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The credentials that you used to call this
//!   operation don't have the minimum required permissions.
//! * `ConflictException` (HTTP 409) — If you attempted to create a view, then the request
//!   failed because either you specified parameters that didn’t match the original request,
//!   or you attempted to create a view with a name that already exists in this Amazon Web
//!   Services Region. If you attempted to create an index, then the request failed because
//!   either you specified parameters that didn't match the original request, or an index
//!   already exists in the current Amazon Web Services Region. If you attempted to update an
//!   index type to AGGREGATOR, then the request failed because you already have an AGGREGATOR
//!   index in a different Amazon Web Services Region.
//! * `InternalServerException` (HTTP 500) — The request failed because of internal service
//!   error. Try your request again later.
//! * `ResourceNotFoundException` (HTTP 404) — You specified a resource that doesn't exist.
//!   Check the ID or ARN that you used to identity the resource, and try again.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request failed because it exceeds a
//!   service quota.
//! * `ThrottlingException` (HTTP 429) — The request failed because you exceeded a rate limit
//!   for this operation. For more information, see Quotas for Resource Explorer.
//! * `ValidationException` (HTTP 400) — You provided an invalid value for one of the
//!   operation's parameters. Check the syntax for the operation, and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateIndexTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateIndexTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.resourceexplorer2#UpdateIndexType")]
#[derive(Debug)]
pub enum UpdateIndexTypeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_index_type::UpdateIndexTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateIndexTypeError = crate::scaffolded::Scaffolded<UpdateIndexTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateIndexTypeErrorModeled> for UpdateIndexTypeError {
    fn from(modeled: UpdateIndexTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The input member a type refusal blames, named the way the model declares it.
const TYPE_MEMBER: &str = "Type";

/// The index type this request asks for, when it is one the model's `IndexType` enum
/// admits.
///
/// That enum is what closes the set, so the boundary validator normally refuses anything
/// else before this handler runs; resolving it again here is what keeps an unmodelled
/// value out of the STORE, which `GetIndex` and `ListIndexes` read back.
fn requested_type(value: &str) -> Option<&'static str> {
    crate::state::INDEX_TYPES
        .into_iter()
        .find(|modelled| *modelled == value)
}

/// Refuse a `Type` outside the modelled enum: the modelled `ValidationException`
/// (HTTP 400), worded the way a constraint failure is worded and naming the member at
/// fault in `FieldList`.
fn invalid_type(value: &str) -> UpdateIndexTypeError {
    let issue = format!(
        "Member must satisfy enum value set: [{}]",
        crate::state::INDEX_TYPES.join(", ")
    );
    UpdateIndexTypeErrorModeled::ValidationException(ValidationException {
        message: format!(
            "1 validation error detected: Value '{value}' at '{TYPE_MEMBER}' failed to \
             satisfy constraint: {issue}"
        ),
        field_list: Some(vec![ValidationExceptionField {
            name: TYPE_MEMBER.to_string(),
            validation_issue: issue,
        }]),
    })
    .into()
}

/// Refuse a type change the index's state or the account's one-aggregator rule does not
/// allow: the modelled `ConflictException` (HTTP 409).
fn conflict(message: String) -> UpdateIndexTypeError {
    UpdateIndexTypeErrorModeled::ConflictException(ConflictException { message }).into()
}

/// The modelled `InternalServerException` (HTTP 500) for a store this handler cannot
/// reach — answered rather than constructing an engine of its own, which would hand this
/// one request a private store.
fn store_unavailable() -> UpdateIndexTypeError {
    UpdateIndexTypeErrorModeled::InternalServerException(InternalServerException {
        message: Some("the Resource Explorer store is unavailable".to_string()),
    })
    .into()
}

/// The `UpdateIndexType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateIndexType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateIndexType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateIndexType<Eng>
{
    type Input = UpdateIndexTypeInput;
    type Output = UpdateIndexTypeOutput;
    type Error = UpdateIndexTypeError;

    const NAME: &'static str = "UpdateIndexType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_INDEX_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_INDEX_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(requested) = requested_type(&input.r#type) else {
            return Err(invalid_type(&input.r#type));
        };
        let Some(store) = crate::state::service_state(&*self.engine) else {
            return Err(store_unavailable());
        };
        let now = ::std::time::SystemTime::now();
        let mut guard = store.lock();
        // The index lifecycle the clock already owed at `now`, so a creation, deletion or
        // earlier type change that has come due is settled before this one is judged.
        guard.settle(now);

        // Which index, and what it is now. Read before anything is written, because the
        // one-aggregator rule is about the account's OTHER indexes: a Resource Explorer
        // ARN carries its own Region, so the index is resolved from the ARN rather than
        // from the Region the request was signed for.
        let Some((region, state, current_type)) = guard
            .indexes
            .iter()
            .find(|(_, index)| index.arn == input.arn)
            .map(|(region, index)| (region.clone(), index.state, index.index_type.clone()))
        else {
            return Err(UpdateIndexTypeErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::resource_not_found_message(&input.arn)),
                },
            )
            .into());
        };

        // An index on its way out has no type left to switch: its content, and its
        // Region's views with it, are already being deleted.
        if state == crate::state::IndexLifecycle::Deleting {
            return Err(conflict(format!(
                "The index {} is in the {} state, so its type cannot be updated.",
                input.arn,
                state.as_wire()
            )));
        }

        // Asking for the type the index already has starts and stops no replication, so
        // there is nothing to change and nothing for the clock to owe.
        let changed = current_type != requested;
        if changed && requested == crate::state::INDEX_TYPE_AGGREGATOR {
            // "You can have only one aggregator index per Amazon Web Services account."
            // The index being promoted is LOCAL (otherwise nothing changed), so any
            // aggregator this finds is a different one.
            if guard.aggregator_index().is_some() {
                return Err(conflict(
                    crate::state::AGGREGATOR_EXISTS_MESSAGE.to_string(),
                ));
            }
            if guard
                .aggregator_promotion_blocked_until(&input.arn, now)
                .is_some()
            {
                return Err(conflict(crate::state::aggregator_wait_message()));
            }
        }
        // Demoting the aggregator is what starts the documented wait before ANOTHER index
        // can take its place.
        let demoted = changed && current_type == crate::state::INDEX_TYPE_AGGREGATOR;

        let Some(index) = guard.indexes.get_mut(&region) else {
            // Unreachable: `region` was just read out of this same map, under this lock.
            return Err(store_unavailable());
        };
        if changed {
            index.index_type = requested.to_string();
            // "Turning on replication from all other Regions is performed by
            // asynchronous background tasks… the Status response of that operation
            // changes from UPDATING to ACTIVE": the index is reported UPDATING, and the
            // clock owes it that transition.
            index.state = crate::state::IndexLifecycle::Updating;
            index.settles_at = Some(now + crate::state::INDEX_SETTLE_DELAY);
            index.last_updated_at = now;
        }
        let output = UpdateIndexTypeOutput {
            arn: Some(index.arn.clone()),
            // "Specifies the type of the specified index after the operation completes."
            r#type: Some(index.index_type.clone()),
            state: Some(index.state.as_wire().to_string()),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(index.last_updated_at)),
        };
        if demoted {
            guard.aggregator_demotion = Some(crate::state::AggregatorDemotion {
                index_arn: input.arn.clone(),
                at: now,
            });
        }
        drop(guard);

        // Audit record for a call that can change stored state: who asked, which index,
        // and what it is now. The presented access-key id is an identifier, not a secret.
        ::tracing::info!(
            target: "audit",
            operation = Self::NAME,
            outcome = if changed { "updated" } else { "unchanged" },
            principal = %::engine::extract_akid(&ctx.parts().headers),
            resource = %input.arn,
            index_type = requested,
            "recorded the index type the request asked for"
        );
        Ok(output)
    }
}
