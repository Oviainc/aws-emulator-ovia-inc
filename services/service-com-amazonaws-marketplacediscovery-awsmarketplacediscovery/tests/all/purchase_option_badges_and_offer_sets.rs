// SPDX-License-Identifier: CC0-1.0

//! Where a purchase option's and an offer set's values come from: the stored records, and
//! nothing beside them.
//!
//! Three rules are pinned here, each over the WHOLE seeded catalog or a table of inputs
//! rather than over one request:
//!
//! 1. a purchase-option badge RESTATES an attribute the stored offer or offer-set record
//!    already has — `PRIVATE_PRICING` its visibility scope, `FUTURE_DATED` an availability
//!    date still ahead of now, `REPLACEMENT_OFFER` a replaced agreement, `AUTO_RENEW` a
//!    renewal term — so no read claims a badge no record supports or omits one a record
//!    does, and the detail read and `ListPurchaseOptions` never disagree
//!    (`PurchaseOptionSummary$badges` / `GetOfferOutput$badges`: "Badges indicating special
//!    attributes … such as private pricing, future dated, or replacement offer";
//!    `RenewalTerm`: "enables automatic agreement renewal");
//! 2. `GetOfferSet` answers the stored record for `offerSetId` — its name, seller, dates,
//!    buyer notes and the (product, offer) pairs of the offers it bundles — and
//!    `ResourceNotFoundException` for an id the catalog does not hold;
//! 3. `ListPurchaseOptions` returns exactly the options whose record satisfies EVERY filter
//!    (AND across filters, OR within one), so a `PRODUCT_ID` + `SELLER_OF_RECORD_PROFILE_ID`
//!    pair selects the options that associate that product AND are sold by that seller, and
//!    an identifier no record carries selects none.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table,
//! input validation and the handlers are exercised end to end.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::PURCHASE_OPTION_BADGE_TYPE_VALUES;
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::PURCHASE_OPTION_BADGE_DISPLAY_NAMES;

// The route prefix, the SigV4 header and the one-service router this crate's suites share,
// declared once in `catalog_drilldown` and reused here rather than copied.
use crate::catalog_drilldown::{array, call, ids, ok, router, text};

/// Wall-clock seconds, which is what a `FUTURE_DATED` badge is decided against.
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// One whole page of purchase options for `filters` (100 is `maxResults`' modelled
/// maximum, and the catalog holds fewer rows than that).
async fn purchase_options(router: &MultiServiceRouter, filters: Value) -> Vec<Value> {
    let input = if filters.is_null() {
        json!({ "maxResults": 100 })
    } else {
        json!({ "maxResults": 100, "filters": filters })
    };
    let body = ok(router, "listPurchaseOptions", &input).await;
    array(&body, "purchaseOptions")
}

/// The badge types a response's `badges` list carries.
fn badge_types(body: &Value) -> BTreeSet<String> {
    array(body, "badges")
        .iter()
        .map(|badge| text(badge, "badgeType"))
        .collect()
}

/// Every term of an offer, read through `GetOfferTerms` one whole page at a time.
async fn offer_terms(router: &MultiServiceRouter, offer_id: &str) -> Vec<Value> {
    let mut terms: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let input = match &next {
            Some(token) => json!({ "offerId": offer_id, "maxResults": 20, "nextToken": token }),
            None => json!({ "offerId": offer_id, "maxResults": 20 }),
        };
        let body = ok(router, "getOfferTerms", &input).await;
        terms.extend(array(&body, "offerTerms"));
        next = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            break;
        }
    }
    terms
}

/// Whether the offer's terms include a `renewalTerm`, the union member that "enables
/// automatic agreement renewal" and so the record an `AUTO_RENEW` badge restates.
async fn renews_automatically(router: &MultiServiceRouter, offer_id: &str) -> bool {
    offer_terms(router, offer_id)
        .await
        .iter()
        .any(|term| term.get("renewalTerm").is_some())
}

