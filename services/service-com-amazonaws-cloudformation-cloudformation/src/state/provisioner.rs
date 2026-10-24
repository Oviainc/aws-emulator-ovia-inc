// SPDX-License-Identifier: CC0-1.0

//! One entry per resource type this emulator really creates: which service owns the type,
//! and the create, update and delete calls that make its resources exist, change and stop
//! existing.
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
//! A deployment that re-declares a resource with a DIFFERENT configuration is applied
//! through the owning service's own update API ([`Provisioner::update`]), which is per
//! type for the same reason the create is: `AWS::SQS::Queue`'s configuration is its
//! attributes and its tags, so the change is a `SetQueueAttributes` plus a
//! `TagQueue`/`UntagQueue` ([`update_queue`]). A type with no arm there has nothing asked
//! of its service and the deployment reports the change as not applied, exactly as a type
//! with no create arm is left declared-only — never a stack that completed while the
//! caller's change went nowhere.
//!
//! How a create call is BUILT depends on what the type's properties are:
//!
//! * `AWS::KinesisFirehose::DeliveryStream`'s properties ARE `CreateDeliveryStream`'s own
//!   members, so the evaluated property document is the request, passed through verbatim
//!   with the one member CloudFormation decides (the name) set on the way out;
//! * `AWS::CloudWatch::MetricStream`'s are `PutMetricStream`'s own members the same way,
//!   and that one call is documented "Creates or updates a metric stream" — so it is also
//!   this type's UPDATE, the one arm here that can apply a changed declaration to a
//!   resource the stack already has;
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
//!   list the create takes as its own member);
//! * `AWS::SNS::Topic`'s are the topic's attributes in the same way, with the three that
//!   are not attributes lifted out (the name, the tag list and the data-protection policy,
//!   each a `CreateTopic` member of its own), the inline `Subscription` list made by
//!   `Subscribe` calls on the created topic, and `DeliveryStatusLogging` written as the
//!   per-protocol feedback attributes SNS names;
//! * `AWS::FIS::ExperimentTemplate`'s are the same members in lower camel case, but its
//!   `Targets` and `Actions` are MAPS keyed by names the template chose (as are a target's
//!   `ResourceTags` and `Parameters` and an action's `Parameters` and `Targets`), so its
//!   arm maps each modelled structure member by member and passes every caller-keyed map
//!   through as written. Its physical id is one the owning service ASSIGNS: FIS mints the
//!   experiment template's id, so that id — not a computed name — is what the stack reports
//!   for the resource;
//! * `AWS::WAFRegional::IPSet`'s two properties reach AWS WAF Classic through TWO calls,
//!   because that is how its API takes them: `CreateIPSet` takes the `Name`, each declared
//!   `IPSetDescriptors` entry is INSERTed by an `UpdateIPSet`, and both carry a change
//!   token of their own. Its identity is its owning service's too ("Ref returns the IPSet
//!   ID"), so its arm answers the `IPSetId` AWS WAF returned as the resource's physical id.
//! * `AWS::EMR::Cluster`'s are mostly `RunJobFlow`'s own members (`ReleaseLabel`,
//!   `Applications`, `BootstrapActions`, `Tags`, the roles and the scalars), so they pass
//!   through verbatim; the two that are shaped differently are mapped — the per-role
//!   capacity properties of `Instances` onto the one `InstanceGroups` or `InstanceFleets`
//!   list the API takes, and a `Configurations` entry's `ConfigurationProperties` onto the
//!   API's `Properties`. Its identity is its owning service's as well ("Ref returns the
//!   cluster ID"), so its arm answers the `j-` job-flow id Amazon EMR minted as the
//!   resource's physical id.
//!
//! No arm invents a value: a property the template does not declare is absent from the
//! request, and what the owning service answers is what the stack records. A property an
//! arm does not map onto the create's request at all is REFUSED rather than dropped, so a
//! stack never reports a resource configured differently from its declaration — the FIS
//! arm's `ExperimentReportConfiguration` is the one such property today. Inside a nested
//! structure the member tables are the type's published members, so a member not in one is
//! not a member of the type.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::provisioning::{
    call_json_rpc, call_query, call_rest_json, xml_element, xml_entry_map, Created, Refusal,
    AWS_JSON_1_0, AWS_JSON_1_1,
};
use super::template::{
    CLOUDWATCH_METRIC_STREAM, ECR_PUBLIC_REPOSITORY, ECS_CLUSTER, EMR_CLUSTER,
    FIREHOSE_DELIVERY_STREAM, FIS_EXPERIMENT_TEMPLATE, SNS_TOPIC, SQS_QUEUE, WAF_REGIONAL_IP_SET,
};
use super::StoredResource;

/// `AWS::S3::Bucket`: "Ref returns the bucket name", so a resource of this type is a
/// bucket of Amazon S3 addressed by its physical resource id.
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// `AWS::IAM::Role`: "Ref returns the role name".
const IAM_ROLE: &str = "AWS::IAM::Role";

/// `AWS::CodePipeline::Pipeline`: "Ref returns the pipeline name".
const CODEPIPELINE_PIPELINE: &str = "AWS::CodePipeline::Pipeline";

/// The code a refusal carries where the change a deployment asks for is one this emulator
/// has not written the owning API call for. It is the same code the scaffold's own 501
/// answers under (`smithy.framework#NotImplemented`), and not a code any AWS service
/// answers, so it is never mistaken for one of theirs.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The services whose resources this emulator creates. One variant per resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Provisioner {
    FirehoseDeliveryStream,
    CloudWatchMetricStream,
    EcrPublicRepository,
    S3Bucket,
    EmrCluster,
    IamRole,
    CodePipelinePipeline,
    EcsCluster,
    SqsQueue,
    SnsTopic,
    FisExperimentTemplate,
    WafRegionalIpSet,
}

/// The provisioner that owns a template resource type, or `None` for a type this emulator
/// leaves declared-only.
pub(super) fn provisioner_for(resource_type: &str) -> Option<Provisioner> {
    match resource_type {
        FIREHOSE_DELIVERY_STREAM => Some(Provisioner::FirehoseDeliveryStream),
        CLOUDWATCH_METRIC_STREAM => Some(Provisioner::CloudWatchMetricStream),
        ECR_PUBLIC_REPOSITORY => Some(Provisioner::EcrPublicRepository),
        S3_BUCKET => Some(Provisioner::S3Bucket),
        EMR_CLUSTER => Some(Provisioner::EmrCluster),
        IAM_ROLE => Some(Provisioner::IamRole),
        CODEPIPELINE_PIPELINE => Some(Provisioner::CodePipelinePipeline),
        ECS_CLUSTER => Some(Provisioner::EcsCluster),
        SQS_QUEUE => Some(Provisioner::SqsQueue),
        SNS_TOPIC => Some(Provisioner::SnsTopic),
        FIS_EXPERIMENT_TEMPLATE => Some(Provisioner::FisExperimentTemplate),
        WAF_REGIONAL_IP_SET => Some(Provisioner::WafRegionalIpSet),
        _ => None,
    }
}

