// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE rule every CAPPED create of this service shares, and
//! that [`crate::outpost_server_connection`] does not state: the store a created
//! record lands in is BOUNDED by this emulator's own entry ceiling
//! (`MAX_RESOURCES`), and crossing that ceiling is a condition of the EMULATOR and
//! not of the request — so it is reported through a CLIENT error the operation's own
//! model declares, never as a 5xx server fault, which SDK retry classifiers treat as
//! retryable and so would have a caller repeat a call that can only fail again.
//!
//! The rule, stated once and then exercised over a SET of inputs:
//!
//! * Below the ceiling the call is ACCEPTED: `StartConnection` mints a
//!   `ConnectionId`, persists the connection and `GetConnection` answers it.
//! * At the ceiling the call is REFUSED with a 4xx error its model declares —
//!   `StartConnection` models no quota error, so that is 400 `ValidationException`,
//!   worded by this crate's `store_full` — and nothing is stored.
//! * A refused call disturbs nothing: every connection already persisted still reads
//!   back exactly as it did before the refusal.
//! * The same holds for every other capped store of this service, each with the
//!   client error ITS model declares: 400 `ValidationException` for `CreateQuote`,
//!   which models no quota error either, and 402 `ServiceQuotaExceededException` for
//!   `CreateSite` and `CreateOutpost`, which do.
//!
//! The ceiling is `MAX_RESOURCES` entries, far more than a test can fill one call at
//! a time, so each row seeds the store it is about directly up to it — the same way
//! [`crate::quote_store_bound`] and [`crate::capacity_task_lifecycle`] state the
//! bounds on theirs — and then drives the boundary over the wire.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{store_full, MAX_RESOURCES};

use crate::outpost_deletion_cascade::outpost_holding_all_in;
use crate::outpost_server_connection::{
    client_key, outpost_holding_all, route, start_request, START_ROUTE,
};
use crate::site_and_outpost_lifecycle::{text, Outposts, Reply, HOME_REGION};

/// A region that is NOT [`HOME_REGION`]: the ceiling is on the WHOLE store rather
/// than on one region's share of it, so a start addressed here is refused by it too.
const OTHER_REGION: &str = "us-west-2";

/// The modelled `AssetId` and `DeviceSerialNumber` maximum length, from
/// `smithy_query(query="operation",operation="StartConnection")`: both are `len
/// 1..100` with pattern `^(\w+)$`.
const MAX_IDENTIFIER_LENGTH: usize = 100;

/// The device indexes the model allows, from
/// `smithy_query(query="operation",operation="StartConnection")`
/// (`NetworkInterfaceDeviceIndex`, `range 0..1`): both published bounds.
const DEVICE_INDEXES: [i32; 2] = [0, 1];

/// Which capped store a row of [`every_store_ceiling_refusal_is_a_modelled_client_error`]
/// is about. Shared with [`crate::outpost_deletion_cascade`], which fills the
/// connection store through [`fill_to_ceiling`] too, and with
/// [`crate::store_ceiling_wording`], which states what every one of these ceilings is
/// WORDED as.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Store {
    Connections,
    Quotes,
    Sites,
    Outposts,
    Orders,
}

/// Fill `map` up to [`MAX_RESOURCES`] entries with copies of the one record a real
/// create already put there, under further keys of `label`'s own that nothing reads.
///
/// The copies occupy slots the ceiling counts, which is the only thing a row asserts
/// about them — no call resolves them, so what the boundary refuses can only be the
/// bound itself. `rekey` still carries each copy's own key into the identifier the
/// record holds for itself, so no filler disagrees with the key it is stored under.
fn pad<R: Clone>(map: &mut BTreeMap<String, R>, label: &str, rekey: impl Fn(&mut R, &str)) {
    let template = map
        .values()
        .next()
        .cloned()
        .expect("the accepted call persisted one record to copy");
    let mut index = 0usize;
    while map.len() < MAX_RESOURCES {
        let key = format!("{label}-filler-{index:06}");
        let mut copy = template.clone();
        rekey(&mut copy, &key);
        map.insert(key, copy);
        index += 1;
    }
}

/// Fill the store `which` names to its ceiling.
pub(crate) fn fill_to_ceiling(outposts: &Outposts, which: Store) {
    let state = outposts.store();
    let mut store = state.lock();
    match which {
        Store::Connections => pad(&mut store.connections, "connection", |record, key| {
            record.connection_id = key.to_string();
        }),
        Store::Quotes => pad(&mut store.quotes, "quote", |record, key| {
            record.quote.quote_id = Some(key.to_string());
        }),
        Store::Sites => pad(&mut store.sites, "site", |record, key| {
            record.site.site_id = Some(key.to_string());
        }),
        Store::Outposts => pad(&mut store.outposts, "outpost", |record, key| {
            record.outpost.outpost_id = Some(key.to_string());
        }),
        Store::Orders => pad(&mut store.orders, "order", |record, key| {
            record.order.order_id = Some(key.to_string());
        }),
    }
}

