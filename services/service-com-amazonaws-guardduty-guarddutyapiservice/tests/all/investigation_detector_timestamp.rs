// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a detector's own `UpdatedAt` records when an
//! investigation is initiated on it.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over a SET of creates rather than the one a recorded
//! call made: a call that changes WHICH sub-resources a detector holds changes the
//! detector record itself, so `GetDetector`'s `UpdatedAt` — "the last-updated
//! timestamp for the detector" (`smithy_query(operation="GetDetector")`) — moves to
//! the instant of that call, while a call that leaves that set alone moves it nowhere,
//! whether it persisted nothing (a refusal, an idempotent replay), changed only one
//! sub-resource's own contents (the clock settling an investigation's status) or
//! changed nothing at all (a read). `detector_sub_resource_creates` and
//! `filter_detector_timestamp` assert the same rule over the detector's other
//! sub-resource families; this suite covers the investigation family, whose create
//! answers the recorded 200 with a minted `InvestigationId` and which has no delete.
//!
//! The rows differ along what the rule generalises over — the SIZE and ALPHABET of the
//! `TriggerPrompt` (`len 1..2048`, no pattern), whether a `ClientToken` (`len 0..64`,
//! `@idempotencyToken`) is carried, and which published bound the refused row steps
//! past (member catalogue and bounds from
//! `smithy_query(operation="CreateInvestigation")`).
//!
//! `UpdatedAt` is an ISO-8601 string with a fixed-width millisecond field
//! (`smithy_query(operation="GetDetector")` models it as a String), so these
//! assertions order the instants as text, which is all a client has.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, call_in, enabled_detector, new_router, Router};
// The detector's own `UpdatedAt` is read through the helper the other timestamp suites
// already share, so every suite sees that instant the same way.
use crate::refused_create_clock::detector_updated_at;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    INVESTIGATION_REGIONS, INVESTIGATION_TRANSITION_MILLIS,
};

/// A Region GuardDuty serves that is NOT in the documented Investigation catalogue,
/// so a create addressed there is refused and persists nothing. Asserted below to be
/// outside [`INVESTIGATION_REGIONS`], so this row cannot drift from the catalogue.
const REGION_WITHOUT_INVESTIGATIONS: &str = "us-west-1";

/// One `CreateInvestigation` request, with the refusal that brackets it.
struct Row {
    case: &'static str,
    /// A create this service accepts.
    create: Value,
    /// The same call with ONE member outside a published bound, so the request is
    /// refused and no investigation is persisted.
    refused: Value,
}

/// The rows, spanning the prompts and tokens a create can carry.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest prompt the model accepts, no token",
            create: json!({"triggerPrompt": "I"}),
            // `TriggerPrompt` is `len 1..2048`, so the empty prompt is outside it.
            refused: json!({"triggerPrompt": ""}),
        },
        Row {
            case: "the prompt form the member's own documentation gives, with a token",
            create: json!({
                "triggerPrompt":
                    "Investigate finding 1ab2c3d4e5f6a7b8c9d0e1f2a3b4c5d6 in account 000000000000",
                "clientToken": "investigation-token-one",
            }),
            // `TriggerPrompt` is required, so a body without it is refused.
            refused: json!({"clientToken": "investigation-token-refused"}),
        },
        Row {
            case: "a 2048-character prompt, the top of the modelled range, with a 64-character \
                   token",
            create: json!({
                "triggerPrompt": "p".repeat(2048),
                "clientToken": "t".repeat(64),
            }),
            // One character past the top of the `TriggerPrompt` range.
            refused: json!({"triggerPrompt": "p".repeat(2049)}),
        },
        Row {
            case: "a non-ASCII prompt carrying newlines, no token",
            create: json!({"triggerPrompt": "Analysiere Funde\nim Konto 000000000000\t— bitte"}),
            // `ClientToken` is `len 0..64`, so a 65-character token is outside it.
            refused: json!({
                "triggerPrompt": "analyze findings in my organization",
                "clientToken": "t".repeat(65),
            }),
        },
    ]
}

