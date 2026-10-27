// SPDX-License-Identifier: CC0-1.0

//! What a remediation execution DOES to the resource it names — the whole arc a
//! guardrail client drives: a running recorder records a bucket, a managed rule places
//! it `NON_COMPLIANT`, and the rule's remediation then runs and makes the bucket comply.
//!
//! The rule under test: a remediation execution RUNS the action of the Systems Manager
//! automation runbook its remediation configuration names, against the resource the
//! execution names and with the inputs that configuration's parameters resolve to (a
//! static value, the resource value, or the runbook's own documented default), so by the
//! time the execution reaches its terminal success the resource has been changed as the
//! runbook specifies — and a rule with NO remediation configuration is refused
//! `NoSuchRemediationConfigurationException`, leaving the resource exactly as it was.
//!
//! The rows differ along what the rule generalises over: which runbook the configuration
//! names (two of the catalogue, and one whose action this emulator does not perform),
//! which input each one's parameters resolve (a value the request states, a value the
//! runbook defaults, and the resource value `RESOURCE_ID`), and which state the resulting
//! change leaves the bucket in — including one that leaves it still NON_COMPLIANT,
//! because the state the request asked for is not the one the rule checks for.
//!
//! Config and S3 are mounted on ONE engine, reusing the harness
//! `config_recorded_resource_compliance` builds: the recorder reads the buckets it
//! records from S3's own API, and the remediation acts on them through the same API, so
//! without S3 mounted there is nothing to record and nothing to remediate.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::{Emulator, ACCOUNT_ID};

/// The resource type Config records an S3 bucket under.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// The only published `RemediationTargetType`: an SSM automation document.
const TARGET_TYPE: &str = "SSM_DOCUMENT";

/// The `Parameters` key carrying the role a remediation runs as, which every
/// remediation configuration must provide.
const AUTOMATION_ASSUME_ROLE: &str = "AutomationAssumeRole";

/// The `Parameters` key the S3 runbooks take the bucket's name in.
const BUCKET_NAME: &str = "BucketName";

/// The `ResourceValue` a parameter resolves from the resource the execution is about
/// (`com.amazonaws.configservice#ResourceValueType`, whose one value this is).
const RESOURCE_ID: &str = "RESOURCE_ID";

/// `ComplianceType` values a managed rule places a recorded bucket at.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The terminal state a remediation execution that has run reports.
const SUCCEEDED: &str = "SUCCEEDED";
/// The state a remediation execution reports before its run has started.
const QUEUED: &str = "QUEUED";

/// The Config rule every row writes, which the remediation under test remediates, and
/// the managed rule it runs: "Checks if versioning is enabled for your S3 buckets".
const RULE_NAME: &str = "bucket-guardrail";
/// See [`RULE_NAME`].
const VERSIONING_RULE: &str = "S3_BUCKET_VERSIONING_ENABLED";

/// The default encryption Amazon S3 reports for a bucket nothing has configured
/// encryption on: "Amazon S3 now applies server-side encryption with Amazon S3 managed
/// keys (SSE-S3) as the base level of encryption for every bucket", whose algorithm is
/// `AES256` (`ServerSideEncryption`).
const DEFAULT_ENCRYPTION: &str = "AES256";

/// How long a poll waits for a span the clock owes — a recording pass, an evaluation, a
/// remediation execution — before giving up. Each is due a fraction of a second after
/// the call that made it due, so this is many times over and not a span the behaviour
/// depends on.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(10);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One row: the runbook the rule's remediation names, the parameters that configuration
/// carries beside the role, and the state the remediation must leave the bucket in.
struct Case {
    /// What the row is called in every assertion of it.
    case: &'static str,
    /// The `TargetId` of the remediation configuration: the runbook it runs.
    target_id: &'static str,
    /// Parameters the configuration states as static values, beside the role.
    statics: &'static [(&'static str, &'static str)],
    /// Whether the configuration names the bucket through the resource value rather than
    /// leaving the runbook to act on the resource the execution names.
    bucket_by_resource_value: bool,
    /// Whether this emulator performs the action the runbook names at all.
    runs_an_action: bool,
    /// The bucket's versioning status after the remediation, absent while its versioning
    /// has never been set.
    versioning: Option<&'static str>,
    /// The bucket's default encryption algorithm after the remediation.
    encryption: Option<&'static str>,
    /// The verdict the rule places the bucket at once the change has been recorded.
    verdict_after: &'static str,
}

