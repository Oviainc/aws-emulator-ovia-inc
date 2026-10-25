// SPDX-License-Identifier: CC0-1.0

//! `GetComplianceSummaryByResourceType`: how many of the account's resources are
//! compliant and how many are noncompliant, by resource type.
//!
//! The rule pinned here: the operation reports, for every resource type the request
//! names — and in a SINGLE summary carrying no resource type at all when it names none
//! — how many distinct resources of that type the account's Config rules have placed at
//! `COMPLIANT` and how many at `NON_COMPLIANT`, rolling the verdicts of every rule that
//! evaluated a resource into one verdict per resource, counting each resource once,
//! counting neither for a resource no verdict places either way, and reporting each
//! count up to the hundred-resource cap a `ComplianceSummary` reports contributors up
//! to with `CapExceeded` set exactly past it.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along which resource types the request
//! names and how many (none, one, several, the same one twice, one the account has
//! nothing of, and the modelled maximum of twenty), which compliance each resource was
//! placed at, how many rules placed the same resource, and how many resources of a type
//! there are around the published cap.
//!
//! Recording an evaluation result for a MANAGED rule is something Config does itself
//! from the configuration state it recorded, which this emulator does not evaluate, so
//! the rows that need results already recorded seed them into the same shared state a
//! handler reads — the state a delivery writes. The last case takes the other path and
//! DELIVERS its verdicts over the wire with `PutEvaluations`, so the whole path from a
//! delivered verdict to the summary that counts it is covered end to end. Everything
//! else goes over the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    EvaluationResultEntry, EVALUATION_MODE_DETECTIVE, EVALUATION_RESULTS_KEPT_PER_RULE,
};
use service_com_amazonaws_configservice_starlingdoveservice::ComAmazonawsConfigserviceStarlingdoveserviceState as ConfigState;

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};
use crate::put_evaluations::{custom_lambda_rule, result_token};

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`]. `INSUFFICIENT_DATA`, the fourth published value, is not one a
/// rule's own evaluation may deliver, so no case below names it.
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// Resource types a verdict names, from
/// `smithy_query(query="shape", shape="ResourceType")`; `AWS::::Account` is the
/// account-level type Config accepts for an evaluation of the account itself
/// (`GetComplianceSummaryByResourceTypeRequest$ResourceTypes`: "You can specify that
/// the resource type is an Amazon Web Services account by specifying AWS::::Account").
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const ACCOUNT: &str = "AWS::::Account";
/// See [`EC2_VOLUME`]. A type no case places a resource of, which the rows that ask for
/// a type the account has nothing of name.
const EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// Twenty published `ResourceType` values, from the same model query, so a row can ask
/// for the modelled maximum of twenty (`ResourceTypes`, `@length(min: 0, max: 20)`).
/// The three types the seeded verdicts place resources of are among them, so the row
/// covers both counted and empty types.
const TWENTY_RESOURCE_TYPES: [&str; 20] = [
    "AWS::ACM::Certificate",
    "AWS::AccessAnalyzer::Analyzer",
    "AWS::ApiGateway::RestApi",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::Backup::BackupPlan",
    "AWS::CloudFront::Distribution",
    "AWS::CloudTrail::Trail",
    "AWS::CloudWatch::Alarm",
    "AWS::DynamoDB::Table",
    "AWS::EC2::Instance",
    "AWS::EC2::SecurityGroup",
    "AWS::EC2::Volume",
    "AWS::ECS::Cluster",
    "AWS::EFS::FileSystem",
    "AWS::IAM::Policy",
    "AWS::IAM::Role",
    "AWS::KMS::Key",
    "AWS::Lambda::Function",
    "AWS::S3::Bucket",
    "AWS::::Account",
];

/// The cap a `ComplianceSummary` counts RESOURCES up to
/// (`ComplianceSummary$CompliantResourceCount`: "The number of Config rules or Amazon
/// Web Services resources that are compliant, up to a maximum of 25 for rules and 100
/// for resources"), which is what this operation counts.
const RESOURCE_CAP: usize = 100;

/// How many evaluation results one Config rule keeps — the store's own bound, taken
/// from it rather than copied, so the chunking below follows it if it ever changes. It
/// is what makes a row needing more verdicts than that spread them over several rules.
const RESULTS_PER_RULE: usize = EVALUATION_RESULTS_KEPT_PER_RULE;

/// One verdict a rule recorded: the type and id of the resource it is about, and the
/// compliance it places that resource at.
type Verdict = (&'static str, String, &'static str);

/// One Config rule a case seeds: its name and the verdicts it recorded.
type Seeded = (String, Vec<Verdict>);

/// The `(CappedCount, CapExceeded)` of a reported `ComplianceContributorCount`.
type Counts = (i64, bool);

/// One summary a `GetComplianceSummaryByResourceType` response carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    /// The `ResourceType` the summary is about, absent on the one summary that covers
    /// every resource type.
    resource_type: Option<String>,
    compliant: Counts,
    non_compliant: Counts,
}

/// Record one evaluation result per entry of `verdicts` against the rule `name`,
/// through the bounded writer the store exposes.
fn record_verdicts(emulator: &Emulator, name: &str, verdicts: &[Verdict]) {
    let state = emulator.engine.state::<ConfigState>();
    let mut store = state.lock();
    let rule = store
        .config_rules
        .get_mut(name)
        .unwrap_or_else(|| panic!("the rule {name:?} must have been created first"));
    for (resource_type, resource_id, compliance) in verdicts {
        rule.record_evaluation_result(EvaluationResultEntry::new(
            resource_type,
            resource_id,
            EVALUATION_MODE_DETECTIVE,
            compliance,
            SystemTime::UNIX_EPOCH,
        ));
    }
}

/// An emulator holding one managed Config rule per name in `rules`, each carrying the
/// verdicts that entry names.
async fn emulator_holding(rules: &[Seeded]) -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for (name, verdicts) in rules {
        emulator.ok("PutConfigRule", &managed_rule(name)).await;
        record_verdicts(&emulator, name, verdicts);
    }
    emulator
}

/// The `ComplianceContributorCount` member `name` of `summary`, which a response
/// carries as a `CappedCount` and a `CapExceeded`.
fn counts(case: &str, summary: &Value, name: &str) -> Counts {
    let count = &summary[name];
    (
        count["CappedCount"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a {name} CappedCount, got {summary}")),
        count["CapExceeded"]
            .as_bool()
            .unwrap_or_else(|| panic!("case {case:?}: expected a {name} CapExceeded, got {summary}")),
    )
}

/// What a `GetComplianceSummaryByResourceType` response reports, one [`Summary`] per
/// `ComplianceSummariesByResourceType` member, in the order the response carried them.
///
/// Every summary must carry the instant Config created it
/// (`ComplianceSummary$ComplianceSummaryTimestamp`), which is checked here rather than
/// compared: it is the time of the call, not a value of the state read.
fn reported(case: &str, body: &Value) -> Vec<Summary> {
    let summaries = body["ComplianceSummariesByResourceType"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected ComplianceSummariesByResourceType, got {body}")
        });
    summaries
        .iter()
        .map(|entry| {
            let summary = &entry["ComplianceSummary"];
            assert!(
                summary["ComplianceSummaryTimestamp"].is_number(),
                "case {case:?}: every summary carries the instant it was created, got {entry}"
            );
            Summary {
                resource_type: entry
                    .get("ResourceType")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case:?}: expected a ResourceType string, got {entry}")
                            })
                            .to_string()
                    }),
                compliant: counts(case, summary, "CompliantResourceCount"),
                non_compliant: counts(case, summary, "NonCompliantResourceCount"),
            }
        })
        .collect()
}

/// The summaries the operation reports for `request`.
async fn summaries(case: &str, emulator: &Emulator, request: &Value) -> Vec<Summary> {
    let body = emulator
        .ok("GetComplianceSummaryByResourceType", request)
        .await;
    reported(case, &body)
}

/// The resource types the seeded scenario below places resources of, with the number of
/// them that roll up to `COMPLIANT` and to `NON_COMPLIANT`. Every other resource type
/// has nothing placed of it at all.
const SEEDED_COUNTS: [(&str, i64, i64); 3] = [(EC2_VOLUME, 1, 1), (S3_BUCKET, 1, 1), (ACCOUNT, 0, 1)];

/// The summary the seeded scenario owes for the resource type `resource_type`.
fn expected_for(resource_type: &str) -> Summary {
    let (compliant, non_compliant) = SEEDED_COUNTS
        .iter()
        .find(|(seeded, _, _)| *seeded == resource_type)
        .map_or((0, 0), |(_, compliant, non_compliant)| {
            (*compliant, *non_compliant)
        });
    Summary {
        resource_type: Some(resource_type.to_string()),
        compliant: (compliant, false),
        non_compliant: (non_compliant, false),
    }
}

/// The rules the scenario seeds, and the verdicts each recorded.
///
/// Two rules place the bucket `logs` — one compliant, one noncompliant — so the row
/// covers a resource whose verdict comes from rolling SEVERAL rules' verdicts into one;
/// a volume is placed `NOT_APPLICABLE` and a rule records nothing at all, so both the
/// resource no verdict places and the rule that placed nothing are covered.
fn seeded_rules() -> Vec<Seeded> {
    vec![
        (
            "volumes".to_string(),
            vec![
                (EC2_VOLUME, "vol-0001".to_string(), COMPLIANT),
                (EC2_VOLUME, "vol-0002".to_string(), NON_COMPLIANT),
                (EC2_VOLUME, "vol-0003".to_string(), NOT_APPLICABLE),
                (S3_BUCKET, "logs".to_string(), COMPLIANT),
            ],
        ),
        (
            "buckets-and-account".to_string(),
            vec![
                (S3_BUCKET, "logs".to_string(), NON_COMPLIANT),
                (S3_BUCKET, "data".to_string(), COMPLIANT),
                (ACCOUNT, "000000000000".to_string(), NON_COMPLIANT),
            ],
        ),
        ("unevaluated".to_string(), Vec::new()),
    ]
}

#[tokio::test]
async fn every_resource_type_the_request_names_is_summarised_over_its_own_resources() {
    // Rows differ along which resource types the request names: none at all, one, one
    // the account has nothing of, several in the order they were named, the same one
    // twice, every placed type beside an unplaced one, and the modelled maximum of
    // twenty.
    struct Case {
        case: &'static str,
        resource_types: Option<&'static [&'static str]>,
    }

    let cases = [
        Case {
            case: "no resource types at all",
            resource_types: None,
        },
        Case {
            case: "one resource type",
            resource_types: Some(&[EC2_VOLUME]),
        },
        Case {
            case: "a resource type the account has nothing of",
            resource_types: Some(&[EC2_INSTANCE]),
        },
        Case {
            case: "several resource types",
            resource_types: Some(&[S3_BUCKET, ACCOUNT, EC2_VOLUME]),
        },
        Case {
            case: "the same resource type twice",
            resource_types: Some(&[ACCOUNT, ACCOUNT]),
        },
        Case {
            case: "every placed type beside an unplaced one",
            resource_types: Some(&[EC2_VOLUME, EC2_INSTANCE, S3_BUCKET, ACCOUNT]),
        },
        Case {
            case: "the modelled maximum of twenty resource types",
            resource_types: Some(&TWENTY_RESOURCE_TYPES),
        },
    ];

    let emulator = emulator_holding(&seeded_rules()).await;

    for row in cases {
        let case = row.case;
        let request = match row.resource_types {
            Some(resource_types) => json!({ "ResourceTypes": resource_types }),
            None => json!({}),
        };

        // What the row owes: one summary per resource type it named, once each and in
        // the order it named them, or — where it named none — the single summary over
        // every resource type, which belongs to no one type and carries none.
        let expected: Vec<Summary> = match row.resource_types {
            None => vec![Summary {
                resource_type: None,
                compliant: (
                    SEEDED_COUNTS.iter().map(|(_, compliant, _)| compliant).sum(),
                    false,
                ),
                non_compliant: (
                    SEEDED_COUNTS
                        .iter()
                        .map(|(_, _, non_compliant)| non_compliant)
                        .sum(),
                    false,
                ),
            }],
            Some(resource_types) => {
                let mut named: Vec<&str> = Vec::new();
                for resource_type in resource_types {
                    if !named.contains(resource_type) {
                        named.push(resource_type);
                    }
                }
                named.into_iter().map(expected_for).collect()
            }
        };

        let actual = summaries(case, &emulator, &request).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the summaries {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_report_is_a_read_that_changes_nothing_a_later_caller_sees() {
    let emulator = emulator_holding(&seeded_rules()).await;
    let request = json!({ "ResourceTypes": [EC2_VOLUME, S3_BUCKET] });

    // Rows differ along which request the report is asked for: both a narrowed one and
    // the one over every resource type.
    for (case, request) in [
        ("a narrowed report", request),
        ("a report over every resource type", json!({})),
    ] {
        let details_before = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": "volumes" }),
            )
            .await;
        let first = summaries(case, &emulator, &request).await;
        let again = summaries(case, &emulator, &request).await;
        let details_after = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": "volumes" }),
            )
            .await;

        assert_eq!(
            again, first,
            "case {case:?}: a second report must say the same, got {again:?}"
        );
        assert_eq!(
            details_after, details_before,
            "case {case:?}: reporting the summary must not change the recorded results, \
             got {details_after}"
        );
    }
}

#[tokio::test]
async fn each_count_is_reported_up_to_the_published_resource_cap() {
    // Rows differ along how many resources of the type are placed noncompliant: none,
    // one, a handful, one short of the published cap, the cap itself, one past it and
    // well past it. Each row places three compliant resources beside them, so the
    // capped count and the uncapped one are both checked on every row.
    let compliant_resources = 3usize;
    for non_compliant_resources in [0usize, 1, 7, RESOURCE_CAP - 1, RESOURCE_CAP, RESOURCE_CAP + 1, 150]
    {
        let case = format!("{non_compliant_resources} noncompliant resources");

        // A rule keeps at most `RESULTS_PER_RULE` results, so the verdicts are spread
        // over as many rules as it takes; a resource is counted once however many rules
        // placed it, and here each one is placed by exactly one rule.
        let mut verdicts: Vec<Verdict> = (0..non_compliant_resources)
            .map(|index| (EC2_VOLUME, format!("vol-{index:04}"), NON_COMPLIANT))
            .collect();
        verdicts.extend(
            (0..compliant_resources)
                .map(|index| (EC2_VOLUME, format!("ok-vol-{index:04}"), COMPLIANT)),
        );
        let rules: Vec<Seeded> = verdicts
            .chunks(RESULTS_PER_RULE)
            .enumerate()
            .map(|(index, chunk)| (format!("rule-{index}"), chunk.to_vec()))
            .collect();
        let emulator = emulator_holding(&rules).await;

        let expected = vec![Summary {
            resource_type: Some(EC2_VOLUME.to_string()),
            compliant: (
                i64::try_from(compliant_resources).expect("a seeded count fits"),
                false,
            ),
            non_compliant: (
                i64::try_from(non_compliant_resources.min(RESOURCE_CAP)).expect("a capped count fits"),
                non_compliant_resources > RESOURCE_CAP,
            ),
        }];
        let actual = summaries(
            &case,
            &emulator,
            &json!({ "ResourceTypes": [EC2_VOLUME] }),
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the counts {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn verdicts_delivered_over_the_wire_are_counted_by_the_summary() {
    // Rows differ along the compliance each delivered verdict reports and how many
    // resources of each type it is about, and name the counts the summary then owes per
    // resource type.
    struct Case {
        case: &'static str,
        /// `(resourceType, resourceId, ComplianceType)` per delivered verdict.
        delivered: &'static [(&'static str, &'static str, &'static str)],
        /// `(resourceType, compliant, noncompliant)` the summary then owes.
        expected: &'static [(&'static str, i64, i64)],
        /// The counts the summary over EVERY resource type then owes.
        total: (i64, i64),
    }

    let cases = [
        Case {
            case: "nothing delivered at all",
            delivered: &[],
            expected: &[(EC2_VOLUME, 0, 0)],
            total: (0, 0),
        },
        Case {
            case: "one noncompliant resource",
            delivered: &[(EC2_VOLUME, "vol-0001", NON_COMPLIANT)],
            expected: &[(EC2_VOLUME, 0, 1)],
            total: (0, 1),
        },
        Case {
            case: "resources of two types, both compliance types",
            delivered: &[
                (EC2_VOLUME, "vol-0001", COMPLIANT),
                (EC2_VOLUME, "vol-0002", NON_COMPLIANT),
                (S3_BUCKET, "logs", COMPLIANT),
            ],
            expected: &[(EC2_VOLUME, 1, 1), (S3_BUCKET, 1, 0)],
            total: (2, 1),
        },
        Case {
            case: "a verdict that applies to nothing counts for neither",
            delivered: &[
                (EC2_VOLUME, "vol-0001", NOT_APPLICABLE),
                (EC2_VOLUME, "vol-0002", NOT_APPLICABLE),
                (ACCOUNT, "000000000000", NON_COMPLIANT),
            ],
            expected: &[(EC2_VOLUME, 0, 0), (ACCOUNT, 0, 1)],
            total: (0, 1),
        },
    ];

    for row in cases {
        let case = row.case;
        let name = "delivering-rule";
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        emulator.ok("PutConfigRule", &custom_lambda_rule(name)).await;

        if !row.delivered.is_empty() {
            let evaluations: Vec<Value> = row
                .delivered
                .iter()
                .enumerate()
                .map(|(index, (resource_type, resource_id, compliance))| {
                    json!({
                        "ComplianceResourceType": resource_type,
                        "ComplianceResourceId": resource_id,
                        "ComplianceType": compliance,
                        "OrderingTimestamp": 1_700_000_000u64 + index as u64,
                    })
                })
                .collect();
            let answered = emulator
                .ok(
                    "PutEvaluations",
                    &json!({
                        "ResultToken": result_token(&emulator, name),
                        "Evaluations": evaluations,
                    }),
                )
                .await;
            assert_eq!(
                answered["FailedEvaluations"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: every delivered verdict was recorded, got {answered}"
            );
        }

        let requested: Vec<&str> = row
            .expected
            .iter()
            .map(|(resource_type, _, _)| *resource_type)
            .collect();
        let expected: Vec<Summary> = row
            .expected
            .iter()
            .map(|(resource_type, compliant, non_compliant)| Summary {
                resource_type: Some((*resource_type).to_string()),
                compliant: (*compliant, false),
                non_compliant: (*non_compliant, false),
            })
            .collect();
        let actual = summaries(
            case,
            &emulator,
            &json!({ "ResourceTypes": requested }),
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the summaries {expected:?}, got {actual:?}"
        );

        let total = summaries(case, &emulator, &json!({})).await;
        assert_eq!(
            total,
            vec![Summary {
                resource_type: None,
                compliant: (row.total.0, false),
                non_compliant: (row.total.1, false),
            }],
            "case {case:?}: expected the summary over every resource type to count \
             {:?}, got {total:?}",
            row.total
        );
    }
}
