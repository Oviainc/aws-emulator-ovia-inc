// SPDX-License-Identifier: CC0-1.0

//! `ListCloudFrontOriginAccessIdentities`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this listing:
//!
//! * "Lists origin access identities": one `CloudFrontOriginAccessIdentitySummary` per
//!   stored identity, each reporting what the same record answers
//!   `GetCloudFrontOriginAccessIdentity` with — the minted id, the S3 canonical user id and
//!   "the comment for this origin access identity, as originally specified when created";
//! * `MaxItems` cuts the page and is echoed as asked for, `IsTruncated` and `NextMarker`
//!   say exactly when identities remain and `NextMarker` names the id the page ended at,
//!   and a request resumes AFTER the identity its `Marker` names — so walking the pages at
//!   ANY admissible page size enumerates every identity exactly once, in one order;
//! * a page size that names no page, and a `Marker` that is no resume point this service
//!   handed out, are refused with the error the operation models (`InvalidArgument`, 400)
//!   and change nothing;
//! * a listing is a READ: it answers the same records again and mints nothing.
//!
//! The table rows differ along everything these rules generalise over: the number of
//! identities stored (none, one, several), the page size (one, several, the collection
//! size, past it and past the server's own ceiling), the resume point, and the form of the
//! comment each summary carries (absent-as-empty, ASCII, XML-significant characters,
//! non-ASCII, and the documented 128-character bound).
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, reused rather than copied; the XML escaper is the
//! response headers policy suite's, for the same reason.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_ORIGIN_ACCESS_IDENTITY_MARKER_MESSAGE, MAX_ITEMS,
    MAX_ITEMS_CLOUD_FRONT_ORIGIN_ACCESS_IDENTITIES, MAX_ITEMS_INVALID_MESSAGE,
    MAX_ORIGIN_ACCESS_IDENTITY_COMMENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/origin-access-identity/cloudfront` with the configuration document
/// `body` — the member the create binds to the HTTP payload.
async fn create_identity(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-access-identity/cloudfront/{Id}`.
async fn get_identity(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-access-identity/cloudfront`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=E…`), from the operation's `http` binding.
async fn list_identities(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The identities under test
// ---------------------------------------------------------------------------

/// One `CloudFrontOriginAccessIdentityConfig` a client submits. Both members are required
/// on the wire, so every row carries both.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    caller_reference: String,
    comment: String,
}

impl Row {
    /// The configuration document, which IS the create's request body (its `httpPayload`
    /// member).
    fn xml(&self) -> String {
        format!(
            "<CloudFrontOriginAccessIdentityConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{}</CallerReference>\
             <Comment>{}</Comment>\
             </CloudFrontOriginAccessIdentityConfig>",
            escape(&self.caller_reference),
            escape(&self.comment)
        )
    }
}

