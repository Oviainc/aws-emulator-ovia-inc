// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a firewall's logging configuration: the destinations
//! `UpdateLoggingConfiguration` accepts, the ones it refuses because their owner
//! does not admit the log-delivery attachment, and what
//! `DescribeLoggingConfiguration` then reports.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value. The buckets a row
//! names are created in the emulated Amazon S3 mounted beside this service, and
//! their policies are put there through Amazon S3's own API — so what gates the
//! call is the destination owner's answer and not a fixture of this suite.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{error_code, error_message, string_at, Harness, ACCOUNT, REGION};

/// The ceiling this emulator retains log destinations up to, which the refusal
/// below states; the sentence is the service's own, so the test spells it out
/// rather than asking the code under test what it says.
const DESTINATION_CEILING: usize = 16;

/// The two permissions attaching log delivery to a bucket needs, which the AWS
/// documentation for a Network Firewall S3 logging destination names: Network
/// Firewall reads the bucket policy and writes the log-delivery statement into
/// it on the caller's behalf.
const ATTACH_ACTIONS: [&str; 2] = ["s3:GetBucketPolicy", "s3:PutBucketPolicy"];

/// A bucket-policy document with one statement of `effect` over `actions`, for
/// the principal `principal` on the bucket `bucket`.
fn bucket_policy(effect: &str, principal: &str, actions: &[&str], bucket: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "NetworkFirewallLoggingSuite",
            "Effect": effect,
            "Principal": { "AWS": principal },
            "Action": actions,
            "Resource": format!("arn:aws:s3:::{bucket}"),
        }],
    })
    .to_string()
}

/// Create `bucket` in [`REGION`] through Amazon S3's own `CreateBucket`, and put
/// `policy` on it when the row gives one.
///
/// A regional endpoint requires the create to state its own location constraint,
/// so every bucket here names the Region it is created in.
async fn create_bucket(harness: &Harness, bucket: &str, policy: Option<String>) {
    let s3 = harness.s3();
    s3.create_bucket()
        .bucket(bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("the emulated Amazon S3 creates the bucket {bucket}: {error}")
        });
    if let Some(policy) = policy {
        s3.put_bucket_policy()
            .bucket(bucket)
            .policy(policy)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("the emulated Amazon S3 takes the bucket policy for {bucket}: {error}")
            });
    }
}

/// Create the firewall `name` with one subnet mapping, and answer its ARN.
async fn create_firewall(harness: &Harness, name: &str, policy_arn: &str) -> String {
    let created = harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-0f1" }],
            }),
        )
        .await;
    string_at(&created, &["Firewall", "FirewallArn"]).to_string()
}

/// One `LogDestinationConfig` for an Amazon S3 bucket.
fn s3_destination(log_type: &str, bucket: &str) -> Value {
    json!({
        "LogType": log_type,
        "LogDestinationType": "S3",
        "LogDestination": { "bucketName": bucket },
    })
}

/// The destinations `DescribeLoggingConfiguration` reports for `firewall_name`.
async fn described_destinations(harness: &Harness, firewall_name: &str) -> Vec<Value> {
    let described = harness
        .ok(
            "DescribeLoggingConfiguration",
            &json!({ "FirewallName": firewall_name }),
        )
        .await;
    described["LoggingConfiguration"]["LogDestinationConfigs"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("expected LoggingConfiguration.LogDestinationConfigs in {described:?}")
        })
        .clone()
}

/// One row of the permission table: a bucket, the policy it carries, and whether
/// log delivery may be attached to it.
#[derive(Debug)]
struct PermissionCase {
    /// The row's label, which is also the bucket's and the firewall's name part.
    label: &'static str,
    /// The policy the bucket carries, or `None` when it carries none.
    policy: Option<String>,
    /// Whether `UpdateLoggingConfiguration` must accept the destination.
    accepted: bool,
}

