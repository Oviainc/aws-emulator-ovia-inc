// SPDX-License-Identifier: CC0-1.0

//! How this service judges the ARGUMENTS of a request, over the `awsJson1_1` wire.
//!
//! Two rules are asserted here, each over a set of inputs rather than over one recorded
//! request:
//!
//! 1. a value that does not have the shape of an IAM role ARN is refused with HTTP 400
//!    and the service's own parameter-value code — `InvalidParameterValueException`,
//!    "Invalid role ARN." — and NOT with the generic constraint error the framework's
//!    boundary walk would otherwise answer. The rule is keyed to the `RoleARN` shape, so
//!    it holds for every member that targets it: a destination's own role, the role of a
//!    nested S3 or VPC configuration, a source's role, and the role of an update;
//! 2. a request whose own arguments cannot hold together is refused as an ARGUMENT fault
//!    before the stream it names is resolved, so the answer does not depend on whether
//!    that stream exists or on what state it is in. `KeyARN` is required exactly when
//!    `KeyType` is `CUSTOMER_MANAGED_CMK`
//!    (`smithy_query(query="shape",shape="DeliveryStreamEncryptionConfigurationInput")`),
//!    and that dependency is a property of the request alone;
//! 3. the same argument-dependency rule over a create's SOURCE: the declared
//!    `DeliveryStreamType` fixes which source configuration the create takes, so the one
//!    that type names is required and any other is refused — as an argument fault whose
//!    sentence is built from the request (the member and the declared type), never from a
//!    value the service made up.
//!
//! The IAM resolution of a role Firehose must be able to ASSUME is a cross-service rule
//! (it needs IAM mounted beside this service) and lives in
//! `core/router/tests/all/firehose_destination_role.rs`.
//!
//! The harness (`Emulator`, `s3_configuration`, …) is the lifecycle suite's.

use serde_json::{json, Value};

use super::delivery_stream_lifecycle::{
    error_code, error_message, s3_configuration, Emulator, BUCKET_ARN, PAST_THE_TRANSITION, REGION,
    ROLE_ARN,
};

/// The error code Firehose answers a value that is not an IAM role ARN with. Not a
/// modelled shape: the service model carries no `InvalidParameterValueException`, and this
/// is the code a client branches on.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The sentence that code carries.
const INVALID_ROLE_ARN: &str = "Invalid role ARN.";

/// Values that are NOT IAM role ARNs, each violating the `RoleARN` pattern
/// (`smithy_query(query="shape", shape="RoleARN")` ->
/// `^arn:.*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$`) in a different way.
fn malformed_role_arns() -> Vec<(&'static str, &'static str)> {
    vec![
        ("not an ARN at all", "firehose-delivery"),
        (
            "an ARN that names another vendor",
            "arn:aws:kms:us-west-2:000000000000:key/delivery",
        ),
        (
            "an ARN that names another IAM entity type",
            "arn:aws:iam::000000000000:user/delivery",
        ),
        (
            "an account id one digit short",
            "arn:aws:iam::00000000000:role/delivery",
        ),
        (
            "a role ARN with no role name",
            "arn:aws:iam::000000000000:role/",
        ),
        (
            "a role name holding a character the pattern excludes",
            "arn:aws:iam::000000000000:role/firehose delivery",
        ),
        (
            "an IAM ARN carrying a Region, which IAM ARNs do not",
            "arn:aws:iam:us-west-2:000000000000:role/delivery",
        ),
    ]
}

/// A `SplunkDestinationConfiguration` whose nested `S3Configuration` carries `role_arn`.
fn splunk_with_s3_role(role_arn: &str) -> Value {
    json!({
        "HECEndpoint": "https://splunk.example.com:8088",
        "HECEndpointType": "Raw",
        "S3Configuration": { "RoleARN": role_arn, "BucketARN": BUCKET_ARN },
    })
}

/// An `ElasticsearchDestinationConfiguration` whose nested `VpcConfiguration` carries
/// `role_arn`, with the destination's own role left well formed.
fn elasticsearch_with_vpc_role(role_arn: &str) -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "DomainARN": format!("arn:aws:es:{REGION}:000000000000:domain/logs"),
        "IndexName": "logs",
        "S3Configuration": s3_configuration(),
        "VpcConfiguration": {
            "SubnetIds": ["subnet-0123456789abcdef0"],
            "RoleARN": role_arn,
            "SecurityGroupIds": ["sg-0123456789abcdef0"],
        },
    })
}

