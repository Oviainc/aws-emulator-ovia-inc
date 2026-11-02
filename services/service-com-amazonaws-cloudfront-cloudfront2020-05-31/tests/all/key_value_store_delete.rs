// SPDX-License-Identifier: CC0-1.0

//! `DeleteKeyValueStore`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * a delete REMOVES the one record every read of that store answered from, so afterwards
//!   the describe reports a store that does not exist and the name is free for a later
//!   create to claim, while every other store stands;
//! * the removal only happens at the version its caller read: `If-Match` is checked against
//!   the STORED version token — bare, quoted or weak, as HTTP entity tags are written — so a
//!   well-formed token that is not the current one fails the precondition (412), a value
//!   that is no version token at all is an invalid `If-Match` version (400), and a refused
//!   delete removes NOTHING;
//! * a name no store holds is not found (404), whatever `If-Match` carries, and removes
//!   nothing;
//! * a store a stored function configuration still associates by ARN cannot be deleted
//!   (`CannotDeleteEntityWhileInUse`, 409) and stays readable — at either stage of either
//!   kind of function — and the same delete succeeds once nothing associates it;
//! * the status the store holds does not gate the delete: a store still provisioning and one
//!   the clock has settled are both removed.
//!
//! The table rows differ along everything the delete generalises over — the name's length
//! (both published bounds of `KeyValueStoreName`) and the character classes its pattern
//! admits, whether the create named a comment, that comment's length and whether it is
//! ASCII, whether the create carried tags, the form the version token is written in, the
//! status the store holds, and which kind of function associates it.

use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    KVS_PROVISIONING_DELAY, KVS_STATUS_PROVISIONING, KVS_STATUS_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_lifecycle::{
    create_connection_function, Create as ConnectionCreate,
};
use crate::distribution_lifecycle::{call, emulator, is_minted_uuid, Answer, API_VERSION};
use crate::function_delete::delete_function;
use crate::function_lifecycle::{create_function, Create as FunctionCreate, VIEWER_REQUEST_CODE};
use crate::function_publish::publish_function;
use crate::function_update::{update_function, UPDATE_ETAG_HEADER};
use crate::key_value_store_lifecycle::{
    create_key_value_store, create_table, describe_key_value_store, member, Create,
};

/// The sentence CloudFront answers a read of a key value store it does not hold with — it
/// names the resource by its modelled shape name — spelled out here so a wrong constant in
/// the crate cannot make these assertions pass.
const KEY_VALUE_STORE_NOT_FOUND_MESSAGE: &str = "The specified KeyValueStore does not exist.";

/// The sentence CloudFront serves a delete of a key value store a function still associates
/// with: its fixed text, carrying the store the REQUEST addressed as its one variable part.
/// Spelled out here for the same reason.
fn key_value_store_in_use_message(name: &str) -> String {
    format!("Cannot delete KeyValueStore {name} because it is associated with a function")
}

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that no store in a fresh emulator holds, so a precondition carrying it is STALE
/// rather than invalid.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

