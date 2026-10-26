// SPDX-License-Identifier: CC0-1.0

//! `DescribeComplianceByResource`: whether the account's resources comply with the
//! Config rules that evaluate them.
//!
//! The rule pinned here: the operation reports one entry per RESOURCE the account's
//! Config rules have recorded an evaluation result for — keyed by the
//! `(resourceType, resourceId)` pair that identifies a resource, so a resource several
//! rules evaluated is reported once — carrying the `ComplianceType` those verdicts roll
//! up to (`NON_COMPLIANT` if any rule placed it so, `COMPLIANT` if at least one did and
//! none placed it noncompliant, `INSUFFICIENT_DATA` where no verdict places it either
//! way) and, where rules place it noncompliant, how many RULES do, counted up to the
//! published rule cap with `CapExceeded` set exactly past it. A request narrows by
//! resource type, by resource type and id together, and by compliance; it is answered a
//! page at a time at the size its `Limit` asks for, with an opaque token resuming the
//! next page; and a resource it identifies that no rule has placed is reported with
//! `INSUFFICIENT_DATA`.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along which resource the request names
//! (none, a type, a type and an id, a type the account has nothing of), which compliance
//! types it filters by and how many, how many rules placed the same resource, how large
//! a page it asks for, and which part of a request is malformed.
//!
//! Recording an evaluation result for a MANAGED rule is something Config does itself
//! from the configuration state it recorded, which this emulator does not evaluate, so
//! the rows that need results already recorded seed them into the same shared state a
//! handler reads — the state a delivery writes. One case takes the other path and
//! DELIVERS its verdicts over the wire with `PutEvaluations`, so the whole path from a
//! delivered verdict to the report that rolls it up is covered end to end.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    EvaluationResultEntry, EVALUATION_MODE_DETECTIVE, EVALUATION_MODE_PROACTIVE,
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
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`]. The value a resource rolls up to when no verdict places it either
/// way, and the one value no filter may name — which is why `ComplianceTypes` is a list
/// of at most three of the four published values.
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Resource types a verdict names, from
/// `smithy_query(query="shape", shape="ResourceType")`; `AWS::::Account` is the
/// account-level type Config accepts for an evaluation of the account itself
/// (`DescribeComplianceByResourceRequest$ResourceType`: "you can specify that the
/// resource type is an Amazon Web Services account by specifying AWS::::Account").
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const ACCOUNT: &str = "AWS::::Account";
/// See [`EC2_VOLUME`]. A published type no case places a resource of, which the rows
/// asking for a type the account has nothing of name.
const EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// The cap a count of contributing RULES is reported up to
/// (`ComplianceSummary$NonCompliantResourceCount`: "The number of Config rules or Amazon
/// Web Services resources that are noncompliant, up to a maximum of 25 for rules and 100
/// for resources"), which is what a resource's contributors are counted as: "If a
/// resource is noncompliant, this operation returns the number of Config rules that the
/// resource does not comply with" (`DescribeComplianceByResource`).
const RULE_CAP: usize = 25;

/// The page this operation serves a request that asks for no size of its own
/// (`DescribeComplianceByResourceRequest$Limit`: "The maximum number of evaluation
/// results returned on each page. The default is 10. … If you specify 0, Config uses the
/// default").
const DEFAULT_LIMIT: usize = 10;

/// The largest page a request may ask for (`Limit`, `@range(min: 0, max: 100)`; the same
/// member's doc: "You cannot specify a number greater than 100").
const MAX_LIMIT: usize = 100;

/// One verdict a rule recorded: the type and id of the resource it is about, and the
/// compliance it places that resource at. Every verdict a row seeds this way is recorded
/// in the detective evaluation mode; [`record_verdicts_in_mode`] is what seeds one in
/// another mode.
type Verdict = (&'static str, String, &'static str);

/// One Config rule a case seeds: its name and the verdicts it recorded.
type Seeded = (String, Vec<Verdict>);

/// One entry a `DescribeComplianceByResource` response carries: the resource it is
/// about, what that resource rolls up to, and the `(CappedCount, CapExceeded)` of the
/// contributor count, absent where nothing contributes a noncompliant result.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    resource_type: String,
    resource_id: String,
    compliance_type: String,
    contributors: Option<(i64, bool)>,
}

/// An entry reporting `resource_id` of `resource_type` at `compliance_type` with no
/// contributor count — what every verdict other than a noncompliant one rolls up to.
fn entry(resource_type: &str, resource_id: &str, compliance_type: &str) -> Reported {
    Reported {
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
        compliance_type: compliance_type.to_string(),
        contributors: None,
    }
}

/// An entry reporting `resource_id` of `resource_type` as noncompliant with `rules`
/// rules contributing that result.
fn non_compliant(resource_type: &str, resource_id: &str, rules: usize) -> Reported {
    Reported {
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
        compliance_type: NON_COMPLIANT.to_string(),
        contributors: Some((
            i64::try_from(rules.min(RULE_CAP)).expect("a capped count fits"),
            rules > RULE_CAP,
        )),
    }
}

/// Record one evaluation result per entry of `verdicts` against the rule `name`, through
/// the bounded writer the store exposes.
fn record_verdicts(emulator: &Emulator, name: &str, verdicts: &[Verdict]) {
    record_verdicts_in_mode(emulator, name, verdicts, EVALUATION_MODE_DETECTIVE);
}

/// [`record_verdicts`] in the evaluation mode `mode` — a rule holds one result per
/// resource PER MODE, which is what lets a case check that one rule placing a resource in
/// both modes still contributes once.
fn record_verdicts_in_mode(
    emulator: &Emulator,
    name: &str,
    verdicts: &[Verdict],
    mode: &str,
) {
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
            mode,
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

/// What a `DescribeComplianceByResource` response reports, in the order it carried it.
fn reported(case: &str, body: &Value) -> Vec<Reported> {
    let entries = body["ComplianceByResources"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ComplianceByResources, got {body}"));
    entries
        .iter()
        .map(|entry| {
            let compliance = &entry["Compliance"];
            let count = &compliance["ComplianceContributorCount"];
            Reported {
                resource_type: string(case, entry, "ResourceType"),
                resource_id: string(case, entry, "ResourceId"),
                compliance_type: string(case, compliance, "ComplianceType"),
                contributors: count.is_object().then(|| {
                    (
                        count["CappedCount"].as_i64().unwrap_or_else(|| {
                            panic!("case {case:?}: expected a CappedCount, got {entry}")
                        }),
                        count["CapExceeded"].as_bool().unwrap_or_else(|| {
                            panic!("case {case:?}: expected a CapExceeded, got {entry}")
                        }),
                    )
                }),
            }
        })
        .collect()
}

/// The string member `name` of `value`.
fn string(case: &str, value: &Value, name: &str) -> String {
    value[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} string, got {value}"))
        .to_string()
}

/// The entries the operation reports for `request`, which must fit in one page.
async fn report(case: &str, emulator: &Emulator, request: &Value) -> Vec<Reported> {
    let body = emulator
        .ok("DescribeComplianceByResource", request)
        .await;
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "case {case:?}: the whole report fits in one page, got {body}"
    );
    reported(case, &body)
}

/// Every entry the operation reports for `request`, walking the pages it hands back, and
/// the size of each page it served.
async fn every_page(case: &str, emulator: &Emulator, request: &Value) -> (Vec<Reported>, Vec<usize>) {
    let mut entries = Vec::new();
    let mut sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let answered = emulator.ok("DescribeComplianceByResource", &body).await;
        let page = reported(case, &answered);
        sizes.push(page.len());
        entries.extend(page);
        token = answered
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            sizes.len() <= 50,
            "case {case:?}: the paged walk must terminate, got {sizes:?}"
        );
    }
    (entries, sizes)
}

/// The rules the reporting scenario seeds, and the verdicts each recorded.
///
/// Two rules place the bucket `logs` — one compliant, one noncompliant — so a resource
/// whose verdict comes from rolling SEVERAL rules' verdicts into one is covered; a volume
/// is placed `NOT_APPLICABLE` only, so a resource no verdict places either way is too,
/// and one rule records nothing at all.
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

/// What the seeded scenario owes over every resource, in the `(resourceType,
/// resourceId)` ordering a page walks.
fn seeded_report() -> Vec<Reported> {
    vec![
        non_compliant(ACCOUNT, "000000000000", 1),
        entry(EC2_VOLUME, "vol-0001", COMPLIANT),
        non_compliant(EC2_VOLUME, "vol-0002", 1),
        entry(EC2_VOLUME, "vol-0003", INSUFFICIENT_DATA),
        entry(S3_BUCKET, "data", COMPLIANT),
        non_compliant(S3_BUCKET, "logs", 1),
    ]
}

#[tokio::test]
async fn every_evaluated_resource_is_reported_with_what_its_rules_roll_up_to() {
    // Rows differ along which resource the request names and which compliance types it
    // filters by: nothing at all, one resource type, a type and an id together, a type
    // the account has nothing of, an id of another type, one compliance type, two of
    // them, and the whole filterable set.
    struct Case {
        case: &'static str,
        resource_type: Option<&'static str>,
        resource_id: Option<&'static str>,
        compliance_types: Option<&'static [&'static str]>,
        expected: Vec<Reported>,
    }

    let cases = vec![
        Case {
            case: "no narrowing at all",
            resource_type: None,
            resource_id: None,
            compliance_types: None,
            expected: seeded_report(),
        },
        Case {
            case: "one resource type",
            resource_type: Some(EC2_VOLUME),
            resource_id: None,
            compliance_types: None,
            expected: vec![
                entry(EC2_VOLUME, "vol-0001", COMPLIANT),
                non_compliant(EC2_VOLUME, "vol-0002", 1),
                entry(EC2_VOLUME, "vol-0003", INSUFFICIENT_DATA),
            ],
        },
        Case {
            case: "a resource type and an id",
            resource_type: Some(S3_BUCKET),
            resource_id: Some("logs"),
            compliance_types: None,
            // Two rules evaluated this bucket and one placed it noncompliant, so that
            // one rule is what contributes the result.
            expected: vec![non_compliant(S3_BUCKET, "logs", 1)],
        },
        Case {
            case: "a resource type the account has nothing of",
            resource_type: Some(EC2_INSTANCE),
            resource_id: None,
            compliance_types: None,
            expected: Vec::new(),
        },
        Case {
            case: "an id that belongs to another resource type",
            resource_type: Some(EC2_VOLUME),
            resource_id: Some("logs"),
            compliance_types: None,
            // The pair identifies no evaluated resource, so the report is about the
            // resource the request named and no rule has placed it.
            expected: vec![entry(EC2_VOLUME, "logs", INSUFFICIENT_DATA)],
        },
        Case {
            case: "filtered to the compliant resources",
            resource_type: None,
            resource_id: None,
            compliance_types: Some(&[COMPLIANT]),
            expected: vec![
                entry(EC2_VOLUME, "vol-0001", COMPLIANT),
                entry(S3_BUCKET, "data", COMPLIANT),
            ],
        },
        Case {
            case: "filtered to the noncompliant resources",
            resource_type: None,
            resource_id: None,
            compliance_types: Some(&[NON_COMPLIANT]),
            expected: vec![
                non_compliant(ACCOUNT, "000000000000", 1),
                non_compliant(EC2_VOLUME, "vol-0002", 1),
                non_compliant(S3_BUCKET, "logs", 1),
            ],
        },
        Case {
            case: "filtered to two compliance types",
            resource_type: None,
            resource_id: None,
            compliance_types: Some(&[NON_COMPLIANT, COMPLIANT]),
            expected: seeded_report()
                .into_iter()
                .filter(|reported| reported.compliance_type != INSUFFICIENT_DATA)
                .collect(),
        },
        Case {
            case: "filtered to every value a filter may name",
            resource_type: None,
            resource_id: None,
            compliance_types: Some(&[COMPLIANT, NON_COMPLIANT, NOT_APPLICABLE]),
            // No resource rolls up to NOT_APPLICABLE — a verdict that applies to nothing
            // leaves the resource unplaced — so naming it adds nothing.
            expected: seeded_report()
                .into_iter()
                .filter(|reported| reported.compliance_type != INSUFFICIENT_DATA)
                .collect(),
        },
        Case {
            case: "filtered to a compliance type nothing of the named type rolls up to",
            resource_type: Some(ACCOUNT),
            resource_id: None,
            compliance_types: Some(&[COMPLIANT]),
            expected: Vec::new(),
        },
    ];

    let emulator = emulator_holding(&seeded_rules()).await;

    for row in cases {
        let case = row.case;
        let mut request = json!({});
        if let Some(resource_type) = row.resource_type {
            request["ResourceType"] = json!(resource_type);
        }
        if let Some(resource_id) = row.resource_id {
            request["ResourceId"] = json!(resource_id);
        }
        if let Some(compliance_types) = row.compliance_types {
            request["ComplianceTypes"] = json!(compliance_types);
        }

        let actual = report(case, &emulator, &request).await;
        assert_eq!(
            actual, row.expected,
            "case {case:?}: expected the report {:?}, got {actual:?}",
            row.expected
        );
    }
}

