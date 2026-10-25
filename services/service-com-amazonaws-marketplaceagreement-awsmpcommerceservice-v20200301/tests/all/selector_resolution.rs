// SPDX-License-Identifier: CC0-1.0

//! Every listing selector of this service is RESOLVED, end to end over HTTP.
//!
//! The rule under test: a selector member whose admissible values this service
//! fixes — `catalog`, which names a catalog AWS Marketplace exposes (it exposes
//! exactly one), `agreementType`, which names one of the types the model
//! documents, and `agreementId`, which must name an agreement the account holds
//! — is resolved against that set BEFORE anything is selected, so a value
//! outside it is refused with HTTP 400 `ValidationException` carrying the
//! `INVALID_<MEMBER>` reason and naming the member, instead of being reported as
//! an empty page. A value INSIDE the set stays a filter: it selects whatever
//! carries it, and an empty page is a legitimate answer.
//!
//! Every operation of this crate that carries one of those members is a row, so
//! the rule is pinned over the whole class rather than on the one operation it
//! was reported against.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_lifecycle::{agreement_over, call, emulator, term, Answer};

/// The one catalog AWS Marketplace exposes, and so the only value a `catalog`
/// selector admits.
pub(crate) const CATALOG: &str = "AWSMarketplace";

/// Every `agreementType` the model documents: "The type of agreement. Values are
/// `PurchaseAgreement` or `VendorInsightsAgreement`" (`smithy_query(query="doc",
/// shape="DescribeAgreementOutput", member="agreementType")`). No operation of
/// this crate mints the second one, so it is a value the member admits that
/// selects nothing.
pub(crate) const AGREEMENT_TYPES: &[&str] = &["PurchaseAgreement", "VendorInsightsAgreement"];

/// Catalog names outside the single published one. `Catalog` is
/// `@length(1, 64)` with `^[a-zA-Z0-9.-]+$`, so each of these reaches a handler
/// rather than the boundary validator: a qualified name, a suffixed one, a
/// differently cased one, a hyphenated one, one a single character long, and one
/// with a trailing digit.
pub(crate) const OTHER_CATALOGS: &[&str] = &[
    "AWSMarketplace.Other",
    "AWSMarketplaceStaging",
    "awsmarketplace",
    "AWS-Marketplace",
    "A",
    "AWSMarketplace1",
];

/// Agreement types outside the documented pair. `AgreementType` is
/// `@length(1, 64)` with `^[A-Za-z]+$`, so each is a bare word the boundary
/// validator admits: an unrelated word, the right word differently cased, a
/// prefix of each documented value, and a pluralised one.
pub(crate) const OTHER_AGREEMENT_TYPES: &[&str] = &[
    "NotAType",
    "purchaseagreement",
    "Purchase",
    "VendorInsights",
    "PurchaseAgreements",
];

/// Agreement ids no agreement of the store carries, derived from `known` so the
/// rows differ along HOW the id misses: a readable name, a path-shaped id, an
/// agreement REQUEST id, the known id with a character appended, and the known
/// id with its first character dropped. `AgreementId` is `@length(1, 64)` with
/// `^[A-Za-z0-9_/-]+$`, which every row satisfies.
pub(crate) fn unknown_agreement_ids(known: &str) -> Vec<String> {
    assert!(
        known.len() > 1,
        "a minted agreement id is longer than one character, got {known:?}"
    );
    vec![
        "agmt-does-not-exist".to_string(),
        "agreement/0000000000000001".to_string(),
        "ar-0000000000000001".to_string(),
        format!("{known}0"),
        known[1..].to_string(),
    ]
}

/// One operation that carries resolved selectors: its wire name, the members
/// every call of it must carry, and whether its input declares `agreementId`
/// and `agreementType` beside `catalog`.
struct Listing {
    operation: &'static str,
    /// The required members, from
    /// `smithy_query(query="operation", operation="<the operation>")`.
    required: Value,
    /// Whether the input declares an `agreementId` selector.
    agreement_id: bool,
    /// Whether the input declares an `agreementType` selector.
    agreement_type: bool,
}

