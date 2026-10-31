// SPDX-License-Identifier: CC0-1.0

//! How long a `CreateSdiSource` `RequestId` (`@idempotencyToken`) stays a
//! replay, and what the retry past the delete is answered with, member for
//! member — end to end over the restJson1 wire.
//!
//! RULE: a create's token binds only to a resource the account still holds, so
//! once the resource it minted has reached its terminal deleted state the
//! token's window is over and a create retried with it mints a NEW resource
//! instead of answering with the deleted one. `DeleteSdiSource` is a tombstone
//! delete — the record stays readable in the terminal `DELETED` state
//! (`smithy_query(query="shape", shape="SdiSourceState")`: IDLE | IN_USE |
//! DELETED) — but that record is one no caller may use or change, and the name
//! it carried is already free for a new source, so replaying it would answer a
//! create with something the caller can do nothing with and store nothing at
//! all.
//!
//! That rule holds for every tombstone-keeping resource of this service, and
//! the kinds it spans — the reservation, the multiplex and the node beside the
//! SdiSource — are the rows of
//! [`crate::idempotency_token_after_terminal_delete`]. What this suite adds is
//! the DEPTH the shared table does not carry: which members the retry's own
//! source reports, that the tombstone is left exactly as the delete made it and
//! still refuses a mutation, what the account holds afterwards, and that the
//! token then names the live source the retry stored.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suite is a TABLE: the reported create-delete-retry sequence is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sdi_source_lifecycle::{created as sdi_created, delete_source};
use crate::wire::{call, error_code, medialive};

/// The collection URI `CreateSdiSource` / `ListSdiSources` are bound to
/// (`smithy_query(query="operation", operation="CreateSdiSource")`:
/// `POST /prod/sdiSources`).
const SDI_SOURCES: &str = "/prod/sdiSources";

/// The token every row spends on its first create and then retries with.
const TOKEN: &str = "token-a";

