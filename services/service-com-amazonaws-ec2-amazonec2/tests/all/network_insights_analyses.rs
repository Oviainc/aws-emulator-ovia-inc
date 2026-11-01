// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Reachability Analyzer path analysis slice —
//! `StartNetworkInsightsAnalysis` and `DescribeNetworkInsightsAnalyses` — driven over the
//! wire the service actually speaks: `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a start STORES ONE ANALYSIS OF THE PATH THE REQUEST NAMES and the describe REPORTS
//!    THAT STORE: the path id, the scoping accounts and ARNs and the tags the request sets
//!    round-trip, and the members the service owns (the analysis id, its ARN, its status and
//!    its start date) are derived rather than taken;
//! 2. the CLOCK, not a caller, ends an analysis — `running` -> `failed`, carrying the reason
//!    this emulator traced no path, and never a reachability verdict, a path component or an
//!    explanation that nothing computed;
//! 3. one `ClientToken` names one analysis, and reusing it for a different request is
//!    refused with `IdempotentParameterMismatch`;
//! 4. `NetworkInsightsAnalysisIds`, `NetworkInsightsPathId`, the
//!    `AnalysisStartTime`/`AnalysisEndTime` window, the two filters the model documents and
//!    `MaxResults` narrow and page the report in analysis-id order, and an id or a path the
//!    account does not hold, a filter name outside that catalog, a page size outside the
//!    published range, a token this service did not issue or an oversized list is refused
//!    with EC2's own error code;
//! 5. a `DryRun` of either operation reports `DryRunOperation` and stores nothing;
//! 6. the analysis store's count ceiling is STRUCTURAL — the writer itself refuses past it,
//!    so no caller can grow the store without bound, and the start answers EC2's quota code.
//!
//! Each table row is a distinct input along what the rule generalises over — how many tags
//! and how long their values are, how many accounts and ARNs scope the analysis, how many
//! analyses one path carries, which client token, which bound of the start-time window and
//! in which direction, which filter name and value form, which page size at each published
//! bound and one past it — so no row is the specification.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use ec2::ops::types::ANALYSIS_STATUS_VALUES;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, NetworkInsightsAnalysisSpec,
    NetworkInsightsPathSpec, ANALYSIS_STATUS_FAILED, ANALYSIS_STATUS_RUNNING,
    IDEMPOTENT_PARAMETER_MISMATCH, INVALID_FILTER, INVALID_NETWORK_INSIGHTS_ANALYSIS_ID_NOT_FOUND,
    INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND, MAX_ANALYSIS_SCOPING_ENTRIES,
    MAX_NETWORK_INSIGHTS_ANALYSES, NETWORK_INSIGHTS_ANALYSIS_ID_PREFIX,
    PATH_ANALYSIS_UNSUPPORTED_STATUS_MESSAGE, RESOURCE_COUNT_EXCEEDED,
    RESOURCE_TYPE_NETWORK_INSIGHTS_ANALYSIS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed, unescaped};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for. An analysis's ARN names the Region the request
/// was signed for, so it is read back out of that ARN below.
const REGION: &str = "us-west-2";

/// The filter names this operation supports, in the order the SERVICE enumerates them in
/// the refusal that names the catalog.
///
/// `DescribeNetworkInsightsAnalysesRequest$Filters` documents them itself: "The filters. The
/// following are the possible values: path-found - A Boolean value that indicates whether a
/// feasible path is found. status - The status of the analysis (running | succeeded |
/// failed)". The ORDER here is the service's own rather than that bullet list — `status`
/// first, the way the recorded `InvalidFilter` refusal of this operation enumerates them and
/// the way the sibling access-scope analyses describe enumerates its own pair. The suite
/// exercises both entries, including `path-found`, which the issue this slice was filed
/// under never mentioned. `pub(super)` so the suite that pins this refusal's WORDING over
/// the whole family of fixed-text refusals ([`super::refusal_fixed_text`]) names the catalog
/// from the one place this test target holds it.
pub(super) const SUPPORTED_FILTERS: [&str; 2] = ["status", "path-found"];