/// Every operation of this service whose input carries one of the resolved
/// selectors, from `smithy_query(query="digest")`: the four listings and the
/// search.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListAgreementCancellationRequests",
            required: json!({"partyType": "Acceptor"}),
            agreement_id: true,
            agreement_type: true,
        },
        Listing {
            operation: "ListAgreementPaymentRequests",
            required: json!({"partyType": "Acceptor"}),
            agreement_id: true,
            agreement_type: true,
        },
        Listing {
            operation: "ListAgreementCharges",
            required: json!({}),
            agreement_id: true,
            agreement_type: true,
        },
        Listing {
            operation: "ListBillingAdjustmentRequests",
            required: json!({}),
            agreement_id: true,
            agreement_type: true,
        },
        Listing {
            operation: "SearchAgreements",
            // `SearchAgreementsInput$filters` requires the agreement-type
            // filter of every search, so it is a member every call of this row
            // must carry.
            required: json!({
                "filters": [{"name": "AgreementType", "values": ["PurchaseAgreement"]}],
            }),
            agreement_id: false,
            agreement_type: false,
        },
    ]
}

/// The call `listing` makes with `selector` added to its required members.
async fn listed(router: &MultiServiceRouter, listing: &Listing, selector: &Value) -> Answer {
    let mut request = listing.required.clone();
    for (name, value) in selector.as_object().expect("a selector is an object") {
        request[name] = value.clone();
    }
    call(router, listing.operation, request).await
}

/// Assert that `selector` is refused with `reason`, naming `member`.
pub(crate) fn assert_refused(case: &str, member: &str, reason: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.reason(),
        reason,
        "case {case:?}: expected the {reason} reason, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/name"),
        member,
        "case {case:?}: the refusal names the member it is about, got {:?}",
        answer.body
    );
    assert!(
        answer.body.get("items").is_none()
            && answer.body.get("agreementViewSummaries").is_none(),
        "case {case:?}: a refused listing reports nothing, got {:?}",
        answer.body
    );
}

/// Assert that `selector` is accepted and answers a page.
fn assert_listed(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.get("items").is_some() || answer.body.get("agreementViewSummaries").is_some(),
        "case {case:?}: an accepted listing answers a page, got {:?}",
        answer.body
    );
}

/// A `catalog` outside the single published one is refused by every operation
/// that carries the member; the published one is accepted by all of them.
#[tokio::test]
async fn a_catalog_no_marketplace_exposes_is_refused_everywhere() {
    let router = emulator();
    agreement_over(&router, "ap-catalog", vec![term("eula.v1")]).await;

    for listing in listings() {
        for catalog in OTHER_CATALOGS {
            let case = format!("{} with catalog {catalog:?}", listing.operation);
            let answer = listed(&router, &listing, &json!({"catalog": catalog})).await;
            assert_refused(&case, "catalog", "INVALID_CATALOG", &answer);
        }
        let case = format!("{} with catalog {CATALOG:?}", listing.operation);
        let answer = listed(&router, &listing, &json!({"catalog": CATALOG})).await;
        assert_listed(&case, &answer);
    }
}

/// An `agreementType` outside the documented pair is refused; both documented
/// values are accepted, including the one no agreement of this service carries.
#[tokio::test]
async fn an_agreement_type_outside_the_documented_pair_is_refused_everywhere() {
    let router = emulator();
    agreement_over(&router, "ap-type", vec![term("eula.v1")]).await;

    for listing in listings().into_iter().filter(|row| row.agreement_type) {
        for agreement_type in OTHER_AGREEMENT_TYPES {
            let case = format!("{} with agreementType {agreement_type:?}", listing.operation);
            let answer = listed(&router, &listing, &json!({"agreementType": agreement_type})).await;
            assert_refused(&case, "agreementType", "INVALID_AGREEMENT_TYPE", &answer);
        }
        for agreement_type in AGREEMENT_TYPES {
            let case = format!("{} with agreementType {agreement_type:?}", listing.operation);
            let answer = listed(&router, &listing, &json!({"agreementType": agreement_type})).await;
            assert_listed(&case, &answer);
        }
    }
}

