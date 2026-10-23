// SPDX-License-Identifier: CC0-1.0

//! One entry per resource type this emulator really creates: which service owns the type,
//! and the create and delete calls that make its resources exist and stop existing.
//!
//! This is the half of provisioning that is PER RESOURCE TYPE. The half that is not —
//! reconciling what a deployment declares against what the stack already has, deleting
//! what it replaced, and the in-process transport every one of these calls rides on —
//! is [`super::provisioning`], which owns the algorithm and reaches this module only
//! through [`provisioner_for`]. Adding a type is therefore a change to THIS file alone:
//! one arm in [`provisioner_for`] and the create/delete pair it names.
//!
//! A type with no entry here is left declared-only, which
//! [`super::provisioning`](crate::state::provisioning) documents: nothing is created in
//! any service, and the resource still carries the physical id and attributes
//! CloudFormation's documented naming rules give it.
//!
//! How a create call is BUILT depends on what the type's properties are:
//!
//! * `AWS::KinesisFirehose::DeliveryStream`'s properties ARE `CreateDeliveryStream`'s own
//!   members, so the evaluated property document is the request, passed through verbatim
//!   with the one member CloudFormation decides (the name) set on the way out;
//! * `AWS::CodePipeline::Pipeline`'s are the same document with one transformation: they
//!   are `PipelineDeclaration`'s members spelled with an upper-case initial, so
//!   [`camel_cased`] lower-cases each key's first letter and the document is the
//!   declaration;
//! * `AWS::ECR::PublicRepository`'s are NOT (`RepositoryName` and
//!   `RepositoryCatalogData.RepositoryDescription` against the API's `repositoryName` and
//!   `catalogData.description`), so its arm maps the members AWS's resource reference
//!   publishes one by one, and sends nothing for a property the template leaves out;
//! * `AWS::S3::Bucket`'s and `AWS::IAM::Role`'s reach services whose wire is not JSON at
//!   all, so those arms go through the official `aws-sdk-s3` / `aws-sdk-iam` clients over
//!   the same in-process callback ([`s3_client`], [`iam_client`]) and name the members
//!   they send one by one;
//! * `AWS::ECS::Cluster`'s are the same members under a different CASE — the Amazon ECS
//!   model spells them in lower camel case, and renames one (`ClusterSettings` sets
//!   `settings`) — so its arm names the properties `CreateCluster` takes and re-cases each
//!   value;
//! * `AWS::SQS::Queue`'s properties are the queue's ATTRIBUTES, named identically on both
//!   sides, so its arm passes them through as `CreateQueue`'s `Attributes` map, lifting
//!   out the two that are not attributes (the name CloudFormation decides, and the tag
//!   list the create takes as its own member).
//!
//! No arm invents a value: a property the template does not declare is absent from the
//! request, and what the owning service answers is what the stack records.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::provisioning::{call_json_rpc, Refusal, AWS_JSON_1_0, AWS_JSON_1_1};
use super::template::{
    ECR_PUBLIC_REPOSITORY, ECS_CLUSTER, FIREHOSE_DELIVERY_STREAM, SQS_QUEUE,
};
use super::StoredResource;

/// `AWS::S3::Bucket`: "Ref returns the bucket name", so a resource of this type is a
/// bucket of Amazon S3 addressed by its physical resource id.
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// `AWS::IAM::Role`: "Ref returns the role name".
const IAM_ROLE: &str = "AWS::IAM::Role";

/// `AWS::CodePipeline::Pipeline`: "Ref returns the pipeline name".
const CODEPIPELINE_PIPELINE: &str = "AWS::CodePipeline::Pipeline";

/// The services whose resources this emulator creates. One variant per resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Provisioner {
    FirehoseDeliveryStream,
    EcrPublicRepository,
    S3Bucket,
    IamRole,
    CodePipelinePipeline,
    EcsCluster,
    SqsQueue,
}

/// The provisioner that owns a template resource type, or `None` for a type this emulator
/// leaves declared-only.
pub(super) fn provisioner_for(resource_type: &str) -> Option<Provisioner> {
    match resource_type {
        FIREHOSE_DELIVERY_STREAM => Some(Provisioner::FirehoseDeliveryStream),
        ECR_PUBLIC_REPOSITORY => Some(Provisioner::EcrPublicRepository),
        S3_BUCKET => Some(Provisioner::S3Bucket),
        IAM_ROLE => Some(Provisioner::IamRole),
        CODEPIPELINE_PIPELINE => Some(Provisioner::CodePipelinePipeline),
        ECS_CLUSTER => Some(Provisioner::EcsCluster),
        SQS_QUEUE => Some(Provisioner::SqsQueue),
        _ => None,
    }
}

