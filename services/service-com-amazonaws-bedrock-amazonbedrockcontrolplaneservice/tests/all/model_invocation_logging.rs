// SPDX-License-Identifier: CC0-1.0

//! Model invocation logging configuration end to end: `Put`, `Get` and `Delete` over
//! HTTP, through the same `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! The model invocation logging configuration is one account-level SETTINGS record, so
//! `PutModelInvocationLoggingConfiguration` REPLACES it whole with the `loggingConfig`
//! the request carries, `GetModelInvocationLoggingConfiguration` reports back exactly
//! the record the last admitted put stored — member for member, nothing merged in from
//! an earlier put and nothing defaulted in — and while no record is stored (none was
//! ever put, or `DeleteModelInvocationLoggingConfiguration` removed it) the get answers
//! 200 with no `loggingConfig` member at all.
//!
//! # What the table below generalises over
//!
//! One row per independent configuration along the axes `LoggingConfig` spans: which
//! destinations it names (CloudWatch only, S3 only, both, neither), whether the
//! CloudWatch destination carries a `largeDataDeliveryS3Config` and whether that config
//! carries a `keyPrefix`, and which of the five `*DataDeliveryEnabled` flags are set —
//! all true, all false, mixed, and omitted entirely. Sizes sit on the published bounds
//! of each member's `@length` (`logGroupName` 1..512, `bucketName` 3..63, `keyPrefix`
//! ..1024, taken from this service's Smithy model), and
//! [`a_value_past_every_bound_is_refused_and_changes_nothing`] drives one value PAST
//! each of those bounds plus a `roleArn` outside the member's published pattern.
//!
//! The rows are driven in sequence against ONE emulator, so each get also witnesses
//! that the put before it replaced the previous row rather than merging into it.

use serde_json::{json, Value};

use crate::support::{emulator, Emulator, Reply};

/// A role ARN inside `RoleArn`'s published pattern
/// (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`, from this service's Smithy model).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/BedrockInvocationLogging";

/// The URI `GetModelInvocationLoggingConfiguration`,
/// `PutModelInvocationLoggingConfiguration` and
/// `DeleteModelInvocationLoggingConfiguration` are all bound to.
const LOGGING_URI: &str = "/logging/modelinvocations";

/// One configuration this suite puts and reads back.
struct Case {
    name: &'static str,
    logging_config: Value,
}

/// `n` repetitions of `c`, for a member driven at the bound of its `@length`.
fn repeat(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}

/// The configurations the rule must round-trip, one per independent shape of
/// `LoggingConfig`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "cloudwatch only, no flags",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": ROLE_ARN,
                },
            }),
        },
        Case {
            name: "cloudwatch with large-data s3 overflow and a key prefix",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": ROLE_ARN,
                    "largeDataDeliveryS3Config": {
                        "bucketName": "bedrock-large-data",
                        "keyPrefix": "overflow/",
                    },
                },
                "textDataDeliveryEnabled": true,
                "imageDataDeliveryEnabled": true,
                "embeddingDataDeliveryEnabled": true,
                "videoDataDeliveryEnabled": true,
                "audioDataDeliveryEnabled": true,
            }),
        },
        Case {
            name: "s3 only, every flag false",
            logging_config: json!({
                "s3Config": {
                    "bucketName": "bedrock-invocation-logs",
                },
                "textDataDeliveryEnabled": false,
                "imageDataDeliveryEnabled": false,
                "embeddingDataDeliveryEnabled": false,
                "videoDataDeliveryEnabled": false,
                "audioDataDeliveryEnabled": false,
            }),
        },
        Case {
            name: "both destinations, flags mixed",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "logs/bedrock",
                    "roleArn": "arn:aws-us-gov:iam::210987654321:role/logging",
                },
                "s3Config": {
                    "bucketName": "bedrock-invocation-logs",
                    "keyPrefix": "prod/bedrock/",
                },
                "textDataDeliveryEnabled": true,
                "imageDataDeliveryEnabled": false,
                "embeddingDataDeliveryEnabled": true,
                "audioDataDeliveryEnabled": false,
            }),
        },
        Case {
            name: "members at their shortest published length",
            logging_config: json!({
                "cloudWatchConfig": {
                    // `logGroupName` @length min 1, `bucketName` @length min 3.
                    "logGroupName": "g",
                    // The published pattern admits an EMPTY account id.
                    "roleArn": "arn:aws:iam:::role/r",
                    "largeDataDeliveryS3Config": { "bucketName": "abc" },
                },
            }),
        },
        Case {
            name: "members at their longest published length",
            logging_config: json!({
                "cloudWatchConfig": {
                    // `logGroupName` @length max 512.
                    "logGroupName": repeat('g', 512),
                    "roleArn": ROLE_ARN,
                },
                "s3Config": {
                    // `bucketName` @length max 63, `keyPrefix` @length max 1024.
                    "bucketName": repeat('b', 63),
                    "keyPrefix": repeat('k', 1024),
                },
            }),
        },
        Case {
            name: "no destination and no flags at all",
            logging_config: json!({}),
        },
    ]
}

