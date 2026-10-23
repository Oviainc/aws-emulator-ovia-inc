// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, over the wire.
//!
//! Three rules are asserted here, each over a set of inputs rather than one recorded
//! request:
//!
//! 1. a constraint violation states the member path and the constraint and NEVER the
//!    value that violated it;
//! 2. a `@pattern` is reported as the expression a value must satisfy — the model's
//!    anchoring normalised away, and restated once around the group for a top-level
//!    alternation;
//! 3. a refusal the handlers raise themselves is scoped to the ACCOUNT: a duplicate
//!    name states name and account, a lookup that found nothing states the account and
//!    not the name it was given, a delete the state blocks states name, account and
//!    state, and two destination configurations that exclude each other are refused with
//!    one sentence over the members supplied.
//!
//! Every request goes through the real router, so what is asserted is the body a client
//! reads. The harness (`Emulator`, `s3_configuration`, …) is the lifecycle suite's.

use serde_json::{json, Value};

use super::delivery_stream_lifecycle::{
    error_code, s3_configuration, Emulator, BUCKET_ARN, ROLE_ARN,
};
// `error_message` lives beside `error_code` in the lifecycle suite, so every suite in this
// test crate reads a refusal the same way rather than carrying its own copy. It is
// re-exported here because a sibling suite imports it from this module.
pub(crate) use super::delivery_stream_lifecycle::error_message;

/// The envelope one constraint violation is reported in.
fn one_error(clause: String) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a value that was supplied and rejected: the member and the
/// constraint, and nothing of the value.
fn value_at(member: &str, requirement: &str) -> String {
    one_error(format!(
        "Value at '{member}' failed to satisfy constraint: {requirement}"
    ))
}

/// The clause for a required member that was absent: there was no value at all, so the
/// null the request held is what is reported.
fn null_at(member: &str) -> String {
    one_error(format!(
        "Value null at '{member}' failed to satisfy constraint: Member must not be null"
    ))
}

/// The requirement clause for a `@pattern`, with the pattern as it is REPORTED.
fn must_match(reported_pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {reported_pattern}")
}

/// An `ExtendedS3DestinationConfiguration` with its two required members
/// (`RoleARN`, `BucketARN`) and whatever else a case overrides.
fn extended_s3_configuration() -> Value {
    json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN })
}

/// A `SplunkDestinationConfiguration` with its required members: `HECEndpoint`,
/// `HECEndpointType` ("This type can be either \"Raw\" or \"Event.\"") and
/// `S3Configuration`.
fn splunk_configuration() -> Value {
    json!({
        "HECEndpoint": "https://splunk.example.com:8088",
        "HECEndpointType": "Raw",
        "S3Configuration": s3_configuration(),
    })
}

/// A record whose `Data` holds `bytes` bytes.
fn record_of(bytes: usize) -> Value {
    json!({ "Data": aws_smithy_types::base64::encode(vec![b'x'; bytes]) })
}

/// The modelled maximum of `Data` (`@length max 1024000`).
const MAX_RECORD_BYTES: usize = 1_024_000;