impl Provisioner {
    /// Create one resource in the service that owns it, and report back the attributes
    /// that service decided (an ARN it minted, say), which replace the ones
    /// CloudFormation's naming rules computed. `Err` is that service's own refusal.
    pub(super) async fn create(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<BTreeMap<String, String>, Refusal> {
        match self {
            Self::FirehoseDeliveryStream => create_delivery_stream(ctx, region, resource).await,
            Self::EcrPublicRepository => create_public_repository(ctx, region, resource).await,
            Self::S3Bucket => create_bucket(ctx, region, resource).await,
            Self::IamRole => create_role(ctx, region, resource).await,
            Self::CodePipelinePipeline => create_pipeline(ctx, region, resource).await,
            Self::EcsCluster => create_cluster(ctx, region, resource).await,
            Self::SqsQueue => create_queue(ctx, region, resource).await,
        }
    }

    /// Delete one resource from the service that owns it. A resource that service no
    /// longer holds is already in the state the delete asks for, so each arm treats its
    /// own not-found CODE — never a message, which carries the request's own name — as
    /// success.
    pub(super) async fn delete(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<(), Refusal> {
        let physical_resource_id = &resource.physical_resource_id;
        match self {
            Self::FirehoseDeliveryStream => {
                delete_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::EcrPublicRepository => {
                delete_public_repository(ctx, region, physical_resource_id).await
            }
            Self::S3Bucket => delete_bucket(ctx, region, physical_resource_id).await,
            // The role's policies have to go first, and only the resource's own record
            // says which ones there are.
            Self::IamRole => delete_role(ctx, region, resource).await,
            Self::CodePipelinePipeline => {
                delete_pipeline(ctx, region, physical_resource_id).await
            }
            Self::EcsCluster => delete_cluster(ctx, region, physical_resource_id).await,
            Self::SqsQueue => delete_queue(ctx, region, physical_resource_id).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Amazon Data Firehose: AWS::KinesisFirehose::DeliveryStream
// ---------------------------------------------------------------------------

/// Amazon Data Firehose's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: firehose`, service shape `Firehose_20150804`), which speaks
/// `awsJson1_1`.
const FIREHOSE_SIGNING_NAME: &str = "firehose";
const FIREHOSE_TARGET_PREFIX: &str = "Firehose_20150804";

/// The error code Firehose answers for a stream it does not have.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Create the stream an `AWS::KinesisFirehose::DeliveryStream` declares.
///
/// The type's properties are the `CreateDeliveryStream` request's own members — the
/// destination configurations, the source configuration, `DeliveryStreamType`,
/// `DeliveryStreamEncryptionConfigurationInput` and `Tags` are named identically on both
/// sides — so the resolved properties ARE the request, with the one member CloudFormation
/// decides (the stream's name, which is this resource's physical id) set to it.
async fn create_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert(
        "DeliveryStreamName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let reported = call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.CreateDeliveryStream"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("DeliveryStreamARN").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(attributes)
}

/// Delete the stream a stack created. A stream Firehose no longer has
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
async fn delete_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "DeliveryStreamName": physical_resource_id });
    match call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.DeleteDeliveryStream"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        // The stream is already in the state the delete asks for. Judged on the error
        // CODE, never on the message, which carries the request's own resource name.
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon ECR Public: AWS::ECR::PublicRepository
// ---------------------------------------------------------------------------

/// Amazon ECR Public's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: ecr-public`, service shape `SpencerFrontendService`), which speaks
/// `awsJson1_1`.
const ECR_PUBLIC_SIGNING_NAME: &str = "ecr-public";
const ECR_PUBLIC_TARGET_PREFIX: &str = "SpencerFrontendService";

/// The error code ECR Public answers for a repository it does not hold
/// (`RepositoryNotFoundException`: "The specified repository can't be found").
const REPOSITORY_NOT_FOUND: &str = "RepositoryNotFoundException";

/// What `AWS::ECR::PublicRepository`'s `RepositoryCatalogData` property declares, as
/// `CreateRepository`'s `catalogData` member spells it.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member against
/// the `RepositoryCatalogDataInput` shape. `LogoImageBlob` is base64 on both sides (the
/// property is "the base64-encoded repository logo payload", and `awsJson1_1` carries a
/// blob as base64), so it goes through as the template spells it.
const CATALOG_DATA_MEMBERS: &[(&str, &str)] = &[
    ("RepositoryDescription", "description"),
    ("AboutText", "aboutText"),
    ("UsageText", "usageText"),
    ("LogoImageBlob", "logoImageBlob"),
    ("Architectures", "architectures"),
    ("OperatingSystems", "operatingSystems"),
];

/// Create the repository an `AWS::ECR::PublicRepository` declares.
///
/// The repository's own ARN is read back off the answer: it is the resource's real
/// identity, so it replaces the one the naming rules computed.
async fn create_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    let properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let mut request = Map::new();
    // The repository's name is this resource's physical id — "Ref returns the repository
    // name" — already resolved against the stack's parameters.
    request.insert(
        "repositoryName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    if let Some(Value::Object(declared)) = properties.get("RepositoryCatalogData") {
        let mut catalog_data = Map::new();
        for (property, member) in CATALOG_DATA_MEMBERS {
            if let Some(value) = declared.get(*property) {
                catalog_data.insert((*member).to_string(), value.clone());
            }
        }
        request.insert("catalogData".to_string(), Value::Object(catalog_data));
    }
    // `Tags` is a list of `{Key, Value}` on both sides: ECR Public's own `Tag` shape
    // spells its members `Key` and `Value` too, so the list goes through as declared.
    if let Some(tags) = properties.get("Tags") {
        request.insert("tags".to_string(), tags.clone());
    }
    let reported = call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.CreateRepository"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("repository")
        .and_then(|repository| repository.get("repositoryArn"))
        .and_then(Value::as_str)
    {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(attributes)
}

/// Delete the repository a stack created. A repository ECR Public no longer holds
/// (`RepositoryNotFoundException`) is already in the state the delete asks for; one that
/// still holds images is NOT deleted (`RepositoryNotEmptyException`), and that refusal is
/// what the stack reports.
async fn delete_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "repositoryName": physical_resource_id });
    match call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.DeleteRepository"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Amazon S3: AWS::S3::Bucket
// ---------------------------------------------------------------------------

/// The error codes Amazon S3 answers when the bucket a `CreateBucket` names is one the
/// account already owns, which is the bucket this deployment wanted.
const BUCKET_ALREADY_THERE: [&str; 2] = ["BucketAlreadyOwnedByYou", "BucketAlreadyExists"];

/// The error code Amazon S3 answers for a bucket it does not hold.
const NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// Create the bucket an `AWS::S3::Bucket` declares, and apply the
/// `VersioningConfiguration` it declares for it.
///
/// "Ref returns the bucket name" for this type, so the physical resource id the stack
/// reports IS the bucket a client addresses, and it is the bucket created here.
/// `CreateBucketConfiguration$LocationConstraint` is what decides a bucket's Region and
/// "buckets in Region us-east-1 have a LocationConstraint of null", so the stack's own
/// Region is sent as that constraint for every other Region and as nothing for that one.
async fn create_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    use ::aws_sdk_s3::types::{
        BucketLocationConstraint, BucketVersioningStatus, CreateBucketConfiguration,
        VersioningConfiguration,
    };

    let client = s3_client(ctx, region);
    let bucket = &resource.physical_resource_id;
    let configuration = (region != super::DEFAULT_REGION).then(|| {
        CreateBucketConfiguration::builder()
            .location_constraint(BucketLocationConstraint::from(region))
            .build()
    });
    if let Err(error) = client
        .create_bucket()
        .bucket(bucket)
        .set_create_bucket_configuration(configuration)
        .send()
        .await
    {
        let refusal = refusal_of(&error);
        // "For legacy compatibility, if you re-create an existing bucket that you already
        // own … Amazon S3 returns 200 OK": the bucket this deployment wanted is already
        // there. Judged on the error CODE, never on the message.
        if !BUCKET_ALREADY_THERE.contains(&refusal.code.as_str()) {
            return Err(refusal);
        }
    }
    // "VersioningConfiguration — enables multiple versions of all objects in this bucket",
    // whose one member is `Status`. The value is the template's: S3's own deserializer is
    // what judges it against `BucketVersioningStatus`.
    if let Some(status) = resource
        .properties
        .get("VersioningConfiguration")
        .and_then(|configuration| configuration.get("Status"))
        .and_then(Value::as_str)
    {
        client
            .put_bucket_versioning()
            .bucket(bucket)
            .versioning_configuration(
                VersioningConfiguration::builder()
                    .status(BucketVersioningStatus::from(status))
                    .build(),
            )
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
    }
    Ok(BTreeMap::new())
}

/// Delete the bucket a stack created. A bucket S3 no longer has is already in the state
/// the delete asks for; one that still holds objects is NOT deleted, and that refusal is
/// what the stack reports.
async fn delete_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match s3_client(ctx, region)
        .delete_bucket()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = refusal_of(&error);
            if refusal.code == NO_SUCH_BUCKET {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

// ---------------------------------------------------------------------------
// IAM: AWS::IAM::Role
// ---------------------------------------------------------------------------

/// The error code IAM answers when the entity a create names already exists, which is the
/// resource this deployment wanted.
const ENTITY_ALREADY_EXISTS: &str = "EntityAlreadyExists";

/// The error code IAM answers for an entity it does not hold.
const NO_SUCH_ENTITY: &str = "NoSuchEntity";

/// Create the role an `AWS::IAM::Role` declares, with its trust policy and its inline
/// policies.
///
/// `AssumeRolePolicyDocument` is REQUIRED on this resource type and on IAM's own
/// `CreateRole`, so a declaration carrying none describes no role that could be created:
/// the resource is refused here, naming the property, rather than a role being created
/// with a trust policy this module invented or a stack completing over a role nothing
/// created.
///
/// `ManagedPolicyArns` is applied too, with `AttachRolePolicy` per entry: IAM is what
/// judges whether a policy ARN is attachable, so a policy it refuses fails the resource
/// like any other refusal rather than leaving the stack complete over a role without the
/// permissions its template declared.
async fn create_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    let Some(trust_policy) = resource
        .properties
        .get("AssumeRolePolicyDocument")
        .and_then(document_text)
    else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property AssumeRolePolicyDocument cannot be empty for resource of type {}",
                resource.resource_type
            ),
        });
    };
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    let created = client
        .create_role()
        .role_name(role_name)
        .assume_role_policy_document(trust_policy)
        // `Path`, `Description` and `MaxSessionDuration` are spelled the same on the
        // resource type and on `CreateRole`; a property the template leaves out is not
        // sent.
        .set_path(resource.properties.get("Path").and_then(scalar_text))
        .set_description(resource.properties.get("Description").and_then(scalar_text))
        .set_max_session_duration(whole_number(resource.properties.get("MaxSessionDuration")))
        .send()
        .await;
    let mut attributes = BTreeMap::new();
    match created {
        Ok(answer) => {
            // What IAM answered wins over the computed ARN: it is the role's real
            // identity.
            if let Some(arn) = answer.role().map(::aws_sdk_iam::types::Role::arn) {
                attributes.insert("Arn".to_string(), arn.to_string());
            }
        }
        Err(error) => {
            let refusal = refusal_of(&error);
            // The role is already there under this name: the resource this deployment
            // wanted.
            if refusal.code != ENTITY_ALREADY_EXISTS {
                return Err(refusal);
            }
        }
    }
    // "Policies — adds or updates an inline policy document that is embedded in the
    // specified IAM role", each entry a `{PolicyName, PolicyDocument}` pair.
    for policy in inline_policies(resource) {
        let (Some(name), Some(body)) = (
            policy.get("PolicyName").and_then(scalar_text),
            policy.get("PolicyDocument").and_then(document_text),
        ) else {
            continue;
        };
        client
            .put_role_policy()
            .role_name(role_name)
            .policy_name(name)
            .policy_document(body)
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
    }
    // "ManagedPolicyArns — a list of Amazon Resource Names (ARNs) of the IAM managed
    // policies that you want to attach to the role", each applied with the call that
    // attaches one.
    for policy_arn in managed_policy_arns(resource) {
        client
            .attach_role_policy()
            .role_name(role_name)
            .policy_arn(policy_arn)
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
    }
    Ok(attributes)
}