/// A well-formed path id the account holds no path under.
const UNKNOWN_PATH_ID: &str = "nip-0ffffffffffffffff";

/// A well-formed analysis id the account holds no analysis under.
const UNKNOWN_ANALYSIS_ID: &str = "nia-0ffffffffffffffff";

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

/// One `StartNetworkInsightsAnalysis` call.
async fn start(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "StartNetworkInsightsAnalysis", params).await
}

/// One `DescribeNetworkInsightsAnalyses` call.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "DescribeNetworkInsightsAnalyses", params).await
}

/// One `DescribeNetworkInsightsAnalyses` call that has to succeed.
async fn described(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    ok(case, router, "DescribeNetworkInsightsAnalyses", params).await
}

/// A Reachability Analyzer path of the account, as its id.
///
/// Written through the store's own one path-insertion path — the one
/// `CreateNetworkInsightsPath` writes through, which is still a scaffold stub of its own —
/// rather than through a second hand-built copy of the modelled shape. This suite is about
/// the ANALYSES started on a path, so what a create stores for the path itself is that
/// operation's own cover.
fn seed_path(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_network_insights_path(NetworkInsightsPathSpec {
            source: Some("igw-0123456789abcdef0".to_string()),
            destination: Some("i-0123456789abcdef0".to_string()),
            protocol: Some("tcp".to_string()),
            destination_port: Some(443),
            region: REGION.to_string(),
            ..NetworkInsightsPathSpec::default()
        })
        .expect("the store holds room for the suite's paths")
}

/// How many analyses the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().network_insights_analyses.len()
}

/// One analysis as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Analysis {
    analysis_id: String,
    analysis_arn: String,
    path_id: String,
    status: String,
    status_message: Option<String>,
    warning_message: Option<String>,
    start_date: String,
    network_path_found: Option<String>,
    has_forward_path_components: bool,
    has_explanations: bool,
    additional_accounts: Vec<String>,
    filter_in_arns: Vec<String>,
    filter_out_arns: Vec<String>,
    tags: Vec<(String, String)>,
}