/// One place a request can carry a role ARN, as the request that carries it there.
struct RoleMember {
    case: &'static str,
    operation: &'static str,
    /// Builds the request with `role_arn` in this member.
    request: fn(&str, &str) -> Value,
}

/// Every member below targets the one `RoleARN` shape, so the rule has to hold for all of
/// them — including the ones the issue did not cite (a nested VPC role, a source role, and
/// a role on `UpdateDestination` rather than on the create).
fn role_members() -> Vec<RoleMember> {
    vec![
        RoleMember {
            case: "the S3 destination's own role",
            operation: "CreateDeliveryStream",
            request: |name, role_arn| {
                json!({
                    "DeliveryStreamName": name,
                    "S3DestinationConfiguration": {
                        "RoleARN": role_arn,
                        "BucketARN": BUCKET_ARN,
                    },
                })
            },
        },
        RoleMember {
            case: "the extended S3 destination's own role",
            operation: "CreateDeliveryStream",
            request: |name, role_arn| {
                json!({
                    "DeliveryStreamName": name,
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": role_arn,
                        "BucketARN": BUCKET_ARN,
                    },
                })
            },
        },
        RoleMember {
            case: "the role of a nested S3 configuration",
            operation: "CreateDeliveryStream",
            request: |name, role_arn| {
                json!({
                    "DeliveryStreamName": name,
                    "SplunkDestinationConfiguration": splunk_with_s3_role(role_arn),
                })
            },
        },
        RoleMember {
            case: "the role of a nested VPC configuration",
            operation: "CreateDeliveryStream",
            request: |name, role_arn| {
                json!({
                    "DeliveryStreamName": name,
                    "ElasticsearchDestinationConfiguration":
                        elasticsearch_with_vpc_role(role_arn),
                })
            },
        },
        RoleMember {
            case: "the role a Kinesis-sourced stream reads its source with",
            operation: "CreateDeliveryStream",
            request: |name, role_arn| {
                json!({
                    "DeliveryStreamName": name,
                    "DeliveryStreamType": "KinesisStreamAsSource",
                    "KinesisStreamSourceConfiguration": {
                        "KinesisStreamARN":
                            format!("arn:aws:kinesis:{REGION}:000000000000:stream/{name}"),
                        "RoleARN": role_arn,
                    },
                    "S3DestinationConfiguration": s3_configuration(),
                })
            },
        },
        RoleMember {
            case: "the role in an MSK source's authentication configuration",
            operation: "CreateDeliveryStream",
            request: |name, role_arn| {
                json!({
                    "DeliveryStreamName": name,
                    "DeliveryStreamType": "MSKAsSource",
                    "MSKSourceConfiguration": {
                        "MSKClusterARN":
                            format!("arn:aws:kafka:{REGION}:000000000000:cluster/{name}/1"),
                        "TopicName": "events",
                        "AuthenticationConfiguration": {
                            "RoleARN": role_arn,
                            "Connectivity": "PRIVATE",
                        },
                    },
                    "S3DestinationConfiguration": s3_configuration(),
                })
            },
        },
        RoleMember {
            case: "the role of a destination update, on another operation",
            operation: "UpdateDestination",
            request: |name, role_arn| {
                json!({
                    "DeliveryStreamName": name,
                    "CurrentDeliveryStreamVersionId": "1",
                    "DestinationId": "destinationId-000000000001",
                    "S3DestinationUpdate": { "RoleARN": role_arn },
                })
            },
        },
    ]
}

