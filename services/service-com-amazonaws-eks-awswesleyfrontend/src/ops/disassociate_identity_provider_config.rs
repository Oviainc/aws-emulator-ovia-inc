// SPDX-License-Identifier: CC0-1.0

//! @op DisassociateIdentityProviderConfig
//! IMPLEMENTED. Starts the asynchronous disassociation of one identity provider
//! configuration from its cluster and answers with the `Update` that tracks it: "If
//! you disassociate an identity provider from your cluster, users included in the
//! provider can no longer access the cluster." The configuration is NOT gone the
//! instant the call returns — it reports [`crate::state::CONFIG_STATUS_DELETING`]
//! until the clock completes that update, which is what takes it off the cluster
//! ([`crate::state::PendingClusterConfig::identity_provider_config_removal`]), so
//! `DescribeIdentityProviderConfig` reports the one record throughout and
//! `DescribeUpdate` reports the update this mints.
//!
//! The rules it enforces:
//!
//! * `identityProviderConfig.type` selects WHICH KIND of configuration the request
//!   addresses, and "the only type available is `oidc`"
//!   (`IdentityProviderConfig$type`), a member the model gives no enum — so a type
//!   this API does not define addresses no resource at all and is
//!   `InvalidParameterException`, answered BEFORE the cluster is resolved;
//! * an unknown `clusterName`, and a configuration name the cluster does not carry,
//!   are each `ResourceNotFoundException` — the same 404
//!   `DescribeIdentityProviderConfig` gives. This operation addresses the
//!   configuration the `(cluster, name)` pair names rather than the cluster, so an
//!   unresolvable cluster is reported WITHOUT being named: the generic sentence and no
//!   identity member ([`crate::state::resource_not_found`]), the same payload
//!   `DescribeIdentityProviderConfig` answers for the same pair;
//! * a cluster `RegisterCluster` connected has no control plane to authenticate
//!   against, which the connected-cluster gate reports as the `ClientException` this
//!   operation models, as `AssociateIdentityProviderConfig` does;
//! * one cluster takes one control-plane update at a time, so a request while
//!   another is in flight is `ResourceInUseException`;
//! * `clientRequestToken` is `@idempotencyToken`, so a retry carrying the token of
//!   the call that started the disassociation is answered with that same update.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DisassociateIdentityProviderConfig`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Disassociates an identity provider configuration from a cluster. If you disassociate an
//! identity provider from your cluster, users included in the provider can no longer access
//! the cluster. However, you can still access the cluster with IAM principals.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `identityProviderConfig` (IdentityProviderConfig) **required** — An object representing
//!   an identity provider configuration.
//!
//! ## Output
//!
//! * `update` (Update) — An object representing an asynchronous update.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ThrottlingException` (HTTP 429) — The request or operation couldn't be performed
//!   because a service is throttling requests.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, PendingClusterConfig, UpdateRecord, UpdateScope, CONFIG_STATUS_DELETING,
    IDENTITY_PROVIDER_CONFIG_TYPE_OIDC, STATUS_ACTIVE, UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS,
    UPDATE_PARAM_IDENTITY_PROVIDER_CONFIG,
    UPDATE_TYPE_DISASSOCIATE_IDENTITY_PROVIDER_CONFIG,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DisassociateIdentityProviderConfigError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisassociateIdentityProviderConfigError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DisassociateIdentityProviderConfig")]
