// SPDX-License-Identifier: CC0-1.0

//! `DescribeComplianceByConfigRule`: whether the account's Config rules are compliant.
//!
//! The rule pinned here: the operation reports, for every rule the request selects,
//! the `ComplianceType` that rule's recorded evaluation results roll up to —
//! `NON_COMPLIANT` if any evaluated resource is noncompliant, `COMPLIANT` if at least
//! one is compliant and none is not, `INSUFFICIENT_DATA` when no result places it
//! either way — together with the number of resources causing the `NON_COMPLIANT`
//! result where there are any, paged over the rules the account holds and refusing a
//! name no rule carries or a token it did not issue.
//!
//! Every case drives the emulator the way a client does, through the same
//! `MultiServiceRouter` the production router builds, over a TABLE of inputs rather
//! than one recorded call: the rows differ along which names the request selects, how
//! many results of which compliance type a rule carries, which compliance types the
//! request filters by, and how many rules the account holds.
//!
//! Recording an evaluation result is `PutEvaluations`' job and that operation is still
//! a scaffold (its own stub issue), so the rows that need results already recorded
//! seed them into the same shared state a handler reads — the state
//! `PutEvaluations` will write once it lands. Everything else goes over the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    EvaluationResultEntry, EVALUATION_MODE_DETECTIVE,
};
use service_com_amazonaws_configservice_starlingdoveservice::ComAmazonawsConfigserviceStarlingdoveserviceState as ConfigState;

use crate::config_setup_and_rule_loop::{managed_rule, with_recorder_and_channel, Emulator};

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`,
/// `NON_COMPLIANT`, `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The cap a count of contributing RESOURCES is reported up to
/// (`ComplianceSummary$NonCompliantResourceCount`: "up to a maximum of 25 for rules
/// and 100 for resources"), which is what a rule's `ComplianceContributorCount`
/// counts.
const RESOURCE_CAP: usize = 100;

/// The page size this emulator serves the report at (the `ConfigRuleNames` length
/// maximum of 0..25 the same operation declares, which `DescribeConfigRules` already
/// pages at).
const PAGE_SIZE: usize = 25;

/// An emulator holding one managed Config rule per name in `names`.
async fn emulator_with_rules(names: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for name in names {
        emulator.ok("PutConfigRule", &managed_rule(name)).await;
    }
    emulator
}

/// Record one evaluation result per entry of `results` against the rule `name`, each
/// for a distinct resource.
///
/// This is the state `PutEvaluations` writes; that operation is still scaffolded, so
/// the results are seeded into the shared store the handler reads.
fn record_results(emulator: &Emulator, name: &str, results: &[&str]) {
    let state = emulator.engine.state::<ConfigState>();
    let mut store = state.lock();
    let rule = store
        .config_rules
        .get_mut(name)
        .unwrap_or_else(|| panic!("the rule {name:?} must have been created first"));
    rule.evaluation_results = results
        .iter()
        .enumerate()
        .map(|(index, compliance)| {
            let entry = EvaluationResultEntry::new(
                "AWS::EC2::Volume",
                &format!("vol-{index:04}"),
                EVALUATION_MODE_DETECTIVE,
                compliance,
                SystemTime::UNIX_EPOCH,
            );
            (entry.key(), entry)
        })
        .collect();
}

/// The `(CappedCount, CapExceeded)` of a reported `ComplianceContributorCount`.
type Contributors = (i64, bool);

/// One entry of a `DescribeComplianceByConfigRule` report.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    compliance_type: String,
    /// The entry's `ComplianceContributorCount`, where it reports one.
    contributors: Option<Contributors>,
}

/// What a `DescribeComplianceByConfigRule` response reports, one [`Entry`] per
/// `ComplianceByConfigRules` member, in the order the response carried them.
fn reported(case: &str, body: &Value) -> Vec<Entry> {
    let entries = body["ComplianceByConfigRules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ComplianceByConfigRules, got {body}"));
    entries
        .iter()
        .map(|entry| {
            let name = entry["ConfigRuleName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ConfigRuleName, got {entry}"))
                .to_string();
            let compliance = &entry["Compliance"];
            let compliance_type = compliance["ComplianceType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ComplianceType, got {entry}"))
                .to_string();
            let contributors = compliance.get("ComplianceContributorCount").and_then(|count| {
                if count.is_null() {
                    return None;
                }
                Some((
                    count["CappedCount"].as_i64().unwrap_or_else(|| {
                        panic!("case {case:?}: expected a CappedCount, got {entry}")
                    }),
                    count["CapExceeded"].as_bool().unwrap_or_else(|| {
                        panic!("case {case:?}: expected a CapExceeded, got {entry}")
                    }),
                ))
            });
            Entry {
                name,
                compliance_type,
                contributors,
            }
        })
        .collect()
}