/// The `PutConfigRule` body for the managed rule under test, scoped to `bucket`.
fn bucket_rule(bucket: &str) -> Value {
    json!({
        "ConfigRule": {
            "ConfigRuleName": RULE_NAME,
            "Source": { "Owner": "AWS", "SourceIdentifier": VERSIONING_RULE },
            "Scope": {
                "ComplianceResourceTypes": [BUCKET_RESOURCE_TYPE],
                "ComplianceResourceId": bucket,
            },
        }
    })
}

/// The `PutRemediationConfigurations` body for this row, built from its runbook and its
/// parameters.
fn remediation(case: &Case) -> Value {
    let mut parameters = json!({
        AUTOMATION_ASSUME_ROLE: {
            "StaticValue": {
                "Values": [format!("arn:aws:iam::{ACCOUNT_ID}:role/config-remediation")],
            },
        },
    });
    for (name, value) in case.statics {
        parameters[*name] = json!({ "StaticValue": { "Values": [value] } });
    }
    if case.bucket_by_resource_value {
        parameters[BUCKET_NAME] = json!({ "ResourceValue": { "Value": RESOURCE_ID } });
    }
    json!({
        "RemediationConfigurations": [{
            "ConfigRuleName": RULE_NAME,
            "TargetType": TARGET_TYPE,
            "TargetId": case.target_id,
            "ResourceType": BUCKET_RESOURCE_TYPE,
            "Parameters": parameters,
        }],
    })
}

/// The `StartRemediationExecution` body for `bucket`.
fn start(bucket: &str) -> Value {
    json!({
        "ConfigRuleName": RULE_NAME,
        "ResourceKeys": [{
            "resourceType": BUCKET_RESOURCE_TYPE,
            "resourceId": bucket,
        }],
    })
}

/// The bucket's versioning status, absent while its versioning has never been set.
async fn versioning_of(emulator: &Emulator, bucket: &str) -> Option<String> {
    emulator
        .s3()
        .get_bucket_versioning()
        .bucket(bucket)
        .send()
        .await
        .ok()
        .and_then(|answer| answer.status().map(|status| status.as_str().to_string()))
}

/// The bucket's default encryption algorithm, absent while it has no default encryption.
async fn encryption_of(emulator: &Emulator, bucket: &str) -> Option<String> {
    emulator
        .s3()
        .get_bucket_encryption()
        .bucket(bucket)
        .send()
        .await
        .ok()
        .and_then(|answer| answer.server_side_encryption_configuration)
        .and_then(|configuration| {
            configuration
                .rules
                .iter()
                .filter_map(|rule| {
                    rule.apply_server_side_encryption_by_default
                        .as_ref()
                        .map(|default| default.sse_algorithm.as_str().to_string())
                })
                .next()
        })
}