/// The rows: a bucket that denies the caller the attachment outright, each of the
/// two permissions denied on its own, a deny aimed at another principal, a deny
/// over an unrelated action, a wildcard deny, an explicit grant, and a bucket
/// with no policy at all.
fn permission_cases() -> Vec<PermissionCase> {
    vec![
        PermissionCase {
            label: "no-policy",
            policy: None,
            accepted: true,
        },
        PermissionCase {
            label: "granted",
            policy: Some(bucket_policy("Allow", ACCOUNT, &ATTACH_ACTIONS, "granted")),
            accepted: true,
        },
        PermissionCase {
            label: "denied-both",
            policy: Some(bucket_policy(
                "Deny",
                ACCOUNT,
                &ATTACH_ACTIONS,
                "denied-both",
            )),
            accepted: false,
        },
        PermissionCase {
            label: "denied-read",
            policy: Some(bucket_policy(
                "Deny",
                ACCOUNT,
                &["s3:GetBucketPolicy"],
                "denied-read",
            )),
            accepted: false,
        },
        PermissionCase {
            label: "denied-write",
            policy: Some(bucket_policy(
                "Deny",
                ACCOUNT,
                &["s3:PutBucketPolicy"],
                "denied-write",
            )),
            accepted: false,
        },
        PermissionCase {
            label: "denied-by-wildcard",
            policy: Some(bucket_policy(
                "Deny",
                ACCOUNT,
                &["s3:*"],
                "denied-by-wildcard",
            )),
            accepted: false,
        },
        PermissionCase {
            label: "denied-for-another-account",
            policy: Some(bucket_policy(
                "Deny",
                "210987654321",
                &ATTACH_ACTIONS,
                "denied-for-another-account",
            )),
            accepted: true,
        },
        PermissionCase {
            label: "denied-unrelated-action",
            policy: Some(bucket_policy(
                "Deny",
                ACCOUNT,
                &["s3:DeleteBucket"],
                "denied-unrelated-action",
            )),
            accepted: true,
        },
    ]
}

/// RULE: Network Firewall attaches log delivery to the destination the caller
/// names, so a configuration is accepted only while that destination's OWN
/// resource policy does not deny the caller the permissions the attachment needs
/// — for an Amazon S3 bucket, reading and writing that bucket's policy. A
/// destination that denies them is refused with HTTP 400 /
/// `LogDestinationPermissionException` and the firewall's logging configuration
/// is left exactly as it was.
#[tokio::test]
async fn a_destination_that_denies_the_attachment_is_refused() {
    let harness = Harness::with_s3();
    let policy_arn = harness.shared_policy("logging-policy").await;

    for case in permission_cases() {
        create_bucket(&harness, case.label, case.policy.clone()).await;
        let firewall_name = format!("fw-{}", case.label);
        create_firewall(&harness, &firewall_name, &policy_arn).await;

        let request = json!({
            "FirewallName": firewall_name,
            "LoggingConfiguration": {
                "LogDestinationConfigs": [s3_destination("ALERT", case.label)],
            },
        });
        let (status, body) = harness.call("UpdateLoggingConfiguration", &request).await;

        if case.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the destination to be accepted, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                body["LoggingConfiguration"]["LogDestinationConfigs"],
                json!([s3_destination("ALERT", case.label)]),
                "case {case:?}: expected the response to report the stored destination, got \
                 {body:?}"
            );
            let described = described_destinations(&harness, &firewall_name).await;
            assert_eq!(
                described,
                vec![s3_destination("ALERT", case.label)],
                "case {case:?}: expected the read to report the stored destination, got \
                 {described:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the destination to be refused, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "LogDestinationPermissionException",
                "case {case:?}: expected LogDestinationPermissionException, got {body:?}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "Access Denied for LogDestination: {}. Please check LogDestination permission",
                    case.label
                ),
                "case {case:?}: expected the refusal to name the destination, got {body:?}"
            );
            // Nothing is stored on the refusal: the firewall still sends its
            // logs nowhere.
            let described = described_destinations(&harness, &firewall_name).await;
            assert!(
                described.is_empty(),
                "case {case:?}: expected the refused call to store no destination, got \
                 {described:?}"
            );
        }
    }
}

