// SPDX-License-Identifier: CC0-1.0

//! The documented uniqueness of an SdiSource's `Name`, end to end over the
//! restJson1 wire.
//!
//! Rule: `CreateSdiSource$Name` and `UpdateSdiSource$Name` are documented
//! "Specify a name that is unique in the AWS account… Names are
//! case-sensitive", so a create or a rename that would leave two live sources
//! of the account under one name is refused with the operation's modelled
//! `ConflictException` (409) carrying this service's fixed name-collision
//! sentence, and stores nothing.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES over what the rule generalises over: the FORM of the
//! colliding name, which of the other members the two requests differ in,
//! whether a token makes the second request a retry, and whether the name's
//! holder is still live. The issue's `curling-cameraA` pair is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sdi_source_lifecycle::{created, delete_source};
use crate::wire::{call, error_code, error_message, medialive, DEFAULT_REGION as REGION};

/// The collection URI `CreateSdiSource` / `ListSdiSources` are bound to
/// (`smithy_query(query="operation", operation="CreateSdiSource")`:
/// `POST /prod/sdiSources`).
const SDI_SOURCES: &str = "/prod/sdiSources";

/// The fixed sentence this service answers every name collision with
/// (`crate::state::NAME_ALREADY_IN_USE_MESSAGE`).
const NAME_ALREADY_IN_USE: &str = "Name already in use";

/// Name forms the collision rule has to hold for. `Name` targets bare
/// `__string` with no `@length` and no `@pattern`
/// (`smithy_query(query="operation", operation="CreateSdiSource")`), so each of
/// these is a name the model admits: the documented example, one character,
/// digits, a name with a space, a non-ASCII name and a long one.
const NAMES: [(&str, &str); 6] = [
    ("the documented example", "curling-cameraA"),
    ("one character", "Z"),
    ("digits only", "407"),
    ("a space inside the name", "camera at the rink"),
    ("non-ASCII characters", "caméra-à-droite"),
    (
        "a long name",
        "curling-cameraA-curling-cameraA-curling-cameraA-curling-cameraA-\
         curling-cameraA-curling-cameraA-curling-cameraA-curling-cameraA",
    ),
];

