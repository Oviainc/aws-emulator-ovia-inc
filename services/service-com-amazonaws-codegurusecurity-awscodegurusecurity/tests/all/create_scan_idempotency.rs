// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a `CreateScan` `clientToken` stands for.
//!
//! RULE: an `@idempotencyToken` identifies ONE request, so a call carrying a token
//! already seen is answered from that token's record only when the request it carries is
//! the same request (every modelled input, with the service's documented defaults
//! applied, identical); a call carrying the token with any other request is two requests
//! sharing one token and is refused with the operation's modelled `ConflictException`
//! (HTTP 409), writing nothing. The record stands only inside a bounded window: once the
//! clock has closed it the token identifies nothing and the next call carrying it is a
//! fresh request.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: WHICH
//! modelled input the second call changes (`resourceId.codeArtifactId`, `scanType`,
//! `analysisType`, `tags`, `scanName`, and several at once), the FORM the same request may
//! be spelled in (a default left out or named explicitly, an absent tag map or an empty
//! one), and WHERE the second call falls relative to the close of the token's window. Each
//! case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), so what is asserted is the wire behaviour a
//! `codeguru-security` SDK client sees.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    self as cgs_state, ANALYSIS_TYPE_SECURITY, CLIENT_TOKEN_WINDOW_MILLIS, SCAN_STATE_FAILED,
    SCAN_STATE_IN_PROGRESS, SCAN_TYPE_EXPRESS, SCAN_TYPE_STANDARD, STANDARD_SCAN_SETTLE_MILLIS,
};

use crate::resource_tagging::list_tags;
use crate::scan_lifecycle::{artifact_for, call, create_scan, harness, store_of, Answer};

/// The one token every row of a table shares.
const TOKEN: &str = "fixed-token";

/// A `Uuid`-shaped code artifact id this emulator never minted, for the row whose second
/// call names a resource that does not exist.
const UNMINTED_ARTIFACT: &str = "0123dead-beef-4000-8000-feed0badc0de";

/// `GET /scans/{scanName}`.
async fn get_scan(router: &MultiServiceRouter, scan_name: &str) -> Answer {
    call(router, "GET", &format!("/scans/{scan_name}"), None).await
}

/// Wait past the clock-owed transition of a Standard run, so a request that the rule says
/// starts a revision is not refused by the one-run-at-a-time rule instead.
async fn wait_for_settle() {
    let span = u64::try_from(STANDARD_SCAN_SETTLE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span + 150)).await;
}

/// The `scanType` the store recorded for the latest run of `scan_name` — `scanType` is an
/// input of `CreateScan` that no response of this service echoes back, so the stored run
/// is where what was asked for is observed.
fn recorded_scan_type(engine: &Arc<engine::InMemoryEngine>, scan_name: &str) -> String {
    let store = store_of(engine);
    let guard = store.lock();
    guard
        .scans
        .get(scan_name)
        .and_then(|scan| scan.latest_run())
        .map(|run| run.scan_type.clone())
        .unwrap_or_else(|| panic!("scan {scan_name} must hold a run"))
}

/// Close the window of `token`'s record `offset_millis` relative to now, as the clock
/// would have closed it. Written through the store's own recording path, so the ledger and
/// its due-queue stay in step exactly as a request would leave them.
fn move_token_window(engine: &Arc<engine::InMemoryEngine>, token: &str, offset_millis: i64) {
    let now = cgs_state::now_millis();
    let store = store_of(engine);
    let mut guard = store.lock();
    let mut record = guard
        .idempotency
        .get(token)
        .cloned()
        .unwrap_or_else(|| panic!("the ledger must hold a record for {token}"));
    record.expires_at_millis = now.saturating_add(offset_millis);
    record.recorded_at_millis = record
        .expires_at_millis
        .saturating_sub(CLIENT_TOKEN_WINDOW_MILLIS);
    guard.remember_client_token(record);
}

/// Which code artifact a row's second call names.
#[derive(Debug)]
enum Artifact {
    /// The artifact the first call named — the same resource.
    Original,
    /// A brand-new artifact minted for the same scan name: a revised resource.
    Fresh,
    /// A `Uuid`-shaped id this emulator never minted, so it names no resource at all.
    Unminted,
    /// A fresh artifact minted for the scan name the row's body names instead.
    ForNamedScan,
}

/// One row: the body the second call sends under the shared token (its `resourceId` comes
/// from `artifact`), and the case name.
#[derive(Debug)]
struct ReplayCase {
    case: &'static str,
    body: Value,
    artifact: Artifact,
}