/// The seller profile a purchase option or offer is sold by.
fn seller_of(body: &Value) -> String {
    body.get("sellerOfRecord")
        .map(|seller| text(seller, "sellerProfileId"))
        .unwrap_or_default()
}

/// The product ids a purchase option associates.
fn associated_products(row: &Value) -> Vec<String> {
    array(row, "associatedEntities")
        .iter()
        .filter_map(|entity| entity.get("product").cloned())
        .map(|product| text(&product, "productId"))
        .collect()
}

/// The offer ids a purchase option or offer set associates, de-duplicated.
fn associated_offers(row: &Value) -> Vec<String> {
    let mut offers: Vec<String> = Vec::new();
    for entity in array(row, "associatedEntities") {
        if let Some(offer) = entity.get("offer") {
            let offer_id = text(offer, "offerId");
            if !offers.contains(&offer_id) {
                offers.push(offer_id);
            }
        }
    }
    offers
}

/// Whether the record's availability date is still ahead of now, which is what a
/// `FUTURE_DATED` badge restates.
fn future_dated(body: &Value) -> bool {
    body.get("availableFromTime")
        .and_then(Value::as_f64)
        .is_some_and(|available_from| available_from > now_secs())
}

/// Rule 1, over EVERY purchase option the catalog holds: each badge restates an attribute
/// of the stored record, and the detail read reports the same badges as the list.
#[tokio::test]
async fn a_purchase_options_badges_are_the_attributes_its_own_records_have() {
    let router = router();
    let rows = purchase_options(&router, Value::Null).await;
    assert!(
        rows.len() > 2,
        "case setup: the seeded catalog must hold several purchase options, got {rows:?}"
    );

    // The options the PRIVATE visibility scope selects, which is the record attribute a
    // PRIVATE_PRICING badge restates.
    let private: BTreeSet<String> = ids(
        &purchase_options(
            &router,
            json!([{ "filterType": "VISIBILITY_SCOPE", "filterValues": ["PRIVATE"] }]),
        )
        .await,
        "purchaseOptionId",
    )
    .into_iter()
    .collect();

    let mut reported: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let id = text(row, "purchaseOptionId");
        let option_type = text(row, "purchaseOptionType");
        let case = format!("{option_type} {id}");

        // The detail read of the same record, and the attributes that decide its badges.
        let (detail, expected_auto_renew, expected_replacement) = match option_type.as_str() {
            "OFFER" => {
                let detail = ok(&router, "getOffer", &json!({ "offerId": &id })).await;
                let auto_renew = renews_automatically(&router, &id).await;
                let replacement = detail.get("replacementAgreementId").is_some();
                (detail, auto_renew, replacement)
            }
            "OFFERSET" => {
                let detail = ok(&router, "getOfferSet", &json!({ "offerSetId": &id })).await;
                // A badge on the SET claims its attribute of the whole bundle, so each is
                // expected only when every bundled offer has it.
                let bundled = associated_offers(&detail);
                let mut auto_renew = !bundled.is_empty();
                let mut replacement = !bundled.is_empty();
                for offer_id in &bundled {
                    let offer = ok(&router, "getOffer", &json!({ "offerId": offer_id })).await;
                    auto_renew &= renews_automatically(&router, offer_id).await;
                    replacement &= offer.get("replacementAgreementId").is_some();
                }
                (detail, auto_renew, replacement)
            }
            other => panic!("case {case}: unexpected purchaseOptionType {other}"),
        };

        let mut expected: BTreeSet<String> = BTreeSet::new();
        if private.contains(&id) {
            expected.insert("PRIVATE_PRICING".to_string());
        }
        if future_dated(row) {
            expected.insert("FUTURE_DATED".to_string());
        }
        if expected_replacement {
            expected.insert("REPLACEMENT_OFFER".to_string());
        }
        if expected_auto_renew {
            expected.insert("AUTO_RENEW".to_string());
        }

        let actual = badge_types(row);
        assert_eq!(
            actual, expected,
            "case {case}: expected the badges its records support {expected:?}, got {actual:?}"
        );
        let from_detail = badge_types(&detail);
        assert_eq!(
            from_detail, actual,
            "case {case}: the detail read must report the list's badges {actual:?}, got \
             {from_detail:?}"
        );

        // Every badge names a modelled type and displays the name that type is displayed
        // by, whichever read reported it.
        for badge in array(row, "badges") {
            let badge_type = text(&badge, "badgeType");
            assert!(
                PURCHASE_OPTION_BADGE_TYPE_VALUES.contains(&badge_type.as_str()),
                "case {case}: {badge_type} must be a modelled PurchaseOptionBadgeType, which \
                 are {PURCHASE_OPTION_BADGE_TYPE_VALUES:?}"
            );
            let display_name = text(&badge, "displayName");
            let expected_name = PURCHASE_OPTION_BADGE_DISPLAY_NAMES
                .iter()
                .find(|(known, _)| *known == badge_type)
                .map(|(_, name)| (*name).to_string())
                .unwrap_or_default();
            assert_eq!(
                display_name, expected_name,
                "case {case}: badge {badge_type} must display as {expected_name:?}, got \
                 {display_name:?}"
            );
            assert!(
                !display_name.trim().is_empty(),
                "case {case}: PurchaseOptionBadge$displayName is non-blank, got {badge}"
            );
        }
        reported.extend(actual);
    }

    // The catalog exercises every badge type the model declares, so none of the rules above
    // is asserted only over records that never carry the badge.
    let modelled: BTreeSet<String> = PURCHASE_OPTION_BADGE_TYPE_VALUES
        .iter()
        .map(|badge| (*badge).to_string())
        .collect();
    assert_eq!(
        reported, modelled,
        "case whole catalog: every modelled purchase-option badge must be reachable, got \
         {reported:?}"
    );
}

