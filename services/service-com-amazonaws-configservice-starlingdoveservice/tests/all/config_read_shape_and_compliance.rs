// SPDX-License-Identifier: CC0-1.0

//! What a Config READ reports: the configuration the service materialised, the ARNs
//! it minted, and the compliance roll-up it computes.
//!
//! Three rules are pinned here, each over a table of inputs rather than one recorded
//! case:
//!
//! 1. a read of a resource reports every member the service derives or defaults at
//!    write time with a concrete value — for a configuration recorder, the recording
//!    strategy implied by its recording group, an empty exclusion list, the recording
//!    frequency and the billing scope;
//! 2. an ARN names the resource type the service spells and every identifying segment
//!    the service assigned — for a configuration recorder,
//!    `configuration-recorder/<name>/<id>`;
//! 3. a read reports what the service computes from state, not the request: the rule
//!    evaluation visibility Config decides, a withheld Guard policy body, and the
//!    compliance roll-up over the rules in state.
//!
//! The harness (`Emulator`, `recorder_request`, `managed_rule`) is the one
//! `config_setup_and_rule_loop` already builds; every case drives the emulator the way
//! a client does, through the `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{
    managed_rule, recorder_request, with_recorder_and_channel, Emulator, ROLE_ARN,
};

/// Every published `RecordingStrategyType` value, from
/// `smithy_query(query="shape", shape="RecordingStrategyType")`.
const ALL_SUPPORTED: &str = "ALL_SUPPORTED_RESOURCE_TYPES";
/// See [`ALL_SUPPORTED`].
const INCLUSION: &str = "INCLUSION_BY_RESOURCE_TYPES";
/// See [`ALL_SUPPORTED`].
const EXCLUSION: &str = "EXCLUSION_BY_RESOURCE_TYPES";

/// Every published `RecordingFrequency` value, from
/// `smithy_query(query="shape", shape="RecordingFrequency")`: `CONTINUOUS`, `DAILY`.
const CONTINUOUS: &str = "CONTINUOUS";
/// See [`CONTINUOUS`].
const DAILY: &str = "DAILY";

/// Every published `RecordingScope` value, from
/// `smithy_query(query="shape", shape="RecordingScope")`: `INTERNAL`, `PAID`.
const INTERNAL_SCOPE: &str = "INTERNAL";
/// See [`INTERNAL_SCOPE`].
const PAID_SCOPE: &str = "PAID";

/// Every published `RuleEvaluationVisibility` value, from
/// `smithy_query(query="shape", shape="RuleEvaluationVisibility")`: `EXTERNAL`,
/// `INTERNAL`.
const EXTERNAL_VISIBILITY: &str = "EXTERNAL";
/// See [`EXTERNAL_VISIBILITY`].
const INTERNAL_VISIBILITY: &str = "INTERNAL";

/// Resource types used by the recording-group rows. `RecordingGroup$resourceTypes`
/// points at the "Resource Type Value" column of *Supported Amazon Web Services
/// resource Types*; these are entries from it, deliberately including ones outside the
/// single type the issue's recording used.
const RESOURCE_TYPES: [&str; 3] = [
    "AWS::DynamoDB::Table",
    "AWS::EC2::Volume",
    "AWS::IAM::Role",
];

/// The cap a `ComplianceSummary` counts Config rules up to
/// (`ComplianceSummary$CompliantResourceCount`: "up to a maximum of 25 for rules").
const RULE_CAP: u64 = 25;

/// The single recorder of an emulator, read back through
/// `DescribeConfigurationRecorders`.
async fn described_recorder(emulator: &Emulator) -> Value {
    let described = emulator
        .ok("DescribeConfigurationRecorders", &json!({}))
        .await;
    described["ConfigurationRecorders"][0].clone()
}

/// The `<name>/<id>` tail of a configuration recorder ARN, with the ARN's six
/// colon-separated fields checked first.
fn recorder_arn_tail(case: &str, arn: &str) -> (String, String) {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected an ARN of six colon-separated fields, got {arn:?}"
    );
    assert_eq!(
        (fields[0], fields[2]),
        ("arn", "config"),
        "case {case:?}: expected an arn:*:config:… ARN, got {arn:?}"
    );
    let resource = fields[5];
    let mut segments = resource.split('/');
    let resource_type = segments.next().unwrap_or_default();
    assert_eq!(
        resource_type, "configuration-recorder",
        "case {case:?}: expected the resource type 'configuration-recorder', got {arn:?}"
    );
    let name = segments.next().unwrap_or_default().to_string();
    let id = segments.next().unwrap_or_default().to_string();
    assert!(
        segments.next().is_none(),
        "case {case:?}: expected exactly configuration-recorder/<name>/<id>, got {arn:?}"
    );
    (name, id)
}