#[tokio::test]
async fn a_value_that_is_not_a_role_arn_is_refused_as_an_invalid_role_arn() {
    let emulator = Emulator::new();

    // Every malformed value, in every member that takes a role: the rule is the shape of
    // the value, so neither the member nor the operation may change the answer.
    for member in role_members() {
        for (index, (shape_case, role_arn)) in malformed_role_arns().into_iter().enumerate() {
            let case = format!("{} / {shape_case}", member.case);
            let name = format!("role-shape-{index}");
            let (status, refused) = emulator
                .call(member.operation, (member.request)(&name, role_arn))
                .await;
            assert_eq!(status, 400, "case {case:?}: got {status} {refused:?}");
            assert_eq!(
                error_code(&refused),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected the service's parameter-value code, got {refused:?}"
            );
            assert_eq!(
                error_message(&refused),
                INVALID_ROLE_ARN,
                "case {case:?}: got {refused:?}"
            );
            // The refusal states the fault and nothing of the value that caused it.
            assert!(
                !error_message(&refused).contains(role_arn),
                "case {case:?}: the rejected value must not be echoed, got {refused:?}"
            );
        }
    }

    // Nothing was created by any refused request.
    let (_, listed) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(
        listed["DeliveryStreamNames"],
        json!([]),
        "a refused request stores nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_well_formed_role_arn_is_admitted_in_every_member_that_takes_one() {
    // The same members, with a role ARN that does have the shape: the refusal above is
    // about the value, so none of these may be answered as an invalid role ARN. (No IAM
    // is mounted beside this service, so the assumability gate has nothing to ask and
    // admits the request — see the router suite for what it answers when IAM is there.)
    for (index, member) in role_members().into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("role-shaped-{index}");
        // `UpdateDestination` needs a stream to update, and the update is applied to the
        // destination the create minted.
        if member.operation == "UpdateDestination" {
            emulator.create_stream(&name, None).await;
            tokio::time::sleep(PAST_THE_TRANSITION).await;
        }
        let (status, answered) = emulator
            .call(member.operation, (member.request)(&name, ROLE_ARN))
            .await;
        assert_eq!(
            status,
            200,
            "case {:?}: a role ARN of the modelled shape is admitted, got {status} {answered:?}",
            member.case
        );
    }
}

#[tokio::test]
async fn a_constraint_violation_that_is_not_a_role_arn_keeps_the_generic_refusal() {
    let emulator = Emulator::new();

    // (case, request, the code and message the refusal must carry)
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "a stream name violating its own pattern",
            json!({
                "DeliveryStreamName": "bad name!",
                "S3DestinationConfiguration": s3_configuration(),
            }),
            "ValidationException",
            "1 validation error detected: Value at 'deliveryStreamName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
        (
            "a file extension violating its own pattern",
            json!({
                "DeliveryStreamName": "bad-extension",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "FileExtension": "nodot",
                },
            }),
            "ValidationException",
            "1 validation error detected: Value at \
             'extendedS3DestinationConfiguration.fileExtension' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             ^(|\\.[0-9a-z!\\-_.*'()]+)$"
                .to_string(),
        ),
        (
            // A role fault beside another one: Firehose validates the role itself, so the
            // role is the fault it reports.
            "a malformed role ARN beside another violation",
            json!({
                "DeliveryStreamName": "bad name!",
                "S3DestinationConfiguration": {
                    "RoleARN": "not-an-arn",
                    "BucketARN": BUCKET_ARN,
                },
            }),
            INVALID_PARAMETER_VALUE,
            INVALID_ROLE_ARN.to_string(),
        ),
    ];

    for (case, request, expected_code, expected_message) in rows {
        let (status, refused) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(status, 400, "case {case:?}: got {status} {refused:?}");
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }
}

/// One row of the table below: how the stream the call names is set up, what encryption
/// configuration the call carries, and the answer it must get.
struct Row {
    case: &'static str,
    setup: Stream,
    /// The `DeliveryStreamEncryptionConfigurationInput` the call carries, if any.
    configuration: Option<Value>,
    /// The code the refusal must carry, with the message it must state (empty where the
    /// message rule does not fix one); `None` for a call that must succeed.
    refused: Option<(&'static str, String)>,
}

/// How the stream a `StartDeliveryStreamEncryption` names is set up before the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    /// No stream of that name was ever created.
    Absent,
    /// Created and left inside its creation span, so it is still `CREATING`.
    Creating,
    /// Created and settled.
    Active,
}

