// SPDX-License-Identifier: CC0-1.0

//! `GetLambdaFunctionRecommendations` over the wire: the recommendation this service derives
//! from each observed Lambda function, and how a request narrows, pages and refuses.
//!
//! Every assertion here is on the RULE, checked over a set of functions that differ in
//! configured memory, demand shape, invocation rate, version (published and `$LATEST`) and
//! how long they have been observed — the response itself carries the analysed metrics, so
//! an option can be checked against the demand it has to carry without this suite restating
//! the handler's arithmetic.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::ANALYSIS_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, call_in_region, emulator, error_code, opt_in};

/// The memory a Lambda function can be configured with, from the AWS Lambda quotas: "128 MB
/// to 10,240 MB, in 1-MB increments". No memory size a recommendation names may fall outside
/// this range.
const MIN_MEMORY_MB: f64 = 128.0;
const MAX_MEMORY_MB: f64 = 10_240.0;

/// The headroom a recommendation option is sized with, which the service documents as the
/// only headroom it names ("`PERCENT_20` for headroom",
/// `PutRecommendationPreferences$utilizationPreferences`).
const HEADROOM: f64 = 0.20;

/// How many invocations the analysed period has to hold for Compute Optimizer to reach a
/// finding for a function (the "Supported resources and requirements" page the
/// `InsufficientData` reason code links: at least 50 invocations over the look-back period).
const MIN_ANALYSED_INVOCATIONS: f64 = 50.0;

/// The `LambdaFunctionRecommendationFinding` values
/// (`crate::ops::types::LAMBDA_FUNCTION_RECOMMENDATION_FINDING_VALUES`).
const FINDINGS: &[&str] = &["Optimized", "NotOptimized", "Unavailable"];

/// The `LambdaFunctionRecommendationFindingReasonCode` values
/// (`crate::ops::types::LAMBDA_FUNCTION_RECOMMENDATION_FINDING_REASON_CODE_VALUES`).
const FINDING_REASON_CODES: &[&str] = &[
    "MemoryOverprovisioned",
    "MemoryUnderprovisioned",
    "InsufficientData",
    "Inconclusive",
];

/// The seeded functions, by name — one per case a function recommendation distinguishes.
const OVER_PROVISIONED: &str = "invoice-renderer";
const UNDER_PROVISIONED_VERSION: &str = "thumbnail-resizer";
const ALREADY_RIGHT_SIZED: &str = "order-dispatcher";
const TOO_FEW_INVOCATIONS: &str = "log-forwarder";
const NO_MEMORY_OBSERVED: &str = "nightly-reconciler";

/// Opt the account in and let the analysis its enrollment starts come due — the gate every
/// recommendation reader of this service applies. The span is the service's own
/// ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// The unqualified ARN of one seeded function, in `us-east-1`.
fn function_arn(name: &str) -> String {
    format!(
        "arn:aws:lambda:us-east-1:{}:function:{name}",
        ::engine::emulator_account_id()
    )
}

/// The qualified ARN of one version of a seeded function.
fn qualified_arn(name: &str, version: &str) -> String {
    format!("{}:{version}", function_arn(name))
}

/// Read function recommendations, asserting the call was served.
async fn recommendations(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "GetLambdaFunctionRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The recommendations of one response, in the order it reports them.
fn reported(body: &Value) -> Vec<Value> {
    body["lambdaFunctionRecommendations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("lambdaFunctionRecommendations must be a list, got {body}")
        })
        .clone()
}

