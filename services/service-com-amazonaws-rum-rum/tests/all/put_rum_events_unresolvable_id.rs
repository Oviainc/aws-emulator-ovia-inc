// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how CloudWatch RUM's data-plane write answers an app-monitor
//! `Id` it cannot resolve.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` the monitor is addressed by, the
//! modelled constraints, the handler and the error envelope are all exercised rather than
//! a handler body.
//!
//! The RULE the assertions pin, over a SET of inputs rather than one request:
//! `PutRumEvents` is authorized against the app monitor its `Id` names, so an `Id` that
//! resolves to no stored monitor leaves no resource for the grant to be established
//! against and the write is refused with `AccessDeniedException` (HTTP 403), whose body
//! describes the caller's missing permission and nothing about the resource — the same
//! answer whether the id was never minted or named a monitor that has since been deleted,
//! so nothing about existence is disclosed.
//!
//! The rows differ along what the rule generalises over: which hex digits the well-formed
//! id carries and in which letter case, and how the id came to resolve to nothing (never
//! minted, deleted while other monitors remain, deleted and its name re-created under a
//! NEW id). The contrast rows — the ids the store does hold — are what show the refusal is
//! driven by stored state rather than by the request's shape.

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::CALLER_LACKS_PERMISSIONS_MESSAGE;
use service_com_amazonaws_rum_rum::ComAmazonawsRumRumState;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator, message, rum_events_body};

/// Well-formed `AppMonitorId` values that no create in these tests mints.
///
/// Source: `smithy_query(query="shape", shape="AppMonitorId")` — `@length(min: 36, max:
/// 36)` and `@pattern
/// ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`. Each
/// row satisfies that pattern — so the request reaches the handler rather than being
/// refused at the boundary — and the rows walk the digit classes and both letter cases the
/// pattern admits.
const NEVER_MINTED_IDS: [(&str, &str); 5] = [
    ("all-zero", "00000000-0000-4000-8000-000000000000"),
    ("all-f-lowercase", "ffffffff-ffff-4fff-bfff-ffffffffffff"),
    ("all-f-uppercase", "FFFFFFFF-FFFF-4FFF-BFFF-FFFFFFFFFFFF"),
    ("mixed-case-hex", "DeAdBeEf-1234-4abc-89De-0123456789aB"),
    ("digits-only", "12345678-9012-4345-8678-901234567890"),
];

/// Assert one `PutRumEvents` call is refused by the rule, returning the error body so a
/// case can compare two refusals with each other.
async fn assert_refused(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(id)),
    )
    .await;
    assert_eq!(
        reply.status,
        403,
        "case {case:?}: expected HTTP 403, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let code = reply.error_code(case);
    assert_eq!(
        code, "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {code:?}"
    );
    let document = reply.json(case);
    assert_eq!(
        message(&document),
        Some(CALLER_LACKS_PERMISSIONS_MESSAGE),
        "case {case:?}: expected the message {CALLER_LACKS_PERMISSIONS_MESSAGE:?}, got {:?}",
        message(&document)
    );
    // Nothing of the request is in the body: an id the caller could probe with must not
    // come back, and neither must the action or the ARN the decision would have been made
    // about.
    let rendered = document.to_string();
    assert!(
        !rendered.contains(id),
        "case {case:?}: the refusal must not report the id it was asked about, got \
         {rendered:?}"
    );
    document
}

/// Assert one `PutRumEvents` call is ACCEPTED — the contrast the refusal is read against.
async fn assert_accepted(router: &MultiServiceRouter, case: &str, id: &str) {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(id)),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

#[tokio::test]
async fn an_unresolvable_id_is_refused_as_an_authorization_failure() {
    let (router, engine) = emulator();
    // One monitor exists throughout, so a refusal means "that id names no monitor" rather
    // than "this emulator holds nothing", and the accepted call below shows the same
    // handler path admits an id the store does hold.
    let live = create(
        &router,
        "seed",
        json!({"Name": "events-present", "Domain": "example.com"}),
    )
    .await;

    for (case, id) in NEVER_MINTED_IDS {
        assert_ne!(
            id, live,
            "case {case:?}: the row must not be the id the seed minted"
        );
        assert_refused(&router, case, id).await;
    }
    assert_accepted(&router, "the id the seed minted", &live).await;

    // The refused calls wrote nothing: the only stored monitor carries just the two events
    // of the one accepted batch, and no monitor was invented for an id that named none.
    let state = engine.state::<ComAmazonawsRumRumState>();
    let store = state.lock();
    assert_eq!(
        store.app_monitors.len(),
        1,
        "case \"store after refusals\": expected the one seeded monitor, got {}",
        store.app_monitors.len()
    );
    let stored = store
        .app_monitors
        .get("events-present")
        .expect("the seeded monitor");
    assert_eq!(
        stored.events.len(),
        2,
        "case \"store after refusals\": expected only the accepted batch's two events, got {}",
        stored.events.len()
    );
}

#[tokio::test]
async fn a_deleted_monitors_id_is_refused_the_same_way_as_one_never_minted() {
    let (router, _engine) = emulator();
    // Two monitors, so the delete leaves the store populated: what stops resolving is the
    // one id, not the service.
    let doomed = create(
        &router,
        "doomed",
        json!({"Name": "events-doomed", "Domain": "example.com"}),
    )
    .await;
    let kept = create(
        &router,
        "kept",
        json!({"Name": "events-kept", "Domain": "kept.example.com"}),
    )
    .await;
    assert_accepted(&router, "before the delete", &doomed).await;

    let deleted = call(&router, "DELETE", "/appmonitor/events-doomed", None).await;
    assert_eq!(
        deleted.status,
        200,
        "case \"delete\": expected DeleteAppMonitor to answer 200, got {} {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // The deleted monitor's id and an id that never existed are refused with the SAME
    // body, byte for byte: that is what makes the refusal disclose nothing about which of
    // the two the caller asked about.
    let after_delete = assert_refused(&router, "the deleted monitor's id", &doomed).await;
    for (case, id) in NEVER_MINTED_IDS {
        let never_minted = assert_refused(&router, case, id).await;
        assert_eq!(
            never_minted, after_delete,
            "case {case:?}: a never-minted id and a deleted monitor's id must be refused \
             identically, got {never_minted:?} and {after_delete:?}"
        );
    }

    // Re-creating the name mints a NEW id: the old one still resolves to nothing, while
    // the new one — and the monitor that was never deleted — are accepted.
    let recreated = create(
        &router,
        "recreated",
        json!({"Name": "events-doomed", "Domain": "again.example.com"}),
    )
    .await;
    assert_ne!(
        recreated, doomed,
        "case \"recreated\": the re-created monitor is a new resource, so its id must differ"
    );
    assert_refused(&router, "the id the delete retired", &doomed).await;
    assert_accepted(&router, "the re-created monitor's id", &recreated).await;
    assert_accepted(&router, "the monitor that was never deleted", &kept).await;
}