impl Provisioner {
    /// Create one resource in the service that owns it, and report back what that service
    /// decided about it ([`Created`]): the attributes it reported (an ARN it minted, say),
    /// which replace the ones CloudFormation's naming rules computed, and — for a type
    /// whose identity is that service's
    /// ([`super::template::identity_is_service_assigned`]) — the physical id it assigned.
    /// `Err` is that service's own refusal.
    pub(super) async fn create(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<Created, Refusal> {
        match self {
            Self::FirehoseDeliveryStream => create_delivery_stream(ctx, region, resource).await,
            Self::CloudWatchMetricStream => put_metric_stream(ctx, region, resource).await,
            Self::EcrPublicRepository => create_public_repository(ctx, region, resource).await,
            Self::S3Bucket => create_bucket(ctx, region, resource).await,
            Self::EmrCluster => run_job_flow(ctx, region, resource).await,
            Self::IamRole => create_role(ctx, region, resource).await,
            Self::CodePipelinePipeline => create_pipeline(ctx, region, resource).await,
            Self::EcsCluster => create_cluster(ctx, region, resource).await,
            Self::SqsQueue => create_queue(ctx, region, resource).await,
            Self::SnsTopic => create_topic(ctx, region, resource).await,
            Self::FisExperimentTemplate => {
                create_experiment_template(ctx, region, resource).await
            }
            Self::WafRegionalIpSet => create_ip_set(ctx, region, resource).await,
        }
    }

    /// Apply a CHANGED configuration to a resource that already exists, through the owning
    /// service's own update API, and report what that service decided about it (the same
    /// [`Created`] an initial create answers with: the attributes it reported now).
    ///
    /// `resource` is the record of the resource as it stands — the identity its service
    /// knows it by, and the properties it was last deployed with — and `declared` is the
    /// property document this deployment declares for it. The difference between the two is
    /// what the call carries, so a property neither side changed is not re-sent.
    ///
    /// `Ok(None)` is a type this emulator has no update path for YET: nothing is asked of
    /// the owning service, and [`super::provisioning::reconcile`] reports the change as not
    /// applied rather than as a success that dropped it. `Err` is that service's own
    /// refusal of the change — including a change its API cannot make to an existing
    /// resource, which is the refusal a client would get.
    pub(super) async fn update(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
    ) -> Result<Option<Created>, Refusal> {
        match self {
            Self::SqsQueue => update_queue(ctx, region, resource, declared).await.map(Some),
            // CloudWatch's own `PutMetricStream` is documented "Creates or updates a metric
            // stream", so the call that created the stream is the call that applies a
            // changed declaration to it — under the name the stack gave it, with the
            // configuration the template now declares.
            Self::CloudWatchMetricStream => {
                put_metric_stream_configuration(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                    declared,
                )
                .await
                .map(Some)
            }
            // Every other type's in-place update is its own entry here, exactly as its
            // create and delete are: until one is written, nothing is asked of the owning
            // service and the deployment says so.
            _ => Ok(None),
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
            Self::CloudWatchMetricStream => {
                delete_metric_stream(ctx, region, physical_resource_id).await
            }
            Self::EcrPublicRepository => {
                delete_public_repository(ctx, region, physical_resource_id).await
            }
            Self::S3Bucket => delete_bucket(ctx, region, physical_resource_id).await,
            Self::EmrCluster => terminate_job_flow(ctx, region, physical_resource_id).await,
            // The role's policies have to go first, and only the resource's own record
            // says which ones there are.
            Self::IamRole => delete_role(ctx, region, resource).await,
            Self::CodePipelinePipeline => {
                delete_pipeline(ctx, region, physical_resource_id).await
            }
            Self::EcsCluster => delete_cluster(ctx, region, physical_resource_id).await,
            Self::SqsQueue => delete_queue(ctx, region, physical_resource_id).await,
            Self::SnsTopic => delete_topic(ctx, region, physical_resource_id).await,
            Self::FisExperimentTemplate => {
                delete_experiment_template(ctx, region, physical_resource_id).await
            }
            Self::WafRegionalIpSet => delete_ip_set(ctx, region, physical_resource_id).await,
        }
    }

    /// What the service that OWNS a resource reports the resource's configuration to be
    /// NOW: the document its own read answers with. This is the ACTUAL configuration
    /// [`super::drift`] compares a resource's expected template configuration against, so
    /// drift is read off the owning service rather than off CloudFormation's own record of
    /// what it once applied.
    ///
    /// `Ok(None)` is that service answering that it no longer holds the resource — judged
    /// on its own not-found CODE, never on a message, exactly as [`Self::delete`] does —
    /// which is the `DELETED` drift status. `Err(refusal)` is any other refusal: the check
    /// did not run, and the detection reports that rather than a resource as checked.
    pub(super) async fn actual_configuration(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<Option<Value>, Refusal> {
        let physical_resource_id = &resource.physical_resource_id;
        match self {
            Self::FirehoseDeliveryStream => {
                describe_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchMetricStream => {
                describe_metric_stream(ctx, region, physical_resource_id).await
            }
            Self::EcrPublicRepository => {
                describe_public_repository(ctx, region, physical_resource_id).await
            }
            Self::S3Bucket => describe_bucket(ctx, region, physical_resource_id).await,
            Self::EmrCluster => describe_job_flow(ctx, region, physical_resource_id).await,
            Self::IamRole => describe_role(ctx, region, physical_resource_id).await,
            Self::CodePipelinePipeline => {
                describe_pipeline(ctx, region, physical_resource_id).await
            }
            Self::EcsCluster => describe_cluster(ctx, region, physical_resource_id).await,
            Self::SqsQueue => describe_queue(ctx, region, physical_resource_id).await,
            Self::SnsTopic => describe_topic(ctx, region, physical_resource_id).await,
            Self::FisExperimentTemplate => {
                describe_experiment_template(ctx, region, physical_resource_id).await
            }
            Self::WafRegionalIpSet => describe_ip_set(ctx, region, physical_resource_id).await,
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
) -> Result<Created, Refusal> {
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
    Ok(Created::attributes(attributes))
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

/// The stream's configuration as Amazon Data Firehose reports it, for drift detection.
/// A stream it no longer has (`ResourceNotFoundException`) was deleted.
async fn describe_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "DeliveryStreamName": physical_resource_id });
    match call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.DescribeDeliveryStream"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "DeliveryStreamDescription"))),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon CloudWatch: AWS::CloudWatch::MetricStream
// ---------------------------------------------------------------------------

/// Amazon CloudWatch's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: monitoring`, service shape `GraniteServiceVersion20100801`), whose
/// protocol list starts with `awsJson1_0` — the wire this call is written on, as the
/// service's own suites drive it.
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// Create — or update — the metric stream an `AWS::CloudWatch::MetricStream` declares.
///
/// The type's properties ARE `PutMetricStream`'s own members: `FirehoseArn`, `RoleArn`,
/// `OutputFormat`, `IncludeFilters`, `ExcludeFilters`, `StatisticsConfigurations`,
/// `IncludeLinkedAccountsMetrics` and `Tags` are named identically on both sides (AWS's
/// resource reference for the type against `PutMetricStreamInput`), so the resolved
/// property document IS the request, with the one member CloudFormation decides — the
/// stream's name, which is this resource's physical id — set on the way out.
///
/// The same call serves an UPDATE of the stream: "Creates or updates a metric stream …
/// When you use PutMetricStream to create a new metric stream, the stream is created in
/// the running state. If you use it to update an existing stream, the state of the stream
/// is not changed" (`PutMetricStream`). That is why [`put_metric_stream_configuration`] is
/// also the type's [`Provisioner::update`]: a deployment that changes a declared filter
/// reaches CloudWatch as the configuration the template now declares, under the same name.
async fn put_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    put_metric_stream_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await
}

/// `PutMetricStream` with `properties` as the request and `name` as the one member
/// CloudFormation decides — the call behind both this type's create and its update, so the
/// two cannot send a different request for the same declaration. `properties` is the
/// configuration to put: the resource's own on a create, and the document the deployment
/// DECLARES on an update.
async fn put_metric_stream_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let mut request = match properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert("Name".to_string(), Value::String(name.to_string()));
    let reported = call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.PutMetricStream"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("Arn").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created::attributes(attributes))
}

/// Delete the metric stream a stack created: "Permanently deletes the metric stream that
/// you specify."
///
/// `DeleteMetricStream` models no not-found error at all (`InternalServiceFault`,
/// `InvalidParameterValueException` and `MissingRequiredParameterException` are its whole
/// error list), unlike `GetMetricStream`, so a stream CloudWatch no longer holds is not
/// refused and there is no not-found code to treat as success here.
async fn delete_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.DeleteMetricStream"),
        &request,
    )
    .await
    .map(|_| ())
}

