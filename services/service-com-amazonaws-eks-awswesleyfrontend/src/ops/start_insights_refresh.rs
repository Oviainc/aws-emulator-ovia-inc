// SPDX-License-Identifier: CC0-1.0

//! @op StartInsightsRefresh
//! IMPLEMENTED. Begins ONE on-demand cluster insights refresh and writes it as the
//! cluster's [`crate::state::InsightsRefreshRecord`], answering with the status and
//! message of the refresh it just started. `DescribeInsightsRefresh` computes its
//! whole answer from that same record; nothing here keeps a second copy.
//!
//! The rules it enforces:
//!
//! * a refresh is a refresh OF a cluster (`/clusters/{clusterName}/insights-refresh`),
//!   so a `clusterName` this store does not hold in the region the request is signed
//!   for is the same `ResourceNotFoundException` every cluster-subject operation gives;
//! * the refresh "[gets] the latest analysis" of a live control plane, so a cluster
//!   that is not `ACTIVE` cannot run one — `InvalidRequestException`, "The request is
//!   invalid given the state of the cluster", the same rule the other operations that
//!   act on a cluster's control plane apply;
//! * `DescribeInsightsRefresh` reports "the LATEST on-demand cluster insights refresh
//!   operation", so a cluster runs at most one at a time: a start while the stored
//!   refresh is still `IN_PROGRESS` is `InvalidRequestException` too — the state of
//!   the cluster's associated operations — and the refresh already running is left
//!   untouched.
//!
//! The refresh starts `IN_PROGRESS` and the clock owes it `COMPLETED`
//! [`crate::state::INSIGHTS_REFRESH_DURATION`] later, so a caller that polls straight
//! after the start still observes the running refresh and the conflict that goes with
//! it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartInsightsRefresh`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Initiates an on-demand refresh operation for cluster insights, getting the latest
//! analysis outside of the standard refresh schedule.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of the cluster for the refresh insights
//!   operation.
//!
//! ## Output
//!
//! * `message` (String) — The message associated with the insights refresh operation.
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
use crate::state::{self, InsightsRefreshRecord, STATUS_ACTIVE};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartInsightsRefreshError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartInsightsRefreshError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#StartInsightsRefresh")]
#[derive(Debug)]
pub enum StartInsightsRefreshErrorModeled {
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_insights_refresh::StartInsightsRefreshError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartInsightsRefreshError = crate::scaffolded::Scaffolded<StartInsightsRefreshErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartInsightsRefreshErrorModeled> for StartInsightsRefreshError {
    fn from(modeled: StartInsightsRefreshErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartInsightsRefresh` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartInsightsRefresh<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartInsightsRefresh<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartInsightsRefresh<Eng>
{
    type Input = StartInsightsRefreshInput;
    type Output = StartInsightsRefreshOutput;
    type Error = StartInsightsRefreshError;

    const NAME: &'static str = "StartInsightsRefresh";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_INSIGHTS_REFRESH_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_INSIGHTS_REFRESH_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartInsightsRefreshError::from(StartInsightsRefreshErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let invalid_request = |message: String| {
            StartInsightsRefreshError::from(
                StartInsightsRefreshErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        let now = SystemTime::now();
        let mut guard = store.lock();
        // Every clock-owed move first, so the refresh this call is judged against is
        // the one the clock already finished rather than a stale `IN_PROGRESS`.
        guard.materialize_due_transitions(now);

        // A cluster name identifies a cluster only within one region, so the cluster
        // this refreshes is the one in the region the request is signed for. The
        // cluster IS the subject of this operation, so an unknown name is reported as
        // the missing cluster, the way `DescribeCluster` reports it.
        let Some(cluster) = guard
            .clusters
            .get_mut(&(region, input.cluster_name.clone()))
        else {
            return Err(StartInsightsRefreshErrorModeled::ResourceNotFoundException(
                state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };

        // "[Getting] the latest analysis" needs a control plane to analyse, so a
        // cluster that is still coming up, updating or going away cannot start one.
        if cluster.status != STATUS_ACTIVE {
            return Err(invalid_request(format!(
                "Cluster {} is in {} status; it must be {STATUS_ACTIVE} to start an insights \
                 refresh.",
                input.cluster_name, cluster.status
            )));
        }

        // At most one on-demand refresh at a time: the API exposes exactly one, "the
        // latest", so a second start while the stored one is still running is refused
        // and the running refresh keeps its own `startedAt` and due completion.
        if cluster
            .insights_refresh
            .as_ref()
            .is_some_and(InsightsRefreshRecord::is_in_progress)
        {
            return Err(invalid_request(format!(
                "An insights refresh is already in progress for cluster {}; wait for it to \
                 complete before starting another.",
                input.cluster_name
            )));
        }

        let refresh = InsightsRefreshRecord::started(now);
        let (status, message) = (refresh.status.clone(), refresh.message());
        // The latest refresh REPLACES the terminal one the cluster reported before:
        // `DescribeInsightsRefresh` reports one refresh, and from now on it is this.
        cluster.insights_refresh = Some(refresh);

        Ok(StartInsightsRefreshOutput {
            status: Some(status),
            message: Some(message),
        })
    }
}
