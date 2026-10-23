// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Network Access Scope analysis slice —
//! `StartNetworkInsightsAccessScopeAnalysis` and
//! `DescribeNetworkInsightsAccessScopeAnalyses` — driven over the wire the service actually
//! speaks: `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a start STORES ONE ANALYSIS OF THE SCOPE THE REQUEST NAMES and the describe REPORTS
//!    THAT STORE: the scope id and the tags the request sets round-trip, and the members the
//!    service owns (the analysis id, its ARN, its status and its start date) are derived
//!    rather than taken;
//! 2. the CLOCK, not a caller, ends an analysis — `running` -> `failed`, carrying the reason
//!    this emulator produced no findings and the end date of the due instant, and never
//!    findings or an analyzed-interface count that nothing computed;
//! 3. one `ClientToken` names one analysis, and reusing it for a different request is
//!    refused with `IdempotentParameterMismatch`;
//! 4. `NetworkInsightsAccessScopeAnalysisIds`, `NetworkInsightsAccessScopeId`, the
//!    `AnalysisStartTimeBegin`/`AnalysisStartTimeEnd` window, the two filters the service
//!    enumerates and `MaxResults` narrow and page the report in analysis-id order, and an id
//!    or a scope the account does not hold, a filter name outside that catalog, a page size
//!    outside the published range or a token this service did not issue is refused with
//!    EC2's own error code;
//! 5. a `DryRun` of either operation reports `DryRunOperation` and stores nothing;
//! 6. the analysis store's count ceiling is STRUCTURAL — the writer itself refuses past it,
//!    so no caller can grow the store without bound, and the start answers EC2's quota code.
//!
//! Each table row is a distinct input along what the rule generalises over — how many tags
//! and how long their values are, how many analyses one scope carries, which client token,
//! which bound of the start-time window and in which direction, which filter name, which
//! page size at each published bound and one past it — so no row is the specification.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use ec2::ops::types::{ANALYSIS_STATUS_VALUES, FINDINGS_FOUND_VALUES};
use ec2::state::{
    ANALYSIS_STATUS_FAILED, ANALYSIS_STATUS_RUNNING, ANALYSIS_UNSUPPORTED_STATUS_MESSAGE,
    ComAmazonawsEc2Amazonec2State as Ec2State, FINDINGS_FOUND_UNKNOWN,
    IDEMPOTENT_PARAMETER_MISMATCH, INVALID_FILTER, MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES,
    NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX, NetworkInsightsAccessScopeAnalysisSpec,
    RESOURCE_COUNT_EXCEEDED, RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed, unescaped};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for. An analysis's ARN names the Region the request
/// was signed for, so it is read back out of that ARN below.
const REGION: &str = "us-west-2";

/// The filter names this operation supports, in the order the service enumerates them.
///
/// The model says "There are no supported filters"
/// (`DescribeNetworkInsightsAccessScopeAnalysesRequest$Filters`); the service's own
/// `InvalidFilter` refusal says "Filter name must be one of [status, findings-found]", and
/// that enumeration is the source of this catalog and of its order. The suite exercises both
/// entries, including `findings-found`, which the issue this slice was filed under never
/// mentioned.
const SUPPORTED_FILTERS: [&str; 2] = ["status", "findings-found"];

/// A well-formed Network Access Scope id the account holds no scope under.
const UNKNOWN_SCOPE_ID: &str = "nis-0ffffffffffffffff";

/// A well-formed analysis id the account holds no analysis under.
const UNKNOWN_ANALYSIS_ID: &str = "nisa-0ffffffffffffffff";

/// How long a clock-owed transition takes to fall due: the service's own span, plus a margin
/// so a slow machine cannot read the state before it is due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `StartNetworkInsightsAccessScopeAnalysis` call.
async fn start(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "StartNetworkInsightsAccessScopeAnalysis", params).await
}

/// One `DescribeNetworkInsightsAccessScopeAnalyses` call.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "DescribeNetworkInsightsAccessScopeAnalyses", params).await
}