/// The `codeArtifactId` a row names, minting one where the row asks for a new resource.
async fn artifact_of(router: &MultiServiceRouter, row: &ReplayCase, original: &str) -> String {
    match row.artifact {
        Artifact::Original => original.to_string(),
        Artifact::Fresh => {
            let scan_name = row.body["scanName"].as_str().expect("the row names a scan");
            artifact_for(router, scan_name).await
        }
        Artifact::Unminted => UNMINTED_ARTIFACT.to_string(),
        Artifact::ForNamedScan => {
            let scan_name = row.body["scanName"].as_str().expect("the row names a scan");
            artifact_for(router, scan_name).await
        }
    }
}

/// The scan name the tables below start their first run under.
const BASE_SCAN: &str = "idem-scan";

/// The body of the first call: no `scanType`, no `analysisType`, no `tags`, so the service
/// resolves each to its documented default.
fn first_body() -> Value {
    json!({ "scanName": BASE_SCAN, "clientToken": TOKEN })
}

/// The rows that ARE the first request again, each spelling it in another admissible form.
/// `scanType` defaults to `Standard` and `analysisType` to `Security`
/// (`smithy_query(query="operation",operation="CreateScan")` members' docs: "Defaults to
/// Standard type if missing" / "Defaults to Security type if missing"), and a `tags` map
/// that names nothing asks for the same tags as no map at all.
fn retry_cases() -> Vec<ReplayCase> {
    vec![
        ReplayCase {
            case: "the identical request again",
            body: first_body(),
            artifact: Artifact::Original,
        },
        ReplayCase {
            case: "the same request naming the defaulted scanType explicitly",
            body: json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "scanType": SCAN_TYPE_STANDARD,
            }),
            artifact: Artifact::Original,
        },
        ReplayCase {
            case: "the same request naming the defaulted analysisType explicitly",
            body: json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "analysisType": ANALYSIS_TYPE_SECURITY,
            }),
            artifact: Artifact::Original,
        },
        ReplayCase {
            case: "the same request naming both defaults explicitly",
            body: json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "scanType": SCAN_TYPE_STANDARD,
                "analysisType": ANALYSIS_TYPE_SECURITY,
            }),
            artifact: Artifact::Original,
        },
        ReplayCase {
            case: "the same request carrying an empty tag map",
            body: json!({ "scanName": BASE_SCAN, "clientToken": TOKEN, "tags": {} }),
            artifact: Artifact::Original,
        },
    ]
}

/// The rows that are a DIFFERENT request under the same token, one per modelled input the
/// rule generalises over, plus one that changes several at once.
fn mismatch_cases() -> Vec<ReplayCase> {
    vec![
        ReplayCase {
            case: "a brand-new code artifact for the same scan",
            body: first_body(),
            artifact: Artifact::Fresh,
        },
        ReplayCase {
            case: "a code artifact this service never minted",
            body: first_body(),
            artifact: Artifact::Unminted,
        },
        ReplayCase {
            case: "scanType Express instead of the defaulted Standard",
            body: json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "scanType": SCAN_TYPE_EXPRESS,
            }),
            artifact: Artifact::Original,
        },
        ReplayCase {
            case: "analysisType All instead of the defaulted Security",
            body: json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "analysisType": "All",
            }),
            artifact: Artifact::Original,
        },
        ReplayCase {
            case: "a tag map the first request did not carry",
            body: json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "tags": { "Env": "prod" },
            }),
            artifact: Artifact::Original,
        },
        ReplayCase {
            case: "another scanName entirely",
            body: json!({ "scanName": "other-idem-scan", "clientToken": TOKEN }),
            artifact: Artifact::ForNamedScan,
        },
        ReplayCase {
            case: "a new artifact, Express, All and tags all at once",
            body: json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "scanType": SCAN_TYPE_EXPRESS,
                "analysisType": "All",
                "tags": { "Env": "prod" },
            }),
            artifact: Artifact::Fresh,
        },
    ]
}

