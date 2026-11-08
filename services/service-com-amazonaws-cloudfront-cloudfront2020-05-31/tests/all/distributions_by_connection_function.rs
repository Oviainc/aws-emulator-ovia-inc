// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByConnectionFunction`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under the connection function its STORED
//! configuration attaches through `ConnectionFunctionAssociation` — whose `Id` is the
//! connection function's own `Id` — and under no other, so a distribution that attaches
//! none is listed by no connection function at all and an edit to the stored configuration
//! moves the distribution between the listings. The connection function the request names
//! is resolved the way every read of that resource kind resolves one (by id, by the name it
//! was created under, or by its ARN), an identifier the store holds none for is
//! `EntityNotFound` rather than an empty page, and each listing pages over its own matching
//! set with `Marker` and `MaxItems` exactly as the unfiltered listing does.
//!
//! The rows below span what the rule generalises over: the number of distributions attached
//! to one connection function (none, one, several, so a page can cut inside the set), a
//! distribution that attaches nothing, each of the three identifier forms the resource
//! answers to, every admissible page size, and the attachment reached both by the create
//! and by a later update. The wire harness (the router, the request helpers and the element
//! scanner) is the one [`crate::distribution_lifecycle`] already builds, and the connection
//! function create is the one [`crate::connection_function_lifecycle`] owns.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ITEMS;
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_lifecycle::{create_connection_function, Create};
use crate::distribution_lifecycle::{
    call, create_distribution, emulator, is_distribution_id, list_distributions,
    update_distribution, Answer, Node, API_VERSION,
};

/// A connection function this service holds: the three identifiers it answers to.
#[derive(Debug, Clone)]
struct Function {
    name: String,
    id: String,
    arn: String,
}

impl Function {
    /// The three forms the identifier of one connection function takes, each labelled, so
    /// every row below is asserted over all of them.
    fn identifiers(&self) -> [(&'static str, &str); 3] {
        [
            ("by-id", self.id.as_str()),
            ("by-name", self.name.as_str()),
            ("by-arn", self.arn.as_str()),
        ]
    }
}

/// Create one connection function and answer the identity its create minted.
async fn connection_function(router: &MultiServiceRouter, name: &str) -> Function {
    let create = Create {
        case: "a connection function the listing is asked about",
        name: name.to_string(),
        comment: format!("the {name} connection function"),
        runtime: "cloudfront-js-2.0",
        code: b"function handler(event) { return event.request; }".to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    };
    let answer = create_connection_function(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected HTTP 201 from CreateConnectionFunction, got {} with {}",
        answer.status, answer.raw
    );
    let summary = answer.root();
    let id = summary
        .text_at("Id")
        .unwrap_or_else(|| panic!("case {name}: the create carries an Id: {}", answer.raw))
        .to_string();
    let arn = summary
        .text_at("ConnectionFunctionArn")
        .unwrap_or_else(|| {
            panic!(
                "case {name}: the create carries a ConnectionFunctionArn: {}",
                answer.raw
            )
        })
        .to_string();
    Function {
        name: name.to_string(),
        id,
        arn,
    }
}

/// The `DistributionConfig` a row submits: one origin and a default cache behaviour, plus
/// the `ConnectionFunctionAssociation` the row attaches, when it attaches one.
pub(crate) fn config_xml(caller_reference: &str, attached: Option<&str>) -> String {
    let association = match attached {
        Some(id) => format!(
            "<ConnectionFunctionAssociation><Id>{id}</Id></ConnectionFunctionAssociation>"
        ),
        None => String::new(),
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled>{association}\
         </DistributionConfig>"
    )
}

/// One `ListDistributionsByConnectionFunction` call: `GET
/// /2020-05-31/distributionsByConnectionFunction` with the identifier and the paging query
/// the caller names.
///
/// `pub(crate)` so the suite that asserts the wording of the connection function refusals
/// reaches this operation through this one call path rather than a second copy of it.
pub(crate) async fn list_by_function(
    router: &MultiServiceRouter,
    identifier: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/distributionsByConnectionFunction\
             ?ConnectionFunctionIdentifier={identifier}{query}"
        ),
        None,
        None,
    )
    .await
}