/// The identities every rule below is asserted over.
///
/// `CloudFrontOriginAccessIdentityConfig$CallerReference` is "A unique value (for example,
/// a date-time stamp)" and `$Comment` is "A comment to describe the origin access identity.
/// The comment cannot be longer than 128 characters" (both from `smithy_query` on
/// `CloudFrontOriginAccessIdentityConfig`); neither member carries a modelled constraint,
/// so the rows span the forms a caller actually sends and sit at the documented comment
/// bound — the summary's `Comment` is read from this configuration, so its form is what the
/// listing generalises over.
fn table() -> Vec<Row> {
    let comments = [
        String::new(),
        "serves s3://assets".to_string(),
        "comment with \"quotes\" & <angles>".to_string(),
        "プロダクション — 配信元アクセス".to_string(),
        "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT),
    ];
    comments
        .iter()
        .enumerate()
        .map(|(index, comment)| Row {
            case: format!(
                "identity {index} with a {}-character comment",
                comment.chars().count()
            ),
            caller_reference: format!("oai-listing-caller-reference-{index}"),
            comment: comment.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// The `(Id, S3CanonicalUserId, Comment)` of every summary a page reports, in page order.
fn page_summaries(answer: &Answer) -> Vec<(String, String, String)> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("CloudFrontOriginAccessIdentitySummary")
                .into_iter()
                .map(|summary| {
                    (
                        summary.text_at("Id").unwrap_or_default().to_string(),
                        summary
                            .text_at("S3CanonicalUserId")
                            .unwrap_or_default()
                            .to_string(),
                        // An empty comment serialises as an empty element, which the reader
                        // reports as the empty text rather than as a missing member.
                        summary.text_at("Comment").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids a page reports, in the order the page carries them.
fn page_ids(answer: &Answer) -> Vec<String> {
    page_summaries(answer)
        .into_iter()
        .map(|(id, _, _)| id)
        .collect()
}

/// Whether `value` has the shape of the id CloudFront mints for an origin access identity:
/// `E` followed by 13 uppercase alphanumerics, as `CloudFrontOriginAccessIdentity$Id`'s own
/// example (`E74FTE3AJFJ256A`) takes.
///
/// Spelled out here rather than taken from the crate, so a changed mint fails this suite.
fn is_identity_id(value: &str) -> bool {
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

/// The text of one member of a document, or a panic naming the case.
fn member(root: &Node, path: &str, case: &str, raw: &str) -> String {
    root.text_at(path)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {path}: {raw}"))
        .to_string()
}

/// Create every row through the API and report `(row, id, s3_canonical_user_id)` in
/// creation order.
async fn create_all(router: &MultiServiceRouter, rows: &[Row]) -> Vec<(Row, String, String)> {
    let mut created: Vec<(Row, String, String)> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let answer = create_identity(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateCloudFrontOriginAccessIdentity, got {} \
             with {}",
            row.case, answer.status, answer.raw
        );
        let id = member(answer.root(), "Id", &row.case, &answer.raw);
        assert!(
            is_identity_id(&id),
            "case {}: expected a minted identity Id, got {id:?}",
            row.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {}: the id {id} repeated across creates",
            row.case
        );
        let canonical = member(answer.root(), "S3CanonicalUserId", &row.case, &answer.raw);
        created.push((row.clone(), id, canonical));
    }
    created
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An account holding no origin access identities lists none: a page with nothing in it,
/// carrying no `Items` element at all, the default page size, and nothing that says more
/// remain.
#[tokio::test]
async fn an_account_using_no_identities_lists_no_items() {
    let router = emulator();
    let case = "empty account";

    let listed = list_identities(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected HTTP 200 from ListCloudFrontOriginAccessIdentities, got {} \
         with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case {case}: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS_CLOUD_FRONT_ORIGIN_ACCESS_IDENTITIES.to_string().as_str()),
        "case {case}: expected the default MaxItems \
         {MAX_ITEMS_CLOUD_FRONT_ORIGIN_ACCESS_IDENTITIES}, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case {case}: expected IsTruncated false, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case {case}: an account holding no identities carries no Items element, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case {case}: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

/// The listing reports every stored identity — each summary carrying what the same record
/// answers a get with — and walking the pages at any admissible page size enumerates every
/// identity exactly once, in one order.
#[tokio::test]
async fn a_listing_reports_every_stored_identity_and_pages_through_them() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    // One stable total order over the stored identities, which every page indexes into.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id, _)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection.
    let listed = list_identities(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let summaries = page_summaries(&listed);
    assert_eq!(
        summaries
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect::<Vec<String>>(),
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
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case whole collection: nothing remains, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Marker"),
        Some(""),
        "case whole collection: a first page names no marker, got {}",
        listed.raw
    );

    // Each summary reports the record the create persisted — "the comment for this origin
    // access identity, as originally specified when created" — which is what the get
    // answers with too.
    for ((row, id, canonical), (listed_id, listed_canonical, listed_comment)) in
        expected.iter().zip(summaries.iter())
    {
        let case = &row.case;
        assert_eq!(
            listed_id, id,
            "case {case}: expected the stored id {id}, got {listed_id:?}"
        );
        assert_eq!(
            listed_canonical, canonical,
            "case {case}: expected the stored S3CanonicalUserId {canonical}, got \
             {listed_canonical:?}"
        );
        assert_eq!(
            listed_comment, &row.comment,
            "case {case}: expected the comment as originally specified, got \
             {listed_comment:?}"
        );

        let read = get_identity(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the get, got {} with {}",
            read.status, read.raw
        );
        let read_canonical = member(read.root(), "S3CanonicalUserId", case, &read.raw);
        assert_eq!(
            &read_canonical, listed_canonical,
            "case {case}: the summary and the get must answer from one record, got \
             {read_canonical:?}"
        );
        let read_comment = read
            .root()
            .at("CloudFrontOriginAccessIdentityConfig")
            .and_then(|config| config.text_at("Comment"))
            .unwrap_or("")
            .to_string();
        assert_eq!(
            &read_comment, listed_comment,
            "case {case}: the summary's comment must be the stored one, got \
             {read_comment:?}"
        );
    }

    // Walking the pages at ANY admissible page size enumerates every identity exactly
    // once, in that same order: sizes below, at and past the collection size, and one past
    // the server's own ceiling.
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
            let page = list_identities(&router, &query).await;
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
            assert_eq!(
                page.root().text_at("Marker"),
                Some(marker.as_deref().unwrap_or("")),
                "case page size {size}: expected the request's Marker echoed, got {}",
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
            let truncated = walked.len() < order.len();
            assert_eq!(
                page.root().text_at("IsTruncated"),
                Some(if truncated { "true" } else { "false" }),
                "case page size {size}: expected IsTruncated {truncated} after {} of {} \
                 items, got {}",
                walked.len(),
                order.len(),
                page.raw
            );
            let next = page.root().text_at("NextMarker").map(str::to_string);
            assert_eq!(
                next.is_some(),
                truncated,
                "case page size {size}: expected NextMarker present {truncated} after {} of \
                 {} items, got {}",
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
            "case page size {size}: walking the pages must enumerate every identity exactly \
             once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_identities(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

/// A page size that names no page, and a `Marker` that is no resume point this service
/// handed out, are refused with the modelled `InvalidArgument` (HTTP 400) and store
/// nothing; a well-shaped marker naming no stored identity is a resume POSITION rather than
/// a refusal.
#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_all(&router, &table()[..3]).await;
    let before: Vec<String> = page_ids(&list_identities(&router, "").await);

    // Rows: every form a page size can be unusable in (zero, negative, no integer at all,
    // past what the member holds), and every form a marker can fail the id shape in (no id
    // at all, too short, too long, the wrong case, another namespace's token, characters
    // outside the id alphabet) — plus an unusable size beside a usable marker, where the
    // size is what is refused.
    let rows = [
        ("a page of no items", "?MaxItems=0", false),
        ("a negative page size", "?MaxItems=-1", false),
        (
            "a page size that is no integer",
            "?MaxItems=not-a-number",
            false,
        ),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999",
            false,
        ),
        ("a marker that is no id at all", "?Marker=nope", true),
        ("a marker too short to be an id", "?Marker=E123", true),
        (
            "a marker too long to be an id",
            "?Marker=E1AAAAAAAAAAAAAAAA",
            true,
        ),
        ("a marker in the wrong case", "?Marker=e1aaaaaaaaaaaa", true),
        (
            "a marker from another resource's namespace",
            "?Marker=K1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker carrying characters outside the id alphabet",
            "?Marker=E1AAAA%20AAAAAA%21",
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=0&Marker=E1AAAAAAAAAAAA",
            false,
        ),
    ];
    for (case, query, over_marker) in rows {
        let answer = list_identities(&router, query).await;
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
        // What each refusal states is the CONSTRAINT on the parameter — a marker is an
        // origin access identity id, a page size is a non-zero positive integer — so every
        // unusable value is refused with that one sentence rather than with text quoted out
        // of the request.
        let expected_message = if over_marker {
            INVALID_ORIGIN_ACCESS_IDENTITY_MARKER_MESSAGE
        } else {
            MAX_ITEMS_INVALID_MESSAGE
        };
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected_message,
            "case {case}: expected the constraint stated for {query}, got {}",
            answer.raw
        );
    }

    // A well-shaped marker naming no stored identity is a resume POSITION: the page it
    // names holds the identities ordered after it.
    let last = before
        .last()
        .cloned()
        .expect("the fixture created some identities");
    let after_last = list_identities(&router, &format!("?Marker={last}")).await;
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
    assert_eq!(
        after_last.root().text_at("Marker"),
        Some(last.as_str()),
        "case resume at the last id: expected the request's Marker echoed, got {}",
        after_last.raw
    );

    // Every refusal above left the store exactly as it was.
    let listed = list_identities(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created identities to be the listed ones"
    );
}
