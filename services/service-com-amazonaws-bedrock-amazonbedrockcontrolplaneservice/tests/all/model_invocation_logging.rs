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
//! A configuration is only ADMITTED when every role it asks Amazon Bedrock to take on is
//! one this account may hand it. The CloudWatch Logs destination is the one destination
//! that names such a role (`CloudWatchConfig.roleArn` is `@required`, while `S3Config`
//! declares no role member at all), and one half of the decision is the ACCOUNT that role
//! ARN names: `iam:PassRole` is granted by the account that HOLDS the role, so a role of
//! the CALLING account is installable — the put answers 200 and the get reports the
//! destination back — while a role whose embedded 12-digit account is another one is
//! refused with HTTP 403 and the modelled `AccessDeniedException` carrying the recorded
//! sentence, before anything is stored, so the record in force is left exactly as it was
//! and no part of the refused configuration is readable afterwards.
//!
//! The pass is refused only on what this account's IAM ANSWERED about the role, and this
//! suite mounts the one service, so there is no IAM here to answer: these rows pin the
//! grounding half of the decision. The half IAM answers — whether the trust policy the role
//! carries admits the Amazon Bedrock service principal the `sts:AssumeRole` such a hand-off
//! is — is pinned in
//! `core/router/tests/all/bedrock_model_invocation_logging_pass_role.rs`, where this
//! service and IAM are mounted over one engine.
//!
//! # What the tables below generalise over
//!
//! [`cases`] holds one row per independent ADMITTED configuration, along the axes
//! `LoggingConfig` spans: whether it names the S3 destination, the CloudWatch Logs
//! destination, both, or no destination at all, whether a destination carries a
//! `keyPrefix` or a `largeDataDeliveryS3Config`, the partition and the account field of an
//! admitted `roleArn` (including the EMPTY account its published pattern admits, and a
//! role under a path), and which of the five `*DataDeliveryEnabled` flags are set — all
//! true, all false, mixed, and omitted entirely. Sizes sit on the published bounds of each
//! member's `@length` (`bucketName` 3..63, `keyPrefix` ..1024, `logGroupName` 1..512,
//! `roleArn` ..2048, taken from this service's Smithy model).
//!
//! [`cross_account_role_cases`] holds one row per independent configuration whose
//! CloudWatch destination names a role of ANOTHER account, along the axes the refusal must
//! hold across: the partition, which other account the ARN carries (including one differing
//! from this account in a single digit), the role's path and its published `@length` max
//! 2048, `logGroupName` at both ends of its published `@length` 1..512, whether the
//! destination carries a `largeDataDeliveryS3Config`, whether a role-free `s3Config` is
//! named beside it, and which flags are set.
//!
//! [`a_value_past_every_bound_is_refused_and_changes_nothing`] drives one value PAST each
//! published bound plus a `roleArn` outside the member's published pattern: those are
//! refused for their form (400 `ValidationException`) ahead of the pass-role decision.
//!
//! The rows are driven in sequence against ONE emulator, so each get also witnesses
//! that the put before it replaced the previous row rather than merging into it.

use serde_json::{json, Value};

use crate::support::{emulator, Emulator, Reply, ACCOUNT};

/// A role ARN of the CALLING account, inside `RoleArn`'s published pattern
/// (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`, from this service's Smithy model) —
/// the pass this account may make.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/BedrockInvocationLogging")
}

/// A 12-digit account id that is NOT the calling account, so a role ARN carrying it names
/// a role no principal of this account can pass.
const OTHER_ACCOUNT: &str = "123456789012";

/// The sentence Amazon Bedrock refuses a role it may not be passed with, spelled as the
/// rest of this tree records AWS spelling the same denial.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

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

