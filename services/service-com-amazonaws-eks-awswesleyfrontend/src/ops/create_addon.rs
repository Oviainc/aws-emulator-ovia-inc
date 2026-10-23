// SPDX-License-Identifier: CC0-1.0

//! @op CreateAddon
//!
//! IMPLEMENTED. `CreateAddon` installs one add-on ON an existing cluster: it mints a
//! [`crate::state::AddonRecord`] in the shared engine's store, keyed by the cluster's
//! own `(region, name)` key plus the add-on name, and answers with the `Addon` it just
//! wrote. `DescribeAddon` reads that same record back.
//!
//! An add-on is a child of a cluster, so the layers this handler checks run from the
//! OUTSIDE IN, and each layer's refusal is the modelled error for what it found:
//!
//! 1. the cluster named by the `@httpLabel` must exist IN THE REQUEST'S REGION —
//!    otherwise `ResourceNotFoundException` (404), before any member of the body is
//!    looked at, because a request against a cluster that does not exist cannot be
//!    answered whatever else it says;
//! 2. that cluster must be `ACTIVE` — otherwise `InvalidRequestException` (400), the
//!    modelled error for "the request is invalid given the state of the cluster";
//! 3. the body's own members must hold up — otherwise `InvalidParameterException`
//!    (400), naming the member and what it must be;
//! 4. the add-on must not already be installed — otherwise `ResourceInUseException`
//!    (409), unless the request replays the `clientRequestToken` that created it, in
//!    which case that add-on is returned again. This layer runs BEFORE the checks on
//!    the child resources the add-on would create, so a retry of a call that already
//!    succeeded is absorbed rather than colliding with its own children;
//! 5. the EKS Pod Identity associations the add-on would create must fit the
//!    cluster's association ceiling and must not double-book a `(namespace, service
//!    account)` pair — otherwise `InvalidRequestException` (400) or
//!    `ResourceInUseException` (409), decided before anything is written, so a
//!    request that cannot be carried out in full writes nothing.
//!
//! The record carries both the caller's input and the server-generated parts
//! (`addonArn`, `createdAt`, `modifiedAt`, the assigned version, the namespace the
//! add-on went into, the ARNs of the EKS Pod Identity associations it owns), so a
//! later read returns exactly these bytes rather than recomputing them.
//!
//! An add-on asked for `podIdentityAssociations` CREATES those associations: one
//! [`crate::state::PodIdentityAssociationRecord`] per entry, minted through the same
//! path `CreatePodIdentityAssociation` uses and carrying this add-on's ARN as its
//! `ownerArn`, so `DescribePodIdentityAssociation` and `ListPodIdentityAssociations`
//! find exactly the associations the add-on reports.
//!
//! A new add-on is reported `CREATING` and the clock owes it `ACTIVE`
//! [`crate::state::ADDON_ACTIVATION_DELAY`] later; the due instant is stored on the
//! record and materialised by whichever read first runs after it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateAddon`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates an Amazon EKS add-on. Amazon EKS add-ons help to automate the provisioning and
//! lifecycle management of common operational software for Amazon EKS clusters. For more
//! information, see Amazon EKS add-ons in the Amazon EKS User Guide.
//!
//! ## Input
//!
//! * `addonName` (String) **required** — The name of the add-on. The name must match one of
//!   the names returned by DescribeAddonVersions.
//! * `addonVersion` (String) — The version of the add-on. The version must match one of the
//!   versions returned by DescribeAddonVersions .
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$)
//!   **required** — The name of your cluster.
//! * `configurationValues` (String) — The set of configuration values for the add-on that's
//!   created. The values that you provide are validated against the schema returned by
//!   DescribeAddonConfiguration.
//! * `namespaceConfig` (AddonNamespaceConfigRequest) — The namespace configuration for the
//!   addon. If specified, this will override the default namespace for the addon.
//! * `podIdentityAssociations` (AddonPodIdentityAssociationsList) — An array of EKS Pod
//!   Identity associations to be created. Each association maps a Kubernetes service account
//!   to an IAM role. For more information, see Attach an IAM Role to an Amazon EKS add-on
//!   using EKS Pod Identity in the Amazon EKS User Guide.
//! * `resolveConflicts` (ResolveConflicts, one of: NONE | OVERWRITE | PRESERVE) — How to
//!   resolve field value conflicts for an Amazon EKS add-on. Conflicts are handled based on
//!   the value you choose: None – If the self-managed version of the add-on is installed on
//!   your cluster, Amazon EKS doesn't change the value. Creation of the add-on might fail.
//!   Overwrite – If the self-managed version of the add-on is installed on your cluster and
//!   the Amazon EKS default value is different than the existing value, Amazon EKS changes
//!   the value to the Amazon EKS default value. Preserve – This is similar to the NONE
//!   option. If the self-managed version of the add-on is installed on your cluster Amazon
//!   EKS doesn't change the add-on resource properties. Creation of the add-on might fail if
//!   …
//! * `serviceAccountRoleArn` (RoleArn, len 1..255) — The Amazon Resource Name (ARN) of an
//!   existing IAM role to bind to the add-on's service account. The role must be assigned the
//!   IAM permissions required by the add-on. If you don't specify an existing IAM role, then
//!   the add-on uses the permissions assigned to the node IAM role. For more information, see
//!   Amazon EKS node IAM role in the Amazon EKS User Guide. To specify an existing IAM role,
//!   you must have an IAM OpenID Connect (OIDC) provider created for your cluster. For more
//!   information, see Enabling IAM roles for service accounts on your cluster in the Amazon
//!   EKS User Guide.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources.
//!
//! ## Output
//!
//! * `addon` (Addon) — An Amazon EKS add-on. For more information, see Amazon EKS add-ons in
//!   the Amazon EKS User Guide.
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

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, AddonRecord, PodIdentityAssociationRecord, ADDON_ACTIVATION_DELAY,
    ADDON_DEFAULT_NAMESPACE, ADDON_OWNER_AWS, ADDON_PUBLISHER_EKS, ADDON_STATUS_CREATING,
    DEFAULT_KUBERNETES_VERSION, MAX_ADDONS_PER_CLUSTER,
    MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER, STATUS_ACTIVE,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateAddonError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAddonError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#CreateAddon")]