/// The text of every `<item>` of the `name` set of `node`, in document order.
fn set_values(node: roxmltree::Node<'_, '_>, name: &str) -> Vec<String> {
    child(node, name)
        .map(|set| {
            set.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every analysis a response carries, in document order: the elements that have a
/// `networkInsightsAnalysisId` child, which is both `<networkInsightsAnalysis>` in a start's
/// answer and each `<item>` of the describe's `<networkInsightsAnalysisSet>`.
fn reported(case: &str, body: &str) -> Vec<Analysis> {
    let doc = document(case, body);
    doc.descendants()
        .filter(|node| node.is_element() && child(*node, "networkInsightsAnalysisId").is_some())
        .map(|node| Analysis {
            analysis_id: child_text(node, "networkInsightsAnalysisId").unwrap_or_default(),
            analysis_arn: child_text(node, "networkInsightsAnalysisArn").unwrap_or_default(),
            path_id: child_text(node, "networkInsightsPathId").unwrap_or_default(),
            status: child_text(node, "status").unwrap_or_default(),
            status_message: child_text(node, "statusMessage"),
            warning_message: child_text(node, "warningMessage"),
            start_date: child_text(node, "startDate").unwrap_or_default(),
            network_path_found: child_text(node, "networkPathFound"),
            has_forward_path_components: child(node, "forwardPathComponentSet").is_some(),
            has_explanations: child(node, "explanationSet").is_some(),
            additional_accounts: set_values(node, "additionalAccountSet"),
            filter_in_arns: set_values(node, "filterInArnSet"),
            filter_out_arns: set_values(node, "filterOutArnSet"),
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
        RESOURCE_TYPE_NETWORK_INSIGHTS_ANALYSIS,
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

/// What one start scopes its analysis with, beside the path and the token: the member
/// accounts the path may traverse and the ARNs it must traverse or ignore.
#[derive(Debug, Clone, Copy, Default)]
struct Scoping<'a> {
    additional_accounts: &'a [&'a str],
    filter_in_arns: &'a [&'a str],
    filter_out_arns: &'a [&'a str],
}

/// A start call for `path_id` under `client_token`, with `tags` and `scoping`.
fn start_params(
    path_id: &str,
    client_token: &str,
    tags: &[(&str, &str)],
    scoping: Scoping<'_>,
) -> Vec<Param> {
    let mut params = vec![
        param("NetworkInsightsPathId", path_id),
        param("ClientToken", client_token),
    ];
    params.extend(tag_specification(tags));
    params.extend(indexed("AdditionalAccount", scoping.additional_accounts));
    params.extend(indexed("FilterInArn", scoping.filter_in_arns));
    params.extend(indexed("FilterOutArn", scoping.filter_out_arns));
    params
}

/// One analysis started on `path_id`, as the start reported it.
async fn start_analysis(
    case: &str,
    router: &MultiServiceRouter,
    path_id: &str,
    client_token: &str,
    tags: &[(&str, &str)],
    scoping: Scoping<'_>,
) -> Analysis {
    let body = ok(
        case,
        router,
        "StartNetworkInsightsAnalysis",
        &start_params(path_id, client_token, tags, scoping),
    )
    .await;
    only(case, &body)
}

/// One plain analysis of `path_id`: no tags, no scoping lists.
async fn start_plain(
    case: &str,
    router: &MultiServiceRouter,
    path_id: &str,
    client_token: &str,
) -> Analysis {
    start_analysis(case, router, path_id, client_token, &[], Scoping::default()).await
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

/// `vec!["a".to_string(), …]` for the entries a scoping list was given.
fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// One row of the start table: the case it names, the tags the request assigns and the
/// lists scoping the analysis it starts.
type StartRow<'a> = (&'a str, &'a [(&'a str, &'a str)], Scoping<'a>);

// ---------------------------------------------------------------------------
// RULE 1: a start stores the analysis, and the describe reports the store
// ---------------------------------------------------------------------------

/// RULE 1: the analysis a start answers is the one the describe reports, the path, the
/// scoping lists and the tags the request set round-trip, and the members the service owns
/// are derived.
///
/// The rows differ in how many tags the request assigns, how long their values are and how
/// many accounts and ARNs scope the analysis, so no one shape of request is the
/// specification.
#[tokio::test]
async fn a_start_stores_the_analysis_and_the_describe_reports_it() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, the tags the start assigns, the lists scoping it)
    let cases: [StartRow; 5] = [
        ("untagged and unscoped", &[], Scoping::default()),
        (
            "one tag",
            &[("Name", "nightly-path-check")],
            Scoping::default(),
        ),
        (
            "three tags",
            &[("Name", "audit"), ("Team", "network"), ("Tier", "prod")],
            Scoping::default(),
        ),
        ("a tag whose value is empty", &[("Owner", "")], Scoping::default()),
        (
            "scoped by accounts and ARNs",
            &[("Name", "cross-account")],
            Scoping {
                additional_accounts: &["111122223333", "444455556666"],
                filter_in_arns: &["arn:aws:ec2:us-west-2:111122223333:subnet/subnet-01"],
                filter_out_arns: &[
                    "arn:aws:ec2:us-west-2:111122223333:natgateway/nat-01",
                    "arn:aws:ec2:us-west-2:111122223333:natgateway/nat-02",
                ],
            },
        ),
    ];

    for (index, (case, tags, scoping)) in cases.into_iter().enumerate() {
        // A path per row, so no row's analysis belongs to another row's path.
        let path_id = seed_path(&engine);
        let client_token = format!("token-{index}");
        let started = start_analysis(case, &router, &path_id, &client_token, tags, scoping).await;

        assert!(
            is_long_id(NETWORK_INSIGHTS_ANALYSIS_ID_PREFIX, &started.analysis_id),
            "case {case:?}: expected an analysis id the service minted, got {:?}",
            started.analysis_id
        );
        let expected_arn = format!(
            "arn:aws:ec2:{REGION}:{account}:{RESOURCE_TYPE_NETWORK_INSIGHTS_ANALYSIS}/{id}",
            account = engine::emulator_account_id(),
            id = started.analysis_id,
        );
        assert_eq!(
            started.analysis_arn, expected_arn,
            "case {case:?}: expected the analysis's own ARN {expected_arn:?}, got {:?}",
            started.analysis_arn
        );
        assert_eq!(
            started.path_id, path_id,
            "case {case:?}: expected the analysis of path {path_id:?}, got {:?}",
            started.path_id
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
                started.status_message.as_deref(),
                started.warning_message.as_deref(),
                started.network_path_found.as_deref(),
            ),
            (None, None, None),
            "case {case:?}: a running analysis has no status message, no warning and no \
             reachability verdict yet, got {started:?}"
        );
        assert_eq!(
            (
                started.additional_accounts.clone(),
                started.filter_in_arns.clone(),
                started.filter_out_arns.clone(),
            ),
            (
                owned(scoping.additional_accounts),
                owned(scoping.filter_in_arns),
                owned(scoping.filter_out_arns),
            ),
            "case {case:?}: expected the scoping lists the request set, got {started:?}"
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
        let body = described(
            case,
            &router,
            &indexed("NetworkInsightsAnalysisId", &[started.analysis_id.as_str()]),
        )
        .await;
        let read_back = only(case, &body);
        assert_eq!(
            read_back, started,
            "case {case:?}: the describe must report the analysis the start stored, got \
             {read_back:?} for {started:?}"
        );
        assert_eq!(
            stored_count(&engine),
            index + 1,
            "case {case:?}: expected one stored analysis per row so far"
        );
    }
}

