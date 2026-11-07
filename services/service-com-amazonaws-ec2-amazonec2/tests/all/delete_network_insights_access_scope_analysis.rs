// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteNetworkInsightsAccessScopeAnalysis`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DeleteNetworkInsightsAccessScopeAnalysis`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. the delete TAKES THE ANALYSIS THE REQUEST NAMES OUT OF THE STORE and answers that id:
//!    every other analysis of the account is left where it was, the bytes the removed record
//!    held go back to the store's budget, the client token that named it names it no longer,
//!    and the analysis is gone for every later caller —
//!    `DescribeNetworkInsightsAccessScopeAnalyses`, which reads the same store, no longer
//!    reports it and no longer resolves its id;
//! 2. an id this account holds no analysis under is REFUSED — whatever its form, including
//!    the id of an analysis an earlier call already deleted — with EC2's
//!    `InvalidParameterValue` stating that the `NetworkInsightsAccessScopeAnalysisId` it was
//!    given is not a valid one, and nothing is removed;
//! 3. a `DryRun` reports `DryRunOperation` and removes nothing, while `DryRun=false` is an
//!    ordinary delete — and a dry run naming an analysis the account does not hold is refused
//!    with that reason instead.
//!
//! Each table row is a distinct input along what the rules generalise over — how many tags
//! the analysis carries and how long their values are, which of several analyses of one scope
//! is deleted and in which order, whether the analysis is still `running` or the clock has
//! already ended it, and each way a value can fail to name an analysis of the account — so no
//! one recorded call is the specification.

use std::time::Duration;

use ec2::state::{
    AccessScopePaths, ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION,
    INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_NOT_FOUND,
    INVALID_PARAMETER_VALUE, NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX,
    NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX, RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed};
use super::wire::{document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// The tag sets the tables here are written against: what makes two stored analyses differ
/// in the caller-chosen bytes they hold, which is what the delete has to give back.
const TAG_ROWS: [(&str, &[(&str, &str)]); 4] = [
    ("untagged", &[]),
    ("one tag", &[("Name", "nightly-scope-check")]),
    (
        "three tags",
        &[("Name", "audit"), ("Team", "network"), ("Tier", "prod")],
    ),
    ("a tag whose value is empty", &[("Owner", "")]),
];

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

/// One `DeleteNetworkInsightsAccessScopeAnalysis` call carrying `params`.
async fn delete(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "DeleteNetworkInsightsAccessScopeAnalysis", params).await
}

/// One `DeleteNetworkInsightsAccessScopeAnalysis` call naming `analysis_id`.
async fn delete_analysis(router: &MultiServiceRouter, analysis_id: &str) -> (u16, String) {
    delete(
        router,
        &[param("NetworkInsightsAccessScopeAnalysisId", analysis_id)],
    )
    .await
}

/// A Network Access Scope of the account, as its id.
///
/// Written through the store's own one insertion path — the same path
/// `CreateNetworkInsightsAccessScope` writes through — so the scope the analyses below are
/// started against is one the account really holds. This suite is about the ANALYSES, so the
/// scope carries no content paths of its own.
fn seed_scope(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_network_insights_access_scope(REGION, Vec::new(), AccessScopePaths::default(), None)
        .expect("the store holds room for the suite's scope")
}

/// How many Network Access Scope analyses the store holds.
fn stored_analyses(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_access_scope_analyses
        .len()
}

/// Every analysis id the store holds, in store order.
fn held_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_access_scope_analyses
        .keys()
        .cloned()
        .collect()
}

/// The caller-chosen bytes the store currently holds against its one byte budget.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
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

/// One analysis started on `scope_id` under `client_token`, as the id the start answered.
async fn start_analysis(
    case: &str,
    router: &MultiServiceRouter,
    scope_id: &str,
    client_token: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut params = vec![
        param("NetworkInsightsAccessScopeId", scope_id),
        param("ClientToken", client_token),
    ];
    params.extend(tag_specification(tags));
    let body = ok(
        case,
        router,
        "StartNetworkInsightsAccessScopeAnalysis",
        &params,
    )
    .await;
    element(&body, "networkInsightsAccessScopeAnalysisId").unwrap_or_else(|| {
        panic!("case {case:?}: the start must answer an analysis id, got {body}")
    })
}

