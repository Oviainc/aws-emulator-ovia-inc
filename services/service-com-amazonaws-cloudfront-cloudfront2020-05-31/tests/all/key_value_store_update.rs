// SPDX-License-Identifier: CC0-1.0

//! `UpdateKeyValueStore`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test:
//!
//! * an update REPLACES the named store's comment and PERSISTS it, so a later
//!   `DescribeKeyValueStore` answers with what the update stored, whatever text the new
//!   comment is;
//! * the mutation mints a FRESH version token and leaves the store's identity (its name,
//!   `Id` and ARN) alone;
//! * `If-Match` governs the mutation against the STORED version: the store's current
//!   token — bare, quoted or weak, as HTTP entity tags are written — lets the update
//!   land, a well-formed token that is not the current one fails the precondition (412),
//!   and a value that is no version token at all is an invalid `If-Match` version (400);
//! * a name no store holds is not found (404), whatever `If-Match` says;
//! * every refusal changes nothing: neither the comment nor the version token.
//!
//! The table rows differ along what those rules generalise over: the new comment's
//! length (the empty comment and the longest `KeyValueStoreComment` the model admits),
//! whether it is ASCII, whether it carries XML metacharacters, whether the store had a
//! comment at all before the update, whether the new comment equals the old one, and the
//! name's length and character classes.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    KVS_PROVISIONING_DELAY, KVS_STATUS_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::key_value_store_lifecycle::{
    create_key_value_store, describe_key_value_store, escape, is_version_token, member, Create,
};

