// SPDX-License-Identifier: CC0-1.0

//! @op CreatePodIdentityAssociation
//! IMPLEMENTED. Writes the [`crate::state::PodIdentityAssociationRecord`] that
//! `DescribePodIdentityAssociation`, `UpdatePodIdentityAssociation`,
//! `DeletePodIdentityAssociation` and `ListPodIdentityAssociations` read: the
//! `associationId`, `associationArn`, `externalId`, `createdAt` and `modifiedAt`
//! it answers with are minted once here and stored, never recomputed on a read.
//!
//! Every identifier the request carries is checked for its documented FORM before
//! any resource is resolved — the cluster name, the IAM role ARNs and the
//! Kubernetes names — so a request that cannot name anything is an
//! `InvalidParameterException` rather than a not-found.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreatePodIdentityAssociation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates an EKS Pod Identity association between a service account in an Amazon EKS
//! cluster and an IAM role with EKS Pod Identity. Use EKS Pod Identity to give temporary
//! IAM credentials to Pods and the credentials are rotated automatically. Amazon EKS Pod
//! Identity associations provide the ability to manage credentials for your applications,
//! similar to the way that Amazon EC2 instance profiles provide credentials to Amazon EC2
//! instances. If a Pod uses a service account that has an association, Amazon EKS sets
//! environment variables in the containers of the Pod. The environment variables configure
//! the Amazon Web Services SDKs, including the Command Line Interface, to use the EKS Pod
//! Identity credentials. EKS Pod Identity is a simpler method than IAM roles for service
//! accounts, as this method doesn't use OIDC identity providers. Additionally, you can
//! configure a role for EKS Pod Identity once, and reuse it across clusters. Similar to
//! Amazon Web Services IAM behavior, EKS Pod Identity associations are eventually
//! consistent, and may take several seconds to be effective after the initial API call
//! returns successfully. You must design your applications to account for these potential
//! delays. We recommend that you don’t include association create/updates in the critical,
//! high-availability code paths of your application. Instead, make changes in a separate
//! initialization or setup routine that you run less frequently. You can set a target IAM
//! role in the same or a different account for advanced scenarios. With a target role, EKS
//! Pod Identity automatically performs two role assumptions in sequence: first assuming the
//! role in the association that is in this account, then using those credentials to assume
//! the target IAM role. This process provides your Pod with temporary credentials that have
//! the permissions defined in the target role, allowing secure access to resources in
//! another Amazon Web Services account.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of the cluster to create the EKS Pod
//!   Identity association in.
//! * `disableSessionTags` (BoxedBoolean) — Disable the automatic sessions tags that are
//!   appended by EKS Pod Identity. EKS Pod Identity adds a pre-defined set of session tags
//!   when it assumes the role. You can use these tags to author a single role that can work
//!   across resources by allowing access to Amazon Web Services resources based on matching
//!   tags. By default, EKS Pod Identity attaches six tags, including tags for cluster name,
//!   namespace, and service account name. For the list of tags added by EKS Pod Identity, see
//!   List of session tags added by EKS Pod Identity in the Amazon EKS User Guide. Amazon Web
//!   Services compresses inline session policies, managed policy ARNs, and session tags into
//!   a packed binary format that has a separate …
//! * `namespace` (String) **required** — The name of the Kubernetes namespace inside the
//!   cluster to create the EKS Pod Identity association in. The service account and the Pods
//!   that use the service account must be in this namespace.
//! * `policy` (String) — An optional IAM policy in JSON format (as an escaped string) that
//!   applies additional restrictions to this pod identity association beyond the IAM policies
//!   attached to the IAM role. This policy is applied as the intersection of the role's
//!   policies and this policy, allowing you to reduce the permissions that applications in
//!   the pods can use. Use this policy to enforce least privilege access while still
//!   leveraging a shared IAM role across multiple applications. Important considerations
//!   Session tags: When using this policy, disableSessionTags must be set to true. Target
//!   role permissions: If you specify both a TargetRoleArn and a policy, the policy
//!   restrictions apply only to the target role's …
//! * `roleArn` (String) **required** — The Amazon Resource Name (ARN) of the IAM role to
//!   associate with the service account. The EKS Pod Identity agent manages credentials to
//!   assume this role for applications in the containers in the Pods that use this service
//!   account.
//! * `serviceAccount` (String) **required** — The name of the Kubernetes service account
//!   inside the cluster to associate the IAM credentials with.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources. The following basic restrictions
//!   apply to tags: Maximum number of tags per resource – 50 For each resource, each tag key
//!   must be unique, and each tag key can have only one value. Maximum key length – 128
//!   Unicode characters in UTF-8 Maximum value length – 256 Unicode characters in UTF-8 If
//!   your tagging schema is used across multiple services and resources, remember that other
//!   services may have restrictions on allowed characters. Generally allowed characters are:
//!   letters, numbers, and spaces …
//! * `targetRoleArn` (String) — The Amazon Resource Name (ARN) of the target IAM role to
//!   associate with the service account. This role is assumed by using the EKS Pod Identity
//!   association role, then the credentials for this role are injected into the Pod. When you
//!   run applications on Amazon EKS, your application might need to access Amazon Web
//!   Services resources from a different role that exists in the same or different Amazon Web
//!   Services account. For example, your application running in “Account A” might need to
//!   access resources, such as Amazon S3 buckets in “Account B” or within “Account A” itself.
//!   You can create a association to access Amazon Web Services resources in “Account B” by
//!   creating two IAM roles: a role in …
//!
//! ## Output
//!
//! * `association` (PodIdentityAssociation) — The full description of your new association.
//!   The description includes an ID for the association. Use the ID of the association in
//!   further actions to manage the association.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, PodIdentityAssociationRecord, KUBERNETES_NAME_PATTERN,
    MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreatePodIdentityAssociationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreatePodIdentityAssociationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#CreatePodIdentityAssociation")]
