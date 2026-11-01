// SPDX-License-Identifier: CC0-1.0

//! @op CreateCapability
//! IMPLEMENTED. Writes a [`crate::state::CapabilityRecord`] for one managed capability
//! of one cluster and answers with it, reporting `CREATING` until the clock owes it
//! `ACTIVE` ([`crate::state::CAPABILITY_ACTIVATION_DELAY`]). `DescribeCapability`,
//! `ListCapabilities` and `DeleteCapability` all compute their answers from that record;
//! nothing here keeps a second copy.
//!
//! The rules it enforces, each from the model's own documentation:
//!
//! * a capability lives in a cluster, so an unknown `clusterName` is a
//!   `ResourceNotFoundException` — the same 404 `DescribeCapability` and
//!   `DeleteCapability` give, kept as ONE rule across the resource (see the
//!   `extra_errors` widening below) — and a cluster that is not `ACTIVE` is an
//!   `InvalidRequestException` ("The request is invalid given the state of the
//!   cluster");
//! * "You can only create one Capability of each type on a given Amazon EKS cluster": a
//!   second capability of a type the cluster already holds is `ResourceInUseException`,
//!   and so is a second one with a name it already holds ("The name must be unique within
//!   your cluster");
//! * `capabilityName` "can contain alphanumeric characters, hyphens, and underscores", a
//!   rule the shape carries no `@pattern` for — anything else is
//!   `InvalidParameterException`;
//! * `configuration`'s "structure ... varies depending on the capability type", so a
//!   configuration member that belongs to another `type` is `InvalidParameterException`,
//!   and the one documented default is materialised: an `ACK` capability reports
//!   `ack.enableCrossNamespace` as `false` when the request named neither the value nor
//!   an `ack` configuration at all;
//! * `clientRequestToken` is `@idempotencyToken`, so a retry carrying the token of the
//!   call that created the capability is answered with that capability instead of a
//!   conflict.
//!
//! ORDER: `clusterName` is this operation's `@httpLabel`, so the cluster it names is
//! resolved BEFORE any member of the body is judged and before `roleArn` is resolved
//! against IAM. A create against a cluster that does not exist therefore reports the
//! missing cluster (404), never a complaint about a sibling member. Only the two
//! questions that need no resource of this service at all precede it: the pass-role
//! authorization (403), decided from the value alone, and WHICH capability the request
//! asks for — a `type` outside the modelled `CapabilityType` names no capability type,
//! so it is `InvalidParameterException` (400) whatever cluster the path names
//! ([`crate::state::INVALID_CAPABILITY_TYPE_MESSAGE`]). Both are answered before this
//! handler runs, because an off-enum `type` never reaches it: the framework's
//! validating walk refuses the value first, so the service words the refusal in the
//! one seam ahead of dispatch (`crate::service::off_enum_member_contract`, answered by
//! `crate::service::precedence`).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateCapability`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates a managed capability resource for an Amazon EKS cluster. Capabilities provide
//! fully managed capabilities to build and scale with Kubernetes. When you create a
//! capability, Amazon EKSprovisions and manages the infrastructure required to run the
//! capability outside of your cluster. This approach reduces operational overhead and
//! preserves cluster resources. You can only create one Capability of each type on a given
//! Amazon EKS cluster. Valid types are Argo CD for declarative GitOps deployment, Amazon
//! Web Services Controllers for Kubernetes (ACK) for resource management, and Kube Resource
//! Orchestrator (KRO) for Kubernetes custom resource orchestration. For more information,
//! see EKS Capabilities in the Amazon EKS User Guide.
//!
//! ## Input
//!
//! * `capabilityName` (String) **required** — A unique name for the capability. The name must
//!   be unique within your cluster and can contain alphanumeric characters, hyphens, and
//!   underscores.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request. This token is valid for 24
//!   hours after creation. If you retry a request with the same client request token and the
//!   same parameters after the original request has completed successfully, the result of the
//!   original request is returned.
//! * `clusterName` (String) **required** — The name of the Amazon EKS cluster where you want
//!   to create the capability.
//! * `configuration` (CapabilityConfigurationRequest) — The configuration settings for the
//!   capability. The structure of this object varies depending on the capability type. For
//!   Argo CD capabilities, you can configure IAM Identity CenterIAM; Identity Center
//!   integration, RBAC role mappings, and network access settings.
//! * `deletePropagationPolicy` (CapabilityDeletePropagationPolicy, one of: RETAIN)
//!   **required** — Specifies how Kubernetes resources managed by the capability should be
//!   handled when the capability is deleted. Currently, the only supported value is RETAIN
//!   which retains all Kubernetes resources managed by the capability when the capability is
//!   deleted. Because resources are retained, all Kubernetes resources created by the
//!   capability should be deleted from the cluster before deleting the capability itself.
//!   After the capability is deleted, these resources become difficult to manage because the
//!   controller is no longer available.
//! * `roleArn` (String) **required** — The Amazon Resource Name (ARN) of the IAM role that
//!   the capability uses to interact with Amazon Web Services services. This role must have a
//!   trust policy that allows the EKS service principal to assume it, and it must have the
//!   necessary permissions for the capability type you're creating. For ACK capabilities, the
//!   role needs permissions to manage the resources you want to control through Kubernetes.
//!   For Argo CD capabilities, the role needs permissions to access Git repositories and
//!   Secrets Manager. For KRO capabilities, the role needs permissions based on the resources
//!   you'll be orchestrating.
//! * `tags` (TagMap, len 1..50) — The metadata that you apply to a resource to help you
//!   categorize and organize them. Each tag consists of a key and an optional value. You
//!   define them. The following basic restrictions apply to tags: Maximum number of tags per
//!   resource – 50 For each resource, each tag key must be unique, and each tag key can have
//!   only one value. Maximum key length – 128 Unicode characters in UTF-8 Maximum value
//!   length – 256 Unicode characters in UTF-8 If your tagging schema is used across multiple
//!   services and resources, remember that other services may have restrictions on allowed
//!   characters. Generally allowed characters are: letters, numbers, and spaces representable
//!   in UTF-8, and the following characters: …
//! * `type` (CapabilityType, one of: ACK | ARGOCD | KRO) **required** — The type of
//!   capability to create. Valid values are: ACK – Amazon Web Services Controllers for
//!   Kubernetes (ACK), which lets you manage resources directly from Kubernetes. ARGOCD –
//!   Argo CD for GitOps-based continuous delivery. KRO – Kube Resource Orchestrator (KRO) for
//!   composing and managing custom Kubernetes resources.
//!
//! ## Output
//!
//! * `capability` (Capability) — An object containing information about the newly created
//!   capability, including its name, ARN, status, and configuration.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ThrottlingException` (HTTP 429) — The request or operation couldn't be performed
//!   because a service is throttling requests.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, CapabilityRecord, CAPABILITY_ACTIVATION_DELAY, CAPABILITY_STATUS_CREATING,
    CAPABILITY_TYPE_ACK, CAPABILITY_TYPE_ARGOCD, EKS_SERVICE_PRINCIPAL, STATUS_ACTIVE,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateCapabilityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors, widened by `ResourceNotFoundException`. The
