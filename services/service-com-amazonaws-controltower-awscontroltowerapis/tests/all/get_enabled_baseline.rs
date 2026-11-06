// SPDX-License-Identifier: CC0-1.0

//! `GetEnabledBaseline` on its own, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handler's
//! Rust signature.
//!
//! What the sibling suites already pin stays theirs: which members an accepted enablement
//! reports lives in [`crate::enable_baseline_persistence`], what an update makes
//! observable in [`crate::enabled_baseline_update`], the `driftStatusSummary` in
//! [`crate::enabled_baseline_drift`], and the agreement with the listing in
//! [`crate::list_enabled_baselines`]. This suite pins the two things that are the READ's
//! own, each over a TABLE of inputs rather than one case:
//!
//! * an enablement is named by the WHOLE ARN this service minted for it — a value that
//!   differs from a minted ARN in any single character, or carries anything before or
//!   after it, and any other well-formed `Arn` nothing minted (another resource kind's,
//!   another service's, a value sitting exactly ON each published `@length` bound) names
//!   no enablement, so it is the modelled `ResourceNotFoundException` (404) reporting no
//!   `enabledBaselineDetails` rather than a near neighbour's details; while the minted ARN
//!   itself reports details carrying every member the model makes `@required` and no
//!   `parentIdentifier`, because every enablement this service holds is an OU-level one;
//! * a read is REPORTING-ONLY, refused or not: once the clock has made every enablement's
//!   operation due — so no clock-owed transition is still pending and none can be mistaken
//!   for a write — a run of misses does not create, destroy or disturb anything: every
//!   enablement reports afterwards exactly what it reported before, the listing still holds
//!   exactly the enablements that were applied, and the missing ARNs are still missing.
//!
//! Every bound and value set written out by hand here names its source:
//! `enabledBaselineIdentifier` targets `Arn`, `@required` with `@length(min 20, max 2048)`
//! and `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`, bound as `@http POST /get-enabled-baseline`
//! with a 200 success status and the errors `AccessDeniedException` (403),
//! `InternalServerException` (500), `ResourceNotFoundException` (404),
//! `ThrottlingException` (429) and `ValidationException` (400) — from
//! `smithy_query(query="operation", operation="GetEnabledBaseline")`; the `@required`
//! members of the reported details (`arn`, `baselineIdentifier`, `targetIdentifier`,
//! `statusSummary`) and the optional `parentIdentifier` from
//! `smithy_query(query="shape", shape="EnabledBaselineDetails")`; and the
//! `EnablementStatus` value set from `smithy_query(query="shape", shape="EnablementStatus")`
//! — all on this crate's `smithy-model/`. The enablements themselves are the
//! `enablements()` table the sibling `baseline_operations` suite drives `EnableBaseline`
//! with, so no baseline, version, target, parameter list or tag map is written out here,
//! and the shared `member`, `listed_arns`, `arn_of_length` and `one_character_off` helpers
//! are reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::baseline_operations::{enable, enablements, member};
use crate::catalog_identifier_equivalence::listed_arns;
use crate::front_door_validation::arn_of_length;
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};
use crate::list_tags_for_resource::one_character_off;

/// The modelled `EnablementStatus` value set: the only statuses an enablement's
/// `statusSummary` may report.
const ENABLEMENT_STATUSES: [&str; 3] = ["SUCCEEDED", "FAILED", "UNDER_CHANGE"];

/// The `@required` members of `EnabledBaselineDetails` that carry a string.
const REQUIRED_TEXT_MEMBERS: [&str; 3] = ["arn", "baselineIdentifier", "targetIdentifier"];

/// POST `/get-enabled-baseline` for `identifier`.
async fn get(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await
}

