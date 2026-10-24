// SPDX-License-Identifier: CC0-1.0

//! Pagination-cursor issuance, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`.
//!
//! The rule under test, stated over the class of tokens and of listings it covers:
//!
//! **A `NextToken` continues a listing only where this service issued it for that
//! listing.** A list operation resumes at the record its OWN cursor names for the catalog
//! and the record kind the request asks about; every other token — one a client built, one
//! naming a record the collection does not hold, one of another record kind, or one issued
//! for another catalog's listing — is `ValidationException` (400) naming `NextToken`,
//! never a position in the collection.
//!
//! The rows span what the rule generalises over: every list operation this service serves
//! (`ListBenefits`, `ListBenefitApplications`, `ListBenefitAllocations`), every seeded
//! catalog, several page sizes, and tokens that are well formed under the service's own
//! encoding but were never minted (each id-body shape the modelled id patterns admit, and
//! a real stored id carrying no catalog binding).

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    ALLOCATION_ID_PREFIX, APPLICATION_ID_PREFIX, BENEFIT_ID_PREFIX, ID_BODY_LEN, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness};
use crate::resource_tagging::error_message;

/// A catalog name the modelled `CatalogName` pattern `^[A-Za-z0-9_-]+$` admits and this
/// service does not serve (see `SEEDED_CATALOGS`).
const UNSERVED_CATALOG: &str = "Nope";

/// One `@paginated` list operation, as the rule covers it: the three differ only in the
/// collection they page and the id kind their cursor names.
struct Listing {
    /// The `X-Amz-Target` operation name.
    operation: &'static str,
    /// The output member carrying the page's summaries.
    summaries: &'static str,
    /// The prefix of an id of the record kind this listing pages, from the modelled id
    /// patterns (`^ben-[0-9a-z]{14}$`, `^benappl-[0-9a-z]{14}$`,
    /// `^benalloc-[0-9a-z]{14}$`).
    id_prefix: &'static str,
}

/// Every list operation this service serves.
const LISTINGS: [Listing; 3] = [
    Listing {
        operation: "ListBenefits",
        summaries: "BenefitSummaries",
        id_prefix: BENEFIT_ID_PREFIX,
    },
    Listing {
        operation: "ListBenefitApplications",
        summaries: "BenefitApplicationSummaries",
        id_prefix: APPLICATION_ID_PREFIX,
    },
    Listing {
        operation: "ListBenefitAllocations",
        summaries: "BenefitAllocationSummaries",
        id_prefix: ALLOCATION_ID_PREFIX,
    },
];

/// `text` as lowercase hex, the encoding this service's pagination tokens use — so a row
/// can hand back a token that is well formed and still was never issued.
fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// Id bodies the service's own id minting could have produced and no record of these
/// catalogs carries: [`ID_BODY_LEN`] characters of `[0-9a-z]`, the alphabet every modelled
/// id pattern admits.
fn unminted_bodies() -> Vec<String> {
    vec![
        "a".repeat(ID_BODY_LEN),
        "0".repeat(ID_BODY_LEN),
        "zz00zz00zz00zz".to_string(),
    ]
}

/// One page of `listing` that must answer 200: the summaries' ids, and the `NextToken`.
async fn page(
    harness: &Harness,
    case: &str,
    listing: &Listing,
    body: &Value,
) -> (Vec<String>, Option<String>) {
    let out = harness.ok(case, listing.operation, body).await;
    let ids = out[listing.summaries]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} list, got {out:?}",
                listing.summaries
            )
        })
        .iter()
        .map(|summary| summary["Id"].as_str().unwrap_or("<no Id>").to_string())
        .collect();
    let token = out["NextToken"].as_str().map(str::to_string);
    (ids, token)
}