async fn create_source(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "POST", SDI_SOURCES, Some(body)).await;
    if status == 201 {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

async fn describe_source(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    let (status, response) = call(router, "GET", &format!("{SDI_SOURCES}/{id}"), None).await;
    if status == 200 {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

async fn update_source(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    let (status, response) =
        call(router, "PUT", &format!("{SDI_SOURCES}/{id}"), Some(body)).await;
    if status == 200 {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

/// The source ids `ListSdiSources` reports, sorted — a deleted source's record
/// included, since the delete keeps it in the account.
async fn held_source_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", SDI_SOURCES, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListSdiSources: expected 200, got {status}: {body}"
    );
    let mut ids: Vec<String> = body["sdiSources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|source| source["id"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// The `id` of a reported source.
fn id_of(case: &str, reported: &Value) -> String {
    reported["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {reported}"))
        .to_string()
}

/// The `state` a reported source carries.
fn state_of(reported: &Value) -> String {
    reported["state"].as_str().unwrap_or_default().to_string()
}

/// Rule: a `CreateSdiSource` retried with the `RequestId` of a source that has
/// since been deleted mints a new source carrying the members THAT request
/// asked for — the deleted record is left exactly as the delete made it and
/// goes on refusing every mutation, and the token names the source the retry
/// stored from then on.
///
/// The rows differ along what the rule generalises over: the `SdiSourceType`
/// and `SdiSourceMode` the two creates carry (including the create that names
/// neither and the retry that carries nothing but the token), whether the retry
/// asks for the deleted source's own freed name or another one, and whether the
/// account holds a second live source whose own token the delete does not touch.
#[tokio::test]
async fn a_deleted_sdi_source_is_not_resurrected_by_its_idempotency_token() {
    // (case, the first create's members, the retry's members, whether a second
    //  live source holding its own token exists)
    let rows: [(&str, Value, Value, bool); 6] = [
        (
            "the retry asks for another name, type and mode",
            json!({ "name": "camera-a", "type": "SINGLE" }),
            json!({ "name": "camera-b", "type": "QUAD", "mode": "QUADRANT" }),
            false,
        ),
        (
            "the retry asks for the deleted source's own name",
            json!({ "name": "camera-a", "type": "SINGLE" }),
            json!({ "name": "camera-a", "type": "QUAD", "mode": "INTERLEAVE" }),
            false,
        ),
        (
            "the retry carries nothing but the token",
            json!({ "name": "camera-a", "type": "QUAD", "mode": "QUADRANT" }),
            json!({}),
            false,
        ),
        (
            "neither create names a type or a mode",
            json!({ "name": "camera-a" }),
            json!({ "name": "camera-c" }),
            false,
        ),
        (
            "QUAD in INTERLEAVE, beside a second live source",
            json!({ "name": "camera-a", "type": "SINGLE" }),
            json!({ "name": "camera-d", "type": "QUAD", "mode": "INTERLEAVE" }),
            true,
        ),
        (
            "SINGLE with no mode, beside a second live source",
            json!({ "name": "camera-a", "type": "QUAD", "mode": "QUADRANT" }),
            json!({ "name": "camera-e", "type": "SINGLE" }),
            true,
        ),
    ];

    for (case, first_body, retry_body, with_bystander) in rows {
        let router = medialive();

        // A second source, created under a token of its own, so the delete below
        // can be shown to end ONE token's window rather than the map's.
        let bystander = if with_bystander {
            Some(
                sdi_created(
                    &router,
                    case,
                    json!({ "name": "bystander", "type": "SINGLE", "requestId": "token-b" }),
                )
                .await,
            )
        } else {
            None
        };

        let mut first = first_body.clone();
        first["requestId"] = json!(TOKEN);
        let source = sdi_created(&router, case, first).await;
        let first_id = id_of(case, &source);

        // While the source is live the token IS a replay: the retry answers with
        // that source, member for member, and stores nothing.
        let mut retry = retry_body.clone();
        retry["requestId"] = json!(TOKEN);
        let (status, replayed) = create_source(&router, retry.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the live replay to answer 201, got {status}: {replayed}"
        );
        assert_eq!(
            replayed, source,
            "case {case:?}: expected a live source's token to replay it, got {replayed}"
        );

        let (status, dead) = delete_source(&router, &first_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteSdiSource: expected 200, got {status}: {dead}"
        );
        assert_eq!(
            state_of(&dead),
            "DELETED",
            "case {case:?}: expected the delete to report DELETED, got {dead}"
        );

        // The window is over: the retry mints a source of its own, carrying the
        // members IT asked for rather than the deleted source's.
        let (status, retried) = create_source(&router, retry.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the retry to answer 201, got {status}: {retried}"
        );
        let retried_id = id_of(case, &retried);
        assert_ne!(
            retried_id, first_id,
            "case {case:?}: expected a freshly minted source, got the deleted {first_id:?}"
        );
        for member in ["name", "type", "mode"] {
            assert_eq!(
                retried[member],
                retry_body.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the retry's own {member}, got {retried}"
            );
        }
        assert_eq!(
            state_of(&retried),
            "IDLE",
            "case {case:?}: expected the retry to answer with a live source, got {retried}"
        );

        // It really STORED that source, and the deleted record stayed exactly as
        // the delete left it.
        let (status, read) = describe_source(&router, &retried_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSdiSource: expected 200, got {status}: {read}"
        );
        assert_eq!(
            read, retried,
            "case {case:?}: expected the reader to report the source the retry stored, got {read}"
        );
        let (status, tombstone) = describe_source(&router, &first_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSdiSource on the tombstone: expected 200, got {status}: {tombstone}"
        );
        assert_eq!(
            tombstone, dead,
            "case {case:?}: expected the deleted source to stay as it was, got {tombstone}"
        );

        let mut expected_held = vec![first_id.clone(), retried_id.clone()];
        if let Some(bystander) = &bystander {
            expected_held.push(id_of(case, bystander));
        }
        expected_held.sort();
        assert_eq!(
            held_source_ids(&router, case).await,
            expected_held,
            "case {case:?}: expected the account to hold the deleted source and the retry's own"
        );

        // The source the retry answered with is one the caller may CHANGE, which
        // is exactly what the tombstone is not.
        let (status, renamed) = update_source(&router, &retried_id, json!({ "name": "usable" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry's source to be updatable, got {status}: {renamed}"
        );
        let (status, refused) = update_source(&router, &first_id, json!({ "name": "ghost" })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the tombstone to refuse a mutation, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "NotFoundException",
            "case {case:?}: expected NotFoundException on the tombstone, got {refused}"
        );

        // And the token now names the source the retry stored: one more retry
        // replays THAT one and stores nothing.
        let (status, again) = create_source(&router, retry).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the third create to answer 201, got {status}: {again}"
        );
        assert_eq!(
            again, renamed,
            "case {case:?}: expected the token to replay the live source, got {again}"
        );
        assert_eq!(
            held_source_ids(&router, case).await,
            expected_held,
            "case {case:?}: expected the second replay to have stored nothing"
        );

        // The bystander's own token was never the delete's business.
        if let Some(bystander) = bystander {
            let (status, replayed) = create_source(
                &router,
                json!({ "name": "another-name", "requestId": "token-b" }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the bystander's replay to answer 201, got {status}: {replayed}"
            );
            assert_eq!(
                replayed, bystander,
                "case {case:?}: expected the surviving token to replay its own source, got {replayed}"
            );
        }
    }
}