/// POST `/list-enabled-baselines` and hand back the `arn` of every listed enablement,
/// sorted, so a case compares SETS without depending on the order they were applied in.
async fn listed_enablement_arns(router: &MultiServiceRouter) -> Vec<String> {
    let listed = call(router, "/list-enabled-baselines", serde_json::json!({})).await;
    assert_eq!(
        listed.status, 200,
        "list-enabled-baselines: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed_arns(&listed, "enabledBaselines")
}

/// Enable every row of the shared table on `router`, handing back each one's case name and
/// the ARN the call minted for it.
async fn applied_enablements(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    let mut applied = Vec::new();
    for row in enablements() {
        let enabled = enable(router, &row).await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            row.case, enabled.status, enabled.body
        );
        applied.push((row.case, enabled.string("arn")));
    }
    applied
}

/// Every well-formed `Arn` this service minted no enablement for: other resource kinds it
/// does mint, another service's resource, and a value sitting exactly on each published
/// `@length` bound and midway between them. All inside `@length(20..2048)` and matching
/// `@pattern`, so the front door passes each one and the handler is what answers it.
fn unminted_arns() -> Vec<(String, String)> {
    vec![
        (
            "a well-formed enabled-baseline ARN nothing minted".to_string(),
            "arn:aws:controltower:us-west-2:000000000000:enabledbaseline/AAAAAAAAAAAAAAAA"
                .to_string(),
        ),
        (
            "an enabled-control ARN, another resource kind of this service".to_string(),
            "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/AAAAAAAAAAAAAAAA"
                .to_string(),
        ),
        (
            "a landing zone ARN, another resource kind of this service".to_string(),
            "arn:aws:controltower:us-west-2:000000000000:landingzone/AAAAAAAAAAAAAAAA".to_string(),
        ),
        (
            "a catalog baseline ARN, which is not an enablement of it".to_string(),
            "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2".to_string(),
        ),
        (
            "an ARN of another AWS service".to_string(),
            "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a".to_string(),
        ),
        (
            "an ARN exactly at the length min of 20".to_string(),
            arn_of_length(20),
        ),
        (
            "an ARN exactly at the length max of 2048".to_string(),
            arn_of_length(2048),
        ),
        (
            "an ARN midway through the length range".to_string(),
            arn_of_length(512),
        ),
    ]
}

/// Every near-miss of `arn` that is still inside `Arn`'s published bounds, so the handler
/// is what refuses each one.
fn near_misses(arn: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "one character of the resource id differs",
            one_character_off(arn),
        ),
        ("a character appended", format!("{arn}x")),
        ("a path segment appended", format!("{arn}/child")),
        ("a colon-separated suffix appended", format!("{arn}:extra")),
        (
            "the final character dropped",
            arn[..arn.len() - 1].to_string(),
        ),
        (
            "a character inserted after the pattern's literal prefix",
            format!("arn:awsx{}", &arn["arn:aws".len()..]),
        ),
    ]
}

/// Assert the details reported for a minted ARN carry what the model requires of them:
/// every `@required` member, a `statusSummary` naming a modelled `EnablementStatus`, and
/// no `parentIdentifier` — every enablement this service holds is itself an OU-level one,
/// so none inherits from a parent.
fn assert_reported_form(case: &str, arn: &str, details: &serde_json::Value) {
    for required in REQUIRED_TEXT_MEMBERS {
        let value = member(details, required);
        assert!(
            value.is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected the required member {required:?} to be reported, got \
             {details:?}"
        );
    }
    assert_eq!(
        member(details, "arn"),
        Some(arn),
        "case {case:?}: expected the enablement to report its own ARN, got {details:?}"
    );

    let summary = details.get("statusSummary").cloned().unwrap_or_default();
    let status = member(&summary, "status").unwrap_or_default().to_string();
    assert!(
        ENABLEMENT_STATUSES.contains(&status.as_str()),
        "case {case:?}: expected a modelled EnablementStatus out of {ENABLEMENT_STATUSES:?}, \
         got {status:?} in {summary:?}"
    );

    assert!(
        details.get("parentIdentifier").is_none_or(serde_json::Value::is_null),
        "case {case:?}: an OU-level enablement inherits from no parent, so no \
         parentIdentifier may be reported, got {details:?}"
    );
}

