// SPDX-License-Identifier: CC0-1.0

//! The request a stack hands the service that owns one of its resources.
//!
//! The rule under test: **the properties a template declares for a resource are evaluated
//! — every intrinsic function resolved against the stack's parameters, its pseudo
//! parameters and the OTHER resources the template declares — and that evaluated document
//! is what the owning service's create API is called with** (see
//! `src/state/provisioning.rs`). A property that cannot be evaluated is dropped rather
//! than sent as a guess, which is also what CloudFormation's own `AWS::NoValue` means.
//!
//! What `DescribeStacks` reports for a deployed stack is `stack_outputs.rs`; the resource
//! really created in the owning service is
//! `core/router/tests/all/cloudformation_provisions_stack_resources.rs`. This suite holds
//! the property evaluation itself to the rule, over a table whose rows differ in which
//! intrinsic a property is written with, how deeply it is nested inside the property tree,
//! and whether it resolves at all.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    self, Budget, Scope, StackScope,
};
use service_com_amazonaws_cloudformation_cloudformation::state::StoredResource;

/// The stack the rows are evaluated for.
const REGION: &str = "us-east-1";
const STACK_NAME: &str = "properties-stack";
const STACK_ID: &str =
    "arn:aws:cloudformation:us-east-1:000000000000:stack/properties-stack/0000";
/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Deploy `body` with `parameters` the way `StoredStack::deploy_template` does, and answer
/// the resources it produced — each with its evaluated `properties`.
fn deployed(body: &str, parameters: &[(&str, &str)]) -> Vec<StoredResource> {
    let declared = template::parse(body).expect("the template body is readable");
    let mut values: BTreeMap<String, String> = declared.parameter_defaults();
    for (key, value) in parameters {
        values.insert((*key).to_string(), (*value).to_string());
    }
    let stack = StackScope {
        region: REGION,
        stack_name: STACK_NAME,
        stack_id: STACK_ID,
        notification_arns: &[],
    };
    let mut budget = Budget::new();
    let parameter_types = declared.parameter_types();
    let no_resources: BTreeMap<String, StoredResource> = BTreeMap::new();
    let no_imports: BTreeMap<String, String> = BTreeMap::new();
    let no_stack_outputs =
        service_com_amazonaws_cloudformation_cloudformation::state::stack_outputs::Values::new();
    // The deployment's scope before any resource is resolved, exactly as
    // `StoredStack::deploy_template` builds it.
    let declaring = Scope {
        stack,
        parameters: &values,
        parameter_types: &parameter_types,
        resources: &no_resources,
        mappings: &declared.mappings,
        conditions: &declared.conditions,
        imports: &no_imports,
        stack_outputs: &no_stack_outputs,
    };
    let mut resources = template::provision(&declared, &declaring, &mut budget);
    let by_logical_id: BTreeMap<String, StoredResource> = resources
        .iter()
        .map(|resource| (resource.logical_id.clone(), resource.clone()))
        .collect();
    let scope = Scope {
        resources: &by_logical_id,
        ..declaring
    };
    template::resolve_properties(&declared, &mut resources, &scope, &mut budget);
    resources
}

/// The resource `logical_id` names.
fn resource<'a>(resources: &'a [StoredResource], logical_id: &str) -> &'a StoredResource {
    resources
        .iter()
        .find(|resource| resource.logical_id == logical_id)
        .unwrap_or_else(|| panic!("no resource {logical_id} in {resources:?}"))
}

/// A template declaring a bucket, a role and a stream whose properties read both of them,
/// with `stream` as the stream's property document.
fn template_with(stream: Value) -> String {
    json!({
        "Parameters": { "Prefix": { "Type": "String", "Default": "default/" } },
        "Resources": {
            "Bucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": { "BucketName": "properties-bucket" }
            },
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": { "RoleName": "properties-role" }
            },
            "Stream": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": stream
            }
        }
    })
    .to_string()
}

