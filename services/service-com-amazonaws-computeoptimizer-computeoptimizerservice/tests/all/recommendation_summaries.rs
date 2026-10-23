// SPDX-License-Identifier: CC0-1.0

//! `GetRecommendationSummaries`, end to end over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests
//! prove is what a client sees — never what the state struct holds.
//!
//! The rule under test: a summary is the account's analysed resources OF ONE RESOURCE TYPE
//! aggregated — a count at each `Finding` the service reports for that resource type (and
//! at each `FindingReasonCode` behind it), a count at each `CurrentPerformanceRisk`, a
//! count at each `IdleFinding` found, and what adopting the recommendations would save per
//! month with that saving as a percentage of what the resources cost. So every figure in
//! one summary is a different view of the SAME rows, and the assertions below are the
//! identities that holds: the risk ratings total the finding counts, a reason code is
//! counted no more often than the finding it explains, and the aggregated saving is the
//! rightsizing saving plus the idle one.

use serde_json::{json, Value};

use crate::harness::{call, emulator, error_code, opt_in};

/// The account every caller of this emulator is (`engine::emulator_account_id`), and so
/// the account whose findings a summary reports.
fn caller_account() -> String {
    engine::emulator_account_id().to_string()
}

/// The `Finding` values a summary of each resource type reports a count for.
///
/// Source: the `GetRecommendationSummaries` documentation in this service's Smithy model —
/// "It returns the number of: Amazon EC2 instances in an account that are
/// Underprovisioned, Overprovisioned, or Optimized. EC2 Auto Scaling groups in an account
/// that are NotOptimized, or Optimized. Amazon EBS volumes in an account that are
/// NotOptimized, or Optimized. Lambda functions in an account that are NotOptimized, or
/// Optimized. Amazon ECS services in an account that are Underprovisioned,
/// Overprovisioned, or Optimized. Commercial software licenses in an account that are
/// InsufficientMetrics, NotOptimized or Optimized. Amazon Aurora and Amazon RDS databases
/// in an account that are Underprovisioned, Overprovisioned, Optimized, or NotOptimized."
///
/// `Summary$name` targets the model's `Finding` enum (`NotOptimized` | `Optimized` |
/// `Overprovisioned` | `Underprovisioned`), so a license's documented
/// `InsufficientMetrics` — a `LicenseFinding` value — is not a count a `Summary` can
/// carry and is not expected here.
const FINDINGS_BY_RESOURCE_TYPE: &[(&str, &[&str])] = &[
    (
        "Ec2Instance",
        &["Underprovisioned", "Overprovisioned", "Optimized"],
    ),
    ("AutoScalingGroup", &["NotOptimized", "Optimized"]),
    ("EbsVolume", &["NotOptimized", "Optimized"]),
    ("LambdaFunction", &["NotOptimized", "Optimized"]),
    (
        "EcsService",
        &["Underprovisioned", "Overprovisioned", "Optimized"],
    ),
    ("License", &["NotOptimized", "Optimized"]),
    (
        "RdsDBInstance",
        &[
            "Underprovisioned",
            "Overprovisioned",
            "Optimized",
            "NotOptimized",
        ],
    ),
];

/// The model's `FindingReasonCode` enum (`crate::ops::types::FINDING_REASON_CODE_VALUES`),
/// the only values `ReasonCodeSummary$name` can carry.
const FINDING_REASON_CODES: &[&str] = &["MemoryOverprovisioned", "MemoryUnderprovisioned"];

/// The model's `InferredWorkloadType` enum
/// (`crate::ops::types::INFERRED_WORKLOAD_TYPE_VALUES`).
const INFERRED_WORKLOAD_TYPES: &[&str] = &[
    "AmazonEmr",
    "ApacheCassandra",
    "ApacheHadoop",
    "Kafka",
    "Memcached",
    "Nginx",
    "PostgreSql",
    "Redis",
    "SQLServer",
];

