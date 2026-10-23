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
//! * `AWS::ECR::PublicRepository`'s are NOT (`RepositoryName` and
//!   `RepositoryCatalogData.RepositoryDescription` against the API's `repositoryName` and
//!   `catalogData.description`), so its arm maps the members AWS's resource reference
//!   publishes one by one, and sends nothing for a property the template leaves out;
//! * `AWS::SQS::Queue`'s properties are the queue's ATTRIBUTES, named identically on both
//!   sides, so its arm passes them through as `CreateQueue`'s `Attributes` map, lifting
//!   out the two that are not attributes (the name CloudFormation decides, and the tag
//!   list the create takes as its own member).
//!
//! Neither arm invents a value: a property the template does not declare is absent from
//! the request, and what the owning service answers is what the stack records.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::provisioning::{call_json_rpc, Refusal, AWS_JSON_1_0, AWS_JSON_1_1};
use super::template::{ECR_PUBLIC_REPOSITORY, FIREHOSE_DELIVERY_STREAM, SQS_QUEUE};
use super::StoredResource;

/// The services whose resources this emulator creates. One variant per resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Provisioner {
    FirehoseDeliveryStream,
    EcrPublicRepository,
    SqsQueue,
}

/// The provisioner that owns a template resource type, or `None` for a type this emulator
/// leaves declared-only.
pub(super) fn provisioner_for(resource_type: &str) -> Option<Provisioner> {
    match resource_type {
        FIREHOSE_DELIVERY_STREAM => Some(Provisioner::FirehoseDeliveryStream),
        ECR_PUBLIC_REPOSITORY => Some(Provisioner::EcrPublicRepository),
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
        physical_resource_id: &str,
    ) -> Result<(), String> {
        match self {
            Self::FirehoseDeliveryStream => {
                delete_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::EcrPublicRepository => {
                delete_public_repository(ctx, region, physical_resource_id).await
            }
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
) -> Result<(), String> {
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
        Err(refusal) => Err(refusal.to_string()),
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
) -> Result<(), String> {
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
        Err(refusal) => Err(refusal.to_string()),
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
) -> Result<(), String> {
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
        Err(refusal) => Err(refusal.to_string()),
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