/// Every row: the property is evaluated to the value its intrinsic resolves to, inside
/// whatever object or array the template nested it in.
#[test]
fn a_resources_properties_are_evaluated_against_the_stacks_other_resources() {
    // The rows differ in the intrinsic the property is written with and in how deep the
    // property tree carries it.
    let rows: [(&str, Value, Value); 7] = [
        (
            "a literal property",
            json!({ "DeliveryStreamType": "DirectPut" }),
            json!({ "DeliveryStreamType": "DirectPut" }),
        ),
        (
            "Fn::GetAtt of a sibling resource, nested two levels deep",
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "BucketARN": { "Fn::GetAtt": ["Bucket", "Arn"] },
                    "RoleARN": { "Fn::GetAtt": ["Role", "Arn"] }
                }
            }),
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "BucketARN": "arn:aws:s3:::properties-bucket",
                    "RoleARN": format!("arn:aws:iam::{ACCOUNT_ID}:role/properties-role")
                }
            }),
        ),
        (
            "Ref of a sibling resource and of a parameter",
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "BucketName": { "Ref": "Bucket" },
                    "Prefix": { "Ref": "Prefix" }
                }
            }),
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "BucketName": "properties-bucket",
                    "Prefix": "raw/events/"
                }
            }),
        ),
        (
            "Fn::Sub over a pseudo parameter and an attribute",
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "ErrorOutputPrefix": { "Fn::Sub": "${AWS::Region}/${Bucket.Arn}/" }
                }
            }),
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "ErrorOutputPrefix": format!("{REGION}/arn:aws:s3:::properties-bucket/")
                }
            }),
        ),
        (
            "an intrinsic inside a list of objects",
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [
                            {
                                "Type": "Lambda",
                                "Parameters": [
                                    { "ParameterName": "RoleArn",
                                      "ParameterValue": { "Fn::GetAtt": ["Role", "Arn"] } }
                                ]
                            }
                        ]
                    }
                }
            }),
            json!({
                "ExtendedS3DestinationConfiguration": {
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [
                            {
                                "Type": "Lambda",
                                "Parameters": [
                                    { "ParameterName": "RoleArn",
                                      "ParameterValue":
                                          format!("arn:aws:iam::{ACCOUNT_ID}:role/properties-role") }
                                ]
                            }
                        ]
                    }
                }
            }),
        ),
        (
            "a property that cannot be evaluated is dropped",
            json!({
                "DeliveryStreamType": "DirectPut",
                "Tags": [{ "Key": "Imported", "Value": { "Fn::ImportValue": "elsewhere" } }],
                "ExtendedS3DestinationConfiguration": {
                    "Prefix": { "Ref": "NothingDeclaresThis" },
                    "CompressionFormat": "GZIP"
                }
            }),
            json!({
                "DeliveryStreamType": "DirectPut",
                "Tags": [{ "Key": "Imported" }],
                "ExtendedS3DestinationConfiguration": { "CompressionFormat": "GZIP" }
            }),
        ),
        (
            "AWS::NoValue removes the property, as CloudFormation documents",
            json!({
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": { "Prefix": { "Ref": "AWS::NoValue" } }
            }),
            json!({
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": {}
            }),
        ),
    ];

    for (case, declared, expected) in rows {
        let resources = deployed(&template_with(declared), &[("Prefix", "raw/events/")]);
        let actual = &resource(&resources, "Stream").properties;
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected the evaluated properties to be {expected}, got {actual}"
        );
    }
}

/// A property whose evaluation would grow past the bound one evaluated value may carry is
/// DROPPED, on the same bound the stack's outputs are held to — so the create request a
/// stack hands the owning service cannot be amplified by the template that declared it.
///
/// The rows differ in which intrinsic does the amplifying and in whether the result fits:
/// a `Fn::Join` inside the bound, one past it, and a nested pair whose product is past it.
#[test]
fn a_property_that_would_outgrow_the_value_bound_is_dropped() {
    // The bound on one evaluated value, from the reader's own published constant.
    let bound = template::MAX_RESOLVED_TEXT_BYTES;
    let piece = |bytes: usize| "x".repeat(bytes);

    let rows: [(&str, Value, bool); 3] = [
        (
            "a join that fits",
            json!({ "Fn::Join": ["", [piece(bound / 4), piece(bound / 4)]] }),
            true,
        ),
        (
            "a join one past the bound",
            json!({ "Fn::Join": ["", [piece(bound / 2 + 1), piece(bound / 2 + 1)]] }),
            false,
        ),
        (
            "a join whose delimiter is itself a join",
            json!({
                "Fn::Join": [
                    { "Fn::Join": ["", [piece(bound / 2), piece(bound / 2)]] },
                    ["", "", "", ""]
                ]
            }),
            false,
        ),
    ];

    for (case, declared, kept) in rows {
        let resources = deployed(
            &template_with(json!({ "ExtendedS3DestinationConfiguration": { "Prefix": declared } })),
            &[],
        );
        let properties = &resource(&resources, "Stream").properties;
        let present = properties["ExtendedS3DestinationConfiguration"]
            .get("Prefix")
            .is_some();
        assert_eq!(
            present, kept,
            "case {case:?}: expected the property to be kept={kept}, got {present} in \
             {properties}"
        );
    }
}

/// The identity a Firehose stream resource carries: what the create call names it with,
/// and what `Ref` and `Fn::GetAtt` report for it. `Ref` returns the delivery stream name
/// and `Fn::GetAtt Arn` the stream's ARN, whether the template names the stream or leaves
/// the name to CloudFormation — and a freshly declared record claims nothing about having
/// been created.
#[test]
fn a_firehose_stream_resource_carries_the_documented_return_values() {
    let named = deployed(
        &template_with(json!({ "DeliveryStreamName": "named-stream" })),
        &[],
    );
    let named = resource(&named, "Stream");
    assert_eq!(
        named.physical_resource_id, "named-stream",
        "expected Ref to report the declared stream name, got {named:?}"
    );
    assert_eq!(
        named.attributes.get("Arn").map(String::as_str),
        Some(format!("arn:aws:firehose:{REGION}:{ACCOUNT_ID}:deliverystream/named-stream").as_str()),
        "expected the stream's ARN attribute, got {named:?}"
    );
    assert!(
        !named.materialised,
        "expected a freshly declared resource not to claim it was created, got {named:?}"
    );

    let unnamed = deployed(&template_with(json!({ "DeliveryStreamType": "DirectPut" })), &[]);
    let unnamed = resource(&unnamed, "Stream");
    let generated = &unnamed.physical_resource_id;
    assert!(
        generated.starts_with(&format!("{STACK_NAME}-Stream-")),
        "expected a generated name of CloudFormation's shape, got {generated:?}"
    );
    assert_eq!(
        unnamed.attributes.get("Arn").map(String::as_str),
        Some(format!("arn:aws:firehose:{REGION}:{ACCOUNT_ID}:deliverystream/{generated}").as_str()),
        "expected the generated stream's ARN attribute, got {unnamed:?}"
    );
}