/// The ARN of every recommendation in a response, in the order it reports them.
fn reported_arns(body: &Value) -> Vec<String> {
    reported(body)
        .iter()
        .map(|recommendation| {
            recommendation["functionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The recommendation a response carries for one seeded function, by name.
fn recommendation_for(body: &Value, name: &str) -> Value {
    reported(body)
        .into_iter()
        .find(|recommendation| {
            recommendation["functionArn"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!(":function:{name}"))
        })
        .unwrap_or_else(|| panic!("case {name:?}: no recommendation for it: {body}"))
}

/// A number the wire carries, or `0` for one the protocol omitted because it equals the
/// member's modelled default.
fn number(value: &Value, member: &str) -> f64 {
    value[member].as_f64().unwrap_or(0.0)
}

/// One reported `utilizationMetrics` entry's value.
fn metric(recommendation: &Value, name: &str, statistic: &str) -> f64 {
    recommendation["utilizationMetrics"]
        .as_array()
        .unwrap_or_else(|| panic!("utilizationMetrics must be a list: {recommendation}"))
        .iter()
        .find(|entry| entry["name"] == json!(name) && entry["statistic"] == json!(statistic))
        .map(|entry| number(entry, "value"))
        .unwrap_or_else(|| panic!("no {name}/{statistic} metric: {recommendation}"))
}

/// The single memory size recommendation option of a recommendation.
fn only_option(recommendation: &Value) -> Value {
    let options = recommendation["memorySizeRecommendationOptions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("memorySizeRecommendationOptions must be a list: {recommendation}")
        });
    assert_eq!(
        options.len(),
        1,
        "case {}: expected one memory size option, got {}: {recommendation}",
        recommendation["functionArn"],
        options.len(),
    );
    options[0].clone()
}

/// The finding reason codes a recommendation reports.
fn reason_codes(recommendation: &Value) -> Vec<String> {
    recommendation["findingReasonCodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|code| code.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every invariant a function recommendation satisfies, whatever function it is for and
/// whatever look-back period it was generated under.
///
/// The rule: a recommendation is the function's OBSERVATION read three ways — the memory
/// size it names is the smallest whole MB inside Lambda's range that carries the reported
/// memory peak with headroom; the finding and its reason code say which side of the
/// configured memory that lands on; and the saving is what the move is worth, never negative
/// and never claimed where the move costs the same or more.
fn assert_recommendation_invariants(recommendation: &Value, look_back_days: f64) {
    let case = recommendation["functionArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    assert_eq!(
        recommendation["accountId"].as_str(),
        Some(::engine::emulator_account_id()),
        "case {case:?}: a recommendation names the account that owns the function, got \
         {recommendation}"
    );
    let version = recommendation["functionVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a recommendation names the function version"));
    // An unqualified ARN is the `$LATEST` version; a published version is qualified with it.
    let qualified = case.ends_with(&format!(":{version}"));
    assert_eq!(
        qualified,
        version != "$LATEST",
        "case {case:?}: a published version is named by a qualified ARN and $LATEST by an \
         unqualified one, got version {version:?}"
    );

    let finding = recommendation["finding"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a recommendation carries a finding"));
    assert!(
        FINDINGS.contains(&finding),
        "case {case:?}: {finding:?} is not a LambdaFunctionRecommendationFinding, got \
         {recommendation}"
    );
    for code in reason_codes(recommendation) {
        assert!(
            FINDING_REASON_CODES.contains(&code.as_str()),
            "case {case:?}: {code:?} is not a LambdaFunctionRecommendationFindingReasonCode, \
             got {recommendation}"
        );
    }

    let analysed = number(recommendation, "lookbackPeriodInDays");
    assert!(
        analysed > 0.0 && analysed <= look_back_days,
        "case {case:?}: the analysed period is at most the {look_back_days} days the \
         look-back period in force covers, got {analysed}"
    );
    let invocations = number(recommendation, "numberOfInvocations");
    assert!(
        invocations >= 0.0 && invocations.fract() == 0.0,
        "case {case:?}: numberOfInvocations is a whole number of invocations, got {invocations}"
    );

    let current_memory = number(recommendation, "currentMemorySize");
    assert!(
        (MIN_MEMORY_MB..=MAX_MEMORY_MB).contains(&current_memory),
        "case {case:?}: a function's memory is within Lambda's {MIN_MEMORY_MB}..={MAX_MEMORY_MB} \
         MB range, got {current_memory}"
    );
    let peak_memory = metric(recommendation, "Memory", "Maximum");
    let average_memory = metric(recommendation, "Memory", "Average");
    assert!(
        average_memory <= peak_memory,
        "case {case:?}: the mean memory an invocation used is at most the peak {peak_memory}, \
         got {average_memory}"
    );
    let average_duration = metric(recommendation, "Duration", "Average");
    assert!(
        average_duration <= metric(recommendation, "Duration", "Maximum"),
        "case {case:?}: the mean duration is at most the longest invocation, got {recommendation}"
    );

    let risk = recommendation["currentPerformanceRisk"]
        .as_str()
        .unwrap_or_default();
    // The model's `CurrentPerformanceRisk` enum.
    assert!(
        ["VeryLow", "Low", "Medium", "High"].contains(&risk),
        "case {case:?}: {risk:?} is not a CurrentPerformanceRisk, got {recommendation}"
    );
    assert_eq!(
        recommendation["effectiveRecommendationPreferences"]["savingsEstimationMode"]["source"],
        json!("PublicPricing"),
        "case {case:?}: this service's money figures come from public pricing, got \
         {recommendation}"
    );

    if finding == "Unavailable" {
        // "Compute Optimizer was unable to generate a recommendation for the function", so
        // there is no memory size to move it to.
        assert!(
            recommendation["memorySizeRecommendationOptions"].is_null(),
            "case {case:?}: an unavailable finding has no memory size option, got \
             {recommendation}"
        );
        let codes = reason_codes(recommendation);
        assert!(
            codes.iter().all(|code| code == "InsufficientData" || code == "Inconclusive"),
            "case {case:?}: an unavailable finding is explained by InsufficientData or \
             Inconclusive, got {codes:?}"
        );
        return;
    }

    // A finding was reached, so the function qualified for a recommendation.
    assert!(
        invocations >= MIN_ANALYSED_INVOCATIONS,
        "case {case:?}: a function is analysed only once it has been invoked at least \
         {MIN_ANALYSED_INVOCATIONS} times in the analysed period, got {invocations}"
    );
    assert!(
        peak_memory > 0.0,
        "case {case:?}: a finding is reached only where memory utilization was observed, got \
         {recommendation}"
    );

    let option = only_option(recommendation);
    assert_eq!(
        number(&option, "rank"),
        1.0,
        "case {case:?}: the one option reported is the first-ranked one, got {option}"
    );
    let memory_size = number(&option, "memorySize");
    // The rule: the smallest whole MB inside Lambda's range that carries the observed peak
    // with HEADROOM to spare.
    let expected_memory = (peak_memory * (1.0 + HEADROOM))
        .ceil()
        .clamp(MIN_MEMORY_MB, MAX_MEMORY_MB);
    assert_eq!(
        memory_size, expected_memory,
        "case {case:?}: a {peak_memory} MB peak with {HEADROOM} headroom asks for \
         {expected_memory} MB, got {memory_size}"
    );

    // The finding is which side of the configured memory that lands on, and its reason code
    // says the same thing in the model's words.
    let codes = reason_codes(recommendation);
    if memory_size == current_memory {
        assert_eq!(
            finding, "Optimized",
            "case {case:?}: a function already at the recommended {memory_size} MB is \
             optimized, got {finding:?}"
        );
        assert!(
            codes.is_empty(),
            "case {case:?}: an optimized function has no finding reason code, got {codes:?}"
        );
    } else {
        assert_eq!(
            finding, "NotOptimized",
            "case {case:?}: a function the analysis would move from {current_memory} MB to \
             {memory_size} MB is not optimized, got {finding:?}"
        );
        let expected_code = if memory_size < current_memory {
            "MemoryOverprovisioned"
        } else {
            "MemoryUnderprovisioned"
        };
        assert_eq!(
            codes,
            vec![expected_code.to_string()],
            "case {case:?}: {current_memory} MB against a recommended {memory_size} MB is \
             {expected_code}, got {codes:?}"
        );
    }

    // The projected duration: one range, on the one metric `LambdaFunctionMemoryMetricName`
    // declares, bounding the value expected inside it.
    let projected = option["projectedUtilizationMetrics"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: an option projects the function's duration"));
    let projected_at = |statistic: &str| -> f64 {
        projected
            .iter()
            .find(|entry| entry["statistic"] == json!(statistic))
            .map(|entry| {
                assert_eq!(
                    entry["name"],
                    json!("Duration"),
                    "case {case:?}: Duration is the only projected metric, got {entry}"
                );
                number(entry, "value")
            })
            .unwrap_or_else(|| panic!("case {case:?}: no projected {statistic}: {option}"))
    };
    let (lower, expected, upper) = (
        projected_at("LowerBound"),
        projected_at("Expected"),
        projected_at("UpperBound"),
    );
    assert!(
        lower <= expected && expected <= upper,
        "case {case:?}: the expected duration lies inside the projected {lower}..={upper} \
         range, got {expected}"
    );
    if memory_size == current_memory {
        assert_eq!(
            expected, average_duration,
            "case {case:?}: at the memory it already runs on, the projected duration is the \
             duration observed, got {expected}"
        );
    }

    // The saving: never negative, in a currency the model names, a percentage of monthly
    // cost that is zero exactly when the amount is, and the same before and after discounts
    // (this service records no negotiated discount).
    for member in ["savingsOpportunity", "savingsOpportunityAfterDiscounts"] {
        let opportunity = &option[member];
        let value = number(&opportunity["estimatedMonthlySavings"], "value");
        let percentage = number(opportunity, "savingsOpportunityPercentage");
        assert!(
            value >= 0.0,
            "case {case:?}/{member}: a saving is never negative, got {value}"
        );
        assert_eq!(
            opportunity["estimatedMonthlySavings"]["currency"],
            json!("USD"),
            "case {case:?}/{member}: the prices this service holds are USD, got {opportunity}"
        );
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
        if memory_size >= current_memory {
            // More memory never costs less, and the memory it already runs on costs the
            // same, so neither move saves anything.
            assert_eq!(
                value, 0.0,
                "case {case:?}/{member}: moving from {current_memory} MB to {memory_size} MB \
                 saves nothing, got {value}"
            );
        }
    }
}

/// Every recommendation the account's inventory yields satisfies the rule, across functions
/// that differ in configured memory, demand, invocation rate, version and observed days —
/// and the over-provisioned one is the case that actually saves money.
#[tokio::test]
async fn every_recommendation_is_its_function_s_observation_derived() {
    let router = emulator();
    analysed(&router).await;

    let body = recommendations(&router, json!({})).await;
    let all = reported(&body);
    assert!(
        all.len() > 2,
        "the inventory must span several functions for this to prove anything, got {body}"
    );
    for recommendation in &all {
        assert_recommendation_invariants(recommendation, 14.0);
    }

    // The three cases a finding distinguishes, each reached from a different observation.
    let over = recommendation_for(&body, OVER_PROVISIONED);
    assert_eq!(
        over["finding"],
        json!("NotOptimized"),
        "case {OVER_PROVISIONED:?}: a function peaking far below its memory is not \
         optimized, got {over}"
    );
    let saved = number(
        &only_option(&over)["savingsOpportunity"]["estimatedMonthlySavings"],
        "value",
    );
    assert!(
        saved > 0.0,
        "case {OVER_PROVISIONED:?}: sizing an over-provisioned function down saves money, \
         got {saved}"
    );

    let under = recommendation_for(&body, UNDER_PROVISIONED_VERSION);
    assert_eq!(
        under["functionVersion"],
        json!("3"),
        "case {UNDER_PROVISIONED_VERSION:?}: a published version reports its own version \
         number, got {under}"
    );
    assert_eq!(
        reason_codes(&under),
        vec!["MemoryUnderprovisioned".to_string()],
        "case {UNDER_PROVISIONED_VERSION:?}: a function peaking at its memory limit is \
         under-provisioned, got {under}"
    );

    let optimized = recommendation_for(&body, ALREADY_RIGHT_SIZED);
    assert_eq!(
        optimized["finding"],
        json!("Optimized"),
        "case {ALREADY_RIGHT_SIZED:?}: a function whose peak plus headroom fits its memory \
         is optimized, got {optimized}"
    );

    // Tags are the function's own: reported where it carries any, absent where it does not.
    assert_eq!(
        over["tags"],
        json!([{"key": "Owner", "value": "TeamA"}]),
        "case {OVER_PROVISIONED:?}: a recommendation reports the function's tags, got {over}"
    );
    assert!(
        optimized["tags"].is_null(),
        "case {ALREADY_RIGHT_SIZED:?}: a function carrying no tag reports none, got {optimized}"
    );
}

/// "Functions with a finding of unavailable are not returned unless you specify the filter
/// parameter with a value of `Unavailable`": the two functions the analysis could reach no
/// finding for are withheld from every other request, and each carries the reason code of
/// what the analysis was missing.
#[tokio::test]
async fn unavailable_functions_are_withheld_unless_a_filter_asks_for_them() {
    let router = emulator();
    analysed(&router).await;

    // Rows: the member absent, an empty filter list, a filter that names other findings, and
    // a reason-code filter that names an unavailable function's own reason code — none of
    // them is "the filter parameter with a value of Unavailable".
    let withholding: [(&str, Value); 4] = [
        ("absent", json!({})),
        ("empty", json!({"filters": []})),
        (
            "another finding",
            json!({"filters": [{"name": "Finding", "values": ["Optimized", "NotOptimized"]}]}),
        ),
        (
            "an unavailable reason code alone",
            json!({"filters": [{"name": "FindingReasonCode", "values": ["InsufficientData"]}]}),
        ),
    ];
    for (case, request) in withholding {
        let body = recommendations(&router, request.clone()).await;
        for recommendation in reported(&body) {
            assert_ne!(
                recommendation["finding"],
                json!("Unavailable"),
                "case {case:?}: an unavailable function is returned only when a Finding filter \
                 names Unavailable, got {recommendation}"
            );
        }
    }

    let body = recommendations(
        &router,
        json!({"filters": [{"name": "Finding", "values": ["Unavailable"]}]}),
    )
    .await;
    let asked = reported(&body);
    assert_eq!(
        asked.len(),
        2,
        "asking for Unavailable must report exactly the functions the analysis reached no \
         finding for, got {body}"
    );
    for recommendation in &asked {
        assert_recommendation_invariants(recommendation, 14.0);
        assert_eq!(
            recommendation["finding"],
            json!("Unavailable"),
            "a Finding filter reports only the functions at that finding, got {recommendation}"
        );
    }
    // Each unavailable function says WHAT the analysis was missing: too few invocations is
    // insufficient data, no memory utilization at all is inconclusive.
    assert_eq!(
        reason_codes(&recommendation_for(&body, TOO_FEW_INVOCATIONS)),
        vec!["InsufficientData".to_string()],
        "case {TOO_FEW_INVOCATIONS:?}: a function invoked too rarely has insufficient data, \
         got {body}"
    );
    assert_eq!(
        reason_codes(&recommendation_for(&body, NO_MEMORY_OBSERVED)),
        vec!["Inconclusive".to_string()],
        "case {NO_MEMORY_OBSERVED:?}: a function whose memory was never observed is \
         inconclusive, got {body}"
    );
}

/// A filter selects on what the service DERIVED, over the whole documented vocabulary of
/// both filter names: the functions a filter reports are exactly the ones whose reported
/// finding (or reason code) is among its values, and several filters are ANDed.
#[tokio::test]
async fn filters_select_on_the_derived_finding_and_reason_code() {
    let router = emulator();
    analysed(&router).await;

    // The whole population, unavailable functions included, so each row below can be checked
    // against the set it should pick out of it.
    let population = reported(
        &recommendations(
            &router,
            json!({"filters": [{"name": "Finding", "values": FINDINGS}]}),
        )
        .await,
    );
    assert_eq!(
        population.len(),
        5,
        "a Finding filter naming every finding reports the whole inventory, got {population:?}"
    );

    // Rows: each `Finding` value alone, each `FindingReasonCode` value alone, a filter naming
    // several values, and a filter whose values select nothing.
    let mut rows: Vec<(String, Value, Vec<String>)> = Vec::new();
    for finding in FINDINGS {
        rows.push((
            format!("Finding={finding}"),
            json!({"name": "Finding", "values": [finding]}),
            vec![(*finding).to_string()],
        ));
    }
    rows.push((
        "Finding=[Optimized, Unavailable]".to_string(),
        json!({"name": "Finding", "values": ["Optimized", "Unavailable"]}),
        vec!["Optimized".to_string(), "Unavailable".to_string()],
    ));
    for (case, filter, wanted) in rows {
        let body = recommendations(&router, json!({"filters": [filter]})).await;
        let expected: Vec<String> = population
            .iter()
            .filter(|recommendation| {
                wanted.contains(&recommendation["finding"].as_str().unwrap_or_default().to_string())
            })
            .map(|recommendation| {
                recommendation["functionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported_arns(&body),
            expected,
            "case {case:?}: a filter reports exactly the functions at those findings"
        );
    }

    // The reason codes: a filter on a code reports the functions that report it — and an
    // `Unavailable` code still only reaches them when the same request asks for Unavailable
    // findings as well.
    for code in FINDING_REASON_CODES {
        let case = format!("FindingReasonCode={code}");
        let body = recommendations(
            &router,
            json!({"filters": [
                {"name": "FindingReasonCode", "values": [code]},
                {"name": "Finding", "values": FINDINGS},
            ]}),
        )
        .await;
        let expected: Vec<String> = population
            .iter()
            .filter(|recommendation| reason_codes(recommendation).iter().any(|has| has == code))
            .map(|recommendation| {
                recommendation["functionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported_arns(&body),
            expected,
            "case {case:?}: a reason-code filter reports exactly the functions reporting it"
        );
        assert!(
            !expected.is_empty(),
            "case {case:?}: this inventory spans every documented reason code, so each must \
             select something"
        );
    }

    // ANDed: a finding and a reason code that never occur together select nothing.
    let body = recommendations(
        &router,
        json!({"filters": [
            {"name": "Finding", "values": ["Optimized"]},
            {"name": "FindingReasonCode", "values": ["MemoryOverprovisioned"]},
        ]}),
    )
    .await;
    assert_eq!(
        reported_arns(&body),
        Vec::<String>::new(),
        "an optimized function carries no reason code, so the two filters together select \
         nothing, got {body}"
    );
}

/// `functionArns` narrows to the function VERSIONS it names: an unqualified ARN is the
/// `$LATEST` version, a qualified one is the version its suffix carries, and an ARN this
/// regional endpoint observes no function for selects nothing.
#[tokio::test]
async fn function_arns_select_the_version_they_name() {
    let router = emulator();
    analysed(&router).await;

    let unqualified_latest = function_arn(OVER_PROVISIONED);
    let published = qualified_arn(UNDER_PROVISIONED_VERSION, "3");

    // Rows: one unqualified ARN, one qualified ARN, both together, the qualified form of a
    // function that has only a `$LATEST` version, the unqualified form of one that has only
    // a published version, a function this service does not observe, and an ARN of another
    // Region.
    let cases: [(&str, Vec<String>, Vec<String>); 7] = [
        (
            "an unqualified ARN selects $LATEST",
            vec![unqualified_latest.clone()],
            vec![unqualified_latest.clone()],
        ),
        (
            "a qualified ARN selects that version",
            vec![published.clone()],
            vec![published.clone()],
        ),
        (
            "both",
            vec![unqualified_latest.clone(), published.clone()],
            vec![unqualified_latest.clone(), published.clone()],
        ),
        (
            "a version of a function that has none",
            vec![qualified_arn(OVER_PROVISIONED, "7")],
            Vec::new(),
        ),
        (
            "the $LATEST version of a function that has only a published one",
            vec![function_arn(UNDER_PROVISIONED_VERSION)],
            Vec::new(),
        ),
        (
            "a function this service does not observe",
            vec![function_arn("no-such-function")],
            Vec::new(),
        ),
        (
            "a function of another Region",
            vec![format!(
                "arn:aws:lambda:eu-west-1:{}:function:{OVER_PROVISIONED}",
                ::engine::emulator_account_id()
            )],
            Vec::new(),
        ),
    ];
    for (case, arns, expected) in cases {
        let body = recommendations(&router, json!({"functionArns": arns})).await;
        assert_eq!(
            reported_arns(&body),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {body}"
        );
    }
}

/// A regional endpoint answers for ITS Region: the ARNs it reports name the Region the
/// request was signed for, and the recommendations are otherwise identical.
#[tokio::test]
async fn the_arns_reported_name_the_region_the_request_addressed() {
    let router = emulator();
    analysed(&router).await;

    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let (status, body) = call_in_region(
            &router,
            region,
            "GetLambdaFunctionRecommendations",
            json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status}: {body}"
        );
        let arns = reported_arns(&body);
        assert!(
            !arns.is_empty(),
            "case {region:?}: every Region's endpoint reports this account's functions, got \
             {body}"
        );
        for arn in arns {
            assert!(
                arn.starts_with(&format!("arn:aws:lambda:{region}:")),
                "case {region:?}: a reported ARN names the Region of the endpoint addressed, \
                 got {arn}"
            );
        }
    }
}

/// The period analysed is the service's own 14 days for every function — Compute Optimizer
/// does not let a `lookBackPeriod` preference be set for the `LambdaFunction` resource type
/// — a function observed for less than that is analysed over the days it has, and only the
/// samples INSIDE the period count towards what a function is recommended.
#[tokio::test]
async fn the_period_analysed_is_the_service_default_and_only_its_samples_count() {
    let router = emulator();
    analysed(&router).await;

    let body = recommendations(&router, json!({})).await;
    let over = recommendation_for(&body, OVER_PROVISIONED);
    assert_eq!(
        number(&over, "lookbackPeriodInDays"),
        14.0,
        "a function observed for longer than the period is analysed over the service's \
         default 14 days, got {over}"
    );
    // A function observed for only a week is analysed over that week.
    let short = recommendation_for(&body, ALREADY_RIGHT_SIZED);
    assert_eq!(
        number(&short, "lookbackPeriodInDays"),
        7.0,
        "a function observed for 7 days is analysed over 7 days, got {short}"
    );
    // The heavier day this function saw 40 days back is OUTSIDE the analysed period, so
    // neither the reported peak nor the memory size asked for carries it.
    assert_eq!(
        metric(&over, "Memory", "Maximum"),
        420.0,
        "the analysed peak is the peak of the samples inside the period, got {over}"
    );
    assert_eq!(
        number(&only_option(&over), "memorySize"),
        504.0,
        "the memory size asked for carries the analysed peak with headroom, got {over}"
    );

    // Rows: each `LookBackPeriodPreference` value the model declares. None of them can be
    // set for a Lambda function — "you can only set this preference for the Amazon EC2
    // instance, Auto Scaling group, Amazon EBS volume, Amazon ECS service and Amazon
    // Aurora and RDS database resource types" — so the period in force cannot be moved and
    // the recommendations do not change.
    for preference in ["DAYS_14", "DAYS_32", "DAYS_93"] {
        let (status, refusal) = call(
            &router,
            "PutRecommendationPreferences",
            json!({"resourceType": "LambdaFunction", "lookBackPeriod": preference}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {preference:?}: a look-back preference is not settable for a Lambda \
             function, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "InvalidParameterValueException",
            "case {preference:?}: expected InvalidParameterValueException, got {refusal}"
        );
        assert_eq!(
            recommendations(&router, json!({})).await,
            body,
            "case {preference:?}: a refused preference changes no recommendation"
        );
    }
}

/// Paging never changes WHAT is reported: walking the recommendations `maxResults` at a time
/// yields exactly the unpaged list, in the same order, with a `nextToken` on every page but
/// the last. A token this service did not issue is refused.
#[tokio::test]
async fn paging_walks_the_same_recommendations_in_the_same_order() {
    let router = emulator();
    analysed(&router).await;

    let unpaged = reported_arns(&recommendations(&router, json!({})).await);
    assert!(
        unpaged.len() > 2,
        "this test needs several recommendations to page over, got {unpaged:?}"
    );

    // Rows: the smallest page, a page that divides the set unevenly, one that holds it all,
    // the modelled upper bound of `maxResults`, its lower bound `0` (no size asked for) and
    // the member left out entirely.
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
        let mut walked: Vec<String> = Vec::new();
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
            let body = recommendations(&router, request).await;
            pages += 1;
            assert!(
                pages <= unpaged.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            let page = reported_arns(&body);
            if let Some(size) = page_size.filter(|size| *size > 0) {
                assert!(
                    page.len() as i64 <= size,
                    "case {case:?}: a page holds at most {size} recommendations, got {}",
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
            "case {case:?}: paging must walk exactly the unpaged recommendations, in order"
        );
        let expected_pages = match page_size.filter(|size| *size > 0) {
            Some(size) => unpaged.len().div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {} recommendations, got \
             {pages}",
            unpaged.len()
        );
    }
}

/// What this operation refuses, and with which modelled error: a request whose value is out
/// of range or not valid is a 400 `InvalidParameterValueException`, and one the model itself
/// rejects never reaches the handler.
#[tokio::test]
async fn a_request_that_is_not_valid_is_refused_with_the_modelled_error() {
    let router = emulator();
    analysed(&router).await;

    let own = ::engine::emulator_account_id().to_string();
    let cases: [(&str, Value, &str); 10] = [
        (
            "more than one account id",
            json!({"accountIds": [own.clone(), "111122223333"]}),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not 12 digits",
            json!({"accountIds": ["12345"]}),
            "InvalidParameterValueException",
        ),
        (
            "another account's functions",
            json!({"accountIds": ["111122223333"]}),
            "InvalidParameterValueException",
        ),
        (
            "a function ARN of another account",
            json!({"functionArns": ["arn:aws:lambda:us-east-1:111122223333:function:f"]}),
            "InvalidParameterValueException",
        ),
        (
            "a function id that is not an ARN",
            json!({"functionArns": [OVER_PROVISIONED]}),
            "InvalidParameterValueException",
        ),
        (
            "an ARN of another service",
            json!({"functionArns": [format!("arn:aws:ec2:us-east-1:{own}:instance/i-0123456789abcdef0")]}),
            "InvalidParameterValueException",
        ),
        (
            "an ARN with no function name",
            json!({"functionArns": [format!("arn:aws:lambda:us-east-1:{own}:function:")]}),
            "InvalidParameterValueException",
        ),
        (
            "a finding value outside the LambdaFunctionRecommendationFinding value set",
            json!({"filters": [{"name": "Finding", "values": ["Oversized"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a filter carrying no name",
            json!({"filters": [{"values": ["Optimized"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a forged nextToken",
            json!({"nextToken": "not-a-token"}),
            "InvalidParameterValueException",
        ),
    ];
    for (case, request, expected) in cases {
        let (status, body) = call(&router, "GetLambdaFunctionRecommendations", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // Rows the MODEL refuses before the handler runs: a filter name outside
    // `LambdaFunctionRecommendationFilterName`, and a `maxResults` outside 0..1000.
    for (case, request) in [
        (
            "a filter name the model does not declare",
            json!({"filters": [{"name": "tag:Owner", "values": ["TeamA"]}]}),
        ),
        ("a maxResults above the modelled range", json!({"maxResults": 1001})),
    ] {
        let (status, body) = call(&router, "GetLambdaFunctionRecommendations", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected the model's own refusal, got {body}"
        );
    }
}

/// The enrollment gate, and that reading is a READ: nothing is reported before the account
/// opts in, nothing until the analysis its enrollment starts is due, two identical calls
/// answer identically, and reading moves neither the enrollment record nor the preferences.
#[tokio::test]
async fn recommendations_need_an_enrolled_account_and_reading_changes_nothing() {
    let router = emulator();

    let (status, body) = call(&router, "GetLambdaFunctionRecommendations", json!({})).await;
    assert_eq!(
        status, 403,
        "before opting in: expected 403, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "before opting in: expected OptInRequiredException, got {body}"
    );

    // Opted in, but the analysis an enrollment starts is not due yet: the account reads the
    // empty set rather than an analysis nothing has produced.
    opt_in(&router).await;
    let body = recommendations(&router, json!({})).await;
    assert_eq!(
        reported_arns(&body),
        Vec::<String>::new(),
        "straight after opting in there is no analysis to report, got {body}"
    );

    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let before = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    let first = recommendations(&router, json!({})).await;
    let second = recommendations(&router, json!({})).await;
    assert!(
        !reported_arns(&first).is_empty(),
        "once the analysis is due the account's functions are reported, got {first}"
    );
    assert_eq!(
        first, second,
        "two reads of the same observation must answer identically"
    );
    let after = call(&router, "GetEnrollmentStatus", json!({})).await.1;
    assert_eq!(
        before, after,
        "reading recommendations must not move the enrollment record"
    );
    let preferences = call(
        &router,
        "GetRecommendationPreferences",
        json!({"resourceType": "LambdaFunction"}),
    )
    .await
    .1;
    assert_eq!(
        preferences["recommendationPreferencesDetails"],
        json!([]),
        "reading recommendations must not write a preference, got {preferences}"
    );

    // Opting back out takes the recommendations away again: the gate is the stored
    // enrollment, not whether the account ever enrolled.
    let (status, body) = call(
        &router,
        "UpdateEnrollmentStatus",
        json!({"status": "Inactive"}),
    )
    .await;
    assert_eq!(status, 200, "opting out: expected 200, got {status}: {body}");
    let (status, body) = call(&router, "GetLambdaFunctionRecommendations", json!({})).await;
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

/// `accountIds` selects WHOSE functions are reported: absent or the caller's own account
/// reads the caller's, and both answer identically.
#[tokio::test]
async fn account_ids_selects_the_caller_s_own_account() {
    let router = emulator();
    analysed(&router).await;

    let own = ::engine::emulator_account_id().to_string();
    let absent = recommendations(&router, json!({})).await;
    for request in [json!({"accountIds": []}), json!({"accountIds": [own]})] {
        let body = recommendations(&router, request.clone()).await;
        assert_eq!(
            body, absent,
            "case {request}: expected the caller's own recommendations"
        );
    }
}

/// The per-function reader and `GetRecommendationSummaries` answer from ONE analysis: the
/// summary of the account's Lambda functions counts exactly the functions this reader
/// reports at each finding.
#[tokio::test]
async fn the_summary_of_lambda_functions_counts_what_this_reader_reports() {
    let router = emulator();
    analysed(&router).await;

    let reported_functions = reported(&recommendations(&router, json!({})).await);
    let (status, summaries) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 200,
        "reading the summaries: expected 200, got {status}: {summaries}"
    );
    let lambda = summaries["recommendationSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["recommendationResourceType"] == json!("LambdaFunction"))
        .unwrap_or_else(|| panic!("no LambdaFunction summary: {summaries}"));

    // The findings a summary of Lambda functions reports: "the number of Lambda functions in
    // an account that are NotOptimized, or Optimized".
    for finding in ["NotOptimized", "Optimized"] {
        let counted = lambda["summaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry["name"] == json!(finding))
            .map(|entry| entry["value"].as_f64().unwrap_or(0.0))
            .unwrap_or_else(|| panic!("case {finding:?}: no count in {lambda}"));
        let reported_at = reported_functions
            .iter()
            .filter(|recommendation| recommendation["finding"] == json!(finding))
            .count() as f64;
        assert_eq!(
            counted, reported_at,
            "case {finding:?}: the summary counts the {reported_at} function(s) this reader \
             reports at that finding, got {counted}"
        );
    }
}
