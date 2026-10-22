// SPDX-License-Identifier: CC0-1.0

//! @op RegisterCluster
//!
//! IMPLEMENTED. `RegisterCluster` connects an EXTERNAL Kubernetes cluster: it mints one
//! [`crate::state::ClusterRecord`] in the shared engine's store, marked a connected
//! cluster by the `connectorConfig` it carries, and answers with the `Cluster` it just
//! wrote. `DescribeCluster`, `ListClusters`, `ListTagsForResource` and `TagResource` all
//! read that same record.
//!
//! A connected cluster has no Amazon EKS control plane behind it, so the record it writes
//! leaves `endpoint`, `roleArn`, `resourcesVpcConfig`, `version`, `platformVersion`,
//! `identity` and `certificateAuthority` unset: what the caller gets back is the
//! registration (`activationId`, `activationCode`, `activationExpiry`, `provider`,
//! `roleArn`), which is what the connector manifest is built from.
//!
//! It is reported `PENDING`, because the cluster only becomes visible to Amazon EKS once
//! the manifest is applied in the external cluster through its native provider — an act
//! outside this API, which nothing in this emulator performs, so no call moves it to
//! `ACTIVE`. The one move the clock does owe it is the lapse of its activation window
//! ([`crate::state::CONNECTOR_ACTIVATION_WINDOW`]): three days after the registration a
//! still-`PENDING` connected cluster is `FAILED` and has to be deregistered. The due
//! instant is stored on the record and materialised by whichever read first runs after
//! it; nothing sleeps for it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RegisterCluster`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Connects a Kubernetes cluster to the Amazon EKS control plane. Any Kubernetes cluster
//! can be connected to the Amazon EKS control plane to view current information about the
//! cluster and its nodes. Cluster connection requires two steps. First, send a
//! RegisterClusterRequest to add it to the Amazon EKS control plane. Second, a Manifest
//! containing the activationID and activationCode must be applied to the Kubernetes cluster
//! through it's native provider to provide visibility. After the manifest is updated and
//! applied, the connected cluster is visible to the Amazon EKS control plane. If the
//! manifest isn't applied within three days, the connected cluster will no longer be
//! visible and must be deregistered using DeregisterCluster.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `connectorConfig` (ConnectorConfigRequest) **required** — The configuration settings
//!   required to connect the Kubernetes cluster to the Amazon EKS control plane.
//! * `name` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$) **required** — A
//!   unique name for this cluster in your Amazon Web Services Region.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources.
//!
//! ## Output
//!
//! * `cluster` (Cluster) — An object representing an Amazon EKS cluster.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
//! * `ResourcePropagationDelayException` (HTTP 428) — Required resources (such as
//!   service-linked roles) were created and are still propagating. Retry later.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, ClusterRecord, CONNECTOR_ACTIVATION_WINDOW, MAX_CLUSTERS, MAX_STORED_CLUSTERS,
    STATUS_PENDING,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterClusterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterClusterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#RegisterCluster")]
