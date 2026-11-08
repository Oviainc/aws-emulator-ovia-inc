// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of AWS Config: the CONFIGURATION RECORDER a stack
//! declares, the DELIVERY CHANNEL it delivers through, and the CONFIG RULE that evaluates
//! the recorded resources. One file of [`super`]'s per-type arms, dispatched to by its
//! `Provisioner`.
//!
//! Each of the three types is deployed through the one Config call documented to create or
//! update such a resource — `PutConfigurationRecorder`, `PutDeliveryChannel` and
//! `PutConfigRule` — so the same call serves this emulator's create and its in-place
//! update, and the resource a stack reports created is one Config's own read surface
//! answers for.
//!
//! The two sides spell the same members differently, and each type is handled as its own
//! API spells it:
//!
//! * `AWS::Config::ConfigurationRecorder`'s and `AWS::Config::DeliveryChannel`'s
//!   properties are the members of `ConfigurationRecorder` and `DeliveryChannel` under a
//!   LOWER-CASE initial (`RoleARN`/`roleARN`, `S3BucketName`/`s3BucketName`,
//!   `RecordingGroup.AllSupported`/`recordingGroup.allSupported`), all the way down, so
//!   each arm carries the evaluated property document through the same re-casing walk
//!   ([`lower_camel_members`]) the Amazon GuardDuty arms need, with the one member
//!   CloudFormation decides — the resource's own name, which is its physical id — set on
//!   the way out;
//! * `AWS::Config::ConfigRule`'s properties ARE `ConfigRule`'s own members, spelled
//!   identically on both sides (`ConfigRuleName`, `Description`, `Scope`, `Source`,
//!   `InputParameters`, `MaximumExecutionFrequency`, `EvaluationModes`), so the evaluated
//!   document is the request. Two properties are not members of the create at all: the
//!   rule's `Compliance`, which is the verdict Config's own evaluation places the rule at
//!   and no input of `PutConfigRule`, and `InputParameters`, which a template declares as
//!   a JSON DOCUMENT where `ConfigRule$InputParameters` takes "a string, in JSON format".
//!
//! Every identity here is CloudFormation's: each type's `Ref` returns the resource's own
//! name, which is the physical id the naming rules compute from the declaration's name
//! property, so no type of this group is in
//! [`super::template::identity_is_service_assigned`]. A Config rule's published
//! attributes are Config's own, though — the ARN and the rule id that service mints — so
//! the rule is read back after it is put and the stack publishes what Config answered.

use super::*;

/// AWS Config's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name:
/// config`, service shape `StarlingDoveService`), whose protocol list is `awsJson1_1` —
/// the wire every call below is written on.
pub(super) const CONFIG_SIGNING_NAME: &str = "config";
/// See [`CONFIG_SIGNING_NAME`].
pub(super) const CONFIG_TARGET_PREFIX: &str = "StarlingDoveService";

/// `AWS::Config::ConfigRule`: "Ref returns the rule name", so a resource of this type is a
/// Config rule addressed by its physical resource id.
pub(super) const CONFIG_CONFIG_RULE: &str = "AWS::Config::ConfigRule";

/// `AWS::Config::ConfigurationRecorder`: "Ref returns the configuration recorder name" —
/// the handle every Config call on one takes
/// (`StartConfigurationRecorder$ConfigurationRecorderName`).
pub(super) const CONFIG_CONFIGURATION_RECORDER: &str = "AWS::Config::ConfigurationRecorder";

/// `AWS::Config::DeliveryChannel`: "Ref returns the delivery channel name" — the handle
/// every Config call on one takes (`DeleteDeliveryChannel$DeliveryChannelName`).
pub(super) const CONFIG_DELIVERY_CHANNEL: &str = "AWS::Config::DeliveryChannel";

/// The code Config answers for a rule it does not hold (`NoSuchConfigRuleException`, the
/// 400 `DescribeConfigRules` and `DeleteConfigRule` model).
pub(super) const CONFIG_NO_SUCH_RULE: &str = "NoSuchConfigRuleException";