/// The configurations the rule must round-trip, one per independent shape of a role-free
/// `LoggingConfig`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "s3 only, no flags",
            logging_config: json!({
                "s3Config": { "bucketName": "bedrock-invocation-logs" },
            }),
        },
        Case {
            name: "s3 with a key prefix, every flag true",
            logging_config: json!({
                "s3Config": {
                    "bucketName": "bedrock-invocation-logs",
                    "keyPrefix": "prod/bedrock/",
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
            name: "s3 only, flags mixed and some omitted",
            logging_config: json!({
                "s3Config": {
                    "bucketName": "bedrock-invocation-logs",
                    "keyPrefix": "mixed/",
                },
                "textDataDeliveryEnabled": true,
                "imageDataDeliveryEnabled": false,
                "embeddingDataDeliveryEnabled": true,
            }),
        },
        Case {
            name: "members at their shortest published length",
            logging_config: json!({
                // `bucketName` @length min 3.
                "s3Config": { "bucketName": "abc" },
            }),
        },
        Case {
            name: "members at their longest published length",
            logging_config: json!({
                "s3Config": {
                    // `bucketName` @length max 63, `keyPrefix` @length max 1024.
                    "bucketName": repeat('b', 63),
                    "keyPrefix": repeat('k', 1024),
                },
            }),
        },
        Case {
            name: "the recorded shape: a same-account cloudwatch role with the text flag on",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": role_arn(),
                },
                "textDataDeliveryEnabled": true,
                "imageDataDeliveryEnabled": false,
                "embeddingDataDeliveryEnabled": false,
            }),
        },
        Case {
            name: "a same-account cloudwatch role, no flags",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": role_arn(),
                },
            }),
        },
        Case {
            name: "cloudwatch with large-data s3 overflow beside an s3 destination",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": role_arn(),
                    "largeDataDeliveryS3Config": {
                        "bucketName": "bedrock-large-data",
                        "keyPrefix": "overflow/",
                    },
                },
                "s3Config": {
                    "bucketName": "bedrock-invocation-logs",
                    "keyPrefix": "prod/bedrock/",
                },
                "textDataDeliveryEnabled": true,
                "videoDataDeliveryEnabled": true,
                "audioDataDeliveryEnabled": true,
            }),
        },
        Case {
            name: "a role ARN with the EMPTY account its pattern admits, which names no \
                   other account",
            logging_config: json!({
                "cloudWatchConfig": {
                    // `logGroupName` @length min 1.
                    "logGroupName": "g",
                    "roleArn": "arn:aws:iam:::role/r",
                    "largeDataDeliveryS3Config": { "bucketName": "abc" },
                },
            }),
        },
        Case {
            name: "a same-account role under a path, in another partition, logGroupName at \
                   its longest length",
            logging_config: json!({
                "cloudWatchConfig": {
                    // `logGroupName` @length max 512.
                    "logGroupName": repeat('g', 512),
                    "roleArn": format!("arn:aws-cn:iam::{ACCOUNT}:role/service-role/delivery"),
                },
                "embeddingDataDeliveryEnabled": true,
            }),
        },
        Case {
            name: "a same-account role ARN at its longest published length",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "logs/bedrock",
                    // `roleArn` @length max 2048, counting the 31 characters of
                    // `arn:aws:iam::000000000000:role/`.
                    "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/{}", repeat('r', 2017)),
                },
            }),
        },
        Case {
            name: "no destination and no flags at all",
            logging_config: json!({}),
        },
    ]
}

