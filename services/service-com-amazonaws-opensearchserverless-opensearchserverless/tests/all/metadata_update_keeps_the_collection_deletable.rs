// SPDX-License-Identifier: CC0-1.0

//! What an accepted `UpdateCollection` leaves behind for the operations that come after
//! it, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: an update that only rewrites what a resource's record REMEMBERS is
//! complete when the call answers, so the resource keeps the settled status it already had
//! and the very next call is served on its own terms; only an update that RECONFIGURES what
//! the resource runs leaves it in a non-terminal status, from which the operations gated on
//! a settled one are refused. For a collection the remembered members are `description` and
//! `deletionProtection` and the reconfiguring one is `vectorOptions`, and the operation the
//! rule is observed through is `DeleteCollection`, which `ConflictException`'s own
//! documentation scopes to a collection "not in the ACTIVE, FAILED, or UPDATE_FAILED state".
//!
//! The table below spans what the rule generalises over for a metadata update: which
//! members it names (each one alone, both at once, and none at all), the value it writes
//! into the enum among them, and the answer the addressed collection then owes its
//! delete — accepted, or refused by the deletion-protection setting it carries, which is
//! the only thing a metadata update can change about whether it may be deleted.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::client_token_scope::await_collection_active;
use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::read_collection;

/// `CollectionStatus$ACTIVE` — "Collection resource is ready to use".
const ACTIVE: &str = "ACTIVE";

/// `CollectionStatus$UPDATING` — "Updating collection resource".
const UPDATING: &str = "UPDATING";

/// `CollectionStatus$DELETING` — "Deleting collection resource".
const DELETING: &str = "DELETING";

/// The `errorCode` and `errorMessage` a purged collection is reported under, from
/// `CollectionErrorDetail`'s own documentation of both members
/// (`smithy_query(query="shape", shape="CollectionErrorDetail")`).
const NOT_FOUND: &str = "NOT_FOUND";
const NOT_FOUND_MESSAGE: &str = "The specified Collection is not found.";

/// How many times the reconfiguring case may set its window up. The `UPDATING -> ACTIVE`
/// move is owed to the clock, so a host that stalls the test thread past the emulator's
/// span closes the window before the case can read an answer inside it; a closed window
/// means the case proved nothing, so it is set up again rather than failed.
const WINDOW_ATTEMPTS: usize = 5;

/// What a row expects of the `DeleteCollection` that follows its update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// The delete is accepted: 200, reported `DELETING`, and the collection purges.
    Accepted,
    /// The delete is refused by the collection's own deletion-protection setting — the
    /// modelled `ValidationException`, never the state conflict.
    ProtectionRefusal,
}

/// One row of the metadata table.
#[derive(Clone, Debug)]
struct Row {
    case: &'static str,
    /// The `deletionProtection` the collection is CREATED with.
    created_protection: &'static str,
    /// The members the update names, beside the id.
    members: Value,
    /// What the collection must report for `description` after the update, if anything.
    expected_description: Option<&'static str>,
    /// What it must report for `deletionProtection` after the update.
    expected_protection: &'static str,
    expect: Expect,
}