/// RULE: the refusal is about the DESTINATION, so it is lifted exactly when the
/// destination stops denying the attachment — the identical call is then served —
/// and one denying destination refuses the whole configuration however many other
/// destinations it names.
#[tokio::test]
async fn removing_the_deny_lets_the_identical_call_through() {
    let harness = Harness::with_s3();
    let policy_arn = harness.shared_policy("relent-policy").await;
    create_bucket(
        &harness,
        "relenting",
        Some(bucket_policy("Deny", ACCOUNT, &ATTACH_ACTIONS, "relenting")),
    )
    .await;
    create_bucket(&harness, "welcoming", None).await;
    let firewall_name = "fw-relent";
    create_firewall(&harness, firewall_name, &policy_arn).await;

    // Two destinations, one of them denying: the configuration is refused and
    // nothing of it is stored.
    let both = json!({
        "FirewallName": firewall_name,
        "LoggingConfiguration": {
            "LogDestinationConfigs": [
                s3_destination("FLOW", "welcoming"),
                s3_destination("ALERT", "relenting"),
            ],
        },
    });
    let (status, body) = harness.call("UpdateLoggingConfiguration", &both).await;
    assert_eq!(
        status, 400,
        "case one-of-two-denies: expected the whole configuration to be refused, got {status} \
         with {body:?}"
    );
    assert_eq!(
        error_message(&body),
        "Access Denied for LogDestination: relenting. Please check LogDestination permission",
        "case one-of-two-denies: expected the refusal to name the denying destination, got \
         {body:?}"
    );
    assert!(
        described_destinations(&harness, firewall_name)
            .await
            .is_empty(),
        "case one-of-two-denies: expected no destination to be stored"
    );

    // The deny goes, and the identical call is served.
    harness
        .s3()
        .delete_bucket_policy()
        .bucket("relenting")
        .send()
        .await
        .expect("the emulated Amazon S3 drops the bucket policy");
    let served = harness.ok("UpdateLoggingConfiguration", &both).await;
    assert_eq!(
        served["LoggingConfiguration"]["LogDestinationConfigs"],
        json!([
            s3_destination("FLOW", "welcoming"),
            s3_destination("ALERT", "relenting"),
        ]),
        "case deny-removed: expected both destinations to be stored, got {served:?}"
    );
    assert_eq!(
        described_destinations(&harness, firewall_name).await,
        vec![
            s3_destination("FLOW", "welcoming"),
            s3_destination("ALERT", "relenting"),
        ],
        "case deny-removed: expected the read to report both destinations"
    );
}

/// RULE: the configuration a firewall carries is the one the last accepted call
/// stored — over every log type and destination type the model declares — and a
/// call that names no configuration at all disables logging ("if you omit this
/// setting, Network Firewall disables logging for the firewall"), which a reader
/// sees as a configuration with no destinations.
#[tokio::test]
async fn the_stored_configuration_is_what_the_last_accepted_call_set() {
    let harness = Harness::with_s3();
    let policy_arn = harness.shared_policy("roundtrip-logging-policy").await;
    create_bucket(&harness, "roundtrip-logs", None).await;
    let firewall_name = "fw-logging-roundtrip";
    create_firewall(&harness, firewall_name, &policy_arn).await;

    // A firewall nobody configured logging on sends its logs nowhere, and its
    // monitoring dashboard is off.
    let described = harness
        .ok(
            "DescribeLoggingConfiguration",
            &json!({ "FirewallName": firewall_name }),
        )
        .await;
    assert_eq!(
        described["LoggingConfiguration"]["LogDestinationConfigs"],
        json!([]),
        "case fresh-firewall: expected no destination, got {described:?}"
    );
    assert_eq!(
        described["EnableMonitoringDashboard"],
        json!(false),
        "case fresh-firewall: expected the monitoring dashboard to be off, got {described:?}"
    );

    // Rows: the configuration one call sets, over the three log types
    // (`smithy_query(query="shape",shape="LogType")`: ALERT, FLOW, TLS) and the
    // destination types the model declares
    // (`smithy_query(query="shape",shape="LogDestinationType")`: S3,
    // CloudWatchLogs, KinesisDataFirehose) — including a prefixed S3 destination
    // ("optionally provide a prefix, with key prefix") and a call that names no
    // configuration at all.
    let rows: Vec<(&str, Option<Value>, Value)> = vec![
        (
            "one-s3-alert",
            Some(json!([s3_destination("ALERT", "roundtrip-logs")])),
            json!([s3_destination("ALERT", "roundtrip-logs")]),
        ),
        (
            "s3-with-prefix",
            Some(json!([{
                "LogType": "FLOW",
                "LogDestinationType": "S3",
                "LogDestination": { "bucketName": "roundtrip-logs", "prefix": "alerts" },
            }])),
            json!([{
                "LogType": "FLOW",
                "LogDestinationType": "S3",
                "LogDestination": { "bucketName": "roundtrip-logs", "prefix": "alerts" },
            }]),
        ),
        (
            "cloudwatch-tls",
            Some(json!([{
                "LogType": "TLS",
                "LogDestinationType": "CloudWatchLogs",
                "LogDestination": { "logGroup": "firewall-tls" },
            }])),
            json!([{
                "LogType": "TLS",
                "LogDestinationType": "CloudWatchLogs",
                "LogDestination": { "logGroup": "firewall-tls" },
            }]),
        ),
        (
            "firehose-and-s3",
            Some(json!([
                {
                    "LogType": "ALERT",
                    "LogDestinationType": "KinesisDataFirehose",
                    "LogDestination": { "deliveryStream": "firewall-alerts" },
                },
                s3_destination("FLOW", "roundtrip-logs"),
            ])),
            json!([
                {
                    "LogType": "ALERT",
                    "LogDestinationType": "KinesisDataFirehose",
                    "LogDestination": { "deliveryStream": "firewall-alerts" },
                },
                s3_destination("FLOW", "roundtrip-logs"),
            ]),
        ),
        ("logging-disabled", None, json!([])),
    ];

    for (label, configured, expected) in rows {
        let mut request = json!({ "FirewallName": firewall_name });
        if let Some(configs) = &configured {
            request["LoggingConfiguration"] = json!({ "LogDestinationConfigs": configs });
        }
        let updated = harness.ok("UpdateLoggingConfiguration", &request).await;
        assert_eq!(
            updated["LoggingConfiguration"]["LogDestinationConfigs"], expected,
            "case {label}: expected the update to report what it stored, got {updated:?}"
        );
        assert_eq!(
            string_at(&updated, &["FirewallName"]),
            firewall_name,
            "case {label}: expected the response to identify the firewall by name, got \
             {updated:?}"
        );
        let expected_arn =
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{firewall_name}");
        assert_eq!(
            string_at(&updated, &["FirewallArn"]),
            expected_arn,
            "case {label}: expected the response to identify the firewall by ARN, got {updated:?}"
        );

        let described = harness
            .ok(
                "DescribeLoggingConfiguration",
                &json!({ "FirewallName": firewall_name }),
            )
            .await;
        assert_eq!(
            described["LoggingConfiguration"]["LogDestinationConfigs"], expected,
            "case {label}: expected the read to report the stored configuration, got {described:?}"
        );
    }

    // The monitoring dashboard is a setting of its own: a call that names it
    // moves it, and one that does not leaves it where the last call left it.
    let enabled = harness
        .ok(
            "UpdateLoggingConfiguration",
            &json!({ "FirewallName": firewall_name, "EnableMonitoringDashboard": true }),
        )
        .await;
    assert_eq!(
        enabled["EnableMonitoringDashboard"],
        json!(true),
        "case dashboard-enabled: expected the setting to be on, got {enabled:?}"
    );
    let unchanged = harness
        .ok(
            "UpdateLoggingConfiguration",
            &json!({ "FirewallName": firewall_name }),
        )
        .await;
    assert_eq!(
        unchanged["EnableMonitoringDashboard"],
        json!(true),
        "case dashboard-untouched: expected the setting to stay on, got {unchanged:?}"
    );
}