/// Read one page of summaries, asserting the call succeeded.
async fn get_summaries(router: &smithy_server_runtime::MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "GetRecommendationSummaries", request.clone()).await;
    assert_eq!(
        status, 200,
        "summaries for {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The summaries of one response, as a list.
fn summaries_of(body: &Value) -> Vec<Value> {
    body["recommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("recommendationSummaries must be a list, got {body}"))
        .clone()
}

/// A `number` member of a response object, which the wire carries as a JSON number.
fn number(value: &Value, case: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a number, got {value}"))
}

/// Before the account opts in there are no findings to summarise at all, and the service
/// says so with the modelled `OptInRequiredException` (HTTP 403) rather than an empty
/// answer; once it opts in, every summary it reports is the caller's own account's.
#[tokio::test]
async fn summaries_need_the_account_to_be_opted_in_and_report_the_caller_account() {
    let router = emulator();

    let (status, body) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 403,
        "before opting in: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "before opting in: expected OptInRequiredException, got {body}"
    );

    opt_in(&router).await;

    let body = get_summaries(&router, json!({})).await;
    let summaries = summaries_of(&body);
    assert!(
        !summaries.is_empty(),
        "after opting in: the account's analysed resources must be summarised, got {body}"
    );
    for summary in &summaries {
        let case = summary["recommendationResourceType"].to_string();
        assert_eq!(
            summary["accountId"].as_str(),
            Some(caller_account().as_str()),
            "case {case:?}: a summary reports the account whose resources it covers, got \
             {summary}"
        );
        assert!(
            FINDINGS_BY_RESOURCE_TYPE
                .iter()
                .any(|(resource_type, _)| summary["recommendationResourceType"] == *resource_type),
            "case {case:?}: recommendationResourceType must be a resource type the service \
             summarises, got {summary}"
        );
    }

    // Opting back out takes the findings away again: the gate is the stored enrollment,
    // not whether the account ever enrolled.
    let (status, body) = call(
        &router,
        "UpdateEnrollmentStatus",
        json!({"status": "Inactive"}),
    )
    .await;
    assert_eq!(status, 200, "opting out: expected 200, got {status}: {body}");
    let (status, body) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 403,
        "after opting out: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "after opting out: expected OptInRequiredException, got {body}"
    );
}