// scaffold's `NotImplemented` cannot live here — `#[operation_error]` accepts only
// variants the model declares — hence the `CreateCapabilityError` alias below.
//
// `extra_errors` is deliberate: `CreateCapability` takes `clusterName` as an
// @httpLabel, so a request can name a cluster that does not exist, and the only
// honest answer is the same 404 every other cluster-scoped operation gives. The
// model's error union for THIS operation omits the shape (the service declares it,
// and `DescribeCapability`, `DeleteCapability`, `DescribeCluster` and `DeleteCluster`
// all list it), so the widening keeps one rule across the resource instead of
// inventing a second one here.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#CreateCapability",
    extra_errors = ["ResourceNotFoundException", "ClientException"]
)]
#[derive(Debug)]
pub enum CreateCapabilityErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_capability::CreateCapabilityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateCapabilityError = crate::scaffolded::Scaffolded<CreateCapabilityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateCapabilityErrorModeled> for CreateCapabilityError {
    fn from(modeled: CreateCapabilityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateCapability` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCapability<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCapability<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The `CapabilityConfigurationResponse` a stored capability reports, given the type it
/// was created as and the `CapabilityConfigurationRequest` (if any) it was created with.
///
/// Every value a caller supplied is carried through unchanged, so a read-back returns
/// what was asked for — including an `ack.disabledServices` list given as empty, because
/// "`DescribeCapability` returns the list exactly as you supplied it". One default is
/// documented and so is materialised here rather than left absent: "If you don't specify
/// this value, or you omit the `ack` configuration entirely, the capability is created
/// with this value set to `false`" (`AckConfigRequest$enableCrossNamespace`), which is why
/// an `ACK` capability reports an `ack` configuration even when the request carried none.
///
/// The two members a real Argo CD capability would have SERVER-generated — `serverUrl`
/// ("the URL of the Argo CD server", for the web interface and API) and
/// `awsIdc.idcManagedApplicationArn` (an application "created and managed by Amazon EKS"
/// in IAM Identity Center) — are left unset: this emulator runs no Argo CD server and
/// creates no Identity Center application, so there is nothing to name, and inventing a
/// URL or an ARN for them would have a client dial an endpoint that does not exist. Both
/// are optional in the model.
fn configuration_response(
    capability_type: &str,
    request: Option<&CapabilityConfigurationRequest>,
) -> Option<CapabilityConfigurationResponse> {
    let argo_cd = request
        .and_then(|request| request.argo_cd.as_ref())
        .map(|argo_cd| ArgoCdConfigResponse {
            namespace: argo_cd.namespace.clone(),
            aws_idc: Some(ArgoCdAwsIdcConfigResponse {
                idc_instance_arn: Some(argo_cd.aws_idc.idc_instance_arn.clone()),
                idc_region: argo_cd.aws_idc.idc_region.clone(),
                idc_managed_application_arn: None,
            }),
            rbac_role_mappings: argo_cd.rbac_role_mappings.clone(),
            network_access: argo_cd
                .network_access
                .as_ref()
                .map(|network| ArgoCdNetworkAccessConfigResponse {
                    vpce_ids: network.vpce_ids.clone(),
                }),
            server_url: None,
            endpoint_prefix: argo_cd.endpoint_prefix.clone(),
        });
    let requested_ack = request.and_then(|request| request.ack.as_ref());
    let ack = (capability_type == CAPABILITY_TYPE_ACK).then(|| AckConfigResponse {
        enable_cross_namespace: Some(
            requested_ack
                .and_then(|ack| ack.enable_cross_namespace)
                .unwrap_or(false),
        ),
        disabled_services: requested_ack.and_then(|ack| ack.disabled_services.clone()),
    });
    (argo_cd.is_some() || ack.is_some()).then_some(CapabilityConfigurationResponse {
        argo_cd,
        ack,
    })
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCapability<Eng>
{
    type Input = CreateCapabilityInput;
    type Output = CreateCapabilityOutput;
    type Error = CreateCapabilityError;

    const NAME: &'static str = "CreateCapability";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CAPABILITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CAPABILITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateCapabilityError::from(CreateCapabilityErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let invalid_parameter = |message: String| {
            CreateCapabilityError::from(CreateCapabilityErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };
        let in_use = |message: String| {
            CreateCapabilityError::from(CreateCapabilityErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        let now = SystemTime::now();

        // AUTHORIZATION FIRST: `roleArn` is a role the capability has Amazon EKS assume,
        // so whether THIS caller may hand it over at all is decided from the value
        // alone ([`state::pass_role_refusal`]) — a permission question that needs no
        // state and precedes every report about the resources the request names.
        match state::pass_role_refusal(&input.role_arn) {
            None => {}
            Some(why) => {
                return Err(CreateCapabilityErrorModeled::AccessDeniedException(
                    AccessDeniedException { message: Some(why) },
                )
                .into());
            }
        }

        // THE PARENT RESOURCE NEXT, BEFORE ANY OTHER MEMBER IS JUDGED: `clusterName` is
        // this operation's `@httpLabel`, and a capability hangs off ONE cluster in the
        // region the request is signed for, so a `clusterName` this store does not hold
        // is answered with the one rule this resource has for an unknown parent — the
        // same 404 `DescribeCapability`, `DeleteCapability` and `ListCapabilities` give.
        // It is resolved before the body is validated and before `roleArn` is resolved
        // against IAM, so a create against a cluster that does not exist reports the
        // MISSING CLUSTER rather than a complaint about a sibling member: the caller's
        // recovery is to create the parent and retry. The ONE member judged ahead of it
        // is `type`, which names no capability type of this API at all and so is
        // answered before dispatch (see the ORDER note in this module's header).
        //
        // The lock is taken for the resolution alone and released again, because the IAM
        // call below is an `await` and this store's guard is not held across one.
        let cluster_connected = {
            let mut guard = store.lock();
            guard.materialize_due_transitions(now);
            guard
                .clusters
                .get(&(region.clone(), input.cluster_name.clone()))
                .map(|cluster| cluster.is_connected())
        };
        let Some(cluster_connected) = cluster_connected else {
            return Err(CreateCapabilityErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };
        // A managed capability is software Amazon EKS installs and runs in the cluster's
        // control plane, and a cluster `RegisterCluster` connected has none of that
        // plane: this operation does not apply to it. The gate is a fact about the
        // CLUSTER, so it is answered as soon as the cluster is resolved — before the role
        // the body names is resolved and before the cluster's status is judged.
        if cluster_connected {
            return Err(CreateCapabilityErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        // The cluster resolved, so the role it names can be resolved too: IAM holds the
        // role and its trust policy must let the Amazon EKS service principal assume it
        // ("This role must have a trust policy that allows the EKS service principal to
        // assume it" — [`state::service_principal_trust_violation`]). It runs with no
        // lock held: it is a call out to another service.
        if let Some(why) = state::service_principal_trust_violation(
            ctx,
            &region,
            &input.role_arn,
            EKS_SERVICE_PRINCIPAL,
        )
        .await
        {
            return Err(invalid_parameter(format!("roleArn {why}")));
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // The lock was released for the IAM call, so the cluster is resolved again — and
        // reported the same way when a delete landed in between.
        let Some(cluster_status) = guard
            .clusters
            .get(&(region.clone(), input.cluster_name.clone()))
            .map(|cluster| cluster.status.clone())
        else {
            return Err(CreateCapabilityErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };

        // "The name must be unique within your cluster and can contain alphanumeric
        // characters, hyphens, and underscores." The shape carries no `@pattern`, so
        // the charset half of that rule is not refused before this handler runs.
        if !state::is_valid_capability_name(&input.capability_name) {
            return Err(invalid_parameter(format!(
                "capabilityName must be a non-empty string of alphanumeric characters, \
                 hyphens and underscores, got {:?}",
                input.capability_name
            )));
        }

        // "The structure of this object varies depending on the capability type": each
        // member of `configuration` belongs to exactly one `CapabilityType`, so a
        // configuration the requested type has no use for is a bad parameter rather
        // than something to store and report back unused.
        if let Some(configuration) = input.configuration.as_ref() {
            for (member, owning_type, supplied) in [
                (
                    "argoCd",
                    CAPABILITY_TYPE_ARGOCD,
                    configuration.argo_cd.is_some(),
                ),
                ("ack", CAPABILITY_TYPE_ACK, configuration.ack.is_some()),
            ] {
                if supplied && input.r#type != owning_type {
                    return Err(invalid_parameter(format!(
                        "configuration.{member} applies only to a capability of type \
                         {owning_type}, not to type {}",
                        input.r#type
                    )));
                }
            }
        }

        // Amazon EKS "provisions and manages the infrastructure required to run the
        // capability" for a live control plane, so a cluster that is still coming up
        // or going away cannot take one — which is what `InvalidRequestException`
        // ("invalid given the state of the cluster") reports.
        if cluster_status != STATUS_ACTIVE {
            return Err(CreateCapabilityErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(format!(
                        "Cluster {} is in {cluster_status} status; a capability can only be \
                         created on a cluster in {STATUS_ACTIVE} status.",
                        input.cluster_name
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let key = (
            region.clone(),
            input.cluster_name.clone(),
            input.capability_name.clone(),
        );
        if let Some(existing) = guard.capabilities.get(&key) {
            // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
            // call and RESENDS the same value when it retries, so a retry of the call
            // that created this capability is absorbed and answered with that
            // capability. A blank token identifies nothing, so it never matches.
            let replayed_token = input
                .client_request_token
                .as_deref()
                .filter(|token| !token.trim().is_empty());
            if replayed_token.is_some_and(|token| {
                existing.client_request_token.as_deref() == Some(token)
            }) {
                return Ok(CreateCapabilityOutput {
                    capability: Some(existing.to_capability()),
                });
            }
            return Err(in_use(format!(
                "Capability already exists with name: {} in cluster: {}",
                input.capability_name, input.cluster_name
            )));
        }

        // "You can only create one Capability of each type on a given Amazon EKS
        // cluster."
        if let Some(conflicting) = guard
            .capabilities_of(&region, &input.cluster_name)
            .find(|record| record.r#type == input.r#type)
            .map(|record| record.capability_name.clone())
        {
            return Err(in_use(format!(
                "Cluster {} already has a capability of type {}: {conflicting}",
                input.cluster_name, input.r#type
            )));
        }

        let record = CapabilityRecord {
            capability_name: input.capability_name.clone(),
            cluster_name: input.cluster_name.clone(),
            arn: state::capability_arn(&region, &input.cluster_name, &input.capability_name),
            r#type: input.r#type.clone(),
            role_arn: input.role_arn.clone(),
            status: CAPABILITY_STATUS_CREATING.to_string(),
            active_at: Some(now + CAPABILITY_ACTIVATION_DELAY),
            configuration: configuration_response(&input.r#type, input.configuration.as_ref()),
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            // Nothing in this emulator runs the capability, so it starts with no
            // health issues and nothing adds one.
            health_issues: Vec::new(),
            created_at: now,
            modified_at: now,
            delete_propagation_policy: input.delete_propagation_policy.clone(),
            client_request_token: input.client_request_token.clone(),
            // A capability takes no update until `UpdateCapability` starts one.
            pending_update: None,
        };

        let capability = record.to_capability();
        guard.capabilities.insert(key, record);

        Ok(CreateCapabilityOutput {
            capability: Some(capability),
        })
    }
}