/// Delete the role a stack created. "You must delete the policies that are associated
/// with the role" first — an attached managed policy is DETACHED and an inline policy
/// deleted — so each one the template declared goes before the role does; a policy or a
/// role IAM no longer has is already in the state the delete asks for.
async fn delete_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    for policy_arn in managed_policy_arns(resource) {
        if let Err(error) = client
            .detach_role_policy()
            .role_name(role_name)
            .policy_arn(policy_arn)
            .send()
            .await
        {
            let refusal = refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    for policy in inline_policies(resource) {
        let Some(name) = policy.get("PolicyName").and_then(scalar_text) else {
            continue;
        };
        if let Err(error) = client
            .delete_role_policy()
            .role_name(role_name)
            .policy_name(name)
            .send()
            .await
        {
            let refusal = refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    match client.delete_role().role_name(role_name).send().await {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The `Policies` entries a role declaration carries, as a slice — empty for a role that
/// declares none, so the create and the delete walk them the same way.
fn inline_policies(resource: &StoredResource) -> &[Value] {
    declared_list(resource, "Policies")
}

/// The `ManagedPolicyArns` a role declaration carries, each as the ARN text
/// `AttachRolePolicy` takes.
fn managed_policy_arns(resource: &StoredResource) -> Vec<String> {
    declared_list(resource, "ManagedPolicyArns")
        .iter()
        .filter_map(scalar_text)
        .collect()
}

/// One list-valued property of a resource, as a slice — empty for a property the template
/// does not declare or declares as something other than a list.
fn declared_list<'a>(resource: &'a StoredResource, property: &str) -> &'a [Value] {
    resource
        .properties
        .get(property)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

// ---------------------------------------------------------------------------
// CodePipeline: AWS::CodePipeline::Pipeline
// ---------------------------------------------------------------------------

/// CodePipeline's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: codepipeline`, service shape `CodePipeline_20150709`).
const CODEPIPELINE_SIGNING_NAME: &str = "codepipeline";
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The error code CodePipeline answers when a pipeline of that name already exists, which
/// is the resource this deployment wanted.
const PIPELINE_NAME_IN_USE: &str = "PipelineNameInUseException";

/// The error code CodePipeline answers for a pipeline it does not hold.
const PIPELINE_NOT_FOUND: &str = "PipelineNotFoundException";

/// Create the pipeline an `AWS::CodePipeline::Pipeline` declares.
///
/// This type's CloudFormation properties ARE `CreatePipeline`'s `PipelineDeclaration`,
/// member for member — `RoleArn`/`roleArn`, `ArtifactStore`/`artifactStore`,
/// `Stages`/`stages`, `ActionTypeId`/`actionTypeId` and the rest
/// (`smithy_query query="shape" shape="PipelineDeclaration"`) — differing only in the case
/// of each member's initial letter, so the evaluated property document is carried through
/// with that one transformation ([`camel_cased`]) rather than mapped member by member.
///
/// Three properties are not members of the declaration and are taken out of it: `Tags` is
/// a member of the `CreatePipeline` REQUEST beside the pipeline, and
/// `DisableInboundStageTransitions` and `RestartExecutionOnUpdate` are CloudFormation's
/// own (a held transition is `DisableStageTransition`'s business, and restart-on-update is
/// a deployment instruction) — so the request carries only what the model declares.
async fn create_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut declaration = match camel_cased(&resource.properties) {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    // The one member CloudFormation itself decides.
    declaration.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = declaration.remove("tags");
    declaration.remove("disableInboundStageTransitions");
    declaration.remove("restartExecutionOnUpdate");
    // `ArtifactStores` is a LIST of `{Region, ArtifactStore}` in a template and
    // `ArtifactStoreMap`, keyed by Region, in the model: the same information either way.
    if let Some(stores) = declaration.remove("artifactStores") {
        let rewritten = match stores.as_array() {
            Some(entries) => {
                let mut by_region = Map::new();
                for entry in entries {
                    let (Some(entry_region), Some(store)) = (
                        entry.get("region").and_then(Value::as_str),
                        entry.get("artifactStore"),
                    ) else {
                        continue;
                    };
                    by_region.insert(entry_region.to_string(), store.clone());
                }
                Value::Object(by_region)
            }
            None => stores,
        };
        declaration.insert("artifactStores".to_string(), rewritten);
    }
    let mut request = Map::new();
    request.insert("pipeline".to_string(), Value::Object(declaration));
    if let Some(tags) = tags {
        request.insert("tags".to_string(), tags);
    }
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.CreatePipeline"),
        &Value::Object(request),
    )
    .await
    {
        Ok(_) => Ok(BTreeMap::new()),
        Err(refusal) if refusal.code == PIPELINE_NAME_IN_USE => Ok(BTreeMap::new()),
        Err(refusal) => Err(refusal),
    }
}

/// Delete the pipeline a stack created. One CodePipeline no longer has is already in the
/// state the delete asks for.
async fn delete_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.DeletePipeline"),
        &serde_json::json!({ "name": physical_resource_id }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == PIPELINE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Reading an evaluated property document
// ---------------------------------------------------------------------------

/// The same document with every object KEY's initial letter lower-cased, which is the one
/// difference between a CloudFormation resource type's property names and the API members
/// they are (`RoleArn` -> `roleArn`).
///
/// An `ActionDeclaration`'s `configuration` is the exception: `ActionConfigurationMap`'s
/// keys are the action PROVIDER's own, case-sensitive settings (`S3Bucket`,
/// `PollForSourceChanges`), not modelled member names, so the entries of that one map are
/// carried through verbatim.
fn camel_cased(value: &Value) -> Value {
    /// The one map whose KEYS are data rather than member names.
    const CONFIGURATION: &str = "configuration";

    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (key, member) in members {
                let name = lower_initial(key);
                let member = if name == CONFIGURATION {
                    member.clone()
                } else {
                    camel_cased(member)
                };
                converted.insert(name, member);
            }
            Value::Object(converted)
        }
        Value::Array(items) => Value::Array(items.iter().map(camel_cased).collect()),
        scalar => scalar.clone(),
    }
}

/// One name with its FIRST character lower-cased and nothing else touched, so an
/// initialism inside a member name keeps its case (`RoleARN` -> `roleARN`).
fn lower_initial(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// One JSON scalar as the text an API member takes, or `None` for a composite: the three
/// scalars a template property can carry all have one textual spelling.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// A policy-document property as the JSON text the IAM member takes.
///
/// CloudFormation accepts a policy document as an inline object (JSON or YAML) or as a
/// string, and IAM takes it as text, so an object is rendered and a string passes through.
fn document_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).ok(),
        _ => None,
    }
}