/// Each summary is one resource type's analysed resources aggregated, so its parts are the
/// identities of that one set of rows: the findings it reports are exactly the ones the
/// service documents for that resource type, the performance-risk ratings total the
/// finding counts, a reason code is counted no more often than the finding it explains,
/// the idle counts are a subset of the resources, and the aggregated saving is the
/// rightsizing saving plus the idle one — every saving in USD, every percentage within
/// 0..=100 and zero exactly when the saving is zero.
#[tokio::test]
async fn every_summary_is_its_resource_type_s_resources_counted_and_costed() {
    let router = emulator();
    opt_in(&router).await;

    let body = get_summaries(&router, json!({})).await;
    let summaries = summaries_of(&body);
    assert!(
        summaries.len() > 1,
        "the catalog must span more than one resource type for this to prove anything, got \
         {body}"
    );

    for summary in &summaries {
        let resource_type = summary["recommendationResourceType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let case = resource_type.clone();
        let (_, expected_findings) = FINDINGS_BY_RESOURCE_TYPE
            .iter()
            .find(|(name, _)| *name == resource_type)
            .unwrap_or_else(|| panic!("case {case:?}: unknown resource type in {summary}"));

        let finding_summaries = summary["summaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: summaries must be a list, got {summary}"))
            .clone();
        let reported: Vec<&str> = finding_summaries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, *expected_findings,
            "case {case:?}: expected a count at each documented finding {expected_findings:?}, \
             got {reported:?}"
        );

        let mut resources = 0.0;
        for entry in &finding_summaries {
            let finding = entry["name"].as_str().unwrap_or_default().to_string();
            let count = number(&entry["value"], &format!("{case}/{finding}"));
            assert!(
                count >= 0.0 && count.fract() == 0.0,
                "case {case:?}/{finding:?}: a finding count is a whole number of resources, got \
                 {count}"
            );
            resources += count;

            if let Some(reason_codes) = entry["reasonCodeSummaries"].as_array() {
                for reason in reason_codes {
                    let name = reason["name"].as_str().unwrap_or_default();
                    assert!(
                        FINDING_REASON_CODES.contains(&name),
                        "case {case:?}/{finding:?}: {name:?} is not a FindingReasonCode, got \
                         {reason}"
                    );
                    let reason_count = number(&reason["value"], &format!("{case}/{finding}"));
                    assert!(
                        reason_count > 0.0 && reason_count <= count,
                        "case {case:?}/{finding:?}: a reason code explains at most the {count} \
                         resources at that finding, got {reason_count}"
                    );
                }
            }
        }
        assert!(
            resources > 0.0,
            "case {case:?}: a resource type is summarised only where the account has analysed \
             resources of it, got {summary}"
        );

        let ratings = &summary["currentPerformanceRiskRatings"];
        let rated = ["high", "medium", "low", "veryLow"]
            .iter()
            .map(|rating| number(&ratings[*rating], &format!("{case}/{rating}")))
            .sum::<f64>();
        assert_eq!(
            rated, resources,
            "case {case:?}: every analysed resource runs exactly one performance risk, so the \
             ratings must total the {resources} resources counted by finding, got {ratings}"
        );

        // An idle resource is a record of its own — what `GetIdleRecommendations` reports
        // — and nothing has written one, so no summary counts an idle resource and the
        // aggregated saving is the rightsizing one. The two readers must agree, which the
        // assertion after this loop pins.
        assert!(
            summary["idleSummaries"].is_null(),
            "case {case:?}: idleSummaries is reported only where resources were found idle, \
             got {summary}"
        );
        assert!(
            summary["idleSavingsOpportunity"].is_null(),
            "case {case:?}: idleSavingsOpportunity is reported only where resources were found \
             idle, got {summary}"
        );

        let rightsizing = savings_value(summary, "savingsOpportunity", &case);
        let aggregated = savings_value(summary, "aggregatedSavingsOpportunity", &case);
        assert_eq!(
            aggregated, rightsizing,
            "case {case:?}: with nothing found idle the aggregated saving is the rightsizing \
             saving {rightsizing}, got {aggregated}"
        );

        if let Some(inferred) = summary["inferredWorkloadSavings"].as_array() {
            assert!(
                !inferred.is_empty() && inferred.len() <= 5,
                "case {case:?}: inferredWorkloadSavings reports the top five opportunities at \
                 most, got {} of them",
                inferred.len()
            );
            let mut previous = f64::INFINITY;
            for entry in inferred {
                let types = entry["inferredWorkloadTypes"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a type list in {entry}"))
                    .clone();
                assert!(
                    !types.is_empty(),
                    "case {case:?}: an inferred-workload saving names the workload types it is \
                     for, got {entry}"
                );
                for workload in &types {
                    let name = workload.as_str().unwrap_or_default();
                    assert!(
                        INFERRED_WORKLOAD_TYPES.contains(&name),
                        "case {case:?}: {name:?} is not an InferredWorkloadType, got {entry}"
                    );
                }
                let value = number(&entry["estimatedMonthlySavings"]["value"], &case);
                assert!(
                    value > 0.0 && value <= aggregated,
                    "case {case:?}: an inferred-workload saving is part of the {aggregated} this \
                     resource type can save, got {value}"
                );
                assert!(
                    value <= previous,
                    "case {case:?}: inferred-workload savings are reported largest first, got \
                     {value} after {previous}"
                );
                previous = value;
            }
        }
    }

    // The agreement the summaries' absent idle members rest on: the idle record this
    // emulator reports from holds nothing, so there is no idle resource for a summary to
    // count.
    let (status, idle) = call(&router, "GetIdleRecommendations", json!({})).await;
    assert_eq!(
        status, 200,
        "reading the idle record: expected 200, got {status}: {idle}"
    );
    assert_eq!(
        idle["idleRecommendations"],
        json!([]),
        "no summary counts an idle resource, so the idle record must hold none either, got \
         {idle}"
    );
}

/// One savings opportunity of a summary, asserting the invariants every one of them holds:
/// a non-negative amount in a currency the model names, and a percentage of monthly cost
/// within 0..=100 that is zero exactly when the amount is.
fn savings_value(summary: &Value, member: &str, case: &str) -> f64 {
    let opportunity = &summary[member];
    let value = number(&opportunity["estimatedMonthlySavings"]["value"], case);
    assert!(
        value >= 0.0,
        "case {case:?}/{member}: a saving is never negative, got {value}"
    );
    // The model's `Currency` enum (`crate::ops::types::CURRENCY_VALUES`).
    let currency = opportunity["estimatedMonthlySavings"]["currency"]
        .as_str()
        .unwrap_or_default();
    assert!(
        ["USD", "CNY"].contains(&currency),
        "case {case:?}/{member}: {currency:?} is not a Currency, got {opportunity}"
    );
    let percentage = number(&opportunity["savingsOpportunityPercentage"], case);
    assert!(
        (0.0..=100.0).contains(&percentage),
        "case {case:?}/{member}: a saving is a percentage of monthly cost, got {percentage}"
    );
    assert_eq!(
        percentage == 0.0,
        value == 0.0,
        "case {case:?}/{member}: a percentage is zero exactly when the saving is, got \
         {percentage} for {value}"
    );
    value
}