/// The metric stream's configuration as CloudWatch reports it, for drift detection: the
/// `GetMetricStream` answer, whose members are the type's own property names. A stream
/// CloudWatch no longer holds (`ResourceNotFoundException`) was deleted.
async fn describe_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.GetMetricStream"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
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
) -> Result<Created, Refusal> {
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
    // The repository was created under the name CloudFormation decided.
    Ok(Created::attributes(attributes))
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

/// The repository's configuration as Amazon ECR Public reports it, for drift detection. A
/// repository it no longer holds (`RepositoryNotFoundException`, and an answer carrying no
/// repository at all) was deleted.
async fn describe_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "repositoryNames": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.DescribeRepositories"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "repositories")),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(None),
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
) -> Result<Created, Refusal> {
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
    Ok(Created::default())
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

/// The error code Amazon S3 answers for a bucket a `HeadBucket` does not find
/// (`HeadBucketOutput`'s one modelled error is `NotFound`).
const BUCKET_NOT_FOUND: &str = "NotFound";

/// The bucket's configuration as Amazon S3 reports it, for drift detection.
///
/// A bucket S3 no longer holds was deleted. For one it still holds, the configuration
/// reported is the `VersioningConfiguration` the create applies — the one property of this
/// type this emulator sets in S3 — under the property name the template spells it with, so
/// a bucket whose versioning was changed outside the stack is MODIFIED. A versioning read
/// the owning service refuses leaves the bucket reported as existing with no comparable
/// property rather than failing the whole detection.
async fn describe_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let client = s3_client(ctx, region);
    if let Err(error) = client
        .head_bucket()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        let refusal = refusal_of(&error);
        if refusal.code == BUCKET_NOT_FOUND || refusal.code == NO_SUCH_BUCKET {
            return Ok(None);
        }
        return Err(refusal);
    }
    let mut actual = Map::new();
    if let Ok(versioning) = client
        .get_bucket_versioning()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        if let Some(status) = versioning.status() {
            actual.insert(
                "VersioningConfiguration".to_string(),
                serde_json::json!({ "Status": status.as_str() }),
            );
        }
    }
    Ok(Some(Value::Object(actual)))
}

// ---------------------------------------------------------------------------
// Amazon EMR: AWS::EMR::Cluster
// ---------------------------------------------------------------------------

/// Amazon EMR's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: elasticmapreduce`, service shape `ElasticMapReduce`).
const EMR_SIGNING_NAME: &str = "elasticmapreduce";
const EMR_TARGET_PREFIX: &str = "ElasticMapReduce";

/// The error code Amazon EMR answers for a cluster id it does not hold.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The deepest nesting of `Configurations` inside `Configurations` this arm descends into.
///
/// The declared properties come from a caller-supplied template body, so the nesting is the
/// REQUESTER's choice and a recursive walk over it must be bounded by a constant rather
/// than by the input: exhausting the native stack aborts the process instead of unwinding.
/// Chosen, and deliberately shallow, because EMR's own documented nesting is one level of
/// application configuration inside another — a deeper chain is a template built to explode
/// the walk, and it is REFUSED rather than quietly truncated.
const MAX_CONFIGURATION_NESTING: usize = 8;

/// `InstanceGroupConfig$InstanceRole` for the three groups an `AWS::EMR::Cluster` declares
/// one property at a time, and which `RunJobFlow` takes as ONE list whose entries carry the
/// role. The one shape difference between the template's `Instances` and the API's.
const INSTANCE_GROUP_ROLES: &[(&str, &str)] = &[
    ("MasterInstanceGroup", "MASTER"),
    ("CoreInstanceGroup", "CORE"),
    ("TaskInstanceGroups", "TASK"),
];

/// The same fold for the FLEET form of the same three node types: the template declares
/// `MasterInstanceFleet`, `CoreInstanceFleet` and `TaskInstanceFleets` one property at a
/// time where `RunJobFlow` takes one `InstanceFleets` list whose entries carry
/// `InstanceFleetConfig$InstanceFleetType` ("The node type that the instance fleet hosts.
/// Valid values are MASTER, CORE, and TASK").
///
/// A cluster may declare groups or fleets but not both ("A valid JobFlowInstancesConfig
/// must contain either InstanceGroups or InstanceFleets. They cannot be used together"),
/// which Amazon EMR's own `RunJobFlow` refuses — so a template declaring both is carried to
/// EMR as declared and left to that refusal rather than judged here.
const INSTANCE_FLEET_ROLES: &[(&str, &str)] = &[
    ("MasterInstanceFleet", "MASTER"),
    ("CoreInstanceFleet", "CORE"),
    ("TaskInstanceFleets", "TASK"),
];

/// Run the job flow an `AWS::EMR::Cluster` declares, and report the `j-` job-flow id EMR
/// ASSIGNED it.
///
/// Unlike a stream, a repository or a bucket, this identifier is not CloudFormation's to
/// decide: "Ref returns the cluster ID", which exists only once EMR has run the flow — so
/// it is answered as the resource's own physical id ([`Created::physical_resource_id`]),
/// which [`super::provisioning::reconcile`] adopts, and the `Id`/`ClusterId` attributes are
/// re-pointed at it ([`super::template::identity_attributes`]).
///
/// Most of the type's properties ARE `RunJobFlow`'s own members (`ReleaseLabel`,
/// `Applications`, `BootstrapActions`, `Tags`, the roles and the scalars), so they are
/// passed through verbatim. Two are shaped differently and are mapped: `Instances`, whose
/// per-role capacity properties the API takes as one `InstanceGroups` or `InstanceFleets`
/// list, and `Configurations`, whose property map the template spells
/// `ConfigurationProperties` where the API spells it `Properties`. `Name` is the one member
/// CloudFormation decides when the template declares none: EMR requires a cluster name, so
/// the generated resource name is used.
async fn run_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let declared = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let mut request = Map::new();
    for (member, value) in &declared {
        match member.as_str() {
            "Instances" => {
                request.insert("Instances".to_string(), instances(value)?);
            }
            "Configurations" => {
                request.insert("Configurations".to_string(), configurations(value, 0)?);
            }
            _ => {
                request.insert(member.clone(), value.clone());
            }
        }
    }
    let name = declared
        .get("Name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| resource.physical_resource_id.clone());
    request.insert("Name".to_string(), Value::String(name));

    let reported = call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.RunJobFlow"),
        &Value::Object(request),
    )
    .await?;
    let Some(job_flow_id) = reported.get("JobFlowId").and_then(Value::as_str) else {
        // EMR answered a success carrying no job-flow id: there is no identifier to report,
        // and nothing here invents one.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon EMR reported no JobFlowId for the job flow it ran".to_string(),
        });
    };
    Ok(Created {
        // The job-flow id is the cluster's identity, not an attribute beside it: "Ref
        // returns the cluster ID".
        physical_resource_id: Some(job_flow_id.to_string()),
        attributes: BTreeMap::new(),
    })
}

/// The cluster's configuration as Amazon EMR reports it, for drift detection:
/// `DescribeCluster`'s own record, read by the `j-` job-flow id EMR minted.
///
/// A cluster id EMR does not hold is refused with its own `InvalidRequestException` — "the
/// cluster id is not valid or does not exist" — which is the resource having been deleted.
/// The record's members are EMR's own (`Name`, `ReleaseLabel`, `Applications`,
/// `Configurations`), so a declared property spelled identically on both sides takes part in
/// the comparison and one spelled differently leaves the cluster reported as existing rather
/// than as modified, exactly as [`super::drift`] documents for every arm here.
async fn describe_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ClusterId": physical_resource_id });
    match call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.DescribeCluster"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("Cluster").cloned()),
        Err(refusal) if refusal.code == INVALID_REQUEST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Terminate the job flow a stack created.
///
/// `TerminateJobFlows` "shuts a list of clusters down" and answers nothing; a cluster EMR
/// does not have is refused with its own `InvalidRequestException`, which is the state the
/// delete asks for.
async fn terminate_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "JobFlowIds": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.TerminateJobFlows"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_REQUEST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The `Instances` an `AWS::EMR::Cluster` declares, as the `JobFlowInstancesConfig`
/// `RunJobFlow` takes: every member passed through, and the per-role capacity properties
/// folded onto the one list the API takes for that form — the three GROUP properties onto
/// `InstanceGroups`, keyed by `InstanceRole`, and the three FLEET properties onto
/// `InstanceFleets`, keyed by `InstanceFleetType`. Nothing a template declares is dropped:
/// a cluster created with less capacity than its template asked for is not the cluster the
/// template described.
fn instances(declared: &Value) -> Result<Value, Refusal> {
    let Value::Object(members) = declared else {
        return Ok(declared.clone());
    };
    let mut config = Map::new();
    let mut groups: Vec<Value> = Vec::new();
    let mut fleets: Vec<Value> = Vec::new();
    for (member, value) in members {
        if let Some((_, role)) = INSTANCE_GROUP_ROLES
            .iter()
            .find(|(property, _)| property == member)
        {
            fold_per_role(member, value, "InstanceRole", role, &mut groups)?;
            continue;
        }
        if let Some((_, role)) = INSTANCE_FLEET_ROLES
            .iter()
            .find(|(property, _)| property == member)
        {
            fold_per_role(member, value, "InstanceFleetType", role, &mut fleets)?;
            continue;
        }
        config.insert(member.clone(), value.clone());
    }
    if !groups.is_empty() {
        config.insert("InstanceGroups".to_string(), Value::Array(groups));
    }
    if !fleets.is_empty() {
        config.insert("InstanceFleets".to_string(), Value::Array(fleets));
    }
    Ok(Value::Object(config))
}