/// The analysis ids the unfiltered `DescribeNetworkInsightsAccessScopeAnalyses` reports.
async fn described_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeNetworkInsightsAccessScopeAnalyses",
        &[],
    )
    .await;
    elements(&body, "networkInsightsAccessScopeAnalysisId")
}

/// The `DescribeNetworkInsightsAccessScopeAnalyses` of exactly `analysis_id`, as its status
/// and body: the read this suite proves a delete through. It resolves its id against the SAME
/// store the delete removes from, so an id it no longer resolves is one no later caller can
/// reach.
async fn describe_one(router: &MultiServiceRouter, analysis_id: &str) -> (u16, String) {
    api(
        router,
        "DescribeNetworkInsightsAccessScopeAnalyses",
        &indexed("NetworkInsightsAccessScopeAnalysisId", &[analysis_id]),
    )
    .await
}

/// The `<Message>` of a refusal, read out of the PARSED XML envelope: the body has to be
/// well-formed XML and the element's own text is what is answered, so a caller-controlled
/// value the service wrote into the markup unescaped cannot read back as the message it
/// belongs to.
fn refusal_message(case: &str, body: &str) -> String {
    document(case, body)
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Message")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {body}"))
        .to_string()
}

/// The refusal every id this account holds no analysis under gets from this operation.
fn unresolvable_message(analysis_id: &str) -> String {
    format!("NetworkInsightsAccessScopeAnalysisId ({analysis_id}) is not valid")
}

/// Each way a value can fail to name an analysis of the account: both published EC2 id suffix
/// lengths with digits nothing was created for, a suffix of neither length, an upper-case
/// suffix, a non-hexadecimal suffix, the id of the SCOPE resource of the same surface,
/// another resource type's id, free text, and a value carrying the characters that are
/// significant in the XML envelope the refusal is written into.
fn unresolvable_ids() -> Vec<(&'static str, String)> {
    let prefix = NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX;
    vec![
        ("the long form", format!("{prefix}-00000000000000000")),
        ("the short form", format!("{prefix}-00000000")),
        ("neither length", format!("{prefix}-0000000000000000")),
        ("an upper-case suffix", format!("{prefix}-0ABCDEF01")),
        ("a non-hex suffix", format!("{prefix}-zzzzzzzz")),
        (
            "the scope resource's id",
            format!("{NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX}-00000000000000000"),
        ),
        ("another resource's id", "vpc-00000000000000000".to_string()),
        (
            "free text",
            "not-a-network-insights-access-scope-analysis".to_string(),
        ),
        ("XML metacharacters", "nisa-<tag>&'\"end".to_string()),
    ]
}

// ---------------------------------------------------------------------------
// RULE 1: the store is what is deleted, and the id is what is reported
// ---------------------------------------------------------------------------