/// The code Config answers for a configuration recorder it does not hold
/// (`NoSuchConfigurationRecorderException`).
pub(super) const CONFIG_NO_SUCH_RECORDER: &str = "NoSuchConfigurationRecorderException";

/// The code Config answers for a delivery channel it does not hold
/// (`NoSuchDeliveryChannelException`).
pub(super) const CONFIG_NO_SUCH_CHANNEL: &str = "NoSuchDeliveryChannelException";

/// The property carrying the rule's own name, which is this resource's physical id, and
/// the member `PutConfigRule` takes it under.
const CONFIG_RULE_NAME_PROPERTY: &str = "ConfigRuleName";

/// The property carrying the parameters a rule is evaluated with. A template declares it
/// as a JSON document, while `ConfigRule$InputParameters` is "a string, in JSON format",
/// so what reaches Config is that document's TEXT.
const CONFIG_RULE_INPUT_PARAMETERS_PROPERTY: &str = "InputParameters";

/// The property carrying the verdict Config's own evaluation places the rule at. It is no
/// member of `PutConfigRule`: a rule's compliance is what Config computes from the
/// recorded resources, so a declaration that states it is deployed without it rather than
/// having a verdict sent for a rule nothing has evaluated yet.
const CONFIG_RULE_COMPLIANCE_PROPERTY: &str = "Compliance";

/// Every property `AWS::Config::ConfigRule` publishes, which is what this arm carries to
/// Config. A declaration naming anything else is refused rather than deployed with the
/// property dropped, exactly as the other arms of this catalogue refuse one.
const CONFIG_RULE_PROPERTIES: &[&str] = &[
    CONFIG_RULE_COMPLIANCE_PROPERTY,
    CONFIG_RULE_NAME_PROPERTY,
    "Description",
    "EvaluationModes",
    CONFIG_RULE_INPUT_PARAMETERS_PROPERTY,
    "MaximumExecutionFrequency",
    "Scope",
    "Source",
];

/// The `ConfigRule` members `DescribeConfigRules` reports that ARE properties of the type,
/// each under the property's name — which for every one of them is the member's own name.
/// Read for DRIFT, so only the members the read answers in the SAME shape the type
/// declares them in are reported.
const CONFIG_RULE_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    (CONFIG_RULE_NAME_PROPERTY, "ConfigRuleName"),
    ("Description", "Description"),
    ("EvaluationModes", "EvaluationModes"),
    (CONFIG_RULE_INPUT_PARAMETERS_PROPERTY, "InputParameters"),
    ("MaximumExecutionFrequency", "MaximumExecutionFrequency"),
    ("Scope", "Scope"),
    ("Source", "Source"),
];

/// The attributes `AWS::Config::ConfigRule` publishes that are CONFIG's own to mint — the
/// rule's ARN and the id that service keyed it by — each read off the rule Config answers
/// with.
const CONFIG_RULE_ATTRIBUTES: &[(&str, &str)] = &[
    ("Arn", "ConfigRuleArn"),
    ("ConfigRuleId", "ConfigRuleId"),
];

/// Every property `AWS::Config::ConfigurationRecorder` publishes: the recorder's name,
/// which CloudFormation decides, the role Config assumes to record, and the two objects
/// that say what is recorded and how often.
const CONFIGURATION_RECORDER_PROPERTIES: &[&str] =
    &["Name", "RoleARN", "RecordingGroup", "RecordingMode"];

/// The `ConfigurationRecorder` members `DescribeConfigurationRecorders` reports that are
/// properties of the type IN THE SAME SHAPE: the two scalars. The two nested objects are
/// reported by Config with their own members spelled in lower camel case, which is not the
/// shape the type declares them in, so they are left out — a property the actual
/// configuration does not carry is not evidence of anything ([`super::drift`]).
const CONFIGURATION_RECORDER_REPORTED_PROPERTIES: &[(&str, &str)] =
    &[("Name", "name"), ("RoleARN", "roleARN")];