/// Fold one per-role capacity property onto the list the API takes, stamping the role onto
/// each entry under `role_member`.
///
/// The task-node property is a LIST (`TaskInstanceGroups`, `TaskInstanceFleets`) where the
/// master and core ones are single structures, so both spellings are accepted. An entry
/// that is not a structure at all is REFUSED rather than dropped: it is capacity the
/// template asked for and this arm cannot carry.
fn fold_per_role(
    property: &str,
    declared: &Value,
    role_member: &str,
    role: &str,
    onto: &mut Vec<Value>,
) -> Result<(), Refusal> {
    let entries: Vec<&Value> = match declared {
        Value::Array(items) => items.iter().collect(),
        single => vec![single],
    };
    for entry in entries {
        let Value::Object(entry) = entry else {
            return Err(malformed_property(property));
        };
        let mut built = entry.clone();
        built.insert(role_member.to_string(), Value::String(role.to_string()));
        onto.push(Value::Object(built));
    }
    Ok(())
}

/// The `Configurations` an `AWS::EMR::Cluster` declares, as the `Configuration` list
/// `RunJobFlow` takes — the application configurations (`hive-site`, `spark-hive-site`, …)
/// a client reads back off the cluster.
///
/// One member is spelled differently on the two sides: the template's property map is
/// `ConfigurationProperties` where the API's is `Properties`. Nested `Configurations` are
/// mapped the same way, to the depth the template declares them; a chain of more than
/// [`MAX_CONFIGURATION_NESTING`] levels is REFUSED, because a cluster created with part of
/// the configuration its template declared is not the cluster the template described.
///
/// `depth` is how many levels are already above this one, so the outermost list is `0` and
/// the deepest list this carries is `MAX_CONFIGURATION_NESTING - 1`: exactly
/// [`MAX_CONFIGURATION_NESTING`] levels, which is what that constant names.
fn configurations(declared: &Value, depth: usize) -> Result<Value, Refusal> {
    if depth >= MAX_CONFIGURATION_NESTING {
        return Err(Refusal {
            code: "ValidationException".to_string(),
            message: format!(
                "Configurations are nested deeper than the {MAX_CONFIGURATION_NESTING} levels \
                 this emulator carries, so the cluster was not created"
            ),
        });
    }
    let Value::Array(items) = declared else {
        return Ok(declared.clone());
    };
    let mut mapped = Vec::with_capacity(items.len());
    for configuration in items {
        let Value::Object(members) = configuration else {
            return Err(malformed_property("Configurations"));
        };
        let mut built = Map::new();
        for (member, value) in members {
            match member.as_str() {
                "ConfigurationProperties" => {
                    built.insert("Properties".to_string(), value.clone());
                }
                "Configurations" => {
                    built.insert(
                        "Configurations".to_string(),
                        configurations(value, depth + 1)?,
                    );
                }
                _ => {
                    built.insert(member.clone(), value.clone());
                }
            }
        }
        mapped.push(Value::Object(built));
    }
    Ok(Value::Array(mapped))
}

/// The refusal for a declared property this arm cannot map onto the owning API's member,
/// named so a caller can tell WHICH property it was. Refusing is what keeps a resource from
/// being created with less than the template declared.
fn malformed_property(property: &str) -> Refusal {
    Refusal {
        code: "ValidationException".to_string(),
        message: format!("Property {property} is not shaped as the resource type declares it"),
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
) -> Result<Created, Refusal> {
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
    Ok(Created::attributes(attributes))
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

/// The role's configuration as IAM reports it, for drift detection.
///
/// A role IAM no longer holds (`NoSuchEntity`) was deleted. The members compared are the
/// ones the resource type and `GetRole` spell identically and the create sends — `Path`,
/// `Description` and `MaxSessionDuration` — so a role whose description or session length
/// was changed outside the stack is MODIFIED.
async fn describe_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = match iam_client(ctx, region)
        .get_role()
        .role_name(physical_resource_id)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let mut actual = Map::new();
    if let Some(role) = answer.role() {
        actual.insert("Path".to_string(), Value::String(role.path().to_string()));
        if let Some(description) = role.description() {
            actual.insert(
                "Description".to_string(),
                Value::String(description.to_string()),
            );
        }
        if let Some(duration) = role.max_session_duration() {
            actual.insert("MaxSessionDuration".to_string(), Value::from(duration));
        }
    }
    Ok(Some(Value::Object(actual)))
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
) -> Result<Created, Refusal> {
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
        Ok(_) => Ok(Created::default()),
        Err(refusal) if refusal.code == PIPELINE_NAME_IN_USE => Ok(Created::default()),
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

/// The pipeline's declaration as CodePipeline reports it, for drift detection. One it no
/// longer holds (`PipelineNotFoundException`) was deleted.
async fn describe_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.GetPipeline"),
        &serde_json::json!({ "name": physical_resource_id }),
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "pipeline"))),
        Err(refusal) if refusal.code == PIPELINE_NOT_FOUND => Ok(None),
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

/// One member of an answer document, or an empty object where the answer carried none: a
/// read that succeeded reported the resource, and a member it did not carry is no
/// configuration to compare rather than a missing resource.
fn reported_member(answer: &Value, member: &str) -> Value {
    answer
        .get(member)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// The FIRST entry of a list an answer carries, or `None` for an answer that lists none —
/// which is how a describe-many call reports a resource the service does not hold.
fn first_listed(answer: &Value, member: &str) -> Option<Value> {
    answer
        .get(member)
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .cloned()
}

/// One JSON scalar as the text an API member takes, or `None` for a composite.
///
/// The one spelling rule, held in [`super::scalar_text`] so the create side and the drift
/// comparison that reads the resource back cannot disagree about it.
fn scalar_text(value: &Value) -> Option<String> {
    super::scalar_text(value)
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
) -> Result<Created, Refusal> {
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
    // The cluster was created under the name CloudFormation decided.
    Ok(Created::attributes(attributes))
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

/// The cluster's configuration as Amazon ECS reports it, for drift detection.
///
/// A cluster Amazon ECS no longer holds (`ClusterNotFoundException`, an answer listing no
/// cluster, or one it reports `INACTIVE` — the status a deleted cluster keeps, which
/// `DeleteCluster` documents) was deleted.
async fn describe_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    /// The status Amazon ECS reports for a cluster whose deletion has gone through.
    const INACTIVE: &str = "INACTIVE";

    let request = serde_json::json!({ "clusters": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DescribeClusters"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "clusters").filter(|cluster| {
            cluster.get("status").and_then(Value::as_str) != Some(INACTIVE)
        })),
        Err(refusal) if refusal.code == CLUSTER_NOT_FOUND => Ok(None),
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
) -> Result<Created, Refusal> {
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
    // The queue was created under the name CloudFormation decided, and its url is built
    // from that name.
    Ok(Created::attributes(attributes))
}

