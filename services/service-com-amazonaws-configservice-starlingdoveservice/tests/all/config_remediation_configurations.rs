// SPDX-License-Identifier: CC0-1.0

//! The remediation configuration of a Config rule, end to end over the wire.
//!
//! The rules pinned here:
//!
//! 1. `DescribeRemediationConfigurations` reports, for each Config rule the request
//!    names that holds a remediation configuration, exactly what
//!    `PutRemediationConfigurations` materialised for it — every member the write
//!    carried, the auto-remediation defaults Config documents, and the ARN Config
//!    minted — and reports no entry for a named rule that holds none (which is what a
//!    rule whose configuration was deleted, or never had one, is);
//! 2. a configuration Config cannot run is refused rather than stored: one breaking a
//!    published constraint, one whose parameter selects neither or both kinds of
//!    value, and one that does not provide the role the remediation runs as;
//! 3. a configuration naming a Config rule this account does not hold is a failed
//!    item of the batch, and costs the other items of the same batch nothing.
//!
//! Every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`, reusing the harness `config_setup_and_rule_loop` builds. The
//! rows of each table differ along what the rules generalise over: whether the
//! remediation is automatic, which kind of parameter value it carries, how many
//! optional members it names, and where its numbers sit in their published ranges.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// The only published `RemediationTargetType` value, from
/// `smithy_query(query="shape", shape="RemediationTargetType")`: `SSM_DOCUMENT`.
const SSM_DOCUMENT: &str = "SSM_DOCUMENT";

/// The published bounds of `RemediationConfiguration$MaximumAutomaticAttempts`
/// (`smithy_query(query="shape", shape="RemediationConfiguration")`: range 1..25).
const MAXIMUM_AUTOMATIC_ATTEMPTS_RANGE: (i64, i64) = (1, 25);

/// The published bounds of `RemediationConfiguration$RetryAttemptSeconds` (same
/// query: range 1..2678000).
const RETRY_ATTEMPT_SECONDS_RANGE: (i64, i64) = (1, 2_678_000);

/// The defaults Config documents for an auto-remediation configuration that names
/// neither number (`RemediationConfiguration$MaximumAutomaticAttempts`: "If you do
/// not select a number, the default is 5"; `$RetryAttemptSeconds`: "If you do not
/// select a number, the default is 60 seconds").
const DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS: i64 = 5;
/// See [`DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS`].
const DEFAULT_RETRY_ATTEMPT_SECONDS: i64 = 60;

/// The `Parameters` keys that can carry the role a remediation runs as
/// (`PutRemediationConfigurations`, "Required fields": "For manual remediation
/// configuration, you need to provide a value for automationAssumeRole or use a value
/// in the assumeRole field ... However, for automatic remediation configuration, the
/// only valid assumeRole field value is AutomationAssumeRole").
const AUTOMATION_ASSUME_ROLE: &str = "AutomationAssumeRole";
/// See [`AUTOMATION_ASSUME_ROLE`].
const ASSUME_ROLE: &str = "AssumeRole";

/// A role ARN a remediation hands to SSM; its value is only ever read back.
const AUTOMATION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/config-remediation";

/// Resource types the rows below remediate. `RemediationConfiguration$ResourceType`
/// points at Config's *Supported Amazon Web Services resource Types*; these are
/// entries from it.
const RESOURCE_TYPES: [&str; 3] = [
    "AWS::S3::Bucket",
    "AWS::EC2::Volume",
    "AWS::DynamoDB::Table",
];

/// One table row: the remediation configuration written for `rule`, and what the
/// write should materialise beyond the members it names.
struct Case {
    case: &'static str,
    rule: &'static str,
    configuration: Value,
    /// The `MaximumAutomaticAttempts` and `RetryAttemptSeconds` a read must report,
    /// which is what the request named, or Config's default for an automatic
    /// configuration, or nothing at all for a manual one.
    expected_retries: (Option<i64>, Option<i64>),
}

/// A static remediation parameter carrying `values`.
fn static_parameter(values: &[&str]) -> Value {
    json!({ "StaticValue": { "Values": values } })
}