/// Every property `AWS::Config::DeliveryChannel` publishes: the channel's name, which
/// CloudFormation decides, the bucket and prefix it delivers into, the KMS key it encrypts
/// what it delivers with, the topic it notifies, and the snapshot schedule.
const DELIVERY_CHANNEL_PROPERTIES: &[&str] = &[
    "Name",
    "S3BucketName",
    "S3KeyPrefix",
    "S3KmsKeyArn",
    "SnsTopicARN",
    "ConfigSnapshotDeliveryProperties",
];

/// The `DeliveryChannel` members `DescribeDeliveryChannels` reports that are properties of
/// the type IN THE SAME SHAPE: its scalars. The snapshot schedule is left out for the
/// reason [`CONFIGURATION_RECORDER_REPORTED_PROPERTIES`] records.
const DELIVERY_CHANNEL_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Name", "name"),
    ("S3BucketName", "s3BucketName"),
    ("S3KeyPrefix", "s3KeyPrefix"),
    ("S3KmsKeyArn", "s3KmsKeyArn"),
    ("SnsTopicARN", "snsTopicARN"),
];

/// Call one AWS Config operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_config(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        CONFIG_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CONFIG_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// The refusal a declaration carrying a property this group's arms do not carry to Config
/// answers: the resource was NOT deployed, rather than deployed without what the template
/// asked for.
fn unmapped_refusal(unmapped: &str, resource_type: &str) -> Refusal {
    Refusal {
        code: NOT_IMPLEMENTED.to_string(),
        message: format!(
            "this emulator does not carry the {unmapped} property of an {resource_type} to \
             AWS Config; the resource was not created."
        ),
    }
}

/// The declared properties of `resource_type` as the object the arms below send, or the
/// refusal a declaration carrying a property they do not carry earns.
///
/// Read by each type's `put_*_configuration`, which is both the create's and the update's
/// one request builder, so a changed declaration that adds an unmapped property is refused
/// exactly as the first deployment of one would be.
fn mapped_properties(
    properties: &Value,
    resource_type: &str,
    published: &[&str],
) -> Result<Map<String, Value>, Refusal> {
    let properties = match properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    match unmapped_property(&properties, published) {
        Some(unmapped) => Err(unmapped_refusal(&unmapped, resource_type)),
        None => Ok(properties),
    }
}

// ---------------------------------------------------------------------------
// AWS Config: AWS::Config::ConfigurationRecorder
// ---------------------------------------------------------------------------

/// Create the configuration recorder an `AWS::Config::ConfigurationRecorder` declares, in
/// Config itself, so a client that deployed the stack can start it and read it back —
/// `DescribeConfigurationRecorders` answers with it — rather than Config's
/// `NoSuchConfigurationRecorderException` for a recorder nothing created.
pub(super) async fn put_configuration_recorder(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    put_recorder_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await
}

/// `PutConfigurationRecorder` with `properties` as the recorder and `name` as the one
/// member CloudFormation decides — the call behind both this type's create and its update,
/// so the two cannot send a different request for the same declaration, nor judge a
/// declared property differently ([`mapped_properties`]).
///
/// That one call is documented "Creates or updates the customer managed configuration
/// recorder", which is what makes it the update: a deployment that changes the declared
/// recording group reaches Config as the configuration the template now declares, under
/// the same name.
pub(super) async fn put_recorder_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let properties = mapped_properties(
        properties,
        CONFIG_CONFIGURATION_RECORDER,
        CONFIGURATION_RECORDER_PROPERTIES,
    )?;
    let mut recorder = match lower_camel_members(&Value::Object(properties), 0)? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    recorder.insert("name".to_string(), Value::String(name.to_string()));
    call_config(
        ctx,
        region,
        "PutConfigurationRecorder",
        &serde_json::json!({ "ConfigurationRecorder": Value::Object(recorder) }),
    )
    .await?;
    Ok(Created::default())
}