/// A property that carries a whole number, in either spelling a template gives one: JSON's
/// number, or the string a quoted YAML scalar leaves behind.
fn whole_number(value: Option<&Value>) -> Option<i32> {
    let value = value?;
    let whole = match value {
        Value::Number(number) => number.as_i64()?,
        Value::String(text) => text.parse().ok()?,
        _ => return None,
    };
    i32::try_from(whole).ok()
}

// ---------------------------------------------------------------------------
// The SDK clients an arm calls its owning service through
// ---------------------------------------------------------------------------

/// The error code this module reports for a declaration that is missing a property its
/// resource type requires, which is the code CloudFormation's own template validation
/// answers with.
const MISSING_PROPERTY_CODE: &str = "ValidationError";

/// The error code that stands in for one an answer did not carry: the call did not
/// succeed, and nothing on the other end identified the problem.
const UNIDENTIFIED_FAILURE: &str = "InternalFailure";

/// An SDK error as the [`Refusal`] the stack reports: the error code the owning service
/// NAMED and the message it worded, kept apart so the caller branches on the code.
///
/// An error carrying no code is still a refusal — the create did not succeed — reported
/// under [`UNIDENTIFIED_FAILURE`] with whatever the transport said.
fn refusal_of<E>(error: &E) -> Refusal
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    Refusal {
        code: error.code().unwrap_or(UNIDENTIFIED_FAILURE).to_string(),
        message: error
            .message()
            .map_or_else(|| error.to_string(), ToString::to_string),
    }
}