/// The `Parameters` of a configuration that names only the role it runs as, under
/// `key`.
fn role_parameters(key: &str) -> Value {
    json!({ key: static_parameter(&[AUTOMATION_ROLE_ARN]) })
}

/// The table every round-trip case in this suite is driven from.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "manual remediation, only the required members and its role",
            rule: "remediation-minimal",
            configuration: json!({
                "ConfigRuleName": "remediation-minimal",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-PublishSNSNotification",
                "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
            }),
            expected_retries: (None, None),
        },
        Case {
            case: "manual remediation naming its role in the AssumeRole parameter",
            rule: "remediation-assume-role",
            configuration: json!({
                "ConfigRuleName": "remediation-assume-role",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-PublishSNSNotification",
                "Parameters": role_parameters(ASSUME_ROLE),
            }),
            expected_retries: (None, None),
        },
        Case {
            case: "manual remediation with a target version and a resource type",
            rule: "remediation-versioned",
            configuration: json!({
                "ConfigRuleName": "remediation-versioned",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-EnableS3BucketEncryption",
                "TargetVersion": "3",
                "ResourceType": RESOURCE_TYPES[0],
                "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
            }),
            expected_retries: (None, None),
        },
        Case {
            case: "automatic remediation taking both documented defaults",
            rule: "remediation-automatic-defaults",
            configuration: json!({
                "ConfigRuleName": "remediation-automatic-defaults",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-ConfigureS3BucketVersioning",
                "Automatic": true,
                "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
            }),
            expected_retries: (
                Some(DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS),
                Some(DEFAULT_RETRY_ATTEMPT_SECONDS),
            ),
        },
        Case {
            case: "automatic remediation at the low end of both published ranges",
            rule: "remediation-automatic-low",
            configuration: json!({
                "ConfigRuleName": "remediation-automatic-low",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-DisableS3BucketPublicReadWrite",
                "Automatic": true,
                "MaximumAutomaticAttempts": MAXIMUM_AUTOMATIC_ATTEMPTS_RANGE.0,
                "RetryAttemptSeconds": RETRY_ATTEMPT_SECONDS_RANGE.0,
                "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
            }),
            expected_retries: (
                Some(MAXIMUM_AUTOMATIC_ATTEMPTS_RANGE.0),
                Some(RETRY_ATTEMPT_SECONDS_RANGE.0),
            ),
        },
        Case {
            case: "automatic remediation at the high end of both published ranges",
            rule: "remediation-automatic-high",
            configuration: json!({
                "ConfigRuleName": "remediation-automatic-high",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-AttachEC2Volume",
                "Automatic": true,
                "MaximumAutomaticAttempts": MAXIMUM_AUTOMATIC_ATTEMPTS_RANGE.1,
                "RetryAttemptSeconds": RETRY_ATTEMPT_SECONDS_RANGE.1,
                "ResourceType": RESOURCE_TYPES[1],
                "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
            }),
            expected_retries: (
                Some(MAXIMUM_AUTOMATIC_ATTEMPTS_RANGE.1),
                Some(RETRY_ATTEMPT_SECONDS_RANGE.1),
            ),
        },
        Case {
            case: "a dynamic (resource) parameter value beside static ones",
            rule: "remediation-dynamic-parameter",
            configuration: json!({
                "ConfigRuleName": "remediation-dynamic-parameter",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-ConfigureS3BucketLogging",
                "ResourceType": RESOURCE_TYPES[2],
                "Parameters": {
                    AUTOMATION_ASSUME_ROLE: static_parameter(&[AUTOMATION_ROLE_ARN]),
                    // `ResourceValue$Value` is the published `ResourceValueType`
                    // enum, whose single value is RESOURCE_ID.
                    "BucketName": { "ResourceValue": { "Value": "RESOURCE_ID" } },
                    "TargetPrefix": static_parameter(&["logs/", "archive/"]),
                },
            }),
            expected_retries: (None, None),
        },
        Case {
            case: "execution controls bound the remediation's SSM concurrency",
            rule: "remediation-execution-controls",
            configuration: json!({
                "ConfigRuleName": "remediation-execution-controls",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-TerminateEC2Instance",
                "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
                "ExecutionControls": {
                    // `Percentage` is published as range 1..100 on both members.
                    "SsmControls": {
                        "ConcurrentExecutionRatePercentage": 1,
                        "ErrorPercentage": 100,
                    }
                },
            }),
            expected_retries: (None, None),
        },
        Case {
            case: "an empty static value list is still a parameter",
            rule: "remediation-empty-static-values",
            configuration: json!({
                "ConfigRuleName": "remediation-empty-static-values",
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-RestartEC2Instance",
                "Parameters": {
                    AUTOMATION_ASSUME_ROLE: static_parameter(&[AUTOMATION_ROLE_ARN]),
                    "Tags": static_parameter(&[]),
                },
            }),
            expected_retries: (None, None),
        },
    ]
}