/// The names a response reports, in the order it reported them.
fn reported_names(case: &str, body: &Value) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

#[tokio::test]
async fn every_selected_rule_is_reported_with_the_compliance_it_rolls_up_to() {
    // The account's rules, each carrying a different mix of recorded results: rows of
    // the roll-up the report must compute.
    struct Seeded {
        name: &'static str,
        results: &'static [&'static str],
        compliance_type: &'static str,
        contributors: Option<Contributors>,
    }

    let seeded = [
        Seeded {
            name: "all-compliant",
            results: &[COMPLIANT, COMPLIANT],
            compliance_type: COMPLIANT,
            contributors: None,
        },
        Seeded {
            name: "mixed",
            results: &[COMPLIANT, NON_COMPLIANT, NON_COMPLIANT],
            compliance_type: NON_COMPLIANT,
            contributors: Some((2, false)),
        },
        Seeded {
            name: "not-applicable",
            results: &[NOT_APPLICABLE],
            compliance_type: INSUFFICIENT_DATA,
            contributors: None,
        },
        Seeded {
            name: "unevaluated",
            results: &[],
            compliance_type: INSUFFICIENT_DATA,
            contributors: None,
        },
    ];

    // Rows differ along how many names the request selects and which: all of them (no
    // filter), one, a subset, every name, and the same name twice.
    struct Selection {
        case: &'static str,
        names: Option<&'static [&'static str]>,
        expected: &'static [&'static str],
    }

    let selections = [
        Selection {
            case: "no ConfigRuleNames at all",
            names: None,
            expected: &["all-compliant", "mixed", "not-applicable", "unevaluated"],
        },
        Selection {
            case: "one name",
            names: Some(&["mixed"]),
            expected: &["mixed"],
        },
        Selection {
            case: "a subset of two names",
            names: Some(&["unevaluated", "all-compliant"]),
            expected: &["all-compliant", "unevaluated"],
        },
        Selection {
            case: "every name",
            names: Some(&["unevaluated", "not-applicable", "mixed", "all-compliant"]),
            expected: &["all-compliant", "mixed", "not-applicable", "unevaluated"],
        },
        Selection {
            case: "the same name twice",
            names: Some(&["mixed", "mixed"]),
            expected: &["mixed"],
        },
    ];

    for selection in selections {
        let case = selection.case;
        let rule_names: Vec<&str> = seeded.iter().map(|rule| rule.name).collect();
        let emulator = emulator_with_rules(&rule_names).await;
        for rule in &seeded {
            record_results(&emulator, rule.name, rule.results);
        }

        let request = match selection.names {
            Some(names) => json!({ "ConfigRuleNames": names }),
            None => json!({}),
        };
        let body = emulator
            .ok("DescribeComplianceByConfigRule", &request)
            .await;
        let entries = reported(case, &body);

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<&str>>(),
            selection.expected,
            "case {case:?}: expected exactly the selected rules once each, in name order, \
             got {body}"
        );
        for entry in &entries {
            let name = &entry.name;
            let expected = seeded
                .iter()
                .find(|rule| rule.name == name.as_str())
                .unwrap_or_else(|| panic!("case {case:?}: unexpected rule {name:?} in {body}"));
            assert_eq!(
                entry.compliance_type,
                expected.compliance_type,
                "case {case:?}: expected rule {name:?} to roll up to {}, got {body}",
                expected.compliance_type
            );
            assert_eq!(
                entry.contributors,
                expected.contributors,
                "case {case:?}: expected rule {name:?} to report contributors {:?}, got {body}",
                expected.contributors
            );
        }

        // The report is a READ: it must not change what a later caller sees.
        let rules_before = emulator.ok("DescribeConfigRules", &json!({})).await;
        let again = emulator
            .ok("DescribeComplianceByConfigRule", &request)
            .await;
        let rules_after = emulator.ok("DescribeConfigRules", &json!({})).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second report must say the same, got {again}"
        );
        assert_eq!(
            rules_after, rules_before,
            "case {case:?}: reporting compliance must not change the rules, got {rules_after}"
        );
    }
}