/// RULE: a `LogDestination` map names its destination under the key its
/// destination type publishes (`bucketName` for S3, `logGroup` for CloudWatch
/// Logs, `deliveryStream` for Firehose), so a map carrying no value for that key
/// names nothing to send logs to and is refused with HTTP 400 /
/// `InvalidRequestException`.
#[tokio::test]
async fn a_destination_naming_nothing_is_a_bad_request() {
    let harness = Harness::with_s3();
    let policy_arn = harness.shared_policy("bad-destination-policy").await;
    let firewall_name = "fw-bad-destination";
    create_firewall(&harness, firewall_name, &policy_arn).await;

    // Rows: each destination type with the WRONG key for it, and an empty map.
    // The key each type names its destination by is
    // `LogDestinationConfig$LogDestination`'s own list, spelled out here rather
    // than read back from the code under test.
    let rows: [(&str, &str, &str, Value); 4] = [
        (
            "s3-without-bucket-name",
            "S3",
            "bucketName",
            json!({ "logGroup": "not-a-bucket" }),
        ),
        (
            "cloudwatch-without-log-group",
            "CloudWatchLogs",
            "logGroup",
            json!({ "bucketName": "not-a-log-group" }),
        ),
        (
            "firehose-without-delivery-stream",
            "KinesisDataFirehose",
            "deliveryStream",
            json!({ "logGroup": "not-a-stream" }),
        ),
        (
            "s3-with-an-empty-map",
            "S3",
            "bucketName",
            json!({}),
        ),
    ];

    for (label, destination_type, key, destination) in rows {
        let (status, body) = harness
            .call(
                "UpdateLoggingConfiguration",
                &json!({
                    "FirewallName": firewall_name,
                    "LoggingConfiguration": { "LogDestinationConfigs": [{
                        "LogType": "ALERT",
                        "LogDestinationType": destination_type,
                        "LogDestination": destination,
                    }]},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {label}: expected InvalidRequestException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "LogDestination must provide the key {key}, parameter: [{destination_type}], \
                 context: LoggingConfiguration.LogDestinationConfigs.LogDestination"
            ),
            "case {label}: expected the refusal to name the missing key, got {body:?}"
        );
        assert!(
            described_destinations(&harness, firewall_name)
                .await
                .is_empty(),
            "case {label}: expected the refused call to store no destination"
        );
    }
}

/// RULE: a logging configuration is bounded: this emulator retains at most
/// [`DESTINATION_CEILING`] destinations for one firewall, so a configuration at
/// that ceiling is served and one past it is refused with HTTP 400 /
/// `InvalidRequestException` and stores nothing.
#[tokio::test]
async fn a_configuration_past_the_destination_ceiling_is_refused() {
    let harness = Harness::with_s3();
    let policy_arn = harness.shared_policy("ceiling-policy").await;
    let firewall_name = "fw-ceiling";
    create_firewall(&harness, firewall_name, &policy_arn).await;

    // Rows: the ceiling itself, and one past it. Every destination is a
    // CloudWatch Logs log group, so the row varies the COUNT and nothing else.
    let rows = [
        (DESTINATION_CEILING, true),
        (DESTINATION_CEILING + 1, false),
    ];
    for (count, accepted) in rows {
        let configs: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "LogType": "ALERT",
                    "LogDestinationType": "CloudWatchLogs",
                    "LogDestination": { "logGroup": format!("group-{index}") },
                })
            })
            .collect();
        let request = json!({
            "FirewallName": firewall_name,
            "LoggingConfiguration": { "LogDestinationConfigs": configs.clone() },
        });
        let (status, body) = harness.call("UpdateLoggingConfiguration", &request).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {count} destinations: expected the ceiling to be served, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                described_destinations(&harness, firewall_name).await,
                configs,
                "case {count} destinations: expected all of them to be stored"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {count} destinations: expected HTTP 400 past the ceiling, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {count} destinations: expected InvalidRequestException, got {body:?}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "LoggingConfiguration.LogDestinationConfigs has {count} entries; at most \
                     {DESTINATION_CEILING} are allowed."
                ),
                "case {count} destinations: expected the ceiling refusal, got {body:?}"
            );
            // The refused call stored nothing: the firewall keeps the
            // configuration the accepted row left.
            assert_eq!(
                described_destinations(&harness, firewall_name).await.len(),
                DESTINATION_CEILING,
                "case {count} destinations: expected the refused call to change nothing"
            );
        }
    }
}