/// An emulator holding a recorder, a delivery channel and one managed Config rule per
/// name in `rules`.
async fn with_rules(emulator: &Emulator, rules: &[&str]) {
    with_recorder_and_channel(emulator).await;
    for rule in rules {
        emulator.ok("PutConfigRule", &managed_rule(rule)).await;
    }
}

/// A manual remediation configuration for `rule`, carrying the role it runs as.
fn configuration_for(rule: &str) -> Value {
    json!({
        "ConfigRuleName": rule,
        "TargetType": SSM_DOCUMENT,
        "TargetId": "AWS-PublishSNSNotification",
        "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
    })
}

/// `PutRemediationConfigurations` for `configurations`, requiring every item to be
/// written (no failed batch).
async fn put_all(emulator: &Emulator, configurations: Vec<Value>) {
    let put = emulator
        .ok(
            "PutRemediationConfigurations",
            &json!({ "RemediationConfigurations": configurations }),
        )
        .await;
    let failed = put["FailedBatches"]
        .as_array()
        .unwrap_or_else(|| panic!("FailedBatches must be a list, got {put}"));
    assert!(
        failed.is_empty(),
        "every configuration named an existing rule, so no batch may fail, got {put}"
    );
}

/// The remediation configurations reported for `names`, in the order reported.
async fn described(emulator: &Emulator, names: &[&str]) -> Vec<Value> {
    let response = emulator
        .ok(
            "DescribeRemediationConfigurations",
            &json!({ "ConfigRuleNames": names }),
        )
        .await;
    response["RemediationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("RemediationConfigurations must be a list, got {response}"))
        .clone()
}

/// Every member the write carried comes back out of state unchanged.
fn assert_members_round_trip(case: &str, requested: &Value, reported: &Value) {
    let members = requested
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: a configuration is an object"));
    for (member, expected) in members {
        // `Automatic` has a modelled default of false, so a read may leave it out
        // where it is false; every other member is compared as sent.
        if member == "Automatic" {
            let actual = reported
                .get(member)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let expected = expected.as_bool().unwrap_or(false);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected Automatic {expected}, got {actual}"
            );
            continue;
        }
        let actual = reported.get(member).unwrap_or(&Value::Null);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {member} {expected} to come back from state, got {actual}"
        );
    }
}

/// The remediation configuration ARN of `rule`, checked against the form Config
/// mints — `arn:aws:config:<region>:<account>:remediation-configuration/<rule>/<id>`
/// — and returned so a later read can be compared against it.
fn assert_arn(case: &str, region: &str, rule: &str, reported: &Value) -> String {
    let arn = reported["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Arn, got {reported}"))
        .to_string();
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: a remediation configuration ARN has six segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[1], segments[2], segments[3]),
        ("arn", "aws", "config", region),
        "case {case:?}: expected an arn:aws:config ARN in {region}, got {arn:?}"
    );
    assert!(
        !segments[4].is_empty(),
        "case {case:?}: expected the account id in the ARN, got {arn:?}"
    );
    let prefix = format!("remediation-configuration/{rule}/");
    let Some(id) = segments[5].strip_prefix(&prefix) else {
        panic!("case {case:?}: expected the ARN resource to start {prefix:?}, got {arn:?}");
    };
    assert!(
        !id.is_empty(),
        "case {case:?}: expected the assigned configuration id in the ARN, got {arn:?}"
    );
    arn
}

