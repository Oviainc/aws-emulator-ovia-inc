// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SDK type collection: `GetSdkTypes` and `GetSdkType`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`] and friends are this crate's one
//! wire-test harness, reused from `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs rather than one recorded call:
//!
//! * the collection is the service's published catalog, so EVERY entry is listed, in the
//!   catalog's order, and every entry reads back member for member through the single
//!   reader — the rows span the whole catalog, including the types the issue cited none
//!   of (`swift`, `ruby`), a type with several configuration properties, a type with one
//!   and a type with none; what each row pins is what the model fixes (the id, the order,
//!   each configuration property's name and whether it is required), while the prose
//!   members are asserted to be reported and to agree between the two readers;
//! * a page is the slice `position` and `limit` name, over page sizes across the
//!   documented `1..=500` range, each published bound and one past each, and over every
//!   cursor the sequence holds;
//! * an id the catalog does not hold is the modelled 404, over the ways an id can miss
//!   the catalog, and a cursor the sequence does not hold is the modelled 400;
//! * both reads are reporting operations: repeating them changes nothing a later caller
//!   sees.

use serde_json::Value;

use crate::rest_api_lifecycle::{assert_failure, page_items, string_member, Answer, Gateway};

/// One expected catalog entry: its id and the `(name, required)` configuration
/// properties it reports, in order — what the model fixes. The `friendlyName` and the
/// descriptions are prose the model does not state, so no literal for them is restated
/// here; they are asserted to be reported and to agree between the listing and the
/// single read.
type CatalogRow = (&'static str, &'static [(&'static str, bool)]);

/// The SDK types API Gateway can generate a client SDK for, written out here rather than
/// read from the service, so this test checks the catalog instead of restating it.
///
/// Source: this service's model, which declares no shape enumerating the types but
/// documents them on the operation that consumes an id —
/// `smithy_query(query="doc", shape="GetSdkRequest", member="sdkType")` ("Currently
/// `java`, `javascript`, `android`, `objectivec` (for iOS), `swift` (for iOS), and
/// `ruby` are supported"), which fixes the ids and the order, and
/// `smithy_query(query="doc", shape="GetSdkRequest", member="parameters")`, which names
/// the parameters each type REQUIRES ("For `sdkType` of `objectivec` or `swift`, a
/// parameter named `classPrefix` is required. For `sdkType` of `android`, parameters
/// named `groupId`, `artifactId`, `artifactVersion`, and `invokerPackage` are required.
/// For `sdkType` of `java`, parameters named `serviceName` and `javaPackageName` are
/// required.").
const CATALOG: &[CatalogRow] = &[
    ("java", &[("serviceName", true), ("javaPackageName", true)]),
    ("javascript", &[]),
    (
        "android",
        &[
            ("groupId", true),
            ("artifactId", true),
            ("artifactVersion", true),
            ("invokerPackage", true),
        ],
    ),
    ("objectivec", &[("classPrefix", true)]),
    ("swift", &[("classPrefix", true)]),
    ("ruby", &[]),
];

/// Every catalog id, in the catalog's order: the sequence a page is cut out of.
fn catalog_ids() -> Vec<&'static str> {
    CATALOG.iter().map(|(id, ..)| *id).collect()
}

/// `GetSdkTypes` with the query string `query` (empty for none).
async fn get_sdk_types(gateway: &Gateway, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/sdktypes".to_string()
    } else {
        format!("/sdktypes?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `GetSdkType` for `id`.
async fn get_sdk_type(gateway: &Gateway, id: &str) -> Answer {
    gateway.call("GET", &format!("/sdktypes/{id}"), None).await
}

/// The ids of a listed page, in the order the page reports them.
fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// Assert one listed or read SDK type is the catalog row `expected`, member for member.
fn assert_catalog_entry(entry: &Value, expected: &CatalogRow, case: &str) {
    let (id, properties) = expected;
    assert_eq!(
        entry["id"].as_str(),
        Some(*id),
        "case {case}: expected the id {id:?}, got {entry:?}"
    );
    // The friendly name and the description are prose the model does not state, so what
    // is asserted is that each member is reported and says something; which sentence it
    // carries is not a rule this test can pin.
    let friendly_name = entry["friendlyName"].as_str().unwrap_or_default();
    assert!(
        !friendly_name.is_empty(),
        "case {case}: expected a non-empty friendly name, got {entry:?}"
    );
    let description = entry["description"].as_str().unwrap_or_default();
    assert!(
        !description.is_empty(),
        "case {case}: expected a non-empty description, got {entry:?}"
    );

    // `configurationProperties` is always reported, and empty for a type the
    // documentation names no parameter for.
    let reported = entry["configurationProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a configurationProperties array, got {entry:?}"));
    assert_eq!(
        reported.len(),
        properties.len(),
        "case {case}: expected {} configuration properties, got {reported:?}",
        properties.len()
    );
    for (property, (name, required)) in reported.iter().zip(properties.iter()) {
        assert_eq!(
            property["name"].as_str(),
            Some(*name),
            "case {case}: expected the configuration property {name:?}, got {property:?}"
        );
        assert_eq!(
            property["required"].as_bool(),
            Some(*required),
            "case {case}: expected required={required} on {name:?}, got {property:?}"
        );
        let property_friendly_name = property["friendlyName"].as_str().unwrap_or_default();
        assert!(
            !property_friendly_name.is_empty(),
            "case {case}: expected a friendly name on {name:?}, got {property:?}"
        );
        let property_description = property["description"].as_str().unwrap_or_default();
        assert!(
            !property_description.is_empty(),
            "case {case}: expected a description on {name:?}, got {property:?}"
        );
    }
}

#[tokio::test]
async fn every_sdk_type_of_the_catalog_is_listed_and_reads_back_through_get_sdk_type() {
    let gateway = Gateway::new();

    // The whole collection fits in one page: the documented default page size is 25 and
    // the catalog is smaller, so a request carrying no `limit` lists all of it.
    let all = get_sdk_types(&gateway, "").await;
    assert_eq!(
        all.status, 200,
        "case GetSdkTypes(no limit): expected 200, got {} with {:?}",
        all.status, all.body
    );
    let items = page_items(&all.body, "GetSdkTypes(no limit)");
    assert_eq!(
        items.len(),
        CATALOG.len(),
        "case GetSdkTypes(no limit): expected the whole catalog of {} types, got {:?}",
        CATALOG.len(),
        all.body
    );
    assert_eq!(
        listed_ids(&all.body, "GetSdkTypes(no limit)"),
        catalog_ids(),
        "case GetSdkTypes(no limit): expected the catalog's order, got {:?}",
        all.body
    );

    for (row, entry) in CATALOG.iter().zip(items.iter()) {
        let (id, ..) = row;
        let case = format!("GetSdkTypes entry {id}");
        assert_catalog_entry(entry, row, &case);

        // Round trip: the single reader reports the listed record, member for member.
        let read = get_sdk_type(&gateway, id).await;
        assert_eq!(
            read.status, 200,
            "case GetSdkType({id}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_catalog_entry(&read.body, row, &format!("GetSdkType({id})"));
        assert_eq!(
            &read.body, entry,
            "case GetSdkType({id}): expected the listed record {entry:?}, got {:?}",
            read.body
        );
    }

    // Reading the collection does not change what a later caller sees: the listing and
    // the single reads above leave the same catalog behind.
    let again = get_sdk_types(&gateway, "").await;
    assert_eq!(
        again.body, all.body,
        "case GetSdkTypes repeated: a read must not change the collection, got {:?} after {:?}",
        again.body, all.body
    );
}

#[tokio::test]
async fn a_page_of_sdk_types_is_the_slice_the_position_and_limit_name() {
    let gateway = Gateway::new();
    let ids = catalog_ids();

    // Rows span the documented page-size range (default 25, maximum 500), each published
    // bound and one past each — a size below 1 and a size above 500 are clamped into the
    // range rather than refused — crossed with every cursor the sequence holds, which
    // includes no cursor at all and the last id.
    let limits = [0, 1, 2, 3, 6, 7, 25, 500, 501, 10_000];
    for limit in limits {
        let effective = usize::try_from(limit.clamp(1, 500)).expect("the clamped limit is positive");
        for (index, cursor) in std::iter::once(None)
            .chain(ids.iter().map(|id| Some(*id)))
            .enumerate()
        {
            // No cursor starts at the first id; a cursor is the id its page starts at.
            let start = index.saturating_sub(1);
            let expected: Vec<&str> = ids
                .iter()
                .skip(start)
                .take(effective)
                .copied()
                .collect();
            let query = match cursor {
                None => format!("limit={limit}"),
                Some(id) => format!("limit={limit}&position={id}"),
            };
            let case = format!("GetSdkTypes({query})");
            let answer = get_sdk_types(&gateway, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_ids(&answer.body, &case),
                expected,
                "case {case}: expected the page {expected:?}, got {:?}",
                answer.body
            );
        }
    }

    // The collection carries no cursor member of its own: the modelled `SdkTypes` output
    // declares `items` alone, so a page reports no `position` even when more types follow
    // it.
    let first = get_sdk_types(&gateway, "limit=1").await;
    assert!(
        first.body["position"].is_null(),
        "case GetSdkTypes(limit=1): this collection reports no position, got {:?}",
        first.body["position"]
    );

    // Walking the catalog one page at a time with the caller's own cursor reassembles the
    // catalog's order, with no entry listed twice.
    let mut seen: Vec<String> = Vec::new();
    for chunk in ids.chunks(2) {
        let query = format!("limit=2&position={}", chunk[0]);
        let case = format!("GetSdkTypes({query})");
        let answer = get_sdk_types(&gateway, &query).await;
        seen.extend(listed_ids(&answer.body, &case));
    }
    assert_eq!(
        seen,
        ids.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
        "case GetSdkTypes paged in twos: expected the catalog's order, got {seen:?}"
    );
}

#[tokio::test]
async fn an_id_the_catalog_does_not_hold_is_a_not_found_and_an_unknown_cursor_is_a_bad_request() {
    let gateway = Gateway::new();

    // The ways an id can miss the catalog: a declared id in the wrong case, one with a
    // trailing character, a plausible other spelling of a declared id, a language the
    // service does not generate, and an id shaped like one of this service's minted
    // resource ids.
    let unknown = [
        "JAVA",
        "java-",
        "ios-swift",
        "typescript",
        "zzzzzzzzzz",
        "objective-c",
    ];
    for id in unknown {
        let case = format!("GetSdkType({id})");
        let answer = get_sdk_type(&gateway, id).await;
        assert_failure(&answer, 404, "NotFoundException", &case);
        // The sentence is built from the request: it names the identifier that could not
        // be resolved.
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(id),
            "case {case}: expected the refusal to name {id:?}, got {message:?}"
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page — in
    // the ways a cursor can miss: an unknown word, a declared id in the wrong case, and a
    // declared id with a trailing character.
    for cursor in ["nosuchcursor", "JAVA", "java-"] {
        let query = format!("position={cursor}");
        assert_failure(
            &get_sdk_types(&gateway, &query).await,
            400,
            "BadRequestException",
            &format!("GetSdkTypes({query})"),
        );
    }

    // None of the refusals changed the collection.
    let listed = get_sdk_types(&gateway, "").await;
    assert_eq!(
        listed_ids(&listed.body, "GetSdkTypes after the refusals"),
        catalog_ids(),
        "case GetSdkTypes after the refusals: expected the catalog intact, got {:?}",
        listed.body
    );
}