async fn create_source(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "POST", SDI_SOURCES, Some(body)).await;
    if status == 201 {
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

async fn describe_source(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    let (status, response) = call(router, "GET", &format!("{SDI_SOURCES}/{id}"), None).await;
    if status == 200 {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

/// Every source `ListSdiSources` reports, so a refused request can be shown to
/// have stored nothing.
async fn listed_sources(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, body) = call(router, "GET", SDI_SOURCES, None).await;
    assert_eq!(
        status, 200,
        "ListSdiSources: expected 200, got {status}: {body}"
    );
    body["sdiSources"].as_array().cloned().unwrap_or_default()
}

/// The `id`s the account holds under `name`, which the rule bounds at one.
async fn ids_named(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    listed_sources(router)
        .await
        .iter()
        .filter(|source| source["name"] == json!(name))
        .map(|source| source["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert a refusal's status, code and exact message — and that the message
/// carries nothing the request did not name, the rule every rejection of this
/// service follows (`crate::rejection_messages`).
fn assert_collision(case: &str, status: u16, body: &Value, must_not_name: &[&str]) {
    assert_eq!(
        status, 409,
        "case {case:?}: expected 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {body}"
    );
    assert_eq!(
        error_message(body),
        NAME_ALREADY_IN_USE,
        "case {case:?}: expected the message {NAME_ALREADY_IN_USE:?}, got {:?}",
        error_message(body)
    );
    for leaked in must_not_name {
        assert!(
            !error_message(body).contains(leaked),
            "case {case:?}: expected the message not to carry {leaked:?}, got {:?}",
            error_message(body)
        );
    }
}

/// Rule: a second `CreateSdiSource` under a `Name` the account already holds is
/// refused with the operation's modelled `ConflictException` and stores no
/// source, whatever else the two requests differ in.
///
/// The rows span the FORM of the colliding name and the members the second
/// request carries beside the name, because the collision is about the name
/// alone: the issue's `curling-cameraA` SINGLE-then-QUAD pair is one row.
#[tokio::test]
async fn a_second_create_under_a_held_name_is_refused() {
    // (case, the first request's extra members, the second request's extra ones)
    let shapes: [(&str, Value, Value); 4] = [
        (
            "SINGLE then QUAD in QUADRANT",
            json!({ "type": "SINGLE" }),
            json!({ "type": "QUAD", "mode": "QUADRANT" }),
        ),
        (
            "the same type and mode twice",
            json!({ "type": "QUAD", "mode": "INTERLEAVE" }),
            json!({ "type": "QUAD", "mode": "INTERLEAVE" }),
        ),
        ("nothing but the name", json!({}), json!({})),
        (
            "tags on the second",
            json!({ "type": "SINGLE" }),
            json!({ "type": "SINGLE", "tags": { "crew": "rink-a" } }),
        ),
    ];

    for (name_case, name) in NAMES {
        for (shape_case, first_extra, second_extra) in &shapes {
            let case = format!("{name_case}, {shape_case}");
            let router = medialive();

            let mut first = first_extra.clone();
            first["name"] = json!(name);
            let source = created(&router, &case, first).await;
            let id = source["id"].as_str().unwrap_or_default().to_string();

            let mut second = second_extra.clone();
            second["name"] = json!(name);
            let (status, refusal) = create_source(&router, second).await;
            assert_collision(&case, status, &refusal, &[name, id.as_str(), REGION]);

            // The refused create stored nothing: the account still holds exactly
            // the one source under that name, unchanged.
            assert_eq!(
                ids_named(&router, name).await,
                vec![id.clone()],
                "case {case:?}: expected the one source under {name:?} to be the only one"
            );
            let (status, described) = describe_source(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeSdiSource: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described, source,
                "case {case:?}: expected the refused create to leave the source as it was, got {described}"
            );
        }
    }
}

/// Rule: only an EXACT match of a held name collides — "Names are
/// case-sensitive" — and a request that names nothing takes no name, so it
/// collides with nothing.
///
/// The rows differ along how the second name relates to the first: the same
/// name case-folded either way, a name that merely contains it, one that
/// differs by surrounding space, and the absent name.
#[tokio::test]
async fn only_an_exact_name_match_collides() {
    let held = "curling-cameraA";
    // (case, the second request's name, whether it is refused)
    let rows: [(&str, Option<&str>, bool); 7] = [
        ("the same name", Some("curling-cameraA"), true),
        ("lowercased", Some("curling-cameraa"), false),
        ("uppercased", Some("CURLING-CAMERAA"), false),
        ("a prefix of it", Some("curling-camera"), false),
        ("the name with a suffix", Some("curling-cameraAB"), false),
        ("the name with trailing space", Some("curling-cameraA "), false),
        ("no name at all", None, false),
    ];

    for (case, second_name, refused) in rows {
        let router = medialive();
        let first = created(&router, case, json!({ "name": held, "type": "SINGLE" })).await;
        let first_id = first["id"].as_str().unwrap_or_default().to_string();

        let mut body = json!({ "type": "SINGLE" });
        if let Some(name) = second_name {
            body["name"] = json!(name);
        }
        let (status, response) = create_source(&router, body).await;

        if refused {
            assert_collision(case, status, &response, &[held, first_id.as_str()]);
            assert_eq!(
                listed_sources(&router).await.len(),
                1,
                "case {case:?}: expected the refused create to store nothing"
            );
            continue;
        }

        assert_eq!(
            status, 201,
            "case {case:?}: expected a distinct name to be served, got {status}: {response}"
        );
        assert_eq!(
            response["name"],
            second_name.map_or(Value::Null, |name| json!(name)),
            "case {case:?}: expected the second source to carry its own name, got {:?}",
            response["name"]
        );
        assert_ne!(
            response["id"], first["id"],
            "case {case:?}: expected a second source, got {:?} twice",
            first["id"]
        );
    }
}

/// Rule: the name check is the CREATE's, not the retry's: a create replayed
/// with the `RequestId` (`@idempotencyToken`) of the one that took the name
/// answers with that same source, while a create carrying a different token — or
/// none — under that name is the collision.
///
/// The rows span the token pairs the two creates can carry, which is what tells
/// a retry apart from a second create.
#[tokio::test]
async fn a_retry_under_the_same_token_is_not_a_collision() {
    // (case, the first token, the second token, whether the second is refused)
    let rows: [(&str, Option<&str>, Option<&str>, bool); 5] = [
        ("the same token", Some("token-a"), Some("token-a"), false),
        ("two tokens", Some("token-a"), Some("token-b"), true),
        ("no token at all", None, None, true),
        ("only the first names one", Some("token-a"), None, true),
        ("only the second names one", None, Some("token-b"), true),
    ];

    for (case, first_token, second_token, refused) in rows {
        let router = medialive();
        let name = "curling-cameraA";

        let mut first = json!({ "name": name, "type": "SINGLE" });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let source = created(&router, case, first).await;
        let id = source["id"].as_str().unwrap_or_default().to_string();

        let mut second = json!({ "name": name, "type": "QUAD", "mode": "QUADRANT" });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }
        let (status, response) = create_source(&router, second).await;

        if refused {
            assert_collision(case, status, &response, &[name, id.as_str()]);
        } else {
            assert_eq!(
                status, 201,
                "case {case:?}: expected the retry to be served, got {status}: {response}"
            );
            assert_eq!(
                response, source,
                "case {case:?}: expected the retry to answer with the first source, got {response}"
            );
        }
        assert_eq!(
            ids_named(&router, name).await,
            vec![id.clone()],
            "case {case:?}: expected the account to hold one source under {name:?}"
        );
    }
}

/// Rule: a `DELETED` source is out of the account, so the name it was created
/// under is free again — for the next create and for a rename alike.
#[tokio::test]
async fn a_deleted_source_frees_its_name() {
    for (name_case, name) in NAMES {
        let router = medialive();
        let first = created(&router, name_case, json!({ "name": name, "type": "SINGLE" })).await;
        let first_id = first["id"].as_str().unwrap_or_default().to_string();

        // While it is live the name is taken.
        let (status, refusal) = create_source(&router, json!({ "name": name })).await;
        assert_collision(name_case, status, &refusal, &[name, first_id.as_str()]);

        let (status, deleted) = delete_source(&router, &first_id).await;
        assert_eq!(
            status, 200,
            "case {name_case:?}: DeleteSdiSource: expected 200, got {status}: {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!("DELETED"),
            "case {name_case:?}: expected the deleted source to be DELETED, got {:?}",
            deleted["state"]
        );

        // And now the name is the next create's to take.
        let (status, second) = create_source(&router, json!({ "name": name, "type": "QUAD" })).await;
        assert_eq!(
            status, 201,
            "case {name_case:?}: expected the freed name to be served, got {status}: {second}"
        );
        assert_ne!(
            second["id"], first["id"],
            "case {name_case:?}: expected a new source, got {:?} twice",
            first["id"]
        );
        // The deleted source keeps the name it was created under; only the live
        // one answers for it from here on.
        let (status, described) = describe_source(&router, &first_id).await;
        assert_eq!(
            status, 200,
            "case {name_case:?}: DescribeSdiSource: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["name"],
            json!(name),
            "case {name_case:?}: expected the deleted source to keep its name, got {:?}",
            described["name"]
        );
    }
}

/// Rule: `UpdateSdiSource` resolves the same uniqueness — "Include this
/// parameter only if you want to change the name… Specify a name that is unique
/// in the AWS account" — so a rename onto a name another LIVE source holds is
/// refused and changes nothing, while re-sending the source's own name, a name
/// nothing holds, and a name only a deleted source carries are all served.
///
/// The rows differ along whose name the rename asks for and along which other
/// members the request carries with it, because the collision is about the name
/// alone.
#[tokio::test]
async fn a_rename_onto_a_held_name_is_refused() {
    // (case, the name the update asks for, extra members, whether it is refused)
    let rows: [(&str, &str, Value, bool); 6] = [
        ("another live source's name", "taken", json!({}), true),
        (
            "another live source's name, with other members too",
            "taken",
            json!({ "type": "QUAD", "mode": "INTERLEAVE" }),
            true,
        ),
        ("its own name", "mine", json!({}), false),
        (
            "its own name, with other members too",
            "mine",
            json!({ "type": "SINGLE" }),
            false,
        ),
        ("a name nothing holds", "fresh", json!({}), false),
        ("a deleted source's name", "gone", json!({}), false),
    ];

    for (case, asked, extra, refused) in rows {
        let router = medialive();
        // The account holds a live source called "taken", a deleted one that was
        // created as "gone", and the source this test renames, called "mine".
        let other = created(&router, case, json!({ "name": "taken", "type": "SINGLE" })).await;
        let other_id = other["id"].as_str().unwrap_or_default().to_string();
        let doomed = created(&router, case, json!({ "name": "gone", "type": "SINGLE" })).await;
        let doomed_id = doomed["id"].as_str().unwrap_or_default().to_string();
        let (status, body) = delete_source(&router, &doomed_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteSdiSource: expected 200, got {status}: {body}"
        );
        let mine = created(
            &router,
            case,
            json!({ "name": "mine", "type": "QUAD", "mode": "QUADRANT" }),
        )
        .await;
        let id = mine["id"].as_str().unwrap_or_default().to_string();

        let mut update = extra.clone();
        update["name"] = json!(asked);
        let (status, response) = update_source(&router, &id, update).await;

        if refused {
            assert_collision(case, status, &response, &[asked, other_id.as_str()]);
            // Nothing the refused update asked for was written — not the name
            // and not the members it carried beside it.
            let (status, described) = describe_source(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeSdiSource: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described, mine,
                "case {case:?}: expected the refused rename to change nothing, got {described}"
            );
            // And the other source still answers for the name.
            assert_eq!(
                ids_named(&router, asked).await,
                vec![other_id.clone()],
                "case {case:?}: expected {asked:?} to be held by the source that had it"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSdiSource: expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["name"],
            json!(asked),
            "case {case:?}: expected the rename to store {asked:?}, got {:?}",
            response["name"]
        );
        assert_eq!(
            response["id"], mine["id"],
            "case {case:?}: expected the rename to keep the id, got {:?}",
            response["id"]
        );
        let (status, described) = describe_source(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSdiSource: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described, response,
            "case {case:?}: expected the store to carry the renamed source, got {described}"
        );
    }
}

/// Rule: the name a rename FREES is the next caller's to take, so the
/// uniqueness holds over a sequence of renames rather than only at create time.
#[tokio::test]
async fn a_renamed_source_frees_the_name_it_left() {
    let router = medialive();
    let source = created(
        &router,
        "rename chain",
        json!({ "name": "curling-cameraA", "type": "SINGLE" }),
    )
    .await;
    let id = source["id"].as_str().unwrap_or_default().to_string();

    let (status, renamed) = update_source(&router, &id, json!({ "name": "curling-cameraB" })).await;
    assert_eq!(
        status, 200,
        "UpdateSdiSource: expected 200, got {status}: {renamed}"
    );

    // The name it left is free: a create takes it.
    let (status, second) =
        create_source(&router, json!({ "name": "curling-cameraA", "type": "QUAD" })).await;
    assert_eq!(
        status, 201,
        "expected the freed name to be served, got {status}: {second}"
    );
    let second_id = second["id"].as_str().unwrap_or_default().to_string();

    // The name it moved TO is now the taken one: the new source cannot rename
    // onto it, and neither can a create claim it.
    let (status, refusal) =
        update_source(&router, &second_id, json!({ "name": "curling-cameraB" })).await;
    assert_collision(
        "rename onto the name the first source moved to",
        status,
        &refusal,
        &["curling-cameraB", id.as_str()],
    );
    let (status, refusal) = create_source(&router, json!({ "name": "curling-cameraB" })).await;
    assert_collision(
        "create under the name the first source moved to",
        status,
        &refusal,
        &["curling-cameraB", id.as_str()],
    );

    // Exactly two sources, one under each name.
    assert_eq!(
        ids_named(&router, "curling-cameraA").await,
        vec![second_id.clone()],
        "expected the second source to hold the freed name"
    );
    assert_eq!(
        ids_named(&router, "curling-cameraB").await,
        vec![id.clone()],
        "expected the first source to hold the name it renamed to"
    );
}