/// Delete the configuration recorder a stack created. A recorder Config no longer holds
/// (`NoSuchConfigurationRecorderException`, judged on the CODE and never on a message that
/// carries the request's own name) is already in the state the delete asks for.
pub(super) async fn delete_configuration_recorder(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_config(
        ctx,
        region,
        "DeleteConfigurationRecorder",
        &serde_json::json!({ "ConfigurationRecorderName": physical_resource_id }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CONFIG_NO_SUCH_RECORDER => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The recorder's configuration as Config reports it, for drift detection. A recorder
/// Config no longer holds was deleted.
///
/// `DescribeConfigurationRecorders` answers a LIST, and the one entry it carries for the
/// name asked about is the recorder; an empty list is a recorder that is gone, which that
/// read answers for a name it holds no recorder for in place of a refusal.
pub(super) async fn describe_configuration_recorder(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = match call_config(
        ctx,
        region,
        "DescribeConfigurationRecorders",
        &serde_json::json!({ "ConfigurationRecorderNames": [physical_resource_id] }),
    )
    .await
    {
        Ok(answer) => answer,
        Err(refusal) if refusal.code == CONFIG_NO_SUCH_RECORDER => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(recorder) = first_listed(&answered, "ConfigurationRecorders") else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        &recorder,
        CONFIGURATION_RECORDER_REPORTED_PROPERTIES,
    )))
}

// ---------------------------------------------------------------------------
// AWS Config: AWS::Config::DeliveryChannel
// ---------------------------------------------------------------------------

/// Create the delivery channel an `AWS::Config::DeliveryChannel` declares, in Config
/// itself, so the recorder the stack declares beside it has somewhere to deliver what it
/// records and `DescribeDeliveryChannels` answers with the channel.
pub(super) async fn put_delivery_channel(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    put_channel_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await
}

/// `PutDeliveryChannel` with `properties` as the channel and `name` as the one member
/// CloudFormation decides — the call behind both this type's create and its update, which
/// is documented "Creates or updates a delivery channel … You can use this operation to
/// create a new delivery channel or to update the Amazon S3 bucket and the Amazon SNS
/// topic of an existing delivery channel". Both judge a declared property through the same
/// [`mapped_properties`].
pub(super) async fn put_channel_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let properties =
        mapped_properties(properties, CONFIG_DELIVERY_CHANNEL, DELIVERY_CHANNEL_PROPERTIES)?;
    let mut channel = match lower_camel_members(&Value::Object(properties), 0)? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    channel.insert("name".to_string(), Value::String(name.to_string()));
    call_config(
        ctx,
        region,
        "PutDeliveryChannel",
        &serde_json::json!({ "DeliveryChannel": Value::Object(channel) }),
    )
    .await?;
    Ok(Created::default())
}

/// Delete the delivery channel a stack created. A channel Config no longer holds
/// (`NoSuchDeliveryChannelException`) is already in the state the delete asks for; a
/// channel Config will not delete while it is the last one of a RUNNING recorder
/// (`LastDeliveryChannelDeleteFailedException`) is that service's own refusal and is
/// reported as such.
pub(super) async fn delete_delivery_channel(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_config(
        ctx,
        region,
        "DeleteDeliveryChannel",
        &serde_json::json!({ "DeliveryChannelName": physical_resource_id }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CONFIG_NO_SUCH_CHANNEL => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The channel's configuration as Config reports it, for drift detection, read the way the
/// recorder's is ([`describe_configuration_recorder`]).
pub(super) async fn describe_delivery_channel(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = match call_config(
        ctx,
        region,
        "DescribeDeliveryChannels",
        &serde_json::json!({ "DeliveryChannelNames": [physical_resource_id] }),
    )
    .await
    {
        Ok(answer) => answer,
        Err(refusal) if refusal.code == CONFIG_NO_SUCH_CHANNEL => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(channel) = first_listed(&answered, "DeliveryChannels") else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        &channel,
        DELIVERY_CHANNEL_REPORTED_PROPERTIES,
    )))
}

// ---------------------------------------------------------------------------
// AWS Config: AWS::Config::ConfigRule
// ---------------------------------------------------------------------------

/// Create the Config rule an `AWS::Config::ConfigRule` declares, in Config itself, so a
/// client that deployed the compliance control the stack declares can read it back —
/// `DescribeConfigRules` answers with it — and Config evaluates the resources its scope
/// covers, rather than answering `NoSuchConfigRuleException` for a rule nothing created.
pub(super) async fn put_config_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    put_config_rule_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await
}