/// Long enough for the clock to owe an investigation its terminal status, plus slack.
fn past_the_transition() -> std::time::Duration {
    std::time::Duration::from_millis(INVESTIGATION_TRANSITION_MILLIS as u64 + 250)
}

/// `GetDetector`, so an assertion can see the detector's own timestamps.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// The minted `InvestigationId` a create returned, failing the test if the create did
/// not succeed or did not name one. The recorded success status is 200.
fn minted_id(case: &str, status: u16, got: &Value) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to answer 200, got {status} {got:?}"
    );
    let id = got["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigationId, got {got:?}"));
    // `InvestigationId` is `len 1..64, pattern ^[a-fA-F0-9\-]+$`.
    assert!(
        !id.is_empty()
            && id.len() <= 64
            && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case:?}: expected a modelled investigationId, got {id:?}"
    );
    id.to_string()
}

// ---------------------------------------------------------------------------
// The rule: initiating an investigation moves the detector's UpdatedAt; a call that
// persists nothing, and a read, move it nowhere.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initiating_an_investigation_moves_the_detectors_updated_at() {
    assert!(
        !INVESTIGATION_REGIONS.contains(&REGION_WITHOUT_INVESTIGATIONS),
        "the refused-Region row must name a Region outside the documented catalogue, got \
         {REGION_WITHOUT_INVESTIGATIONS:?} in {INVESTIGATION_REGIONS:?}"
    );

    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass on another row's
        // write.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let collection = format!("/detector/{detector_id}/investigation");

        let before = read_detector(&router, &detector_id, case).await;
        let created_at = before["createdAt"].clone();
        let updated_before = before["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {before:?}"))
            .to_string();

        // A refused create persists nothing, so the detector is unchanged by it.
        let (status, refusal) = call(&router, "POST", &collection, Some(row.refused.clone())).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a member outside its published bound must be refused, got {status} \
             {refusal:?}"
        );
        let after_refusal = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_refusal, updated_before,
            "case {case:?}: a refused create must not move the detector's updatedAt, expected \
             {updated_before:?}, got {after_refusal:?}"
        );

        // A create the Region does not serve persists nothing either.
        let (status, unserved) = call_in(
            &router,
            REGION_WITHOUT_INVESTIGATIONS,
            "POST",
            &collection,
            Some(row.create.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a create outside the Investigation Region catalogue must be refused, \
             got {status} {unserved:?}"
        );
        let after_unserved = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_unserved, updated_before,
            "case {case:?}: a create the Region does not serve must not move the detector's \
             updatedAt, expected {updated_before:?}, got {after_unserved:?}"
        );

        // A read is not a change either.
        let after_read = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_read, updated_before,
            "case {case:?}: reading the detector must not move its updatedAt, expected \
             {updated_before:?}, got {after_read:?}"
        );

        // The accepted create: the detector now holds one investigation more than it
        // did, so its UpdatedAt moves.
        let (status, created) = call(&router, "POST", &collection, Some(row.create.clone())).await;
        let id = minted_id(case, status, &created);

        let after_create = read_detector(&router, &detector_id, case).await;
        let updated_after_create = after_create["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {after_create:?}"))
            .to_string();
        assert!(
            updated_after_create > updated_before,
            "case {case:?}: the create changed the detector, so its updatedAt must move past \
             {updated_before:?}, got {updated_after_create:?}"
        );
        assert_eq!(
            after_create["createdAt"], created_at,
            "case {case:?}: initiating an investigation must not move the detector's createdAt, \
             expected {created_at:?}, got {:?}",
            after_create["createdAt"]
        );

        // Round trip: the id the create minted is an investigation a read resolves
        // carrying the prompt it was initiated with, so the moved UpdatedAt records a
        // change that really landed in the store.
        let (status, read_back) = call(&router, "GET", &format!("{collection}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the minted investigation to answer 200, got \
             {status} {read_back:?}"
        );
        assert_eq!(
            read_back["investigation"]["investigationId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the minted investigation to read back under its id, got \
             {read_back:?}"
        );
        assert_eq!(
            read_back["investigation"]["triggerPrompt"],
            row.create["triggerPrompt"],
            "case {case:?}: expected the prompt the create stored, got {read_back:?}"
        );

        // A second create of the same body persists nothing when it replays the row's
        // token: the detector holds the same investigations, so nothing moves. A row
        // without a token initiates another one, which moves it again.
        let (status, again) = call(&router, "POST", &collection, Some(row.create.clone())).await;
        let again_id = minted_id(case, status, &again);
        let after_again = detector_updated_at(&router, &detector_id, case).await;
        if row.create.get("clientToken").is_some() {
            assert_eq!(
                again_id, id,
                "case {case:?}: a replayed clientToken must return the first call's id, expected \
                 {id:?}, got {again_id:?}"
            );
            assert_eq!(
                after_again, updated_after_create,
                "case {case:?}: a replayed create initiates nothing, so the detector's updatedAt \
                 must stay at {updated_after_create:?}, got {after_again:?}"
            );
        } else {
            assert_ne!(
                again_id, id,
                "case {case:?}: a create without a clientToken must initiate another \
                 investigation, got {again_id:?} twice"
            );
            assert!(
                after_again > updated_after_create,
                "case {case:?}: the second create changed the detector too, so its updatedAt must \
                 move past {updated_after_create:?}, got {after_again:?}"
            );
        }

        // The clock settling the investigation's own status changes the investigation,
        // not WHICH sub-resources the detector holds, so the detector's own UpdatedAt
        // stays where the last create put it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, settled) = call(&router, "GET", &format!("{collection}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read past the transition to answer 200, got {status} \
             {settled:?}"
        );
        let settled_status = settled["investigation"]["status"].as_str().unwrap_or_default();
        assert_ne!(
            settled_status, "RUNNING",
            "case {case:?}: past the transition the investigation must have settled, got \
             {settled:?}"
        );
        let after_settle = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_settle, after_again,
            "case {case:?}: the detector holds the same investigations after one settles, so its \
             updatedAt must stay at {after_again:?}, got {after_settle:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every change to the set moves the instant again, so the detector's UpdatedAt names
// the LAST such change and not the first.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_investigation_initiated_on_one_detector_moves_the_instant_again() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let collection = format!("/detector/{detector_id}/investigation");

    let mut previous = detector_updated_at(&router, &detector_id, "the fresh detector").await;
    let mut seen: Vec<String> = Vec::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // The rows carry tokens of their own, so each create here initiates a fresh
        // investigation rather than replaying the row before it.
        let mut body = row.create.clone();
        body["clientToken"] = json!(format!("one-detector-{index}"));
        let (status, created) = call(&router, "POST", &collection, Some(body)).await;
        let id = minted_id(case, status, &created);
        assert!(
            !seen.contains(&id),
            "case {case:?}: each create must mint its own investigationId, got {id:?} again"
        );
        seen.push(id);

        let now = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            now > previous,
            "case {case:?}: each investigation initiated on the detector must move its updatedAt \
             past {previous:?}, got {now:?}"
        );
        previous = now;
    }

    // Listing them is a report: it changes nothing, so it moves nothing.
    let (status, listed) = call(
        &router,
        "POST",
        &format!("{collection}/list"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "list-investigations: expected 200, got {status} {listed:?}"
    );
    let listed_ids: Vec<String> = listed["investigations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|summary| summary["investigationId"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        listed_ids.len(),
        seen.len(),
        "list-investigations: expected the {} investigations the creates persisted, got \
         {listed_ids:?}",
        seen.len()
    );
    let after_list = detector_updated_at(&router, &detector_id, "after the list").await;
    assert_eq!(
        after_list, previous,
        "list-investigations changes nothing, so the detector's updatedAt must stay at \
         {previous:?}, got {after_list:?}"
    );
}
