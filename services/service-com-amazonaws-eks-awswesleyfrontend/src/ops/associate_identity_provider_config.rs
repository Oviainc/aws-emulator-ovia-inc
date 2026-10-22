// SPDX-License-Identifier: CC0-1.0

//! @op AssociateIdentityProviderConfig
//!
//! IMPLEMENTED. `AssociateIdentityProviderConfig` writes one
//! [`crate::state::IdentityProviderConfigRecord`] into the owning cluster's record in the
//! shared engine's store and answers with the asynchronous `Update` that reports the
//! association; `DescribeIdentityProviderConfig` reads that same record back.
//!
//! The association is reported through an `Update` of type
//! `AssociateIdentityProviderConfig`, stored in the cluster's own
//! [`crate::state::ClusterRecord::updates`] so `DescribeUpdate` reads back the same
//! record. The update is `InProgress` and the configuration `CREATING`; the clock owes
//! both the move to `Successful`/`ACTIVE` [`crate::state::UPDATE_COMPLETION_DELAY`]
//! later. The due instant is stored on the records and materialised by whichever read
//! first runs after it.
//!
//! One update at a time per cluster, as the sibling `AssociateEncryptionConfig`
//! enforces; `clientRequestToken` is `@idempotencyToken`, so a retry is answered with
//! the update the first call minted rather than with a conflict.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `AssociateIdentityProviderConfig`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Associates an identity provider configuration to a cluster. If you want to authenticate
//! identities using an identity provider, you can create an identity provider configuration
//! and associate it to your cluster. After configuring authentication to your cluster you
//! can create Kubernetes Role and ClusterRole objects, assign permissions to them, and then
//! bind them to the identities using Kubernetes RoleBinding and ClusterRoleBinding objects.
//! For more information see Using RBAC Authorization in the Kubernetes documentation.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `oidc` (OidcIdentityProviderConfigRequest) **required** — An object representing an
//!   OpenID Connect (OIDC) identity provider configuration.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources.
//!
//! ## Output
//!
//! * `tags` (TagMap, len 1..50) — The tags for the resource.
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
    self, IdentityProviderConfigRecord, UpdateRecord, CONFIG_STATUS_CREATING,
    IDENTITY_PROVIDER_CONFIG_TYPE_OIDC, MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER, STATUS_ACTIVE,
    UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS, UPDATE_PARAM_IDENTITY_PROVIDER_CONFIG,
    UPDATE_TYPE_ASSOCIATE_IDENTITY_PROVIDER_CONFIG,
};
use std::collections::BTreeMap;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateIdentityProviderConfigError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateIdentityProviderConfigError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#AssociateIdentityProviderConfig")]
#[derive(Debug)]
pub enum AssociateIdentityProviderConfigErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_identity_provider_config::AssociateIdentityProviderConfigError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateIdentityProviderConfigError = crate::scaffolded::Scaffolded<AssociateIdentityProviderConfigErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateIdentityProviderConfigErrorModeled> for AssociateIdentityProviderConfigError {
    fn from(modeled: AssociateIdentityProviderConfigErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateIdentityProviderConfig` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateIdentityProviderConfig<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateIdentityProviderConfig<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateIdentityProviderConfig<Eng>
{
    type Input = AssociateIdentityProviderConfigInput;
    type Output = AssociateIdentityProviderConfigOutput;
    type Error = AssociateIdentityProviderConfigError;

    const NAME: &'static str = "AssociateIdentityProviderConfig";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_IDENTITY_PROVIDER_CONFIG_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            AssociateIdentityProviderConfigError::from(
                AssociateIdentityProviderConfigErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let invalid_parameter = |message: String| {
            AssociateIdentityProviderConfigError::from(
                AssociateIdentityProviderConfigErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };
        let invalid_request = |message: String| {
            AssociateIdentityProviderConfigError::from(
                AssociateIdentityProviderConfigErrorModeled::InvalidRequestException(
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
        guard.materialize_due_transitions(now);

        // The cluster is resolved FIRST: a configuration is a sub-resource of its
        // cluster, so a request that names no cluster of this account in this region
        // has nothing to validate against. A cluster name identifies a cluster only
        // within one region, so this is the record in the region the request is signed
        // for.
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            return Err(
                AssociateIdentityProviderConfigErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "No cluster found for name: {}.",
                            input.cluster_name
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        };

        // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
        // call and RESENDS the same value when it retries, so a retry of the call that
        // started this association is answered with THAT update rather than starting a
        // second one. A blank token identifies nothing, so it never matches. The token
        // is the idempotency key for the CALL, so it answers the update it minted even
        // if the resent body names something else — which is also how the sibling
        // `AssociateEncryptionConfig` reads it.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(update) = replayed_token.and_then(|token| cluster.update_for_token(token)) {
            // "The tags for the resource": the tags of the configuration that update
            // associated, so a retry answers exactly what the first call did.
            let tags = cluster
                .identity_provider_configs
                .values()
                .find(|config| config.update_id == update.id)
                .map(|config| config.tags.clone())
                .unwrap_or_default();
            return Ok(AssociateIdentityProviderConfigOutput {
                update: Some(update.to_update()),
                tags: (!tags.is_empty()).then(|| tags.into_iter().collect()),
            });
        }

        // One update at a time per cluster: while another is running the cluster is in
        // use, which is exactly what `ResourceInUseException` reports.
        if let Some(in_flight) = cluster.in_flight_update() {
            return Err(
                AssociateIdentityProviderConfigErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "Cluster {} already has update {} in progress",
                            input.cluster_name, in_flight.id
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // "The request is invalid given the state of the cluster" — authentication is
        // configured on a running control plane, so a cluster that is not `ACTIVE`
        // cannot take an identity provider.
        if cluster.status != STATUS_ACTIVE {
            return Err(invalid_request(format!(
                "Cluster {} is in {} status; it must be {STATUS_ACTIVE} to associate an \
                 identity provider configuration",
                input.cluster_name, cluster.status
            )));
        }

        let oidc = &input.oidc;
        let config_name = oidc.identity_provider_config_name.clone();

        // Every rule below is one the model's own documentation states, and none of
        // them is expressible as a modelled constraint, so none is refused before this
        // handler runs. `requiredClaims` key and value lengths ARE modelled, so they
        // already are. The body is judged BEFORE the stored configurations are
        // consulted, so a malformed request is answered as malformed whatever the
        // cluster already carries.
        //
        // A required string member can still arrive empty, and an empty name, issuer
        // or client id identifies nothing.
        for (member, value) in [
            ("oidc.identityProviderConfigName", config_name.as_str()),
            ("oidc.issuerUrl", oidc.issuer_url.as_str()),
            ("oidc.clientId", oidc.client_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(invalid_parameter(format!("{member} must not be empty")));
            }
        }

        // `issuerUrl`: "The URL must begin with `https://` ... Based on the OIDC
        // standard, path components are allowed but query parameters are not."
        if !oidc.issuer_url.starts_with("https://") {
            return Err(invalid_parameter(format!(
                "oidc.issuerUrl must begin with https://, got {}",
                oidc.issuer_url
            )));
        }
        if oidc.issuer_url.contains('?') {
            return Err(invalid_parameter(format!(
                "oidc.issuerUrl must not carry query parameters, got {}",
                oidc.issuer_url
            )));
        }

        // `usernamePrefix` and `groupsPrefix` exist to keep claims out of the names
        // Kubernetes already uses, and the model says of each that "the prefix can't
        // contain `system:`" (`OidcIdentityProviderConfig$usernamePrefix`,
        // `$groupsPrefix`).
        for (member, prefix) in [
            ("oidc.usernamePrefix", oidc.username_prefix.as_deref()),
            ("oidc.groupsPrefix", oidc.groups_prefix.as_deref()),
        ] {
            if prefix.is_some_and(|prefix| prefix.contains("system:")) {
                return Err(invalid_parameter(format!(
                    "{member} must not contain system:, got {}",
                    prefix.unwrap_or_default()
                )));
            }
        }

        // "The specified resource is in use." A configuration is addressed by name
        // within its cluster, so a second association under a name the cluster already
        // carries is a conflict, not a second resource.
        if cluster.identity_provider_configs.contains_key(&config_name) {
            return Err(
                AssociateIdentityProviderConfigErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "Identity provider config already exists with name: {config_name}"
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // What bounds the store: without a ceiling, repeated associates on one cluster
        // would retain an unbounded number of records. The service-limit error is not
        // modelled on this operation, so it is reported as an invalid request.
        if cluster.identity_provider_configs.len() >= MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER {
            return Err(invalid_request(format!(
                "Cluster {} already carries the maximum of \
                 {MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER} identity provider \
                 configurations",
                input.cluster_name
            )));
        }

        // Two draws of the store's own counter, so the update id and the id inside the
        // configuration's ARN are independent of each other.
        let update_id = state::new_update_id(guard.next_update_sequence());
        let resource_id = state::new_resource_id(guard.next_update_sequence());
        let arn = state::identity_provider_config_arn(
            &region,
            &input.cluster_name,
            IDENTITY_PROVIDER_CONFIG_TYPE_OIDC,
            &config_name,
            &resource_id,
        );
        let tags: BTreeMap<String, String> = input.tags.unwrap_or_default().into_iter().collect();
        let update = UpdateRecord {
            id: update_id.clone(),
            r#type: UPDATE_TYPE_ASSOCIATE_IDENTITY_PROVIDER_CONFIG.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            // "A key-value map that contains the parameters associated with the
            // update": the one parameter of this update is the configuration it
            // associates, named by the `type`/`name` pair the API identifies a
            // configuration by everywhere else (`IdentityProviderConfig`).
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
            // This update associates an identity provider, so it carries no pending
            // `encryptionConfig` for the cluster itself.
            pending_encryption_config: None,
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
        };
        let answered_update = update.to_update();

        let record = IdentityProviderConfigRecord {
            name: config_name.clone(),
            arn,
            issuer_url: oidc.issuer_url.clone(),
            client_id: oidc.client_id.clone(),
            // The claim and prefix members are stored exactly as given and absent when
            // not given: the documented fallbacks (`sub` for the username claim, the
            // issuer URL for the username prefix) are applied by the Kubernetes API
            // server when it validates a token, not by this API when it records the
            // configuration, so nothing is invented here that the caller did not send.
            username_claim: oidc.username_claim.clone(),
            username_prefix: oidc.username_prefix.clone(),
            groups_claim: oidc.groups_claim.clone(),
            groups_prefix: oidc.groups_prefix.clone(),
            required_claims: oidc
                .required_claims
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            tags: tags.clone(),
            // The configuration is `CREATING` until the clock completes the update
            // that associates it; both are owed at the same instant.
            status: CONFIG_STATUS_CREATING.to_string(),
            active_at: Some(now + UPDATE_COMPLETION_DELAY),
            update_id: update_id.clone(),
        };

        let cluster = guard.clusters.get_mut(&cluster_key).ok_or_else(|| {
            AssociateIdentityProviderConfigError::from(
                AssociateIdentityProviderConfigErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the cluster record disappeared while updating it".to_string()),
                    ..Default::default()
                }),
            )
        })?;
        // The cluster itself is NOT taken to `UPDATING`: an identity provider
        // configuration carries its own `configStatus` (`CREATING | DELETING |
        // ACTIVE`), which is what tracks this association, unlike
        // `AssociateEncryptionConfig`, which changes the cluster's own
        // `encryptionConfig`.
        cluster.updates.insert(update_id, update);
        cluster
            .identity_provider_configs
            .insert(config_name, record);

        Ok(AssociateIdentityProviderConfigOutput {
            update: Some(answered_update),
            // "The tags for the resource." `TagMap` is bounded 1..50, so a
            // configuration associated without tags reports no map rather than an
            // empty one.
            tags: (!tags.is_empty()).then(|| tags.into_iter().collect()),
        })
    }
}