#[tokio::test]
async fn an_argument_fault_is_answered_before_the_stream_is_resolved() {
    // A customer managed CMK in the signing Region, satisfying `KeyARN`'s modelled
    // pattern.
    let key_arn = format!("arn:aws:kms:{REGION}:000000000000:key/demo-key");
    let other_region_key = "arn:aws:kms:eu-west-1:000000000000:key/demo-key".to_string();
    // The two sentences the `KeyType`/`KeyARN` dependency is stated in. `KeyType`'s
    // catalog is the model's whole enum (`smithy_query(query="shape",shape="KeyType")` ->
    // ["AWS_OWNED_CMK", "CUSTOMER_MANAGED_CMK"]), and each value's rule is the other's
    // mirror.
    let key_arn_required = "KeyARN has to be specified when KeyType is CUSTOMER_MANAGED_CMK";
    let key_arn_forbidden = "KeyARN cannot be specified when KeyType is AWS_OWNED_CMK";

    let rows: Vec<Row> = vec![
        Row {
            case: "a customer managed CMK with no key ARN, on a stream that exists",
            setup: Stream::Active,
            configuration: Some(json!({ "KeyType": "CUSTOMER_MANAGED_CMK" })),
            refused: Some(("InvalidArgumentException", key_arn_required.to_string())),
        },
        Row {
            // The divergence this suite was written for: the fault is the argument, so
            // the answer cannot be a resource-identity error.
            case: "a customer managed CMK with no key ARN, on a stream that does not exist",
            setup: Stream::Absent,
            configuration: Some(json!({ "KeyType": "CUSTOMER_MANAGED_CMK" })),
            refused: Some(("InvalidArgumentException", key_arn_required.to_string())),
        },
        Row {
            case: "a customer managed CMK with no key ARN, on a stream that has not settled",
            setup: Stream::Creating,
            configuration: Some(json!({ "KeyType": "CUSTOMER_MANAGED_CMK" })),
            refused: Some(("InvalidArgumentException", key_arn_required.to_string())),
        },
        Row {
            case: "a service-owned CMK with a key ARN, on a stream that exists",
            setup: Stream::Active,
            configuration: Some(json!({ "KeyType": "AWS_OWNED_CMK", "KeyARN": key_arn })),
            refused: Some(("InvalidArgumentException", key_arn_forbidden.to_string())),
        },
        Row {
            case: "a service-owned CMK with a key ARN, on a stream that does not exist",
            setup: Stream::Absent,
            configuration: Some(json!({ "KeyType": "AWS_OWNED_CMK", "KeyARN": key_arn })),
            refused: Some(("InvalidArgumentException", key_arn_forbidden.to_string())),
        },
        Row {
            // A CMK of another Region is a fault of the request's own values too, so it
            // is judged on the same side of the stream lookup.
            case: "a CMK in another Region, on a stream that does not exist",
            setup: Stream::Absent,
            configuration: Some(json!({
                "KeyType": "CUSTOMER_MANAGED_CMK",
                "KeyARN": other_region_key,
            })),
            refused: Some(("InvalidKMSResourceException", String::new())),
        },
        Row {
            // Once the arguments hold together, the missing stream is what is reported.
            case: "a configuration that holds together, on a stream that does not exist",
            setup: Stream::Absent,
            configuration: Some(json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn })),
            refused: Some(("ResourceNotFoundException", String::new())),
        },
        Row {
            case: "no configuration at all, on a stream that does not exist",
            setup: Stream::Absent,
            configuration: None,
            refused: Some(("ResourceNotFoundException", String::new())),
        },
        Row {
            case: "a configuration that holds together, on a stream that exists",
            setup: Stream::Active,
            configuration: Some(json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn })),
            refused: None,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let Row {
            case,
            setup,
            configuration,
            refused: expected,
        } = row;
        let emulator = Emulator::new();
        let name = format!("argument-first-{index}");
        match setup {
            Stream::Absent => {}
            Stream::Creating => emulator.create_stream(&name, None).await,
            Stream::Active => {
                emulator.create_stream(&name, None).await;
                tokio::time::sleep(PAST_THE_TRANSITION).await;
            }
        }

        let mut request = json!({ "DeliveryStreamName": name });
        if let Some(configuration) = configuration {
            request["DeliveryStreamEncryptionConfigurationInput"] = configuration;
        }
        let (status, answered) = emulator
            .call("StartDeliveryStreamEncryption", request)
            .await;

        match expected {
            None => assert_eq!(status, 200, "case {case:?}: got {status} {answered:?}"),
            Some((expected_code, expected_message)) => {
                assert_eq!(status, 400, "case {case:?}: got {status} {answered:?}");
                assert_eq!(
                    error_code(&answered),
                    expected_code,
                    "case {case:?}: got {answered:?}"
                );
                if !expected_message.is_empty() {
                    let message = error_message(&answered);
                    assert_eq!(
                        message, expected_message,
                        "case {case:?}: expected {expected_message:?}, got {message:?}"
                    );
                }
                // The missing stream is reported ONLY when the arguments hold together:
                // an argument fault never answers as a resource-identity error.
                if expected_code == "InvalidArgumentException" {
                    assert!(
                        !error_message(&answered).contains("not found"),
                        "case {case:?}: an argument fault must not report a missing \
                         resource, got {answered:?}"
                    );
                }
            }
        }
    }
}