/// The `DistributionSummary` elements of a `DistributionList` answer, in document order.
fn summaries(list: &Node) -> Vec<&Node> {
    list.child("Items")
        .map(|items| items.children_named("DistributionSummary"))
        .unwrap_or_default()
}

/// The ids of a `DistributionList` answer, in document order.
fn listed_ids(list: &Node) -> Vec<String> {
    summaries(list)
        .into_iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// One distribution a row creates: the case it stands for, and which of the created
/// connection functions (an index into the fixture's functions) its configuration attaches.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// An index into the fixture's connection functions, or `None` for a configuration that
    /// attaches none.
    attaches: Option<usize>,
}

/// Several distributions on the first connection function so a page can cut inside its set,
/// exactly one on the second, nothing on the third, and one distribution attaching nothing.
const ROWS: &[Row] = &[
    Row {
        case: "first-on-the-first-function",
        attaches: Some(0),
    },
    Row {
        case: "second-on-the-first-function",
        attaches: Some(0),
    },
    Row {
        case: "third-on-the-first-function",
        attaches: Some(0),
    },
    Row {
        case: "only-one-on-the-second-function",
        attaches: Some(1),
    },
    Row {
        case: "no-connection-function-at-all",
        attaches: None,
    },
];

/// What one emulator the rows were created in holds: the connection functions, and each
/// created distribution with the function index it attaches and its current version token.
struct Fixture {
    functions: Vec<Function>,
    /// `(case, attaches, id, etag)` per created distribution.
    created: Vec<(&'static str, Option<usize>, String, String)>,
}

impl Fixture {
    /// The ids of the distributions attached to the connection function `index` names, in
    /// id order — what the listing for that function must report.
    fn expected(&self, index: usize) -> Vec<String> {
        let mut ids: Vec<String> = self
            .created
            .iter()
            .filter(|(_, attaches, _, _)| *attaches == Some(index))
            .map(|(_, _, id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }
}

/// Create three connection functions and every row's distribution.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let mut functions = Vec::new();
    for name in ["first_function", "second_function", "third_function"] {
        functions.push(connection_function(router, name).await);
    }

    let mut created = Vec::new();
    for row in ROWS {
        let attached = row.attaches.map(|index| functions[index].id.as_str());
        let body = config_xml(&format!("ref-{}", row.case), attached);
        let answer = create_distribution(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&id),
            "case {}: expected a minted distribution id, got {id:?}",
            row.case
        );
        let etag = answer.header("etag").to_string();
        created.push((row.case, row.attaches, id, etag));
    }

