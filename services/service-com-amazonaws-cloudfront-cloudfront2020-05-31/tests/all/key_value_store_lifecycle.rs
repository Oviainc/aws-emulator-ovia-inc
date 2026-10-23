// SPDX-License-Identifier: CC0-1.0

//! The CloudFront key value store control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront's key value store
//! surface states:
//!
//! * a key value store is PERSISTED by the call that creates it, which MINTS its
//!   identity (a UUID `Id`, the ARN built from it, and a version token), so a later
//!   `DescribeKeyValueStore` answers from that record rather than from the request being
//!   served — down to a comment the create left out;
//! * "In your account, the key value store names must be unique": a name identifies ONE
//!   store for its whole life, so a repeated name is refused and leaves the store that
//!   holds it untouched;
//! * a store CloudFront has just accepted is not provisioned yet, and the CLOCK — not a
//!   later call — owes it the terminal `READY` status;
//! * a name no store holds is not found, whatever the name.
//!
//! The table rows differ along everything these rules generalise over: the name's
//! length (both published bounds of `KeyValueStoreName`, and one past the upper one),
//! which of the name pattern's character classes it draws from, whether the create names
//! a comment at all, that comment's length and whether it is ASCII, and whether the
//! create carries tags.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    KVS_PROVISIONING_DELAY, KVS_STATUS_PROVISIONING, KVS_STATUS_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_minted_uuid, Answer, Node, ACCOUNT_ID, API_VERSION, HOST,
};

// ---------------------------------------------------------------------------
// Calling the key value store surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/key-value-store` with the request document `body`.
pub(crate) async fn create_key_value_store(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/key-value-store"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/key-value-store/{Name}`.
pub(crate) async fn describe_key_value_store(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/key-value-store/{name}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One `CreateKeyValueStore` request a client submits.
#[derive(Debug, Clone)]
pub(crate) struct Create {
    pub(crate) case: &'static str,
    pub(crate) name: String,
    /// `Comment`, or `None` to leave the optional member out of the document entirely.
    pub(crate) comment: Option<String>,
    /// `Tags.Items`, as `(Key, Value)` pairs.
    pub(crate) tags: Vec<(String, String)>,
}

impl Create {
    /// The `CreateKeyValueStoreRequest` XML a client sends for this create.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<CreateKeyValueStoreRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        if !self.tags.is_empty() {
            out.push_str("<Tags><Items>");
            for (key, value) in &self.tags {
                out.push_str(&format!(
                    "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                    escape(key),
                    escape(value)
                ));
            }
            out.push_str("</Items></Tags>");
        }
        out.push_str("</CreateKeyValueStoreRequest>");
        out
    }

    /// The `Comment` the record must report: the one submitted, or the empty comment
    /// when the create named none — `KeyValueStore$Comment` is required on the wire.
    fn expected_comment(&self) -> &str {
        self.comment.as_deref().unwrap_or("")
    }
}