/// RULE 1: deleting an analysis removes exactly it and answers its id; every other analysis
/// of the account is left where it was, and the analysis is gone for every later reader — the
/// describe that reported it a moment ago no longer does, and no longer resolves its id. One
/// row per tag set, crossed with which of three analyses of one scope is deleted.
#[tokio::test]
async fn deleting_an_analysis_removes_exactly_it_and_answers_its_id() {
    for (row, tags) in TAG_ROWS {
        for position in 0..3usize {
            let case = format!("{row}, deleted at position {position} of three");
            let case = case.as_str();
            let (router, engine) = super::ec2_query::emulator();
            let scope_id = seed_scope(&engine);

            // Three analyses of the one scope, so an analysis the request did not name has to
            // be left alone; the row's own tags sit on the one at `position`.
            let mut started = Vec::new();
            for index in 0..3usize {
                let row_tags: &[(&str, &str)] = if index == position { tags } else { &[] };
                started.push(
                    start_analysis(
                        case,
                        &router,
                        &scope_id,
                        &format!("token-{index}"),
                        row_tags,
                    )
                    .await,
                );
            }
            let target = started[position].clone();

            // The read reports the analysis BEFORE the delete, so the refusal below is the
            // delete's doing rather than a read that never reported it.
            let (status, body) = describe_one(&router, &target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the started analysis must resolve before the delete, got \
                 {status} with body {body}"
            );

            let (status, body) = delete_analysis(&router, &target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let answered = element(&body, "networkInsightsAccessScopeAnalysisId");
            assert_eq!(
                answered.as_deref(),
                Some(target.as_str()),
                "case {case:?}: expected the response to name {target}, got {answered:?} in \
                 {body}"
            );

            let remaining: Vec<String> = started
                .iter()
                .filter(|id| **id != target)
                .cloned()
                .collect();
            let held = held_ids(&engine);
            assert_eq!(
                held, remaining,
                "case {case:?}: expected the other analyses {remaining:?} to be left alone, \
                 got {held:?}"
            );

            // The unfiltered describe reports the store: the deleted analysis is gone from it
            // and the two the account still holds are both still there.
            let listed = described_ids(case, &router).await;
            assert!(
                !listed.contains(&target),
                "case {case:?}: the unfiltered describe must not list the deleted {target}, \
                 got {listed:?}"
            );
            for id in &remaining {
                assert!(
                    listed.contains(id),
                    "case {case:?}: the unfiltered describe must still list {id}, got {listed:?}"
                );
            }

            // The deleted id resolves for nobody: the read answers this resource's own
            // not-found code, which is NOT the wording the delete answers (a fact of the
            // operation, not of the id).
            let (status, body) = describe_one(&router, &target).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the describe of the deleted id to be refused, got \
                 {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_NOT_FOUND,
                "case {case:?}: expected \
                 {INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_NOT_FOUND} from the \
                 describe of the deleted id, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("{target} does not exist"),
                "case {case:?}: the describe's refusal must name {target}, got {body}"
            );
        }
    }
}

/// RULE 1: whatever ORDER the analyses are deleted in, the store is left holding exactly the
/// ones that were not deleted, in store order. The rows are distinct orders over the started
/// analyses, plus one that deletes every one of them.
#[tokio::test]
async fn deleting_in_any_order_leaves_exactly_the_analyses_not_deleted() {
    let orders: [&[usize]; 4] = [&[0, 1, 2], &[3, 2, 1], &[2, 0, 3], &[0, 1, 2, 3]];

    for order in orders {
        let (router, engine) = super::ec2_query::emulator();
        let case = format!("order {order:?}");
        let case = case.as_str();
        let scope_id = seed_scope(&engine);

        let mut started = Vec::new();
        for (index, (_, tags)) in TAG_ROWS.into_iter().enumerate() {
            started.push(
                start_analysis(case, &router, &scope_id, &format!("token-{index}"), tags).await,
            );
        }

        for position in order {
            let analysis_id = &started[*position];
            let (status, body) = delete_analysis(&router, analysis_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting {analysis_id} expected HTTP 200, got {status} with \
                 body {body}"
            );
            let answered = element(&body, "networkInsightsAccessScopeAnalysisId");
            assert_eq!(
                answered.as_deref(),
                Some(analysis_id.as_str()),
                "case {case:?}: expected the response to name {analysis_id}, got {answered:?}"
            );
        }

        let expected: Vec<String> = started
            .iter()
            .enumerate()
            .filter(|(position, _)| !order.contains(position))
            .map(|(_, id)| id.clone())
            .collect();
        let held = held_ids(&engine);
        assert_eq!(
            held, expected,
            "case {case:?}: expected the analyses not deleted {expected:?}, got {held:?}"
        );
    }
}

/// RULE 1: the bytes a deleted analysis held go back to the store's byte budget, so a start
/// followed by a delete leaves the store exactly where it started — whatever the analysis
/// carried. One row per tag set, which is what makes the charge vary: the tags and the client
/// token are the caller-chosen members a record retains.
#[tokio::test]
async fn the_bytes_a_deleted_analysis_held_go_back_to_the_store() {
    for (index, (case, tags)) in TAG_ROWS.into_iter().enumerate() {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = seed_scope(&engine);
        let before = stored_bytes(&engine);

        let analysis_id = start_analysis(
            case,
            &router,
            &scope_id,
            &format!("a-client-token-{index}"),
            tags,
        )
        .await;
        let charged = stored_bytes(&engine);
        assert!(
            charged > before,
            "case {case:?}: starting an analysis must charge the byte budget, got {charged} \
             after {before}"
        );

        let (status, body) = delete_analysis(&router, &analysis_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = stored_bytes(&engine);
        assert_eq!(
            after, before,
            "case {case:?}: expected the delete to give back the {} bytes the analysis held, \
             got {after} against {before}",
            charged - before
        );
    }
}

/// RULE 1: the `ClientToken` that named a deleted analysis names it no longer, so a start
/// replaying that token begins a NEW analysis instead of reporting one nothing holds. One row
/// per tag set.
#[tokio::test]
async fn the_client_token_of_a_deleted_analysis_names_it_no_longer() {
    for (case, tags) in TAG_ROWS {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = seed_scope(&engine);
        let token = "one-token-for-both-starts";

        let first = start_analysis(case, &router, &scope_id, token, tags).await;
        // The token is idempotent while the analysis is held: the replay reports that one.
        let replayed = start_analysis(case, &router, &scope_id, token, tags).await;
        assert_eq!(
            replayed, first,
            "case {case:?}: a replay before the delete must report {first}, got {replayed}"
        );

        let (status, body) = delete_analysis(&router, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let minted = start_analysis(case, &router, &scope_id, token, tags).await;
        assert_ne!(
            minted, first,
            "case {case:?}: a start replayed after the delete must begin a new analysis, got \
             {minted}"
        );
        let held = held_ids(&engine);
        assert_eq!(
            held,
            vec![minted.clone()],
            "case {case:?}: the store must hold exactly the new analysis {minted}, got {held:?}"
        );
    }
}

/// RULE 1: an analysis is deletable in either status it can be in — still `running`, and
/// ended by the clock — since what the delete removes is the record, not a state it is
/// waiting on. One sleep covers both rows: the first analysis is past its span by the time
/// the second is started.
#[tokio::test]
async fn an_analysis_is_deletable_running_and_after_the_clock_ended_it() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);

    let ended = start_analysis("ended by the clock", &router, &scope_id, "token-ended", &[]).await;
    past_the_transition().await;
    let running = start_analysis("still running", &router, &scope_id, "token-running", &[]).await;

    for (case, analysis_id) in [("ended by the clock", &ended), ("still running", &running)] {
        let (status, body) = delete_analysis(&router, analysis_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let answered = element(&body, "networkInsightsAccessScopeAnalysisId");
        assert_eq!(
            answered.as_deref(),
            Some(analysis_id.as_str()),
            "case {case:?}: expected the response to name {analysis_id}, got {answered:?}"
        );
    }
    assert_eq!(
        stored_analyses(&engine),
        0,
        "both analyses were deleted, so the store must hold none, got {}",
        stored_analyses(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 2: an id that names no analysis of the account
// ---------------------------------------------------------------------------

/// RULE 2: an id this account holds no analysis under is refused — HTTP 400
/// `InvalidParameterValue`, stating that the `NetworkInsightsAccessScopeAnalysisId` it was
/// given is not a valid one and naming the value the request sent — and nothing is removed.
/// One row per way a value can fail to name an analysis of the account.
#[tokio::test]
async fn an_analysis_id_that_names_no_analysis_is_refused() {
    for (case, analysis_id) in unresolvable_ids() {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = seed_scope(&engine);
        let held = start_analysis(case, &router, &scope_id, "a-held-analysis", &[]).await;

        let (status, body) = delete_analysis(&router, &analysis_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        // Read the message out of the PARSED envelope rather than by substring: a value whose
        // metacharacters reached the markup verbatim would either stop the body from parsing
        // or cut the element's text short at that character, so this is what pins that a
        // caller-controlled id is written into the envelope escaped.
        let message = refusal_message(case, &body);
        assert_eq!(
            message,
            unresolvable_message(&analysis_id),
            "case {case:?}: expected this operation's one sentence, got {message:?}"
        );
        assert_eq!(
            message,
            error_message(&body),
            "case {case:?}: the parsed message and the one on the wire must agree, got \
             {message:?} and {:?}",
            error_message(&body)
        );
        let remaining = held_ids(&engine);
        assert_eq!(
            remaining,
            vec![held.clone()],
            "case {case:?}: a refused delete must remove nothing, got {remaining:?}"
        );
    }
}

/// RULE 2: a delete is not idempotent — the analysis is gone after the first call, so a
/// second call naming the same id names no analysis of the account and gets the same refusal
/// any other unresolvable id gets, naming that id. One row per tag set.
#[tokio::test]
async fn deleting_the_same_analysis_twice_refuses_the_second_call() {
    for (case, tags) in TAG_ROWS {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = seed_scope(&engine);
        let analysis_id = start_analysis(case, &router, &scope_id, "a-token", tags).await;

        let (status, body) = delete_analysis(&router, &analysis_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first delete expected HTTP 200, got {status} with body {body}"
        );

        let (status, body) = delete_analysis(&router, &analysis_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the second delete expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: the second delete expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            unresolvable_message(&analysis_id),
            "case {case:?}: the second delete must name {analysis_id}, got {body}"
        );
        assert_eq!(
            stored_analyses(&engine),
            0,
            "case {case:?}: the refused second call must remove nothing, got {} analyses",
            stored_analyses(&engine)
        );
    }
}

/// RULE 2: a request that names no `NetworkInsightsAccessScopeAnalysisId` at all names no
/// analysis to delete, so it is refused rather than answered with an empty success, and
/// nothing is removed.
#[tokio::test]
async fn a_request_that_names_no_analysis_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let scope_id = seed_scope(&engine);
    let held = start_analysis("no id", &router, &scope_id, "a-token", &[]).await;

    let (status, body) = delete(&router, &[]).await;
    assert_eq!(
        status, 400,
        "no NetworkInsightsAccessScopeAnalysisId: expected HTTP 400, got {status} with body \
         {body}"
    );
    // The member is `@required`, so the BOUNDARY validator refuses the request before this
    // operation's handler runs — with the framework's own `MalformedInput`, not with a code
    // the handler chose.
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "no NetworkInsightsAccessScopeAnalysisId: expected the boundary validator's \
         MalformedInput, got {body}"
    );
    let remaining = held_ids(&engine);
    assert_eq!(
        remaining,
        vec![held],
        "no NetworkInsightsAccessScopeAnalysisId: a refused delete must remove nothing, got \
         {remaining:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: DryRun
// ---------------------------------------------------------------------------

/// RULE 3: `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and removes
/// nothing, so the analysis is still there and still deletable afterwards; `DryRun=false` is
/// an ordinary delete. One row per tag set.
#[tokio::test]
async fn a_dry_run_reports_the_permitted_outcome_without_deleting() {
    for (case, tags) in TAG_ROWS {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = seed_scope(&engine);
        let analysis_id = start_analysis(case, &router, &scope_id, "a-token", tags).await;

        let (status, body) = delete(
            &router,
            &[
                param("NetworkInsightsAccessScopeAnalysisId", &analysis_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        assert_eq!(
            stored_analyses(&engine),
            1,
            "case {case:?}: a dry run must delete nothing, got {} analyses",
            stored_analyses(&engine)
        );

        // `DryRun=false` is no dry run at all: the delete happens.
        let (status, body) = delete(
            &router,
            &[
                param("NetworkInsightsAccessScopeAnalysisId", &analysis_id),
                param("DryRun", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DryRun=false expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            stored_analyses(&engine),
            0,
            "case {case:?}: DryRun=false must delete the analysis, got {} analyses",
            stored_analyses(&engine)
        );
    }
}

/// RULE 3: a dry run asks whether the request WOULD have worked, so one naming an analysis
/// the account does not hold is refused with THAT reason rather than reported as permitted.
/// The rows are the same forms of unresolvable id the refusal table covers.
#[tokio::test]
async fn a_dry_run_naming_no_analysis_of_the_account_is_refused_with_that_reason() {
    for (case, analysis_id) in unresolvable_ids() {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = seed_scope(&engine);
        start_analysis(case, &router, &scope_id, "a-held-analysis", &[]).await;

        let (status, body) = delete(
            &router,
            &[
                param("NetworkInsightsAccessScopeAnalysisId", &analysis_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE} rather than a permitted dry \
             run, got {body}"
        );
        assert_eq!(
            error_message(&body),
            unresolvable_message(&analysis_id),
            "case {case:?}: expected the refusal to name {analysis_id}, got {body}"
        );
        assert_eq!(
            stored_analyses(&engine),
            1,
            "case {case:?}: a refused dry run must remove nothing, got {} analyses",
            stored_analyses(&engine)
        );
    }
}