/// Assert `operation` refuses `token` for `catalog` the way the rule says: 400
/// `ValidationException` naming `NextToken` in both the message and the `FieldList`.
async fn assert_refused(harness: &Harness, case: &str, operation: &str, catalog: &str, token: &str) {
    let (status, out) = harness
        .call(
            operation,
            &json!({ "Catalog": catalog, "NextToken": token }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to refuse the token with 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {out:?}"
    );
    assert!(
        error_message(&out).contains("NextToken"),
        "case {case:?}: the refusal must name NextToken, got {out:?}"
    );
    let named = out["FieldList"]
        .as_array()
        .and_then(|fields| fields.first())
        .and_then(|field| field["Name"].as_str());
    assert_eq!(
        named,
        Some("NextToken"),
        "case {case:?}: the refusal's FieldList must name NextToken, got {out:?}"
    );
}

/// A harness whose every seeded catalog also holds benefit applications, so all three
/// listings have more than one page's worth of records to cursor through.
async fn harness_with_applications() -> Harness {
    let harness = Harness::new();
    for catalog in SEEDED_CATALOGS {
        let benefit = harness.active_benefit_of(catalog).await;
        let benefit_id = text(catalog, &benefit, "Id");
        for index in 0..3 {
            harness
                .create(
                    catalog,
                    &json!({
                        "Catalog": catalog,
                        "ClientToken": format!("tok-cursor-{catalog}-{index}"),
                        "BenefitIdentifier": benefit_id,
                        "Name": format!("cursor-{index}"),
                    }),
                )
                .await;
        }
    }
    harness
}

/// A cursor the listing itself issued resumes it exactly where the page ended: paging each
/// listing of each catalog end to end reports the unpaged listing once, in the same order.
///
/// Rows: every list operation, every seeded catalog, and several page sizes.
#[tokio::test]
async fn a_cursor_this_listing_issued_resumes_it_where_the_page_ended() {
    let harness = harness_with_applications().await;

    for listing in &LISTINGS {
        for catalog in SEEDED_CATALOGS {
            let unpaged_case = format!("{} in {catalog}, unpaged", listing.operation);
            let (unpaged, token) =
                page(&harness, &unpaged_case, listing, &json!({ "Catalog": catalog })).await;
            assert!(
                unpaged.len() >= 2,
                "case {unpaged_case:?}: the listing must hold at least two records to cursor \
                 through, got {unpaged:?}"
            );
            assert_eq!(
                token, None,
                "case {unpaged_case:?}: a page holding every record carries no NextToken"
            );

            for max_results in [1usize, 2, 3] {
                let case = format!("{} in {catalog}, MaxResults {max_results}", listing.operation);
                let mut seen: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0usize;
                loop {
                    let mut body = json!({ "Catalog": catalog, "MaxResults": max_results });
                    if let Some(token) = &token {
                        body["NextToken"] = json!(token);
                    }
                    let (ids, next) = page(&harness, &case, listing, &body).await;
                    assert!(
                        ids.len() <= max_results,
                        "case {case:?}: a page holds at most {max_results} summaries, got {ids:?}"
                    );
                    if next.is_some() {
                        assert_eq!(
                            ids.len(),
                            max_results,
                            "case {case:?}: a page carrying a NextToken must be full, got {ids:?}"
                        );
                    }
                    seen.extend(ids);
                    token = next;
                    pages += 1;
                    assert!(
                        pages <= unpaged.len() + 1,
                        "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
                    );
                    if token.is_none() {
                        break;
                    }
                }
                assert_eq!(
                    seen, unpaged,
                    "case {case:?}: paging with an issued cursor must report the unpaged listing \
                     once, in order, got {seen:?}"
                );
            }
        }
    }
}

/// A `NextToken` this service never issued is refused, however well formed it is under the
/// service's own encoding.
///
/// Rows: every list operation, every seeded catalog, each never-minted id body of the right
/// record kind (bare, and bound to the catalog being paged), a stored id of this very
/// catalog carrying no catalog binding, a body outside the id pattern, and text that is not
/// a token at all.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let harness = harness_with_applications().await;

    for listing in &LISTINGS {
        for catalog in SEEDED_CATALOGS {
            let prefix = listing.id_prefix;
            let (stored, _) = page(
                &harness,
                &format!("{} in {catalog}", listing.operation),
                listing,
                &json!({ "Catalog": catalog }),
            )
            .await;
            let stored_id = stored
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("{} in {catalog} must report a record", listing.operation));

            let mut rows: Vec<(String, String)> = vec![
                (
                    "text that is not hex at all".to_string(),
                    "not-a-token".to_string(),
                ),
                ("hex that is not a cursor".to_string(), hex("hello")),
                (
                    "a cursor of this catalog whose id body is too short".to_string(),
                    hex(&format!("{catalog}:{prefix}0000")),
                ),
                (
                    "a cursor of this catalog whose id body is outside the pattern".to_string(),
                    hex(&format!("{catalog}:{prefix}ABCDEFGHIJKLMN")),
                ),
                (
                    "a stored id of this catalog carrying no catalog binding".to_string(),
                    hex(&stored_id),
                ),
            ];
            for body in unminted_bodies() {
                rows.push((
                    format!("a bare {prefix}{body} that was never minted"),
                    hex(&format!("{prefix}{body}")),
                ));
                rows.push((
                    format!("a cursor of this catalog naming the never-minted {prefix}{body}"),
                    hex(&format!("{catalog}:{prefix}{body}")),
                ));
            }

            for (row, token) in &rows {
                let case = format!("{} in {catalog}: {row}", listing.operation);
                assert_refused(&harness, &case, listing.operation, catalog, token).await;
            }
        }
    }
}

