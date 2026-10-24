// SPDX-License-Identifier: CC0-1.0

//! The CloudFront public key control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's public key surface states:
//!
//! * a public key is PERSISTED by the call that uploads it, which MINTS its identity (an
//!   id and a version token) and records the instant it was uploaded, so a later
//!   `GetPublicKey` and every `ListPublicKeys` page answer from that record rather than
//!   from the request being served — down to the members the create left out;
//! * `ListPublicKeys` "lists all public keys that have been added to CloudFront for this
//!   account": one summary per stored key, reporting what the same record answers a get
//!   with;
//! * `MaxItems` cuts the page, `NextMarker` is present exactly when keys remain and names
//!   the id the page ended at, and a request resumes after the key its `Marker` names — so
//!   walking the pages at ANY admissible page size enumerates every key exactly once, in
//!   one order;
//! * a page size that names no page and a `Marker` that is no resume point this service
//!   handed out are refused with the error the operation models (`InvalidArgument` 400),
//!   and change nothing;
//! * `PublicKeyConfig$CallerReference` is "a string included in the request to help make
//!   sure that the request can't be replayed": the same request again reports the key the
//!   first create stored, and a different configuration under a reference already taken is
//!   refused (`PublicKeyAlreadyExists`, 409);
//! * an id no public key holds is `NoSuchPublicKey` (404);
//! * `PublicKeyConfig$Comment`: "The comment cannot be longer than 128 characters", so a
//!   longer comment is refused before anything is stored.
//!
//! The table rows differ along everything these rules generalise over: the form of the
//! encoded key (single-line and multi-line PEM, long and short bodies), whether a comment
//! is carried at all and whether it is empty, ASCII or XML metacharacters, and the
//! published comment bound, one inside it and one past it.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_PUBLIC_KEY_MARKER_MESSAGE, MAX_ITEMS, MAX_PUBLIC_KEY_COMMENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_distribution_id, Answer, Node, API_VERSION, HOST,
};
// The XML escaper is the crate's: one escaper for every suite, so no two of them can
// disagree about how a submitted value is written.
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the public key surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/public-key` with the `PublicKeyConfig` document `body` — the member
/// the operation binds to the HTTP payload.
async fn create_key(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/public-key"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/public-key/{Id}`.
async fn get_key(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/public-key`, with `query` appended verbatim (`?MaxItems=2&Marker=K…`),
/// from the operation's `http` binding.
async fn list_keys(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `PublicKeyConfig`, as a client submits it.
#[derive(Debug, Clone)]
struct Key {
    /// What this row is a case of, named in every assertion it drives.
    case: String,
    caller_reference: String,
    name: String,
    encoded_key: String,
    /// `Comment`, or `None` to leave the optional member out entirely.
    comment: Option<String>,
}

impl Key {
    /// The `PublicKeyConfig` XML a client sends for this configuration, in the model's
    /// member declaration order (`CallerReference`, `Name`, `EncodedKey`, `Comment`).
    fn xml(&self) -> String {
        let mut out = format!(
            "<PublicKeyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!(
            "<CallerReference>{}</CallerReference>",
            escape(&self.caller_reference)
        ));
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        out.push_str(&format!(
            "<EncodedKey>{}</EncodedKey>",
            escape(&self.encoded_key)
        ));
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        out.push_str("</PublicKeyConfig>");
        out
    }

    /// Assert that `config` — a `PublicKeyConfig` element the emulator serialised —
    /// reports exactly this configuration.
    fn assert_matches(&self, config: &Node, context: &str, raw: &str) {
        for (member, expected) in [
            ("CallerReference", self.caller_reference.clone()),
            ("Name", self.name.clone()),
            ("EncodedKey", self.encoded_key.clone()),
        ] {
            let actual = config.text_at(member).map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {} ({context}): expected {member} {expected:?}, got {actual:?} in {raw}",
                self.case
            );
        }
        // An optional member the create left out is reported as the stored record has it:
        // absent, rather than as an empty element invented by the read.
        let actual = config.text_at("Comment").map(str::to_string);
        assert_eq!(
            actual,
            self.comment.clone(),
            "case {} ({context}): expected Comment {:?}, got {actual:?} in {raw}",
            self.case,
            self.comment
        );
    }

    /// Assert that `summary` — a `PublicKeySummary` element of a listing — reports this
    /// configuration under `id`, created at `created_time`.
    fn assert_summary(&self, summary: &Node, id: &str, created_time: &str, raw: &str) {
        for (member, expected) in [
            ("Id", Some(id.to_string())),
            ("Name", Some(self.name.clone())),
            ("CreatedTime", Some(created_time.to_string())),
            ("EncodedKey", Some(self.encoded_key.clone())),
            // `PublicKeySummary$Comment` is optional, exactly as the configuration's is, so
            // a key uploaded without one is summarised without one.
            ("Comment", self.comment.clone()),
        ] {
            let actual = summary.text_at(member).map(str::to_string);
            assert_eq!(
                actual, expected,
                "case {} (summary of {id}): expected {member} {expected:?}, got {actual:?} \
                 in {raw}",
                self.case
            );
        }
    }
}

/// A PEM-encoded public key body of `lines` 64-character lines, written the way a PEM
/// document is: the armour, then the base64 body, then the closing armour.
///
/// The VALUE is immaterial to every rule under test — the service stores the encoded key
/// as submitted and reports it back — so the rows below vary its FORM (one line or many,
/// short body or long) rather than trying to be a particular key. The document carries no
/// trailing newline: the XML text of an element is read with its surrounding whitespace
/// stripped at the request boundary, so a value is compared as the boundary hands it to a
/// handler.
fn pem(lines: usize, filler: char) -> String {
    let body: Vec<String> = (0..lines)
        .map(|index| {
            let mut line: String = std::iter::repeat_n(filler, 63).collect();
            // One differing character per line, so no two lines of the body are equal.
            line.push(char::from(b'A' + u8::try_from(index % 26).unwrap_or(0)));
            line
        })
        .collect();
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        body.join("\n")
    )
}

/// The configurations every case below is run over.
///
/// The rows span the forms the configuration's members take: a single-line and a
/// multi-line encoded key, a long and a short body, and the comment forms (absent, empty,
/// plain ASCII, XML metacharacters, and the published 128-character bound) cycled across
/// them.
fn table() -> Vec<Key> {
    let comments: [Option<String>; 5] = [
        None,
        Some(String::new()),
        Some("the public key for an emulator test".to_string()),
        Some("tags <key> & \"quotes\" and 'apostrophes'".to_string()),
        // "The comment cannot be longer than 128 characters": the bound itself is
        // admissible, so one row sits exactly on it.
        Some("c".repeat(MAX_PUBLIC_KEY_COMMENT)),
    ];
    let keys: [String; 5] = [
        pem(1, 'A'),
        pem(2, 'B'),
        pem(4, 'C'),
        pem(8, 'D'),
        "-----BEGIN PUBLIC KEY-----\nSHORT\n-----END PUBLIC KEY-----".to_string(),
    ];

    keys.iter()
        .enumerate()
        .map(|(index, encoded_key)| Key {
            case: format!(
                "a {}-character encoded key with comment {:?}",
                encoded_key.len(),
                comments[index % comments.len()]
                    .as_deref()
                    .map(|comment| comment.chars().take(12).collect::<String>())
            ),
            caller_reference: format!("public-key-caller-reference-{index}"),
            name: format!("public-key-{index}"),
            encoded_key: encoded_key.clone(),
            comment: comments[index % comments.len()].clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// The ids a listing's page reports, in the order the page carries them.
fn page_ids(answer: &Answer) -> Vec<String> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("PublicKeySummary")
                .into_iter()
                .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `value` has the shape of a public key id CloudFront mints: `K` followed by 13
/// characters of the id alphabet, the shape its own answers show (`K2K8NC4HVFE3MO`).
///
/// Spelled out here rather than taken from the crate, so a changed mint fails this suite.
fn is_public_key_id(value: &str) -> bool {
    match value.strip_prefix('K') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// Whether `value` is an instant written the way this service's wire format writes one:
/// an ISO-8601 date and time at UTC, which is what a restXml `timestamp` carries.
fn is_wire_timestamp(value: &str) -> bool {
    value.contains('T') && value.ends_with('Z') && value.len() >= "2026-01-01T00:00:00Z".len()
}

/// Create every row of `rows` through the API and report `(row, id, created_time)` in
/// creation order.
async fn create_all(router: &MultiServiceRouter, rows: &[Key]) -> Vec<(Key, String, String)> {
    let mut created: Vec<(Key, String, String)> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut etags: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let answer = create_key(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreatePublicKey, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_public_key_id(&id),
            "case {}: expected a minted public key id, got {id:?}",
            row.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {}: the id {id} repeated across creates",
            row.case
        );
        let etag = answer.header("etag").to_string();
        assert!(
            is_distribution_id(&etag),
            "case {}: expected a minted version token in ETag, got {etag:?}",
            row.case
        );
        assert!(
            etags.insert(etag.clone()),
            "case {}: the version token {etag} repeated across creates",
            row.case
        );
        let location = answer.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/public-key/{id}"),
            "case {}: expected the new key's URI in Location, got {location:?}",
            row.case
        );
        let created_time = answer
            .root()
            .text_at("CreatedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&created_time),
            "case {}: expected an ISO-8601 CreatedTime, got {created_time:?} in {}",
            row.case,
            answer.raw
        );
        let config = answer
            .root()
            .at("PublicKeyConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, answer.raw));
        row.assert_matches(config, "CreatePublicKey", &answer.raw);
        created.push((row.clone(), id, created_time));
    }
    created
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_persists_the_configuration_every_read_then_reports() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    for (row, id, created_time) in &created {
        let read = get_key(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected HTTP 200 from GetPublicKey, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {}: expected the stored id {id} in the read, got {}",
            row.case,
            read.raw
        );
        // "The date and time when the public key was uploaded": written once by the
        // create, so a read reports that same instant rather than the instant of the read.
        assert_eq!(
            read.root().text_at("CreatedTime"),
            Some(created_time.as_str()),
            "case {}: expected the stored CreatedTime {created_time}, got {}",
            row.case,
            read.raw
        );
        let config = read
            .root()
            .at("PublicKeyConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
        row.assert_matches(config, "GetPublicKey", &read.raw);
        assert!(
            is_distribution_id(read.header("etag")),
            "case {}: expected a version token in the read's ETag, got {:?}",
            row.case,
            read.header("etag")
        );

        // A read must not change what a later caller sees: the same request answers the
        // same record again, at the same version.
        let again = get_key(&router, id).await;
        assert_eq!(
            again.raw, read.raw,
            "case {}: a repeated read must answer the same record, got {}",
            row.case, again.raw
        );
        assert_eq!(
            again.header("etag"),
            read.header("etag"),
            "case {}: a read must not mint a new version token",
            row.case
        );
    }

    // An id no public key holds exists in no namespace: neither a well-shaped one nor an
    // arbitrary string.
    for id in ["K1AAAAAAAAAAAA", "E1AAAAAAAAAAAA", "not-an-id", "0"] {
        let missing = get_key(&router, id).await;
        assert_eq!(
            missing.status, 404,
            "case {id}: expected HTTP 404 for an id no public key holds, got {} with {}",
            missing.status, missing.raw
        );
        assert_eq!(
            missing.error_code(),
            "NoSuchPublicKey",
            "case {id}: expected NoSuchPublicKey, got {}",
            missing.raw
        );
    }
}

#[tokio::test]
async fn an_account_using_no_public_keys_lists_no_items() {
    let router = emulator();

    let listed = list_keys(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case empty account: expected HTTP 200 from ListPublicKeys, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case empty account: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case empty account: expected the default MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case empty account: an account holding no public keys carries no Items element, \
         got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case empty account: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

#[tokio::test]
async fn a_listing_reports_every_stored_key_and_pages_through_them() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    // One stable total order over the stored keys, which every page indexes into.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id, _)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection,
    // each summary reporting what the stored record answers a get with.
    let listed = list_keys(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        page_ids(&listed),
        order,
        "case whole collection: expected every stored id once, in order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(order.len().to_string().as_str()),
        "case whole collection: expected Quantity {}, got {}",
        order.len(),
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case whole collection: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
    let items = listed
        .root()
        .at("Items")
        .unwrap_or_else(|| panic!("case whole collection: no Items in {}", listed.raw));
    let summaries = items.children_named("PublicKeySummary");
    for ((row, id, created_time), summary) in expected.iter().zip(summaries) {
        row.assert_summary(summary, id, created_time, &listed.raw);
    }

    // Walking the pages at ANY admissible page size enumerates every key exactly once, in
    // that same order: sizes below, at and past the collection size, and one past the
    // server's own ceiling.
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let sizes = [
        1usize,
        2,
        3,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        ceiling + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Marker={value}"),
                None => format!("?MaxItems={size}"),
            };
            let page = list_keys(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {size}: expected HTTP 200 for {query}, got {} with {}",
                page.status, page.raw
            );
            assert_eq!(
                page.root().text_at("MaxItems"),
                Some(size.to_string().as_str()),
                "case page size {size}: expected the requested MaxItems echoed, got {}",
                page.raw
            );
            let ids = page_ids(&page);
            // The page served is the requested size bounded above by the server's own
            // ceiling, and never longer than what remains.
            assert!(
                ids.len() <= size.min(ceiling),
                "case page size {size}: a page served {} items, got {}",
                ids.len(),
                page.raw
            );
            assert_eq!(
                page.root().text_at("Quantity"),
                Some(ids.len().to_string().as_str()),
                "case page size {size}: expected Quantity to state the page's length, got {}",
                page.raw
            );
            walked.extend(ids.iter().cloned());
            let next = page.root().text_at("NextMarker").map(str::to_string);
            // `PublicKeyList` models no `IsTruncated`: the presence of `NextMarker` is the
            // whole of what says more keys remain.
            assert_eq!(
                next.is_some(),
                walked.len() < order.len(),
                "case page size {size}: expected NextMarker present {} after {} of {} \
                 items, got {}",
                walked.len() < order.len(),
                walked.len(),
                order.len(),
                page.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        ids.last().map(String::as_str),
                        "case page size {size}: expected NextMarker to name the id the page \
                         ended at, got {}",
                        page.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case page size {size}: walking the pages must enumerate every public key \
             exactly once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_keys(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_all(&router, &table()[..3]).await;
    let before: Vec<String> = page_ids(&list_keys(&router, "").await);

    let rows = [
        ("a page of no items", "?MaxItems=0", false),
        ("a negative page size", "?MaxItems=-1", false),
        ("a page size that is no integer", "?MaxItems=not-a-number", false),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999",
            false,
        ),
        ("a marker that is no id at all", "?Marker=nope", true),
        ("a marker too short to be an id", "?Marker=K123", true),
        ("a marker in the wrong case", "?Marker=k1aaaaaaaaaaaa", true),
        (
            "a marker too long to be an id",
            "?Marker=K1AAAAAAAAAAAAAAAA",
            true,
        ),
        (
            "a marker from another resource's namespace",
            "?Marker=E1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker carrying characters outside the id alphabet",
            "?Marker=K1AAAA%20AAAAAA%21",
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=0&Marker=K1AAAAAAAAAAAA",
            false,
        ),
    ];
    for (case, query, over_marker) in rows {
        let answer = list_keys(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {query}, got {}",
            answer.raw
        );
        if over_marker {
            // What the refusal states is the CONSTRAINT on the parameter — a marker is a
            // public key id — so every unusable marker is refused with that one sentence
            // rather than with text quoted out of the request.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, INVALID_PUBLIC_KEY_MARKER_MESSAGE,
                "case {case}: expected the marker constraint stated for {query}, got {}",
                answer.raw
            );
        }
    }

    // A well-shaped marker naming no stored key is a resume POSITION, not a refusal: the
    // page it names holds the keys ordered after it.
    let last = before
        .last()
        .cloned()
        .expect("the fixture created some public keys");
    let after_last = list_keys(&router, &format!("?Marker={last}")).await;
    assert_eq!(
        after_last.status, 200,
        "case resume at the last id: expected HTTP 200, got {} with {}",
        after_last.status, after_last.raw
    );
    assert!(
        page_ids(&after_last).is_empty(),
        "case resume at the last id: nothing is ordered after it, got {}",
        after_last.raw
    );

    // Every refusal above left the store exactly as it was.
    let listed = list_keys(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created keys to be the listed ones"
    );
}

#[tokio::test]
async fn a_caller_reference_identifies_one_public_key() {
    let router = emulator();
    let row = Key {
        case: "the key the account already holds".to_string(),
        caller_reference: "public-key-replayed-reference".to_string(),
        name: "public-key-replayed".to_string(),
        encoded_key: pem(2, 'E'),
        comment: Some("the first create's comment".to_string()),
    };
    let created = create_key(&router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    // "A string included in the request to help make sure that the request can't be
    // replayed": the SAME configuration under the same reference reports the key the first
    // create stored, at its current version, and creates nothing.
    let replayed = create_key(&router, &row.xml()).await;
    assert_eq!(
        replayed.status, 201,
        "case {}: expected the replayed create to report the stored key, got {} with {}",
        row.case, replayed.status, replayed.raw
    );
    assert_eq!(
        replayed.root().text_at("Id"),
        Some(id.as_str()),
        "case {}: expected the replayed create to name {id}, got {}",
        row.case,
        replayed.raw
    );
    assert_eq!(
        replayed.header("etag"),
        etag,
        "case {}: expected the replayed create to report the stored version, got {:?}",
        row.case,
        replayed.header("etag")
    );

    // A DIFFERENT configuration under a reference already taken is refused, whichever
    // member differs — and leaves the stored key untouched.
    let variants = [
        Key {
            case: "a different name".to_string(),
            name: "public-key-replayed-renamed".to_string(),
            ..row.clone()
        },
        Key {
            case: "a different encoded key".to_string(),
            encoded_key: pem(3, 'F'),
            ..row.clone()
        },
        Key {
            case: "a different comment".to_string(),
            comment: Some("another comment".to_string()),
            ..row.clone()
        },
        Key {
            case: "no comment at all".to_string(),
            comment: None,
            ..row.clone()
        },
    ];
    for variant in &variants {
        let answer = create_key(&router, &variant.xml()).await;
        assert_eq!(
            answer.status, 409,
            "case {}: expected HTTP 409 under a taken caller reference, got {} with {}",
            variant.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "PublicKeyAlreadyExists",
            "case {}: expected PublicKeyAlreadyExists, got {}",
            variant.case,
            answer.raw
        );
    }

    // The same configuration under a FRESH reference is a second key, with its own id.
    let second = Key {
        case: "the same key under another reference".to_string(),
        caller_reference: "public-key-second-reference".to_string(),
        ..row.clone()
    };
    let answer = create_key(&router, &second.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        second.case, answer.status, answer.raw
    );
    let other = answer
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        other, id,
        "case {}: expected a fresh id, got {other} again",
        second.case
    );

    // The key the conflicts named is untouched, and the account holds exactly the two
    // keys the accepted creates stored.
    let read = get_key(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {}: the refusals must leave the stored key readable, got {} with {}",
        row.case, read.status, read.raw
    );
    let config = read
        .root()
        .at("PublicKeyConfig")
        .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
    row.assert_matches(config, "after a refused repeat", &read.raw);

    let listed = list_keys(&router, "").await;
    let listed_ids: BTreeSet<String> = page_ids(&listed).into_iter().collect();
    assert_eq!(
        listed_ids,
        BTreeSet::from([id.clone(), other.clone()]),
        "case the whole account: expected exactly the created keys, got {}",
        listed.raw
    );
}

#[tokio::test]
async fn a_comment_past_the_documented_bound_is_refused() {
    let router = emulator();

    // "The comment cannot be longer than 128 characters": the bound itself is accepted,
    // one past it is not, and neither is a comment far past it.
    let accepted = Key {
        case: "a comment at the published bound".to_string(),
        caller_reference: "public-key-comment-at-bound".to_string(),
        name: "public-key-comment-at-bound".to_string(),
        encoded_key: pem(1, 'G'),
        comment: Some("a".repeat(MAX_PUBLIC_KEY_COMMENT)),
    };
    let answer = create_key(&router, &accepted.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201 at the bound, got {} with {}",
        accepted.case, answer.status, answer.raw
    );

    for (case, length) in [
        ("one past the published bound", MAX_PUBLIC_KEY_COMMENT + 1),
        ("far past the published bound", MAX_PUBLIC_KEY_COMMENT * 4),
    ] {
        let refused = Key {
            case: case.to_string(),
            caller_reference: format!("public-key-comment-{length}"),
            name: format!("public-key-comment-{length}"),
            encoded_key: pem(1, 'H'),
            comment: Some("b".repeat(length)),
        };
        let answer = create_key(&router, &refused.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for a {length}-character comment, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // The refusals stored nothing: only the accepted key is listed.
    let listed = list_keys(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("1"),
        "case refused comments: expected only the accepted key to be stored, got {}",
        listed.raw
    );
}