/// RULE: an enablement is named by the WHOLE ARN this service minted for it — anything
/// else that is a well-formed `Arn` names no enablement and is the modelled
/// `ResourceNotFoundException` (404) carrying no details, never a near neighbour's — while
/// the minted ARN reports details carrying every member the model requires and no
/// `parentIdentifier`.
#[tokio::test]
async fn only_the_exact_minted_arn_names_an_enablement() {
    let router = set_up_emulator().await;
    let applied = applied_enablements(&router).await;
    let minted: Vec<String> = applied.iter().map(|(_, arn)| arn.clone()).collect();

    // The minted ARN resolves, and the details it reports have the modelled form.
    for (case, arn) in &applied {
        let read = get(&router, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the minted ARN {arn:?} to resolve, got {} body {:?}",
            read.status, read.body
        );
        assert_reported_form(case, arn, read.get("enabledBaselineDetails"));
    }

    // Every near miss of a minted ARN, and every well-formed ARN nothing minted, is the
    // handler's own 404 with no details in the body.
    let mut misses: Vec<(String, String)> = unminted_arns();
    for (case, arn) in &applied {
        for (miss_case, mutant) in near_misses(arn) {
            assert!(
                !minted.contains(&mutant),
                "case {case:?}/{miss_case:?}: the near miss {mutant:?} must name no \
                 enablement, but it is one of the minted ARNs"
            );
            misses.push((format!("{case}: {miss_case}"), mutant));
        }
    }

    for (case, arn) in &misses {
        let refused = get(&router, arn).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?} ({arn:?}): expected 404 ResourceNotFoundException, got {:?} body \
             {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused
                .body
                .get("enabledBaselineDetails")
                .is_none_or(serde_json::Value::is_null),
            "case {case:?}: a refusal must report no enablement, got {:?}",
            refused.body
        );
    }
}

/// RULE (the other side of the same read): the operation only REPORTS. A run of reads that
/// name no enablement creates nothing and disturbs nothing — every enablement reports
/// afterwards exactly what it reported before, the listing still holds exactly the
/// enablements that were applied, and the missing ARNs are still missing.
#[tokio::test]
async fn a_read_that_names_no_enablement_changes_nothing_a_later_caller_sees() {
    let router = set_up_emulator().await;
    let applied = applied_enablements(&router).await;

    // Let the clock make every enablement's operation due FIRST, so each one has reached
    // its terminal status and the clock owes it nothing further: what this case isolates is
    // whether a MISSED read writes, not the settling a sibling suite already pins. Nothing
    // sleeps server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    let before: Vec<serde_json::Value> = {
        let mut details = Vec::new();
        for (case, arn) in &applied {
            let read = get(&router, arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the minted ARN to resolve, got {} body {:?}",
                read.status, read.body
            );
            details.push(read.get("enabledBaselineDetails").clone());
        }
        details
    };
    let listed_before = listed_enablement_arns(&router).await;
    let mut expected_listing: Vec<String> = applied.iter().map(|(_, arn)| arn.clone()).collect();
    expected_listing.sort();
    assert_eq!(
        listed_before, expected_listing,
        "expected the listing to hold exactly the enablements that were applied, got \
         {listed_before:?}"
    );

    // Miss on every unminted ARN, twice each: repeating a miss must not make the second
    // one answer differently either.
    let misses = unminted_arns();
    for round in 0..2 {
        for (case, arn) in &misses {
            let refused = get(&router, arn).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}, round {round}: expected 404 ResourceNotFoundException, got \
                 {:?} body {:?}",
                (refused.status, refused.error_code.as_deref()),
                refused.body
            );
        }
    }

    // Nothing the misses touched changed: the enablements report the same details, and the
    // listing holds the same set.
    for ((case, arn), expected) in applied.iter().zip(&before) {
        let read = get(&router, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the enablement to still resolve, got {} body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.get("enabledBaselineDetails"),
            expected,
            "case {case:?}: expected the same details after a run of missed reads, got {:?}",
            read.body
        );
    }
    let listed_after = listed_enablement_arns(&router).await;
    assert_eq!(
        listed_after, expected_listing,
        "expected the missed reads to add no enablement and remove none, got \
         {listed_after:?}"
    );
}