/// An `agreementId` that names no agreement is refused; the id of a stored
/// agreement is accepted, even when it carries none of the records listed.
#[tokio::test]
async fn an_agreement_id_naming_no_agreement_is_refused_everywhere() {
    let router = emulator();
    let known = agreement_over(&router, "ap-id", vec![term("eula.v1")]).await;

    for listing in listings().into_iter().filter(|row| row.agreement_id) {
        for agreement_id in unknown_agreement_ids(&known) {
            let case = format!("{} with agreementId {agreement_id:?}", listing.operation);
            let answer = listed(&router, &listing, &json!({"agreementId": agreement_id})).await;
            assert_refused(&case, "agreementId", "INVALID_AGREEMENT_ID", &answer);
        }
        let case = format!("{} with agreementId {known:?}", listing.operation);
        let answer = listed(&router, &listing, &json!({"agreementId": known.clone()})).await;
        assert_listed(&case, &answer);
    }
}

/// The refusal does not depend on what the store holds: on an empty store, a
/// value outside the fixed set is refused the same way, and the id of an
/// agreement that does not exist yet is refused rather than reported empty.
#[tokio::test]
async fn the_refusal_is_the_same_on_an_empty_store() {
    let router = emulator();

    for listing in listings() {
        let case = format!("{} on an empty store", listing.operation);
        let answer = listed(&router, &listing, &json!({"catalog": "AWSMarketplace.Other"})).await;
        assert_refused(&case, "catalog", "INVALID_CATALOG", &answer);

        if listing.agreement_type {
            let answer = listed(&router, &listing, &json!({"agreementType": "NotAType"})).await;
            assert_refused(&case, "agreementType", "INVALID_AGREEMENT_TYPE", &answer);
        }
        if listing.agreement_id {
            let answer =
                listed(&router, &listing, &json!({"agreementId": "agmt-nothing"})).await;
            assert_refused(&case, "agreementId", "INVALID_AGREEMENT_ID", &answer);
        }

        // Every selector at its admitted value still answers a page, which on
        // an empty store is an empty one.
        let mut selector = json!({"catalog": CATALOG});
        if listing.agreement_type {
            selector["agreementType"] = json!(AGREEMENT_TYPES[0]);
        }
        let answer = listed(&router, &listing, &selector).await;
        assert_listed(&case, &answer);
    }
}

/// A selector the service refuses wins over one it admits, whichever order the
/// request spells them in: the call is refused rather than answered with a page
/// computed from the admitted members alone.
#[tokio::test]
async fn a_refused_selector_wins_over_an_admitted_one() {
    let router = emulator();
    let known = agreement_over(&router, "ap-mixed", vec![term("eula.v1")]).await;

    struct Mixed {
        case: &'static str,
        selector: Value,
        member: &'static str,
        reason: &'static str,
    }

    let cases = vec![
        Mixed {
            case: "the published catalog and an undocumented agreement type",
            selector: json!({"catalog": CATALOG, "agreementType": "NotAType"}),
            member: "agreementType",
            reason: "INVALID_AGREEMENT_TYPE",
        },
        Mixed {
            case: "a documented agreement type and an unpublished catalog",
            selector: json!({"agreementType": "PurchaseAgreement", "catalog": "Other"}),
            member: "catalog",
            reason: "INVALID_CATALOG",
        },
        Mixed {
            case: "a stored agreement and an unpublished catalog",
            selector: json!({"agreementId": known.clone(), "catalog": "Other"}),
            member: "catalog",
            reason: "INVALID_CATALOG",
        },
        Mixed {
            case: "the published catalog and an agreement that does not exist",
            selector: json!({"catalog": CATALOG, "agreementId": "agmt-does-not-exist"}),
            member: "agreementId",
            reason: "INVALID_AGREEMENT_ID",
        },
    ];

    let rows = listings();
    for Mixed {
        case,
        selector,
        member,
        reason,
    } in cases
    {
        for listing in &rows {
            let needs = selector.as_object().expect("a selector is an object");
            if needs.contains_key("agreementId") && !listing.agreement_id {
                continue;
            }
            if needs.contains_key("agreementType") && !listing.agreement_type {
                continue;
            }
            let case = format!("{}: {case}", listing.operation);
            let answer = listed(&router, listing, &selector).await;
            assert_refused(&case, member, reason, &answer);
        }
    }
}