#[derive(Debug)]
pub enum DisassociateIdentityProviderConfigErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disassociate_identity_provider_config::DisassociateIdentityProviderConfigError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DisassociateIdentityProviderConfigError = crate::scaffolded::Scaffolded<DisassociateIdentityProviderConfigErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DisassociateIdentityProviderConfigErrorModeled> for DisassociateIdentityProviderConfigError {
    fn from(modeled: DisassociateIdentityProviderConfigErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisassociateIdentityProviderConfig` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DisassociateIdentityProviderConfig<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisassociateIdentityProviderConfig<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisassociateIdentityProviderConfig<Eng>
{
    type Input = DisassociateIdentityProviderConfigInput;
    type Output = DisassociateIdentityProviderConfigOutput;
    type Error = DisassociateIdentityProviderConfigError;

    const NAME: &'static str = "DisassociateIdentityProviderConfig";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_IDENTITY_PROVIDER_CONFIG_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            DisassociateIdentityProviderConfigError::from(
                DisassociateIdentityProviderConfigErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let invalid_parameter = |message: String| {
            DisassociateIdentityProviderConfigError::from(
                DisassociateIdentityProviderConfigErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };
        // The CLUSTER half of an unresolvable request: this operation addresses the
        // configuration the `(cluster, name)` pair names, not the cluster, so the
        // cluster is not the resource it reports — the generic sentence and no identity
        // member, the same payload `DescribeIdentityProviderConfig` answers for the
        // same pair ([`state::resource_not_found`]). Naming the cluster is for the
        // operations whose SUBJECT is the cluster ([`state::cluster_not_found`]).
        let not_found = || {
            DisassociateIdentityProviderConfigError::from(
                DisassociateIdentityProviderConfigErrorModeled::ResourceNotFoundException(
                    state::resource_not_found(),
                ),
            )
        };

        let requested = &input.identity_provider_config;

        // THE KIND OF CONFIGURATION THE REQUEST ADDRESSES FIRST: `type` is what says
        // WHICH kind of identity provider the `(type, name)` pair names, and "the only
        // type available is `oidc`" (`IdentityProviderConfig$type`) — a member the model
        // types as a plain string, so a value outside that set is not refused before
        // this handler runs. A request naming a kind of provider this API does not
        // define addresses no resource of any cluster, so it is refused as the bad
        // parameter it is before any cluster is resolved. The comparison is
        // case-insensitive, as it is everywhere else this service reads the token
        // ([`state::IDENTITY_PROVIDER_CONFIG_TYPE_OIDC`]).
        if !requested
            .r#type
            .eq_ignore_ascii_case(IDENTITY_PROVIDER_CONFIG_TYPE_OIDC)
        {
            return Err(invalid_parameter(format!(
                "identityProviderConfig.type must be {IDENTITY_PROVIDER_CONFIG_TYPE_OIDC}, got {}",
                state::elided_value(&requested.r#type, state::MAX_ECHOED_VALUE)
            )));
        }

        let config_name = requested.name.clone();
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // THE CLUSTER NEXT: a configuration is a sub-resource of its cluster, so a
        // request that names no cluster of this account in this region has nothing to
        // disassociate from, and a cluster name identifies a cluster only within one
        // region.
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            return Err(not_found());
        };

        // An identity-provider configuration authenticates users to an Amazon EKS
        // control plane's API server, and a cluster `RegisterCluster` connected has no
        // such control plane: the operation does not apply to it, which the
        // connected-cluster gate reports permanently. It is answered as soon as the
        // cluster is resolved, before anything in the body is judged against it — the
        // same order `AssociateIdentityProviderConfig` keeps.
        if cluster.is_connected() {
            return Err(
                DisassociateIdentityProviderConfigErrorModeled::ClientException(
                    state::connected_cluster_refusal(&input.cluster_name),
                )
                .into(),
            );
        }

        // A retry. `clientRequestToken` is `@idempotencyToken`: an SDK generates it
        // once per call and RESENDS the same value when it retries, so a retry of the
        // call that started this disassociation is answered with THAT update rather
        // than starting a second one — which matters here, because the first call has
        // already taken the configuration to `DELETING` and a retry must not be
        // refused as a miss. A blank token identifies nothing, so it never matches.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(update) = replayed_token.and_then(|token| cluster.update_for_token(token)) {
            return Ok(DisassociateIdentityProviderConfigOutput {
                update: Some(update.to_update()),
            });
        }

        // THE CONFIGURATION the pair names, which only the resolved cluster can carry:
        // a name this cluster does not hold is the same 404 the cluster itself earns —
        // "the specified resource could not be found".
        if !cluster.identity_provider_configs.contains_key(&config_name) {
            return Err(DisassociateIdentityProviderConfigErrorModeled::ResourceNotFoundException(
                state::child_resource_not_found(format!(
                    "No identity provider config found for name: {config_name}."
                )),
            )
            .into());
        }

        // One update at a time per cluster: while another is running the cluster is in
        // use, which is exactly what `ResourceInUseException` reports, worded by
        // [`crate::state::update_in_progress`] for the update type this call would have
        // minted.
        if let Some(in_flight) = cluster.in_flight_update() {
            return Err(
                DisassociateIdentityProviderConfigErrorModeled::ResourceInUseException(
                    state::update_in_progress(
                        UPDATE_TYPE_DISASSOCIATE_IDENTITY_PROVIDER_CONFIG,
                        &input.cluster_name,
                        in_flight,
                    ),
                )
                .into(),
            );
        }

        // "The request is invalid given the state of the cluster" — authentication is
        // reconfigured on a running control plane, so a cluster that is not `ACTIVE`
        // cannot take this change.
        if cluster.status != STATUS_ACTIVE {
            return Err(
                DisassociateIdentityProviderConfigErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "Cluster {} is in {} status; it must be {STATUS_ACTIVE} to \
                             disassociate an identity provider configuration",
                            input.cluster_name, cluster.status
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let update_id = state::new_update_id(guard.next_update_sequence());
        let update = UpdateRecord {
            id: update_id.clone(),
            r#type: UPDATE_TYPE_DISASSOCIATE_IDENTITY_PROVIDER_CONFIG.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            // "A key-value map that contains the parameters associated with the
            // update": the one parameter of this update is the configuration it
            // disassociates, named by the `type`/`name` pair the API identifies a
            // configuration by everywhere else (`IdentityProviderConfig`) — the same
            // parameter the association reports.
            params: vec![UpdateParam {
                r#type: Some(UPDATE_PARAM_IDENTITY_PROVIDER_CONFIG.to_string()),
                value: Some(
                    ::serde_json::json!({
                        "type": IDENTITY_PROVIDER_CONFIG_TYPE_OIDC,
                        "name": config_name,
                    })
                    .to_string(),
                ),
            }],
            client_request_token: replayed_token.map(str::to_owned),
            succeeds_at: Some(now + UPDATE_COMPLETION_DELAY),
            // The change this update carries is the REMOVAL of the configuration from
            // the cluster, applied when the clock completes it.
            pending_config: Some(PendingClusterConfig {
                identity_provider_config_removal: Some(config_name.clone()),
                ..Default::default()
            }),
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
            // An identity provider is associated with the cluster itself, so this is an
            // update of the cluster rather than of a resource inside it — the same
            // scope the association records.
            scope: UpdateScope::Cluster,
        };
        let answer = update.to_update();

        let cluster = guard.clusters.get_mut(&cluster_key).ok_or_else(|| {
            DisassociateIdentityProviderConfigError::from(
                DisassociateIdentityProviderConfigErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the cluster record disappeared while updating it".to_string()),
                    ..Default::default()
                }),
            )
        })?;
        // The cluster itself is NOT taken to `UPDATING`: an identity provider
        // configuration carries its own `configStatus`, which is what tracks this
        // disassociation — exactly as the association tracks its own.
        cluster.record_update(update);
        if let Some(record) = cluster.identity_provider_configs.get_mut(&config_name) {
            record.status = CONFIG_STATUS_DELETING.to_string();
            // The configuration is on its way out, so the `CREATING` -> `ACTIVE` move
            // the clock may still have owed it is no longer owed to anybody.
            record.active_at = None;
        }

        Ok(DisassociateIdentityProviderConfigOutput {
            update: Some(answer),
        })
    }
}