/// Rule 2: an offer set reads back as the record `ListPurchaseOptions` projected it from,
/// and its associated entities are the (product, offer) pairs of the offers it bundles.
#[tokio::test]
async fn an_offer_set_reads_back_the_record_the_purchase_option_came_from() {
    let router = router();
    let sets = purchase_options(
        &router,
        json!([{ "filterType": "PURCHASE_OPTION_TYPE", "filterValues": ["OFFERSET"] }]),
    )
    .await;
    assert!(
        !sets.is_empty(),
        "case setup: the catalog must hold at least one offer set, got {sets:?}"
    );

    for row in &sets {
        let id = text(row, "purchaseOptionId");
        let detail = ok(&router, "getOfferSet", &json!({ "offerSetId": &id })).await;

        for member in ["offerSetId", "catalog"] {
            let value = text(&detail, member);
            assert!(
                !value.trim().is_empty(),
                "case {id}: {member} is required and non-blank, got {detail}"
            );
        }
        assert_eq!(
            text(&detail, "offerSetId"),
            id,
            "case {id}: the read must answer for the requested id, got {detail}"
        );
        assert_eq!(
            text(&detail, "catalog"),
            text(row, "catalog"),
            "case {id}: the catalog must be the one the purchase option names, got {detail}"
        );
        assert_eq!(
            text(&detail, "offerSetName"),
            text(row, "purchaseOptionName"),
            "case {id}: the name must be the one the purchase option names, got {detail}"
        );
        assert_eq!(
            seller_of(&detail),
            seller_of(row),
            "case {id}: the seller must be the one the purchase option names, got {detail}"
        );
        for member in ["availableFromTime", "expirationTime"] {
            assert_eq!(
                detail.get(member).and_then(Value::as_f64),
                row.get(member).and_then(Value::as_f64),
                "case {id}: {member} must be the stored instant, got {detail}"
            );
        }

        // Each entity pairs an offer the set bundles with a product that offer covers, and
        // both resolve through their own reads.
        let entities = array(&detail, "associatedEntities");
        assert!(
            !entities.is_empty(),
            "case {id}: an offer set bundles offers, got {detail}"
        );
        for entity in &entities {
            let offer_id = entity
                .get("offer")
                .map(|offer| text(offer, "offerId"))
                .unwrap_or_default();
            let product_id = entity
                .get("product")
                .map(|product| text(product, "productId"))
                .unwrap_or_default();
            let offer = ok(&router, "getOffer", &json!({ "offerId": &offer_id })).await;
            let covered = associated_products(&offer);
            assert!(
                covered.contains(&product_id),
                "case {id}: offer {offer_id} must cover the paired product {product_id}, got \
                 {covered:?}"
            );
            let product = ok(&router, "getProduct", &json!({ "productId": &product_id })).await;
            assert_eq!(
                text(&product, "productId"),
                product_id,
                "case {id}: the paired product must resolve, got {product}"
            );
        }
        // The purchase option and the detail read agree on which offers the set bundles.
        assert_eq!(
            associated_offers(&detail),
            associated_offers(row),
            "case {id}: both reads must name the same bundled offers"
        );
    }
}