#[tokio::test]
async fn a_remediation_configuration_round_trips_through_state() {
    let emulator = Emulator::in_region("eu-west-1");
    let cases = cases();
    let rules: Vec<&str> = cases.iter().map(|case| case.rule).collect();
    with_rules(&emulator, &rules).await;

    put_all(
        &emulator,
        cases
            .iter()
            .map(|case| case.configuration.clone())
            .collect(),
    )
    .await;

    let reported = described(&emulator, &rules).await;
    assert_eq!(
        reported.len(),
        cases.len(),
        "every rule holds a remediation configuration, so each must be reported, got {reported:?}"
    );

    let mut arns: Vec<String> = Vec::new();
    for (case, reported) in cases.iter().zip(&reported) {
        assert_eq!(
            reported["ConfigRuleName"], case.rule,
            "case {:?}: the configurations come back in the order they were named, got {reported}",
            case.case
        );
        assert_members_round_trip(case.case, &case.configuration, reported);
        arns.push(assert_arn(case.case, &emulator.region, case.rule, reported));

        let (attempts, seconds) = case.expected_retries;
        for (member, expected) in [
            ("MaximumAutomaticAttempts", attempts),
            ("RetryAttemptSeconds", seconds),
        ] {
            let actual = reported.get(member).and_then(Value::as_i64);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {member} {expected:?}, got {actual:?}",
                case.case
            );
        }

        // Config assigns `CreatedByService` only for a service-linked rule's
        // configuration, and this API does not create those.
        assert!(
            reported.get("CreatedByService").is_none_or(Value::is_null),
            "case {:?}: expected no CreatedByService, got {reported}",
            case.case
        );
    }

    let distinct = {
        let mut sorted = arns.clone();
        sorted.sort();
        sorted.dedup();
        sorted.len()
    };
    assert_eq!(
        distinct,
        arns.len(),
        "every remediation configuration gets its own ARN, got {arns:?}"
    );
}