/// `PutConfigRule` with `properties` as the rule and `name` as the one member
/// CloudFormation decides — the call behind both this type's create and its update, which
/// is documented "Adds or updates an Config rule to evaluate if your Amazon Web Services
/// resources comply with your desired configurations". Both judge a declared property
/// through the same [`mapped_properties`].
///
/// The rule is read back afterwards for the two attributes the type publishes that are
/// CONFIG's own to mint ([`CONFIG_RULE_ATTRIBUTES`]): `PutConfigRule` answers with no
/// document at all, so the ARN and the rule id a stack publishes for the resource are the
/// ones that service's own read reports. A read that answers for no rule leaves the
/// resource with the attributes CloudFormation's naming rules computed rather than with
/// invented ones.
pub(super) async fn put_config_rule_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let mut rule = mapped_properties(properties, CONFIG_CONFIG_RULE, CONFIG_RULE_PROPERTIES)?;
    // The verdict is Config's own and no member of this call.
    rule.remove(CONFIG_RULE_COMPLIANCE_PROPERTY);
    // "A string, in JSON format, that is passed to the Config rule Lambda function": the
    // parameters are a member modelled a STRING that carries a JSON document, which a
    // template may spell either way — so they are rendered by this module's ONE
    // JSON-to-text rule, the same one every other forwarded document member goes through.
    render_document_members(&mut rule, &[CONFIG_RULE_INPUT_PARAMETERS_PROPERTY]);
    rule.insert(
        CONFIG_RULE_NAME_PROPERTY.to_string(),
        Value::String(name.to_string()),
    );
    call_config(
        ctx,
        region,
        "PutConfigRule",
        &serde_json::json!({ "ConfigRule": Value::Object(rule) }),
    )
    .await?;
    let reported = read_config_rule(ctx, region, name).await?;
    Ok(Created::attributes(
        reported
            .map(|rule| reported_attributes(&rule, CONFIG_RULE_ATTRIBUTES))
            .unwrap_or_default(),
    ))
}

/// Delete the Config rule a stack created. A rule Config no longer holds
/// (`NoSuchConfigRuleException`) is already in the state the delete asks for.
pub(super) async fn delete_config_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_config(
        ctx,
        region,
        "DeleteConfigRule",
        &serde_json::json!({ "ConfigRuleName": physical_resource_id }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CONFIG_NO_SUCH_RULE => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The rule's configuration as Config reports it, for drift detection: the members
/// `DescribeConfigRules` answers that the type declares in the same shape. A rule Config no
/// longer holds was deleted.
pub(super) async fn describe_config_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_config_rule(ctx, region, physical_resource_id)
        .await?
        .map(|rule| reported_as_properties(&rule, CONFIG_RULE_REPORTED_PROPERTIES)))
}

/// The `ConfigRule` Config answers for one rule, or `None` for a name it holds no rule for
/// — which that read reports as `NoSuchConfigRuleException` ("The Config rule in the
/// request is not valid. Verify that the rule is an Config Process Check rule, that the
/// rule name is correct, and that valid Amazon Resouce Names (ARNs) are used.") rather
/// than as an empty list.
async fn read_config_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    match call_config(
        ctx,
        region,
        "DescribeConfigRules",
        &serde_json::json!({ "ConfigRuleNames": [name] }),
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "ConfigRules")),
        Err(refusal) if refusal.code == CONFIG_NO_SUCH_RULE => Ok(None),
        Err(refusal) => Err(refusal),
    }
}