/// `PUT /logging/modelinvocations` with `loggingConfig`.
async fn put(emulator: &Emulator, logging_config: &Value) -> Reply {
    emulator
        .call(
            "PUT",
            LOGGING_URI,
            Some(json!({ "loggingConfig": logging_config })),
        )
        .await
}

/// `GET /logging/modelinvocations`.
async fn get(emulator: &Emulator) -> Reply {
    emulator.call("GET", LOGGING_URI, None).await
}

/// `DELETE /logging/modelinvocations`.
async fn delete(emulator: &Emulator) -> Reply {
    emulator.call("DELETE", LOGGING_URI, None).await
}

#[tokio::test]
async fn a_get_before_any_put_reports_no_configuration() {
    let emulator = emulator();
    let reply = get(&emulator).await;
    assert_eq!(
        reply.status, 200,
        "case unconfigured: GetModelInvocationLoggingConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.get("loggingConfig"),
        Value::Null,
        "case unconfigured: nothing was ever put, so no loggingConfig may be reported, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn every_configuration_is_reported_back_exactly_as_it_was_put() {
    let emulator = emulator();
    for case in cases() {
        let put_reply = put(&emulator, &case.logging_config).await;
        assert_eq!(
            put_reply.status, 200,
            "case {}: PutModelInvocationLoggingConfiguration must answer 200, got {} {:?}",
            case.name, put_reply.status, put_reply.body
        );

        let reply = get(&emulator).await;
        assert_eq!(
            reply.status, 200,
            "case {}: GetModelInvocationLoggingConfiguration must answer 200, got {} {:?}",
            case.name, reply.status, reply.body
        );
        // The whole record, compared as one value: a member the row omitted must be
        // absent here too, whether an earlier row set it or not.
        assert_eq!(
            reply.get("loggingConfig"),
            case.logging_config,
            "case {}: the stored configuration must be reported member for member, got {:?}",
            case.name,
            reply.get("loggingConfig")
        );

        // Reading twice changes nothing: this operation is @readonly.
        let again = get(&emulator).await;
        assert_eq!(
            again.get("loggingConfig"),
            case.logging_config,
            "case {}: a second read must report the same configuration, got {:?}",
            case.name,
            again.get("loggingConfig")
        );
    }
}

#[tokio::test]
async fn putting_the_same_configuration_twice_leaves_the_same_configuration() {
    let emulator = emulator();
    for case in cases() {
        for attempt in 1..=2 {
            let put_reply = put(&emulator, &case.logging_config).await;
            assert_eq!(
                put_reply.status, 200,
                "case {} attempt {attempt}: put must answer 200, got {} {:?}",
                case.name, put_reply.status, put_reply.body
            );
        }
        let reply = get(&emulator).await;
        assert_eq!(
            reply.get("loggingConfig"),
            case.logging_config,
            "case {}: an @idempotent put applied twice must leave one configuration, got {:?}",
            case.name,
            reply.get("loggingConfig")
        );
    }
}

#[tokio::test]
async fn a_delete_leaves_no_configuration_and_repeats_cleanly() {
    let emulator = emulator();
    for case in cases() {
        let put_reply = put(&emulator, &case.logging_config).await;
        assert_eq!(
            put_reply.status, 200,
            "case {}: put must answer 200, got {} {:?}",
            case.name, put_reply.status, put_reply.body
        );

        // Deleting the stored record, and deleting again with nothing stored, are the
        // same call: the model declares no not-found error for it.
        for attempt in 1..=2 {
            let deleted = delete(&emulator).await;
            assert_eq!(
                deleted.status, 200,
                "case {} delete {attempt}: DeleteModelInvocationLoggingConfiguration must \
                 answer 200, got {} {:?}",
                case.name, deleted.status, deleted.body
            );
            let reply = get(&emulator).await;
            assert_eq!(
                reply.status, 200,
                "case {} delete {attempt}: the get after a delete must answer 200, got {} {:?}",
                case.name, reply.status, reply.body
            );
            assert_eq!(
                reply.get("loggingConfig"),
                Value::Null,
                "case {} delete {attempt}: nothing is configured after a delete, got {:?}",
                case.name,
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_value_past_every_bound_is_refused_and_changes_nothing() {
    // Each row drives ONE member past the bound its Smithy model declares, or outside
    // the pattern it declares; the bounds themselves are exercised in `cases()`.
    let refused: Vec<(&str, Value)> = vec![
        (
            "logGroupName one past @length max 512",
            json!({
                "cloudWatchConfig": {
                    "logGroupName": repeat('g', 513),
                    "roleArn": ROLE_ARN,
                },
            }),
        ),
        (
            "logGroupName one under @length min 1",
            json!({
                "cloudWatchConfig": { "logGroupName": "", "roleArn": ROLE_ARN },
            }),
        ),
        (
            "bucketName one under @length min 3",
            json!({ "s3Config": { "bucketName": "ab" } }),
        ),
        (
            "bucketName one past @length max 63",
            json!({ "s3Config": { "bucketName": repeat('b', 64) } }),
        ),
        (
            "keyPrefix one past @length max 1024",
            json!({
                "s3Config": { "bucketName": "bedrock-logs", "keyPrefix": repeat('k', 1025) },
            }),
        ),
        (
            "roleArn outside the published role-ARN pattern",
            json!({
                "cloudWatchConfig": {
                    "logGroupName": "logs/bedrock",
                    "roleArn": "arn:aws:iam::000000000000:user/not-a-role",
                },
            }),
        ),
        (
            "cloudWatchConfig without the required roleArn",
            json!({ "cloudWatchConfig": { "logGroupName": "logs/bedrock" } }),
        ),
        (
            "s3Config without the required bucketName",
            json!({ "s3Config": { "keyPrefix": "prod/" } }),
        ),
    ];

    let emulator = emulator();
    // A configuration that IS admitted, so each refusal is checked against a known
    // stored record rather than against an empty store.
    let stored = json!({
        "s3Config": { "bucketName": "bedrock-invocation-logs", "keyPrefix": "prod/" },
        "textDataDeliveryEnabled": true,
    });
    let put_reply = put(&emulator, &stored).await;
    assert_eq!(
        put_reply.status, 200,
        "the baseline put must answer 200, got {} {:?}",
        put_reply.status, put_reply.body
    );

    for (case, logging_config) in refused {
        let reply = put(&emulator, &logging_config).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: the put must be refused 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case}: a constraint violation is a ValidationException, got {:?}",
            reply.error
        );
        let after = get(&emulator).await;
        assert_eq!(
            after.get("loggingConfig"),
            stored,
            "case {case}: a refused put must store nothing, got {:?}",
            after.get("loggingConfig")
        );
    }

    // The whole member is @required: a request carrying no loggingConfig is refused the
    // same way, and stores nothing either.
    let missing = emulator.call("PUT", LOGGING_URI, Some(json!({}))).await;
    assert_eq!(
        missing.status, 400,
        "case missing loggingConfig: the put must be refused 400, got {} {:?}",
        missing.status, missing.body
    );
    let after = get(&emulator).await;
    assert_eq!(
        after.get("loggingConfig"),
        stored,
        "case missing loggingConfig: a refused put must store nothing, got {:?}",
        after.get("loggingConfig")
    );
}