/// Apply a changed `AWS::SQS::Queue` declaration to the queue that already exists.
///
/// A queue's configuration is its ATTRIBUTES and its TAGS, and Amazon SQS has an API for
/// changing each of them on an existing queue, so that is what this calls: one
/// `SetQueueAttributes` carrying the attributes this deployment states differently (added
/// or given another value), one `TagQueue` carrying the tags it adds or re-values, and one
/// `UntagQueue` naming the tag keys it no longer declares. A property neither revision
/// changed is not sent at all, and nothing is called where nothing changed.
///
/// The queue keeps its name, so it keeps the url its `Ref` resolves to and everything it
/// holds: the change is applied to the queue the stack created rather than replacing it.
/// The create reported the `QueueUrl` attribute and the queue still answers to it, so this
/// reports no attribute of its own.
///
/// Two changes Amazon SQS will not make to an existing queue are REFUSED rather than
/// quietly dropped, each with the refusal a client calling that API would get, so the stack
/// never reports a queue configured differently from its declaration:
///
/// * an attribute `SetQueueAttributes` does not take — `FifoQueue` is "an attribute [you
///   can] provide only during queue creation", so SQS answers `InvalidAttributeName`;
/// * a property this deployment no longer declares, which AWS resets to that attribute's
///   documented default. `SetQueueAttributes` has no way to ask for a default and the
///   defaults are Amazon SQS's own (restating them here would be a second copy of its
///   documentation), so the removal is answered with `NotImplemented` naming the property.
async fn update_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let url = resource.physical_resource_id.clone();
    let before = queue_attributes(&resource.properties);
    let after = queue_attributes(declared);

    // A property the new declaration drops: nothing is called at all, so the queue is left
    // exactly as it is and the deployment reports why.
    if let Some(dropped) = before.keys().find(|name| !after.contains_key(*name)) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not reset the {dropped} property of an existing \
                 {SQS_QUEUE} that an update no longer declares to Amazon SQS's documented \
                 default; the resource was left as it is."
            ),
        });
    }

    let changed: Map<String, Value> = after
        .into_iter()
        .filter(|(name, value)| before.get(name) != Some(value))
        .collect();
    if !changed.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.SetQueueAttributes"),
            &serde_json::json!({ "QueueUrl": url, "Attributes": Value::Object(changed) }),
        )
        .await?;
    }

    let before_tags = queue_tags(&resource.properties);
    let after_tags = queue_tags(declared);
    let added: Map<String, Value> = after_tags
        .iter()
        .filter(|(key, value)| before_tags.get(*key) != Some(value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !added.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.TagQueue"),
            &serde_json::json!({ "QueueUrl": url, "Tags": Value::Object(added) }),
        )
        .await?;
    }
    let removed: Vec<Value> = before_tags
        .keys()
        .filter(|key| !after_tags.contains_key(*key))
        .map(|key| Value::String(key.clone()))
        .collect();
    if !removed.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.UntagQueue"),
            &serde_json::json!({ "QueueUrl": url, "TagKeys": Value::Array(removed) }),
        )
        .await?;
    }

    Ok(Created::default())
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

/// The queue's configuration as Amazon SQS reports it, for drift detection.
///
/// A queue SQS no longer has (`QueueDoesNotExist`) was deleted. A queue's configuration IS
/// its attribute map, named identically to this type's properties ([`queue_attributes`]),
/// so the attributes are the actual configuration compared against the declared one and a
/// queue whose visibility timeout was changed outside the stack is MODIFIED.
async fn describe_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "QueueUrl": physical_resource_id,
        "AttributeNames": ["All"],
    });
    match call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.GetQueueAttributes"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "Attributes"))),
        Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => Ok(None),
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
/// takes a map, which is the conversion [`tag_map`] does.
fn queue_tags(properties: &Value) -> Map<String, Value> {
    properties.get("Tags").map(tag_map).unwrap_or_default()
}

/// CloudFormation's `[{Key, Value}]` tag spelling as the MAP of tag key to value a
/// creating API takes.
///
/// A tag with no key is not a tag and is dropped rather than sent as an empty one; a value
/// that is not a string is sent as the text it is, since a tag value is a string on every
/// wire that takes one.
fn tag_map(tags: &Value) -> Map<String, Value> {
    tags.as_array()
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

// ---------------------------------------------------------------------------
// Amazon SNS: AWS::SNS::Topic
// ---------------------------------------------------------------------------

/// Amazon SNS's signing name and API version, from its model (`sigv4Name: sns`, service
/// shape `AmazonSimpleNotificationService`, `version: 2010-03-31`), which speaks
/// `awsQuery` — so its calls ride on [`call_query`] rather than on [`call_json_rpc`].
const SNS_SIGNING_NAME: &str = "sns";
const SNS_API_VERSION: &str = "2010-03-31";

/// The error code SNS answers for a topic ARN no topic answers to: `NotFoundException`'s
/// own `awsQueryError` code, which is what an awsQuery client branches on.
const NOT_FOUND: &str = "NotFound";

/// The properties of an `AWS::SNS::Topic` that are NOT topic attributes, each a member of
/// `CreateTopic` in its own right or a call of its own:
///
/// * `TopicName` is the one value CloudFormation decides, and the create's `Name` member;
/// * `Tags` is the create's own tag list;
/// * `DataProtectionPolicy` is the create's own member ("The body of the policy document
///   you want to use for this topic");
/// * `Subscription` is a list of subscriptions, which are `Subscribe` calls on the topic
///   once it exists.
const NON_ATTRIBUTE_PROPERTIES: &[&str] =
    &["TopicName", "Tags", "DataProtectionPolicy", "Subscription"];

/// The property whose entries are the per-protocol delivery-status attributes.
const DELIVERY_STATUS_LOGGING: &str = "DeliveryStatusLogging";

/// How a `DeliveryStatusLogging` entry's `Protocol` is spelled in the delivery-status
/// attribute names SNS documents: a `sqs` entry's role ARN is the topic's
/// `SQSSuccessFeedbackRoleArn`, a `http/s` entry's its `HTTPSuccessFeedbackRoleArn`.
///
/// Source: the `Protocol` value set AWS's resource reference publishes for
/// `AWS::SNS::Topic`'s `LoggingConfig`, against the attribute names
/// `SetTopicAttributes` documents. A protocol outside this set is passed through as the
/// template spells it, so SNS's own validation is what refuses it rather than a name
/// invented here.
const LOGGING_PROTOCOL_PREFIXES: &[(&str, &str)] = &[
    ("http/s", "HTTP"),
    ("sqs", "SQS"),
    ("lambda", "Lambda"),
    ("firehose", "Firehose"),
    ("application", "Application"),
];

/// The members of one `LoggingConfig`, named identically on both sides once the protocol's
/// prefix is in front of them. `Protocol` itself is the prefix, not a member.
const LOGGING_CONFIG_MEMBERS: &[&str] = &[
    "SuccessFeedbackRoleArn",
    "SuccessFeedbackSampleRate",
    "FailureFeedbackRoleArn",
];

/// Create the topic an `AWS::SNS::Topic` declares, and the subscriptions it declares on it.
///
/// A topic's configuration is its ATTRIBUTES, so the call is `CreateTopic` with the topic's
/// name — the one thing CloudFormation decides, which is the last segment of the ARN its
/// `Ref` resolves to — and the attribute map [`topic_attributes`] reads off the declared
/// properties.
///
/// The ARN the create answers with is reported back as the resource's `TopicArn`
/// attribute. It is the SAME ARN the type's published return values computed
/// (`arn:aws:sns:<region>:<account>:<name>`), because both are built from the one account
/// id this emulator mints and the topic's own name — and SNS keys a topic by exactly that
/// ARN, so the handle the stack publishes addresses the topic this call created however a
/// client reaches SNS.
async fn create_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut parameters = vec![("Name".to_string(), topic_name(resource))];
    for (index, (name, value)) in topic_attributes(&resource.properties).iter().enumerate() {
        let entry = index + 1;
        parameters.push((format!("Attributes.entry.{entry}.key"), name.clone()));
        parameters.push((format!("Attributes.entry.{entry}.value"), value.clone()));
    }
    for (index, (key, value)) in topic_tags(&resource.properties).iter().enumerate() {
        let member = index + 1;
        parameters.push((format!("Tags.member.{member}.Key"), key.clone()));
        parameters.push((format!("Tags.member.{member}.Value"), value.clone()));
    }
    if let Some(policy) = resource.properties.get("DataProtectionPolicy") {
        parameters.push(("DataProtectionPolicy".to_string(), attribute_text(policy)));
    }
    let answered = call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "CreateTopic",
        &parameters,
    )
    .await?;
    let arn = xml_element(&answered, "TopicArn")
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon SNS answered CreateTopic without a TopicArn".to_string(),
        })?;

    // "The SNS subscriptions (endpoints) for this topic" — declared inline on the topic,
    // so they are subscriptions of the topic this call just created.
    for subscription in declared_subscriptions(&resource.properties) {
        let mut parameters = vec![("TopicArn".to_string(), arn.clone())];
        parameters.extend(subscription);
        let subscribed = call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "Subscribe",
            &parameters,
        )
        .await;
        if let Err(refusal) = subscribed {
            // The resource the template declares is the topic AND the subscriptions on
            // it, so a subscription SNS refuses fails the resource. The stack records no
            // physical id for a resource whose create failed, so nothing would ever
            // delete the topic this call already made: it is deleted here, before the
            // refusal propagates, and what the stack reports is SNS's own refusal.
            delete_topic(ctx, region, &arn)
                .await
                .map_err(|failure| Refusal {
                    code: refusal.code.clone(),
                    message: format!(
                        "{refusal}; the topic it created was left behind: {failure}"
                    ),
                })?;
            return Err(refusal);
        }
    }

    let mut attributes = BTreeMap::new();
    attributes.insert("TopicArn".to_string(), arn);
    Ok(Created::attributes(attributes))
}