#[derive(Debug)]
pub enum RegisterClusterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ResourcePropagationDelayException(ResourcePropagationDelayException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_cluster::RegisterClusterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterClusterError = crate::scaffolded::Scaffolded<RegisterClusterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterClusterErrorModeled> for RegisterClusterError {
    fn from(modeled: RegisterClusterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// Why a `connectorConfig.roleArn` cannot be used to connect a cluster.
///
/// The two cases are client-visible and different: a value that is not an IAM ARN at
/// all is a malformed PARAMETER, while a well-formed IAM ARN naming something this
/// caller cannot have Amazon EKS assume on its behalf is a PERMISSION failure.
#[derive(Debug)]
enum RoleRefusal {
    /// Not an IAM ARN at all.
    NotAnIamArn,
    /// A well-formed IAM ARN this caller cannot pass to Amazon EKS, and why.
    ///
    /// The split is drawn at one question: is the value an IAM ARN? Everything
    /// past that — which account and partition it belongs to, and what it names
    /// within IAM — is about WHAT the caller is asking Amazon EKS to assume, so
    /// an IAM ARN whose resource is `role/` with no name after it lands here
    /// rather than on [`Self::NotAnIamArn`]: it is a well-formed IAM ARN, and it
    /// names no role this caller could pass.
    NotPassable(String),
}

/// Validate `role_arn` as the IAM role Amazon EKS is to assume for the connector.
///
/// The connector role is a role Amazon EKS assumes on the caller's behalf, so what is
/// checked is that the value names a role THIS caller can pass: a well-formed IAM role
/// ARN of the caller's own partition and account. The `region` field is not read — IAM
/// is global, so an IAM ARN carries none.
///
/// What is NOT checked is whether such a role exists and trusts the connector: that
/// lives in IAM, which this emulator does not reach from here, so a structurally
/// passable role is accepted rather than guessed about. `service_common::arn::vendor`
/// reads only the vendor field; this rule needs the partition, the account and the
/// resource type too, so the parse is here.
fn validate_connector_role(
    role_arn: &str,
    partition: &str,
    account: &str,
) -> Result<(), RoleRefusal> {
    // `splitn(6, ':')` leaves the whole resource — colons included — as the last field.
    let fields: Vec<&str> = role_arn.splitn(6, ':').collect();
    let [literal, arn_partition, vendor, _region, arn_account, resource] = fields.as_slice()
    else {
        return Err(RoleRefusal::NotAnIamArn);
    };
    if *literal != "arn" || arn_partition.is_empty() || *vendor != "iam" || resource.is_empty() {
        return Err(RoleRefusal::NotAnIamArn);
    }
    // An IAM role's resource field is `role/<path><name>`; any other identity
    // (`user/`, `group/`, `instance-profile/`) is not something a service assumes.
    let role_name = resource.strip_prefix("role/").unwrap_or_default();
    if role_name.is_empty() {
        return Err(RoleRefusal::NotPassable(format!(
            "{role_arn} does not name an IAM role"
        )));
    }
    if arn_partition != &partition {
        return Err(RoleRefusal::NotPassable(format!(
            "{role_arn} is in partition {arn_partition}, not {partition}"
        )));
    }
    if arn_account != &account {
        return Err(RoleRefusal::NotPassable(format!(
            "{role_arn} belongs to account {arn_account}, not {account}"
        )));
    }
    Ok(())
}

/// The `RegisterCluster` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterCluster<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterCluster<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterCluster<Eng>
{
    type Input = RegisterClusterInput;
    type Output = RegisterClusterOutput;
    type Error = RegisterClusterError;

    const NAME: &'static str = "RegisterCluster";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_CLUSTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_CLUSTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            RegisterClusterError::from(RegisterClusterErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let arn = ::engine::emulator_arn("eks", &region, &format!("cluster/{}", input.name));

        let invalid_parameter = |message: String| {
            RegisterClusterError::from(RegisterClusterErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // `provider` targets the `ConnectorConfigProvider` enum, whose members are the
        // only cloud providers a registration can name. An enum's member set is not a
        // constraint the boundary validator refuses on, so it is read here.
        let provider = input.connector_config.provider.clone();
        if !CONNECTOR_CONFIG_PROVIDER_VALUES.contains(&provider.as_str()) {
            return Err(invalid_parameter(format!(
                "connectorConfig.provider must be one of {}, got {provider}",
                CONNECTOR_CONFIG_PROVIDER_VALUES.join(", ")
            )));
        }

        // The partition every ARN this emulator mints carries, read off the cluster ARN
        // just built rather than written out again, and the single account it serves.
        let partition = arn.split(':').nth(1).unwrap_or_default();
        let account = ::engine::emulator_account_id();
        let role_arn = input.connector_config.role_arn.clone();
        match validate_connector_role(&role_arn, partition, account) {
            Ok(()) => {}
            Err(RoleRefusal::NotAnIamArn) => {
                return Err(invalid_parameter(format!(
                    "connectorConfig.roleArn must be an IAM role ARN of the form \
                     arn:{partition}:iam::{account}:role/<name>, got {role_arn}"
                )));
            }
            // A role the caller cannot pass is a permission failure, not a bad
            // parameter: Amazon EKS would have to assume a role this principal may not
            // hand it.
            Err(RoleRefusal::NotPassable(why)) => {
                return Err(RegisterClusterErrorModeled::AccessDeniedException(
                    AccessDeniedException {
                        message: Some(format!(
                            "You don't have permissions to pass connectorConfig.roleArn to \
                             Amazon EKS: {why}"
                        )),
                    },
                )
                .into());
            }
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A cluster name is unique within one Region and account ("A unique name for
        // this cluster in your Amazon Web Services Region", `RegisterCluster$name`), and
        // a registration occupies that name exactly as a created cluster does — the
        // store is keyed `(region, name)` for both.
        if let Some(existing) = guard.clusters.get(&(region.clone(), input.name.clone())) {
            // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
            // call and RESENDS the same value when it retries, so a retry of the call
            // that registered this cluster is absorbed and answered with that cluster —
            // with the SAME activation id and code, since minting new ones would make a
            // retry invalidate the manifest the first answer was used to build. A blank
            // token identifies nothing, so it never matches.
            let replayed_token = input
                .client_request_token
                .as_deref()
                .filter(|token| !token.trim().is_empty());
            if let Some(token) = replayed_token {
                // And only a retry of a REGISTRATION: a token that happens to match
                // the one a `CreateCluster` used for this name is not a retry of
                // this call, so it conflicts rather than answering with an Amazon
                // EKS cluster the caller never asked for.
                if existing.is_connected() && existing.client_request_token.as_deref() == Some(token)
                {
                    return Ok(RegisterClusterOutput {
                        cluster: Some(existing.to_cluster()),
                    });
                }
            }
            return Err(RegisterClusterErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(format!("Cluster already exists with name: {}", input.name)),
                    ..Default::default()
                },
            )
            .into());
        }

        // The same two ceilings `CreateCluster` enforces, for the same reasons: the
        // published per-Region EKS cluster quota, and the store-wide ceiling that bounds
        // how much repeated calls can retain across caller-chosen regions. A connected
        // cluster occupies a cluster record, so it is counted by both.
        if guard.cluster_names_in(&region).count() >= MAX_CLUSTERS {
            return Err(RegisterClusterErrorModeled::ResourceLimitExceededException(
                ResourceLimitExceededException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(format!(
                        "You have reached the limit of {MAX_CLUSTERS} clusters for this \
                         account in this Region"
                    )),
                    ..Default::default()
                },
            )
            .into());
        }
        if guard.clusters.len() >= MAX_STORED_CLUSTERS {
            return Err(RegisterClusterErrorModeled::ResourceLimitExceededException(
                ResourceLimitExceededException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(format!(
                        "You have reached the limit of {MAX_STORED_CLUSTERS} clusters for \
                         this account"
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // "If the manifest isn't applied within three days, the connected cluster will
        // no longer be visible and must be deregistered using DeregisterCluster" — so
        // the activation expires one window after the registration, and that instant is
        // both what the caller is told and when the lapse falls due.
        let activation_expiry = now + CONNECTOR_ACTIVATION_WINDOW;
        let connector_config = ConnectorConfigResponse {
            activation_id: Some(state::derive_activation_id(&arn, now)),
            activation_code: Some(state::derive_activation_code(&arn, now)),
            activation_expiry: Some(::aws_smithy_types::DateTime::from(activation_expiry)),
            provider: Some(provider),
            role_arn: Some(role_arn),
        };

        let record = ClusterRecord {
            name: input.name.clone(),
            arn: arn.clone(),
            created_at: now,
            // Everything an Amazon EKS control plane would carry is unset: this cluster
            // runs outside Amazon Web Services, so none of it exists. See
            // `crate::state::ClusterRecord`.
            version: None,
            platform_version: None,
            endpoint: None,
            role_arn: None,
            resources_vpc_config: None,
            kubernetes_network_config: None,
            logging: None,
            identity: None,
            certificate_authority: None,
            // Visible to Amazon EKS only once the manifest is applied in the external
            // cluster, which is not something this API — or this emulator — does.
            status: STATUS_PENDING.to_string(),
            active_at: None,
            tags: input
                .tags
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            encryption_config: Vec::new(),
            access_config: None,
            deletion_protection: false,
            client_request_token: input.client_request_token.clone(),
            outpost_config: None,
            upgrade_policy: None,
            zonal_shift_config: None,
            remote_network_config: None,
            compute_config: None,
            storage_config: None,
            control_plane_scaling_config: None,
            kube_api_server_config: None,
            kube_scheduler_config: None,
            kube_controller_manager_config: None,
            connector_config: Some(connector_config),
            activation_expires_at: Some(activation_expiry),
            // No asynchronous update has been started against this cluster, and no
            // identity provider is associated with it. (A connected cluster is not
            // one those operations act on today; each is still a 501 with its own
            // gap issue.)
            updates: ::std::collections::BTreeMap::new(),
            identity_provider_configs: ::std::collections::BTreeMap::new(),
        };

        let cluster = record.to_cluster();
        guard
            .arn_index
            .insert(arn, (region.clone(), input.name.clone()));
        guard.clusters.insert((region, input.name), record);

        Ok(RegisterClusterOutput {
            cluster: Some(cluster),
        })
    }
}
