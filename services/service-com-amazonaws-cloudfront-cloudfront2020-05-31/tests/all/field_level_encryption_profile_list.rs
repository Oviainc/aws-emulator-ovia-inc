// SPDX-License-Identifier: CC0-1.0

//! `ListFieldLevelEncryptionProfiles`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's listing surface states:
//!
//! * the operation "request[s] a list of field-level encryption profiles that have been
//!   created in CloudFront for this account": one summary per stored profile, every member
//!   read from the record the create stored — the minted id, the instant it was written, and
//!   the configuration's `Name`, `EncryptionEntities` and `Comment` — rather than from the
//!   request being served, so an account using none of the resource lists nothing;
//! * `MaxItems` cuts the page, `NextMarker` is present exactly when profiles remain and names
//!   the id the page ended at, and a request resumes after the profile its `Marker` names —
//!   so walking the pages at ANY admissible page size enumerates every profile exactly once,
//!   in one order;
//! * a page size that names no page and a `Marker` that is no resume point this service
//!   handed out are refused with the error the operation models (`InvalidArgument`, 400) and
//!   change nothing;
//! * a listing is a READ: the same request answers the same records again.
//!
//! The rows are the profile suite's table, which spans what a summary generalises over (no
//! encryption entity at all, one and several; one field pattern and several, in the forms a
//! pattern takes; and a comment absent, empty, plain, carrying XML metacharacters and exactly
//! on the published bound), and the page sizes span sizes below, at and past the collection
//! size plus one past this emulator's own ceiling.
//!
//! The harness (the router, the request helpers, the element scanner) and the profile
//! configurations are the sibling suites', reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_FIELD_LEVEL_ENCRYPTION_PROFILE_MARKER_MESSAGE, MAX_ITEMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, is_profile_id, is_wire_timestamp, table, upload_keys, Profile,
};

/// `GET /2020-05-31/field-level-encryption-profile`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=P…`), from the operation's `http` binding.
async fn list_profiles(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption-profile{query}"),
        None,
        None,
    )
    .await
}

/// The `FieldLevelEncryptionProfileSummary` elements of one page, in the order they are
/// serialised.
fn page_summaries(page: &Answer) -> Vec<&Node> {
    page.root()
        .at("Items")
        .map(|items| items.children_named("FieldLevelEncryptionProfileSummary"))
        .unwrap_or_default()
}

/// The ids one page reports, in order.
fn page_ids(page: &Answer) -> Vec<String> {
    page_summaries(page)
        .into_iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Assert that `summary` reports exactly what the record created from `row` holds: the minted
/// `id`, the stored `last_modified` instant and the configuration's `Name`,
/// `EncryptionEntities` and `Comment`.
fn assert_summary(row: &Profile, summary: &Node, id: &str, last_modified: &str, raw: &str) {
    for (member, expected) in [
        ("Id", Some(id.to_string())),
        ("LastModifiedTime", Some(last_modified.to_string())),
        ("Name", Some(row.name.clone())),
        // The summary repeats the stored configuration's `Comment`, which a profile created
        // without one carries as the empty comment the service supplies.
        ("Comment", Some(row.effective_comment())),
    ] {
        let actual = summary.text_at(member).map(str::to_string);
        assert_eq!(
            actual, expected,
            "case {:?}: expected the summary's {member} to be {expected:?}, got {actual:?} \
             in {raw}",
            row.case
        );
    }
    let quantity = summary
        .text_at("EncryptionEntities/Quantity")
        .map(str::to_string);
    assert_eq!(
        quantity.as_deref(),
        Some(row.entities_quantity.to_string().as_str()),
        "case {:?}: expected the summary's EncryptionEntities Quantity {}, got {quantity:?} \
         in {raw}",
        row.case,
        row.entities_quantity
    );
    let serialised: Vec<&Node> = summary
        .at("EncryptionEntities/Items")
        .map(|items| items.children_named("EncryptionEntity"))
        .unwrap_or_default();
    assert_eq!(
        serialised.len(),
        row.entities.len(),
        "case {:?}: expected {} encryption entities in the summary, got {} in {raw}",
        row.case,
        row.entities.len(),
        serialised.len()
    );
    for (expected, actual) in row.entities.iter().zip(serialised) {
        expected.assert_matches(actual, &row.case, "the listing's summary", raw);
    }
}

/// Create every row of the profile table through the API and report
/// `(row, id, last_modified)` in creation order.
async fn create_all(router: &MultiServiceRouter) -> Vec<(Profile, String, String)> {
    let keys = upload_keys(router).await;
    let mut created: Vec<(Profile, String, String)> = Vec::new();
    for row in table(&keys) {
        let answer = create_profile(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionProfile, got {} \
             with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_profile_id(&id),
            "case {:?}: expected a minted profile id, got {id:?}",
            row.case
        );
        let last_modified = answer
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&last_modified),
            "case {:?}: expected a wire timestamp for LastModifiedTime, got {last_modified:?}",
            row.case
        );
        created.push((row, id, last_modified));
    }
    created
}