/// The endpoint URL an in-process call to another emulated service is addressed at.
///
/// The host is built from `service` and the FIXED [`super::DEFAULT_REGION`] literal, never
/// from template-derived text: the framework's callback routes such a call in process on
/// the service label, and the client carries the emulator's own credentials, so no value a
/// caller sends may reach the host position. The stack's own Region is what the call is
/// ABOUT, and is the client's signing scope; it is not part of the destination.
fn in_process_endpoint(service: &str) -> String {
    format!(
        "https://{service}.{}.amazonaws.com",
        super::DEFAULT_REGION
    )
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process and signs with the emulator's own credentials, so no network
/// client is configured here. Path-style addressing is what the emulated S3 service routes
/// on (its operations are bound under a `{Bucket}` path label).
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(in_process_endpoint("s3"))
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: nothing to retry, time out or watch for a stalled
        // stream, and each would need a sleep implementation this client is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// An IAM client over the same in-process callback, configured the same way.
fn iam_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_iam::Client {
    let config = ::aws_sdk_iam::Config::builder()
        .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
        .region(::aws_sdk_iam::config::Region::new(region.to_string()))
        .endpoint_url(in_process_endpoint("iam"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_iam::Client::from_conf(config)
}

// ---------------------------------------------------------------------------
// Amazon ECS: AWS::ECS::Cluster
// ---------------------------------------------------------------------------

/// Amazon ECS's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name: ecs`,
/// service shape `AmazonEC2ContainerServiceV20141113`), which speaks `awsJson1_1`.
const ECS_SIGNING_NAME: &str = "ecs";
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// The error code Amazon ECS answers for a cluster it does not hold
/// (`ClusterNotFoundException`: "The specified cluster wasn't found").
const CLUSTER_NOT_FOUND: &str = "ClusterNotFoundException";

/// The `AWS::ECS::Cluster` properties `CreateCluster` takes, as (property, member) pairs.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member against
/// the `CreateClusterRequest` shape. The Amazon ECS model spells its members in LOWER
/// CAMEL case, so each value's own members are re-cased by [`lower_camel_members`], and
/// one property's documented name is not its member's at all — a cluster's
/// `ClusterSettings` sets `settings` ("The setting to use when creating a cluster").
/// A property these pairs do not name is one `CreateCluster` has no member for; the
/// cluster's `ClusterName` is set from the physical id instead.
const ECS_CLUSTER_MEMBERS: &[(&str, &str)] = &[
    ("CapacityProviders", "capacityProviders"),
    ("ClusterSettings", "settings"),
    ("Configuration", "configuration"),
    (
        "DefaultCapacityProviderStrategy",
        "defaultCapacityProviderStrategy",
    ),
    ("ServiceConnectDefaults", "serviceConnectDefaults"),
    ("Tags", "tags"),
];

/// Create the cluster an `AWS::ECS::Cluster` declares, so a client reading the stack's
/// outputs and then calling Amazon ECS against that cluster (`ListTasks`,
/// `DescribeClusters`) finds it there rather than a `ClusterNotFoundException`.
///
/// `Ref` of this type returns the cluster NAME, which is this resource's physical id, so
/// the cluster is created under exactly the name the stack reports for it; the ARN it
/// reports back is the resource's real identity and replaces the computed one.
async fn create_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut request = Map::new();
    if let Value::Object(properties) = &resource.properties {
        for (property, member) in ECS_CLUSTER_MEMBERS {
            if let Some(value) = properties.get(*property) {
                request.insert((*member).to_string(), lower_camel_members(value));
            }
        }
    }
    request.insert(
        "clusterName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let reported = call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.CreateCluster"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("cluster")
        .and_then(|cluster| cluster.get("clusterArn"))
        .and_then(Value::as_str)
    {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(attributes)
}

/// Delete the cluster a stack created. A cluster Amazon ECS no longer holds
/// (`ClusterNotFoundException`) is already in the state the delete asks for; one that
/// still holds active services or tasks is NOT deleted, and that refusal is what the stack
/// reports.
async fn delete_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "cluster": physical_resource_id });
    match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DeleteCluster"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CLUSTER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// One property value with every member name inside it in LOWER CAMEL case, which is how
/// a service whose model spells its members that way names the very same members a
/// CloudFormation property names in upper camel case (`ClusterSettings: [{Name, Value}]`
/// is `settings: [{name, value}]`).
///
/// The walk recurses over a tree the template reader already bounded in depth and node
/// count (`super::template`'s `MAX_TEMPLATE_DEPTH` / `MAX_TEMPLATE_NODES`), so a
/// caller-supplied body cannot drive it deeper than that constant. It is applied only
/// where every member inside the property is a modelled name: a property carrying a MAP of
/// caller data (a container definition's `DockerLabels`) would have its keys re-cased too,
/// and no property of the type above carries one.
fn lower_camel_members(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| (lower_initial(name), lower_camel_members(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(lower_camel_members).collect()),
        scalar => scalar.clone(),
    }
}

// ---------------------------------------------------------------------------
// Amazon SQS: AWS::SQS::Queue
// ---------------------------------------------------------------------------

/// Amazon SQS's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: sqs`, service shape `AmazonSQS`), which speaks `awsJson1_0`.
const SQS_SIGNING_NAME: &str = "sqs";
const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// The error code SQS answers for a queue url no queue answers to (`QueueDoesNotExist`,
/// the modelled error of `DeleteQueue`).
const QUEUE_DOES_NOT_EXIST: &str = "QueueDoesNotExist";

/// Create the queue an `AWS::SQS::Queue` declares.
///
/// A queue's configuration is its ATTRIBUTES, so the call is `CreateQueue` with the
/// queue's name — the one thing CloudFormation decides, which is this resource's name and
/// the last segment of the queue url its `Ref` resolves to — and the attribute map
/// [`queue_attributes`] reads off the declared properties.
///
/// The url the create answers with is reported back as the resource's `QueueUrl`
/// attribute. It is the SAME url the type's published return values computed
/// (`https://sqs.<region>.amazonaws.com/<account>/<name>`), because both are built from
/// the one account id this emulator mints and the queue's own name — and SQS resolves a
/// `QueueUrl` by exactly that pair (`queue_name_from_url` in its own state module), so
/// the handle the stack publishes addresses the queue this call created however a client
/// reaches SQS.
async fn create_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut request = Map::new();
    request.insert("QueueName".to_string(), Value::String(queue_name(resource)));
    let attributes = queue_attributes(&resource.properties);
    if !attributes.is_empty() {
        request.insert("Attributes".to_string(), Value::Object(attributes));
    }
    let tags = queue_tags(&resource.properties);
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.CreateQueue"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(url) = reported.get("QueueUrl").and_then(Value::as_str) {
        attributes.insert("QueueUrl".to_string(), url.to_string());
    }
    Ok(attributes)
}