#[derive(Debug)]
pub enum CreateAddonErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_addon::CreateAddonError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAddonError = crate::scaffolded::Scaffolded<CreateAddonErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAddonErrorModeled> for CreateAddonError {
    fn from(modeled: CreateAddonErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAddon` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAddon<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAddon<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAddon<Eng>
{
    type Input = CreateAddonInput;
    type Output = CreateAddonOutput;
    type Error = CreateAddonError;

    const NAME: &'static str = "CreateAddon";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ADDON_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ADDON_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateAddonError::from(CreateAddonErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.cluster_name.clone()),
                addon_name: Some(input.addon_name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // An `InvalidParameterException` naming the member that is wrong: "The
        // specified parameter is invalid. Review the available parameters for the API
        // request." Built here so every parameter layer below reports the same way.
        let invalid_parameter = |message: String| {
            CreateAddonError::from(CreateAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // LAYER 1 — the cluster. An add-on is created ON a cluster
        // (`POST /clusters/{clusterName}/addons`) and a cluster name identifies a
        // cluster only within one region, so the cluster this installs onto is the one
        // in the region the request is signed for. Nothing in the body can make a
        // request against a cluster that does not exist answerable, so this is checked
        // FIRST and reported as the modelled 404.
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            return Err(CreateAddonErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "No cluster found for name: {}.",
                        input.cluster_name
                    )),
                    ..Default::default()
                },
            )
            .into());
        };

        // LAYER 2 — the cluster's state. `InvalidRequestException` is "The request is
        // invalid given the state of the cluster. Check the state of the cluster and
        // the associated operations": an add-on is installed into a running control
        // plane, so a cluster that is still `CREATING` (or deleting, updating, failed)
        // cannot take one.
        if cluster.status != STATUS_ACTIVE {
            return Err(CreateAddonErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "Cluster {} is in {} status; it must be {STATUS_ACTIVE} to create \
                         an add-on.",
                        input.cluster_name, cluster.status
                    )),
                    ..Default::default()
                },
            )
            .into());
        }
        // The cluster's own Kubernetes version, which the add-on's default version is
        // taken from. A cluster `RegisterCluster` connected reports none (its control
        // plane is not EKS's), so the emulator's default Kubernetes version stands in.
        let cluster_version = cluster
            .version
            .clone()
            .unwrap_or_else(|| DEFAULT_KUBERNETES_VERSION.to_string());

        // LAYER 3 — the body's own members. Each is a rule the model cannot express,
        // so the boundary validator passed the request and this is where it is caught.
        if !state::is_addon_name_shaped(&input.addon_name) {
            return Err(invalid_parameter(format!(
                "addonName {:?} is not an add-on name: it must be a non-empty string of \
                 letters, digits, '-', '.' and '_'.",
                input.addon_name
            )));
        }
        if let Some(role_arn) = input
            .service_account_role_arn
            .as_deref()
            .filter(|arn| !state::is_iam_role_arn(arn))
        {
            return Err(invalid_parameter(format!(
                "serviceAccountRoleArn {role_arn:?} is not an IAM role ARN of the form \
                 arn:<partition>:iam::<account>:role/<name>."
            )));
        }
        // "The set of configuration values for the add-on that's created. The values
        // that you provide are validated against the schema returned by
        // DescribeAddonConfiguration" — that operation is still a scaffold here (its
        // own gap issue), so there is no schema to validate against. What IS checked is
        // that a document which opens like JSON is well-formed JSON: an add-on
        // configuration is JSON or YAML, and a JSON document that does not parse can
        // never satisfy any schema.
        if let Some(values) = input
            .configuration_values
            .as_deref()
            .map(str::trim)
            .filter(|values| values.starts_with('{') || values.starts_with('['))
        {
            if let Err(error) = serde_json::from_str::<serde_json::Value>(values) {
                return Err(invalid_parameter(format!(
                    "configurationValues opens like JSON but is not valid JSON: {error}"
                )));
            }
        }
        // `podIdentityAssociations` maps a Kubernetes service account to an IAM role,
        // so each entry's role must be a role ARN and each service account must be a
        // name a Kubernetes object can have, and one service account cannot be mapped
        // twice by the same add-on. Both form checks are the ones the pod identity
        // slice already shares ([`state::is_iam_role_arn`],
        // [`state::is_kubernetes_name`]).
        //
        // The list's LENGTH is refused first, before any per-entry work: the model
        // puts no `length` on `AddonPodIdentityAssociationsList`, so the boundary
        // validator accepts a list of any size, and a list longer than a cluster may
        // ever hold cannot be carried out whatever its entries say. Every loop below
        // therefore runs over at most
        // [`MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER`] entries.
        let requested_associations = input.pod_identity_associations.clone().unwrap_or_default();
        // Reported as `InvalidRequestException` rather than the
        // `ResourceLimitExceededException` `CreatePodIdentityAssociation` answers for
        // the same ceiling: `CreateAddon` does not model that error, and a cluster
        // that cannot hold the associations asked for is a statement about the state
        // of the cluster.
        let too_many = |requested: usize, held: Option<usize>| {
            // The held count is stated only where it was MEASURED: the
            // length-only refusal below returns before the store is walked.
            let held = match held {
                Some(held) => format!(" and cluster {} already holds {held}", input.cluster_name),
                None => String::new(),
            };
            CreateAddonError::from(CreateAddonErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "podIdentityAssociations asks for {requested} associations{held}; a \
                         cluster holds at most \
                         {MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER} EKS Pod Identity \
                         associations."
                    )),
                    ..Default::default()
                },
            ))
        };
        if requested_associations.len() > MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER {
            // The list alone is past the ceiling, so no walk of the store is needed to
            // know the request cannot be carried out.
            return Err(too_many(requested_associations.len(), None));
        }
        let mut seen_service_accounts = ::std::collections::BTreeSet::new();
        for association in &requested_associations {
            if !state::is_iam_role_arn(&association.role_arn) {
                return Err(invalid_parameter(format!(
                    "podIdentityAssociations[].roleArn {:?} is not an IAM role ARN of the \
                     form arn:<partition>:iam::<account>:role/<name>.",
                    association.role_arn
                )));
            }
            if !state::is_kubernetes_name(&association.service_account) {
                return Err(invalid_parameter(format!(
                    "podIdentityAssociations[].serviceAccount {:?} is not a Kubernetes \
                     object name (pattern {}).",
                    association.service_account,
                    state::KUBERNETES_NAME_PATTERN
                )));
            }
            if !seen_service_accounts.insert(association.service_account.clone()) {
                return Err(invalid_parameter(format!(
                    "podIdentityAssociations names serviceAccount {:?} more than once; \
                     each service account can be mapped at most once.",
                    association.service_account
                )));
            }
        }
        // `namespaceConfig.namespace` names a Kubernetes namespace in the cluster, so
        // a value no Kubernetes namespace can be named is refused rather than stored.
        // The namespace the add-on goes into is also the namespace of the service
        // accounts its EKS Pod Identity associations are for.
        if let Some(namespace) = input
            .namespace_config
            .as_ref()
            .and_then(|config| config.namespace.as_deref())
        {
            if !state::is_kubernetes_name(namespace) {
                return Err(invalid_parameter(format!(
                    "namespaceConfig.namespace {namespace:?} is not a Kubernetes object \
                     name (pattern {}).",
                    state::KUBERNETES_NAME_PATTERN
                )));
            }
        }
        let namespace = input
            .namespace_config
            .as_ref()
            .and_then(|config| config.namespace.clone())
            .unwrap_or_else(|| ADDON_DEFAULT_NAMESPACE.to_string());

        // LAYER 4 — the add-on itself. One add-on name installs at most once per
        // cluster. This layer runs BEFORE the checks on the child associations below:
        // those exist to stop a request that cannot be carried out in full from
        // writing anything, which says nothing about a call whose write ALREADY
        // happened — a retry of it would otherwise collide with its own associations
        // and be answered with a conflict instead of being absorbed.
        let addon_key = (
            region.clone(),
            input.cluster_name.clone(),
            input.addon_name.clone(),
        );
        if let Some(existing) = guard.addons.get(&addon_key) {
            // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
            // call and RESENDS the same value when it retries, so a retry of the call
            // that installed this add-on is absorbed and answered with that add-on
            // rather than with a conflict. A blank token identifies nothing, so it
            // never matches — absorbing on it would make two unrelated calls that both
            // sent `""` look like a retry of each other.
            //
            // The comparison is on the TOKEN alone: this does not check that the rest
            // of the retry's body matches the stored record (the same simplification
            // the cluster slice makes). An SDK retry resends the identical body, so the
            // case this would catch is a caller reusing a token for a different
            // request, which the real service refuses as an invalid parameter.
            let replayed_token = input
                .client_request_token
                .as_deref()
                .filter(|token| !token.trim().is_empty());
            if let Some(token) = replayed_token {
                if existing.client_request_token.as_deref() == Some(token) {
                    return Ok(CreateAddonOutput {
                        addon: Some(existing.to_addon()),
                    });
                }
            }
            return Err(CreateAddonErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "Addon {} is already installed on cluster {}.",
                        input.addon_name, input.cluster_name
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // What bounds the store: add-on names are caller-supplied, so without a
        // ceiling repeated creates would let one cluster retain an unbounded number of
        // records. A cluster that already holds its maximum cannot take another
        // add-on, which is a statement about the state of the cluster.
        if guard
            .addon_names_in(&region, &input.cluster_name)
            .count()
            >= MAX_ADDONS_PER_CLUSTER
        {
            return Err(CreateAddonErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "Cluster {} already has the limit of {MAX_ADDONS_PER_CLUSTER} \
                         add-ons.",
                        input.cluster_name
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // LAYER 5 — the child resources this add-on would create. The associations
        // count against the cluster's association ceiling like any others, so a
        // request that would push the cluster past it is refused BEFORE the store is
        // walked for the pairs below: the ceiling bounds both the stored side and the
        // requested side of that comparison.
        let held_associations = guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .count();
        if held_associations + requested_associations.len()
            > MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER
        {
            return Err(too_many(
                requested_associations.len(),
                Some(held_associations),
            ));
        }

        // An association the add-on would create for a `(namespace, service account)`
        // pair that already holds one would make the credential lookup that pair
        // resolves ambiguous, which is why the pod identity slice refuses a second
        // association for it; the same refusal belongs here, BEFORE anything is
        // written, so a request that cannot be carried out in full writes nothing.
        //
        // The cluster's pairs are collected in ONE walk and each requested entry is
        // looked up in that set, rather than re-walking the store per entry: the work
        // is the sum of the two bounded sides, not their product.
        let taken_service_accounts: ::std::collections::BTreeSet<&str> = guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .filter(|record| record.namespace == namespace)
            .map(|record| record.service_account.as_str())
            .collect();
        if let Some(association) = requested_associations
            .iter()
            .find(|association| taken_service_accounts.contains(association.service_account.as_str()))
        {
            return Err(CreateAddonErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "Association already exists with namespace: {namespace} and \
                         service account: {}",
                        association.service_account
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let resource_id =
            state::derive_addon_resource_id(&region, &input.cluster_name, &input.addon_name);
        let addon_arn = state::derive_addon_arn(
            &region,
            &input.cluster_name,
            &input.addon_name,
            &resource_id,
        );

        // The add-on OWNS the EKS Pod Identity associations it was asked for, so each
        // one is created here as a record of its own — through the same minting path
        // `CreatePodIdentityAssociation` uses, so the id is unique within the cluster
        // and has the form every other association id has — carrying this add-on's ARN
        // as its `ownerArn`. `DescribePodIdentityAssociation` and
        // `ListPodIdentityAssociations` therefore find exactly the associations whose
        // ARNs the add-on reports.
        let mut association_arns = Vec::with_capacity(requested_associations.len());
        for association in &requested_associations {
            let association_id = guard.mint_association_id(
                &region,
                &input.cluster_name,
                &namespace,
                &association.service_account,
            );
            let association_arn =
                state::derive_association_arn(&region, &input.cluster_name, &association_id);
            association_arns.push(association_arn.clone());
            let association_record = PodIdentityAssociationRecord {
                cluster_name: input.cluster_name.clone(),
                association_id: association_id.clone(),
                association_arn,
                namespace: namespace.clone(),
                service_account: association.service_account.clone(),
                role_arn: association.role_arn.clone(),
                // `CreateAddon` asks only for a service account and a role, so an
                // association it creates names no target role — and `externalId`
                // exists to be matched in a TARGET role's trust policy, so there is
                // nothing to condition — no permissions policy and no session-tag
                // override.
                target_role_arn: None,
                external_id: None,
                policy: None,
                disable_session_tags: None,
                // "Tags don't propagate to any other cluster or Amazon Web Services
                // resources" (`CreateAddon$tags`), so the add-on's tags are not copied
                // onto the associations it creates.
                tags: ::std::collections::BTreeMap::new(),
                created_at: now,
                modified_at: now,
                // The association is created by the add-on, not by a
                // `CreatePodIdentityAssociation` call, so no association-level
                // idempotency token applies to it.
                client_request_token: None,
                update_request_token: None,
                owner_arn: Some(addon_arn.clone()),
            };
            guard.pod_identity_associations.insert(
                (region.clone(), input.cluster_name.clone(), association_id),
                association_record,
            );
        }

        let record = AddonRecord {
            arn: addon_arn,
            // The caller's version if it named one, else the default version for the
            // cluster's Kubernetes version.
            addon_version: input
                .addon_version
                .clone()
                .filter(|version| !version.trim().is_empty())
                .unwrap_or_else(|| state::derive_default_addon_version(&cluster_version)),
            status: ADDON_STATUS_CREATING.to_string(),
            created_at: now,
            modified_at: now,
            active_at: Some(now + ADDON_ACTIVATION_DELAY),
            service_account_role_arn: input.service_account_role_arn.clone(),
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            configuration_values: input.configuration_values.clone(),
            // The ARNs of the associations created just above, in the order the
            // request asked for them.
            pod_identity_association_arns: association_arns,
            namespace,
            publisher: ADDON_PUBLISHER_EKS.to_string(),
            owner: ADDON_OWNER_AWS.to_string(),
            client_request_token: input.client_request_token.clone(),
            cluster_name: input.cluster_name.clone(),
            addon_name: input.addon_name.clone(),
            // `resolveConflicts` decides what happens to a SELF-MANAGED copy of the
            // add-on already running on the cluster; this emulator runs no Kubernetes,
            // so there is never a self-managed copy to resolve against, and the member
            // is not part of what an `Addon` reports.
        };

        let addon = record.to_addon();
        guard.addons.insert(addon_key, record);

        Ok(CreateAddonOutput {
            addon: Some(addon),
        })
    }
}