/// An account that has created no profile lists nothing: the wrapper states a length of zero
/// at the default page size, carries no `Items` element at all and names no next page.
#[tokio::test]
async fn an_account_using_no_profiles_lists_no_items() {
    let router = emulator();

    let listed = list_profiles(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case empty account: expected HTTP 200 from ListFieldLevelEncryptionProfiles, got {} \
         with {}",
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
        "case empty account: an account holding no profiles carries no Items element, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case empty account: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

/// The listing reports one summary per stored profile, read from the record the create
/// stored, and walking the pages at any admissible page size enumerates every profile exactly
/// once in one order.
#[tokio::test]
async fn a_listing_reports_every_stored_profile_and_pages_through_them() {
    let router = emulator();
    let created = create_all(&router).await;

    // One stable total order over the stored profiles, which every page indexes into.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id, _)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection, each
    // summary reporting what the stored record holds.
    let listed = list_profiles(&router, "").await;
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
    for ((row, id, last_modified), summary) in expected.iter().zip(page_summaries(&listed)) {
        assert_summary(row, summary, id, last_modified, &listed.raw);
    }

    // Walking the pages at ANY admissible page size enumerates every profile exactly once, in
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
            let page = list_profiles(&router, &query).await;
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
            // `FieldLevelEncryptionProfileList` models no `IsTruncated`: the presence of
            // `NextMarker` is the whole of what says more profiles remain.
            assert_eq!(
                next.is_some(),
                walked.len() < order.len(),
                "case page size {size}: expected NextMarker present {} after {} of {} items, \
                 got {}",
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
            "case page size {size}: walking the pages must enumerate every profile exactly \
             once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_profiles(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

/// A page size that names no page and a `Marker` that is no resume point this service handed
/// out are refused with the error the operation models, and change nothing. A well-shaped
/// marker naming no stored profile is a resume POSITION rather than a refusal.
#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_all(&router).await;
    let before: Vec<String> = page_ids(&list_profiles(&router, "").await);

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
        ("a marker too short to be an id", "?Marker=P123", true),
        ("a marker in the wrong case", "?Marker=p1aaaaaaaaaaaa", true),
        (
            "a marker too long to be an id",
            "?Marker=P1AAAAAAAAAAAAAAAA",
            true,
        ),
        (
            "a marker from another resource's namespace",
            "?Marker=K1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker carrying characters outside the id alphabet",
            "?Marker=P1AAAA%20AAAAAA%21",
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=0&Marker=P1AAAAAAAAAAAA",
            false,
        ),
    ];
    for (case, query, over_marker) in rows {
        let answer = list_profiles(&router, query).await;
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
            // profile id — so every unusable marker is refused with that one sentence rather
            // than with text quoted out of the request.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, INVALID_FIELD_LEVEL_ENCRYPTION_PROFILE_MARKER_MESSAGE,
                "case {case}: expected the marker constraint stated for {query}, got {}",
                answer.raw
            );
        }
    }

    // A well-shaped marker naming no stored profile is a resume POSITION, not a refusal: the
    // page it names holds the profiles ordered after it.
    let last = before
        .last()
        .cloned()
        .expect("the fixture created some profiles");
    let after_last = list_profiles(&router, &format!("?Marker={last}")).await;
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
    let listed = list_profiles(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created profiles to be the listed ones"
    );
}