/// Delete the topic a stack created. A topic SNS no longer has (`NotFound`) is already in
/// the state the delete asks for.
///
/// The topic is addressed by its ARN, which is this resource's physical id — "Ref returns
/// the topic ARN" for this type.
async fn delete_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("TopicArn".to_string(), physical_resource_id.to_string())];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "DeleteTopic",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The topic's configuration as Amazon SNS reports it, for drift detection.
///
/// A topic SNS no longer has (`NotFound`) was deleted. A topic's configuration IS its
/// attribute map, named identically to this type's properties ([`topic_attributes`]), so
/// the attributes are the actual configuration compared against the declared one and a
/// topic whose display name was changed outside the stack is MODIFIED. The map the query
/// protocol serializes as `<entry><key/><value/></entry>` is read back as the JSON object
/// the comparison takes.
async fn describe_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("TopicArn".to_string(), physical_resource_id.to_string())];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "GetTopicAttributes",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_entry_map(&answered, "Attributes")))),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the topic a resource record describes: the `TopicName` its type's published
/// return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the topic's ARN.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// last segment of that ARN, which is the topic's name.
fn topic_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("TopicName") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Attributes` map an `AWS::SNS::Topic`'s declared properties are, as `CreateTopic`
/// takes them.
///
/// This type's properties ARE the topic's own attributes, named identically on both sides
/// (`DisplayName`, `FifoTopic`, `KmsMasterKeyId`, `SignatureVersion`, `TracingConfig`,
/// `ArchivePolicy`, `ContentBasedDeduplication`, `FifoThroughputScope`): CloudFormation's
/// property names for the type are the attribute names
/// `CreateTopic`/`SetTopicAttributes` document. So every property is passed through under
/// its own name except the ones that are not attributes ([`NON_ATTRIBUTE_PROPERTIES`]) and
/// `DeliveryStatusLogging`, whose entries are the per-protocol attributes
/// [`delivery_status_attributes`] names.
///
/// An attribute value is a STRING on the wire, so a number or a boolean the template wrote
/// as such is rendered as the text it is, and the policy attributes — documented as JSON
/// objects — as the compact JSON of the object declared. A property SNS does not know is
/// not filtered out here: the owning service's own validation is what judges an attribute,
/// and its refusal leaves the stack `CREATE_FAILED` rather than a topic configured
/// differently from the template.
fn topic_attributes(properties: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let Some(properties) = properties.as_object() else {
        return attributes;
    };
    for (property, value) in properties {
        if NON_ATTRIBUTE_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        if property == DELIVERY_STATUS_LOGGING {
            attributes.extend(delivery_status_attributes(value));
            continue;
        }
        attributes.insert(property.clone(), attribute_text(value));
    }
    attributes
}

/// The topic attributes a `DeliveryStatusLogging` list declares: one entry per member each
/// of its `LoggingConfig`s carries, under that config's protocol prefix
/// ([`LOGGING_PROTOCOL_PREFIXES`]).
///
/// A member the config does not declare is absent. `Protocol` is required on a
/// `LoggingConfig`, so an entry carrying none is a template error: its members go through
/// under their BARE names, which are not topic attribute names, so SNS refuses them — the
/// same way an unknown protocol spelling goes through as the template wrote it. Neither is
/// dropped, which would configure the topic differently from what the template declared
/// while the stack still completed.
fn delivery_status_attributes(declared: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    for config in declared.as_array().map(Vec::as_slice).unwrap_or_default() {
        let prefix = config
            .get("Protocol")
            .and_then(Value::as_str)
            .map_or("", |protocol| {
                LOGGING_PROTOCOL_PREFIXES
                    .iter()
                    .find(|(declared, _)| declared.eq_ignore_ascii_case(protocol))
                    .map_or(protocol, |(_, prefix)| prefix)
            });
        for member in LOGGING_CONFIG_MEMBERS {
            if let Some(value) = config.get(*member) {
                attributes.insert(format!("{prefix}{member}"), attribute_text(value));
            }
        }
    }
    attributes
}

/// The `Tags` list an `AWS::SNS::Topic`'s `Tags` property is, as `CreateTopic` takes it:
/// the same `{Key, Value}` pairs, since SNS's own `Tag` shape spells its members `Key` and
/// `Value` too. A tag with no key is not a tag and is dropped rather than sent as an empty
/// one.
fn topic_tags(properties: &Value) -> Vec<(String, String)> {
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
                    Some((
                        key.to_string(),
                        tag.get("Value").map(attribute_text).unwrap_or_default(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `Subscribe` parameters each entry of a `Subscription` property declares: its
/// `Protocol` and its `Endpoint`, named identically on both sides. A member the entry does
/// not declare is absent from the call, so SNS's own validation is what refuses an
/// incomplete subscription.
fn declared_subscriptions(properties: &Value) -> Vec<Vec<(String, String)>> {
    properties
        .get("Subscription")
        .and_then(Value::as_array)
        .map(|declared| {
            declared
                .iter()
                .map(|subscription| {
                    ["Protocol", "Endpoint"]
                        .iter()
                        .filter_map(|member| {
                            let value = subscription.get(*member)?;
                            Some(((*member).to_string(), attribute_text(value)))
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// AWS Fault Injection Service: AWS::FIS::ExperimentTemplate
// ---------------------------------------------------------------------------

/// AWS FIS's signing name, from its model (`sigv4Name: fis`). FIS speaks `restJson1`, so
/// its operations are bound to a METHOD and a URI rather than to an `X-Amz-Target` header:
/// `CreateExperimentTemplate` is `POST /experimentTemplates` and
/// `DeleteExperimentTemplate` is `DELETE /experimentTemplates/{id}`
/// (`smithy_query(query="operation", operation="CreateExperimentTemplate,\
/// DeleteExperimentTemplate")` on that service's model).
const FIS_SIGNING_NAME: &str = "fis";

/// The URI `CreateExperimentTemplate` is bound to.
const FIS_EXPERIMENT_TEMPLATES_URI: &str = "/experimentTemplates";

/// The error code FIS answers for an experiment template it does not hold
/// (`ResourceNotFoundException`, a modelled error of `DeleteExperimentTemplate`).
const FIS_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `AWS::FIS::ExperimentTemplate` properties `CreateExperimentTemplate` takes whose
/// value goes through unchanged under its lower-camel member name, as (property, member)
/// pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateExperimentTemplateRequest` shape. The FIS model spells its members in lower
/// camel case and renames nothing, so the only difference is the first character's case;
/// `StopConditions`, `Targets`, `Actions`, `LogConfiguration` and `ExperimentOptions` are
/// NOT here because their own members have to be re-cased one structure at a time (a map
/// of targets or of actions is keyed by names the TEMPLATE chose, which must not be
/// re-cased), and `Tags` is not because the type spells it as a map of tag keys the
/// template chose.
const FIS_SCALAR_MEMBERS: &[(&str, &str)] =
    &[("Description", "description"), ("RoleArn", "roleArn")];

/// The members of one `ExperimentTemplateStopCondition` the type declares.
const FIS_STOP_CONDITION_MEMBERS: &[(&str, &str)] = &[("Source", "source"), ("Value", "value")];

/// The members of one `ExperimentTemplateTarget` the type declares. `ResourceTags` and
/// `Parameters` are maps keyed by DATA (tag keys, and the target parameter names the
/// resource type publishes), so their values go through as the template wrote them.
const FIS_TARGET_MEMBERS: &[(&str, &str)] = &[
    ("ResourceType", "resourceType"),
    ("SelectionMode", "selectionMode"),
    ("ResourceArns", "resourceArns"),
    ("ResourceTags", "resourceTags"),
    ("Parameters", "parameters"),
];