/// The source configuration member each `DeliveryStreamType` is configured with.
///
/// The catalog is the model's whole enum
/// (`smithy_query(query="shape",shape="DeliveryStreamType")` -> ["DirectPut",
/// "KinesisStreamAsSource", "MSKAsSource", "DatabaseAsSource"]); each entry's member is
/// the `CreateDeliveryStreamInput` member that names that source
/// (`smithy_query(query="operation",operation="CreateDeliveryStream")`).
const SOURCE_OF_TYPE: &[(&str, &str)] = &[
    ("DirectPut", "DirectPutSourceConfiguration"),
    ("KinesisStreamAsSource", "KinesisStreamSourceConfiguration"),
    ("MSKAsSource", "MSKSourceConfiguration"),
    ("DatabaseAsSource", "DatabaseSourceConfiguration"),
];

/// A well-formed value for the source configuration member `member` takes, for the three
/// whose shape is small enough to spell; `None` for one this suite only ever omits.
fn source_configuration(member: &str, name: &str) -> Option<Value> {
    match member {
        // `ThroughputHintInMBs` is @required, range 1..100
        // (`smithy_query(query="shape",shape="DirectPutSourceConfiguration")`).
        "DirectPutSourceConfiguration" => Some(json!({ "ThroughputHintInMBs": 1 })),
        "KinesisStreamSourceConfiguration" => Some(json!({
            "KinesisStreamARN": format!("arn:aws:kinesis:{REGION}:000000000000:stream/{name}"),
            "RoleARN": ROLE_ARN,
        })),
        "MSKSourceConfiguration" => Some(json!({
            "MSKClusterARN": format!("arn:aws:kafka:{REGION}:000000000000:cluster/{name}/1"),
            "TopicName": "events",
            "AuthenticationConfiguration": {
                "RoleARN": ROLE_ARN,
                "Connectivity": "PRIVATE",
            },
        })),
        // `DatabaseSourceConfiguration` carries a VPC endpoint, a secret and table
        // include/exclude lists; this suite only ever leaves it out.
        _ => None,
    }
}

