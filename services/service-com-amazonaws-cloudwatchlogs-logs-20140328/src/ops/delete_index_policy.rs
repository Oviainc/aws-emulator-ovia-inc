// SPDX-License-Identifier: CC0-1.0

//! @op DeleteIndexPolicy
//!
//! Removes the LOG-GROUP level field index policy `PutIndexPolicy` stored on the group the
//! request names ([`crate::state::LogGroupRecord::index_policy`]), after which
//! `DescribeIndexPolicies` reports whichever account-level `FIELD_INDEX_POLICY` applies to
//! that group instead, and nothing at all when none does.
//!
//! The policy is the resource this operation deletes, so a stored group holding no
//! log-group level policy has none to delete — including a group only an account-level
//! policy applies to, because "You can't use this operation to delete an account-level index
//! policy. Instead, use DeleteAccountPolicy."
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteIndexPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json")
//! ```
//!
//! Deletes a log-group level field index policy that was applied to a single log group. The
//! indexing of the log events that happened before you delete the policy will still be used
//! for as many as 30 days to improve CloudWatch Logs Insights queries. If the deleted
//! policy included facet configurations, those facets will no longer be available for
//! interactive exploration in the CloudWatch Logs Insights console for this log group.
//! However, facet data is retained for up to 30 days. You can't use this operation to
//! delete an account-level index policy. Instead, use DeleteAccountPolicy. If you delete a
//! log-group level field index policy and there is an account-level field index policy, in
//! a few minutes the log group begins using that account-wide policy to index new incoming
//! log events. This operation only affects log group-level policies, including any facet
//! configurations, and preserves any data source-based account policies that may apply to
//! the log group.
//!
//! ## Input
//!
//! * `logGroupIdentifier` (LogGroupIdentifier, len 1..2048, pattern ^[\w#+=/:,.@-]*$)
//!   **required** — The log group to delete the index policy for. You can specify either the
//!   name or the ARN of the log group.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — A parameter is specified incorrectly.
//! * `LimitExceededException` (HTTP 400) — You have reached the maximum number of resources
//!   that can be created.
//! * `OperationAbortedException` (HTTP 400) — Multiple concurrent requests to update the same
//!   resource were in conflict.
//! * `ResourceNotFoundException` (HTTP 400) — The specified resource does not exist.
//! * `ServiceUnavailableException` (HTTP 500) — The service cannot complete the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteIndexPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteIndexPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudwatchlogs#DeleteIndexPolicy")]
#[derive(Debug)]
pub enum DeleteIndexPolicyErrorModeled {
    InvalidParameterException(InvalidParameterException),
    LimitExceededException(LimitExceededException),
    OperationAbortedException(OperationAbortedException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_index_policy::DeleteIndexPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteIndexPolicyError = crate::scaffolded::Scaffolded<DeleteIndexPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteIndexPolicyErrorModeled> for DeleteIndexPolicyError {
    fn from(modeled: DeleteIndexPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteIndexPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteIndexPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteIndexPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteIndexPolicy<Eng>
{
    type Input = DeleteIndexPolicyInput;
    type Output = DeleteIndexPolicyOutput;
    type Error = DeleteIndexPolicyError;

    const NAME: &'static str = "DeleteIndexPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_INDEX_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_INDEX_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `ctx` carries only request metadata; the policy this call removes belongs to the
        // log group `input` names.
        let _ = ctx;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteIndexPolicyError::from(
                DeleteIndexPolicyErrorModeled::ServiceUnavailableException(
                    ServiceUnavailableException {
                        message: Some("the log store is unavailable".to_string()),
                    },
                ),
            )
        })?;
        let mut store = state.lock();

        let not_found = |message: &str| {
            DeleteIndexPolicyError::from(
                DeleteIndexPolicyErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(message.to_string()),
                    },
                ),
            )
        };

        // `logGroupIdentifier` is "either the name or the ARN of the log group", so an ARN
        // resolves to the name the store is keyed by.
        let log_group_name =
            crate::state::log_group_name_from_identifier(&input.log_group_identifier).to_string();
        let group = store
            .log_groups
            .get_mut(&log_group_name)
            .ok_or_else(|| not_found(&crate::state::resource_not_found_message("log group")))?;

        // "Deletes a log-group level field index policy that was applied to a single log
        // group": the policy is the resource this operation deletes, so a group holding none
        // of its own has nothing for it to delete — an account-level policy that applies to
        // the group is not one this operation can remove.
        // The request identifies the policy by nothing of its own — only by the group it
        // is attached to — so the refusal states nothing about it beyond that the resource
        // is not there.
        if group.index_policy.take().is_none() {
            return Err(not_found(
                crate::state::UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE,
            ));
        }

        Ok(DeleteIndexPolicyOutput {})
    }
}