/// RULE, the retry side: a token replayed with the SAME request answers with the run the
/// first call started, starts no revision and reports that run's CURRENT state.
#[tokio::test]
async fn a_token_replayed_with_the_same_request_answers_with_its_run() {
    let (_engine, router) = harness();
    let artifact = artifact_for(&router, BASE_SCAN).await;

    let first = create_scan(&router, first_body(), &artifact).await;
    assert_eq!(
        first.status, 200,
        "the first CreateScan: expected 200, got {first:?}"
    );
    let run_id = first.str_of("runId");

    for row in retry_cases() {
        let replay = create_scan(&router, row.body.clone(), &artifact).await;
        assert_eq!(
            replay.status, 200,
            "case {:?}: expected 200, got {replay:?}",
            row.case
        );
        assert_eq!(
            replay.str_of("runId"),
            run_id,
            "case {:?}: expected the original run {run_id}, got {replay:?}",
            row.case
        );
        assert_eq!(
            replay.get("resourceId")["codeArtifactId"].as_str(),
            Some(artifact.as_str()),
            "case {:?}: expected the replay to echo artifact {artifact}, got {replay:?}",
            row.case
        );
        assert_eq!(
            replay.str_of("scanState"),
            SCAN_STATE_IN_PROGRESS,
            "case {:?}: the original run has not settled yet, got {replay:?}",
            row.case
        );

        let read = get_scan(&router, BASE_SCAN).await;
        assert_eq!(
            read.get("numberOfRevisions").as_i64(),
            Some(0),
            "case {:?}: a retry must start no revision, got {read:?}",
            row.case
        );
    }

    // The replay reports what the run IS, not a frozen copy of the first answer: once the
    // clock has settled the run, the same retry answers its settled state.
    wait_for_settle().await;
    let settled = create_scan(&router, first_body(), &artifact).await;
    assert_eq!(
        settled.status, 200,
        "the retry of a settled run: expected 200, got {settled:?}"
    );
    assert_eq!(
        settled.str_of("runId"),
        run_id,
        "the retry of a settled run: expected the original run {run_id}, got {settled:?}"
    );
    assert_eq!(
        settled.str_of("scanState"),
        SCAN_STATE_FAILED,
        "the retry of a settled run: expected the run's current state, got {settled:?}"
    );
}

/// RULE, the conflict side: the same token carrying ANY other request is refused with the
/// modelled `ConflictException` (HTTP 409) and changes nothing — no run is recorded, the
/// scan keeps its own `analysisType` and `scanType`, and none of the refused request's tags
/// is stored.
#[tokio::test]
async fn a_token_carrying_a_different_request_is_a_conflict_and_writes_nothing() {
    let (engine, router) = harness();
    let artifact = artifact_for(&router, BASE_SCAN).await;

    let first = create_scan(&router, first_body(), &artifact).await;
    assert_eq!(
        first.status, 200,
        "the first CreateScan: expected 200, got {first:?}"
    );
    let run_id = first.str_of("runId");
    let arn = first.str_of("scanNameArn");

    // Past the first run's clock-owed transition, so a request the rule accepted would
    // have started a revision: the refusal below is the token's, not the one-run-at-a-time
    // rule's.
    wait_for_settle().await;

    for row in mismatch_cases() {
        let named = artifact_of(&router, &row, &artifact).await;
        let refused = create_scan(&router, row.body.clone(), &named).await;
        assert_eq!(
            refused.status, 409,
            "case {:?}: expected HTTP 409, got {refused:?}",
            row.case
        );
        assert!(
            refused.error_type.contains("ConflictException"),
            "case {:?}: expected error code ConflictException, got {refused:?}",
            row.case
        );
        assert!(
            refused
                .get("message")
                .as_str()
                .is_some_and(|message| message.contains(TOKEN)),
            "case {:?}: the refusal must name the client token that identifies the other \
             request, got {refused:?}",
            row.case
        );

        let read = get_scan(&router, BASE_SCAN).await;
        assert_eq!(
            read.str_of("runId"),
            run_id,
            "case {:?}: the scan must still report the first run, got {read:?}",
            row.case
        );
        assert_eq!(
            read.get("numberOfRevisions").as_i64(),
            Some(0),
            "case {:?}: a refused request must start no revision, got {read:?}",
            row.case
        );
        assert_eq!(
            read.str_of("analysisType"),
            ANALYSIS_TYPE_SECURITY,
            "case {:?}: the scan keeps the analysisType its own run was started with, got \
             {read:?}",
            row.case
        );
        assert_eq!(
            recorded_scan_type(&engine, BASE_SCAN),
            SCAN_TYPE_STANDARD,
            "case {:?}: the scan keeps the scanType its own run was started with",
            row.case
        );

        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            tags.get("tags"),
            &json!({}),
            "case {:?}: none of a refused request's tags may be stored, got {tags:?}",
            row.case
        );
    }

    // The scan the "another scanName" row named was never created either.
    let other = get_scan(&router, "other-idem-scan").await;
    assert_eq!(
        other.status, 404,
        "a refused request must create no scan under the name it carried, got {other:?}"
    );
}

/// One row of the window table: where the close of the token's record falls relative to
/// the second call, and whether the token still identifies its request then.
#[derive(Debug)]
struct WindowCase {
    case: &'static str,
    /// Offset from now of the instant the record stops standing for its request.
    offset_millis: i64,
    open: bool,
}