#[tokio::test]
async fn a_recorder_reports_the_configuration_the_service_materialised() {
    // Rows differ along what the rule generalises over: which recording strategy the
    // group implies (absent, all-supported, an inclusion list of one and of three, an
    // exclusion list), and whether the request asked for a frequency, an override or a
    // scope of its own.
    struct Case {
        name: &'static str,
        recorder: Value,
        strategy: &'static str,
        excluded: Value,
        frequency: &'static str,
        overrides: Value,
        scope: &'static str,
    }

    let cases = [
        Case {
            name: "no recording group at all",
            recorder: json!({ "name": "no-group", "roleARN": ROLE_ARN }),
            strategy: ALL_SUPPORTED,
            excluded: json!([]),
            frequency: CONTINUOUS,
            overrides: json!([]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "all supported resource types",
            recorder: json!({
                "name": "all-supported",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": true },
            }),
            strategy: ALL_SUPPORTED,
            excluded: json!([]),
            frequency: CONTINUOUS,
            overrides: json!([]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "an inclusion list of one resource type",
            recorder: json!({
                "name": "include-one",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "resourceTypes": [RESOURCE_TYPES[0]] },
            }),
            strategy: INCLUSION,
            excluded: json!([]),
            frequency: CONTINUOUS,
            overrides: json!([]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "an inclusion list of three resource types",
            recorder: json!({
                "name": "include-three",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "resourceTypes": RESOURCE_TYPES },
            }),
            strategy: INCLUSION,
            excluded: json!([]),
            frequency: CONTINUOUS,
            overrides: json!([]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "an exclusion list",
            recorder: json!({
                "name": "exclude",
                "roleARN": ROLE_ARN,
                "recordingGroup": {
                    "exclusionByResourceTypes": { "resourceTypes": [RESOURCE_TYPES[2]] },
                },
            }),
            strategy: EXCLUSION,
            excluded: json!([RESOURCE_TYPES[2]]),
            frequency: CONTINUOUS,
            overrides: json!([]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "a strategy the request stated itself",
            recorder: json!({
                "name": "stated-strategy",
                "roleARN": ROLE_ARN,
                "recordingGroup": {
                    "resourceTypes": [RESOURCE_TYPES[1]],
                    "recordingStrategy": { "useOnly": INCLUSION },
                },
            }),
            strategy: INCLUSION,
            excluded: json!([]),
            frequency: CONTINUOUS,
            overrides: json!([]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "daily recording",
            recorder: json!({
                "name": "daily",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": true },
                "recordingMode": { "recordingFrequency": DAILY },
            }),
            strategy: ALL_SUPPORTED,
            excluded: json!([]),
            frequency: DAILY,
            overrides: json!([]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "a recording mode override the request carried",
            recorder: json!({
                "name": "overridden",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": true },
                "recordingMode": {
                    "recordingFrequency": DAILY,
                    "recordingModeOverrides": [{
                        "description": "volumes continuously",
                        "resourceTypes": [RESOURCE_TYPES[1]],
                        "recordingFrequency": CONTINUOUS,
                    }],
                },
            }),
            strategy: ALL_SUPPORTED,
            excluded: json!([]),
            frequency: DAILY,
            overrides: json!([{
                "description": "volumes continuously",
                "resourceTypes": [RESOURCE_TYPES[1]],
                "recordingFrequency": CONTINUOUS,
            }]),
            scope: PAID_SCOPE,
        },
        Case {
            name: "a recording scope the request stated itself",
            recorder: json!({
                "name": "internal-scope",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": true },
                "recordingScope": INTERNAL_SCOPE,
            }),
            strategy: ALL_SUPPORTED,
            excluded: json!([]),
            frequency: CONTINUOUS,
            overrides: json!([]),
            scope: INTERNAL_SCOPE,
        },
    ];

    for case in cases {
        let label = case.name;
        let emulator = Emulator::in_region("eu-central-1");
        emulator
            .ok("PutConfigurationRecorder", &recorder_request(case.recorder))
            .await;
        let recorder = described_recorder(&emulator).await;

        assert_eq!(
            recorder["recordingGroup"]["recordingStrategy"]["useOnly"],
            json!(case.strategy),
            "case {label:?}: expected recordingStrategy.useOnly {:?}, got {recorder}",
            case.strategy
        );
        assert_eq!(
            recorder["recordingGroup"]["exclusionByResourceTypes"]["resourceTypes"],
            case.excluded,
            "case {label:?}: expected exclusionByResourceTypes.resourceTypes {}, got {recorder}",
            case.excluded
        );
        assert_eq!(
            recorder["recordingMode"]["recordingFrequency"],
            json!(case.frequency),
            "case {label:?}: expected recordingMode.recordingFrequency {:?}, got {recorder}",
            case.frequency
        );
        assert_eq!(
            recorder["recordingMode"]["recordingModeOverrides"], case.overrides,
            "case {label:?}: expected recordingMode.recordingModeOverrides {}, got {recorder}",
            case.overrides
        );
        assert_eq!(
            recorder["recordingScope"],
            json!(case.scope),
            "case {label:?}: expected recordingScope {:?}, got {recorder}",
            case.scope
        );

        // A read reports state; it must not change what the next read sees.
        let again = described_recorder(&emulator).await;
        assert_eq!(
            again, recorder,
            "case {label:?}: a second describe must report the same recorder, got {again}"
        );
    }
}

#[tokio::test]
async fn a_recorder_arn_names_the_resource_type_and_the_assigned_id() {
    // Rows differ along the Region the request is signed for and whether the recorder
    // name is defaulted or stated.
    let cases: [(&str, &str, Option<&str>, &str); 3] = [
        ("defaulted name", "us-west-2", None, "default"),
        ("stated name", "us-west-2", Some("probe-recorder"), "probe-recorder"),
        ("another Region", "ap-northeast-1", Some("tokyo"), "tokyo"),
    ];

    for (case, region, requested_name, expected_name) in cases {
        let emulator = Emulator::in_region(region);
        let mut recorder = json!({ "roleARN": ROLE_ARN });
        if let Some(name) = requested_name {
            recorder["name"] = json!(name);
        }
        emulator
            .ok("PutConfigurationRecorder", &recorder_request(recorder))
            .await;

        let described = described_recorder(&emulator).await;
        let arn = described["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {described}"))
            .to_string();
        assert!(
            arn.contains(&format!(":{region}:")),
            "case {case:?}: expected the ARN to carry the request's Region {region:?}, got {arn:?}"
        );
        let (name, id) = recorder_arn_tail(case, &arn);
        assert_eq!(
            name, expected_name,
            "case {case:?}: expected the ARN to name the recorder {expected_name:?}, got {arn:?}"
        );
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case:?}: expected an assigned recorder id of lowercase base-36 \
             characters, got {id:?}"
        );

        // The id is the recorder's identity, so it survives an update and every read.
        emulator
            .ok(
                "PutConfigurationRecorder",
                &recorder_request(json!({
                    "name": expected_name,
                    "roleARN": "arn:aws:iam::000000000000:role/other-config-role",
                })),
            )
            .await;
        let after = described_recorder(&emulator).await;
        assert_eq!(
            after["arn"],
            json!(arn),
            "case {case:?}: the recorder keeps the ARN it was created with, got {after}"
        );

        // The status record reports the same ARN, so the two reads agree.
        let status = emulator
            .ok("DescribeConfigurationRecorderStatus", &json!({}))
            .await;
        assert_eq!(
            status["ConfigurationRecordersStatus"][0]["arn"],
            json!(arn),
            "case {case:?}: the status must report the recorder's ARN, got {status}"
        );

        // And the ARN selects that recorder.
        let by_arn = emulator
            .ok("DescribeConfigurationRecorders", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            by_arn["ConfigurationRecorders"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {case:?}: the minted ARN must select the recorder, got {by_arn}"
        );
    }
}

#[tokio::test]
async fn described_rules_report_the_evaluation_visibility_and_withhold_the_policy_body() {
    // Rows differ along the rule's source owner and, for a Guard policy rule, the size
    // and shape of the policy body; the last row states a visibility of its own, which
    // Config decides rather than echoes.
    let cases: [(&str, Value, Option<&str>); 5] = [
        (
            "an Config managed rule",
            json!({
                "ConfigRuleName": "managed",
                "Source": { "Owner": "AWS", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
            }),
            None,
        ),
        (
            "a custom Lambda rule",
            json!({
                "ConfigRuleName": "lambda",
                "Source": {
                    "Owner": "CUSTOM_LAMBDA",
                    "SourceIdentifier":
                        "arn:aws:lambda:us-west-2:000000000000:function:evaluate",
                },
            }),
            None,
        ),
        (
            "a Guard policy rule with a one-line policy",
            json!({
                "ConfigRuleName": "guard-short",
                "Source": {
                    "Owner": "CUSTOM_POLICY",
                    "CustomPolicyDetails": {
                        "PolicyRuntime": "guard-2.x.x",
                        "PolicyText": "rule always { true }",
                    },
                },
            }),
            Some("rule always { true }"),
        ),
        (
            "a Guard policy rule with a multi-line policy",
            json!({
                "ConfigRuleName": "guard-long",
                "Source": {
                    "Owner": "CUSTOM_POLICY",
                    "CustomPolicyDetails": {
                        "PolicyRuntime": "guard-2.x.x",
                        "PolicyText":
                            "rule tagged when resourceType == 'AWS::DynamoDB::Table' {\n  \
                             configuration.tags != null\n}\n",
                        "EnableDebugLogDelivery": true,
                    },
                },
            }),
            Some(
                "rule tagged when resourceType == 'AWS::DynamoDB::Table' {\n  \
                 configuration.tags != null\n}\n",
            ),
        ),
        (
            "a rule whose request states INTERNAL visibility",
            json!({
                "ConfigRuleName": "states-visibility",
                "Source": { "Owner": "AWS", "SourceIdentifier": "IAM_PASSWORD_POLICY" },
                "RuleEvaluationVisibility": INTERNAL_VISIBILITY,
            }),
            None,
        ),
    ];

    for (case, rule, sent_policy) in cases {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        let name = rule["ConfigRuleName"]
            .as_str()
            .expect("every case names its rule")
            .to_string();
        emulator
            .ok("PutConfigRule", &json!({ "ConfigRule": rule.clone() }))
            .await;

        let described = emulator
            .ok("DescribeConfigRules", &json!({ "ConfigRuleNames": [name] }))
            .await;
        let stored = &described["ConfigRules"][0];

        assert_eq!(
            stored["RuleEvaluationVisibility"],
            json!(EXTERNAL_VISIBILITY),
            "case {case:?}: expected RuleEvaluationVisibility {EXTERNAL_VISIBILITY:?} \
             (a rule of this account has retrievable Evaluations), got {described}"
        );

        match sent_policy {
            None => assert!(
                stored["Source"]["CustomPolicyDetails"].is_null(),
                "case {case:?}: a rule with no CustomPolicyDetails must report none, \
                 got {described}"
            ),
            Some(sent) => {
                assert_eq!(
                    stored["Source"]["CustomPolicyDetails"]["PolicyText"],
                    json!(""),
                    "case {case:?}: DescribeConfigRules must not hand back the {} byte \
                     policy body, got {described}",
                    sent.len()
                );
                assert_eq!(
                    stored["Source"]["CustomPolicyDetails"]["PolicyRuntime"],
                    json!("guard-2.x.x"),
                    "case {case:?}: the policy runtime must still be reported, got {described}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_compliance_summary_is_computed_over_the_rules_in_state() {
    // Rows differ along how many rules the account holds: none at all, one, a handful,
    // exactly the cap the summary counts rules up to, and one past it.
    for rules in [0_u64, 1, 4, RULE_CAP, RULE_CAP + 1] {
        let case = format!("{rules} rules");
        let emulator = Emulator::new();
        if rules > 0 {
            with_recorder_and_channel(&emulator).await;
        }
        for index in 0..rules {
            emulator
                .ok("PutConfigRule", &managed_rule(&format!("rule-{index:03}")))
                .await;
        }

        let (status, body) = emulator
            .call("GetComplianceSummaryByConfigRule", &json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetComplianceSummaryByConfigRule is a modeled 200, got \
             {status} with {body}"
        );
        let summary = &body["ComplianceSummary"];
        assert!(
            summary.is_object(),
            "case {case:?}: expected a ComplianceSummary, got {body}"
        );

        // Nothing has evaluated any of these rules — no operation of this emulator can
        // record an evaluation result yet — so every rule is INSUFFICIENT_DATA and
        // contributes to neither count, under the cap and with CapExceeded clear.
        for member in ["CompliantResourceCount", "NonCompliantResourceCount"] {
            assert_eq!(
                summary[member]["CappedCount"],
                json!(0),
                "case {case:?}: expected {member}.CappedCount 0 for rules nothing has \
                 evaluated, got {body}"
            );
            assert_eq!(
                summary[member]["CapExceeded"],
                json!(false),
                "case {case:?}: expected {member}.CapExceeded false under the cap of \
                 {RULE_CAP}, got {body}"
            );
        }
        let at = summary["ComplianceSummaryTimestamp"].as_f64();
        assert!(
            at.is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: expected a ComplianceSummaryTimestamp, got {body}"
        );

        // The summary is a report: it must not change the rules a later read sees.
        let before = emulator.ok("DescribeConfigRules", &json!({})).await;
        emulator
            .ok("GetComplianceSummaryByConfigRule", &json!({}))
            .await;
        let after = emulator.ok("DescribeConfigRules", &json!({})).await;
        assert_eq!(
            after, before,
            "case {case:?}: summarising compliance must not change the rules, got {after}"
        );
    }
}