/// `accountIds` selects WHOSE findings are summarised: absent or the caller's own account
/// reads the caller's, any other account reads nothing (this emulator presents one
/// account), and more than one account id is refused — "Only one account ID can be
/// specified per request."
#[tokio::test]
async fn account_ids_selects_one_account_and_refuses_more_than_one() {
    let router = emulator();
    opt_in(&router).await;

    let own = caller_account();
    let unpaged = summaries_of(&get_summaries(&router, json!({})).await);

    // Rows: the member is absent, empty, names the caller, and names another account.
    let cases: [(&str, Value, bool); 4] = [
        ("absent", json!({}), true),
        ("empty", json!({"accountIds": []}), true),
        ("own account", json!({"accountIds": [own.clone()]}), true),
        (
            "another account",
            json!({"accountIds": ["111122223333"]}),
            false,
        ),
    ];
    for (case, request, is_own) in cases {
        let body = get_summaries(&router, request.clone()).await;
        let summaries = summaries_of(&body);
        if is_own {
            assert_eq!(
                summaries, unpaged,
                "case {case:?}: expected the caller's own summaries, got {body}"
            );
        } else {
            assert!(
                summaries.is_empty(),
                "case {case:?}: this emulator holds no other account's resources, so nothing is \
                 summarised, got {body}"
            );
        }
    }

    // Rows: two distinct ids, and the same id twice — both ask for more than one account.
    for accounts in [vec![own.clone(), "111122223333".to_string()], vec![
        own.clone(),
        own.clone(),
    ]] {
        let case = format!("{accounts:?}");
        let (status, body) = call(
            &router,
            "GetRecommendationSummaries",
            json!({"accountIds": accounts}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: only one account ID can be specified per request, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// Paging never changes WHAT is summarised: walking the summaries `maxResults` at a time
/// yields exactly the unpaged list, in the same order, with a `nextToken` on every page
/// but the last and none on it. A forged token is refused.
#[tokio::test]
async fn paging_walks_the_same_summaries_in_the_same_order() {
    let router = emulator();
    opt_in(&router).await;

    let unpaged = summaries_of(&get_summaries(&router, json!({})).await);
    assert!(
        unpaged.len() > 2,
        "this test needs several summaries to page over, got {}",
        unpaged.len()
    );

    // Rows: the smallest page, a page that divides the set unevenly, one that holds it
    // all, the modelled upper bound of `maxResults`, its lower bound `0` (no size asked
    // for) and the member left out entirely.
    let page_sizes: [Option<i64>; 6] = [
        Some(1),
        Some(2),
        Some(unpaged.len() as i64),
        Some(1000),
        Some(0),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let body = get_summaries(&router, request).await;
            let page = summaries_of(&body);
            pages += 1;
            assert!(
                pages <= unpaged.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            if let Some(size) = page_size.filter(|size| *size > 0) {
                assert!(
                    page.len() as i64 <= size,
                    "case {case:?}: a page holds at most {size} summaries, got {}",
                    page.len()
                );
            }
            walked.extend(page);
            next_token = body["nextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, unpaged,
            "case {case:?}: paging must walk exactly the unpaged summaries, in order"
        );
        let expected_pages = match page_size.filter(|size| *size > 0) {
            // One page per whole `size`, plus one more for any remainder.
            Some(size) => unpaged.len().div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {} summaries, got {pages}",
            unpaged.len()
        );
    }

    // Rows: a token that is not hex, one that is hex but not a token this service issued,
    // and one a client built from a resource type it can see.
    for token in ["not-a-token", "6e6f742d612d746f6b656e", "Ec2Instance"] {
        let (status, body) = call(
            &router,
            "GetRecommendationSummaries",
            json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token this service did not issue names no page, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {token:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// Summarising is a READ: two identical calls answer identically, and reading the
/// summaries moves neither the enrollment record nor the stored preferences.
#[tokio::test]
async fn reading_summaries_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    opt_in(&router).await;

    let before = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    let first = get_summaries(&router, json!({})).await;
    let second = get_summaries(&router, json!({})).await;
    assert_eq!(
        first, second,
        "two reads of the same findings must answer identically"
    );

    let after = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    assert_eq!(
        before, after,
        "summarising findings must not move the enrollment record"
    );
    let preferences = call(
        &router,
        "GetRecommendationPreferences",
        json!({"resourceType": "Ec2Instance"}),
    )
    .await
    .1;
    assert_eq!(
        preferences["recommendationPreferencesDetails"],
        json!([]),
        "summarising findings must not write a preference, got {preferences}"
    );
}