/// Rule 2's refusal: an `offerSetId` the catalog does not hold is the modelled 404, with the
/// message rule this service follows everywhere — the kind of resource and nothing from the
/// request.
#[tokio::test]
async fn an_offer_set_the_catalog_does_not_hold_is_not_found() {
    let router = router();
    let known = ids(
        &purchase_options(
            &router,
            json!([{ "filterType": "PURCHASE_OPTION_TYPE", "filterValues": ["OFFERSET"] }]),
        )
        .await,
        "purchaseOptionId",
    )
    .first()
    .cloned()
    .expect("a seeded offer set");

    // Rows: an absent id, an id of another kind of resource, the known id with a suffix,
    // and a long opaque id — all inside `OfferSetId`'s modelled `^[\w\-]+$`.
    for unknown in [
        "does-not-exist".to_string(),
        "o-0001-analytics-annual".to_string(),
        format!("{known}-x"),
        "x-00000000000000000000000000000000".to_string(),
    ] {
        let (status, body) = call(&router, "getOfferSet", &json!({ "offerSetId": &unknown })).await;
        assert_eq!(
            status, 404,
            "case offerSetId={unknown}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            text(&body, "__type"),
            "ResourceNotFoundException",
            "case offerSetId={unknown}: expected ResourceNotFoundException, got {body}"
        );
        let message = text(&body, "message");
        assert_eq!(
            message, "OfferSet not found",
            "case offerSetId={unknown}: expected the fixed resource-only message, got \
             {message:?}"
        );
        assert!(
            !message.contains(unknown.as_str()),
            "case offerSetId={unknown}: the message must carry no part of the request, got \
             {message:?}"
        );
    }

    // The positive control still resolves, so the refusal is about the id.
    let found = ok(&router, "getOfferSet", &json!({ "offerSetId": &known })).await;
    assert_eq!(
        text(&found, "offerSetId"),
        known,
        "case offerSetId={known}: the known id must resolve, got {found}"
    );
}

