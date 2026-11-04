// SPDX-License-Identifier: CC0-1.0

//! `UpdateCollection`'s `clientToken` replay, scoped to the collection the request
//! addresses, end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: an idempotency token names the request it was first spent on, so a
//! replay answers the record that call left only when the request addresses the SAME
//! resource; a token spent again against another resource is a request for that other
//! resource, which is served on its own terms — its write applied and its own record
//! answered, or the refusal its own state owes. The tables below span what that rule
//! generalises over for this operation: every member an update may apply, every value of
//! the two enums among them, and every answer the addressed collection can owe (its write,
//! the invalid-input refusal for an id naming nothing, the state conflict for a collection
//! that is not ready to use) — plus the token's own scope, which is one operation.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::client_token_scope::await_collection_active;
use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;

/// `CollectionStatus$UPDATING` — the non-terminal status an update that RECONFIGURES the
/// collection (its vector-search acceleration) leaves it in
/// (`smithy_query(query="shape", shape="CollectionStatus")`).
const UPDATING: &str = "UPDATING";

/// `CollectionStatus$ACTIVE` — "Collection resource is ready to use", the only status a
/// collection may be updated from.
const ACTIVE: &str = "ACTIVE";

/// One row of the scope table: the member an update names, and the value each of the two
/// collections asks for.
struct Row {
    case: &'static str,
    /// The request member the update names, which is also the detail member it is reported
    /// in (`smithy_query(query="shape", shape="UpdateCollectionDetail")`).
    member: &'static str,
    /// What the FIRST collection asks for, and what the second one asks for instead.
    first: Value,
    second: Value,
}

/// The `UpdateCollection` request that writes `value` into `member` of the collection `id`
/// addresses, carrying `token`.
fn update_request(id: &str, member: &str, value: &Value, token: &str) -> Value {
    let mut request = json!({ "id": id, "clientToken": token });
    request[member] = value.clone();
    request
}

/// Call `UpdateCollection` with `request`, asserting it was accepted, and answer the
/// `updateCollectionDetail` it reports.
async fn accepted(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, code, answer) = call(router, "UpdateCollection", request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from UpdateCollection, got {status} {code} with {answer}"
    );
    at(&answer, "updateCollectionDetail").clone()
}