/// The members of one `ExperimentTemplateTargetFilter`, which a target's `Filters` list
/// carries.
const FIS_FILTER_MEMBERS: &[(&str, &str)] = &[("Path", "path"), ("Values", "values")];

/// The members of one `ExperimentTemplateAction` the type declares. `Parameters` is keyed
/// by the action's published parameter names and `Targets` by the action's published target
/// names, so both go through as the template wrote them.
const FIS_ACTION_MEMBERS: &[(&str, &str)] = &[
    ("ActionId", "actionId"),
    ("Description", "description"),
    ("Parameters", "parameters"),
    ("Targets", "targets"),
    ("StartAfter", "startAfter"),
];

/// The members of the type's `LogConfiguration` property, and of the two configurations it
/// nests.
const FIS_LOG_CONFIGURATION_MEMBERS: &[(&str, &str)] = &[("LogSchemaVersion", "logSchemaVersion")];
const FIS_CLOUDWATCH_LOGS_MEMBERS: &[(&str, &str)] = &[("LogGroupArn", "logGroupArn")];
const FIS_S3_LOG_MEMBERS: &[(&str, &str)] = &[("BucketName", "bucketName"), ("Prefix", "prefix")];

/// The members of the type's `ExperimentOptions` property.
const FIS_EXPERIMENT_OPTIONS_MEMBERS: &[(&str, &str)] = &[
    ("AccountTargeting", "accountTargeting"),
    ("EmptyTargetResolutionMode", "emptyTargetResolutionMode"),
];

/// The one property of the type this provisioner does not map onto the create's request.
const FIS_REPORT_CONFIGURATION: &str = "ExperimentReportConfiguration";

/// Create the experiment template an `AWS::FIS::ExperimentTemplate` declares, so a client
/// that reads the id out of the stack's outputs can call FIS with it
/// (`GetExperimentTemplate`, `StartExperiment`) rather than being handed an id FIS never
/// minted.
///
/// FIS ASSIGNS the id — `Ref` of this type returns it, and the type has no name property —
/// so the id this call answers with becomes the resource's physical id, replacing the
/// stand-in the naming rules computed, and is reported back as the `Id` attribute
/// `Fn::GetAtt` reads.
///
/// `clientToken` is the create's `@idempotencyToken`, required by the model: it is the
/// computed physical id, which is a function of the stack and the resource's logical id, so
/// the same declaration of the same stack asks for the same template rather than a second
/// one.
async fn create_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if properties.contains_key(FIS_REPORT_CONFIGURATION) {
        // The property is not mapped onto `CreateExperimentTemplate`'s own
        // `experimentReportConfiguration`, so creating the template would create one
        // configured differently from the declaration. Refused rather than silently
        // dropped; mapping it is its own fix.
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not implement the {FIS_REPORT_CONFIGURATION} property of \
                 {}",
                resource.resource_type
            ),
        });
    }
    let mut request = Map::new();
    request.insert(
        "clientToken".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    copy_members(&properties, FIS_SCALAR_MEMBERS, &mut request);
    if let Some(Value::Array(conditions)) = properties.get("StopConditions") {
        let conditions: Vec<Value> = conditions
            .iter()
            .map(|condition| mapped_members(condition, FIS_STOP_CONDITION_MEMBERS))
            .collect();
        request.insert("stopConditions".to_string(), Value::Array(conditions));
    }
    if let Some(Value::Object(targets)) = properties.get("Targets") {
        let targets: Map<String, Value> = targets
            .iter()
            .map(|(name, target)| (name.clone(), experiment_target(target)))
            .collect();
        request.insert("targets".to_string(), Value::Object(targets));
    }
    if let Some(Value::Object(actions)) = properties.get("Actions") {
        let actions: Map<String, Value> = actions
            .iter()
            .map(|(name, action)| (name.clone(), mapped_members(action, FIS_ACTION_MEMBERS)))
            .collect();
        request.insert("actions".to_string(), Value::Object(actions));
    }
    if let Some(configuration) = properties.get("LogConfiguration") {
        request.insert(
            "logConfiguration".to_string(),
            log_configuration(configuration),
        );
    }
    if let Some(options) = properties.get("ExperimentOptions") {
        request.insert(
            "experimentOptions".to_string(),
            mapped_members(options, FIS_EXPERIMENT_OPTIONS_MEMBERS),
        );
    }
    if let Some(tags) = experiment_template_tags(&properties) {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "POST",
        FIS_EXPERIMENT_TEMPLATES_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(id) = reported
        .get("experimentTemplate")
        .and_then(|template| template.get("id"))
        .and_then(Value::as_str)
    else {
        // FIS answered a success carrying no id, so there is nothing a client could call
        // it with: reported as a refusal rather than recorded as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateExperimentTemplate answered no experiment template id".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.to_string());
    Ok(Created {
        physical_resource_id: Some(id.to_string()),
        attributes,
    })
}

/// Delete the experiment template a stack created, by the id FIS minted for it — this
/// resource's physical id. A template FIS no longer holds
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
async fn delete_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "DELETE",
        &format!("{FIS_EXPERIMENT_TEMPLATES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == FIS_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The experiment template's configuration as FIS reports it, for drift detection:
/// `GetExperimentTemplate`'s own record, read by the id FIS minted.
///
/// A template FIS no longer holds (`ResourceNotFoundException`) was deleted. The record's
/// members are FIS's own (lower camel), so only a declared property spelled identically on
/// both sides takes part in the comparison — the position the Amazon ECS arm above is in
/// too, and what [`super::drift`] documents: a name mismatch reports the resource as
/// existing rather than as modified.
async fn describe_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "GET",
        &format!("{FIS_EXPERIMENT_TEMPLATES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("experimentTemplate").cloned()),
        Err(refusal) if refusal.code == FIS_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One `ExperimentTemplateTarget` as `CreateExperimentTemplate` takes it: its own members
/// re-cased, and its `Filters` list — whose entries are modelled structures of their own —
/// mapped entry by entry.
fn experiment_target(target: &Value) -> Value {
    let mut mapped = mapped_members(target, FIS_TARGET_MEMBERS);
    if let Some(Value::Array(filters)) = target.get("Filters") {
        let filters: Vec<Value> = filters
            .iter()
            .map(|filter| mapped_members(filter, FIS_FILTER_MEMBERS))
            .collect();
        if let Value::Object(members) = &mut mapped {
            members.insert("filters".to_string(), Value::Array(filters));
        }
    }
    mapped
}

/// The type's `LogConfiguration` property as `CreateExperimentTemplate` takes it: its own
/// member plus the two nested configurations, each mapped member by member.
fn log_configuration(configuration: &Value) -> Value {
    let mut mapped = mapped_members(configuration, FIS_LOG_CONFIGURATION_MEMBERS);
    let Value::Object(members) = &mut mapped else {
        return mapped;
    };
    if let Some(logs) = configuration.get("CloudWatchLogsConfiguration") {
        members.insert(
            "cloudWatchLogsConfiguration".to_string(),
            mapped_members(logs, FIS_CLOUDWATCH_LOGS_MEMBERS),
        );
    }
    if let Some(s3) = configuration.get("S3Configuration") {
        members.insert(
            "s3Configuration".to_string(),
            mapped_members(s3, FIS_S3_LOG_MEMBERS),
        );
    }
    mapped
}

/// The `tags` map an `AWS::FIS::ExperimentTemplate`'s `Tags` property is.
///
/// The type spells its tags as a MAP of tag key to value, which is the shape
/// `CreateExperimentTemplate`'s own `tags` member takes, so such a declaration goes
/// through as written. A declaration that wrote CloudFormation's other tag spelling — the
/// `[{Key, Value}]` list most resource types use — is converted by [`tag_map`], so the tags
/// the template asked for reach FIS either way. `None` where the template declares none.
fn experiment_template_tags(properties: &Map<String, Value>) -> Option<Map<String, Value>> {
    match properties.get("Tags")? {
        Value::Object(tags) => Some(tags.clone()),
        list @ Value::Array(_) => Some(tag_map(list)),
        _ => None,
    }
}

/// One declared structure with the members `pairs` names mapped onto the member names the
/// owning service's model spells, and every value passed through as the template wrote it.
///
/// A property `pairs` does not name is one the create has no member for, so it is not
/// sent: the request carries what the declaration said and nothing invented.
fn mapped_members(declared: &Value, pairs: &[(&str, &str)]) -> Value {
    let mut mapped = Map::new();
    if let Some(members) = declared.as_object() {
        copy_members(members, pairs, &mut mapped);
    }
    Value::Object(mapped)
}

/// Copy the members `pairs` names out of `declared` into `request`, under the member name
/// each pair gives.
fn copy_members(
    declared: &Map<String, Value>,
    pairs: &[(&str, &str)],
    request: &mut Map<String, Value>,
) {
    for (property, member) in pairs {
        if let Some(value) = declared.get(*property) {
            request.insert((*member).to_string(), value.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// AWS WAF Classic, regional: AWS::WAFRegional::IPSet
// ---------------------------------------------------------------------------

/// AWS WAF Classic Regional's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: waf-regional`, service shape `AWSWAF_Regional_20161128`), which speaks
/// `awsJson1_1`.
const WAF_REGIONAL_SIGNING_NAME: &str = "waf-regional";
const WAF_REGIONAL_TARGET_PREFIX: &str = "AWSWAF_Regional_20161128";

/// The error code AWS WAF Classic answers for an object it does not hold
/// (`WAFNonexistentItemException`: "the referenced object doesn't exist"), and the one
/// `UpdateIPSet` answers when the IPSet the updates are for is not there at all
/// (`WAFNonexistentContainerException`: "you tried to add an IP address to or delete an IP
/// address from an IPSet that doesn't exist").
const WAF_NONEXISTENT_ITEM: &str = "WAFNonexistentItemException";
const WAF_NONEXISTENT_CONTAINER: &str = "WAFNonexistentContainerException";

/// The two `ChangeAction` values an `UpdateIPSet` update carries: "whether to insert or
/// delete the IP address".
const WAF_INSERT: &str = "INSERT";
const WAF_DELETE: &str = "DELETE";

/// Create the IPSet an `AWS::WAFRegional::IPSet` declares, and answer the `IPSetId` AWS WAF
/// Classic minted as the resource's physical id — "Ref returns the IPSet ID", which is a
/// value only that service produces.
///
/// The type's properties are `Name` and `IPSetDescriptors`, and AWS WAF Classic takes them
/// through the two calls its own documentation sequences: "Submit a CreateIPSet request …
/// Submit an UpdateIPSet request to specify the IP addresses that you want AWS WAF to watch
/// for". So the create carries the name, and each declared descriptor is INSERTed by one
/// update. Every mutating call carries its own change token ("Use GetChangeToken to get the
/// change token that you provide in the ChangeToken parameter"), so a resource whose
/// descriptors the template declares is created with them rather than with the name alone.
async fn create_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let request = serde_json::json!({
        "Name": ip_set_name(resource),
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
    });
    let reported = call_waf_regional(ctx, region, "CreateIPSet", &request).await?;
    let Some(ip_set_id) = reported
        .pointer("/IPSet/IPSetId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        // The create answered success and identified nothing, so there is no id to address
        // the IPSet by: not something this stack can report as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateIPSet answered no IPSet.IPSetId".to_string(),
        });
    };
    let descriptors = declared_ip_set_descriptors(&resource.properties);
    if !descriptors.is_empty() {
        let updates: Vec<Value> = descriptors
            .into_iter()
            .map(|descriptor| {
                serde_json::json!({ "Action": WAF_INSERT, "IPSetDescriptor": descriptor })
            })
            .collect();
        if let Err(refusal) = update_ip_set(ctx, region, &ip_set_id, updates).await {
            // The resource the template declares is the IPSet WITH its addresses, and this
            // create could not make it. The empty IPSet the first call left is deleted
            // again, so a create the stack reports as failed leaves nothing behind in the
            // owning service; the refusal reported is the one that failed.
            let _ = delete_ip_set(ctx, region, &ip_set_id).await;
            return Err(refusal);
        }
    }
    Ok(Created {
        physical_resource_id: Some(ip_set_id),
        attributes: BTreeMap::new(),
    })
}

/// Delete the IPSet a stack created.
///
/// "You can't delete an IPSet if it still includes any IP addresses", so the addresses go
/// first, which is the sequence AWS WAF Classic documents for permanently deleting one:
/// "Update the IPSet to remove IP address ranges, if any. Submit a DeleteIPSet request".
/// The IPSet is read back, every descriptor it still holds is DELETEd by one update, and
/// the IPSet itself is then deleted.
///
/// An IPSet AWS WAF Classic no longer holds is already in the state the delete asks for,
/// judged on its own not-found CODES (`WAFNonexistentItemException` from the read and the
/// delete, `WAFNonexistentContainerException` from the update that empties it) and never on
/// a message, which carries the request's own id. One a Rule still references is NOT
/// deleted (`WAFReferencedItemException`), and that refusal is what the stack reports.
async fn delete_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let Some(ip_set) = describe_ip_set(ctx, region, physical_resource_id).await? else {
        return Ok(());
    };
    let descriptors = ip_set
        .get("IPSetDescriptors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !descriptors.is_empty() {
        let updates: Vec<Value> = descriptors
            .into_iter()
            .map(|descriptor| {
                serde_json::json!({ "Action": WAF_DELETE, "IPSetDescriptor": descriptor })
            })
            .collect();
        match update_ip_set(ctx, region, physical_resource_id, updates).await {
            Ok(()) => {}
            Err(refusal) if refusal.code == WAF_NONEXISTENT_CONTAINER => return Ok(()),
            Err(refusal) => return Err(refusal),
        }
    }
    let request = serde_json::json!({
        "IPSetId": physical_resource_id,
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
    });
    match call_waf_regional(ctx, region, "DeleteIPSet", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == WAF_NONEXISTENT_ITEM => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The IPSet as AWS WAF Classic reports it, for drift detection and for the delete that has
/// to empty it first. An IPSet it no longer holds (`WAFNonexistentItemException`) was
/// deleted.
///
/// The `IPSet` the read answers carries `Name` and `IPSetDescriptors` under exactly the
/// names the type's properties have, so it is the actual configuration the declared one is
/// compared against.
async fn describe_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "IPSetId": physical_resource_id });
    match call_waf_regional(ctx, region, "GetIPSet", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "IPSet"))),
        Err(refusal) if refusal.code == WAF_NONEXISTENT_ITEM => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Apply one batch of `Updates` to an IPSet, under a change token of its own.
async fn update_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ip_set_id: &str,
    updates: Vec<Value>,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "IPSetId": ip_set_id,
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
        "Updates": updates,
    });
    call_waf_regional(ctx, region, "UpdateIPSet", &request)
        .await
        .map(|_| ())
}