/// A cursor is bound to the listing that issued it: the same token is accepted by the
/// operation and catalog it was issued for, and refused by every other operation and every
/// other catalog.
///
/// Rows: every (list operation, seeded catalog) pair as the issuer, against every other
/// pair as the recipient.
#[tokio::test]
async fn a_cursor_issued_for_another_catalog_or_another_listing_is_refused() {
    let harness = harness_with_applications().await;

    let mut issued: Vec<(&Listing, &str, String)> = Vec::new();
    for listing in &LISTINGS {
        for catalog in SEEDED_CATALOGS {
            let case = format!("{} in {catalog}, first page of one", listing.operation);
            let (_, token) = page(
                &harness,
                &case,
                listing,
                &json!({ "Catalog": catalog, "MaxResults": 1 }),
            )
            .await;
            let token = token.unwrap_or_else(|| {
                panic!("case {case:?}: a one-record page of a longer listing carries a NextToken")
            });
            issued.push((listing, catalog, token));
        }
    }

    for (listing, catalog, token) in &issued {
        let case = format!("{} in {catalog}, resumed where issued", listing.operation);
        let (ids, _) = page(
            &harness,
            &case,
            listing,
            &json!({ "Catalog": catalog, "MaxResults": 1, "NextToken": token }),
        )
        .await;
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: resuming must report the next record, got {ids:?}"
        );

        for (other, other_catalog, _) in &issued {
            if other.operation == listing.operation && other_catalog == catalog {
                continue;
            }
            let case = format!(
                "a cursor {} issued for {catalog}, replayed on {} for {other_catalog}",
                listing.operation, other.operation
            );
            assert_refused(&harness, &case, other.operation, other_catalog, token).await;
        }
    }
}

/// A catalog this service does not serve is still reported `ResourceNotFoundException`
/// (404) — the catalog is looked up before any cursor can name a record in it — whether or
/// not the request carries a token.
#[tokio::test]
async fn an_unserved_catalog_is_not_found_whatever_token_the_request_carries() {
    let harness = Harness::new();

    for listing in &LISTINGS {
        let tokens: Vec<Option<String>> = vec![
            None,
            Some(hex(&format!(
                "{UNSERVED_CATALOG}:{}{}",
                listing.id_prefix,
                "a".repeat(ID_BODY_LEN)
            ))),
            Some("not-a-token".to_string()),
        ];
        for token in tokens {
            let case = format!("{} in {UNSERVED_CATALOG} with {token:?}", listing.operation);
            let mut body = json!({ "Catalog": UNSERVED_CATALOG });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let (status, out) = harness.call(listing.operation, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} with {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {out:?}"
            );
        }
    }
}