// ---------------------------------------------------------------------------
// Calling the delete
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/key-value-store/{Name}`, carrying `if_match` as the `If-Match`
/// header the operation binds its required `IfMatch` member to (left out entirely when
/// `None`).
async fn delete_key_value_store(
    router: &MultiServiceRouter,
    name: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/key-value-store/{name}"),
        None,
        if_match,
    )
    .await
}

/// Create the store `create` asks for and answer `(its version token, its Id)`.
async fn seed(router: &MultiServiceRouter, create: &Create) -> (String, String) {
    let created = create_key_value_store(router, &create.xml()).await;
    assert_eq!(
        created.status,
        201,
        "case {}: the seed create must answer HTTP 201, got {} with {}",
        create.case,
        created.status,
        created.raw
    );
    let raw = created.raw.clone();
    (
        created.header("etag").to_string(),
        member(created.root(), "Id", create.case, &raw),
    )
}

/// Assert the store `name` is gone: the describe reports the service's not-found refusal.
async fn assert_absent(router: &MultiServiceRouter, name: &str, case: &str) {
    let described = describe_key_value_store(router, name).await;
    assert_eq!(
        described.status, 404,
        "case {case}: expected HTTP 404 after the delete, got {} with {}",
        described.status, described.raw
    );
    assert_eq!(
        described.error_code(),
        "EntityNotFound",
        "case {case}: expected EntityNotFound, got {}",
        described.raw
    );
    let message = described.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, KEY_VALUE_STORE_NOT_FOUND_MESSAGE,
        "case {case}: expected the not-found sentence, got {message:?}"
    );
}

/// Assert the store `name` is still there, holding `etag`.
async fn assert_present(router: &MultiServiceRouter, name: &str, etag: &str, case: &str) {
    let described = describe_key_value_store(router, name).await;
    assert_eq!(
        described.status, 200,
        "case {case}: the store must still be readable, got {} with {}",
        described.status, described.raw
    );
    assert_eq!(
        described.header("etag"),
        etag,
        "case {case}: a refused delete must not spend a version token, got {}",
        described.raw
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A delete under the store's current token REMOVES the record every read answered from:
/// the answer is an empty 204, the describe then reports a store that does not exist, a
/// second delete of the same name is refused the same way, every other store stands, and
/// the name is free for a later create — which mints an identity of its own.
#[tokio::test]
async fn delete_removes_the_store_every_read_answered_from() {
    let router = emulator();

    // Every row exists at once, so each delete is asserted to touch ONE record.
    let table = create_table();
    let mut held: Vec<(Create, String, String)> = Vec::new();
    for create in table {
        let (etag, id) = seed(&router, &create).await;
        held.push((create, etag, id));
    }

    for index in 0..held.len() {
        let (create, etag, id) = held[index].clone();
        let case = create.case;

        let deleted = delete_key_value_store(&router, &create.name, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204, got {} with {}",
            deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {case}: expected an empty body, got {:?}",
            deleted.raw
        );

        assert_absent(&router, &create.name, case).await;

        // The record is gone, so the name no longer names a version to delete either.
        let again = delete_key_value_store(&router, &create.name, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected HTTP 404 for the second delete, got {} with {}",
            again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for the second delete, got {}",
            again.raw
        );

        // Every store the delete did not name is untouched.
        for (other, other_etag, _) in held.iter().skip(index + 1) {
            assert_present(&router, &other.name, other_etag, other.case).await;
        }

        // The name is free again, and the store created under it is a NEW resource rather
        // than the one that was deleted.
        let recreated = create_key_value_store(&router, &create.xml()).await;
        assert_eq!(
            recreated.status, 201,
            "case {case}: the freed name must be claimable, got {} with {}",
            recreated.status, recreated.raw
        );
        let new_id = member(recreated.root(), "Id", case, &recreated.raw);
        assert!(
            is_minted_uuid(&new_id),
            "case {case}: expected a UUID Id, got {new_id:?}"
        );
        assert_ne!(
            new_id, id,
            "case {case}: the re-created store must not reuse the deleted store's Id"
        );
        // Delete it again so the rest of the walk sees only the rows it seeded.
        let cleanup = delete_key_value_store(
            &router,
            &create.name,
            Some(recreated.header("etag")),
        )
        .await;
        assert_eq!(
            cleanup.status, 204,
            "case {case}: the re-created store must be deletable, got {} with {}",
            cleanup.status, cleanup.raw
        );
    }
}

/// `If-Match` is compared against the store's CURRENT version: the token bare, quoted or
/// weak lets the delete land; a well-formed token that is not the current one fails the
/// precondition; a value that is no version token at all is an invalid `If-Match` version;
/// and every refusal leaves the record and its token exactly as they were.
#[tokio::test]
async fn if_match_governs_the_delete_against_the_stored_version() {
    let router = emulator();

    // All three forms are the same entity tag: CloudFront reports it bare, and HTTP writes
    // one quoted, optionally weak. A delete consumes its store, so each form gets one.
    type HeaderForm = (&'static str, fn(&str) -> String);
    let accepted: [HeaderForm; 3] = [
        ("the current token, bare", |etag| etag.to_string()),
        ("the current token, quoted", |etag| format!("\"{etag}\"")),
        ("the current token, weak", |etag| format!("W/\"{etag}\"")),
    ];
    for (index, (case, render)) in accepted.iter().enumerate() {
        let create = Create {
            case,
            name: format!("accepted_form_{index}"),
            comment: Some(format!("deleted under accepted form {index}")),
            tags: Vec::new(),
        };
        let (etag, _) = seed(&router, &create).await;
        let deleted = delete_key_value_store(&router, &create.name, Some(&render(&etag))).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_absent(&router, &create.name, case).await;
    }

    // A second store, so "another store's current token" is a real token this service
    // minted rather than one invented by the test.
    let other = Create {
        case: "the other store",
        name: "other_store".to_string(),
        comment: Some("another store entirely".to_string()),
        tags: Vec::new(),
    };
    let (other_etag, _) = seed(&router, &other).await;

    let subject = Create {
        case: "the store under test",
        name: "governed_store".to_string(),
        comment: Some("the comment the create stored".to_string()),
        tags: vec![("stage".to_string(), "prod".to_string())],
    };
    let (etag, _) = seed(&router, &subject).await;

    // (case, the If-Match header, expected status, expected error code). A version token is
    // `E` + 13 uppercase alphanumerics, so the first two rows are well-formed tokens that
    // are not this store's current one, and the rest are not tokens at all.
    let refusals: &[(&str, Option<&str>, u16, &str)] = &[
        (
            "another store's current token",
            Some(&other_etag),
            412,
            "PreconditionFailed",
        ),
        (
            "a well-formed token this service never minted",
            Some(UNHELD_VERSION_TOKEN),
            412,
            "PreconditionFailed",
        ),
        ("no version token at all", Some("nonsense"), 400, "InvalidIfMatchVersion"),
        (
            "a token one character too short",
            Some("E2QWRUHAPOMQ"),
            400,
            "InvalidIfMatchVersion",
        ),
        ("a token in lowercase", Some("e2qwruhapomqz"), 400, "InvalidIfMatchVersion"),
        ("the empty header", Some(""), 400, "InvalidIfMatchVersion"),
    ];

    for (case, if_match, status, code) in refusals {
        let answer = delete_key_value_store(&router, &subject.name, *if_match).await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        assert_present(&router, &subject.name, &etag, case).await;
    }

    // The store the refusals left standing is still deletable at the version it holds.
    let deleted = delete_key_value_store(&router, &subject.name, Some(&etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case after the refusals: expected HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );
    assert_present(&router, &other.name, &other_etag, "the other store").await;
}

/// A name no store holds names nothing to delete — `EntityNotFound` (404) — whatever shape
/// the name takes and whatever `If-Match` carries, and the store that does exist is left
/// alone.
#[tokio::test]
async fn delete_refuses_a_name_no_store_holds() {
    let router = emulator();
    let present = Create {
        case: "the one store that exists",
        name: "present_store".to_string(),
        comment: Some("the stored comment".to_string()),
        tags: Vec::new(),
    };
    let (etag, _) = seed(&router, &present).await;

    // Rows: a name never created, the same name in another case (names are compared as
    // given), a prefix of the name that exists, and the longest name the model admits —
    // each with the existing store's current token, so the lookup, not the precondition, is
    // what refuses them.
    let absent: [(&str, String); 4] = [
        ("a name never created", "absent_store".to_string()),
        ("the same name in another case", "PRESENT_STORE".to_string()),
        ("a prefix of the existing name", "present".to_string()),
        ("the longest admissible name", "z".repeat(64)),
    ];
    for (case, name) in absent {
        let answer = delete_key_value_store(&router, &name, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {name:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for {name:?}, got {}",
            answer.raw
        );
        assert_present(&router, &present.name, &etag, case).await;
    }
}

/// A store a stored function configuration associates by ARN is IN USE: the delete is
/// refused with `CannotDeleteEntityWhileInUse` (409) and the store stays readable — whether
/// the association sits in a CloudFront function's `DEVELOPMENT` stage, in the `LIVE` stage
/// a publish copied it into, or in a connection function's configuration — while a store
/// nothing associates is deleted. Once nothing names it any more, the same delete lands.
#[tokio::test]
async fn a_store_a_function_associates_cannot_be_deleted() {
    let router = emulator();

    let associated = Create {
        case: "a store a CloudFront function associates",
        name: "function_store".to_string(),
        comment: Some("read by a function".to_string()),
        tags: Vec::new(),
    };
    let unassociated = Create {
        case: "a store nothing associates",
        name: "lonely_store".to_string(),
        comment: None,
        tags: Vec::new(),
    };
    let by_connection = Create {
        case: "a store a connection function associates",
        name: "connection_store".to_string(),
        comment: Some("read by a connection function".to_string()),
        tags: Vec::new(),
    };

    let (associated_etag, _) = seed(&router, &associated).await;
    let (unassociated_etag, _) = seed(&router, &unassociated).await;
    let (connection_etag, _) = seed(&router, &by_connection).await;

    let associated_arn = describe_key_value_store(&router, &associated.name)
        .await
        .root()
        .text_at("ARN")
        .expect("the stored store reports its ARN")
        .to_string();
    let connection_arn = describe_key_value_store(&router, &by_connection.name)
        .await
        .root()
        .text_at("ARN")
        .expect("the stored store reports its ARN")
        .to_string();

    // A CloudFront function whose DEVELOPMENT configuration associates the store.
    let function = FunctionCreate {
        case: "the function that reads the store",
        name: "catalog_lookup".to_string(),
        comment: "reads the catalog from a key value store".to_string(),
        runtime: "cloudfront-js-2.0",
        code: VIEWER_REQUEST_CODE.as_bytes().to_vec(),
        associations: vec![associated_arn.clone()],
        tags: Vec::new(),
    };
    let created = create_function(&router, &function.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: the function must be created, got {} with {}",
        function.case, created.status, created.raw
    );
    let development_etag = created.header("etag").to_string();

    // A connection function whose configuration associates the other store.
    let connection_function = ConnectionCreate {
        case: "the connection function that reads the store",
        name: "connection_lookup".to_string(),
        comment: "reads the catalog from a key value store".to_string(),
        runtime: "cloudfront-js-2.0",
        code: b"function handler(event) { return event.request; }".to_vec(),
        associations: vec![connection_arn.clone()],
        tags: Vec::new(),
    };
    let created_connection =
        create_connection_function(&router, &connection_function.xml()).await;
    assert_eq!(
        created_connection.status, 201,
        "case {}: the connection function must be created, got {} with {}",
        connection_function.case, created_connection.status, created_connection.raw
    );

    // (case, the store, its token) — each association, in the stage it sits in.
    let in_use: [(&str, &Create, &String); 2] = [
        (
            "associated by a CloudFront function's DEVELOPMENT stage",
            &associated,
            &associated_etag,
        ),
        (
            "associated by a connection function",
            &by_connection,
            &connection_etag,
        ),
    ];
    for (case, store, etag) in in_use {
        let answer = delete_key_value_store(&router, &store.name, Some(etag)).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected HTTP 409, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "CannotDeleteEntityWhileInUse",
            "case {case}: expected CannotDeleteEntityWhileInUse, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        let expected = key_value_store_in_use_message(&store.name);
        assert_eq!(
            message, expected,
            "case {case}: expected the in-use sentence {expected:?}, got {message:?}"
        );
        assert_present(&router, &store.name, etag, case).await;
    }

    // A store nothing associates stands beside them and is deleted.
    let deleted = delete_key_value_store(
        &router,
        &unassociated.name,
        Some(&unassociated_etag),
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "case {}: expected HTTP 204, got {} with {}",
        unassociated.case, deleted.status, deleted.raw
    );
    assert_absent(&router, &unassociated.name, unassociated.case).await;

    // A publish COPIES the configuration into the LIVE stage, so the store is now named by
    // both stages — and still in use.
    let published = publish_function(&router, &function.name, &development_etag).await;
    assert_eq!(
        published.status, 200,
        "case the publish: expected HTTP 200, got {} with {}",
        published.status, published.raw
    );
    let after_publish =
        delete_key_value_store(&router, &associated.name, Some(&associated_etag)).await;
    assert_eq!(
        after_publish.status, 409,
        "case associated by both stages: expected HTTP 409, got {} with {}",
        after_publish.status, after_publish.raw
    );

    // An update that drops the association from DEVELOPMENT leaves the LIVE stage naming
    // the store, and a stage's own configuration is what counts: still in use.
    let without = FunctionCreate {
        associations: Vec::new(),
        ..function.clone()
    };
    let updated = update_function(
        &router,
        &function.name,
        &without,
        Some(&development_etag),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case the update: expected HTTP 200, got {} with {}",
        updated.status, updated.raw
    );
    let after_update =
        delete_key_value_store(&router, &associated.name, Some(&associated_etag)).await;
    assert_eq!(
        after_update.status, 409,
        "case associated by the LIVE stage only: expected HTTP 409, got {} with {}",
        after_update.status, after_update.raw
    );
    assert_present(
        &router,
        &associated.name,
        &associated_etag,
        "associated by the LIVE stage only",
    )
    .await;

    // With the function gone, nothing associates the store any more, so the same delete
    // lands.
    // `UpdateFunction` binds its version token to the `ETtag` response header, where the
    // rest of the function surface binds `ETag` (the sibling suite names the same header).
    let function_deleted =
        delete_function(&router, &function.name, updated.header(UPDATE_ETAG_HEADER)).await;
    assert_eq!(
        function_deleted.status, 204,
        "case deleting the function: expected HTTP 204, got {} with {}",
        function_deleted.status, function_deleted.raw
    );
    let freed = delete_key_value_store(&router, &associated.name, Some(&associated_etag)).await;
    assert_eq!(
        freed.status, 204,
        "case nothing associates it any more: expected HTTP 204, got {} with {}",
        freed.status, freed.raw
    );
    assert_absent(
        &router,
        &associated.name,
        "nothing associates it any more",
    )
    .await;
}

/// The status a store holds does not gate its delete: one still `PROVISIONING` and one the
/// clock has settled to `READY` are both removed at the version they hold.
#[tokio::test]
async fn a_store_is_deletable_in_either_status() {
    let router = emulator();

    let provisioning = Create {
        case: "a store still provisioning",
        name: "deleted_while_provisioning".to_string(),
        comment: Some("deleted before the clock settled it".to_string()),
        tags: Vec::new(),
    };
    let ready = Create {
        case: "a store the clock has settled",
        name: "deleted_once_ready".to_string(),
        comment: Some("deleted after the clock settled it".to_string()),
        tags: Vec::new(),
    };
    let (provisioning_etag, _) = seed(&router, &provisioning).await;
    let (ready_etag, _) = seed(&router, &ready).await;

    // A caller acting straight after the create still observes the transient status.
    let described = describe_key_value_store(&router, &provisioning.name).await;
    let status = member(described.root(), "Status", provisioning.case, &described.raw);
    assert_eq!(
        status, KVS_STATUS_PROVISIONING,
        "case {}: expected the transient status, got {status:?}",
        provisioning.case
    );
    let deleted =
        delete_key_value_store(&router, &provisioning.name, Some(&provisioning_etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case {}: expected HTTP 204, got {} with {}",
        provisioning.case, deleted.status, deleted.raw
    );
    assert_absent(&router, &provisioning.name, provisioning.case).await;

    // The clock owes the other store the terminal status; nothing calls for it.
    tokio::time::sleep(KVS_PROVISIONING_DELAY + Duration::from_millis(250)).await;
    let described = describe_key_value_store(&router, &ready.name).await;
    let status = member(described.root(), "Status", ready.case, &described.raw);
    assert_eq!(
        status, KVS_STATUS_READY,
        "case {}: expected the terminal status, got {status:?}",
        ready.case
    );
    let deleted = delete_key_value_store(&router, &ready.name, Some(&ready_etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case {}: expected HTTP 204, got {} with {}",
        ready.case, deleted.status, deleted.raw
    );
    assert_absent(&router, &ready.name, ready.case).await;
}