/// One create the source rule must ADMIT: the declared type and the source configuration
/// member it carries, if any.
struct Admitted {
    case: String,
    stream_type: &'static str,
    source: Option<(&'static str, Value)>,
}

#[tokio::test]
async fn the_source_configuration_a_create_takes_is_fixed_by_its_declared_type() {
    let emulator = Emulator::new();

    // The absent source: every type whose source configuration this suite can leave out
    // is refused for leaving it out, and the sentence names the member that is missing and
    // the type that requires it. `DirectPut` is the one type whose source configuration is
    // optional ("Provider applications access the Firehose stream directly"), so it is
    // asserted below as an ADMITTED create rather than here.
    for (index, (stream_type, member)) in SOURCE_OF_TYPE
        .iter()
        .filter(|(stream_type, _)| *stream_type != "DirectPut")
        .enumerate()
    {
        let case = format!("{stream_type} with no {member}");
        let name = format!("source-absent-{index}");
        let (status, refused) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": name,
                    "DeliveryStreamType": stream_type,
                    "S3DestinationConfiguration": s3_configuration(),
                }),
            )
            .await;
        assert_eq!(status, 400, "case {case:?}: got {status} {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains(member) && message.contains(stream_type),
            "case {case:?}: the refusal must name the missing member and the declared \
             type, got {message:?}"
        );
        // A refused create stores nothing.
        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&described).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused create must store nothing, got {described:?}"
        );
    }

    // The foreign source: every (declared type, supplied member) pair where the member is
    // not the one that type names is refused, whichever way round.
    let mut index = 0usize;
    for (stream_type, _) in SOURCE_OF_TYPE {
        for (_, member) in SOURCE_OF_TYPE {
            if SOURCE_OF_TYPE
                .iter()
                .any(|(owner, owned)| owner == stream_type && owned == member)
            {
                continue;
            }
            let name = format!("source-foreign-{index}");
            index += 1;
            let Some(configuration) = source_configuration(member, &name) else {
                continue;
            };
            let case = format!("{stream_type} carrying {member}");
            let mut request = json!({
                "DeliveryStreamName": name,
                "DeliveryStreamType": stream_type,
                "S3DestinationConfiguration": s3_configuration(),
            });
            request[*member] = configuration;
            let (status, refused) = emulator.call("CreateDeliveryStream", request).await;
            assert_eq!(status, 400, "case {case:?}: got {status} {refused:?}");
            assert_eq!(
                error_code(&refused),
                "InvalidArgumentException",
                "case {case:?}: got {refused:?}"
            );
            let message = error_message(&refused);
            assert!(
                message.contains(member) && message.contains(stream_type),
                "case {case:?}: the refusal must name the foreign member and the declared \
                 type, got {message:?}"
            );
        }
    }

    // The matching source: the member the declared type names is admitted, and the create
    // reads back under that type. `DirectPut` is admitted both with its optional source
    // configuration and without one.
    let mut admitted: Vec<Admitted> = vec![Admitted {
        case: "DirectPut with no source configuration at all".to_string(),
        stream_type: "DirectPut",
        source: None,
    }];
    for (stream_type, member) in SOURCE_OF_TYPE {
        if let Some(configuration) = source_configuration(member, "source-match") {
            admitted.push(Admitted {
                case: format!("{stream_type} carrying its own {member}"),
                stream_type,
                source: Some((member, configuration)),
            });
        }
    }

    for (index, admitted) in admitted.into_iter().enumerate() {
        let Admitted {
            case,
            stream_type,
            source,
        } = admitted;
        let name = format!("source-match-{index}");
        let mut request = json!({
            "DeliveryStreamName": name,
            "DeliveryStreamType": stream_type,
            "S3DestinationConfiguration": s3_configuration(),
        });
        if let Some((member, configuration)) = source {
            request[member] = configuration;
        }
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {created:?}");
        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {described:?}");
        assert_eq!(
            described["DeliveryStreamDescription"]["DeliveryStreamType"],
            json!(stream_type),
            "case {case:?}: the created stream reports the type it was created with, \
             got {described:?}"
        );
    }
}

#[tokio::test]
async fn the_same_argument_dependency_is_enforced_on_the_create_that_takes_it() {
    let emulator = Emulator::new();
    let key_arn = format!("arn:aws:kms:{REGION}:000000000000:key/created-key");

    // `CreateDeliveryStream` takes the same `DeliveryStreamEncryptionConfigurationInput`,
    // so the dependency is the same rule there.
    //
    // (case, the configuration, the expected message — `None` for an accepted create)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a customer managed CMK with no key ARN",
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK" }),
            Some("KeyARN has to be specified when KeyType is CUSTOMER_MANAGED_CMK"),
        ),
        (
            "a service-owned CMK with a key ARN",
            json!({ "KeyType": "AWS_OWNED_CMK", "KeyARN": key_arn }),
            Some("KeyARN cannot be specified when KeyType is AWS_OWNED_CMK"),
        ),
        (
            "a customer managed CMK with its key ARN",
            json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn }),
            None,
        ),
        (
            "a service-owned CMK with no key ARN",
            json!({ "KeyType": "AWS_OWNED_CMK" }),
            None,
        ),
    ];

    for (index, (case, configuration, expected)) in rows.into_iter().enumerate() {
        let name = format!("create-encrypted-{index}");
        let (status, answered) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": name,
                    "S3DestinationConfiguration": s3_configuration(),
                    "DeliveryStreamEncryptionConfigurationInput": configuration,
                }),
            )
            .await;
        match expected {
            None => assert_eq!(status, 200, "case {case:?}: got {status} {answered:?}"),
            Some(expected_message) => {
                assert_eq!(status, 400, "case {case:?}: got {status} {answered:?}");
                assert_eq!(
                    error_code(&answered),
                    "InvalidArgumentException",
                    "case {case:?}: got {answered:?}"
                );
                let message = error_message(&answered);
                assert_eq!(
                    message, expected_message,
                    "case {case:?}: expected {expected_message:?}, got {message:?}"
                );
                // A refused create stores nothing.
                let (status, described) = emulator
                    .call(
                        "DescribeDeliveryStream",
                        json!({ "DeliveryStreamName": name }),
                    )
                    .await;
                assert_eq!(
                    (status, error_code(&described).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case:?}: a refused create must store nothing, got {described:?}"
                );
            }
        }
    }
}
