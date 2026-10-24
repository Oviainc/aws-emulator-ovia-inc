// SPDX-License-Identifier: CC0-1.0

//! @op DescribeInsightsRefresh
//! IMPLEMENTED. Reports "the status of the latest on-demand cluster insights refresh
//! operation" from the cluster's [`crate::state::InsightsRefreshRecord`] — the record
//! `StartInsightsRefresh` writes — so the status, message and instants a caller reads
//! back are the stored ones.
//!
//! It is a REPORTING operation: the only write it performs is materialising the
//! completion the clock already owed the stored refresh
//! ([`crate::state::INSIGHTS_REFRESH_DURATION`]), which was due at that instant whether
//! or not anybody called — so a later caller sees exactly what this one saw.
//!
//! The rules it enforces:
//!
//! * an unknown `clusterName` in the region the request is signed for is the same
//!   `ResourceNotFoundException` every cluster-subject operation gives;
//! * a cluster that has never had an on-demand refresh started has no refresh
//!   operation to report, which is a `ResourceNotFoundException` for that resource
//!   rather than an answer with nothing in it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeInsightsRefresh`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Returns the status of the latest on-demand cluster insights refresh operation.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of the cluster associated with the
//!   insights refresh operation.
//!
//! ## Output
//!
//! * `endedAt` (Timestamp) — The date and time when the insights refresh operation ended.
//! * `message` (String) — The message associated with the insights refresh operation.
//! * `startedAt` (Timestamp) — The date and time when the insights refresh operation started.
//! * `status` (InsightsRefreshStatus, one of: COMPLETED | FAILED | IN_PROGRESS) — The current
//!   status of the insights refresh operation.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeInsightsRefreshError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeInsightsRefreshError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeInsightsRefresh")]
#[derive(Debug)]
pub enum DescribeInsightsRefreshErrorModeled {
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_insights_refresh::DescribeInsightsRefreshError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeInsightsRefreshError = crate::scaffolded::Scaffolded<DescribeInsightsRefreshErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeInsightsRefreshErrorModeled> for DescribeInsightsRefreshError {
    fn from(modeled: DescribeInsightsRefreshErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeInsightsRefresh` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeInsightsRefresh<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeInsightsRefresh<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeInsightsRefresh<Eng>
{
    type Input = DescribeInsightsRefreshInput;
    type Output = DescribeInsightsRefreshOutput;
    type Error = DescribeInsightsRefreshError;

    const NAME: &'static str = "DescribeInsightsRefresh";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_INSIGHTS_REFRESH_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_INSIGHTS_REFRESH_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeInsightsRefreshError::from(
                DescribeInsightsRefreshErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let now = SystemTime::now();
        let mut guard = store.lock();
        // The completion the clock owes a running refresh is materialised here, so
        // what this reports is what every later caller reports too.
        guard.materialize_due_transitions(now);

        let Some(cluster) = guard.clusters.get(&(region, input.cluster_name.clone())) else {
            return Err(
                DescribeInsightsRefreshErrorModeled::ResourceNotFoundException(
                    state::cluster_not_found(&input.cluster_name),
                )
                .into(),
            );
        };

        // The resource this reports is the refresh OPERATION, not the cluster, so a
        // cluster nobody has started one for reports the missing refresh — without
        // naming the cluster in the payload, the rule every child-resource read of
        // this service follows ([`state::child_resource_not_found`]).
        let Some(refresh) = cluster.insights_refresh.as_ref() else {
            return Err(
                DescribeInsightsRefreshErrorModeled::ResourceNotFoundException(
                    state::child_resource_not_found(format!(
                        "No insights refresh found for cluster: {}.",
                        input.cluster_name
                    )),
                )
                .into(),
            );
        };

        Ok(DescribeInsightsRefreshOutput {
            status: Some(refresh.status.clone()),
            message: Some(refresh.message()),
            started_at: Some(::aws_smithy_types::DateTime::from(refresh.started_at)),
            // Unset while the refresh is still running: it has not ended yet.
            ended_at: refresh.ended_at.map(::aws_smithy_types::DateTime::from),
        })
    }
}