/// A Network Access Scope of the account, as its id.
///
/// `CreateNetworkInsightsAccessScope` is still a scaffold stub (its own issue,
/// `ec2-createnetworkinsightsaccessscope-stub`), so the scope a start resolves is written
/// through the store's own one insertion path — the same path that create will use — rather
/// than through a second hand-built copy of the modelled shape.
fn seed_scope(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_network_insights_access_scope(REGION, Vec::new())
        .expect("the store holds room for the suite's scopes")
}

/// How many analyses the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_access_scope_analyses
        .len()
}

/// One analysis as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Analysis {
    analysis_id: String,
    analysis_arn: String,
    scope_id: String,
    status: String,
    status_message: Option<String>,
    start_date: String,
    end_date: Option<String>,
    findings_found: Option<String>,
    analyzed_eni_count: Option<String>,
    tags: Vec<(String, String)>,
}

/// Every analysis a response carries, in document order: the elements that have a
/// `networkInsightsAccessScopeAnalysisId` child, which is both
/// `<networkInsightsAccessScopeAnalysis>` in a start's answer and each `<item>` of the
/// describe's `<networkInsightsAccessScopeAnalysisSet>`.
fn reported(case: &str, body: &str) -> Vec<Analysis> {
    let doc = document(case, body);
    doc.descendants()
        .filter(|node| {
            node.is_element() && child(*node, "networkInsightsAccessScopeAnalysisId").is_some()
        })
        .map(|node| Analysis {
            analysis_id: child_text(node, "networkInsightsAccessScopeAnalysisId")
                .unwrap_or_default(),
            analysis_arn: child_text(node, "networkInsightsAccessScopeAnalysisArn")
                .unwrap_or_default(),
            scope_id: child_text(node, "networkInsightsAccessScopeId").unwrap_or_default(),
            status: child_text(node, "status").unwrap_or_default(),
            status_message: child_text(node, "statusMessage"),
            start_date: child_text(node, "startDate").unwrap_or_default(),
            end_date: child_text(node, "endDate"),
            findings_found: child_text(node, "findingsFound"),
            analyzed_eni_count: child_text(node, "analyzedEniCount"),
            tags: child(node, "tagSet")
                .map(|set| {
                    set.children()
                        .filter(|item| item.is_element() && item.tag_name().name() == "item")
                        .map(|item| {
                            (
                                child_text(item, "key").unwrap_or_default(),
                                child_text(item, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The one analysis a response carries, or a panic naming the case.
fn only(case: &str, body: &str) -> Analysis {
    let found = reported(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one analysis, got {found:?} from {body}"
    );
    found.into_iter().next().expect("one analysis")
}

/// Every analysis id a response reports, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|analysis| analysis.analysis_id)
        .collect()
}

/// The form fields for one `TagSpecification` of `tags` on the analysis being started.
fn tag_specification(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// A start call for `scope_id` under `client_token`, with `tags`.
fn start_params(scope_id: &str, client_token: &str, tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![
        param("NetworkInsightsAccessScopeId", scope_id),
        param("ClientToken", client_token),
    ];
    params.extend(tag_specification(tags));
    params
}

/// One analysis started for `scope_id`, as the start reported it.
async fn start_analysis(
    case: &str,
    router: &MultiServiceRouter,
    scope_id: &str,
    client_token: &str,
    tags: &[(&str, &str)],
) -> Analysis {
    let body = ok(
        case,
        router,
        "StartNetworkInsightsAccessScopeAnalysis",
        &start_params(scope_id, client_token, tags),
    )
    .await;
    only(case, &body)
}

/// The whole second an analysis started in, as epoch seconds: `startDate` read back off the
/// wire through the one parser the model's `date-time` form is written with.
fn start_second(case: &str, analysis: &Analysis) -> i64 {
    DateTime::from_str(&analysis.start_date, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: startDate {:?} must be a modelled timestamp ({error})",
                analysis.start_date
            )
        })
        .secs()
}

/// An epoch second as an ec2Query form value, in the `date-time` form the model's
/// `MillisecondDateTime` takes.
fn timestamp(at_epoch_seconds: i64) -> String {
    DateTime::from_secs(at_epoch_seconds)
        .fmt(Format::DateTime)
        .expect("a renderable timestamp")
}

// ---------------------------------------------------------------------------
// RULE 1: a start stores the analysis, and the describe reports the store
// ---------------------------------------------------------------------------

/// RULE 1: the analysis a start answers is the one the describe reports, the scope and the
/// tags the request set round-trip, and the members the service owns are derived.
///
/// The rows differ in how many tags the request assigns and how long their values are, so no
/// one shape of request is the specification.
#[tokio::test]
async fn a_start_stores_the_analysis_and_the_describe_reports_it() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, the tags the start assigns)
    let cases: [(&str, &[(&str, &str)]); 4] = [
        ("untagged", &[]),
        ("one tag", &[("Name", "nightly-scope-check")]),
        (
            "three tags",
            &[("Name", "audit"), ("Team", "network"), ("Tier", "prod")],
        ),
        ("a tag whose value is empty", &[("Owner", "")]),
    ];

    for (index, (case, tags)) in cases.into_iter().enumerate() {
        // A scope per row, so no row's analysis belongs to another row's scope.
        let scope_id = seed_scope(&engine);
        let client_token = format!("token-{index}");
        let started = start_analysis(case, &router, &scope_id, &client_token, tags).await;

        assert!(
            is_long_id(NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX, &started.analysis_id),
            "case {case:?}: expected an analysis id the service minted, got {:?}",
            started.analysis_id
        );
        let expected_arn = format!(
            "arn:aws:ec2:{REGION}:{account}:{RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS}/{id}",
            account = engine::emulator_account_id(),
            id = started.analysis_id,
        );
        assert_eq!(
            started.analysis_arn, expected_arn,
            "case {case:?}: expected the analysis's own ARN {expected_arn:?}, got {:?}",
            started.analysis_arn
        );
        assert_eq!(
            started.scope_id, scope_id,
            "case {case:?}: expected the analysis of scope {scope_id:?}, got {:?}",
            started.scope_id
        );
        assert_eq!(
            started.status, ANALYSIS_STATUS_RUNNING,
            "case {case:?}: an analysis AWS hands back is still running, got {:?}",
            started.status
        );
        assert!(
            ANALYSIS_STATUS_VALUES.contains(&started.status.as_str()),
            "case {case:?}: expected a member of the modelled AnalysisStatus catalog \
             {ANALYSIS_STATUS_VALUES:?}, got {:?}",
            started.status
        );
        assert!(
            !started.start_date.is_empty(),
            "case {case:?}: expected a start date, got {started:?}"
        );
        assert_eq!(
            (
                started.end_date.as_deref(),
                started.findings_found.as_deref(),
                started.analyzed_eni_count.as_deref(),
            ),
            (None, None, None),
            "case {case:?}: a running analysis has not ended, found nothing and analyzed \
             nothing yet, got {started:?}"
        );
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            started.tags, expected_tags,
            "case {case:?}: expected the requested tags {expected_tags:?}, got {:?}",
            started.tags
        );

        // The describe reports the same record, member for member.
        let body = ok(
            case,
            &router,
            "DescribeNetworkInsightsAccessScopeAnalyses",
            &indexed(
                "NetworkInsightsAccessScopeAnalysisId",
                &[started.analysis_id.as_str()],
            ),
        )
        .await;
        let described = only(case, &body);
        assert_eq!(
            described, started,
            "case {case:?}: the describe must report the analysis the start stored, got \
             {described:?} for {started:?}"
        );
        assert_eq!(
            stored_count(&engine),
            index + 1,
            "case {case:?}: expected one stored analysis per row so far"
        );
    }
}

/// RULE 1: one scope carries every analysis started against it, each with its own id, and the
/// describe narrowed to that scope reports all of them in id order.
///
/// The rows differ in how many analyses the scope carries.
#[tokio::test]
async fn a_scope_carries_every_analysis_started_against_it() {
    let (router, engine) = super::ec2_query::emulator();

    for count in [1usize, 2, 5] {
        let case = format!("{count} analyses of one scope");
        let scope_id = seed_scope(&engine);
        let mut started: Vec<String> = Vec::new();
        for index in 0..count {
            let analysis = start_analysis(
                &case,
                &router,
                &scope_id,
                &format!("{scope_id}-{index}"),
                &[],
            )
            .await;
            started.push(analysis.analysis_id);
        }
        started.sort();
        assert_eq!(
            started.len(),
            count,
            "case {case:?}: every start mints its own analysis, got {started:?}"
        );

        let body = ok(
            &case,
            &router,
            "DescribeNetworkInsightsAccessScopeAnalyses",
            &[param("NetworkInsightsAccessScopeId", &scope_id)],
        )
        .await;
        let described = reported_ids(&case, &body);
        assert_eq!(
            described, started,
            "case {case:?}: expected every analysis of {scope_id:?} in id order, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the clock ends the analysis
// ---------------------------------------------------------------------------

/// RULE 2: an analysis is `running` when the start answers — the span is never zero, so a
/// caller reading straight after the call still sees it — and the CLOCK moves it to the
/// terminal `failed` status, carrying why no findings were produced and the end date of the
/// due instant. Nothing reports findings or an analyzed-interface count that nothing
/// computed.
#[tokio::test]
async fn the_clock_ends_the_analysis_with_the_reason_nothing_analyzed_it() {
    let case = "the clock ends the analysis";
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let started = start_analysis(case, &router, &scope_id, "clock-token", &[]).await;

    // Straight after the start, before the span has passed: still running.
    let ids = indexed(
        "NetworkInsightsAccessScopeAnalysisId",
        &[started.analysis_id.as_str()],
    );
    let body = ok(
        case,
        &router,
        "DescribeNetworkInsightsAccessScopeAnalyses",
        &ids,
    )
    .await;
    let polled = only(case, &body);
    assert_eq!(
        polled.status, ANALYSIS_STATUS_RUNNING,
        "case {case:?}: a caller reading straight after the start still sees the analysis \
         running, got {polled:?}"
    );

    past_the_transition().await;

    let body = ok(
        case,
        &router,
        "DescribeNetworkInsightsAccessScopeAnalyses",
        &ids,
    )
    .await;
    let ended = only(case, &body);
    assert_eq!(
        ended.status, ANALYSIS_STATUS_FAILED,
        "case {case:?}: expected the terminal status the clock owes the analysis, got {ended:?}"
    );
    assert_eq!(
        ended.status_message.as_deref(),
        Some(ANALYSIS_UNSUPPORTED_STATUS_MESSAGE),
        "case {case:?}: expected the reason no findings were produced, got {ended:?}"
    );
    assert_eq!(
        ended.findings_found.as_deref(),
        Some(FINDINGS_FOUND_UNKNOWN),
        "case {case:?}: whether the scope has findings was never determined, got {ended:?}"
    );
    assert!(
        FINDINGS_FOUND_VALUES.contains(&ended.findings_found.clone().unwrap_or_default().as_str()),
        "case {case:?}: expected a member of the modelled FindingsFound catalog \
         {FINDINGS_FOUND_VALUES:?}, got {ended:?}"
    );
    assert_eq!(
        ended.analyzed_eni_count, None,
        "case {case:?}: no network interface was analyzed, so none is counted, got {ended:?}"
    );
    let end_date = ended
        .end_date
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: an ended analysis reports an end date: {ended:?}"));
    let ended_at = DateTime::from_str(&end_date, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {end_date:?} must be a timestamp ({error})"));
    let started_at = DateTime::from_str(&ended.start_date, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: a start date ({error})"));
    assert!(
        ended_at.as_secs_f64() >= started_at.as_secs_f64(),
        "case {case:?}: the analysis cannot have ended before it started, got {ended:?}"
    );
    // The id, the ARN, the scope and the tags are the start's, unchanged by the move.
    assert_eq!(
        (
            ended.analysis_id.as_str(),
            ended.analysis_arn.as_str(),
            ended.scope_id.as_str()
        ),
        (
            started.analysis_id.as_str(),
            started.analysis_arn.as_str(),
            started.scope_id.as_str()
        ),
        "case {case:?}: the move changes the status, not the identity, got {ended:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: one ClientToken names one analysis
// ---------------------------------------------------------------------------

/// RULE 3: repeating a start with the same `ClientToken` answers the analysis that token
/// already started and stores no second one; a different token on the same scope starts
/// another analysis, and the same token on a DIFFERENT request is refused.
///
/// The rows differ in the token's own form — short, long, punctuated, non-ASCII — none of
/// which the service reads as anything but a name, and then in which other scope the reused
/// token names.
#[tokio::test]
async fn one_client_token_names_one_analysis() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);

    let tokens = [
        "a",
        "0123456789abcdef0123456789abcdef",
        "token.with-punctuation_and~tilde",
        "tōken-with-non-ascii",
    ];
    for (index, token) in tokens.into_iter().enumerate() {
        let case = format!("ClientToken={token:?}");
        let first = start_analysis(&case, &router, &scope_id, token, &[]).await;
        let replayed = start_analysis(&case, &router, &scope_id, token, &[]).await;
        assert_eq!(
            replayed.analysis_id, first.analysis_id,
            "case {case:?}: a replay answers the analysis the token started, got {replayed:?}"
        );
        assert_eq!(
            stored_count(&engine),
            index + 1,
            "case {case:?}: a replay stores no second analysis"
        );
    }

    // A token names ONE request: reusing it for a different scope is refused rather than
    // answered with the analysis it already started. The rows differ in which scope the
    // second request names — another scope of the account, and one the account does not
    // hold at all.
    let other_scope = seed_scope(&engine);
    let held = stored_count(&engine);
    for (case, scope) in [
        ("another scope of the account", other_scope.as_str()),
        ("a scope the account does not hold", UNKNOWN_SCOPE_ID),
    ] {
        let (status, body) = start(&router, &start_params(scope, "a", &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected {IDEMPOTENT_PARAMETER_MISMATCH}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            held,
            "case {case:?}: a refused start stores nothing"
        );
    }

    // The same token with the same scope is still the replay it always was.
    let again = start_analysis("the token replayed again", &router, &scope_id, "a", &[]).await;
    assert_eq!(
        stored_count(&engine),
        held,
        "the token replayed again: still no second analysis, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: narrowing, paging and refusals
// ---------------------------------------------------------------------------

/// RULE 4: the start-time window selects on the analysis's own start date, with both bounds
/// INCLUSIVE.
///
/// The rows differ in which bound the request carries, in which direction it sits from the
/// start, and in whether both bounds are given — including a window that has already closed
/// and one that opens later.
#[tokio::test]
async fn the_start_time_window_selects_on_the_analysis_start_date() {
    let case = "the start-time window";
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let started = start_analysis(case, &router, &scope_id, "window-token", &[]).await;
    let at = start_second(case, &started);

    // (case, AnalysisStartTimeBegin, AnalysisStartTimeEnd, is the analysis reported?)
    let cases: [(&str, Option<i64>, Option<i64>, bool); 7] = [
        ("begin at the second it started in", Some(at), None, true),
        ("begin a minute later", Some(at + 60), None, false),
        ("end a second later", None, Some(at + 1), true),
        ("end a minute earlier", None, Some(at - 60), false),
        ("a window around it", Some(at - 60), Some(at + 60), true),
        ("a window after it", Some(at + 60), Some(at + 120), false),
        ("an inverted window", Some(at + 60), Some(at - 60), false),
    ];

    for (row, begin, end, reported_in_window) in cases {
        let mut params = Vec::new();
        if let Some(begin) = begin {
            params.push(param("AnalysisStartTimeBegin", &timestamp(begin)));
        }
        if let Some(end) = end {
            params.push(param("AnalysisStartTimeEnd", &timestamp(end)));
        }
        let body = ok(
            row,
            &router,
            "DescribeNetworkInsightsAccessScopeAnalyses",
            &params,
        )
        .await;
        let ids = reported_ids(row, &body);
        let expected: Vec<String> = if reported_in_window {
            vec![started.analysis_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids, expected,
            "case {row:?}: expected {expected:?} for the window begin={begin:?} end={end:?}, \
             got {ids:?}"
        );
    }
}

/// RULE 4: an id or a scope the account does not hold is refused with EC2's own
/// `Invalid<Resource>Id.NotFound` code rather than reported as an absent row, by both
/// operations.
#[tokio::test]
async fn an_id_the_account_does_not_hold_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let held = start_analysis("held", &router, &scope_id, "held-token", &[]).await;

    // (case, the describe's parameters, the expected code)
    let cases: [(&str, Vec<Param>, &str); 4] = [
        (
            "an unknown analysis id",
            indexed(
                "NetworkInsightsAccessScopeAnalysisId",
                &[UNKNOWN_ANALYSIS_ID],
            ),
            "InvalidNetworkInsightsAccessScopeAnalysisId.NotFound",
        ),
        (
            "a known and an unknown analysis id",
            indexed(
                "NetworkInsightsAccessScopeAnalysisId",
                &[held.analysis_id.as_str(), UNKNOWN_ANALYSIS_ID],
            ),
            "InvalidNetworkInsightsAccessScopeAnalysisId.NotFound",
        ),
        (
            "an unknown scope id",
            vec![param("NetworkInsightsAccessScopeId", UNKNOWN_SCOPE_ID)],
            "InvalidNetworkInsightsAccessScopeId.NotFound",
        ),
        (
            "a scope id that is not an id at all",
            vec![param("NetworkInsightsAccessScopeId", "not-a-scope")],
            "InvalidNetworkInsightsAccessScopeId.NotFound",
        ),
    ];
    for (case, params, expected) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // The start resolves the scope the same way.
    for scope in [UNKNOWN_SCOPE_ID, "not-a-scope"] {
        let (status, body) = start(&router, &start_params(scope, "unknown-scope", &[])).await;
        assert_eq!(
            status, 400,
            "case {scope:?}: expected HTTP 400 from the start, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNetworkInsightsAccessScopeId.NotFound",
            "case {scope:?}: expected InvalidNetworkInsightsAccessScopeId.NotFound, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {scope:?}: a refused start stores nothing"
        );
    }
}

/// RULE 4: the two filters the SERVICE enumerates select on the analysis's own members, and
/// every other filter name is refused with EC2's `InvalidFilter` and the message naming the
/// supported catalog.
///
/// The model says "There are no supported filters"
/// (`DescribeNetworkInsightsAccessScopeAnalysesRequest$Filters`), but the service answers
/// `InvalidFilter` with "Filter name must be one of [status, findings-found]" — so the
/// catalog under test is the one the service enumerates, including the `tag:` family and the
/// id names other EC2 describes do document, which are NOT in it.
#[tokio::test]
async fn the_enumerated_filters_select_and_every_other_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let running = start_analysis("running", &router, &scope_id, "filter-token", &[]).await;

    // Every supported name, on an analysis the clock has not ended yet: `status` holds
    // `running`, and `findings-found` holds nothing at all, so it selects nothing.
    //
    // (case, filter name, values, is the analysis reported?)
    let while_running: [(&str, &str, &[&str], bool); 6] = [
        ("its own status", "status", &[ANALYSIS_STATUS_RUNNING], true),
        ("another status", "status", &[ANALYSIS_STATUS_FAILED], false),
        (
            "either status (values are OR-ed)",
            "status",
            &[ANALYSIS_STATUS_FAILED, ANALYSIS_STATUS_RUNNING],
            true,
        ),
        ("a status wildcard", "status", &["run*"], true),
        (
            "findings-found, which a running analysis does not carry",
            "findings-found",
            &[FINDINGS_FOUND_UNKNOWN],
            false,
        ),
        ("a findings wildcard", "findings-found", &["*"], false),
    ];
    for (case, name, values, selects) in while_running {
        let body = ok(
            case,
            &router,
            "DescribeNetworkInsightsAccessScopeAnalyses",
            &filters(&[(name, values)]),
        )
        .await;
        let ids = reported_ids(case, &body);
        let expected: Vec<String> = if selects {
            vec![running.analysis_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?} for the filter {name}={values:?}, got {ids:?}"
        );
    }

    // Once the clock has ended it, the same two filters select on what it reports NOW.
    past_the_transition().await;
    let when_ended: [(&str, &str, &[&str], bool); 4] = [
        ("the ended status", "status", &[ANALYSIS_STATUS_FAILED], true),
        ("the status it left", "status", &[ANALYSIS_STATUS_RUNNING], false),
        (
            "the findings it reports",
            "findings-found",
            &[FINDINGS_FOUND_UNKNOWN],
            true,
        ),
        ("other findings", "findings-found", &["true"], false),
    ];
    for (case, name, values, selects) in when_ended {
        let body = ok(
            case,
            &router,
            "DescribeNetworkInsightsAccessScopeAnalyses",
            &filters(&[(name, values)]),
        )
        .await;
        let ids = reported_ids(case, &body);
        let expected: Vec<String> = if selects {
            vec![running.analysis_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?} for the filter {name}={values:?}, got {ids:?}"
        );
    }

    // Both filters at once are AND-ed: one that holds and one that does not selects nothing.
    let body = ok(
        "both filters",
        &router,
        "DescribeNetworkInsightsAccessScopeAnalyses",
        &filters(&[
            ("status", &[ANALYSIS_STATUS_FAILED]),
            ("findings-found", &["true"]),
        ]),
    )
    .await;
    assert_eq!(
        reported_ids("both filters", &body),
        Vec::<String>::new(),
        "both filters: filters are AND-ed, so one that does not hold selects nothing"
    );

    // Every name outside the enumerated catalog, including ones other EC2 describes document.
    let unsupported: [(&str, &[&str]); 6] = [
        ("network-insights-access-scope-id", &[scope_id.as_str()]),
        (
            "network-insights-access-scope-analysis-id",
            &[running.analysis_id.as_str()],
        ),
        ("tag-key", &["Name"]),
        ("tag:Name", &["audit"]),
        ("Status", &[ANALYSIS_STATUS_FAILED]),
        ("", &[]),
    ];
    let expected_message = format!(
        "Filter name must be one of [{}]",
        SUPPORTED_FILTERS.join(", ")
    );
    for (name, values) in unsupported {
        let (status, body) = describe(&router, &filters(&[(name, values)])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_FILTER,
            "case {name:?}: expected {INVALID_FILTER}, got {body}"
        );
        assert_eq!(
            unescaped(&element(&body, "Message").unwrap_or_default()),
            expected_message,
            "case {name:?}: expected the message naming the supported catalog, got {body}"
        );
    }
}

/// RULE 4: `MaxResults` bounds each page and `NextToken` resumes from where the last page
/// ended, so walking the pages reports every analysis exactly once in id order.
#[tokio::test]
async fn max_results_pages_the_report_and_the_token_resumes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let mut seeded: Vec<String> = Vec::new();
    for index in 0..7 {
        let analysis =
            start_analysis("seed", &router, &scope_id, &format!("page-{index}"), &[]).await;
        seeded.push(analysis.analysis_id);
    }
    seeded.sort();

    for page_size in [1usize, 3, 7, 100] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let body = ok(
                &case,
                &router,
                "DescribeNetworkInsightsAccessScopeAnalyses",
                &params,
            )
            .await;
            let page = reported_ids(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults analyses, got {page:?}"
            );
            walked.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: the walk must report every analysis exactly once in id order, got \
             {walked:?}"
        );
    }

    let body = ok(
        "no MaxResults",
        &router,
        "DescribeNetworkInsightsAccessScopeAnalyses",
        &[],
    )
    .await;
    assert_eq!(
        reported_ids("no MaxResults", &body),
        seeded,
        "no MaxResults: expected every analysis in one answer"
    );
    assert_eq!(stored_count(&engine), seeded.len(), "every start stored one analysis");
}

/// RULE 4: `MaxResults` outside the range the model publishes is refused at the boundary;
/// each published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    start_analysis("bounds", &router, &scope_id, "bounds-token", &[]).await;

    // The published bounds (`NetworkInsightsMaxResults`, `range 1..100`) and one past each.
    for (max_results, expected_status) in [(0, 400), (1, 200), (100, 200), (101, 400)] {
        let (status, body) = describe(&router, &[param("MaxResults", &max_results.to_string())])
            .await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// RULE 4: a `NextToken` this service did not issue is refused with `InvalidPaginationToken`
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let held = start_analysis("held", &router, &scope_id, "held-token", &[]).await;

    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        held.analysis_id.clone(),
        ::hex::encode(&held.analysis_id),
    ];
    for token in cases {
        let (status, body) = describe(&router, &[param("NextToken", &token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// RULE 4: more ids than [`ec2::state::MAX_RESOURCE_IDS`] is refused before the store is
/// read, and the cap is accepted AT its bound.
#[tokio::test]
async fn the_id_list_is_capped() {
    use ec2::state::MAX_RESOURCE_IDS;

    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let held = start_analysis("held", &router, &scope_id, "held-token", &[]).await;

    // At the bound the request is read (the held id, then the cap filled out with repeats of
    // it, so every id resolves); one past it is refused for its size alone.
    for (count, expected_status) in [(MAX_RESOURCE_IDS, 200), (MAX_RESOURCE_IDS + 1, 400)] {
        let ids: Vec<&str> = (0..count).map(|_| held.analysis_id.as_str()).collect();
        let (status, body) =
            describe(&router, &indexed("NetworkInsightsAccessScopeAnalysisId", &ids)).await;
        assert_eq!(
            status, expected_status,
            "case {count} ids: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {count} ids: expected InvalidParameterValue, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 5: a dry run reports the outcome and stores nothing
// ---------------------------------------------------------------------------

/// RULE 5: `DryRun` on either operation reports `DryRunOperation` — every caller of this
/// emulator is permitted — and leaves the store exactly as it was.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    start_analysis("held", &router, &scope_id, "held-token", &[]).await;
    let held = stored_count(&engine);

    // (case, action, parameters)
    let cases: [(&str, &str, Vec<Param>); 3] = [
        (
            "a dry-run start",
            "StartNetworkInsightsAccessScopeAnalysis",
            {
                let mut params = start_params(&scope_id, "dry-run-token", &[("Name", "audit")]);
                params.push(param("DryRun", "true"));
                params
            },
        ),
        (
            "a dry-run describe",
            "DescribeNetworkInsightsAccessScopeAnalyses",
            vec![param("DryRun", "true")],
        ),
        (
            "a dry-run describe of one scope",
            "DescribeNetworkInsightsAccessScopeAnalyses",
            vec![
                param("DryRun", "true"),
                param("NetworkInsightsAccessScopeId", &scope_id),
            ],
        ),
    ];
    for (case, action, params) in cases {
        let (status, body) = api(&router, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            held,
            "case {case:?}: a dry run stores nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 6: the store's count ceiling is structural
// ---------------------------------------------------------------------------

/// RULE 6: nothing in this emulator reclaims a stored analysis, so the store's ceiling is
/// enforced by the WRITER itself — a caller that forgets to ask
/// `has_network_insights_access_scope_analysis_capacity` cannot grow it past
/// [`MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES`] — and a start past the ceiling answers
/// EC2's quota code rather than reporting an analysis the store does not hold.
#[tokio::test]
async fn the_analysis_store_has_a_structural_count_ceiling() {
    let case = "the store's ceiling";
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);

    // Fill the store to the ceiling through the one writer, bypassing the handler's own
    // gate: the row under test is the write that does not fit.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for index in 0..MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES {
            let stored = guard.insert_network_insights_access_scope_analysis(
                NetworkInsightsAccessScopeAnalysisSpec {
                    network_insights_access_scope_id: scope_id.clone(),
                    region: REGION.to_string(),
                    tags: Vec::new(),
                    client_token: format!("ceiling-{index}"),
                },
            );
            assert!(
                stored.is_some(),
                "case {case:?}: analysis {index} is inside the ceiling and must be stored"
            );
        }
        assert!(
            guard
                .insert_network_insights_access_scope_analysis(
                    NetworkInsightsAccessScopeAnalysisSpec {
                        network_insights_access_scope_id: scope_id.clone(),
                        region: REGION.to_string(),
                        tags: Vec::new(),
                        client_token: "past-the-ceiling".to_string(),
                    }
                )
                .is_none(),
            "case {case:?}: the writer itself must refuse past \
             {MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES} analyses"
        );
    }
    assert_eq!(
        stored_count(&engine),
        MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES,
        "case {case:?}: a refused write stores nothing"
    );

    let (status, body) = start(&router, &start_params(&scope_id, "over-the-ceiling", &[])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from a start past the ceiling, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        RESOURCE_COUNT_EXCEEDED,
        "case {case:?}: expected {RESOURCE_COUNT_EXCEEDED}, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES,
        "case {case:?}: a refused start stores nothing"
    );
}