#[tokio::test]
async fn the_contributor_count_counts_the_noncompliant_resources_up_to_the_cap() {
    // Rows differ along how many resources contribute a NON_COMPLIANT result and what
    // else was recorded beside them: none, one, a handful, a mix with compliant and
    // not-applicable results, one short of the published cap, the cap itself, and one
    // past it.
    struct Case {
        name: &'static str,
        non_compliant: usize,
        other: &'static [&'static str],
    }

    let cases = [
        Case {
            name: "nothing recorded",
            non_compliant: 0,
            other: &[],
        },
        Case {
            name: "only compliant results",
            non_compliant: 0,
            other: &[COMPLIANT, COMPLIANT, COMPLIANT],
        },
        Case {
            name: "one noncompliant resource",
            non_compliant: 1,
            other: &[],
        },
        Case {
            name: "a handful of noncompliant resources",
            non_compliant: 4,
            other: &[],
        },
        Case {
            name: "noncompliant beside compliant and not-applicable results",
            non_compliant: 3,
            other: &[COMPLIANT, NOT_APPLICABLE, COMPLIANT],
        },
        Case {
            name: "one short of the resource cap",
            non_compliant: RESOURCE_CAP - 1,
            other: &[],
        },
        Case {
            name: "exactly the resource cap",
            non_compliant: RESOURCE_CAP,
            other: &[],
        },
        Case {
            name: "one past the resource cap",
            non_compliant: RESOURCE_CAP + 1,
            other: &[],
        },
    ];

    for case in cases {
        let label = case.name;
        let emulator = emulator_with_rules(&["counted"]).await;
        let mut results: Vec<&str> = vec![NON_COMPLIANT; case.non_compliant];
        results.extend_from_slice(case.other);
        record_results(&emulator, "counted", &results);

        let body = emulator
            .ok("DescribeComplianceByConfigRule", &json!({}))
            .await;
        let entries = reported(label, &body);
        assert_eq!(
            entries.len(),
            1,
            "case {label:?}: expected the one rule the account holds, got {body}"
        );
        let entry = &entries[0];

        let expected_type = if case.non_compliant > 0 {
            NON_COMPLIANT
        } else if case.other.contains(&COMPLIANT) {
            COMPLIANT
        } else {
            INSUFFICIENT_DATA
        };
        assert_eq!(
            entry.compliance_type, expected_type,
            "case {label:?}: expected the rule to roll up to {expected_type}, got {body}"
        );

        // The contributor count reports the resources causing the NON_COMPLIANT
        // result, so a rule nothing places as noncompliant reports none at all.
        let expected_contributors = (case.non_compliant > 0).then(|| {
            (
                i64::try_from(case.non_compliant.min(RESOURCE_CAP)).expect("a capped count fits"),
                case.non_compliant > RESOURCE_CAP,
            )
        });
        assert_eq!(
            entry.contributors, expected_contributors,
            "case {label:?}: expected contributors {expected_contributors:?} for \
             {} noncompliant resources, got {body}",
            case.non_compliant
        );
    }
}

#[tokio::test]
async fn the_compliance_filter_selects_rules_by_what_they_roll_up_to() {
    // One rule per roll-up the report can compute, so a filter has something to both
    // select and leave out.
    let seeded: [(&str, &[&str]); 4] = [
        ("compliant-rule", &[COMPLIANT]),
        ("noncompliant-rule", &[COMPLIANT, NON_COMPLIANT]),
        ("not-applicable-rule", &[NOT_APPLICABLE, NOT_APPLICABLE]),
        ("unevaluated-rule", &[]),
    ];

    // Rows cover every published ComplianceType on its own, a pair, the whole
    // modelled maximum of three, and no filter at all; the last row combines the
    // filter with a name selection.
    struct Case {
        case: &'static str,
        compliance_types: Option<&'static [&'static str]>,
        names: Option<&'static [&'static str]>,
        expected: &'static [&'static str],
    }

    let cases = [
        Case {
            case: "no ComplianceTypes at all",
            compliance_types: None,
            names: None,
            expected: &[
                "compliant-rule",
                "noncompliant-rule",
                "not-applicable-rule",
                "unevaluated-rule",
            ],
        },
        Case {
            case: "COMPLIANT only",
            compliance_types: Some(&[COMPLIANT]),
            names: None,
            expected: &["compliant-rule"],
        },
        Case {
            case: "NON_COMPLIANT only",
            compliance_types: Some(&[NON_COMPLIANT]),
            names: None,
            expected: &["noncompliant-rule"],
        },
        Case {
            case: "INSUFFICIENT_DATA only",
            compliance_types: Some(&[INSUFFICIENT_DATA]),
            names: None,
            expected: &["not-applicable-rule", "unevaluated-rule"],
        },
        Case {
            // No rule rolls up to NOT_APPLICABLE: a rule whose results are all
            // NOT_APPLICABLE has nothing placing it either way, which is
            // INSUFFICIENT_DATA.
            case: "NOT_APPLICABLE only",
            compliance_types: Some(&[NOT_APPLICABLE]),
            names: None,
            expected: &[],
        },
        Case {
            case: "a pair of compliance types",
            compliance_types: Some(&[NON_COMPLIANT, COMPLIANT]),
            names: None,
            expected: &["compliant-rule", "noncompliant-rule"],
        },
        Case {
            case: "the modelled maximum of three",
            compliance_types: Some(&[COMPLIANT, NON_COMPLIANT, INSUFFICIENT_DATA]),
            names: None,
            expected: &[
                "compliant-rule",
                "noncompliant-rule",
                "not-applicable-rule",
                "unevaluated-rule",
            ],
        },
        Case {
            case: "a compliance type and a name selection",
            compliance_types: Some(&[INSUFFICIENT_DATA]),
            names: Some(&["unevaluated-rule", "compliant-rule"]),
            expected: &["unevaluated-rule"],
        },
    ];

    let rule_names: Vec<&str> = seeded.iter().map(|(name, _)| *name).collect();
    let emulator = emulator_with_rules(&rule_names).await;
    for (name, results) in seeded {
        record_results(&emulator, name, results);
    }

    for row in cases {
        let case = row.case;
        let mut request = json!({});
        if let Some(compliance_types) = row.compliance_types {
            request["ComplianceTypes"] = json!(compliance_types);
        }
        if let Some(names) = row.names {
            request["ConfigRuleNames"] = json!(names);
        }
        let body = emulator
            .ok("DescribeComplianceByConfigRule", &request)
            .await;
        assert_eq!(
            reported_names(case, &body),
            row.expected,
            "case {case:?}: expected the filter to select {:?}, got {body}",
            row.expected
        );
    }
}