/// The rows: a window with time left to run, the instant it closes, one millisecond past
/// it and a whole window past it. The window's length is this emulator's own
/// `CLIENT_TOKEN_WINDOW_MILLIS`, so the rows are expressed relative to its close rather
/// than by waiting it out.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            case: "a minute of the window still to run",
            offset_millis: 60_000,
            open: true,
        },
        WindowCase {
            case: "the instant the window closes",
            offset_millis: 0,
            open: false,
        },
        WindowCase {
            case: "one millisecond past the window",
            offset_millis: -1,
            open: false,
        },
        WindowCase {
            case: "a whole window past its close",
            offset_millis: -CLIENT_TOKEN_WINDOW_MILLIS,
            open: false,
        },
    ]
}

/// RULE, the window side: a token identifies its request only while its record stands.
/// Inside the window a different request under it conflicts; once the clock has closed the
/// window the token identifies nothing, so the very same call is a FRESH request — it
/// starts a revision, and every input it carries is the input that revision runs with.
#[tokio::test]
async fn a_token_identifies_its_request_only_inside_its_window() {
    for row in window_cases() {
        let (engine, router) = harness();
        let artifact = artifact_for(&router, BASE_SCAN).await;
        let first = create_scan(&router, first_body(), &artifact).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first CreateScan expected 200, got {first:?}",
            row.case
        );
        let run_id = first.str_of("runId");
        let arn = first.str_of("scanNameArn");

        move_token_window(&engine, TOKEN, row.offset_millis);
        wait_for_settle().await;

        // The same token with a different request: Express/All on a revised resource,
        // carrying a tag the scan does not hold.
        let revised = artifact_for(&router, BASE_SCAN).await;
        let second = create_scan(
            &router,
            json!({
                "scanName": BASE_SCAN,
                "clientToken": TOKEN,
                "scanType": SCAN_TYPE_EXPRESS,
                "analysisType": "All",
                "tags": { "Env": "prod" },
            }),
            &revised,
        )
        .await;

        if row.open {
            assert_eq!(
                second.status, 409,
                "case {:?}: inside the window a different request conflicts, got {second:?}",
                row.case
            );
            let read = get_scan(&router, BASE_SCAN).await;
            assert_eq!(
                read.get("numberOfRevisions").as_i64(),
                Some(0),
                "case {:?}: the refusal must start no revision, got {read:?}",
                row.case
            );
            continue;
        }

        assert_eq!(
            second.status, 200,
            "case {:?}: past its window the token is a fresh request, got {second:?}",
            row.case
        );
        let second_run = second.str_of("runId");
        assert_ne!(
            second_run, run_id,
            "case {:?}: a fresh request must start a new run, got {second_run} twice",
            row.case
        );
        assert_eq!(
            second.get("resourceId")["codeArtifactId"].as_str(),
            Some(revised.as_str()),
            "case {:?}: the answer must name the artifact THIS request asked for, got \
             {second:?}",
            row.case
        );

        let read = get_scan(&router, BASE_SCAN).await;
        assert_eq!(
            read.get("numberOfRevisions").as_i64(),
            Some(1),
            "case {:?}: the fresh request must add a revision, got {read:?}",
            row.case
        );
        assert_eq!(
            read.str_of("analysisType"),
            "All",
            "case {:?}: the revision must run with the analysisType it asked for, got {read:?}",
            row.case
        );
        assert_eq!(
            recorded_scan_type(&engine, BASE_SCAN),
            SCAN_TYPE_EXPRESS,
            "case {:?}: the revision must run with the scanType it asked for",
            row.case
        );
        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            tags.get("tags"),
            &json!({ "Env": "prod" }),
            "case {:?}: the tags the fresh request carried must be stored, got {tags:?}",
            row.case
        );

        // The closed record was forgotten rather than kept beside the new one: the ledger
        // holds exactly the record of the request just accepted.
        let store = store_of(&engine);
        let guard = store.lock();
        assert_eq!(
            guard.idempotency.len(),
            1,
            "case {:?}: expected one live token record, got {:?}",
            row.case,
            guard.idempotency
        );
        assert_eq!(
            guard.idempotency.get(TOKEN).map(|record| &record.run_id),
            Some(&second_run),
            "case {:?}: the token must now identify the request just accepted, got {:?}",
            row.case,
            guard.idempotency
        );
        assert_eq!(
            guard.idempotency_expiry.len(),
            1,
            "case {:?}: expected one entry on the expiry due-queue, got {:?}",
            row.case,
            guard.idempotency_expiry
        );
    }
}