/// Rule 3: a `PRODUCT_ID` + `SELLER_OF_RECORD_PROFILE_ID` pair selects the options that
/// associate one of the products AND are sold by one of the sellers, and an identifier no
/// record carries selects none. The expectation is computed from the unfiltered result set,
/// so each row is a property of the catalog rather than a hand-written answer.
#[tokio::test]
async fn a_product_and_seller_filter_pair_selects_the_options_whose_records_match_both() {
    let router = router();
    let all = purchase_options(&router, Value::Null).await;
    assert!(
        all.len() > 2,
        "case setup: the seeded catalog must hold several purchase options, got {all:?}"
    );

    // Two rows sold by DIFFERENT sellers, so a mismatched pair is constructible.
    let first = all
        .iter()
        .find(|row| !associated_products(row).is_empty())
        .expect("a purchase option associating a product");
    let other = all
        .iter()
        .find(|row| !associated_products(row).is_empty() && seller_of(row) != seller_of(first))
        .expect("a purchase option sold by another seller");

    let first_product = associated_products(first)[0].clone();
    let first_seller = seller_of(first);
    let other_product = associated_products(other)[0].clone();
    let other_seller = seller_of(other);

    // Rows differ in how many values each filter carries, whether the pair agrees, and
    // whether each identifier exists at all.
    let cases: [(&str, Vec<String>, Vec<String>); 6] = [
        (
            "matching pair",
            vec![first_product.clone()],
            vec![first_seller.clone()],
        ),
        (
            "mismatched pair",
            vec![first_product.clone()],
            vec![other_seller.clone()],
        ),
        (
            "two products, two sellers",
            vec![first_product.clone(), other_product.clone()],
            vec![first_seller.clone(), other_seller.clone()],
        ),
        (
            "unknown product, known seller",
            vec!["p-no-such-product".to_string()],
            vec![first_seller.clone()],
        ),
        (
            "known product, unknown seller",
            vec![first_product.clone()],
            vec!["sp-no-such-seller".to_string()],
        ),
        (
            "both unknown",
            vec!["p-no-such-product".to_string()],
            vec!["sp-no-such-seller".to_string()],
        ),
    ];

    for (case, products, sellers) in cases {
        // What the records say the answer is: AND across the two filters, OR within each.
        let expected: Vec<String> = all
            .iter()
            .filter(|row| {
                associated_products(row)
                    .iter()
                    .any(|product| products.contains(product))
                    && sellers.contains(&seller_of(row))
            })
            .map(|row| text(row, "purchaseOptionId"))
            .collect();

        let rows = purchase_options(
            &router,
            json!([
                { "filterType": "PRODUCT_ID", "filterValues": products },
                { "filterType": "SELLER_OF_RECORD_PROFILE_ID", "filterValues": sellers },
            ]),
        )
        .await;
        let actual = ids(&rows, "purchaseOptionId");
        assert_eq!(
            actual, expected,
            "case {case} products={products:?} sellers={sellers:?}: expected {expected:?}, got \
             {actual:?}"
        );
        for row in &rows {
            let id = text(row, "purchaseOptionId");
            assert!(
                associated_products(row)
                    .iter()
                    .any(|product| products.contains(product)),
                "case {case}: row {id} must associate a filtered product, got {:?}",
                associated_products(row)
            );
            assert!(
                sellers.contains(&seller_of(row)),
                "case {case}: row {id} must be sold by a filtered seller, got {:?}",
                seller_of(row)
            );
            // The option carries the entities a client drills into next, so each one names
            // both the product and the offer the record pairs.
            for entity in array(row, "associatedEntities") {
                assert!(
                    entity.get("product").is_some() && entity.get("offer").is_some(),
                    "case {case}: row {id} must pair a product with an offer, got {entity}"
                );
            }
        }
    }

    // The matching pair is the class the drill-down needs: at least one option, carrying an
    // id that reads back.
    let matched = purchase_options(
        &router,
        json!([
            { "filterType": "PRODUCT_ID", "filterValues": [first_product.clone()] },
            { "filterType": "SELLER_OF_RECORD_PROFILE_ID", "filterValues": [first_seller.clone()] },
        ]),
    )
    .await;
    assert!(
        !matched.is_empty(),
        "case matching pair: a product's own seller must select its purchase options, got \
         {matched:?}"
    );
    for row in &matched {
        let id = text(row, "purchaseOptionId");
        let operation = match text(row, "purchaseOptionType").as_str() {
            "OFFERSET" => "getOfferSet",
            _ => "getOffer",
        };
        let member = if operation == "getOfferSet" {
            "offerSetId"
        } else {
            "offerId"
        };
        let detail = ok(&router, operation, &json!({ member: &id })).await;
        assert_eq!(
            text(&detail, member),
            id,
            "case matching pair: {operation} must read back {id}, got {detail}"
        );
    }
}
