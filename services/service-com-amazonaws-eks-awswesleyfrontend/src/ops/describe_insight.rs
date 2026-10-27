// SPDX-License-Identifier: CC0-1.0

//! @op DescribeInsight
//! IMPLEMENTED. "Returns details about an insight that you specify using its ID":
//! the one insight of the named cluster whose id the request carries, taken from the
//! same [`crate::state::ComAmazonawsEksAwswesleyfrontendState::cluster_insights`]
//! walk `ListInsights` reports — so the two cannot disagree about an insight — plus
//! the parts only the full `Insight` carries: the check's recommendation and the
//! resources it examined, each with its own status
//! ([`crate::state::ComAmazonawsEksAwswesleyfrontendState::insight_resources`]).
//!
//! It is a REPORTING operation: the only write it performs is materialising the
//! transitions the clock already owed, which were due at that instant whether or not
//! anybody called.
//!
//! The rules it enforces:
//!
//! * an unknown `clusterName` in the region the request is signed for is the same
//!   `ResourceNotFoundException` every cluster-subject operation gives;
//! * a cluster `RegisterCluster` connected has no insights analysis at all — the
//!   permanent `ClientException` refusal this service's EKS-managed plane gives;
//! * an `id` that is not one of that cluster's insights is the missing insight, a
//!   `ResourceNotFoundException` for the child resource rather than an empty answer.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeInsight`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Returns details about an insight that you specify using its ID.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of the cluster to describe the insight
//!   for.
//! * `id` (String) **required** — The identity of the insight to describe.
//!
//! ## Output
//!
//! * `insight` (Insight) — The full description of the insight.
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
// `DescribeInsightError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeInsightError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]), which is
// what the other insights operations answer for such a cluster too.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#DescribeInsight",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum DescribeInsightErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_insight::DescribeInsightError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeInsightError = crate::scaffolded::Scaffolded<DescribeInsightErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeInsightErrorModeled> for DescribeInsightError {
    fn from(modeled: DescribeInsightErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeInsight` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeInsight<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeInsight<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeInsight<Eng>
{
    type Input = DescribeInsightInput;
    type Output = DescribeInsightOutput;
    type Error = DescribeInsightError;

    const NAME: &'static str = "DescribeInsight";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_INSIGHT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_INSIGHT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeInsightError::from(DescribeInsightErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let now = SystemTime::now();
        let mut guard = store.lock();
        // Every clock-owed move first — the completion of an on-demand insights
        // refresh among them — so what this reports is what every later caller
        // reports too.
        guard.materialize_due_transitions(now);

        // THE CLUSTER FIRST: an insight is addressed through its cluster
        // (`/clusters/{clusterName}/insights/{id}`), so a cluster this region does not
        // hold is the missing cluster whatever id the request names.
        let Some(cluster) = guard.clusters.get(&(region.clone(), input.cluster_name.clone()))
        else {
            return Err(DescribeInsightErrorModeled::ResourceNotFoundException(
                state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };
        // Cluster insights are an analysis Amazon EKS runs against the control plane
        // it manages, and a cluster `RegisterCluster` connected has none.
        if cluster.is_connected() {
            return Err(DescribeInsightErrorModeled::ClientException(
                state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        // The insight is identified WITHIN the cluster by its id, which is the same id
        // `ListInsights` reported for it.
        let Some(insight) = guard
            .cluster_insights(&region, &input.cluster_name)
            .into_iter()
            .find(|insight| insight.id == input.id)
        else {
            // The resource this reports is the INSIGHT, not the cluster, so the
            // missing one is reported without naming the cluster in the payload — the
            // rule every child-resource read of this service follows.
            return Err(DescribeInsightErrorModeled::ResourceNotFoundException(
                state::child_resource_not_found(format!(
                    "No insight found with id: {} for cluster: {}.",
                    input.id, input.cluster_name
                )),
            )
            .into());
        };
        let resources = guard.insight_resources(insight.check, &region, &input.cluster_name);

        Ok(DescribeInsightOutput {
            insight: Some(insight.to_insight(resources)),
        })
    }
}