#[tokio::test]
async fn the_report_pages_over_the_rules_in_name_order() {
    // Rule counts around the page size: none at all, one, one short of a page, exactly
    // a page, one past it, and several pages.
    for count in [0_usize, 1, PAGE_SIZE - 1, PAGE_SIZE, PAGE_SIZE + 1, 53] {
        let case = format!("{count} rules");
        let emulator = Emulator::new();
        if count > 0 {
            with_recorder_and_channel(&emulator).await;
        }
        let mut expected: Vec<String> = Vec::with_capacity(count);
        for index in 0..count {
            let name = format!("rule-{index:03}");
            emulator.ok("PutConfigRule", &managed_rule(&name)).await;
            expected.push(name);
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let request = match &token {
                Some(token) => json!({ "NextToken": token }),
                None => json!({}),
            };
            let page = emulator
                .ok("DescribeComplianceByConfigRule", &request)
                .await;
            let names = reported_names(&case, &page);
            pages += 1;
            assert!(
                names.len() <= PAGE_SIZE,
                "case {case:?}: a page carries at most {PAGE_SIZE} rules, got {}",
                names.len()
            );
            seen.extend(names.iter().cloned());
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let remaining = count - seen.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case {case:?}: NextToken is present exactly while rules remain \
                 ({remaining} left), got {page}"
            );
            if token.is_none() {
                break;
            }
            assert_eq!(
                names.len(),
                PAGE_SIZE,
                "case {case:?}: a non-final page is full, got {page}"
            );
            assert!(
                pages <= count + 1,
                "case {case:?}: pagination must terminate, {pages} pages so far"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: the pages must together carry every rule once, in name order"
        );
    }
}

#[tokio::test]
async fn a_name_no_rule_carries_and_a_token_the_report_never_issued_are_refused() {
    let emulator = emulator_with_rules(&["present", "also-present"]).await;

    // Rows differ along where the unknown name sits in the request and how the token
    // is malformed; each refusal names its own modelled error.
    let cases: [(&str, Value, &str); 6] = [
        (
            "one unknown name",
            json!({ "ConfigRuleNames": ["absent"] }),
            "NoSuchConfigRuleException",
        ),
        (
            "an unknown name after a known one",
            json!({ "ConfigRuleNames": ["present", "absent"] }),
            "NoSuchConfigRuleException",
        ),
        (
            "an unknown name before a known one",
            json!({ "ConfigRuleNames": ["absent", "present"] }),
            "NoSuchConfigRuleException",
        ),
        (
            "a name differing only in case",
            json!({ "ConfigRuleNames": ["PRESENT"] }),
            "NoSuchConfigRuleException",
        ),
        (
            "a token of characters no token carries",
            json!({ "NextToken": "not-a-token" }),
            "InvalidNextTokenException",
        ),
        (
            "a token that is not even base64",
            json!({ "NextToken": "%%%" }),
            "InvalidNextTokenException",
        ),
    ];

    for (case, request, expected_code) in cases {
        let (status, code) = emulator
            .refused("DescribeComplianceByConfigRule", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
    }

    // And a token the report DID issue resumes the listing.
    let first = emulator
        .ok("DescribeComplianceByConfigRule", &json!({}))
        .await;
    assert_eq!(
        reported_names("a complete listing", &first),
        vec!["also-present".to_string(), "present".to_string()],
        "case \"a complete listing\": expected both rules in name order, got {first}"
    );
    assert!(
        first.get("NextToken").is_none_or(Value::is_null),
        "case \"a complete listing\": a single page needs no NextToken, got {first}"
    );
}
