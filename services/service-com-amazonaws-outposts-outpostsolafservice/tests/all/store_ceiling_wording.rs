// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE rule every store-ceiling refusal of this service
//! shares, and that the suites about each individual bound
//! ([`crate::quote_store_bound`], [`crate::connection_store_bound`]) state only for
//! the code and status: the BODY of that refusal.
//!
//! The ceiling (`MAX_RESOURCES`) is this emulator's own in-memory entry count and not
//! an AWS quota, so the sentence that reports it is stated ONCE for the whole crate,
//! by `state::store_full`: it names the emulator, the kind of resource and the bound,
//! and so cannot be read as a statement about the caller's request or about a
//! published service quota. Every path that refuses a call for that ceiling words it
//! there, whatever modelled error its own operation carries — which is what this
//! suite drives over the wire.
//!
//! The rows differ along what the rule generalises over: which store is full, which
//! operation fills it, and which modelled error that operation answers the ceiling
//! with (402 `ServiceQuotaExceededException` where the model declares one, 400
//! `ValidationException` where it does not). Each row asserts the body is exactly the
//! sentence `store_full` renders for its resource kind, and the rows together assert
//! that every one of those bodies is the SAME sentence apart from the resource kind,
//! so no path can drift into a wording of its own again.
//!
//! The two remaining capped paths carry the same body, each pinned where its own
//! operation is stated: `CreateRenewal` in [`crate::renewal_contract`] and
//! `StartCapacityTask` (which answers 409 `ConflictException`) in
//! [`crate::capacity_task_lifecycle`].
//!
//! The ceiling is far more than a test can fill one call at a time, so every row
//! makes the call once below the ceiling — which also leaves the one record the
//! filler copies — then fills the store through the seam
//! [`crate::connection_store_bound::fill_to_ceiling`] and repeats the SAME call.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{store_full, MAX_RESOURCES};

use crate::connection_store_bound::{fill_to_ceiling, held, Store};
use crate::outpost_server_connection::{client_key, outpost_holding_all, start_request, START_ROUTE};
use crate::site_and_outpost_lifecycle::{text, Outposts, HOME_REGION};

/// A `NetworkInterfaceDeviceIndex` the model allows, from
/// `smithy_query(query="operation",operation="StartConnection")`
/// (`NetworkInterfaceDeviceIndex`, `range 0..1`): its published minimum.
const DEVICE_INDEX: i32 = 0;

/// A `CatalogItemId` of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`) that this
/// service's published catalog offers, so the line item below is orderable.
const SKU: &str = "OR-1A2B3C4";

/// One row: the store that fills up, the resource kind its refusal names, and the
/// modelled error the operation that fills it answers the ceiling with.
struct Case {
    case: &'static str,
    store: Store,
    resource: &'static str,
    status: u16,
    code: &'static str,
}

/// The rows, one per capped store this suite drives.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "CreateSite, the site store",
            store: Store::Sites,
            resource: "sites",
            status: 402,
            code: "ServiceQuotaExceededException",
        },
        Case {
            case: "CreateOutpost, the Outpost store",
            store: Store::Outposts,
            resource: "Outposts",
            status: 402,
            code: "ServiceQuotaExceededException",
        },
        Case {
            case: "CreateOrder, the order store",
            store: Store::Orders,
            resource: "orders",
            status: 402,
            code: "ServiceQuotaExceededException",
        },
        Case {
            case: "CreateQuote, the quote store",
            store: Store::Quotes,
            resource: "quotes",
            status: 400,
            code: "ValidationException",
        },
        Case {
            case: "StartConnection, the connection store",
            store: Store::Connections,
            resource: "connections",
            status: 400,
            code: "ValidationException",
        },
    ]
}

