// SPDX-License-Identifier: CC0-1.0

//! @op DescribeReplicationConfigurations
//!
//! Reports the stored replication configurations — one file system's, or every one this
//! account holds in the Region. A reporting operation: it materialises only the
//! destination-status transition the clock has already made due and writes nothing else.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeReplicationConfigurations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")
//! ```
//!
//! Retrieves the replication configuration for a specific file system. If a file system is
//! not specified, all of the replication configurations for the Amazon Web Services account
//! in an Amazon Web Services Region are retrieved.
//!
//! ## Input
//!
//! * `FileSystemId` (FileSystemId, len 0..128, pattern
//!   ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$)
//!   — You can retrieve the replication configuration for a specific file system by providing
//!   its file system ID. For cross-account,cross-region replication, an account can only
//!   describe the replication configuration for a file system in its own Region.
//! * `MaxResults` (MaxResults, range 1..) — (Optional) To limit the number of objects
//!   returned in a response, you can specify the MaxItems parameter. The default value is
//!   100.
//! * `NextToken` (Token, len 1..128, pattern ^.+$) — NextToken is present if the response is
//!   paginated. You can use NextToken in a subsequent request to fetch the next page of
//!   output.
//!
//! ## Output
//!
//! * `NextToken` (Token, len 1..128, pattern ^.+$) — You can use the NextToken from the
//!   previous response in a subsequent request to fetch the additional descriptions.
//! * `Replications` (ReplicationConfigurationDescriptions) — The collection of replication
//!   configurations that is returned.
//!
//! ## Errors
//!
//! * `BadRequest` (HTTP 400) — Returned if the request is malformed or contains an error such
//!   as an invalid parameter value or a missing required parameter.
//! * `FileSystemNotFound` (HTTP 404) — Returned if the specified FileSystemId value doesn't
//!   exist in the requester's Amazon Web Services account.
//! * `InternalServerError` (HTTP 500) — Returned if an error occurred on the server side.
//! * `ReplicationNotFound` (HTTP 404) — Returned if the specified file system does not have a
//!   replication configuration.
//! * `ValidationException` (HTTP 400) — Returned if the Backup service is not available in
//!   the Amazon Web Services Region in which the request was made.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeReplicationConfigurationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeReplicationConfigurationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.efs#DescribeReplicationConfigurations")]
#[derive(Debug)]
pub enum DescribeReplicationConfigurationsErrorModeled {
    BadRequest(BadRequest),
    FileSystemNotFound(FileSystemNotFound),
    InternalServerError(InternalServerError),
    ReplicationNotFound(ReplicationNotFound),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_replication_configurations::DescribeReplicationConfigurationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeReplicationConfigurationsError = crate::scaffolded::Scaffolded<DescribeReplicationConfigurationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeReplicationConfigurationsErrorModeled> for DescribeReplicationConfigurationsError {
    fn from(modeled: DescribeReplicationConfigurationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeReplicationConfigurations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeReplicationConfigurations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeReplicationConfigurations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeReplicationConfigurations<Eng>
{
    type Input = DescribeReplicationConfigurationsInput;
    type Output = DescribeReplicationConfigurationsOutput;
    type Error = DescribeReplicationConfigurationsError;

    const NAME: &'static str = "DescribeReplicationConfigurations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_REPLICATION_CONFIGURATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_REPLICATION_CONFIGURATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as efs;

        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_server_error());
        };
        let now = ::std::time::SystemTime::now();

        let mut state = state.lock();
        state.materialize_due_transitions(now);

        // "You can retrieve the replication configuration for a specific file system by
        // providing its file system ID" — the member accepts an id or an ARN, and names
        // the file system on either side of the configuration.
        let wanted = input.file_system_id.as_deref().map(efs::file_system_id_of);
        if let Some(id) = wanted {
            // This read reports replication configurations, not file systems: a filter
            // that matches none of them is `ReplicationNotFound`, whether the id names
            // a file system without a configuration or no file system at all. Nothing
            // on this path raises `FileSystemNotFound`.
            if state.replication_of(id).is_none() {
                return Err(replication_not_found());
            }
        }

        let matched: Vec<&efs::ReplicationConfiguration> = state
            .replications_in_order()
            .into_iter()
            .filter(|replication| wanted.is_none_or(|id| replication.holds(id)))
            .collect();

        // The token is the source file system the next page starts at, so resuming from
        // a returned NextToken continues exactly where the previous page stopped.
        let start = match input.next_token.as_deref() {
            Some(token) => matched
                .iter()
                .position(|replication| replication.source_file_system_id == token)
                .ok_or_else(|| {
                    bad_request(format!(
                        "NextToken {} does not name a replication configuration of this \
                         request's result set",
                        efs::quoted(token)
                    ))
                })?,
            None => 0,
        };
        let page_size = efs::page_size(input.max_results, efs::DEFAULT_REPLICATION_PAGE_SIZE);
        let page: Vec<CreateReplicationConfigurationOutput> = matched[start..]
            .iter()
            .take(page_size)
            .map(|replication| replication.describe())
            .collect();
        let next_token = matched
            .get(start + page.len())
            .map(|replication| replication.source_file_system_id.clone());

        Ok(DescribeReplicationConfigurationsOutput {
            replications: Some(page),
            next_token,
        })
    }
}

/// A 400 `BadRequest`: "the request is malformed or contains an error such as an invalid
/// parameter value or a missing required parameter".
fn bad_request(message: impl Into<String>) -> DescribeReplicationConfigurationsError {
    DescribeReplicationConfigurationsErrorModeled::BadRequest(BadRequest {
        error_code: "BadRequest".to_string(),
        message: Some(message.into()),
    })
    .into()
}

/// A 404 `ReplicationNotFound`: "the specified file system does not have a replication
/// configuration" — this read's refusal reports that the request's filter matched no
/// replication configuration, so it carries the fixed sentence
/// [`crate::state::NO_REPLICATIONS_FOUND_MESSAGE`] and names no id.
fn replication_not_found() -> DescribeReplicationConfigurationsError {
    DescribeReplicationConfigurationsErrorModeled::ReplicationNotFound(ReplicationNotFound {
        error_code: Some("ReplicationNotFound".to_string()),
        message: Some(crate::state::NO_REPLICATIONS_FOUND_MESSAGE.to_string()),
    })
    .into()
}

/// A 500 `InternalServerError`, for the one case this handler cannot serve: the engine
/// this service was mounted on holds no store for it.
fn internal_server_error() -> DescribeReplicationConfigurationsError {
    DescribeReplicationConfigurationsErrorModeled::InternalServerError(InternalServerError {
        error_code: "InternalServerError".to_string(),
        message: Some("the EFS emulator state store is unavailable".to_string()),
    })
    .into()
}