/// RULE: both operations identify the firewall the way every firewall operation
/// does — "you must specify the ARN or the name, and you can specify both" — so
/// either names it, and a reference naming no stored firewall is answered with
/// `ResourceNotFoundException`.
#[tokio::test]
async fn the_firewall_is_identified_by_name_or_arn() {
    let harness = Harness::with_s3();
    let policy_arn = harness.shared_policy("identify-logging-policy").await;
    create_bucket(&harness, "identify-logs", None).await;
    let firewall_arn = create_firewall(&harness, "fw-identify", &policy_arn).await;

    let selectors = [
        ("by-name", json!({ "FirewallName": "fw-identify" })),
        ("by-arn", json!({ "FirewallArn": firewall_arn })),
        (
            "by-both",
            json!({ "FirewallName": "fw-identify", "FirewallArn": firewall_arn }),
        ),
    ];
    for (label, selector) in &selectors {
        let mut request = selector.clone();
        request["LoggingConfiguration"] =
            json!({ "LogDestinationConfigs": [s3_destination("ALERT", "identify-logs")] });
        let updated = harness.ok("UpdateLoggingConfiguration", &request).await;
        assert_eq!(
            updated["LoggingConfiguration"]["LogDestinationConfigs"],
            json!([s3_destination("ALERT", "identify-logs")]),
            "case {label}: expected the configuration to be stored, got {updated:?}"
        );
        let described = harness.ok("DescribeLoggingConfiguration", selector).await;
        assert_eq!(
            described["LoggingConfiguration"]["LogDestinationConfigs"],
            json!([s3_destination("ALERT", "identify-logs")]),
            "case {label}: expected the read to report it, got {described:?}"
        );
    }

    for operation in ["UpdateLoggingConfiguration", "DescribeLoggingConfiguration"] {
        let (status, body) = harness
            .call(operation, &json!({ "FirewallName": "fw-nowhere" }))
            .await;
        assert_eq!(
            status, 400,
            "case {operation}: expected HTTP 400 for a firewall nothing holds, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {operation}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}