    Fixture { functions, created }
}

/// Walk every page of one connection function's listing, returning the ids in the order
/// they were served.
async fn walk(
    router: &MultiServiceRouter,
    identifier: &str,
    label: &str,
    page_size: Option<i32>,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    // At most one page per distribution plus one, so a listing that never stops truncating
    // fails here rather than looping forever.
    for page in 0..=ROWS.len() + 1 {
        let query = match (page_size, marker.as_deref()) {
            (None, None) => String::new(),
            (None, Some(value)) => format!("&Marker={value}"),
            (Some(size), None) => format!("&MaxItems={size}"),
            (Some(size), Some(value)) => format!("&MaxItems={size}&Marker={value}"),
        };
        let answer = list_by_function(router, identifier, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {label} page {page}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        assert_eq!(
            list.text_at("Marker"),
            Some(marker.as_deref().unwrap_or_default()),
            "case {label} page {page}: expected the Marker the request carried, got {:?}",
            list.text_at("Marker")
        );
        // "The value you provided for the MaxItems request parameter."
        assert_eq!(
            list.text_at("MaxItems"),
            Some(page_size.unwrap_or(MAX_ITEMS).to_string().as_str()),
            "case {label} page {page}: expected MaxItems to echo the page size, got {:?}",
            list.text_at("MaxItems")
        );
        let ids = listed_ids(list);
        assert_eq!(
            list.text_at("Quantity"),
            Some(ids.len().to_string().as_str()),
            "case {label} page {page}: expected Quantity to state the items served ({}), got {:?}",
            ids.len(),
            list.text_at("Quantity")
        );
        if let Some(size) = page_size {
            assert!(
                i32::try_from(ids.len()).unwrap_or(i32::MAX) <= size.min(MAX_ITEMS),
                "case {label} page {page}: expected at most {} items, got {}",
                size.min(MAX_ITEMS),
                ids.len()
            );
        }
        collected.extend(ids.iter().cloned());

        let next_marker = list.text_at("NextMarker").map(str::to_string);
        let is_truncated = list.text_at("IsTruncated").unwrap_or_default().to_string();
        match next_marker {
            Some(next) => {
                assert_eq!(
                    is_truncated, "true",
                    "case {label} page {page}: a NextMarker means IsTruncated true, got \
                     {is_truncated:?}"
                );
                assert_eq!(
                    ids.last().map(String::as_str),
                    Some(next.as_str()),
                    "case {label} page {page}: expected NextMarker to be the id this page ended \
                     at, got {next:?}"
                );
                marker = Some(next);
            }
            None => {
                assert_eq!(
                    is_truncated, "false",
                    "case {label} page {page}: no NextMarker means IsTruncated false, got \
                     {is_truncated:?}"
                );
                return collected;
            }
        }
    }
    panic!("case {label}: the listing never stopped truncating");
}

/// Each distribution is listed under the connection function its configuration attaches,
/// under every identifier form that function answers to, and the summaries are the stored
/// records the unfiltered listing reports.
#[tokio::test]
async fn each_distribution_is_listed_under_the_connection_function_it_attaches() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let unfiltered = list_distributions(&router, "").await;

    for (index, function) in fixture.functions.iter().enumerate() {
        let expected = fixture.expected(index);
        for (form, identifier) in function.identifiers() {
            let label = format!("{}/{form}", function.name);
            let answer = list_by_function(&router, identifier, "").await;
            assert_eq!(
                answer.status, 200,
                "case {label}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let list = answer.root();
            let ids = listed_ids(list);
            assert_eq!(
                ids, expected,
                "case {label}: expected the distributions attached to this connection function \
                 in id order, got {ids:?}"
            );

            // The summaries are the stored records, not something this listing invented:
            // each one matches the summary the unfiltered listing reports for the same id.
            for id in &ids {
                let filtered_summary = summaries(list)
                    .into_iter()
                    .find(|summary| summary.text_at("Id") == Some(id.as_str()))
                    .cloned()
                    .unwrap_or_default();
                let plain_summary = summaries(unfiltered.root())
                    .into_iter()
                    .find(|summary| summary.text_at("Id") == Some(id.as_str()))
                    .cloned()
                    .unwrap_or_default();
                for member in ["ARN", "Status", "DomainName", "ETag", "Comment"] {
                    assert_eq!(
                        filtered_summary.text_at(member),
                        plain_summary.text_at(member),
                        "case {label} {id}: expected {member} to be the stored record's {:?}, \
                         got {:?}",
                        plain_summary.text_at(member),
                        filtered_summary.text_at(member)
                    );
                }
            }
        }
    }

    // A distribution that attaches no connection function is listed by none of them, and no
    // listing ever reports a distribution twice or one attached elsewhere.
    let attached_nowhere: Vec<String> = fixture
        .created
        .iter()
        .filter(|(_, attaches, _, _)| attaches.is_none())
        .map(|(_, _, id, _)| id.clone())
        .collect();
    assert!(
        !attached_nowhere.is_empty(),
        "the table must cover a distribution that attaches no connection function"
    );
    let mut union: Vec<String> = Vec::new();
    for function in &fixture.functions {
        union.extend(listed_ids(list_by_function(&router, &function.id, "").await.root()));
    }
    for id in &attached_nowhere {
        assert!(
            !union.contains(id),
            "expected {id} to be listed under no connection function, got {union:?}"
        );
    }
    let mut sorted = union.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        union.len(),
        "expected each distribution under at most one connection function, got {union:?}"
    );
}

/// Walking the pages at ANY admissible page size enumerates one connection function's
/// distributions exactly once, in id order, under every identifier form.
#[tokio::test]
async fn walking_the_pages_enumerates_each_function_exactly_once() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for (index, function) in fixture.functions.iter().enumerate() {
        let expected = fixture.expected(index);
        for (form, identifier) in function.identifiers() {
            for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_ITEMS), Some(i32::MAX)] {
                let label = match page_size {
                    None => format!("{}/{form}/default", function.name),
                    Some(size) => format!("{}/{form}/MaxItems={size}", function.name),
                };
                let collected = walk(&router, identifier, &label, page_size).await;
                assert_eq!(
                    collected, expected,
                    "case {label}: expected every distribution attached to this connection \
                     function exactly once in id order, got {collected:?}"
                );
            }
        }
    }
}

