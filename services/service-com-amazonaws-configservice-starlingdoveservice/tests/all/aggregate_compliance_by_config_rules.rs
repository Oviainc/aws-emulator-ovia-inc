// SPDX-License-Identifier: CC0-1.0

//! `DescribeAggregateComplianceByConfigRules`: which of the Config rules a
//! configuration aggregator takes in are compliant.
//!
//! The rule pinned here: the operation reports one entry per Config rule of every source
//! the named aggregator takes in whose recorded evaluation results place it in a
//! compliance type an aggregate read holds — `COMPLIANT` or `NON_COMPLIANT`, the only
//! two a `ConfigRuleComplianceFilters` may name — carrying the rule's name, the
//! `Compliance` its results roll up to with the number of resources causing a
//! noncompliant result, and the source account and source Region the data came from; a
//! rule its results place nowhere has no compliance result and is not reported at all.
//! The entries are narrowed by the request's `Filters`, paged at the `Limit` it asks
//! for, and the request is refused for an aggregator that does not exist, for a
//! `ComplianceType` no filter may name, for a `NextToken` this operation did not issue
//! and for a `Limit` outside the published range.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along which evaluation results of
//! which compliance type a rule carries (including the counts at the contributor cap and
//! one past it), which member the request filters on and with what value, which sources
//! the aggregator aggregates, which Region the request is signed for, and which `Limit`
//! it asks for.
//!
//! Recording an evaluation result is `PutEvaluations`' job and that operation is still a
//! scaffold (its own stub issue), so the rows that need results already recorded seed
//! them into the same shared state a handler reads, through the `emulator_holding` seed
//! `aggregate_config_rule_compliance_summary` already defines. Everything else goes over
//! the wire.

use serde_json::{json, Value};

use crate::aggregate_config_rule_compliance_summary::{
    account, account_source, contributors, emulator_holding, organization_source, put_aggregator,
    put_whole_account_aggregator, Contributors, AGGREGATOR, HOME_REGION, MAX_LIMIT, MIN_LIMIT,
    OTHER_ACCOUNT, OTHER_REGION,
};
use crate::config_setup_and_rule_loop::Emulator;

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The cap a count of contributing RESOURCES is reported up to
/// (`ComplianceSummary$NonCompliantResourceCount`: "up to a maximum of 25 for rules and
/// 100 for resources"), which is what a rule's `ComplianceContributorCount` counts.
const RESOURCE_CAP: usize = 100;

/// The rule every row that seeds exactly one rule names it.
const RULE: &str = "rule-under-test";

/// One reported `AggregateComplianceByConfigRule`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    compliance_type: String,
    /// The entry's `ComplianceContributorCount`, where it reports one.
    contributor_count: Option<Contributors>,
    account_id: String,
    aws_region: String,
}

/// Every entry a response reports, in the order it carried them.
fn reported(case: &str, body: &Value) -> Vec<Entry> {
    let entries = body["AggregateComplianceByConfigRules"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected AggregateComplianceByConfigRules, got {body}")
        });
    entries
        .iter()
        .map(|entry| {
            let compliance = &entry["Compliance"];
            Entry {
                name: string(case, entry, "ConfigRuleName"),
                compliance_type: string(case, compliance, "ComplianceType"),
                contributor_count: compliance
                    .get("ComplianceContributorCount")
                    .map(contributors),
                account_id: string(case, entry, "AccountId"),
                aws_region: string(case, entry, "AwsRegion"),
            }
        })
        .collect()
}

/// `member` of `value` as a string, which every reported member of an entry is.
fn string(case: &str, value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {value}"))
        .to_string()
}

/// The names a response reports, in the order it carried them.
fn reported_names(case: &str, body: &Value) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

/// The `DescribeAggregateComplianceByConfigRules` body for the aggregator every row
/// uses.
fn describe_request() -> Value {
    json!({ "ConfigurationAggregatorName": AGGREGATOR })
}