#[tokio::test]
async fn a_constraint_violation_names_the_member_and_the_constraint_but_not_the_value() {
    let emulator = Emulator::new();

    // One row per constrained shape kind the model uses — string length, list length,
    // blob length, integer range on two different members, a nested member two and
    // three levels down, a list element, and an absent required member. Every bound
    // comes from the model
    // (`smithy_query(query="shape", shape="DeliveryStreamName,Data,SizeInMBs,...")`).
    //
    // (case, operation, request, expected message)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a name one character over its 64-character maximum",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "a".repeat(65),
                "S3DestinationConfiguration": s3_configuration(),
            }),
            value_at(
                "deliveryStreamName",
                "Member must have length less than or equal to 64",
            ),
        ),
        (
            "an empty record list",
            "PutRecordBatch",
            json!({ "DeliveryStreamName": "any-stream", "Records": [] }),
            value_at(
                "records",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            "one record over the 500-record maximum",
            "PutRecordBatch",
            json!({
                "DeliveryStreamName": "any-stream",
                "Records": vec![record_of(1); 501],
            }),
            value_at(
                "records",
                "Member must have length less than or equal to 500",
            ),
        ),
        (
            "a record one byte over its maximum",
            "PutRecord",
            json!({
                "DeliveryStreamName": "any-stream",
                "Record": record_of(MAX_RECORD_BYTES + 1),
            }),
            value_at(
                "record.data",
                "Member must have length less than or equal to 1024000",
            ),
        ),
        (
            "the second record of a batch one byte over its maximum",
            "PutRecordBatch",
            json!({
                "DeliveryStreamName": "any-stream",
                "Records": [record_of(1), record_of(MAX_RECORD_BYTES + 1)],
            }),
            value_at(
                "records.2.member.data",
                "Member must have length less than or equal to 1024000",
            ),
        ),
        (
            "a tag value one character over its 256-character maximum",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "tagged-stream",
                "S3DestinationConfiguration": s3_configuration(),
                "Tags": [{ "Key": "owner", "Value": "v".repeat(257) }],
            }),
            value_at(
                "tags.1.member.value",
                "Member must have length less than or equal to 256",
            ),
        ),
        (
            "a buffering size above its maximum",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "buffered-stream",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "BufferingHints": { "SizeInMBs": 129, "IntervalInSeconds": 60 },
                },
            }),
            value_at(
                "extendedS3DestinationConfiguration.bufferingHints.sizeInMBs",
                "Member must have value less than or equal to 128",
            ),
        ),
        (
            "a splunk retry duration below its minimum",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "splunk-stream",
                "SplunkDestinationConfiguration": {
                    "HECEndpoint": "https://splunk.example.com:8088",
                    "HECEndpointType": "Raw",
                    "S3Configuration": s3_configuration(),
                    "RetryOptions": { "DurationInSeconds": -1 },
                },
            }),
            value_at(
                "splunkDestinationConfiguration.retryOptions.durationInSeconds",
                "Member must have value greater than or equal to 0",
            ),
        ),
        (
            "an absent required member",
            "CreateDeliveryStream",
            json!({ "S3DestinationConfiguration": s3_configuration() }),
            null_at("deliveryStreamName"),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, refused) = emulator.call(operation, request).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The value-free form has no `Value '…'` subject at all, so nothing the caller
        // sent — a record payload, a tag value — can reach the body through it.
        assert!(
            !message.contains("Value '"),
            "case {case:?}: expected no rejected value in the message, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_pattern_is_reported_without_the_models_anchoring() {
    let emulator = Emulator::new();

    // (case, operation, request, member, model pattern, reported pattern)
    //
    // The model patterns are `smithy_query(query="shape",
    // shape="DeliveryStreamName,FileExtension,CustomTimeZone")`. A single anchored
    // expression is reported bare; a top-level alternation keeps one pair of anchors
    // around the group, because dropping them branch by branch would widen the language
    // the clause describes.
    let rows: Vec<(&str, &str, Value, &str, &str, &str)> = vec![
        (
            "a stream name with an illegal character, on the control plane",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "bad name!",
                "S3DestinationConfiguration": s3_configuration(),
            }),
            "deliveryStreamName",
            "^[a-zA-Z0-9_.-]+$",
            "[a-zA-Z0-9_.-]+",
        ),
        (
            "the same name rule on the data plane",
            "PutRecord",
            json!({
                "DeliveryStreamName": "bad/name",
                "Record": record_of(1),
            }),
            "deliveryStreamName",
            "^[a-zA-Z0-9_.-]+$",
            "[a-zA-Z0-9_.-]+",
        ),
        (
            "a file extension with no leading dot (an alternation)",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "extension-stream",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "FileExtension": "nodot",
                },
            }),
            "extendedS3DestinationConfiguration.fileExtension",
            "^$|\\.[0-9a-z!\\-_.*'()]+$",
            "^(|\\.[0-9a-z!\\-_.*'()]+)$",
        ),
        (
            // Not a member the issue cited: the same alternation shape, so the same
            // rendering rule has to hold for it.
            "a custom time zone with an illegal character (an alternation)",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "timezone-stream",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "CustomTimeZone": "Berlin+1",
                },
            }),
            "extendedS3DestinationConfiguration.customTimeZone",
            "^$|[a-zA-Z/_]+$",
            "^(|[a-zA-Z/_]+)$",
        ),
    ];

    for (case, operation, request, member, model_pattern, reported) in rows {
        let (status, refused) = emulator.call(operation, request).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        let expected = value_at(member, &must_match(reported));
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(model_pattern),
            "case {case:?}: expected the reported pattern, not the model's \
             {model_pattern:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_range_is_enforced_at_its_published_bounds_and_reported_without_the_value() {
    let emulator = Emulator::new();

    // Each published bound, and one step past each — the ranges are
    // `smithy_query(query="shape", shape="SizeInMBs,IntervalInSeconds,\
    // SplunkRetryDurationInSeconds")`: SizeInMBs 1..128, IntervalInSeconds 0..900,
    // SplunkRetryDurationInSeconds 0..7200.
    //
    // (case, destination member, value, member path, expected requirement or accepted)
    struct Row {
        case: &'static str,
        request: Value,
        /// `None` when the value is inside the range and the create is accepted.
        refused: Option<(&'static str, String)>,
    }

    fn buffering_row(
        case: &'static str,
        stream: &str,
        size: i64,
        interval: i64,
        refused: Option<(&'static str, String)>,
    ) -> Row {
        Row {
            case,
            request: json!({
                "DeliveryStreamName": stream,
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "BufferingHints": { "SizeInMBs": size, "IntervalInSeconds": interval },
                },
            }),
            refused,
        }
    }

    fn splunk_row(
        case: &'static str,
        stream: &str,
        duration: i64,
        refused: Option<(&'static str, String)>,
    ) -> Row {
        let mut splunk = splunk_configuration();
        splunk["RetryOptions"] = json!({ "DurationInSeconds": duration });
        Row {
            case,
            request: json!({
                "DeliveryStreamName": stream,
                "SplunkDestinationConfiguration": splunk,
            }),
            refused,
        }
    }

    const SIZE: &str = "extendedS3DestinationConfiguration.bufferingHints.sizeInMBs";
    const INTERVAL: &str = "extendedS3DestinationConfiguration.bufferingHints.intervalInSeconds";
    const DURATION: &str = "splunkDestinationConfiguration.retryOptions.durationInSeconds";

    let rows = vec![
        buffering_row(
            "a buffering size below its minimum",
            "size-under",
            0,
            60,
            Some((
                SIZE,
                "Member must have value greater than or equal to 1".to_string(),
            )),
        ),
        buffering_row("a buffering size at its minimum", "size-min", 1, 60, None),
        buffering_row("a buffering size at its maximum", "size-max", 128, 60, None),
        buffering_row(
            "a buffering size above its maximum",
            "size-over",
            129,
            60,
            Some((
                SIZE,
                "Member must have value less than or equal to 128".to_string(),
            )),
        ),
        buffering_row(
            "a buffering interval below its minimum",
            "interval-under",
            5,
            -1,
            Some((
                INTERVAL,
                "Member must have value greater than or equal to 0".to_string(),
            )),
        ),
        buffering_row(
            "a buffering interval at its minimum",
            "interval-min",
            5,
            0,
            None,
        ),
        buffering_row(
            "a buffering interval at its maximum",
            "interval-max",
            5,
            900,
            None,
        ),
        buffering_row(
            "a buffering interval above its maximum",
            "interval-over",
            5,
            901,
            Some((
                INTERVAL,
                "Member must have value less than or equal to 900".to_string(),
            )),
        ),
        splunk_row(
            "a splunk retry duration below its minimum",
            "duration-under",
            -1,
            Some((
                DURATION,
                "Member must have value greater than or equal to 0".to_string(),
            )),
        ),
        splunk_row(
            "a splunk retry duration at its minimum",
            "duration-min",
            0,
            None,
        ),
        splunk_row(
            "a splunk retry duration at its maximum",
            "duration-max",
            7200,
            None,
        ),
        splunk_row(
            "a splunk retry duration above its maximum",
            "duration-over",
            7201,
            Some((
                DURATION,
                "Member must have value less than or equal to 7200".to_string(),
            )),
        ),
    ];

    for row in rows {
        let (status, answer) = emulator.call("CreateDeliveryStream", row.request).await;
        match row.refused {
            None => assert_eq!(
                status, 200,
                "case {:?}: a value inside the range is accepted, got {answer:?}",
                row.case
            ),
            Some((member, requirement)) => {
                assert_eq!(status, 400, "case {:?}: got {answer:?}", row.case);
                let message = error_message(&answer);
                let expected = value_at(member, &requirement);
                assert_eq!(
                    message, expected,
                    "case {:?}: expected {expected:?}, got {message:?}",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn a_duplicate_name_is_refused_with_the_name_and_the_account_that_holds_it() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // The name is the caller's, so it varies; the account is the one the emulator
    // presents, whatever the name.
    for name in ["duplicate-one", "duplicate.two", "DUPLICATE_3"] {
        emulator.create_stream(name, None).await;
        let (status, refused) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": name,
                    "S3DestinationConfiguration": s3_configuration(),
                }),
            )
            .await;
        assert_eq!(status, 400, "case {name:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {name:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        let expected = format!("Firehose {name} under accountId {account} already exists");
        assert_eq!(
            message, expected,
            "case {name:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_lookup_that_resolved_nothing_names_the_account_and_never_the_probed_name() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    // The control plane states the account the lookup was scoped to; the data plane
    // states the same fact in its own wording. Neither echoes the name it was given.
    let control_plane = format!("Firehose under account {account} not found.");
    let data_plane = format!("Firehose not found under account {account}.");

    // One stream exists, so the refusals below are a failed lookup and not an empty
    // register.
    emulator.create_stream("present-stream", None).await;

    /// One operation that resolves a stream by name, and the sentence it states when
    /// the lookup finds none.
    struct Lookup {
        case: &'static str,
        operation: &'static str,
        /// Builds the request for a probed name.
        request: fn(&str) -> Value,
        expected: String,
    }

    let operations = vec![
        Lookup {
            case: "describe",
            operation: "DescribeDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name }),
            expected: control_plane.clone(),
        },
        Lookup {
            case: "delete",
            operation: "DeleteDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name }),
            expected: control_plane.clone(),
        },
        Lookup {
            case: "list tags",
            operation: "ListTagsForDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name }),
            expected: control_plane.clone(),
        },
        Lookup {
            case: "put a record",
            operation: "PutRecord",
            request: |name| json!({ "DeliveryStreamName": name, "Record": { "Data": "eA==" } }),
            expected: data_plane.clone(),
        },
        Lookup {
            case: "put a batch",
            operation: "PutRecordBatch",
            request: |name| json!({ "DeliveryStreamName": name, "Records": [{ "Data": "eA==" }] }),
            expected: data_plane.clone(),
        },
    ];

    // The probed names differ in every way a name can: the message must not. (Each is
    // chosen not to occur incidentally inside the expected sentence, so the
    // "never echoed" assertion below means what it says.)
    for probed in ["ghost", "q", "present-stream-2", &"z".repeat(64)] {
        for lookup in &operations {
            let case = lookup.case;
            let expected = &lookup.expected;
            let (status, refused) = emulator
                .call(lookup.operation, (lookup.request)(probed))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} of {probed:?}: got {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?} of {probed:?}: got {refused:?}"
            );
            let message = error_message(&refused);
            assert_eq!(
                &message, expected,
                "case {case:?} of {probed:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(probed),
                "case {case:?} of {probed:?}: the probed name must not be echoed, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_the_state_blocks_names_the_stream_the_account_and_that_state() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // A create answers with the stream in `CREATING`, and "you can't delete a Firehose
    // stream that is in the CREATING state" — the state that blocks the delete is what
    // the message states, whatever it is.
    for name in ["creating-one", "creating.two"] {
        emulator.create_stream(name, None).await;
        let (status, refused) = emulator
            .call(
                "DeleteDeliveryStream",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        assert_eq!(status, 400, "case {name:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {name:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        let expected =
            format!("Firehose {name} under account {account} cannot be deleted in CREATING state");
        assert_eq!(
            message, expected,
            "case {name:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn destinations_that_exclude_each_other_are_refused_with_one_sentence_over_them() {
    let emulator = Emulator::new();

    // The model declares each destination as its own optional member with no union over
    // them, in this order: S3DestinationConfiguration, ExtendedS3DestinationConfiguration,
    // …, SplunkDestinationConfiguration (`smithy_query(query="operation",
    // operation="CreateDeliveryStream")`). The sentence names the members that were
    // supplied, in THAT order — never the order the request happened to list them in.
    //
    // (case, request, expected alternatives)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "the deprecated S3 member beside its replacement",
            json!({
                "DeliveryStreamName": "two-s3",
                "S3DestinationConfiguration": s3_configuration(),
                "ExtendedS3DestinationConfiguration": extended_s3_configuration(),
            }),
            "S3DestinationConfiguration or ExtendedS3DestinationConfiguration",
        ),
        (
            "two unrelated destinations",
            json!({
                "DeliveryStreamName": "s3-and-splunk",
                "S3DestinationConfiguration": s3_configuration(),
                "SplunkDestinationConfiguration": splunk_configuration(),
            }),
            "S3DestinationConfiguration or SplunkDestinationConfiguration",
        ),
        (
            "a later member beside an earlier one",
            json!({
                "DeliveryStreamName": "extended-and-splunk",
                "SplunkDestinationConfiguration": splunk_configuration(),
                "ExtendedS3DestinationConfiguration": extended_s3_configuration(),
            }),
            "ExtendedS3DestinationConfiguration or SplunkDestinationConfiguration",
        ),
        (
            "three destinations at once",
            json!({
                "DeliveryStreamName": "three-destinations",
                "SplunkDestinationConfiguration": splunk_configuration(),
                "ExtendedS3DestinationConfiguration": extended_s3_configuration(),
                "S3DestinationConfiguration": s3_configuration(),
            }),
            "S3DestinationConfiguration, ExtendedS3DestinationConfiguration or \
             SplunkDestinationConfiguration",
        ),
    ];

    for (case, request, alternatives) in rows {
        let (status, refused) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        let expected = format!("Exactly one of {alternatives} must be specified.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Nothing was stored for any refused create.
    let (_, listed) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(
        listed["DeliveryStreamNames"],
        json!([]),
        "a refused create stores nothing, got {listed:?}"
    );
}