// ---------------------------------------------------------------------------
// A token spent again on another collection is a request for THAT collection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_collection_token_spent_again_serves_the_collection_the_request_addresses() {
    // One row per member `UpdateCollection` applies, each with the value the first
    // collection asks for and the DIFFERENT value the second one does. `DeletionProtection`
    // is `ENABLED | DISABLED` and `ServerlessVectorAccelerationStatus` is
    // `ENABLED | DISABLED | ALLOWED` (`smithy_query(query="shape",
    // shape="DeletionProtection,ServerlessVectorAccelerationStatus")`), so the rows below
    // cover every value of both, `ALLOWED` included.
    let rows = vec![
        Row {
            case: "description",
            member: "description",
            first: json!("the change the first collection asked for"),
            second: json!("the change the second collection asked for"),
        },
        Row {
            case: "deletionProtection DISABLED then ENABLED",
            member: "deletionProtection",
            first: json!("DISABLED"),
            second: json!("ENABLED"),
        },
        Row {
            case: "deletionProtection ENABLED then DISABLED",
            member: "deletionProtection",
            first: json!("ENABLED"),
            second: json!("DISABLED"),
        },
        Row {
            case: "vectorOptions DISABLED then ALLOWED",
            member: "vectorOptions",
            first: json!({ "ServerlessVectorAcceleration": "DISABLED" }),
            second: json!({ "ServerlessVectorAcceleration": "ALLOWED" }),
        },
        Row {
            case: "vectorOptions ALLOWED then ENABLED",
            member: "vectorOptions",
            first: json!({ "ServerlessVectorAcceleration": "ALLOWED" }),
            second: json!({ "ServerlessVectorAcceleration": "ENABLED" }),
        },
    ];

    for row in rows {
        // One emulator per row, so a row is answered only out of what it stored itself.
        let router = emulator();
        let case = row.case;
        let token = "one-spent-update-collection-token";
        // The status an accepted update of this row's member leaves behind: `vectorOptions`
        // reconfigures what the collection runs, so it is reported `UPDATING` until the
        // clock settles it; a `description` or `deletionProtection` write is complete when
        // the call answers, so the collection keeps its `ACTIVE`.
        let expected_status = if row.member == "vectorOptions" {
            UPDATING
        } else {
            ACTIVE
        };

        let first_name = "token-scope-first";
        let second_name = "token-scope-second";
        let first_id = text(&active_collection(&router, first_name).await, "id");
        let second_id = text(&active_collection(&router, second_name).await, "id");

        // The token's FIRST call: an ordinary update of the first collection.
        let what = format!("{case}: the first call");
        let detail = accepted(
            &router,
            &what,
            update_request(&first_id, row.member, &row.first, token),
        )
        .await;
        assert_eq!(
            (text(&detail, "id"), text(&detail, "name")),
            (first_id.clone(), first_name.to_string()),
            "case {what}: expected the answer to describe the collection it addressed, got {detail}"
        );
        assert_eq!(
            at(&detail, row.member),
            &row.first,
            "case {what}: expected {} {}, got {detail}",
            row.member,
            row.first
        );
        assert_eq!(
            text(&detail, "status"),
            expected_status,
            "case {what}: an accepted update of {} reports {expected_status}, got {detail}",
            row.member
        );
        let settled_first = await_collection_active(&router, &what, &first_id).await;

        // The SAME token against the OTHER collection: a request for that other
        // collection, so its own write is applied and its own record answered.
        let what = format!("{case}: the token spent again on another collection");
        let detail = accepted(
            &router,
            &what,
            update_request(&second_id, row.member, &row.second, token),
        )
        .await;
        assert_eq!(
            (text(&detail, "id"), text(&detail, "name")),
            (second_id.clone(), second_name.to_string()),
            "case {what}: expected the answer to describe the collection it addressed, got {detail}"
        );
        assert_eq!(
            at(&detail, row.member),
            &row.second,
            "case {what}: expected the change it asked for ({} {}), got {detail}",
            row.member,
            row.second
        );
        assert_eq!(
            text(&detail, "status"),
            expected_status,
            "case {what}: the write was applied, so it reports {expected_status}, got {detail}"
        );

        // The STORE agrees: each collection holds its own change and nothing of the
        // other's.
        let what = format!("{case}: the stored second collection");
        let stored_second = await_collection_active(&router, &what, &second_id).await;
        assert_eq!(
            at(&stored_second, row.member),
            &row.second,
            "case {what}: expected the accepted write to be stored ({} {}), got {stored_second}",
            row.member,
            row.second
        );
        let what = format!("{case}: the stored first collection");
        let stored_first = await_collection_active(&router, &what, &first_id).await;
        assert_eq!(
            at(&stored_first, row.member),
            &row.first,
            "case {what}: expected its own change to be untouched ({} {}), got {stored_first}",
            row.member,
            row.first
        );

        // A RETRY of the token's FIRST request still replays the record that call left:
        // scoping the replay has not turned a retry into a second write, which the
        // unchanged `lastModifiedDate` and the terminal status both show.
        let what = format!("{case}: the retry of the first request");
        let detail = accepted(
            &router,
            &what,
            update_request(&first_id, row.member, &row.first, token),
        )
        .await;
        assert_eq!(
            text(&detail, "id"),
            first_id,
            "case {what}: expected the collection it addressed, got {detail}"
        );
        assert_eq!(
            at(&detail, row.member),
            &row.first,
            "case {what}: expected the record the first call left ({} {}), got {detail}",
            row.member,
            row.first
        );
        assert_eq!(
            at(&detail, "lastModifiedDate"),
            at(&settled_first, "lastModifiedDate"),
            "case {what}: a retry writes nothing, so lastModifiedDate does not move, got {detail}"
        );
        assert_eq!(
            text(&detail, "status"),
            ACTIVE,
            "case {what}: a retry writes nothing, so the collection stays {ACTIVE}, got {detail}"
        );
    }
}