/// Escape the five entities XML predefines, so a comment may carry any text.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The creates every rule below is asserted over.
///
/// `KeyValueStoreName` is `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$` and
/// `KeyValueStoreComment` is `length 0..128` (both from `smithy_query` on
/// `CreateKeyValueStoreRequest`), so the rows sit at both name bounds, draw from each
/// character class the pattern admits, and cover a missing comment, the empty comment,
/// a non-ASCII comment and the longest comment the model allows.
fn create_table() -> Vec<Create> {
    vec![
        Create {
            case: "the shortest admissible name, no comment",
            name: "a".to_string(),
            comment: None,
            tags: Vec::new(),
        },
        Create {
            case: "the longest admissible name (64), a comment",
            name: "n".repeat(64),
            comment: Some("the longest name this model admits".to_string()),
            tags: Vec::new(),
        },
        Create {
            case: "hyphens and underscores, the empty comment",
            name: "orders-by_region-2".to_string(),
            comment: Some(String::new()),
            tags: Vec::new(),
        },
        Create {
            case: "mixed case and digits, a non-ASCII comment",
            name: "Catalog42".to_string(),
            comment: Some("プロダクション — flags & limits".to_string()),
            tags: Vec::new(),
        },
        Create {
            case: "the longest admissible comment (128), with tags",
            name: "feature_flags".to_string(),
            comment: Some("c".repeat(128)),
            tags: vec![
                ("stage".to_string(), "prod".to_string()),
                ("owner".to_string(), "edge-team".to_string()),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service minted
// ---------------------------------------------------------------------------

/// Whether `value` has the shape of a CloudFront version token (`ETag`): `E` followed by
/// 13 uppercase alphanumerics, the shape this service's own ids and tokens take.
pub(crate) fn is_version_token(value: &str) -> bool {
    match value.strip_prefix('E') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// The text of one member of the `KeyValueStore` document, or a panic naming the case.
pub(crate) fn member(store: &Node, name: &str, case: &str, raw: &str) -> String {
    store
        .text_at(name)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {name}: {raw}"))
        .to_string()
}

/// Assert the `KeyValueStore` document `answer` carries is the record `create` asked
/// for, in `status`, and answer its `(Id, ARN, Comment)`.
fn assert_record(
    answer: &Answer,
    create: &Create,
    status: &str,
    context: &str,
) -> (String, String, String) {
    let case = create.case;
    let store = answer.root();
    let raw = answer.raw.as_str();

    let name = member(store, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    let id = member(store, "Id", case, raw);
    assert!(
        is_minted_uuid(&id),
        "case {case} ({context}): expected a UUID Id, got {id:?}"
    );

    let arn = member(store, "ARN", case, raw);
    assert_eq!(
        arn,
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/{id}"),
        "case {case} ({context}): expected the ARN built from the Id, got {arn:?}"
    );

    let comment = store.text_at("Comment").unwrap_or("").to_string();
    assert_eq!(
        comment,
        create.expected_comment(),
        "case {case} ({context}): expected the submitted Comment, got {comment:?}"
    );

    let actual_status = member(store, "Status", case, raw);
    assert_eq!(
        actual_status, status,
        "case {case} ({context}): expected Status {status}, got {actual_status:?}"
    );

    let last_modified = member(store, "LastModifiedTime", case, raw);
    assert!(
        !last_modified.is_empty(),
        "case {case} ({context}): expected a LastModifiedTime, got {last_modified:?}"
    );

    (id, arn, comment)
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the store it mints, and the describe answers from that one record:
/// the name as submitted, the comment as submitted (empty when none was), a UUID id, the
/// ARN built from that id, and the version token the create handed back. Across the
/// table, no two stores share an id, an ARN or a version token.
#[tokio::test]
async fn create_persists_the_store_the_describe_then_reports() {
    let router = emulator();
    let mut ids = BTreeSet::new();
    let mut arns = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for create in create_table() {
        let case = create.case;
        let created = create_key_value_store(&router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        // A store CloudFront has just accepted is still provisioning.
        let (id, arn, comment) =
            assert_record(&created, &create, KVS_STATUS_PROVISIONING, "the create");

        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/key-value-store/{}", create.name),
            "case {case}: expected the new store's URI in Location, got {location:?}"
        );

        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(arns.insert(arn.clone()), "case {case}: the ARN {arn} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The read answers from the stored record, not from any request.
        let described = describe_key_value_store(&router, &create.name).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected HTTP 200 from the describe, got {} with {}",
            described.status, described.raw
        );
        let (described_id, described_arn, described_comment) =
            assert_record(&described, &create, KVS_STATUS_PROVISIONING, "the describe");
        assert_eq!(
            (described_id, described_arn, described_comment),
            (id, arn, comment),
            "case {case}: the describe must report the record the create stored"
        );
        let described_etag = described.header("etag").to_string();
        assert_eq!(
            described_etag, etag,
            "case {case}: expected the create's version token, got {described_etag:?}"
        );
    }
}

/// "In your account, the key value store names must be unique": the second create under
/// a name is refused with `EntityAlreadyExists` (HTTP 409), and the store that holds the
/// name keeps the identity and the version token its own create minted.
#[tokio::test]
async fn a_name_names_at_most_one_key_value_store() {
    let router = emulator();

    for create in create_table() {
        let case = create.case;
        let first = create_key_value_store(&router, &create.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let id = member(first.root(), "Id", case, &first.raw);
        let etag = first.header("etag").to_string();

        // The same name again — with a comment that differs, so a store that was
        // replaced rather than refused would be visible.
        let repeat = Create {
            comment: Some("a second create under a name already taken".to_string()),
            ..create.clone()
        };
        let second = create_key_value_store(&router, &repeat.xml()).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a repeated name, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "EntityAlreadyExists",
            "case {case}: expected EntityAlreadyExists, got {}",
            second.raw
        );

        let described = describe_key_value_store(&router, &create.name).await;
        let (described_id, _, described_comment) =
            assert_record(&described, &create, KVS_STATUS_PROVISIONING, "after a refusal");
        assert_eq!(
            described_id, id,
            "case {case}: the refused create must not have replaced the store"
        );
        assert_eq!(
            described_comment,
            create.expected_comment(),
            "case {case}: the refused create must not have changed the comment"
        );
        assert_eq!(
            described.header("etag"),
            etag,
            "case {case}: the refused create must not have spent a version token"
        );
    }
}

/// A store CloudFront has just created is `PROVISIONING`, and the CLOCK owes it `READY`:
/// no call asks for the transition, and the transition changes nothing else about the
/// record — not its identity, and not the version token an `If-Match` is checked against.
#[tokio::test]
async fn the_clock_settles_provisioning_to_ready() {
    let router = emulator();
    let create = Create {
        case: "provisioning settles",
        name: "settles_to_ready".to_string(),
        comment: Some("provisioned by the clock".to_string()),
        tags: Vec::new(),
    };

    let created = create_key_value_store(&router, &create.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        create.case, created.status, created.raw
    );
    let (id, arn, _) = assert_record(&created, &create, KVS_STATUS_PROVISIONING, "the create");
    let etag = created.header("etag").to_string();

    // A caller acting straight after the create still observes the transient status.
    let immediate = describe_key_value_store(&router, &create.name).await;
    assert_record(
        &immediate,
        &create,
        KVS_STATUS_PROVISIONING,
        "straight after the create",
    );

    // Once the span has passed, the clock has settled the store — without any call
    // having asked for it.
    tokio::time::sleep(KVS_PROVISIONING_DELAY + Duration::from_millis(250)).await;

    let settled = describe_key_value_store(&router, &create.name).await;
    let (settled_id, settled_arn, _) =
        assert_record(&settled, &create, KVS_STATUS_READY, "after the span");
    assert_eq!(
        (settled_id, settled_arn),
        (id, arn),
        "case {}: provisioning must not change the store's identity",
        create.case
    );
    assert_eq!(
        settled.header("etag"),
        etag,
        "case {}: provisioning is not a mutation, so the version token stands",
        create.case
    );
}

/// A name no store holds is not found — `EntityNotFound` (HTTP 404) — whatever shape the
/// name takes, and a create under one name leaves every other name unfound.
#[tokio::test]
async fn describe_refuses_a_name_no_store_holds() {
    let router = emulator();
    let present = Create {
        case: "the one store that exists",
        name: "present_store".to_string(),
        comment: None,
        tags: Vec::new(),
    };
    let created = create_key_value_store(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );

    // Rows: a plain name, a name differing only in case (store names are compared as
    // given), a name that is a prefix of the one that exists, and the longest name the
    // model admits.
    let absent: [(&str, String); 4] = [
        ("a name never created", "absent_store".to_string()),
        ("the same name in another case", "PRESENT_STORE".to_string()),
        ("a prefix of the existing name", "present".to_string()),
        ("the longest admissible name", "z".repeat(64)),
    ];
    for (case, name) in absent {
        let answer = describe_key_value_store(&router, &name).await;
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
}

/// `KeyValueStoreName` is `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$`: a name outside
/// it is refused before anything is stored, so the name stays unused.
#[tokio::test]
async fn a_name_outside_the_modelled_constraint_is_refused() {
    let router = emulator();
    let cases: [(&str, String); 3] = [
        ("one character past the length bound", "n".repeat(65)),
        ("a character the pattern excludes", "orders.by.region".to_string()),
        ("the empty name", String::new()),
    ];

    for (case, name) in cases {
        let create = Create {
            case: "outside the constraint",
            name: name.clone(),
            comment: None,
            tags: Vec::new(),
        };
        let answer = create_key_value_store(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {name:?}, got {} with {}",
            answer.status, answer.raw
        );
        // Nothing was stored under the refused name.
        if !name.is_empty() {
            let described = describe_key_value_store(&router, &name).await;
            assert_ne!(
                described.status, 200,
                "case {case}: the refused create must store nothing, got {}",
                described.raw
            );
        }
    }
}

/// Importing a store's data from S3 is not implemented here, so a create that names an
/// `ImportSource` answers the scaffold's 501 rather than creating a store whose contents
/// were never imported — and it stores nothing.
#[tokio::test]
async fn a_create_naming_an_import_source_is_not_implemented() {
    let router = emulator();
    let name = "imported_store";
    let body = format!(
        "<CreateKeyValueStoreRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         <ImportSource><SourceType>S3</SourceType>\
         <SourceARN>arn:aws:s3:::import-bucket/data.json</SourceARN></ImportSource>\
         </CreateKeyValueStoreRequest>"
    );

    let answer = create_key_value_store(&router, &body).await;
    assert_eq!(
        answer.status, 501,
        "case an ImportSource: expected HTTP 501, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "NotImplemented",
        "case an ImportSource: expected NotImplemented, got {}",
        answer.raw
    );

    let described = describe_key_value_store(&router, name).await;
    assert_eq!(
        described.status, 404,
        "case an ImportSource: the refused create must store nothing, got {} with {}",
        described.status, described.raw
    );
}