/// How many entries the store `which` names holds right now.
pub(crate) fn held(outposts: &Outposts, which: Store) -> usize {
    let state = outposts.store();
    let store = state.lock();
    match which {
        Store::Connections => store.connections.len(),
        Store::Quotes => store.quotes.len(),
        Store::Sites => store.sites.len(),
        Store::Outposts => store.outposts.len(),
        Store::Orders => store.orders.len(),
    }
}

/// The rule about ONE refusal: it is the status and code the operation's model
/// declares, and it is a CLIENT error — nothing in the 5xx range, which a caller's
/// SDK would retry.
fn assert_client_refusal(case: &str, reply: &Reply, status: u16, code: &str) {
    assert!(
        (400..500).contains(&reply.status),
        "case {case:?}: the emulator's own store ceiling must be reported as a client \
         error, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (status, code),
        "case {case:?}: expected the refusal {status} {code}, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
}

/// One row of [`the_connection_store_bound_is_the_client_error_the_model_declares`]:
/// a `StartConnection` request, and the region it is addressed to.
struct Case {
    case: &'static str,
    region: &'static str,
    asset_id: String,
    client_public_key: String,
    device_serial_number: Option<String>,
    index: i32,
}

/// The rows, differing along everything `StartConnection` generalises over: the
/// spelling and length of the `AssetId`, which characters of the modelled
/// `WireGuardPublicKey` alphabet the client key carries, whether the optional dongle
/// serial is present and how long it is, which of the two device indexes the model
/// allows the request names, and which region the call is addressed to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "required members only, the device index at the modelled minimum",
            region: HOME_REGION,
            asset_id: "asset_required_only".to_string(),
            client_public_key: client_key("B"),
            device_serial_number: None,
            index: DEVICE_INDEXES[0],
        },
        Case {
            case: "a dongle serial and the device index at the modelled maximum",
            region: HOME_REGION,
            asset_id: "asset_with_dongle".to_string(),
            client_public_key: client_key("c"),
            device_serial_number: Some("DNG0000000001".to_string()),
            index: DEVICE_INDEXES[1],
        },
        Case {
            case: "an AssetId at the modelled maximum length",
            region: HOME_REGION,
            asset_id: "a".repeat(MAX_IDENTIFIER_LENGTH),
            client_public_key: client_key("7"),
            device_serial_number: None,
            index: DEVICE_INDEXES[0],
        },
        Case {
            case: "a dongle serial at the modelled maximum length",
            region: HOME_REGION,
            asset_id: "asset_long_serial".to_string(),
            client_public_key: client_key("z"),
            device_serial_number: Some("D".repeat(MAX_IDENTIFIER_LENGTH)),
            index: DEVICE_INDEXES[1],
        },
        Case {
            case: "a start addressed to another region, whose own Outpost holds the hardware",
            region: OTHER_REGION,
            asset_id: "asset_elsewhere".to_string(),
            client_public_key: client_key("Q"),
            device_serial_number: Some("DNG9".to_string()),
            index: DEVICE_INDEXES[0],
        },
    ]
}

/// The rule for `StartConnection`, over a SET of start requests.
#[tokio::test]
async fn the_connection_store_bound_is_the_client_error_the_model_declares() {
    for Case {
        case,
        region,
        asset_id,
        client_public_key,
        device_serial_number,
        index,
    } in cases()
    {
        // A fresh emulator per row: the bound is a property of ONE store, so a row
        // must not inherit the entries another row left behind.
        let outposts = Outposts::new();
        if region == HOME_REGION {
            outpost_holding_all(&outposts, "bound", &[asset_id.as_str()]).await;
        } else {
            outpost_holding_all_in(&outposts, region, "bound", &[asset_id.as_str()]).await;
        }
        let payload = start_request(
            &asset_id,
            &client_public_key,
            device_serial_number.as_deref(),
            Some(index),
        );

        // Below the ceiling the start is accepted, persisted, and readable.
        let started = outposts
            .ok_in(region, "POST", START_ROUTE, Some(&payload))
            .await;
        let connection_id = text(&started, "ConnectionId");
        assert!(
            !connection_id.is_empty(),
            "case {case:?}: below the ceiling a start mints a ConnectionId, got {started}"
        );
        let read = outposts
            .ok_in(region, "GET", &route(&connection_id), None)
            .await;

        // Filled to the ceiling, the SAME start is refused — as the 400
        // `ValidationException` this operation models for a request it will not
        // accept, and not as the server fault an SDK would retry.
        fill_to_ceiling(&outposts, Store::Connections);
        let at_ceiling = held(&outposts, Store::Connections);
        assert_eq!(
            at_ceiling, MAX_RESOURCES,
            "case {case:?}: the store must stand at its ceiling: expected \
             {MAX_RESOURCES} connections held, got {at_ceiling}"
        );
        let refused = outposts
            .call_in(region, "POST", START_ROUTE, Some(&payload))
            .await;
        assert_client_refusal(case, &refused, 400, "ValidationException");
        let expected = store_full("connections");
        let message = text(&refused.body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal message {expected:?}, got {}",
            refused.body
        );

        // The refusal stored nothing...
        let after = held(&outposts, Store::Connections);
        assert_eq!(
            after, MAX_RESOURCES,
            "case {case:?}: a refused start must store nothing: expected \
             {MAX_RESOURCES} connections held, got {after}"
        );
        // ...and disturbed nothing: the connection started below the ceiling still
        // reads back exactly as it did.
        let again = outposts
            .ok_in(region, "GET", &route(&connection_id), None)
            .await;
        assert_eq!(
            again, read,
            "case {case:?}: after the refusal GetConnection must still answer the \
             connection the accepted start reported, got {again}"
        );
    }
}