/// A connection function no distribution attaches lists nothing — no `Items` element,
/// `Quantity` zero and no `NextMarker` — rather than falling back to the whole store.
#[tokio::test]
async fn a_connection_function_nothing_attaches_lists_nothing() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let idle = &fixture.functions[2];

    for (form, identifier) in idle.identifiers() {
        let answer = list_by_function(&router, identifier, "").await;
        assert_eq!(
            answer.status, 200,
            "case {form}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        assert_eq!(
            list.text_at("Quantity"),
            Some("0"),
            "case {form}: expected Quantity 0, got {:?}",
            list.text_at("Quantity")
        );
        assert!(
            list.child("Items").is_none(),
            "case {form}: expected no Items element at all, got {}",
            answer.raw
        );
        assert_eq!(
            list.text_at("IsTruncated"),
            Some("false"),
            "case {form}: expected IsTruncated false, got {:?}",
            list.text_at("IsTruncated")
        );
        assert!(
            list.text_at("NextMarker").is_none(),
            "case {form}: expected no NextMarker, got {:?}",
            list.text_at("NextMarker")
        );
    }
}

/// An identifier no stored connection function answers to is `EntityNotFound` (HTTP 404)
/// rather than a page of nothing.
#[tokio::test]
async fn an_identifier_naming_no_connection_function_is_not_found() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let present = &fixture.functions[0];

    // Rows: a name never created, a prefix of a name that exists, the same name in another
    // case (identifiers are compared as given), an id in the shape this service mints that
    // was never allocated, and the ARN of such an id.
    let absent: Vec<(&str, String)> = vec![
        ("a-name-never-created", "no_such_function".to_string()),
        (
            "a-prefix-of-a-name-that-exists",
            present.name[..present.name.len() - 1].to_string(),
        ),
        ("the-name-in-another-case", present.name.to_uppercase()),
        ("an-id-never-allocated", "cf_ABCDEFGHIJKLM".to_string()),
        (
            "the-arn-of-an-id-never-allocated",
            present.arn.replace(&present.id, "cf_ABCDEFGHIJKLM"),
        ),
    ];

    for (case, identifier) in &absent {
        let answer = list_by_function(&router, identifier, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {}",
            answer.raw
        );
        let message = answer
            .root()
            .find_text("Message")
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message, "The specified connection function does not exist.",
            "case {case}: expected the fixed not-found sentence, got {message:?}"
        );
        assert!(
            !message.contains(identifier.as_str()),
            "case {case}: the refusal must not name the identifier {identifier:?}, got \
             {message:?}"
        );
    }
}