/// Wait until the recorder has captured a configuration item for `bucket`, which is what
/// every leg downstream of discovery reads.
async fn await_configuration_item(case: &str, emulator: &Emulator, bucket: &str) {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let (status, body) = emulator
            .call(
                "config",
                "StarlingDoveService",
                "1.1",
                "GetResourceConfigHistory",
                &json!({ "resourceType": BUCKET_RESOURCE_TYPE, "resourceId": bucket }),
            )
            .await;
        let items = body["configurationItems"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        if status == 200 && items > 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: a running recorder must capture a configuration item for \
             {bucket:?}; the last read answered {status} with {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until the rule places `bucket` at `wanted`, and answer what it placed it at.
async fn await_verdict(case: &str, emulator: &Emulator, bucket: &str, wanted: &str) -> String {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let body = emulator
            .config(
                "GetComplianceDetailsByResource",
                &json!({ "ResourceType": BUCKET_RESOURCE_TYPE, "ResourceId": bucket }),
            )
            .await;
        let placed: Vec<String> = body["EvaluationResults"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|result| result["ComplianceType"].as_str().map(ToString::to_string))
            .collect();
        if placed.iter().any(|verdict| verdict == wanted) {
            return wanted.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the rule must place {bucket:?} at {wanted}, got {placed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The state the rule's remediation execution for `bucket` reports right now.
async fn execution_state(case: &str, emulator: &Emulator, bucket: &str) -> String {
    let body = emulator
        .config(
            "DescribeRemediationExecutionStatus",
            &json!({ "ConfigRuleName": RULE_NAME }),
        )
        .await;
    let reported: Vec<(String, String)> = body["RemediationExecutionStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|status| {
            let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
            (
                text(&status["ResourceKey"]["resourceId"]),
                text(&status["State"]),
            )
        })
        .collect();
    reported
        .iter()
        .find(|(resource, _)| resource == bucket)
        .map(|(_, state)| state.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: an execution must be reported for {bucket:?}, got {reported:?}")
        })
}

/// Wait until the rule's remediation execution for `bucket` reports `wanted`.
async fn await_execution_state(case: &str, emulator: &Emulator, bucket: &str, wanted: &str) {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let reported = execution_state(case, emulator, bucket).await;
        if reported == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the remediation execution for {bucket:?} must reach {wanted}, \
             got {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_remediation_execution_runs_the_runbook_its_configuration_names() {
    // The catalogue entries exercised below are the AWS-owned automation runbooks a
    // Config remediation configuration names on an S3 bucket, from the Systems Manager
    // Automation runbook reference: `AWS-ConfigureS3BucketVersioning` (parameters
    // `BucketName` and `VersioningState`, whose default is `Enabled`) and
    // `AWS-EnableS3BucketEncryption` (parameters `BucketName` and `SSEAlgorithm`, whose
    // default is `AES256`). `AWS-EnableS3BucketEncryption` is the entry this issue did
    // not cite, and `AWS-UpdateSSMAgent` is a published document whose action this
    // emulator does not perform at all.
    let cases: [Case; 5] = [
        Case {
            case: "the versioning runbook, left to its default state",
            target_id: "AWS-ConfigureS3BucketVersioning",
            statics: &[],
            bucket_by_resource_value: false,
            runs_an_action: true,
            versioning: Some("Enabled"),
            encryption: Some(DEFAULT_ENCRYPTION),
            verdict_after: COMPLIANT,
        },
        Case {
            case: "the versioning runbook, naming the bucket through the resource value",
            target_id: "AWS-ConfigureS3BucketVersioning",
            statics: &[("VersioningState", "Enabled")],
            bucket_by_resource_value: true,
            runs_an_action: true,
            versioning: Some("Enabled"),
            encryption: Some(DEFAULT_ENCRYPTION),
            verdict_after: COMPLIANT,
        },
        Case {
            case: "the versioning runbook, asked to suspend versioning",
            target_id: "AWS-ConfigureS3BucketVersioning",
            statics: &[("VersioningState", "Suspended")],
            bucket_by_resource_value: false,
            runs_an_action: true,
            versioning: Some("Suspended"),
            encryption: Some(DEFAULT_ENCRYPTION),
            // Suspended versioning is not versioning enabled, so the rule still places
            // the bucket as NON_COMPLIANT after the remediation ran.
            verdict_after: NON_COMPLIANT,
        },
        Case {
            case: "the encryption runbook, asked for KMS encryption",
            target_id: "AWS-EnableS3BucketEncryption",
            statics: &[("SSEAlgorithm", "aws:kms")],
            bucket_by_resource_value: true,
            runs_an_action: true,
            // The runbook this configuration names sets no versioning state, so the
            // bucket the versioning rule checks is left NON_COMPLIANT.
            versioning: None,
            encryption: Some("aws:kms"),
            verdict_after: NON_COMPLIANT,
        },
        Case {
            case: "a runbook whose action this emulator does not perform",
            target_id: "AWS-UpdateSSMAgent",
            statics: &[],
            bucket_by_resource_value: false,
            runs_an_action: false,
            versioning: None,
            encryption: Some(DEFAULT_ENCRYPTION),
            verdict_after: NON_COMPLIANT,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let bucket = "remediated-bucket";
        emulator.recording().await;
        emulator.create_bucket(bucket, &[]).await;

        // Everything downstream of discovery reads the configuration item the recorder
        // captures for the bucket, so the arc starts there.
        await_configuration_item(case.case, &emulator, bucket).await;
        emulator.config("PutConfigRule", &bucket_rule(bucket)).await;
        await_verdict(case.case, &emulator, bucket, NON_COMPLIANT).await;

        // A rule whose remediation configuration has been DELETED has no remediation to
        // run, which is the one precondition this refusal states — and the resource is
        // left exactly as it was.
        let put = emulator
            .config("PutRemediationConfigurations", &remediation(&case))
            .await;
        assert_eq!(
            put["FailedBatches"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: the remediation configuration must be stored, got {put}",
            case.case
        );
        emulator
            .config(
                "DeleteRemediationConfiguration",
                &json!({ "ConfigRuleName": RULE_NAME }),
            )
            .await;
        let (status, code) = emulator
            .refused("StartRemediationExecution", &start(bucket))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchRemediationConfigurationException"),
            "case {:?}: a disarmed rule must refuse the execution",
            case.case
        );
        assert_eq!(
            (
                versioning_of(&emulator, bucket).await.as_deref(),
                encryption_of(&emulator, bucket).await.as_deref(),
            ),
            (None, Some(DEFAULT_ENCRYPTION)),
            "case {:?}: the refused execution must leave the bucket untouched",
            case.case
        );

        // The identical configuration, put again, re-arms the identical execution: it
        // runs the runbook's action and reaches its terminal success.
        emulator
            .config("PutRemediationConfigurations", &remediation(&case))
            .await;
        let started = emulator
            .config("StartRemediationExecution", &start(bucket))
            .await;
        assert_eq!(
            started["FailedItems"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: the execution must start for the bucket, got {started}",
            case.case
        );

        // What the FIRST read after the call reports: an execution whose action this
        // emulator ran has run it, so it reports the run as finished and the bucket
        // already carries the change. One whose runbook names no action this emulator
        // performs has nothing to report the progress of, so it reports the moves the
        // clock makes due, starting at `QUEUED`.
        let reported = execution_state(case.case, &emulator, bucket).await;
        let (versioning_now, encryption_now) = (
            versioning_of(&emulator, bucket).await,
            encryption_of(&emulator, bucket).await,
        );
        assert_eq!(
            (
                reported.as_str(),
                versioning_now.as_deref(),
                encryption_now.as_deref(),
            ),
            if case.runs_an_action {
                (SUCCEEDED, case.versioning, case.encryption)
            } else {
                (QUEUED, case.versioning, case.encryption)
            },
            "case {:?}: the first read after the call must report the run and the bucket \
             it changed",
            case.case
        );
        await_execution_state(case.case, &emulator, bucket, SUCCEEDED).await;

        assert_eq!(
            (
                versioning_of(&emulator, bucket).await.as_deref(),
                encryption_of(&emulator, bucket).await.as_deref(),
            ),
            (case.versioning, case.encryption),
            "case {:?}: the execution must leave the bucket as its runbook specifies",
            case.case
        );

        // The recorder captures the state the remediation produced, and the rule places
        // the bucket from THAT configuration item.
        let placed = await_verdict(case.case, &emulator, bucket, case.verdict_after).await;
        assert_eq!(
            placed, case.verdict_after,
            "case {:?}: the rule must place the remediated bucket at {}",
            case.case, case.verdict_after
        );
    }
}