/// Make the call that fills the store `which` names ONCE, below the ceiling, and
/// answer the request to repeat at the ceiling as `(method, path, body)`.
///
/// The accepted call is what leaves the one real record the filler copies, so what
/// refuses the repeat can only be the bound and not anything about the request.
async fn accepted_call(
    outposts: &Outposts,
    which: Store,
) -> (&'static str, String, Option<Value>) {
    match which {
        Store::Sites => {
            let site = json!({ "Name": "at-the-ceiling" });
            outposts.create_site(&site).await;
            ("POST", "/sites".to_string(), Some(site))
        }
        Store::Outposts => {
            let site_id = outposts.site_id_in(HOME_REGION, "ceiling-site").await;
            let outpost = json!({ "Name": "at-the-ceiling", "SiteId": site_id });
            outposts.create_outpost(&outpost).await;
            ("POST", "/outposts".to_string(), Some(outpost))
        }
        Store::Orders => {
            let (outpost_id, _) = outposts.site_and_outpost("ceiling-order").await;
            let order = json!({
                "OutpostIdentifier": outpost_id,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": SKU, "Quantity": 1 }],
            });
            outposts.ok("POST", "/orders", Some(&order)).await;
            ("POST", "/orders".to_string(), Some(order))
        }
        Store::Quotes => {
            let quote = json!({ "CountryCode": "US", "RequestedCapacities": [] });
            outposts.ok("POST", "/quotes", Some(&quote)).await;
            ("POST", "/quotes".to_string(), Some(quote))
        }
        Store::Connections => {
            let asset_id = "asset_at_the_ceiling";
            outpost_holding_all(outposts, "ceiling-connection", &[asset_id]).await;
            let payload = start_request(asset_id, &client_key("B"), None, Some(DEVICE_INDEX));
            outposts.ok("POST", START_ROUTE, Some(&payload)).await;
            ("POST", START_ROUTE.to_string(), Some(payload))
        }
    }
}

/// The rule, over every capped store this suite drives.
#[tokio::test]
async fn every_store_ceiling_refusal_carries_the_one_body_this_crate_states_it_with() {
    // Each row's body with its own resource kind masked out: the rule is that one
    // ceiling is stated once, so every row must leave the same skeleton behind.
    let mut skeletons: Vec<(&str, String)> = Vec::new();

    for Case {
        case,
        store,
        resource,
        status,
        code,
    } in cases()
    {
        // A fresh emulator per row: the bound is a property of ONE store, so a row
        // must not inherit the entries another row left behind.
        let outposts = Outposts::new();
        let (method, path, body) = accepted_call(&outposts, store).await;

        fill_to_ceiling(&outposts, store);
        let at_ceiling = held(&outposts, store);
        assert_eq!(
            at_ceiling, MAX_RESOURCES,
            "case {case:?}: the store must stand at its ceiling: expected \
             {MAX_RESOURCES} entries held, got {at_ceiling}"
        );

        let refused = outposts.call(method, &path, body.as_ref()).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (status, code),
            "case {case:?}: expected the refusal {status} {code}, got {} {} {}",
            refused.status,
            refused.error_code,
            refused.body
        );
        let expected = store_full(resource);
        let message = text(&refused.body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal body {expected:?}, got {:?}",
            refused.body
        );
        // The body states the emulator's own bound, so it names that bound and the
        // kind of resource it is on — and nothing a caller could read as a quota on
        // the request they sent.
        assert!(
            message.contains(&MAX_RESOURCES.to_string()) && message.contains(resource),
            "case {case:?}: the refusal body must name the bound {MAX_RESOURCES} and \
             the resource kind {resource:?}, got {message:?}"
        );
        skeletons.push((case, message.replace(resource, "<resource>")));

        // The refusal stored nothing: the store stands exactly where it did.
        let after = held(&outposts, store);
        assert_eq!(
            after, MAX_RESOURCES,
            "case {case:?}: a refused call must store nothing: expected \
             {MAX_RESOURCES} entries held, got {after}"
        );
    }

    let (first_case, first) = skeletons
        .first()
        .cloned()
        .expect("every row contributes a body");
    for (case, skeleton) in &skeletons {
        assert_eq!(
            skeleton, &first,
            "case {case:?}: every store-ceiling refusal is the one sentence this crate \
             states, apart from the resource kind: expected {first:?} (as in case \
             {first_case:?}), got {skeleton:?}"
        );
    }
}