/// The configurations whose CloudWatch destination names a role of ANOTHER account, which
/// no principal of the calling account can pass to Amazon Bedrock.
fn cross_account_role_cases() -> Vec<Case> {
    vec![
        Case {
            name: "another account's role, no flags",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/logging"),
                },
            }),
        },
        Case {
            name: "another account's role, with the text flag on",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/logging"),
                },
                "textDataDeliveryEnabled": true,
                "imageDataDeliveryEnabled": false,
                "embeddingDataDeliveryEnabled": false,
            }),
        },
        Case {
            name: "another account's role with large-data s3 overflow",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "aws/bedrock/modelinvocations",
                    "roleArn": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/service-role/logging"),
                    "largeDataDeliveryS3Config": {
                        "bucketName": "bedrock-large-data",
                        "keyPrefix": "overflow/",
                    },
                },
                "textDataDeliveryEnabled": true,
                "videoDataDeliveryEnabled": true,
                "audioDataDeliveryEnabled": true,
            }),
        },
        Case {
            name: "both destinations: the role-free one does not rescue the other",
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
            }),
        },
        Case {
            name: "an account differing from the caller's in a single digit",
            logging_config: json!({
                "cloudWatchConfig": {
                    // `logGroupName` @length min 1.
                    "logGroupName": "g",
                    "roleArn": "arn:aws:iam::000000000001:role/r",
                    "largeDataDeliveryS3Config": { "bucketName": "abc" },
                },
            }),
        },
        Case {
            name: "another account in another partition, logGroupName at its longest length",
            logging_config: json!({
                "cloudWatchConfig": {
                    // `logGroupName` @length max 512.
                    "logGroupName": repeat('g', 512),
                    "roleArn": format!("arn:aws-cn:iam::987654321098:role/{}", repeat('r', 64)),
                },
            }),
        },
        Case {
            name: "another account's role ARN at its longest published length",
            logging_config: json!({
                "cloudWatchConfig": {
                    "logGroupName": "logs/bedrock",
                    // `roleArn` @length max 2048, counting the 31 characters of
                    // `arn:aws:iam::123456789012:role/`.
                    "roleArn": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/{}", repeat('r', 2017)),
                },
            }),
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
async fn a_role_of_another_account_is_refused_and_installs_nothing() {
    let emulator = emulator();

    // With nothing configured at all first: the refusal must not install the
    // configuration it refused, so the account is still unconfigured afterwards.
    for case in cross_account_role_cases() {
        let reply = put(&emulator, &case.logging_config).await;
        assert_eq!(
            reply.status, 403,
            "case {} (unconfigured account): a role of another account may not be passed, so \
             the put is refused 403, got {} {:?}",
            case.name, reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("AccessDeniedException"),
            "case {} (unconfigured account): the refusal is an AccessDeniedException, got {:?}",
            case.name,
            reply.error
        );
        assert_eq!(
            reply.get("message"),
            json!(PASS_ROLE_REFUSAL),
            "case {} (unconfigured account): expected the pass-role refusal \
             {PASS_ROLE_REFUSAL:?}, got {:?}",
            case.name,
            reply.get("message")
        );
        let after = get(&emulator).await;
        assert_eq!(
            after.get("loggingConfig"),
            Value::Null,
            "case {} (unconfigured account): a refused put installs nothing, got {:?}",
            case.name,
            after.get("loggingConfig")
        );
    }

    // And against a record that IS in force: the refused put must leave it untouched,
    // with no part of the configuration it carried merged in.
    let stored = json!({
        "s3Config": { "bucketName": "bedrock-invocation-logs", "keyPrefix": "prod/" },
        "textDataDeliveryEnabled": true,
    });
    let baseline = put(&emulator, &stored).await;
    assert_eq!(
        baseline.status, 200,
        "the baseline put must answer 200, got {} {:?}",
        baseline.status, baseline.body
    );
    for case in cross_account_role_cases() {
        let reply = put(&emulator, &case.logging_config).await;
        assert_eq!(
            reply.status, 403,
            "case {} (configured account): a role of another account may not be passed, so \
             the put is refused 403, got {} {:?}",
            case.name, reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("AccessDeniedException"),
            "case {} (configured account): the refusal is an AccessDeniedException, got {:?}",
            case.name,
            reply.error
        );
        let after = get(&emulator).await;
        assert_eq!(
            after.get("loggingConfig"),
            stored,
            "case {}: the configuration in force must be unchanged by a refused put, got {:?}",
            case.name,
            after.get("loggingConfig")
        );
    }
}

#[tokio::test]
async fn only_the_role_arns_account_decides_a_cloudwatch_destination() {
    // Two puts of the SAME configuration differing in NOTHING but the account the
    // `roleArn` carries: the caller's own account is installable, every other account is
    // refused, so the two cases are distinguishable. The role name, the partition and the
    // log group are held fixed across the pair, which is what makes the account the only
    // thing the decision can be reading.
    let accounts: Vec<(&str, String, bool)> = vec![
        ("the caller's own account", ACCOUNT.to_string(), true),
        ("the empty account the pattern admits", String::new(), true),
        ("another account", OTHER_ACCOUNT.to_string(), false),
        (
            "an account differing in a single digit",
            "000000000001".to_string(),
            false,
        ),
        (
            "an account differing in its first digit",
            "100000000000".to_string(),
            false,
        ),
    ];

    for (case, account, installable) in accounts {
        // A fresh emulator per row, so each decision is read against an empty store.
        let emulator = emulator();
        let logging_config = json!({
            "cloudWatchConfig": {
                "logGroupName": "aws/bedrock/modelinvocations",
                "roleArn": format!("arn:aws:iam::{account}:role/BedrockInvocationLogging"),
            },
            "textDataDeliveryEnabled": true,
        });
        let reply = put(&emulator, &logging_config).await;
        let after = get(&emulator).await;
        if installable {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a role of the calling account may be passed, so the put \
                 answers 200, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                after.get("loggingConfig"),
                logging_config,
                "case {case:?}: the installed configuration must be reported back, got {:?}",
                after.get("loggingConfig")
            );
        } else {
            assert_eq!(
                reply.status, 403,
                "case {case:?}: a role of another account may not be passed, so the put is \
                 refused 403, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("AccessDeniedException"),
                "case {case:?}: the refusal is an AccessDeniedException, got {:?}",
                reply.error
            );
            assert_eq!(
                reply.get("message"),
                json!(PASS_ROLE_REFUSAL),
                "case {case:?}: expected the pass-role refusal {PASS_ROLE_REFUSAL:?}, got {:?}",
                reply.get("message")
            );
            assert_eq!(
                after.get("loggingConfig"),
                Value::Null,
                "case {case:?}: a refused put installs nothing, got {:?}",
                after.get("loggingConfig")
            );
        }
    }
}

#[tokio::test]
async fn a_value_past_every_bound_is_refused_and_changes_nothing() {
    // Each row drives ONE member past the bound its Smithy model declares, or outside
    // the pattern it declares; the bounds themselves are exercised in `cases()` and
    // `cross_account_role_cases()`. A value outside a member's published form is refused
    // for that form — 400 `ValidationException` — ahead of any pass-role decision, so the
    // cloudwatch rows here are refused 400 rather than 403.
    let refused: Vec<(&str, Value)> = vec![
        (
            "logGroupName one past @length max 512",
            json!({
                "cloudWatchConfig": {
                    "logGroupName": repeat('g', 513),
                    "roleArn": role_arn(),
                },
            }),
        ),
        (
            "logGroupName one under @length min 1",
            json!({
                "cloudWatchConfig": { "logGroupName": "", "roleArn": role_arn() },
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