/// The `NextToken` a response carries, where it carries one.
fn next_token(body: &Value) -> Option<String> {
    body.get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// An emulator holding ONE Config rule, named [`RULE`], whose recorded evaluation
/// results are `results`, aggregated by an aggregator over this whole account.
async fn emulator_with_one_rule(results: &[&'static str]) -> Emulator {
    let emulator = emulator_holding(HOME_REGION, &[(RULE.to_string(), results.to_vec())]).await;
    put_whole_account_aggregator(&emulator).await;
    emulator
}

#[tokio::test]
async fn reports_the_compliance_each_rules_results_roll_up_to_against_its_source() {
    struct Case {
        case: &'static str,
        results: Vec<&'static str>,
        /// The `ComplianceType` and contributor count expected, or `None` for a rule
        /// with no compliance result to report.
        expected: Option<(&'static str, Option<Contributors>)>,
    }

    let cases = vec![
        Case {
            case: "a rule nothing has evaluated has no compliance result",
            results: Vec::new(),
            expected: None,
        },
        Case {
            case: "every evaluated resource compliant",
            results: vec![COMPLIANT],
            expected: Some((COMPLIANT, None)),
        },
        Case {
            case: "several compliant resources",
            results: vec![COMPLIANT, COMPLIANT, COMPLIANT],
            expected: Some((COMPLIANT, None)),
        },
        Case {
            case: "one noncompliant resource",
            results: vec![NON_COMPLIANT],
            expected: Some((NON_COMPLIANT, Some((1, false)))),
        },
        Case {
            case: "any noncompliant resource makes the rule noncompliant",
            results: vec![COMPLIANT, NON_COMPLIANT, COMPLIANT],
            expected: Some((NON_COMPLIANT, Some((1, false)))),
        },
        Case {
            case: "two noncompliant resources contribute two",
            results: vec![NON_COMPLIANT, NON_COMPLIANT],
            expected: Some((NON_COMPLIANT, Some((2, false)))),
        },
        Case {
            case: "results that place the rule nowhere are no compliance result",
            results: vec![NOT_APPLICABLE],
            expected: None,
        },
        Case {
            case: "an undecided result is no compliance result",
            results: vec![INSUFFICIENT_DATA, NOT_APPLICABLE],
            expected: None,
        },
        Case {
            case: "exactly at the contributor cap",
            results: vec![NON_COMPLIANT; RESOURCE_CAP],
            expected: Some((NON_COMPLIANT, Some((RESOURCE_CAP as i64, false)))),
        },
        Case {
            case: "one resource past the contributor cap",
            results: vec![NON_COMPLIANT; RESOURCE_CAP + 1],
            expected: Some((NON_COMPLIANT, Some((RESOURCE_CAP as i64, true)))),
        },
    ];

    for Case {
        case,
        results,
        expected,
    } in cases
    {
        let emulator = emulator_with_one_rule(&results).await;
        let body = emulator
            .ok(
                "DescribeAggregateComplianceByConfigRules",
                &describe_request(),
            )
            .await;
        let entries = reported(case, &body);

        match expected {
            None => assert!(
                entries.is_empty(),
                "case {case:?}: expected a rule with no compliance result to be left out, \
                 got {entries:?}"
            ),
            Some((compliance_type, contributor_count)) => {
                assert_eq!(
                    entries,
                    vec![Entry {
                        name: RULE.to_string(),
                        compliance_type: compliance_type.to_string(),
                        contributor_count,
                        account_id: account(),
                        aws_region: HOME_REGION.to_string(),
                    }],
                    "case {case:?}: expected the rule reported as {compliance_type} with \
                     contributors {contributor_count:?} against its source, got {entries:?}"
                );

                // The account's own report of the same rule's results: an aggregate read
                // of this account's data and the account's own compliance report cannot
                // disagree about the rule.
                let own = emulator
                    .ok("DescribeComplianceByConfigRule", &json!({}))
                    .await;
                let own_compliance = &own["ComplianceByConfigRules"][0]["Compliance"];
                assert_eq!(
                    own_compliance["ComplianceType"].as_str(),
                    Some(compliance_type),
                    "case {case:?}: expected the account's own report {own} to agree with the \
                     aggregated {compliance_type}"
                );
            }
        }

        assert!(
            next_token(&body).is_none(),
            "case {case:?}: expected no NextToken for a single rule, got {body}"
        );
    }
}

#[tokio::test]
async fn narrows_the_results_to_the_source_rule_and_compliance_the_filters_name() {
    // The rules every row of this table holds, by name and recorded results.
    let seeded: Vec<(String, Vec<&'static str>)> = vec![
        ("alpha".to_string(), vec![COMPLIANT]),
        ("beta".to_string(), vec![NON_COMPLIANT]),
        ("gamma".to_string(), vec![COMPLIANT, NON_COMPLIANT]),
        ("delta".to_string(), Vec::new()),
    ];
    let account = account();

    // (case, the request's Filters, the rule names expected)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filters report every rule with a compliance result",
            Value::Null,
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "one rule by name",
            json!({ "ConfigRuleName": "beta" }),
            vec!["beta"],
        ),
        (
            "a rule with no compliance result, by name",
            json!({ "ConfigRuleName": "delta" }),
            Vec::new(),
        ),
        (
            "a name no rule carries",
            json!({ "ConfigRuleName": "no-such-rule" }),
            Vec::new(),
        ),
        (
            "the compliant rules",
            json!({ "ComplianceType": COMPLIANT }),
            vec!["alpha"],
        ),
        (
            "the noncompliant rules",
            json!({ "ComplianceType": NON_COMPLIANT }),
            vec!["beta", "gamma"],
        ),
        (
            "a rule name and a compliance type that place it",
            json!({ "ConfigRuleName": "gamma", "ComplianceType": NON_COMPLIANT }),
            vec!["gamma"],
        ),
        (
            "a rule name and a compliance type that does not place it",
            json!({ "ConfigRuleName": "gamma", "ComplianceType": COMPLIANT }),
            Vec::new(),
        ),
        (
            "this source account",
            json!({ "AccountId": account }),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "another source account",
            json!({ "AccountId": OTHER_ACCOUNT }),
            Vec::new(),
        ),
        (
            "this source Region",
            json!({ "AwsRegion": HOME_REGION }),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "another source Region",
            json!({ "AwsRegion": OTHER_REGION }),
            Vec::new(),
        ),
        (
            "this account in this Region, one rule by name",
            json!({
                "AccountId": account,
                "AwsRegion": HOME_REGION,
                "ConfigRuleName": "alpha",
                "ComplianceType": COMPLIANT,
            }),
            vec!["alpha"],
        ),
        (
            "this account in another Region",
            json!({ "AccountId": account, "AwsRegion": OTHER_REGION }),
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in cases {
        let emulator = emulator_holding(HOME_REGION, &seeded).await;
        put_whole_account_aggregator(&emulator).await;

        let mut request = describe_request();
        if !filters.is_null() {
            request["Filters"] = filters;
        }
        let body = emulator
            .ok("DescribeAggregateComplianceByConfigRules", &request)
            .await;
        let names = reported_names(case, &body);
        assert_eq!(
            names, expected,
            "case {case:?}: expected the rules {expected:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn refuses_a_compliance_type_no_filter_may_name() {
    // (case, the ComplianceType filtered on, whether a filter may name it)
    let cases: [(&str, &str, bool); 4] = [
        ("compliant", COMPLIANT, true),
        ("noncompliant", NON_COMPLIANT, true),
        ("not applicable", NOT_APPLICABLE, false),
        ("insufficient data", INSUFFICIENT_DATA, false),
    ];

    for (case, compliance_type, filterable) in cases {
        let emulator = emulator_with_one_rule(&[COMPLIANT]).await;
        let mut request = describe_request();
        request["Filters"] = json!({ "ComplianceType": compliance_type });

        if filterable {
            let body = emulator
                .ok("DescribeAggregateComplianceByConfigRules", &request)
                .await;
            let names = reported_names(case, &body);
            let expected: Vec<String> = if compliance_type == COMPLIANT {
                vec![RULE.to_string()]
            } else {
                Vec::new()
            };
            assert_eq!(
                names, expected,
                "case {case:?}: expected the rules {expected:?}, got {names:?}"
            );
        } else {
            let (status, code) = emulator
                .refused("DescribeAggregateComplianceByConfigRules", &request)
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException for a ComplianceType \
                 no filter may name, got {status} {code}"
            );
        }
    }
}

#[tokio::test]
async fn reports_only_the_rules_of_the_sources_the_aggregator_takes_in() {
    let account = account();
    // (case, the aggregator's sources, the Region the request is signed for, whether the
    // rule is reported)
    let cases: Vec<(&str, Value, &str, bool)> = vec![
        (
            "this account in every Region",
            account_source(&[&account], &[]),
            HOME_REGION,
            true,
        ),
        (
            "this account in this Region",
            account_source(&[&account], &[HOME_REGION]),
            HOME_REGION,
            true,
        ),
        (
            "this account in another Region only",
            account_source(&[&account], &[OTHER_REGION]),
            HOME_REGION,
            false,
        ),
        (
            "another account in every Region",
            account_source(&[OTHER_ACCOUNT], &[]),
            HOME_REGION,
            false,
        ),
        (
            "this account among several sources",
            account_source(&[OTHER_ACCOUNT, &account], &[HOME_REGION, OTHER_REGION]),
            HOME_REGION,
            true,
        ),
        (
            "an organization over every Region",
            organization_source(&[]),
            HOME_REGION,
            true,
        ),
        (
            "an organization over another Region, read from that Region",
            organization_source(&[OTHER_REGION]),
            OTHER_REGION,
            true,
        ),
        (
            "an organization over another Region, read from this one",
            organization_source(&[OTHER_REGION]),
            HOME_REGION,
            false,
        ),
    ];

    for (case, sources, region, expected) in cases {
        let emulator =
            emulator_holding(region, &[(RULE.to_string(), vec![NON_COMPLIANT])]).await;
        put_aggregator(&emulator, sources).await;

        let body = emulator
            .ok(
                "DescribeAggregateComplianceByConfigRules",
                &describe_request(),
            )
            .await;
        let entries = reported(case, &body);

        if expected {
            assert_eq!(
                entries,
                vec![Entry {
                    name: RULE.to_string(),
                    compliance_type: NON_COMPLIANT.to_string(),
                    contributor_count: Some((1, false)),
                    account_id: account.clone(),
                    aws_region: region.to_string(),
                }],
                "case {case:?}: expected the aggregated rule against its own source, got \
                 {entries:?}"
            );
        } else {
            assert!(
                entries.is_empty(),
                "case {case:?}: expected no entry for a source the aggregator does not take \
                 in, got {entries:?}"
            );
        }
    }
}

#[tokio::test]
async fn pages_the_rules_at_the_limit_the_request_asks_for() {
    // (case, the page size asked for, how many rules the account holds)
    let cases: [(&str, usize, usize); 5] = [
        ("one rule per page", 1, 4),
        ("two rules per page", 2, 5),
        ("a page larger than the rules held", 10, 3),
        ("a page exactly as large as the rules held", 3, 3),
        ("nothing to page", 2, 0),
    ];

    for (case, page_size, held) in cases {
        let seeded: Vec<(String, Vec<&'static str>)> = (0..held)
            .map(|ordinal| (format!("rule-{ordinal:02}"), vec![COMPLIANT]))
            .collect();
        let emulator = emulator_holding(HOME_REGION, &seeded).await;
        put_whole_account_aggregator(&emulator).await;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = describe_request();
            request["Limit"] = json!(page_size);
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let body = emulator
                .ok("DescribeAggregateComplianceByConfigRules", &request)
                .await;
            let names = reported_names(case, &body);
            assert!(
                names.len() <= page_size,
                "case {case:?}: expected at most {page_size} rules per page, got {names:?}"
            );
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= held + 1,
                "case {case:?}: the page walk did not terminate, got {walked:?}"
            );
            token = next_token(&body);
            if token.is_none() {
                break;
            }
        }

        let expected: Vec<String> = seeded.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the page walk to serve every rule once, in name order, \
             got {walked:?}"
        );
    }
}

#[tokio::test]
async fn serves_every_published_limit_and_refuses_one_outside_the_range() {
    // (case, the Limit asked for, whether it is served)
    let cases: [(&str, i64, bool); 5] = [
        ("the published minimum asks for the default", MIN_LIMIT, true),
        ("one rule per page", 1, true),
        ("the published maximum", MAX_LIMIT, true),
        ("one past the maximum", MAX_LIMIT + 1, false),
        ("one past the minimum", MIN_LIMIT - 1, false),
    ];

    for (case, limit, served) in cases {
        let emulator = emulator_with_one_rule(&[COMPLIANT]).await;
        let mut request = describe_request();
        request["Limit"] = json!(limit);

        if served {
            let body = emulator
                .ok("DescribeAggregateComplianceByConfigRules", &request)
                .await;
            let names = reported_names(case, &body);
            assert_eq!(
                names,
                vec![RULE.to_string()],
                "case {case:?}: expected the aggregated rule, got {names:?}"
            );
            assert!(
                next_token(&body).is_none(),
                "case {case:?}: expected no NextToken once every rule is served, got {body}"
            );
        } else {
            // The shared request validation refuses the modelled range before the
            // handler's own `InvalidLimitException` can answer for it, so either code is
            // a correct refusal of a limit outside the allowable range.
            let (status, code) = emulator
                .refused("DescribeAggregateComplianceByConfigRules", &request)
                .await;
            assert!(
                status == 400
                    && ["InvalidLimitException", "ValidationException"].contains(&code.as_str()),
                "case {case:?}: expected HTTP 400 for a limit outside the range, got \
                 {status} {code}"
            );
        }
    }
}

#[tokio::test]
async fn refuses_an_unknown_aggregator_and_a_token_it_did_not_issue() {
    let emulator = emulator_with_one_rule(&[COMPLIANT]).await;

    // (case, the request, the expected error code)
    let cases: [(&str, Value, &str); 3] = [
        (
            "an aggregator that does not exist",
            json!({ "ConfigurationAggregatorName": "no-such-aggregator" }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "an aggregator that does not exist, with filters",
            json!({
                "ConfigurationAggregatorName": "no-such-aggregator",
                "Filters": { "ComplianceType": COMPLIANT },
            }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "a token this operation did not issue",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "NextToken": "not-a-token-this-operation-issued!",
            }),
            "InvalidNextTokenException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, code) = emulator
            .refused("DescribeAggregateComplianceByConfigRules", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn reporting_the_rules_changes_nothing_a_later_caller_sees() {
    let emulator = emulator_with_one_rule(&[COMPLIANT, NON_COMPLIANT]).await;
    let case = "a report is repeatable";

    let first = emulator
        .ok(
            "DescribeAggregateComplianceByConfigRules",
            &describe_request(),
        )
        .await;
    let second = emulator
        .ok(
            "DescribeAggregateComplianceByConfigRules",
            &describe_request(),
        )
        .await;
    assert_eq!(
        reported(case, &first),
        reported(case, &second),
        "case {case:?}: expected the second report {second} to match the first {first}"
    );

    // The rules and their results are untouched, so the account's own report still
    // places the rule where the aggregate read did.
    let own = emulator
        .ok("DescribeComplianceByConfigRule", &json!({}))
        .await;
    let compliance = &own["ComplianceByConfigRules"][0]["Compliance"];
    assert_eq!(
        compliance["ComplianceType"].as_str(),
        Some(NON_COMPLIANT),
        "case {case:?}: expected the rule's own compliance to be unchanged, got {own}"
    );
}