#[derive(Debug)]
pub enum CreatePodIdentityAssociationErrorModeled {
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_pod_identity_association::CreatePodIdentityAssociationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreatePodIdentityAssociationError = crate::scaffolded::Scaffolded<CreatePodIdentityAssociationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreatePodIdentityAssociationErrorModeled> for CreatePodIdentityAssociationError {
    fn from(modeled: CreatePodIdentityAssociationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePodIdentityAssociation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreatePodIdentityAssociation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePodIdentityAssociation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePodIdentityAssociation<Eng>
{
    type Input = CreatePodIdentityAssociationInput;
    type Output = CreatePodIdentityAssociationOutput;
    type Error = CreatePodIdentityAssociationError;

    const NAME: &'static str = "CreatePodIdentityAssociation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POD_IDENTITY_ASSOCIATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreatePodIdentityAssociationError::from(
                CreatePodIdentityAssociationErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        // Every refusal below names the member, the value and the constraint it
        // failed, the way the boundary validator does: the caller has to fix the
        // request from this text alone.
        let invalid_parameter = |message: String| {
            CreatePodIdentityAssociationError::from(
                CreatePodIdentityAssociationErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        // FORM FIRST: an identifier outside its documented form names nothing, in
        // any account or Region, so it is an invalid parameter and never a
        // not-found. None of these members carries a modelled constraint, so
        // nothing refused them before this handler ran.
        if !state::is_cluster_name(&input.cluster_name) {
            return Err(invalid_parameter(state::cluster_name_refusal(
                "clusterName",
                &input.cluster_name,
            )));
        }
        for (member, value) in [
            ("namespace", &input.namespace),
            ("serviceAccount", &input.service_account),
        ] {
            if !state::is_kubernetes_name(value) {
                return Err(invalid_parameter(format!(
                    "Value '{value}' at '{member}' failed to satisfy constraint: Member \
                     must satisfy regular expression pattern: {KUBERNETES_NAME_PATTERN}"
                )));
            }
        }
        for (member, value) in [
            ("roleArn", Some(&input.role_arn)),
            ("targetRoleArn", input.target_role_arn.as_ref()),
        ] {
            if let Some(value) = value {
                if !state::is_iam_role_arn(value) {
                    return Err(invalid_parameter(format!(
                        "Value '{value}' at '{member}' failed to satisfy constraint: Member \
                         must be an IAM role ARN"
                    )));
                }
            }
        }
        // `policy`'s own documentation: "Session tags: When using this policy,
        // disableSessionTags must be set to true."
        if input.policy.is_some() && input.disable_session_tags != Some(true) {
            return Err(invalid_parameter(
                "Value at 'policy' requires 'disableSessionTags' to be true".to_string(),
            ));
        }

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // An association is a child of a cluster, so the cluster is resolved
        // before anything is written: "You can view your available clusters with
        // ListClusters."
        if !guard
            .clusters
            .contains_key(&(region.clone(), input.cluster_name.clone()))
        {
            return Err(
                CreatePodIdentityAssociationErrorModeled::ResourceNotFoundException(
                    crate::state::cluster_not_found(&input.cluster_name),
                )
                .into(),
            );
        }

        // One service account in one namespace can hold at most one association:
        // the pair is what EKS Pod Identity resolves a Pod's credentials by, so a
        // second association for the same pair would make that lookup ambiguous.
        let existing = guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .find(|record| {
                record.namespace == input.namespace
                    && record.service_account == input.service_account
            })
            .cloned();
        if let Some(existing) = existing {
            // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once
            // per call and RESENDS the same value when it retries, so a retry of
            // the call that created this association is absorbed and answered with
            // that association rather than with a conflict. A blank token
            // identifies nothing, so it never matches.
            let replayed = input
                .client_request_token
                .as_deref()
                .filter(|token| !token.trim().is_empty());
            if let Some(token) = replayed {
                if existing.client_request_token.as_deref() == Some(token) {
                    return Ok(CreatePodIdentityAssociationOutput {
                        association: Some(existing.to_association()),
                    });
                }
            }
            return Err(
                CreatePodIdentityAssociationErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "Association already exists with namespace: {} and service \
                             account: {}",
                            input.namespace, input.service_account
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        if guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .count()
            >= MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER
        {
            return Err(
                CreatePodIdentityAssociationErrorModeled::ResourceLimitExceededException(
                    ResourceLimitExceededException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "You have reached the limit of \
                             {MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER} EKS Pod Identity \
                             associations for cluster {}",
                            input.cluster_name
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let association_id = guard.mint_association_id(
            &region,
            &input.cluster_name,
            &input.namespace,
            &input.service_account,
        );
        let association_arn =
            state::derive_association_arn(&region, &input.cluster_name, &association_id);
        let record = PodIdentityAssociationRecord {
            cluster_name: input.cluster_name.clone(),
            association_id: association_id.clone(),
            // `externalId` exists to be matched in the TARGET role's trust policy,
            // so an association with no target role has nothing to condition and
            // carries none.
            external_id: input
                .target_role_arn
                .as_ref()
                .map(|_| state::derive_association_external_id(&association_arn)),
            association_arn,
            namespace: input.namespace.clone(),
            service_account: input.service_account.clone(),
            role_arn: input.role_arn.clone(),
            target_role_arn: input.target_role_arn.clone(),
            policy: input.policy.clone(),
            disable_session_tags: input.disable_session_tags,
            tags: input
                .tags
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            // A create is also the association's last modification, so both
            // timestamps start equal; only an update moves `modifiedAt`.
            created_at: now,
            modified_at: now,
            client_request_token: input.client_request_token.clone(),
            // No update has applied to a brand-new association.
            update_request_token: None,
            // An association created through THIS API is owned by the caller:
            // `ownerArn` is set only on one an Amazon EKS add-on created
            // (`CreateAddon$podIdentityAssociations`).
            owner_arn: None,
        };

        let association = record.to_association();
        guard.pod_identity_associations.insert(
            (region, input.cluster_name, association_id),
            record,
        );

        Ok(CreatePodIdentityAssociationOutput {
            association: Some(association),
        })
    }
}