/// A page size that names no page, and a marker this service never issued, are refused as
/// invalid arguments — the same constraint the unfiltered listing states.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let fixture = fixture(&router).await;

    let refusals: &[(&str, &str)] = &[
        ("&MaxItems=-7", "a negative page size"),
        ("&MaxItems=not-a-number", "a page size that is no integer"),
        ("&Marker=not-an-id", "a marker that is no distribution id"),
    ];
    for function in &fixture.functions {
        for (query, description) in refusals {
            let answer = list_by_function(&router, &function.id, query).await;
            assert_eq!(
                answer.status, 400,
                "case {} with {description}: expected HTTP 400, got {} with {}",
                function.name, answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {} with {description}: expected InvalidArgument, got {:?}",
                function.name,
                answer.error_code()
            );
        }
    }
}

/// The listing answers from the STORED configuration at this instant: moving a
/// distribution's attachment to another connection function, and dropping it altogether,
/// move the distribution between the listings.
#[tokio::test]
async fn the_listing_follows_the_stored_configuration() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let first = fixture.functions[0].id.clone();
    let second = fixture.functions[1].id.clone();

    // The distribution this row moves: the first one attached to the first function.
    let (case, _, moved_id, etag) = fixture
        .created
        .iter()
        .find(|(_, attaches, _, _)| *attaches == Some(0))
        .cloned()
        .expect("the table attaches a distribution to the first connection function");

    // Move it onto the second connection function, under the version the create minted.
    let body = config_xml(&format!("ref-{case}"), Some(&second));
    let updated = update_distribution(&router, &moved_id, &body, Some(&etag)).await;
    assert_eq!(
        updated.status, 200,
        "case {case} moved: expected HTTP 200 from UpdateDistribution, got {} with {}",
        updated.status, updated.raw
    );
    let moved_etag = updated.header("etag").to_string();

    let on_first = listed_ids(list_by_function(&router, &first, "").await.root());
    assert!(
        !on_first.contains(&moved_id),
        "case {case} moved: expected {moved_id} to leave the first function's listing, got \
         {on_first:?}"
    );
    let on_second = listed_ids(list_by_function(&router, &second, "").await.root());
    assert!(
        on_second.contains(&moved_id),
        "case {case} moved: expected {moved_id} in the second function's listing, got \
         {on_second:?}"
    );

    // Drop the attachment entirely: the distribution is then listed under no connection
    // function at all, while still existing in the unfiltered listing.
    let body = config_xml(&format!("ref-{case}"), None);
    let cleared = update_distribution(&router, &moved_id, &body, Some(&moved_etag)).await;
    assert_eq!(
        cleared.status, 200,
        "case {case} cleared: expected HTTP 200 from UpdateDistribution, got {} with {}",
        cleared.status, cleared.raw
    );
    for function in &fixture.functions {
        let ids = listed_ids(list_by_function(&router, &function.id, "").await.root());
        assert!(
            !ids.contains(&moved_id),
            "case {case} cleared: expected {moved_id} under no connection function, got {ids:?} \
             for {}",
            function.name
        );
    }
    let all_ids = listed_ids(list_distributions(&router, "").await.root());
    assert!(
        all_ids.contains(&moved_id),
        "case {case} cleared: expected {moved_id} to still exist, got {all_ids:?}"
    );
}

/// Listing is a READ: calling it never changes what a later caller sees.
#[tokio::test]
async fn listing_changes_nothing() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for function in &fixture.functions {
        let first = list_by_function(&router, &function.id, "").await;
        let before = list_distributions(&router, "").await;
        let second = list_by_function(&router, &function.id, "").await;
        let after = list_distributions(&router, "").await;
        assert_eq!(
            listed_ids(first.root()),
            listed_ids(second.root()),
            "case {}: expected a repeated listing to report the same distributions",
            function.name
        );
        assert_eq!(
            listed_ids(before.root()),
            listed_ids(after.root()),
            "case {}: expected ListDistributions to be unchanged by the filtered listing",
            function.name
        );
    }
}