#[tokio::test]
async fn the_contributor_count_counts_the_rules_up_to_the_published_cap() {
    // Rows differ along how many rules place the same resource noncompliant: none, one,
    // a handful, one short of the published cap, the cap itself, one past it and well
    // past it. Every row also has one rule placing the resource compliant, so the
    // noncompliant verdict's precedence is checked on each.
    for rules_placing_it in [0usize, 1, 4, RULE_CAP - 1, RULE_CAP, RULE_CAP + 1, 40] {
        let case = format!("{rules_placing_it} rules placing the resource noncompliant");
        let resource_id = "vol-0001".to_string();

        let mut rules: Vec<Seeded> = (0..rules_placing_it)
            .map(|index| {
                (
                    format!("non-compliant-{index:03}"),
                    vec![(EC2_VOLUME, resource_id.clone(), NON_COMPLIANT)],
                )
            })
            .collect();
        rules.push((
            "compliant".to_string(),
            vec![(EC2_VOLUME, resource_id.clone(), COMPLIANT)],
        ));
        let emulator = emulator_holding(&rules).await;

        let expected = if rules_placing_it == 0 {
            vec![entry(EC2_VOLUME, &resource_id, COMPLIANT)]
        } else {
            vec![non_compliant(EC2_VOLUME, &resource_id, rules_placing_it)]
        };
        let actual = report(
            &case,
            &emulator,
            &json!({ "ResourceType": EC2_VOLUME, "ResourceId": resource_id }),
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn one_rule_placing_a_resource_in_both_modes_contributes_once() {
    // What the contributor count counts is RULES, while a rule holds one result per
    // evaluation mode — so the rows differ along how many of the two published
    // `EvaluationMode` values a rule placed the same resource in, and how many rules did.
    for (case, rules, modes) in [
        ("one rule, one mode", 1usize, 1usize),
        ("one rule, both modes", 1, 2),
        ("three rules, both modes each", 3, 2),
    ] {
        let resource_id = "vol-0001".to_string();
        let verdicts = vec![(EC2_VOLUME, resource_id.clone(), NON_COMPLIANT)];
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        for index in 0..rules {
            let name = format!("rule-{index}");
            emulator.ok("PutConfigRule", &managed_rule(&name)).await;
            // Every published `EvaluationMode` value, from
            // `smithy_query(query="shape", shape="EvaluationMode")`: `DETECTIVE` and
            // `PROACTIVE`.
            for mode in [EVALUATION_MODE_DETECTIVE, EVALUATION_MODE_PROACTIVE]
                .iter()
                .take(modes)
            {
                record_verdicts_in_mode(&emulator, &name, &verdicts, mode);
            }
        }

        let expected = vec![non_compliant(EC2_VOLUME, &resource_id, rules)];
        let actual = report(
            case,
            &emulator,
            &json!({ "ResourceType": EC2_VOLUME, "ResourceId": resource_id }),
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_what_the_limit_asks_for_and_the_token_resumes_the_rest() {
    // Rows differ along the page size the request asks for: none at all (the documented
    // default of ten), one, a size that divides the report evenly, one that does not, the
    // report's own size, the modelled maximum, and a size past the whole report.
    let resources = 23usize;
    let verdicts: Vec<Verdict> = (0..resources)
        .map(|index| {
            let compliance = if index % 2 == 0 {
                NON_COMPLIANT
            } else {
                COMPLIANT
            };
            (EC2_VOLUME, format!("vol-{index:04}"), compliance)
        })
        .collect();
    let emulator = emulator_holding(&[("volumes".to_string(), verdicts.clone())]).await;

    let whole: Vec<Reported> = {
        let mut sorted = verdicts.clone();
        sorted.sort_by(|left, right| left.1.cmp(&right.1));
        sorted
            .iter()
            .map(|(resource_type, resource_id, compliance)| {
                if *compliance == NON_COMPLIANT {
                    non_compliant(resource_type, resource_id, 1)
                } else {
                    entry(resource_type, resource_id, compliance)
                }
            })
            .collect()
    };

    for limit in [None, Some(1usize), Some(23), Some(7), Some(5), Some(MAX_LIMIT), Some(40)] {
        let case = match limit {
            None => "no limit at all".to_string(),
            Some(limit) => format!("a limit of {limit}"),
        };
        let page_size = limit.unwrap_or(DEFAULT_LIMIT);
        let mut request = json!({ "ResourceType": EC2_VOLUME });
        if let Some(limit) = limit {
            request["Limit"] = json!(limit);
        }

        let (entries, sizes) = every_page(&case, &emulator, &request).await;
        assert_eq!(
            entries, whole,
            "case {case:?}: the pages together must report every resource once, in order, \
             got {entries:?}"
        );
        let expected_sizes: Vec<usize> = {
            let mut sizes = Vec::new();
            let mut left = resources;
            loop {
                let served = left.min(page_size);
                sizes.push(served);
                left -= served;
                if left == 0 {
                    break;
                }
            }
            sizes
        };
        assert_eq!(
            sizes, expected_sizes,
            "case {case:?}: expected the pages {expected_sizes:?}, got {sizes:?}"
        );
    }
}

#[tokio::test]
async fn a_request_it_cannot_serve_is_refused_as_the_model_declares() {
    // Rows differ along what is wrong with the request: a compliance filter naming the
    // one value no filter may name, a filter value outside the published enum, a resource
    // id with no resource type, and a `NextToken` this operation did not issue.
    struct Case {
        case: &'static str,
        request: Value,
        code: &'static str,
        message: String,
    }

    let filterable = "the valid values are COMPLIANT, NON_COMPLIANT, NOT_APPLICABLE.";
    let cases = vec![
        Case {
            case: "a filter naming INSUFFICIENT_DATA",
            request: json!({ "ComplianceTypes": [INSUFFICIENT_DATA] }),
            code: "InvalidParameterValueException",
            message: format!("'{INSUFFICIENT_DATA}' is not a valid value for ComplianceTypes; {filterable}"),
        },
        Case {
            case: "a filter naming INSUFFICIENT_DATA beside a value it may name",
            request: json!({ "ComplianceTypes": [COMPLIANT, INSUFFICIENT_DATA] }),
            code: "InvalidParameterValueException",
            message: format!("'{INSUFFICIENT_DATA}' is not a valid value for ComplianceTypes; {filterable}"),
        },
        Case {
            case: "a resource id with no resource type",
            request: json!({ "ResourceId": "vol-0001" }),
            code: "InvalidParameterValueException",
            message: "If you specify a resource ID, you must also specify a type for ResourceType."
                .to_string(),
        },
        Case {
            case: "a next token this operation did not issue",
            request: json!({ "NextToken": "not-a-token-this-operation-issued" }),
            code: "InvalidNextTokenException",
            message: "The nextToken provided is invalid".to_string(),
        },
    ];

    let emulator = emulator_holding(&seeded_rules()).await;

    for row in cases {
        let case = row.case;
        let (status, body) = emulator
            .call("DescribeComplianceByResource", &row.request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = body
            .get("__type")
            .and_then(Value::as_str)
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
            .unwrap_or_else(|| format!("<no __type in {body}>"));
        assert_eq!(
            code, row.code,
            "case {case:?}: expected the error code {}, got {code:?} in {body}",
            row.code
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"));
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the message {:?}, got {message:?}",
            row.message
        );
    }
}

#[tokio::test]
async fn the_report_is_a_read_that_changes_nothing_a_later_caller_sees() {
    let emulator = emulator_holding(&seeded_rules()).await;

    // Rows differ along which request the report is asked for: the whole report, one
    // narrowed to a resource, a filtered one, and one about a resource no rule placed.
    for (case, request) in [
        ("the whole report", json!({})),
        (
            "a report about one resource",
            json!({ "ResourceType": S3_BUCKET, "ResourceId": "logs" }),
        ),
        (
            "a filtered report",
            json!({ "ComplianceTypes": [NON_COMPLIANT] }),
        ),
        (
            "a report about a resource no rule placed",
            json!({ "ResourceType": EC2_INSTANCE, "ResourceId": "i-0001" }),
        ),
    ] {
        let details_before = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": "volumes" }),
            )
            .await;
        let first = report(case, &emulator, &request).await;
        let again = report(case, &emulator, &request).await;
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
            "case {case:?}: reporting compliance must not change the recorded results, \
             got {details_after}"
        );
    }
}

#[tokio::test]
async fn verdicts_delivered_over_the_wire_are_reported_by_the_resource_report() {
    // Rows differ along the compliance each delivered verdict reports and how many
    // resources of each type it is about.
    struct Case {
        case: &'static str,
        /// `(resourceType, resourceId, ComplianceType)` per delivered verdict.
        delivered: &'static [(&'static str, &'static str, &'static str)],
    }

    let cases = [
        Case {
            case: "nothing delivered at all",
            delivered: &[],
        },
        Case {
            case: "one noncompliant resource",
            delivered: &[(EC2_VOLUME, "vol-0001", NON_COMPLIANT)],
        },
        Case {
            case: "resources of two types, both compliance types",
            delivered: &[
                (EC2_VOLUME, "vol-0001", COMPLIANT),
                (EC2_VOLUME, "vol-0002", NON_COMPLIANT),
                (S3_BUCKET, "logs", COMPLIANT),
            ],
        },
        Case {
            case: "a verdict that applies to nothing leaves the resource unplaced",
            delivered: &[
                (EC2_VOLUME, "vol-0001", NOT_APPLICABLE),
                (ACCOUNT, "000000000000", NON_COMPLIANT),
            ],
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

        // One delivering rule placed each resource, so a delivered noncompliant verdict
        // has exactly one rule contributing it and every other delivered verdict rolls
        // the resource up with no contributor count at all.
        let mut expected: Vec<Reported> = row
            .delivered
            .iter()
            .map(|(resource_type, resource_id, compliance)| match *compliance {
                NON_COMPLIANT => non_compliant(resource_type, resource_id, 1),
                COMPLIANT => entry(resource_type, resource_id, COMPLIANT),
                _ => entry(resource_type, resource_id, INSUFFICIENT_DATA),
            })
            .collect();
        expected.sort_by(|left, right| {
            (&left.resource_type, &left.resource_id).cmp(&(&right.resource_type, &right.resource_id))
        });

        let actual = report(case, &emulator, &json!({})).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the report {expected:?}, got {actual:?}"
        );

        // Each resource the delivery named is also reported on its own, which is the
        // narrowed read of the same verdicts.
        for (resource_type, resource_id, _) in row.delivered {
            let one = report(
                case,
                &emulator,
                &json!({ "ResourceType": resource_type, "ResourceId": resource_id }),
            )
            .await;
            let owed: Vec<Reported> = expected
                .iter()
                .filter(|reported| {
                    reported.resource_type == *resource_type && reported.resource_id == *resource_id
                })
                .cloned()
                .collect();
            assert_eq!(
                one, owed,
                "case {case:?}: expected {resource_id:?} to be reported as {owed:?}, got {one:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_no_rule_has_evaluated_is_reported_as_insufficient_data() {
    // Rows differ along what the account holds and what the request names: an account
    // with no rules at all, one whose rules evaluated other resources, and a request for
    // a resource of a type that has evaluated resources in it.
    let empty = Emulator::new();
    with_recorder_and_channel(&empty).await;
    let seeded = emulator_holding(&seeded_rules()).await;

    for (case, emulator, resource_type, resource_id) in [
        ("an account with no rules at all", &empty, EC2_VOLUME, "vol-9999"),
        ("a resource of an unevaluated type", &seeded, EC2_INSTANCE, "i-0001"),
        ("a resource of an evaluated type", &seeded, EC2_VOLUME, "vol-9999"),
    ] {
        let actual = report(
            case,
            emulator,
            &json!({ "ResourceType": resource_type, "ResourceId": resource_id }),
        )
        .await;
        let expected = vec![entry(resource_type, resource_id, INSUFFICIENT_DATA)];
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // A request that identifies NO resource names nothing an absence could be reported
    // for, so the report is empty rather than carrying an invented entry.
    for (case, request) in [
        ("no narrowing at all", json!({})),
        ("a resource type with nothing of it", json!({ "ResourceType": EC2_INSTANCE })),
    ] {
        let actual = report(case, &empty, &request).await;
        assert!(
            actual.is_empty(),
            "case {case:?}: expected an empty report, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn one_resource_several_rules_evaluated_is_reported_once() {
    // The same resource is placed by rules whose verdicts differ, so the row covers both
    // the roll-up precedence and the de-duplication of one resource across rules.
    let resource_id = "logs".to_string();
    let rules: Vec<Seeded> = [COMPLIANT, NOT_APPLICABLE, COMPLIANT, NON_COMPLIANT, COMPLIANT]
        .iter()
        .enumerate()
        .map(|(index, compliance)| {
            (
                format!("rule-{index}"),
                vec![(S3_BUCKET, resource_id.clone(), *compliance)],
            )
        })
        .collect();
    let emulator = emulator_holding(&rules).await;

    let actual = report("five rules placing one bucket", &emulator, &json!({})).await;
    let expected = vec![non_compliant(S3_BUCKET, &resource_id, 1)];
    assert_eq!(
        actual, expected,
        "case \"five rules placing one bucket\": expected {expected:?}, got {actual:?}"
    );

    let resources: BTreeSet<(String, String)> = actual
        .iter()
        .map(|reported| (reported.resource_type.clone(), reported.resource_id.clone()))
        .collect();
    assert_eq!(
        resources.len(),
        actual.len(),
        "case \"five rules placing one bucket\": every resource is reported once, got {actual:?}"
    );
}