/// The `IPSetDescriptors` an `AWS::WAFRegional::IPSet` declares, as `UpdateIPSet` takes
/// them.
///
/// An `IPSetDescriptor` is `{Type, Value}` on both sides — the property's members are the
/// API shape's own — so each entry goes through as the template wrote it. A declaration
/// AWS WAF Classic will not take is not filtered out here: that service's own validation is
/// what judges a descriptor, and its refusal leaves the stack `CREATE_FAILED` rather than
/// an IPSet configured differently from the template.
fn declared_ip_set_descriptors(properties: &Value) -> Vec<Value> {
    properties
        .get("IPSetDescriptors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The NAME of the IPSet a resource record declares: the `Name` its type's property
/// carries, which is the name the template asked for or the `<stack>-<logical id>-<suffix>`
/// name CloudFormation generated for it — never the physical id, which for this type is the
/// id AWS WAF Classic minted once the create went through.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// physical id, which before the create IS that computed name.
fn ip_set_name(resource: &StoredResource) -> String {
    resource
        .attributes
        .get("Name")
        .cloned()
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// A change token for one mutating AWS WAF Classic call. "Each create, update, or delete
/// request must use a unique change token", so one is fetched per call rather than reused.
async fn waf_regional_change_token(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<String, Refusal> {
    let answer = call_waf_regional(ctx, region, "GetChangeToken", &serde_json::json!({})).await?;
    answer
        .get("ChangeToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "GetChangeToken answered no ChangeToken".to_string(),
        })
}

/// Call one AWS WAF Classic Regional operation on the awsJson1_1 wire its model declares.
async fn call_waf_regional(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        WAF_REGIONAL_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{WAF_REGIONAL_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
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