#[tokio::test]
async fn a_describe_reports_only_the_named_rules_that_hold_a_configuration() {
    let emulator = Emulator::new();
    let held = "remediation-held";
    let bare = "remediation-bare";
    with_rules(&emulator, &[held, bare]).await;
    put_all(&emulator, vec![configuration_for(held)]).await;

    // Rows differ along which names are asked for, how many, and in what order.
    let cases: [(&str, &[&str], &[&str]); 7] = [
        ("no names at all", &[], &[]),
        ("the rule holding a configuration", &[held], &[held]),
        ("a rule holding none", &[bare], &[]),
        ("a name that is no Config rule", &["remediation-absent"], &[]),
        (
            "the holder beside a rule holding none",
            &[bare, held],
            &[held],
        ),
        ("the same name repeated", &[held, held, held], &[held]),
        (
            "every name, holders and non-holders",
            &[bare, held, "remediation-absent"],
            &[held],
        ),
    ];

    for (case, names, expected) in cases {
        let reported = described(&emulator, names).await;
        let actual: Vec<&str> = reported
            .iter()
            .map(|configuration| configuration["ConfigRuleName"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} reported for {names:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_second_put_updates_the_configuration_and_keeps_its_arn() {
    let emulator = Emulator::new();
    let rule = "remediation-updated";
    with_rules(&emulator, &[rule]).await;

    // Rows differ along what the update changes: the target, the parameters, whether
    // the remediation becomes automatic, and the retry numbers.
    let revisions: [Case; 4] = [
        Case {
            case: "created manual",
            rule,
            configuration: configuration_for(rule),
            expected_retries: (None, None),
        },
        Case {
            case: "retargeted with another parameter",
            rule,
            configuration: json!({
                "ConfigRuleName": rule,
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-ConfigureS3BucketVersioning",
                "Parameters": {
                    AUTOMATION_ASSUME_ROLE: static_parameter(&[AUTOMATION_ROLE_ARN]),
                    "BucketName": { "ResourceValue": { "Value": "RESOURCE_ID" } },
                },
            }),
            expected_retries: (None, None),
        },
        Case {
            case: "made automatic, taking the defaults",
            rule,
            configuration: json!({
                "ConfigRuleName": rule,
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-ConfigureS3BucketVersioning",
                "Automatic": true,
                "ResourceType": RESOURCE_TYPES[0],
                "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE),
            }),
            expected_retries: (
                Some(DEFAULT_MAXIMUM_AUTOMATIC_ATTEMPTS),
                Some(DEFAULT_RETRY_ATTEMPT_SECONDS),
            ),
        },
        Case {
            case: "back to manual, dropping every optional member",
            rule,
            configuration: configuration_for(rule),
            expected_retries: (None, None),
        },
    ];

    let mut minted: Option<String> = None;
    for Case {
        case,
        configuration,
        expected_retries: (attempts, seconds),
        ..
    } in revisions
    {
        put_all(&emulator, vec![configuration.clone()]).await;
        let reported = described(&emulator, &[rule]).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: a rule holds exactly one remediation configuration, got {reported:?}"
        );
        let reported = &reported[0];
        assert_members_round_trip(case, &configuration, reported);

        for (member, expected) in [
            ("MaximumAutomaticAttempts", attempts),
            ("RetryAttemptSeconds", seconds),
        ] {
            let actual = reported.get(member).and_then(Value::as_i64);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?} after the update, got {actual:?}"
            );
        }
        // A member the update did not name is not carried over from the revision
        // before it: the configuration is replaced, not merged.
        for member in ["TargetVersion", "ResourceType", "ExecutionControls"] {
            if configuration.get(member).is_none() {
                assert!(
                    reported.get(member).is_none_or(Value::is_null),
                    "case {case:?}: {member} was not named by this put, so it must not be \
                     reported, got {reported}"
                );
            }
        }

        let arn = assert_arn(case, &emulator.region, rule, reported);
        match &minted {
            None => minted = Some(arn),
            Some(first) => assert_eq!(
                &arn, first,
                "case {case:?}: an update keeps the ARN minted at creation, expected {first}, \
                 got {arn}"
            ),
        }
    }
}

#[tokio::test]
async fn a_configuration_for_a_rule_that_does_not_exist_is_a_failed_batch() {
    let emulator = Emulator::new();
    let existing = "remediation-existing-rule";
    with_rules(&emulator, &[existing]).await;

    // Rows differ along how many absent rules the batch names and where they sit in
    // it, so a failed item never costs a valid one its write.
    let absent_names: [(&str, &[&str]); 3] = [
        ("one absent rule alone", &["remediation-no-rule"]),
        (
            "an absent rule before the existing one",
            &["remediation-no-rule-a"],
        ),
        (
            "two absent rules around the existing one",
            &["remediation-no-rule-b", "remediation-no-rule-c"],
        ),
    ];

    for (case, absent) in absent_names {
        let mut configurations: Vec<Value> =
            absent.iter().map(|rule| configuration_for(rule)).collect();
        // The existing rule's configuration sits in the middle of the batch.
        configurations.insert(configurations.len() / 2, configuration_for(existing));

        let response = emulator
            .ok(
                "PutRemediationConfigurations",
                &json!({ "RemediationConfigurations": configurations }),
            )
            .await;
        let batches = response["FailedBatches"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: FailedBatches must be a list, {response}"));
        let failed: Vec<&str> = batches
            .iter()
            .flat_map(|batch| {
                batch["FailedItems"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
            })
            .map(|item| item["ConfigRuleName"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            failed, absent,
            "case {case:?}: expected the items naming {absent:?} to fail, got {response}"
        );
        for batch in batches {
            let message = batch["FailureMessage"].as_str().unwrap_or("");
            assert!(
                !message.is_empty(),
                "case {case:?}: every failed batch states its reason, got {batch}"
            );
        }

        // Nothing was persisted for the absent rules, and the valid item was.
        let mut asked: Vec<&str> = absent.to_vec();
        asked.push(existing);
        let reported = described(&emulator, &asked).await;
        let names: Vec<&str> = reported
            .iter()
            .map(|configuration| configuration["ConfigRuleName"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            names,
            vec![existing],
            "case {case:?}: only the existing rule's configuration is persisted, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_configuration_must_provide_the_role_the_remediation_runs_as() {
    let emulator = Emulator::new();
    let rule = "remediation-assume-role-required";
    with_rules(&emulator, &[rule]).await;

    // Rows differ along what stands in for the role and whether the remediation is
    // automatic: an automatic configuration remediates only through
    // `AutomationAssumeRole`, a manual one through either key, and neither accepts a
    // key that carries no value.
    let refused: [(&str, bool, Value); 7] = [
        ("manual with no Parameters at all", false, json!({})),
        ("automatic with no Parameters at all", true, json!({})),
        (
            "manual with other parameters only",
            false,
            json!({ "BucketName": { "ResourceValue": { "Value": "RESOURCE_ID" } } }),
        ),
        (
            "automatic naming only the manual AssumeRole key",
            true,
            role_parameters(ASSUME_ROLE),
        ),
        (
            "manual with a blank role value",
            false,
            json!({ AUTOMATION_ASSUME_ROLE: static_parameter(&["  "]) }),
        ),
        (
            "manual with an empty role value list",
            false,
            json!({ AUTOMATION_ASSUME_ROLE: static_parameter(&[]) }),
        ),
        (
            "manual with a lowercased role parameter name",
            false,
            json!({ "automationassumerole": static_parameter(&[AUTOMATION_ROLE_ARN]) }),
        ),
    ];

    for (case, automatic, parameters) in refused {
        let mut configuration = json!({
            "ConfigRuleName": rule,
            "TargetType": SSM_DOCUMENT,
            "TargetId": "AWS-ConfigureS3BucketVersioning",
            "Automatic": automatic,
        });
        if !parameters
            .as_object()
            .expect("parameters are an object")
            .is_empty()
        {
            configuration["Parameters"] = parameters;
        }
        let (status, code) = emulator
            .refused(
                "PutRemediationConfigurations",
                &json!({ "RemediationConfigurations": [configuration] }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {code}"
        );
        let reported = described(&emulator, &[rule]).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused put persists nothing, got {reported:?}"
        );
    }

    // The accepted rows: whichever key and value kind the role arrives under.
    let accepted: [(&str, bool, Value); 3] = [
        (
            "automatic with a static AutomationAssumeRole",
            true,
            role_parameters(AUTOMATION_ASSUME_ROLE),
        ),
        (
            "automatic with a dynamic AutomationAssumeRole",
            true,
            json!({ AUTOMATION_ASSUME_ROLE: { "ResourceValue": { "Value": "RESOURCE_ID" } } }),
        ),
        (
            "manual with the AssumeRole key",
            false,
            role_parameters(ASSUME_ROLE),
        ),
    ];
    for (case, automatic, parameters) in accepted {
        put_all(
            &emulator,
            vec![json!({
                "ConfigRuleName": rule,
                "TargetType": SSM_DOCUMENT,
                "TargetId": "AWS-ConfigureS3BucketVersioning",
                "Automatic": automatic,
                "Parameters": parameters,
            })],
        )
        .await;
        let reported = described(&emulator, &[rule]).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the configuration must be persisted, got {reported:?}"
        );
        let actual = reported[0]["Automatic"].as_bool().unwrap_or(false);
        assert_eq!(
            actual, automatic,
            "case {case:?}: expected Automatic {automatic}, got {actual}"
        );
    }
}

#[tokio::test]
async fn a_configuration_outside_its_published_constraints_is_refused() {
    let emulator = Emulator::new();
    let rule = "remediation-constraints";
    with_rules(&emulator, &[rule]).await;

    // Rows differ along which published constraint the configuration breaks: the
    // `RemediationTargetType` enum, the `TargetId` length (1..256), the
    // `MaximumAutomaticAttempts` range (1..25) and the `RetryAttemptSeconds` range
    // (1..2678000) on each side of each bound, and the `RemediationParameterValue`
    // rule that exactly one of the two kinds of value is selected. Every row is
    // otherwise a configuration this emulator accepts.
    let rows: [(&str, &str, Value); 8] = [
        (
            "target type outside the published enum",
            "ValidationException",
            json!({ "ConfigRuleName": rule, "TargetType": "LAMBDA_FUNCTION", "TargetId": "doc",
                    "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE) }),
        ),
        (
            "a target id shorter than the published minimum",
            "ValidationException",
            json!({ "ConfigRuleName": rule, "TargetType": SSM_DOCUMENT, "TargetId": "",
                    "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE) }),
        ),
        (
            "maximum attempts one below the published range",
            "ValidationException",
            json!({ "ConfigRuleName": rule, "TargetType": SSM_DOCUMENT, "TargetId": "doc",
                    "MaximumAutomaticAttempts": MAXIMUM_AUTOMATIC_ATTEMPTS_RANGE.0 - 1,
                    "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE) }),
        ),
        (
            "maximum attempts one above the published range",
            "ValidationException",
            json!({ "ConfigRuleName": rule, "TargetType": SSM_DOCUMENT, "TargetId": "doc",
                    "MaximumAutomaticAttempts": MAXIMUM_AUTOMATIC_ATTEMPTS_RANGE.1 + 1,
                    "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE) }),
        ),
        (
            "retry seconds one below the published range",
            "ValidationException",
            json!({ "ConfigRuleName": rule, "TargetType": SSM_DOCUMENT, "TargetId": "doc",
                    "RetryAttemptSeconds": RETRY_ATTEMPT_SECONDS_RANGE.0 - 1,
                    "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE) }),
        ),
        (
            "retry seconds one above the published range",
            "ValidationException",
            json!({ "ConfigRuleName": rule, "TargetType": SSM_DOCUMENT, "TargetId": "doc",
                    "RetryAttemptSeconds": RETRY_ATTEMPT_SECONDS_RANGE.1 + 1,
                    "Parameters": role_parameters(AUTOMATION_ASSUME_ROLE) }),
        ),
        (
            "a parameter selecting both kinds of value",
            "InvalidParameterValueException",
            json!({ "ConfigRuleName": rule, "TargetType": SSM_DOCUMENT, "TargetId": "doc",
                    "Parameters": {
                        AUTOMATION_ASSUME_ROLE: static_parameter(&[AUTOMATION_ROLE_ARN]),
                        "Probe": {
                            "StaticValue": { "Values": ["a"] },
                            "ResourceValue": { "Value": "RESOURCE_ID" },
                        },
                    } }),
        ),
        (
            "a parameter selecting neither kind of value",
            "InvalidParameterValueException",
            json!({ "ConfigRuleName": rule, "TargetType": SSM_DOCUMENT, "TargetId": "doc",
                    "Parameters": {
                        AUTOMATION_ASSUME_ROLE: static_parameter(&[AUTOMATION_ROLE_ARN]),
                        "Probe": {},
                    } }),
        ),
    ];

    for (case, expected, configuration) in rows {
        let (status, code) = emulator
            .refused(
                "PutRemediationConfigurations",
                &json!({ "RemediationConfigurations": [configuration] }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected a 400 {expected}, got {status} {code}"
        );
        let reported = described(&emulator, &[rule]).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused put persists nothing, got {reported:?}"
        );
    }
}

/// One delete row: what the rule holds before the delete, and the `ResourceType` the
/// delete names.
struct DeleteCase {
    case: &'static str,
    rule: &'static str,
    /// Whether the rule holds a remediation configuration at all before the delete.
    holds: bool,
    /// The `ResourceType` the held configuration remediates.
    stored_resource_type: Option<&'static str>,
    /// The `ResourceType` the delete request names.
    deleted_resource_type: Option<&'static str>,
    /// Whether that delete must take the configuration away.
    deletes: bool,
}

#[tokio::test]
async fn a_deleted_remediation_configuration_is_no_longer_reported() {
    let emulator = Emulator::new();

    // Rows differ along whether the rule holds a configuration, and whether the
    // delete names the resource type that configuration remediates.
    let cases = [
        DeleteCase {
            case: "neither the configuration nor the delete names a resource type",
            rule: "remediation-delete-plain",
            holds: true,
            stored_resource_type: None,
            deleted_resource_type: None,
            deletes: true,
        },
        DeleteCase {
            case: "the delete names the resource type the configuration remediates",
            rule: "remediation-delete-matching-type",
            holds: true,
            stored_resource_type: Some(RESOURCE_TYPES[0]),
            deleted_resource_type: Some(RESOURCE_TYPES[0]),
            deletes: true,
        },
        DeleteCase {
            case: "the delete names no resource type, the configuration does",
            rule: "remediation-delete-any-type",
            holds: true,
            stored_resource_type: Some(RESOURCE_TYPES[1]),
            deleted_resource_type: None,
            deletes: true,
        },
        DeleteCase {
            case: "the delete names another resource type",
            rule: "remediation-delete-other-type",
            holds: true,
            stored_resource_type: Some(RESOURCE_TYPES[2]),
            deleted_resource_type: Some(RESOURCE_TYPES[0]),
            deletes: false,
        },
        DeleteCase {
            case: "the rule holds no remediation configuration",
            rule: "remediation-delete-absent",
            holds: false,
            stored_resource_type: None,
            deleted_resource_type: None,
            deletes: false,
        },
    ];

    let rules: Vec<&str> = cases.iter().map(|case| case.rule).collect();
    with_rules(&emulator, &rules).await;

    for case in &cases {
        if case.holds {
            let mut configuration = configuration_for(case.rule);
            if let Some(resource_type) = case.stored_resource_type {
                configuration["ResourceType"] = json!(resource_type);
            }
            put_all(&emulator, vec![configuration]).await;
        }

        let mut request = json!({ "ConfigRuleName": case.rule });
        if let Some(resource_type) = case.deleted_resource_type {
            request["ResourceType"] = json!(resource_type);
        }

        if case.deletes {
            emulator
                .ok("DeleteRemediationConfiguration", &request)
                .await;
        } else {
            let (status, code) = emulator
                .refused("DeleteRemediationConfiguration", &request)
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NoSuchRemediationConfigurationException"),
                "case {:?}: expected a 400 NoSuchRemediationConfigurationException, got \
                 {status} {code}",
                case.case
            );
        }

        // The describe is the record of what the delete did: the configuration is gone
        // exactly when the delete took it away, and a second delete of a configuration
        // that is gone is refused.
        let reported = described(&emulator, &[case.rule]).await;
        assert_eq!(
            reported.len(),
            usize::from(case.holds && !case.deletes),
            "case {:?}: expected the rule to hold {} configuration after the delete, got \
             {reported:?}",
            case.case,
            if case.holds && !case.deletes {
                "its"
            } else {
                "no"
            }
        );

        if case.deletes {
            let (status, code) = emulator
                .refused("DeleteRemediationConfiguration", &request)
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NoSuchRemediationConfigurationException"),
                "case {:?}: a second delete finds nothing left, got {status} {code}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_config_rule_cannot_be_deleted_while_it_holds_a_remediation_configuration() {
    let emulator = Emulator::new();
    let rule = "remediation-attached-rule";
    let bare = "remediation-unattached-rule";
    with_rules(&emulator, &[rule, bare]).await;
    put_all(&emulator, vec![configuration_for(rule)]).await;

    // Rows differ along whether the rule holds a remediation configuration at the
    // moment of the delete: only the attached rule is refused, and the same rule
    // deletes once its remediation action is gone.
    let (status, code) = emulator
        .refused("DeleteConfigRule", &json!({ "ConfigRuleName": rule }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceInUseException"),
        "a rule with a remediation action associated cannot be deleted, got {status} {code}"
    );

    // The refusal left both the rule and its remediation configuration in place.
    let reported = described(&emulator, &[rule]).await;
    assert_eq!(
        reported.len(),
        1,
        "the refused delete keeps the remediation configuration, got {reported:?}"
    );
    emulator.ok("PutConfigRule", &managed_rule(rule)).await;

    // A rule holding no remediation configuration deletes, and so does the attached
    // one once its configuration is deleted — which is what keeps a remediation
    // configuration from outliving its rule.
    emulator
        .ok("DeleteConfigRule", &json!({ "ConfigRuleName": bare }))
        .await;
    emulator
        .ok(
            "DeleteRemediationConfiguration",
            &json!({ "ConfigRuleName": rule }),
        )
        .await;
    emulator
        .ok("DeleteConfigRule", &json!({ "ConfigRuleName": rule }))
        .await;
    let reported = described(&emulator, &[rule, bare]).await;
    assert!(
        reported.is_empty(),
        "no remediation configuration outlives its Config rule, got {reported:?}"
    );
}