/// Delete the queue a stack created. A queue SQS no longer has (`QueueDoesNotExist`) is
/// already in the state the delete asks for.
///
/// The queue is addressed by its url, which is this resource's physical id — "Ref returns
/// the queue URL" for this type.
async fn delete_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "QueueUrl": physical_resource_id });
    match call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.DeleteQueue"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the queue a resource record describes: the `QueueName` its type's
/// published return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the queue's url.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to
/// the last segment of that url, which is the queue's name.
fn queue_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("QueueName") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Attributes` map an `AWS::SQS::Queue`'s declared properties are, as `CreateQueue`
/// takes them.
///
/// This type's properties ARE the queue's own attributes, named identically on both sides
/// (`VisibilityTimeout`, `MessageRetentionPeriod`, `DelaySeconds`, `FifoQueue`,
/// `RedrivePolicy`, …): CloudFormation's property names for the type are the attribute
/// names `CreateQueue`/`SetQueueAttributes` document. So every property is passed through
/// under its own name except the two that are not attributes — `QueueName`, which is the
/// create's own member and the one value CloudFormation decides, and `Tags`, which the
/// create takes as its own `tags` member ([`queue_tags`]).
///
/// An attribute value is a STRING on the wire, so a number or a boolean the template
/// wrote as such is rendered as the text it is, and the policy attributes — documented
/// "as a JSON object" — as the compact JSON of the object declared. A property SQS does
/// not know is not filtered out here: the owning service's own validation is what judges
/// an attribute, and its refusal leaves the stack `CREATE_FAILED` rather than a queue
/// configured differently from the template.
fn queue_attributes(properties: &Value) -> Map<String, Value> {
    let Some(properties) = properties.as_object() else {
        return Map::new();
    };
    properties
        .iter()
        .filter(|(property, _)| property.as_str() != "QueueName" && property.as_str() != "Tags")
        .map(|(property, value)| (property.clone(), Value::String(attribute_text(value))))
        .collect()
}

/// One attribute value as the wire carries it: a string stays itself, every other JSON
/// value is its own compact JSON (`30`, `true`, `{"maxReceiveCount":3}`).
fn attribute_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The `tags` map an `AWS::SQS::Queue`'s `Tags` property is.
///
/// CloudFormation's tag property is a LIST of `{Key, Value}` pairs and `CreateQueue`
/// takes a map, so each pair becomes one entry; a tag with no key is not a tag and is
/// dropped rather than sent as an empty one.
fn queue_tags(properties: &Value) -> Map<String, Value> {
    properties
        .get("Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    let key = tag.get("Key").and_then(Value::as_str)?;
                    if key.is_empty() {
                        return None;
                    }
                    let value = tag.get("Value").map(attribute_text).unwrap_or_default();
                    Some((key.to_string(), Value::String(value)))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every property name becomes the API member it is, and only the first letter moves.
    #[test]
    fn property_names_become_their_api_members() {
        for (property, expected) in [
            ("RoleArn", "roleArn"),
            ("ActionTypeId", "actionTypeId"),
            ("Name", "name"),
            ("RoleARN", "roleARN"),
            ("", ""),
        ] {
            let actual = lower_initial(property);
            assert_eq!(
                actual, expected,
                "case {property:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// An action's `Configuration` keys are the provider's own and are NOT converted,
    /// while every member name around them is.
    #[test]
    fn action_configuration_keys_are_carried_verbatim() {
        let declared = serde_json::json!({
            "Stages": [{
                "Name": "Source",
                "Actions": [{
                    "Name": "Fetch",
                    "Configuration": {"S3Bucket": "b", "PollForSourceChanges": "false"}
                }]
            }]
        });
        let converted = camel_cased(&declared);
        let action = &converted["stages"][0]["actions"][0];
        assert_eq!(
            action["name"].as_str(),
            Some("Fetch"),
            "expected the member names converted, got {converted}"
        );
        assert_eq!(
            action["configuration"]["S3Bucket"].as_str(),
            Some("b"),
            "expected the provider's own configuration keys verbatim, got {converted}"
        );
        assert!(
            action["configuration"].get("s3Bucket").is_none(),
            "a configuration key must not be converted, got {converted}"
        );
    }

    /// A policy document reaches IAM as text whichever way the template spells it.
    #[test]
    fn policy_documents_render_as_text() {
        for declared in [
            serde_json::json!({"Version": "2012-10-17"}),
            serde_json::json!("{\"Version\":\"2012-10-17\"}"),
        ] {
            let actual = document_text(&declared);
            assert_eq!(
                actual.as_deref(),
                Some(r#"{"Version":"2012-10-17"}"#),
                "case {declared:?}: expected the document as text, got {actual:?}"
            );
        }
    }
}