// ---------------------------------------------------------------------------
// A spent token cannot answer for a collection the request does not address: each
// addressed collection still owes the answer its OWN state implies.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_spent_update_collection_token_cannot_answer_for_a_collection_it_does_not_address() {
    let router = emulator();
    let token = "a-token-spent-on-the-first-collection";

    let first_id = text(&active_collection(&router, "refused-scope-first").await, "id");
    let mine = json!("the change the first collection asked for");
    let detail = accepted(
        &router,
        "the first call",
        update_request(&first_id, "description", &mine, token),
    )
    .await;
    assert_eq!(
        text(&detail, "id"),
        first_id,
        "case the first call: expected the collection it addressed, got {detail}"
    );
    let settled = await_collection_active(&router, "the first collection", &first_id).await;

    // A collection that has settled, so it can be moved into the non-terminal `UPDATING`
    // below, and one still `CREATING`: the two rows that are not "ready to use".
    let updating_id = text(
        &active_collection(&router, "refused-scope-updating").await,
        "id",
    );
    let (status, code, answer) = call(
        &router,
        "CreateCollection",
        json!({ "name": "refused-scope-creating", "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection(refused-scope-creating): expected 200, got {status} {code} with {answer}"
    );
    let creating_id = text(at(&answer, "createCollectionDetail"), "id");
    // Starting an update that RECONFIGURES this one leaves it mid-update, which is the
    // state a second writer collides with. Both non-terminal rows are addressed immediately
    // below, before the clock makes their transitions due.
    accepted(
        &router,
        "the collection taken into UPDATING",
        json!({
            "id": updating_id,
            "vectorOptions": { "ServerlessVectorAcceleration": "ENABLED" },
        }),
    )
    .await;

    // Each row addresses a collection on its own terms, so each owes the answer that
    // collection's own state implies — and a spent token turns none of them into a 200
    // describing the collection it was first spent on. `UpdateCollection` models no
    // not-found error (its errors are `ConflictException`, `InternalServerException` and
    // `ValidationException`, `smithy_query(query="operation",
    // operation="UpdateCollection")`), so an id naming no collection is invalid input,
    // while a collection that is not `ACTIVE` is the state conflict `ConflictException`
    // describes.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "an id naming no collection",
            "nosuchcollection000".to_string(),
            "ValidationException",
        ),
        (
            "a collection still CREATING",
            creating_id,
            "ConflictException",
        ),
        (
            "a collection already UPDATING",
            updating_id,
            "ConflictException",
        ),
    ];
    for (what, id, expected_code) in rows {
        let (status, code, answer) = call(
            &router,
            "UpdateCollection",
            update_request(
                &id,
                "description",
                &json!("the change this request asked for"),
                token,
            ),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, expected_code),
            "case {what}: expected {CLIENT_ERROR} {expected_code}, got {status} {code} with {answer}"
        );
    }

    // The collection the token was first spent on is untouched by those refusals.
    let what = "the first collection after the refusals";
    let stored = await_collection_active(&router, what, &first_id).await;
    assert_eq!(
        at(&stored, "description"),
        &mine,
        "case {what}: expected only its own change, got {stored}"
    );
    assert_eq!(
        at(&stored, "lastModifiedDate"),
        at(&settled, "lastModifiedDate"),
        "case {what}: a refused write stores nothing, so lastModifiedDate does not move, got {stored}"
    );
}

// ---------------------------------------------------------------------------
// A token's replay is scoped to ONE operation as well as to one resource.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_collection_token_is_scoped_to_the_operation_that_spent_it() {
    let router = emulator();
    let token = "a-token-first-spent-on-a-create";

    // `CreateCollection` spends the token first, minting its own collection.
    let (status, code, answer) = call(
        &router,
        "CreateCollection",
        json!({
            "name": "operation-scope-created",
            "encryptionConfig": { "aWSOwnedKey": true },
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection(carrying the token): expected 200, got {status} {code} with {answer}"
    );
    let created_id = text(at(&answer, "createCollectionDetail"), "id");
    let created = await_collection_active(&router, "the created collection", &created_id).await;

    // The SAME token on `UpdateCollection` against another collection: the token names the
    // request it was first spent on, which was another operation's, so this is an ordinary
    // update of the collection it addresses.
    let other = active_collection(&router, "operation-scope-other").await;
    let other_id = text(&other, "id");
    let marker = json!("the change the addressed collection asked for");
    let what = "the update carrying the create's token";
    let detail = accepted(
        &router,
        what,
        update_request(&other_id, "description", &marker, token),
    )
    .await;
    assert_eq!(
        text(&detail, "id"),
        other_id,
        "case {what}: expected the collection it addressed, got {detail}"
    );
    assert_eq!(
        at(&detail, "description"),
        &marker,
        "case {what}: expected the change it asked for ({marker}), got {detail}"
    );

    let stored = await_collection_active(&router, "the addressed collection", &other_id).await;
    assert_eq!(
        at(&stored, "description"),
        &marker,
        "case {what}: expected the accepted write to be stored ({marker}), got {stored}"
    );
    let stored_created =
        await_collection_active(&router, "the created collection", &created_id).await;
    assert_eq!(
        at(&stored_created, "description"),
        at(&created, "description"),
        "case {what}: expected the collection the create minted to be untouched, got {stored_created}"
    );
}