/// The same rule across the service: every store this emulator caps reports the
/// ceiling as the client error ITS operation's model declares.
///
/// The rows differ along which store is capped and which error that operation
/// carries for a request it will not accept — the two that model no quota error
/// answer 400 `ValidationException`, the two that model one answer it (402) — so what
/// each row states is the rule and not one operation's choice of code.
///
/// Each of those boundaries also has a suite of its own, stating the rest of its
/// operation's bound over a table of that operation's inputs
/// ([`crate::quote_store_bound`] for `CreateQuote`, and the first test above for
/// `StartConnection`); what this one adds is the ONE rule they share, so the four
/// cannot drift into disagreeing about what a ceiling is reported as.
#[tokio::test]
async fn every_store_ceiling_refusal_is_a_modelled_client_error() {
    // `StartConnection`: the connection store, 400 `ValidationException`.
    let outposts = Outposts::new();
    let asset_id = "asset_every_store";
    outpost_holding_all(&outposts, "every-store", &[asset_id]).await;
    let payload = start_request(asset_id, &client_key("B"), None, Some(DEVICE_INDEXES[0]));
    outposts.ok("POST", START_ROUTE, Some(&payload)).await;
    refuses_at_its_ceiling(
        "StartConnection, the connection store",
        &outposts,
        Store::Connections,
        "POST",
        START_ROUTE,
        Some(payload),
        400,
        "ValidationException",
    )
    .await;

    // `CreateQuote`: the quote store, 400 `ValidationException` — it models no quota
    // error either.
    let outposts = Outposts::new();
    let quote = json!({ "CountryCode": "US", "RequestedCapacities": [] });
    outposts.ok("POST", "/quotes", Some(&quote)).await;
    refuses_at_its_ceiling(
        "CreateQuote, the quote store",
        &outposts,
        Store::Quotes,
        "POST",
        "/quotes",
        Some(quote),
        400,
        "ValidationException",
    )
    .await;

    // `CreateSite`: the site store, 402 `ServiceQuotaExceededException` — the quota
    // error this one does model.
    let outposts = Outposts::new();
    let site = json!({ "Name": "at-the-ceiling" });
    outposts.create_site(&site).await;
    refuses_at_its_ceiling(
        "CreateSite, the site store",
        &outposts,
        Store::Sites,
        "POST",
        "/sites",
        Some(site),
        402,
        "ServiceQuotaExceededException",
    )
    .await;

    // `CreateOutpost`: the Outpost store, 402 `ServiceQuotaExceededException`.
    let outposts = Outposts::new();
    let site_id = outposts.site_id_in(HOME_REGION, "ceiling-site").await;
    let outpost = json!({ "Name": "at-the-ceiling", "SiteId": site_id });
    outposts.create_outpost(&outpost).await;
    refuses_at_its_ceiling(
        "CreateOutpost, the Outpost store",
        &outposts,
        Store::Outposts,
        "POST",
        "/outposts",
        Some(outpost),
        402,
        "ServiceQuotaExceededException",
    )
    .await;
}

/// Fill the store `which` names to its ceiling, repeat `method path body` and require
/// that it is refused `status code` and stored nothing.
#[allow(clippy::too_many_arguments)]
async fn refuses_at_its_ceiling(
    case: &str,
    outposts: &Outposts,
    which: Store,
    method: &str,
    path: &str,
    body: Option<Value>,
    status: u16,
    code: &str,
) {
    fill_to_ceiling(outposts, which);
    let at_ceiling = held(outposts, which);
    assert_eq!(
        at_ceiling, MAX_RESOURCES,
        "case {case:?}: the store must stand at its ceiling: expected {MAX_RESOURCES} \
         entries held, got {at_ceiling}"
    );
    let refused = outposts.call(method, path, body.as_ref()).await;
    assert_client_refusal(case, &refused, status, code);
    let after = held(outposts, which);
    assert_eq!(
        after, MAX_RESOURCES,
        "case {case:?}: a refused call must store nothing: expected {MAX_RESOURCES} \
         entries held, got {after}"
    );
}