// ---------------------------------------------------------------------------
// Calling the operation
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/key-value-store/{Name}` with `Comment` in the request document and
/// `if_match` in the `If-Match` header (absent when `None`).
async fn update_key_value_store(
    router: &MultiServiceRouter,
    name: &str,
    comment: &str,
    if_match: Option<&str>,
) -> Answer {
    let body = format!(
        "<UpdateKeyValueStoreRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Comment>{}</Comment></UpdateKeyValueStoreRequest>",
        escape(comment)
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/key-value-store/{name}"),
        Some(&body),
        if_match,
    )
    .await
}

/// Create one store and answer `(its version token, its Id, its ARN)`.
async fn seed(router: &MultiServiceRouter, create: &Create) -> (String, String, String) {
    let created = create_key_value_store(router, &create.xml()).await;
    assert_eq!(
        created.status,
        201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        create.case,
        created.status,
        created.raw
    );
    let raw = created.raw.clone();
    (
        created.header("etag").to_string(),
        member(created.root(), "Id", create.case, &raw),
        member(created.root(), "ARN", create.case, &raw),
    )
}

/// The `Comment` the answer's `KeyValueStore` document carries. An empty comment
/// serializes as an empty element, which the parser reports as empty text.
fn comment_of(answer: &Answer) -> String {
    answer.root().text_at("Comment").unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// The updates under test
// ---------------------------------------------------------------------------

/// One row: the store to create, and the comment the update submits for it.
///
/// `KeyValueStoreComment` is `length 0..128` and `KeyValueStoreName` is
/// `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$` (both from `smithy_query` on
/// `UpdateKeyValueStoreRequest`), so the rows sit at both comment bounds, cover ASCII,
/// non-ASCII and XML-metacharacter text, a store created without a comment, a new comment
/// equal to the old one, and names drawn from each class the pattern admits.
struct Row {
    create: Create,
    new_comment: String,
}

fn update_table() -> Vec<Row> {
    vec![
        Row {
            create: Create {
                case: "the empty comment replaces a non-empty one",
                name: "orders-by_region-2".to_string(),
                comment: Some("staging flags".to_string()),
                tags: Vec::new(),
            },
            new_comment: String::new(),
        },
        Row {
            create: Create {
                case: "a comment replaces the one the create left out",
                name: "a".to_string(),
                comment: None,
                tags: Vec::new(),
            },
            new_comment: "now commented".to_string(),
        },
        Row {
            create: Create {
                case: "the longest admissible comment (128) on the longest name (64)",
                name: "n".repeat(64),
                comment: Some("short".to_string()),
                tags: Vec::new(),
            },
            new_comment: "c".repeat(128),
        },
        Row {
            create: Create {
                case: "a non-ASCII comment",
                name: "Catalog42".to_string(),
                comment: Some("ascii only".to_string()),
                tags: Vec::new(),
            },
            new_comment: "プロダクション — flags & limits".to_string(),
        },
        Row {
            create: Create {
                case: "a comment carrying XML metacharacters",
                name: "feature_flags".to_string(),
                comment: Some("plain".to_string()),
                tags: vec![("stage".to_string(), "prod".to_string())],
            },
            new_comment: "a < b & c > d \"quoted\" 'too'".to_string(),
        },
        Row {
            create: Create {
                case: "the same comment again",
                name: "steady_store".to_string(),
                comment: Some("steady".to_string()),
                tags: Vec::new(),
            },
            new_comment: "steady".to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An update under the store's current version token replaces the comment, persists it —
/// the describe answers from the stored record — mints a fresh token, and leaves the
/// store's identity alone. Across the table, no two tokens repeat.
#[tokio::test]
async fn update_persists_the_comment_the_describe_then_reports() {
    let router = emulator();
    let mut etags = BTreeSet::new();

    for row in update_table() {
        let case = row.create.case;
        let (created_etag, id, arn) = seed(&router, &row.create).await;
        assert!(
            etags.insert(created_etag.clone()),
            "case {case}: the version token {created_etag} repeated"
        );

        let updated =
            update_key_value_store(&router, &row.create.name, &row.new_comment, Some(&created_etag))
                .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            updated.status, updated.raw
        );

        let comment = comment_of(&updated);
        assert_eq!(
            comment, row.new_comment,
            "case {case}: expected the submitted Comment, got {comment:?}"
        );
        let name = member(updated.root(), "Name", case, &updated.raw);
        assert_eq!(
            name, row.create.name,
            "case {case}: an update must not rename the store, got {name:?}"
        );
        let updated_id = member(updated.root(), "Id", case, &updated.raw);
        assert_eq!(
            updated_id, id,
            "case {case}: an update must not re-mint the Id, got {updated_id:?}"
        );
        let updated_arn = member(updated.root(), "ARN", case, &updated.raw);
        assert_eq!(
            updated_arn, arn,
            "case {case}: an update must not re-mint the ARN, got {updated_arn:?}"
        );
        let last_modified = member(updated.root(), "LastModifiedTime", case, &updated.raw);
        assert!(
            !last_modified.is_empty(),
            "case {case}: expected a LastModifiedTime, got {last_modified:?}"
        );

        let etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        assert_ne!(
            etag, created_etag,
            "case {case}: the mutation must mint a fresh version token"
        );
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The read answers from the stored record, not from the request being served.
        let described = describe_key_value_store(&router, &row.create.name).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected HTTP 200 from the describe, got {} with {}",
            described.status, described.raw
        );
        let described_comment = comment_of(&described);
        assert_eq!(
            described_comment, row.new_comment,
            "case {case}: the describe must report the stored Comment, got {described_comment:?}"
        );
        assert_eq!(
            described.header("etag"),
            etag,
            "case {case}: the describe must report the update's version token"
        );
        let described_id = member(described.root(), "Id", case, &described.raw);
        assert_eq!(
            described_id, id,
            "case {case}: the describe must report the stored Id, got {described_id:?}"
        );

        // The token the update replaced is no longer current, so it no longer satisfies
        // the precondition — and the refusal changes nothing.
        let stale = update_key_value_store(
            &router,
            &row.create.name,
            "written under a stale token",
            Some(&created_etag),
        )
        .await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the replaced token, got {} with {}",
            stale.status, stale.raw
        );
        assert_eq!(
            stale.error_code(),
            "PreconditionFailed",
            "case {case}: expected PreconditionFailed, got {}",
            stale.raw
        );
        let after = describe_key_value_store(&router, &row.create.name).await;
        assert_eq!(
            comment_of(&after),
            row.new_comment,
            "case {case}: a refused update must not change the comment, got {}",
            after.raw
        );
        assert_eq!(
            after.header("etag"),
            etag,
            "case {case}: a refused update must not spend a version token"
        );
    }
}

/// `If-Match` is compared against the store's CURRENT version: the token bare, quoted or
/// weak lets the update land; a well-formed token that is not the current one fails the
/// precondition; a value that is no version token at all is an invalid `If-Match`
/// version. Each accepted form is checked against the token the store holds at that
/// moment, and each refusal must leave the comment and the token untouched.
#[tokio::test]
async fn if_match_governs_the_update_against_the_stored_version() {
    let router = emulator();

    // A second store, so "another store's current token" is a real token this service
    // minted rather than one invented by the test.
    let other = Create {
        case: "the other store",
        name: "other_store".to_string(),
        comment: Some("another store entirely".to_string()),
        tags: Vec::new(),
    };
    let (other_etag, _, _) = seed(&router, &other).await;

    let subject = Create {
        case: "the store under test",
        name: "governed_store".to_string(),
        comment: Some("the comment the create stored".to_string()),
        tags: Vec::new(),
    };
    let (first_etag, _, _) = seed(&router, &subject).await;

    // How the current token is written in the header. All three are the same entity tag:
    // CloudFront reports it bare, and HTTP writes one quoted, optionally weak.
    type HeaderForm = (&'static str, fn(&str) -> String);
    let accepted: [HeaderForm; 3] = [
        ("the current token, bare", |etag| etag.to_string()),
        ("the current token, quoted", |etag| format!("\"{etag}\"")),
        ("the current token, weak", |etag| format!("W/\"{etag}\"")),
    ];
    let mut current = first_etag.clone();
    for (index, (case, render)) in accepted.iter().enumerate() {
        let comment = format!("accepted form {index}");
        let answer =
            update_key_value_store(&router, &subject.name, &comment, Some(&render(&current))).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            comment_of(&answer),
            comment,
            "case {case}: expected the submitted Comment, got {}",
            answer.raw
        );
        let next = answer.header("etag").to_string();
        assert_ne!(
            next, current,
            "case {case}: the mutation must mint a fresh version token"
        );
        current = next;
    }

    // The comment the store holds going into the refusals, which every one of them must
    // leave in place.
    let settled = describe_key_value_store(&router, &subject.name).await;
    let held_comment = comment_of(&settled);
    assert_eq!(
        settled.header("etag"),
        current,
        "case the refusals: expected the last update's token, got {}",
        settled.raw
    );

    // (case, the If-Match header, expected status, expected error code). A version token
    // is `E` + 13 uppercase alphanumerics, so rows 1-3 are well-formed tokens that are
    // not this store's current one, and rows 4-7 are not tokens at all.
    let refusals: &[(&str, Option<&str>, u16, &str)] = &[
        (
            "the token this store had before its first update",
            Some(&first_etag),
            412,
            "PreconditionFailed",
        ),
        (
            "another store's current token",
            Some(&other_etag),
            412,
            "PreconditionFailed",
        ),
        (
            "a well-formed token this service never minted",
            Some("EZZZZZZZZZZZZZ"),
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
        (
            "a token in lowercase",
            Some("e2qwruhapomqz"),
            400,
            "InvalidIfMatchVersion",
        ),
        ("the empty header", Some(""), 400, "InvalidIfMatchVersion"),
    ];

    for (case, if_match, status, code) in refusals {
        let answer = update_key_value_store(
            &router,
            &subject.name,
            "written under a refused precondition",
            *if_match,
        )
        .await;
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

        let after = describe_key_value_store(&router, &subject.name).await;
        assert_eq!(
            comment_of(&after),
            held_comment,
            "case {case}: a refused update must not change the comment, got {}",
            after.raw
        );
        assert_eq!(
            after.header("etag"),
            current,
            "case {case}: a refused update must not spend a version token"
        );
    }
}

/// A name no store holds names nothing to update — `EntityNotFound` (HTTP 404) — whatever
/// shape the name takes and whatever `If-Match` carries, and the store that does exist is
/// left alone.
#[tokio::test]
async fn update_refuses_a_name_no_store_holds() {
    let router = emulator();
    let present = Create {
        case: "the one store that exists",
        name: "present_store".to_string(),
        comment: Some("the stored comment".to_string()),
        tags: Vec::new(),
    };
    let (etag, _, _) = seed(&router, &present).await;

    // Rows: a name never created, the same name in another case (names are compared as
    // given), a prefix of the name that exists, and the longest name the model admits —
    // each with the existing store's current token, so the lookup, not the precondition,
    // is what refuses them.
    let absent: [(&str, String); 4] = [
        ("a name never created", "absent_store".to_string()),
        ("the same name in another case", "PRESENT_STORE".to_string()),
        ("a prefix of the existing name", "present".to_string()),
        ("the longest admissible name", "z".repeat(64)),
    ];
    for (case, name) in absent {
        let answer = update_key_value_store(&router, &name, "a comment", Some(&etag)).await;
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
    }

    let described = describe_key_value_store(&router, &present.name).await;
    assert_eq!(
        comment_of(&described),
        present.comment.clone().unwrap_or_default(),
        "case the store that exists: a refused update must not change it, got {}",
        described.raw
    );
    assert_eq!(
        described.header("etag"),
        etag,
        "case the store that exists: a refused update must not spend a version token"
    );
}

/// Changing a comment is not a re-provisioning: a store the clock has already settled to
/// `READY` stays `READY` across an update, so the status an update reports is the one the
/// store holds rather than the transient one a create reports.
#[tokio::test]
async fn an_update_does_not_reprovision_the_store() {
    let router = emulator();
    let create = Create {
        case: "an update after provisioning",
        name: "ready_then_updated".to_string(),
        comment: Some("provisioned, then updated".to_string()),
        tags: Vec::new(),
    };
    let (etag, id, _) = seed(&router, &create).await;

    // The clock owes the terminal status; nothing calls for it.
    tokio::time::sleep(KVS_PROVISIONING_DELAY + Duration::from_millis(250)).await;

    let updated = update_key_value_store(&router, &create.name, "updated once ready", Some(&etag))
        .await;
    assert_eq!(
        updated.status, 200,
        "case {}: expected HTTP 200, got {} with {}",
        create.case, updated.status, updated.raw
    );
    let status = member(updated.root(), "Status", create.case, &updated.raw);
    assert_eq!(
        status, KVS_STATUS_READY,
        "case {}: an update must not re-provision the store, got {status:?}",
        create.case
    );
    let updated_id = member(updated.root(), "Id", create.case, &updated.raw);
    assert_eq!(
        updated_id, id,
        "case {}: an update must not re-mint the Id, got {updated_id:?}",
        create.case
    );

    let described = describe_key_value_store(&router, &create.name).await;
    let described_status = member(described.root(), "Status", create.case, &described.raw);
    assert_eq!(
        described_status, KVS_STATUS_READY,
        "case {}: the describe must report the stored Status, got {described_status:?}",
        create.case
    );
    assert_eq!(
        comment_of(&described),
        "updated once ready",
        "case {}: the describe must report the stored Comment, got {}",
        create.case,
        described.raw
    );
}
