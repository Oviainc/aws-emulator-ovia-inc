// SPDX-License-Identifier: CC0-1.0

//! @op DescribeIdentityProviderConfig
//!
//! IMPLEMENTED. `DescribeIdentityProviderConfig` reports the
//! [`crate::state::IdentityProviderConfigRecord`] that `AssociateIdentityProviderConfig`
//! wrote onto the cluster's record in the shared engine's store — it keeps no copy of
//! its own and computes nothing per request beyond materialising the status move the
//! clock already owes.
//!
//! A configuration is addressed by the `type`/`name` pair of
//! `IdentityProviderConfig`, within the cluster named by the path. `oidc` is the only
//! type this API defines, so any other type names no configuration. That pair is ONE
//! resource, so a describe whose cluster or whose configuration name is not in the
//! store answers the same way, with the service's generic memberless not-found
//! payload ([`crate::state::resource_not_found`]) rather than naming the half it could
//! not find.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeIdentityProviderConfig`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Describes an identity provider configuration.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `identityProviderConfig` (IdentityProviderConfig) **required** — An object representing
//!   an identity provider configuration.
//!
//! ## Output
//!
//! * `identityProviderConfig` (IdentityProviderConfigResponse) — The object that represents
//!   an OpenID Connect (OIDC) identity provider configuration.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, IDENTITY_PROVIDER_CONFIG_TYPE_OIDC};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeIdentityProviderConfigError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeIdentityProviderConfigError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeIdentityProviderConfig")]
#[derive(Debug)]
pub enum DescribeIdentityProviderConfigErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_identity_provider_config::DescribeIdentityProviderConfigError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeIdentityProviderConfigError = crate::scaffolded::Scaffolded<DescribeIdentityProviderConfigErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeIdentityProviderConfigErrorModeled> for DescribeIdentityProviderConfigError {
    fn from(modeled: DescribeIdentityProviderConfigErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeIdentityProviderConfig` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeIdentityProviderConfig<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeIdentityProviderConfig<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeIdentityProviderConfig<Eng>
{
    type Input = DescribeIdentityProviderConfigInput;
    type Output = DescribeIdentityProviderConfigOutput;
    type Error = DescribeIdentityProviderConfigError;

    const NAME: &'static str = "DescribeIdentityProviderConfig";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_IDENTITY_PROVIDER_CONFIG_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeIdentityProviderConfigError::from(
                DescribeIdentityProviderConfigErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let requested = &input.identity_provider_config;

        let mut guard = store.lock();
        // The only write a reporting operation may make: the `CREATING` -> `ACTIVE`
        // move was due whether or not anybody called.
        guard.materialize_due_transitions(SystemTime::now());

        // The cluster is resolved first: a configuration is a sub-resource of its
        // cluster, and a cluster name identifies a cluster only within one region.
        // An unknown cluster and a name no configuration on it carries are the same
        // answer, in the same payload: this operation addresses ONE resource — the
        // `(cluster, configuration)` pair — so it reports the service's generic
        // not-found payload ([`crate::state::resource_not_found`], shared with the
        // access-entry reads, which names nothing and sets no identity member)
        // rather than naming the half it could not find, which is what
        // `DescribeCluster`, resolving a cluster by name alone, does instead.
        let Some(cluster) = guard.clusters.get(&(region, input.cluster_name.clone())) else {
            return Err(
                DescribeIdentityProviderConfigErrorModeled::ResourceNotFoundException(
                    crate::state::resource_not_found(),
                )
                .into(),
            );
        };

        // "The only type available is `oidc`" (`IdentityProviderConfig$type`), so any
        // other type is not a configuration this API could hold.
        if !requested
            .r#type
            .eq_ignore_ascii_case(IDENTITY_PROVIDER_CONFIG_TYPE_OIDC)
        {
            return Err(
                DescribeIdentityProviderConfigErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "identityProviderConfig.type must be \
                             {IDENTITY_PROVIDER_CONFIG_TYPE_OIDC}, got {}",
                            requested.r#type
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        match cluster.identity_provider_configs.get(&requested.name) {
            Some(record) => Ok(DescribeIdentityProviderConfigOutput {
                identity_provider_config: Some(IdentityProviderConfigResponse {
                    oidc: Some(record.to_oidc_config(&input.cluster_name)),
                }),
            }),
            None => Err(
                DescribeIdentityProviderConfigErrorModeled::ResourceNotFoundException(
                    crate::state::resource_not_found(),
                )
                .into(),
            ),
        }
    }
}