/// RULE 1: one path carries every analysis started on it, each with its own id, and the
/// describe narrowed to that path reports all of them in id order — and none of another
/// path's.
///
/// The rows differ in how many analyses the path carries.
#[tokio::test]
async fn a_path_carries_every_analysis_started_on_it() {
    let (router, engine) = super::ec2_query::emulator();
    // A second path of the account, with an analysis of its own: narrowing to one path must
    // leave this one out.
    let other_path = seed_path(&engine);
    let other = start_plain("another path", &router, &other_path, "other-token").await;

    for count in [1usize, 2, 5] {
        let case = format!("{count} analyses of one path");
        let path_id = seed_path(&engine);
        let mut started: Vec<String> = Vec::new();
        for index in 0..count {
            let analysis =
                start_plain(&case, &router, &path_id, &format!("{path_id}-{index}")).await;
            started.push(analysis.analysis_id);
        }
        started.sort();
        assert_eq!(
            started.len(),
            count,
            "case {case:?}: every start mints its own analysis, got {started:?}"
        );

        let body = described(&case, &router, &[param("NetworkInsightsPathId", &path_id)]).await;
        let reported = reported_ids(&case, &body);
        assert_eq!(
            reported, started,
            "case {case:?}: expected every analysis of {path_id:?} in id order, got {reported:?}"
        );
        assert!(
            !reported.contains(&other.analysis_id),
            "case {case:?}: another path's analysis must not be reported, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the clock ends the analysis
// ---------------------------------------------------------------------------

/// RULE 2: an analysis is `running` when the start answers — the span is never zero, so a
/// caller reading straight after the call still sees it — and the CLOCK moves it to the
/// terminal `failed` status, carrying why no path was traced. Nothing reports a reachability
/// verdict, a path component or an explanation that nothing computed.
#[tokio::test]
async fn the_clock_ends_the_analysis_with_the_reason_nothing_traced_the_path() {
    let case = "the clock ends the analysis";
    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);
    let started = start_plain(case, &router, &path_id, "clock-token").await;

    // Straight after the start, before the span has passed: still running.
    let ids = indexed("NetworkInsightsAnalysisId", &[started.analysis_id.as_str()]);
    let body = described(case, &router, &ids).await;
    let polled = only(case, &body);
    assert_eq!(
        polled.status, ANALYSIS_STATUS_RUNNING,
        "case {case:?}: a caller reading straight after the start still sees the analysis \
         running, got {polled:?}"
    );

    past_the_transition().await;

    let body = described(case, &router, &ids).await;
    let ended = only(case, &body);
    assert_eq!(
        ended.status, ANALYSIS_STATUS_FAILED,
        "case {case:?}: expected the terminal status the clock owes the analysis, got {ended:?}"
    );
    assert!(
        ANALYSIS_STATUS_VALUES.contains(&ended.status.as_str()),
        "case {case:?}: expected a member of the modelled AnalysisStatus catalog \
         {ANALYSIS_STATUS_VALUES:?}, got {ended:?}"
    );
    assert_eq!(
        ended.status_message.as_deref(),
        Some(PATH_ANALYSIS_UNSUPPORTED_STATUS_MESSAGE),
        "case {case:?}: expected the reason no reachability result was produced, got {ended:?}"
    );
    assert_eq!(
        ended.network_path_found, None,
        "case {case:?}: nothing traced the path, so whether it is reachable is not reported, \
         got {ended:?}"
    );
    assert!(
        !ended.has_forward_path_components && !ended.has_explanations,
        "case {case:?}: no path component and no explanation is reported for a trace nothing \
         ran, got {ended:?}"
    );
    // The id, the ARN, the path and the start date are the start's, unchanged by the move.
    assert_eq!(
        (
            ended.analysis_id.as_str(),
            ended.analysis_arn.as_str(),
            ended.path_id.as_str(),
            ended.start_date.as_str(),
        ),
        (
            started.analysis_id.as_str(),
            started.analysis_arn.as_str(),
            started.path_id.as_str(),
            started.start_date.as_str(),
        ),
        "case {case:?}: the move changes the status, not the identity, got {ended:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: one ClientToken names one analysis
// ---------------------------------------------------------------------------

/// RULE 3: repeating a start with the same `ClientToken` answers the analysis that token
/// already started and stores no second one; a different token on the same path starts
/// another analysis, and the same token on a DIFFERENT request is refused.
///
/// The rows differ in the token's own form — short, long, punctuated, non-ASCII — none of
/// which the service reads as anything but a name, and then in which other path the reused
/// token names.
#[tokio::test]
async fn one_client_token_names_one_analysis() {
    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);

    let tokens = [
        "a",
        "0123456789abcdef0123456789abcdef",
        "token.with-punctuation_and~tilde",
        "tōken-with-non-ascii",
    ];
    for (index, token) in tokens.into_iter().enumerate() {
        let case = format!("ClientToken={token:?}");
        let first = start_plain(&case, &router, &path_id, token).await;
        let replayed = start_plain(&case, &router, &path_id, token).await;
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

    // A token names ONE request: reusing it for a different path is refused rather than
    // answered with the analysis it already started. The rows differ in which path the
    // second request names — another path of the account, and one the account does not hold
    // at all.
    let other_path = seed_path(&engine);
    let held = stored_count(&engine);
    for (case, path) in [
        ("another path of the account", other_path.as_str()),
        ("a path the account does not hold", UNKNOWN_PATH_ID),
    ] {
        let (status, body) = start(
            &router,
            &start_params(path, "a", &[], Scoping::default()),
        )
        .await;
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

    // The same token with the same path is still the replay it always was.
    let again = start_plain("the token replayed again", &router, &path_id, "a").await;
    assert_eq!(
        stored_count(&engine),
        held,
        "the token replayed again: still no second analysis, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: narrowing, paging and refusals
// ---------------------------------------------------------------------------

/// RULE 4: the start-time window selects on the analysis's own start date — the only instant
/// the reported shape carries — with both bounds INCLUSIVE.
///
/// The rows differ in which bound the request carries, in which direction it sits from the
/// start, and in whether both bounds are given — including a window that has already closed,
/// one that opens later and an inverted one.
#[tokio::test]
async fn the_time_window_selects_on_the_analysis_start_date() {
    let case = "the start-time window";
    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);
    let started = start_plain(case, &router, &path_id, "window-token").await;
    let at = start_second(case, &started);

    // (case, AnalysisStartTime, AnalysisEndTime, is the analysis reported?)
    let cases: [(&str, Option<i64>, Option<i64>, bool); 7] = [
        ("from the second it started in", Some(at), None, true),
        ("from a minute later", Some(at + 60), None, false),
        ("until a second later", None, Some(at + 1), true),
        ("until a minute earlier", None, Some(at - 60), false),
        ("a window around it", Some(at - 60), Some(at + 60), true),
        ("a window after it", Some(at + 60), Some(at + 120), false),
        ("an inverted window", Some(at + 60), Some(at - 60), false),
    ];

    for (row, begin, end, reported_in_window) in cases {
        let mut params = Vec::new();
        if let Some(begin) = begin {
            params.push(param("AnalysisStartTime", &timestamp(begin)));
        }
        if let Some(end) = end {
            params.push(param("AnalysisEndTime", &timestamp(end)));
        }
        let body = described(row, &router, &params).await;
        let ids = reported_ids(row, &body);
        let expected: Vec<String> = if reported_in_window {
            vec![started.analysis_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids, expected,
            "case {row:?}: expected {expected:?} for the window start={begin:?} end={end:?}, \
             got {ids:?}"
        );
    }
}

/// RULE 4: an id or a path the account does not hold is refused with EC2's own
/// `Invalid<Resource>Id.NotFound` code rather than reported as an absent row, by both
/// operations.
#[tokio::test]
async fn an_id_the_account_does_not_hold_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);
    let held = start_plain("held", &router, &path_id, "held-token").await;

    // (case, the describe's parameters, the expected code)
    let cases: [(&str, Vec<Param>, &str); 5] = [
        (
            "an unknown analysis id",
            indexed("NetworkInsightsAnalysisId", &[UNKNOWN_ANALYSIS_ID]),
            INVALID_NETWORK_INSIGHTS_ANALYSIS_ID_NOT_FOUND,
        ),
        (
            "a known and an unknown analysis id",
            indexed(
                "NetworkInsightsAnalysisId",
                &[held.analysis_id.as_str(), UNKNOWN_ANALYSIS_ID],
            ),
            INVALID_NETWORK_INSIGHTS_ANALYSIS_ID_NOT_FOUND,
        ),
        (
            "an analysis id that is not an id at all",
            indexed("NetworkInsightsAnalysisId", &["not-an-analysis"]),
            INVALID_NETWORK_INSIGHTS_ANALYSIS_ID_NOT_FOUND,
        ),
        (
            "an unknown path id",
            vec![param("NetworkInsightsPathId", UNKNOWN_PATH_ID)],
            INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND,
        ),
        (
            "a path id that is not an id at all",
            vec![param("NetworkInsightsPathId", "not-a-path")],
            INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND,
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

    // The start resolves the path the same way, and stores nothing when it refuses.
    for path in [UNKNOWN_PATH_ID, "not-a-path"] {
        let (status, body) = start(
            &router,
            &start_params(path, &format!("unknown-{path}"), &[], Scoping::default()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {path:?}: expected HTTP 400 from the start, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND,
            "case {path:?}: expected {INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {path:?}: a refused start stores nothing"
        );
    }
}

/// RULE 4: the two filters the model documents select on the analysis's own members, and
/// every other filter name is refused with EC2's `InvalidFilter` and the message naming the
/// supported catalog.
#[tokio::test]
async fn the_documented_filters_select_and_every_other_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);
    let running = start_plain("running", &router, &path_id, "filter-token").await;

    // Every supported name, on an analysis the clock has not ended yet: `status` holds
    // `running`, and `path-found` holds nothing at all, so it selects nothing.
    //
    // (case, filter name, values, is the analysis reported?)
    let while_running: [(&str, &str, &[&str], bool); 7] = [
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
            "path-found true, which no trace determined",
            "path-found",
            &["true"],
            false,
        ),
        (
            "path-found false, which no trace determined either",
            "path-found",
            &["false"],
            false,
        ),
        ("a path-found wildcard", "path-found", &["*"], false),
    ];
    for (case, name, values, selects) in while_running {
        let body = described(case, &router, &filters(&[(name, values)])).await;
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

    // Once the clock has ended it, `status` selects on what it reports NOW.
    past_the_transition().await;
    let when_ended: [(&str, &str, &[&str], bool); 3] = [
        ("the ended status", "status", &[ANALYSIS_STATUS_FAILED], true),
        (
            "the status it left",
            "status",
            &[ANALYSIS_STATUS_RUNNING],
            false,
        ),
        ("path-found on an ended analysis", "path-found", &["*"], false),
    ];
    for (case, name, values, selects) in when_ended {
        let body = described(case, &router, &filters(&[(name, values)])).await;
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
    let body = described(
        "both filters",
        &router,
        &filters(&[
            ("status", &[ANALYSIS_STATUS_FAILED]),
            ("path-found", &["true"]),
        ]),
    )
    .await;
    assert_eq!(
        reported_ids("both filters", &body),
        Vec::<String>::new(),
        "both filters: filters are AND-ed, so one that does not hold selects nothing"
    );

    // Every name outside the documented catalog, including ones other EC2 describes document.
    let unsupported: [(&str, &[&str]); 6] = [
        ("network-insights-path-id", &[path_id.as_str()]),
        (
            "network-insights-analysis-id",
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
    let path_id = seed_path(&engine);
    let mut seeded: Vec<String> = Vec::new();
    for index in 0..7 {
        let analysis = start_plain("seed", &router, &path_id, &format!("page-{index}")).await;
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
            let body = described(&case, &router, &params).await;
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

    let body = described("no MaxResults", &router, &[]).await;
    assert_eq!(
        reported_ids("no MaxResults", &body),
        seeded,
        "no MaxResults: expected every analysis in one answer"
    );
    assert_eq!(
        stored_count(&engine),
        seeded.len(),
        "every start stored one analysis"
    );
}

/// RULE 4: `MaxResults` outside the range the model publishes is refused at the boundary;
/// each published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);
    start_plain("bounds", &router, &path_id, "bounds-token").await;

    // The published bounds (`NetworkInsightsMaxResults`, `range 1..100`) and one past each.
    for (max_results, expected_status) in [(0, 400), (1, 200), (100, 200), (101, 400)] {
        let (status, body) =
            describe(&router, &[param("MaxResults", &max_results.to_string())]).await;
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
    let path_id = seed_path(&engine);
    let held = start_plain("held", &router, &path_id, "held-token").await;

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

/// RULE 4: a request-sized list is capped before the store is read — the describe's ids at
/// [`ec2::state::MAX_RESOURCE_IDS`] and each of the start's scoping lists at
/// [`MAX_ANALYSIS_SCOPING_ENTRIES`] — and each cap is accepted AT its bound.
#[tokio::test]
async fn the_request_sized_lists_are_capped() {
    use ec2::state::MAX_RESOURCE_IDS;

    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);
    let held = start_plain("held", &router, &path_id, "held-token").await;

    // At the bound the request is read (the held id, then the cap filled out with repeats of
    // it, so every id resolves); one past it is refused for its size alone.
    for (count, expected_status) in [(MAX_RESOURCE_IDS, 200), (MAX_RESOURCE_IDS + 1, 400)] {
        let ids: Vec<&str> = (0..count).map(|_| held.analysis_id.as_str()).collect();
        let (status, body) = describe(&router, &indexed("NetworkInsightsAnalysisId", &ids)).await;
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

    // Each scoping list of the start, at its bound and one past it. The accounts are twelve
    // digits and the ARNs are ARNs, so the rows differ in the member AND in what it holds.
    let accounts: Vec<String> = (0..=MAX_ANALYSIS_SCOPING_ENTRIES)
        .map(|index| format!("{:012}", index + 1))
        .collect();
    let arns: Vec<String> = (0..=MAX_ANALYSIS_SCOPING_ENTRIES)
        .map(|index| format!("arn:aws:ec2:{REGION}:111122223333:subnet/subnet-{index:08}"))
        .collect();
    for (member, entries) in [
        ("AdditionalAccount", &accounts),
        ("FilterInArn", &arns),
        ("FilterOutArn", &arns),
    ] {
        for (count, expected_status) in [
            (MAX_ANALYSIS_SCOPING_ENTRIES, 200),
            (MAX_ANALYSIS_SCOPING_ENTRIES + 1, 400),
        ] {
            let values: Vec<&str> = entries
                .iter()
                .take(count)
                .map(String::as_str)
                .collect();
            let mut params = start_params(
                &path_id,
                &format!("{member}-{count}"),
                &[],
                Scoping::default(),
            );
            params.extend(indexed(member, &values));
            let (status, body) = start(&router, &params).await;
            assert_eq!(
                status, expected_status,
                "case {member} x{count}: expected HTTP {expected_status}, got {status} with \
                 body {body}"
            );
            if expected_status == 400 {
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValue",
                    "case {member} x{count}: expected InvalidParameterValue, got {body}"
                );
            }
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
    let path_id = seed_path(&engine);
    start_plain("held", &router, &path_id, "held-token").await;
    let held = stored_count(&engine);

    // (case, action, parameters)
    let cases: [(&str, &str, Vec<Param>); 3] = [
        ("a dry-run start", "StartNetworkInsightsAnalysis", {
            let mut params = start_params(
                &path_id,
                "dry-run-token",
                &[("Name", "audit")],
                Scoping::default(),
            );
            params.push(param("DryRun", "true"));
            params
        }),
        (
            "a dry-run describe",
            "DescribeNetworkInsightsAnalyses",
            vec![param("DryRun", "true")],
        ),
        (
            "a dry-run describe of one path",
            "DescribeNetworkInsightsAnalyses",
            vec![
                param("DryRun", "true"),
                param("NetworkInsightsPathId", &path_id),
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
/// `has_network_insights_analysis_capacity` cannot grow it past
/// [`MAX_NETWORK_INSIGHTS_ANALYSES`] — and a start past the ceiling answers EC2's quota code
/// rather than reporting an analysis the store does not hold.
#[tokio::test]
async fn the_analysis_store_has_a_structural_count_ceiling() {
    let case = "the store's ceiling";
    let (router, engine) = super::ec2_query::emulator();
    let path_id = seed_path(&engine);

    let spec = |client_token: String| NetworkInsightsAnalysisSpec {
        network_insights_path_id: path_id.clone(),
        additional_accounts: Vec::new(),
        filter_in_arns: Vec::new(),
        filter_out_arns: Vec::new(),
        region: REGION.to_string(),
        tags: Vec::new(),
        client_token,
    };

    // Fill the store to the ceiling through the one writer, bypassing the handler's own
    // gate: the row under test is the write that does not fit.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for index in 0..MAX_NETWORK_INSIGHTS_ANALYSES {
            let stored = guard.insert_network_insights_analysis(spec(format!("ceiling-{index}")));
            assert!(
                stored.is_some(),
                "case {case:?}: analysis {index} is inside the ceiling and must be stored"
            );
        }
        assert!(
            guard
                .insert_network_insights_analysis(spec("past-the-ceiling".to_string()))
                .is_none(),
            "case {case:?}: the writer itself must refuse past {MAX_NETWORK_INSIGHTS_ANALYSES} \
             analyses"
        );
    }
    assert_eq!(
        stored_count(&engine),
        MAX_NETWORK_INSIGHTS_ANALYSES,
        "case {case:?}: a refused write stores nothing"
    );

    let (status, body) = start(
        &router,
        &start_params(&path_id, "over-the-ceiling", &[], Scoping::default()),
    )
    .await;
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
        MAX_NETWORK_INSIGHTS_ANALYSES,
        "case {case:?}: a refused start stores nothing"
    );
}