/// Create a collection named `name` with `protection`, and answer it once it is usable.
async fn protected_collection(
    router: &MultiServiceRouter,
    name: &str,
    protection: &str,
    case: &str,
) -> Value {
    let (status, code, answer) = call(
        router,
        "CreateCollection",
        json!({
            "name": name,
            "encryptionConfig": { "aWSOwnedKey": true },
            "deletionProtection": protection,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from CreateCollection, got {status} {code} with {answer}"
    );
    let id = text(at(&answer, "createCollectionDetail"), "id");
    await_collection_active(router, case, &id).await
}

/// Call `UpdateCollection` with `members` against `id`, asserting it was accepted, and
/// answer the `updateCollectionDetail` it reports.
async fn update(router: &MultiServiceRouter, id: &str, members: &Value, case: &str) -> Value {
    let mut request = members.clone();
    request["id"] = json!(id);
    let (status, code, answer) = call(router, "UpdateCollection", request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from UpdateCollection, got {status} {code} with {answer}"
    );
    at(&answer, "updateCollectionDetail").clone()
}

/// Poll `BatchGetCollection` until `id` is reported as an error detail rather than a
/// collection, and answer that whole body: the purge of a `DELETING` collection is owed to
/// the clock, so this waits for the wall clock rather than for a call.
///
/// The sibling suites' purge waits (`collection_lifecycle`, `delete_token_replay`) are
/// private to their modules and answer nothing, while this case asserts the `errorCode` and
/// `errorMessage` the purged collection is reported under, so it needs the body back.
async fn await_purged(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    for _ in 0..20 {
        let (status, code, answer) =
            call(router, "BatchGetCollection", json!({ "ids": [id] })).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from BatchGetCollection, got {status} {code} with {answer}"
        );
        if at(&answer, "collectionDetails")
            .as_array()
            .is_some_and(Vec::is_empty)
        {
            return answer;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    panic!("case {case}: the collection {id} was never purged");
}

// ---------------------------------------------------------------------------
// A metadata update leaves the collection usable, so the NEXT call is served on
// the collection's own terms.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_metadata_update_leaves_the_collection_deletable_on_the_very_next_call() {
    // `DeletionProtection` is `ENABLED | DISABLED` (`smithy_query(query="shape",
    // shape="DeletionProtection")`), and the rows below write both of its values, name each
    // remembered member alone, both together, and none at all.
    let rows = vec![
        Row {
            case: "deletionProtection turned off",
            created_protection: "ENABLED",
            members: json!({ "deletionProtection": "DISABLED" }),
            expected_description: None,
            expected_protection: "DISABLED",
            expect: Expect::Accepted,
        },
        Row {
            case: "deletionProtection turned off beside a new description",
            created_protection: "ENABLED",
            members: json!({
                "deletionProtection": "DISABLED",
                "description": "both members at once",
            }),
            expected_description: Some("both members at once"),
            expected_protection: "DISABLED",
            expect: Expect::Accepted,
        },
        Row {
            case: "description only",
            created_protection: "DISABLED",
            members: json!({ "description": "the updated description" }),
            expected_description: Some("the updated description"),
            expected_protection: "DISABLED",
            expect: Expect::Accepted,
        },
        Row {
            case: "no member at all",
            created_protection: "DISABLED",
            members: json!({}),
            expected_description: None,
            expected_protection: "DISABLED",
            expect: Expect::Accepted,
        },
        Row {
            case: "deletionProtection turned on",
            created_protection: "DISABLED",
            members: json!({ "deletionProtection": "ENABLED" }),
            expected_description: None,
            expected_protection: "ENABLED",
            expect: Expect::ProtectionRefusal,
        },
        Row {
            case: "description only, on a protected collection",
            created_protection: "ENABLED",
            members: json!({ "description": "still protected" }),
            expected_description: Some("still protected"),
            expected_protection: "ENABLED",
            expect: Expect::ProtectionRefusal,
        },
    ];

    for (position, row) in rows.into_iter().enumerate() {
        // One emulator per row, so a row is answered only out of what it stored itself.
        let router = emulator();
        let case = row.case;
        let name = format!("metadata-update-{position}");
        let created = protected_collection(&router, &name, row.created_protection, case).await;
        let id = text(&created, "id");
        let created_date = at(&created, "createdDate").as_i64().unwrap_or_default();

        // The update is accepted and reports the collection in the settled status it was
        // already in: nothing it changed has to be brought up.
        let detail = update(&router, &id, &row.members, case).await;
        assert_eq!(
            text(&detail, "status"),
            ACTIVE,
            "case {case}: a metadata update reports {ACTIVE}, got {detail}"
        );
        assert_eq!(
            text(&detail, "deletionProtection"),
            row.expected_protection,
            "case {case}: expected deletionProtection {}, got {detail}",
            row.expected_protection
        );
        assert_eq!(
            detail.get("description").and_then(Value::as_str),
            row.expected_description,
            "case {case}: expected description {:?}, got {detail}",
            row.expected_description
        );
        assert!(
            at(&detail, "lastModifiedDate")
                .as_i64()
                .unwrap_or_default()
                >= created_date,
            "case {case}: lastModifiedDate is the write instant, got {detail}"
        );

        // The store agrees, read back with no wait at all: the write landed inside the call
        // and the collection is usable.
        let read = read_collection(&router, &id).await;
        assert_eq!(
            text(&read, "status"),
            ACTIVE,
            "case {case}: the stored collection is {ACTIVE} straight after the update, got {read}"
        );
        assert_eq!(
            text(&read, "deletionProtection"),
            row.expected_protection,
            "case {case}: expected the stored deletionProtection {}, got {read}",
            row.expected_protection
        );
        assert_eq!(
            read.get("description").and_then(Value::as_str),
            row.expected_description,
            "case {case}: expected the stored description {:?}, got {read}",
            row.expected_description
        );

        // And the IMMEDIATELY following delete is served on the collection's own terms.
        let (status, code, answer) =
            call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
        match row.expect {
            Expect::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200 from the delete after the update, got {status} \
                     {code} with {answer}"
                );
                let deleted = at(&answer, "deleteCollectionDetail").clone();
                assert_eq!(
                    (text(&deleted, "id"), text(&deleted, "status")),
                    (id.clone(), DELETING.to_string()),
                    "case {case}: expected the delete to report {id} {DELETING}, got {deleted}"
                );

                // The accepted delete runs to completion: the collection purges and is
                // reported as missing rather than lingering.
                let purged = await_purged(&router, &id, case).await;
                let error = at(&purged, "collectionErrorDetails")
                    .as_array()
                    .and_then(|details| details.first())
                    .cloned()
                    .unwrap_or(Value::Null);
                assert_eq!(
                    (text(&error, "errorCode"), text(&error, "errorMessage")),
                    (NOT_FOUND.to_string(), NOT_FOUND_MESSAGE.to_string()),
                    "case {case}: expected the purged collection reported {NOT_FOUND}, got {purged}"
                );
            }
            Expect::ProtectionRefusal => {
                assert_eq!(
                    (status, code.as_str()),
                    (CLIENT_ERROR, "ValidationException"),
                    "case {case}: a protected collection is refused by its own setting, not by \
                     its status, got {status} {code} with {answer}"
                );
                // It is still there, still usable: the refused delete stored nothing.
                let read = read_collection(&router, &id).await;
                assert_eq!(
                    text(&read, "status"),
                    ACTIVE,
                    "case {case}: a refused delete leaves the collection {ACTIVE}, got {read}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The complement: an update that RECONFIGURES the collection is the one that
// leaves it in a non-terminal status.
// ---------------------------------------------------------------------------

/// One attempt at reading the refusal inside the window a reconfiguring update opens.
/// Answers `false` when the window closed before the delete was made, so the caller sets it
/// up again.
async fn attempt_window(router: &MultiServiceRouter, suffix: &str, case: &str) -> bool {
    let name = format!("reconfigured-{suffix}");
    let created = protected_collection(router, &name, "DISABLED", case).await;
    let id = text(&created, "id");

    let acceleration = json!({ "ServerlessVectorAcceleration": "ENABLED" });
    let detail = update(
        router,
        &id,
        &json!({ "vectorOptions": acceleration.clone() }),
        case,
    )
    .await;
    assert_eq!(
        text(&detail, "status"),
        UPDATING,
        "case {case}: an update that reconfigures the collection reports {UPDATING}, got {detail}"
    );
    assert_eq!(
        detail.get("vectorOptions"),
        Some(&acceleration),
        "case {case}: expected the change it asked for ({acceleration}), got {detail}"
    );

    // Inside that window the delete is the modelled state conflict...
    let (status, code, answer) =
        call(router, "DeleteCollection", json!({ "id": id.clone() })).await;
    if status == 200 {
        // The window closed before the delete reached the handler, so this attempt proved
        // nothing about what the service answers inside it.
        return false;
    }
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ConflictException"),
        "case {case}: expected the state conflict inside the window, got {status} {code} with \
         {answer}"
    );

    // ...and once the collection settles back on its own, the same delete is accepted.
    let settled = await_collection_active(router, case, &id).await;
    assert_eq!(
        settled.get("vectorOptions"),
        Some(&acceleration),
        "case {case}: expected the settled collection to carry the change, got {settled}"
    );
    let (status, code, answer) = call(router, "DeleteCollection", json!({ "id": id.clone() })).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from the delete once the update settled, got {status} {code} \
         with {answer}"
    );
    assert_eq!(
        text(at(&answer, "deleteCollectionDetail"), "status"),
        DELETING,
        "case {case}: expected the accepted delete to report {DELETING}, got {answer}"
    );
    true
}

#[tokio::test]
async fn a_reconfiguring_update_is_the_one_that_leaves_the_collection_mid_update() {
    let router = emulator();
    let case = "vectorOptions";
    let mut held = false;
    for attempt in 0..WINDOW_ATTEMPTS {
        if attempt_window(&router, &format!("{attempt}"), case).await {
            held = true;
            break;
        }
    }
    assert!(
        held,
        "case {case}: the collection left {UPDATING} before the delete could be refused in it, \
         on all {WINDOW_ATTEMPTS} attempts"
    );
}
