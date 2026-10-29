// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH unit of hardware an Outpost-server installation
//! connection can be started for, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! `StartConnection` is the call that "installs Outpost servers" and its `AssetId` is
//! "the ID of the Outpost server", so it acts on hardware the store holds that IS a
//! server: a unit whose recorded `AssetType` is `COMPUTE` — "Server asset used for
//! customer compute", the one kind of unit an Outposts server is installed with.
//! Naming such a unit is answered a connection that reads back through
//! `GetConnection`; naming a unit of any other kind the modelled `AssetType` enum
//! declares — the power shelf, the switches, the networking asset and the storage
//! servers an Outposts RACK is built of — or hardware whose kind was never recorded,
//! is 400 `ValidationException` naming the member the value arrived in and nothing
//! else, and persists nothing.
//!
//! An `AssetId` no Outpost of the region holds is 404, and everything an ACCEPTED
//! start persists and reports is [`crate::outpost_server_connection`]'s own rule;
//! this suite is about the kind of hardware alone.
//!
//! The rows span that kind in both ways hardware reaches a store: recorded through
//! this crate's store, one row per value of the modelled value set
//! (`ops::types::ASSET_TYPE_VALUES`) plus a unit with no recorded kind at all; and
//! installed the way the service installs hardware — by the fulfilment of an order
//! placed on the Outpost — for both form factors a configuration is published for, a
//! whole rack of units and the one server a server Outpost is.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::ASSET_TYPE_VALUES;
use service_com_amazonaws_outposts_outpostsolafservice::state::ASSET_ID_MEMBER;

use crate::outpost_assets::{asset, record};
use crate::site_and_outpost_lifecycle::{install_hardware, segment, text, Outposts};

/// The route `StartConnection` is bound to, from
/// `smithy_query(query="operation",operation="StartConnection")`.
const START_ROUTE: &str = "/connections";

/// The `AssetType` of the hardware an Outposts SERVER is — "COMPUTE - Server asset
/// used for customer compute", from
/// `smithy_query(query="doc",shape="ListAssetsInput",member="AssetTypeFilter")`.
const SERVER_ASSET_TYPE: &str = "COMPUTE";

/// The route `GetConnection` binds for one `ConnectionId`. The label is
/// percent-encoded, because the modelled `ConnectionId` alphabet carries `+`, `/`
/// and `=`.
fn connection_route(connection_id: &str) -> String {
    format!("{START_ROUTE}/{}", segment(connection_id))
}

/// The route `ListAssets` binds for one Outpost, asking for more units than any
/// configuration holds so one page carries all of them (the modelled `MaxResults`
/// maximum is 1000).
fn assets_route(identifier: &str) -> String {
    format!("/outposts/{}/assets?MaxResults=1000", segment(identifier))
}

/// A `WireGuardPublicKey` of the modelled form (`len 44..44`, pattern
/// `^[a-zA-Z0-9/+]{43}=$`) that is row `row`'s own, so no two rows of a table send
/// one client key.
fn client_key(row: usize) -> String {
    let body: String = format!("{row}").chars().cycle().take(43).collect();
    format!("{body}=")
}

/// The smallest `StartConnection` payload: the three members the model marks
/// `@required`.
fn start_request(asset_id: &str, client_public_key: &str) -> Value {
    json!({
        "AssetId": asset_id,
        "ClientPublicKey": client_public_key,
        "NetworkInterfaceDeviceIndex": 0,
    })
}

/// What a refusal's body must open with: the standard AWS validation report around
/// one clause naming the member the offending value arrived in. The value itself is
/// never part of it, which each row asserts separately.
fn report_prefix() -> String {
    format!("1 validation error detected: Value at '{ASSET_ID_MEMBER}' failed to satisfy constraint: ")
}

/// How many connections the store holds.
fn held(outposts: &Outposts) -> usize {
    outposts.store().lock().connections.len()
}

/// Start a connection for `asset_id` and require that the call was accepted, that
/// the connection reads back through `GetConnection` with the client key it was
/// started with, and that the record names the hardware it installs.
async fn expect_started(outposts: &Outposts, asset_id: &str, key: &str, case: &str) {
    let start = outposts
        .ok("POST", START_ROUTE, Some(&start_request(asset_id, key)))
        .await;
    let connection_id = text(&start, "ConnectionId");
    let read = outposts
        .ok("GET", &connection_route(&connection_id), None)
        .await;
    assert_eq!(
        text(&read, "ConnectionId"),
        connection_id,
        "case {case:?}: GetConnection must report the id the start minted, got {read}"
    );
    let details = read
        .get("ConnectionDetails")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectionDetails member, got {read}"));
    assert_eq!(
        text(&details, "ClientPublicKey"),
        key,
        "case {case:?}: the connection must report the client key it was started with, got \
         {details}"
    );
    let store = outposts.store();
    let store = store.lock();
    let persisted = store
        .connections
        .get(&connection_id)
        .unwrap_or_else(|| panic!("case {case:?}: the start must have persisted the connection"));
    assert_eq!(
        persisted.asset_id, asset_id,
        "case {case:?}: the record must name the server the connection installs"
    );
}

/// Start a connection for `asset_id` and require that the call was refused as the bad
/// request it is: 400 `ValidationException`, whose body states the member and the
/// requirement and never the value.
async fn expect_refused(outposts: &Outposts, asset_id: &str, key: &str, case: &str) {
    let reply = outposts
        .call("POST", START_ROUTE, Some(&start_request(asset_id, key)))
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: hardware that is no Outpost server must be refused as a bad request, got \
         {} {} {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    let message = text(&reply.body, "Message");
    let prefix = report_prefix();
    assert!(
        message.starts_with(&prefix),
        "case {case:?}: expected a report opening {prefix:?}, got {message:?}"
    );
    assert!(
        !message.contains(asset_id),
        "case {case:?}: a validation report never renders the offending value, got {message:?}"
    );
}

/// The rule over hardware RECORDED on an Outpost: one row per kind the modelled
/// `AssetType` value set declares, and one row for hardware whose kind was never
/// recorded.
#[tokio::test]
async fn only_server_hardware_starts_an_installation_connection() {
    let outposts = Outposts::new();
    // The kinds come off the GENERATED value set rather than being spelled out here,
    // so every kind the model declares is a row — not only the ones a report cited.
    let mut kinds: Vec<Option<&str>> = ASSET_TYPE_VALUES.iter().map(|kind| Some(*kind)).collect();
    kinds.push(None);
    assert!(
        kinds.len() > 2 && kinds.contains(&Some(SERVER_ASSET_TYPE)),
        "the modelled AssetType set must span the server kind and others, got {kinds:?}"
    );

    let mut started = 0usize;
    for (row, kind) in kinds.iter().enumerate() {
        let case = format!("hardware recorded as {kind:?}");
        let asset_id = format!("asset_recorded_{row}");
        let (outpost_id, _) = outposts.site_and_outpost(&format!("kind-{row}")).await;
        record(
            &outposts,
            &outpost_id,
            &[asset(&asset_id, "rack-1", *kind, None, None)],
            &case,
        );

        let key = client_key(row);
        if *kind == Some(SERVER_ASSET_TYPE) {
            expect_started(&outposts, &asset_id, &key, &case).await;
            started += 1;
        } else {
            expect_refused(&outposts, &asset_id, &key, &case).await;
        }
        // A refused start persists nothing, so the store holds exactly the
        // connections the accepted rows made.
        assert_eq!(
            held(&outposts),
            started,
            "case {case:?}: only an accepted start may add a connection"
        );
    }
    assert_eq!(
        started, 1,
        "exactly one kind of hardware is an Outpost server, got {started} accepted row(s)"
    );
}

/// One site and one Outpost on it carrying `form_factor`, as that Outpost's
/// `OutpostId`. An Outpost belongs to a site, so the site comes first.
async fn an_outpost(outposts: &Outposts, name: &str, form_factor: &str) -> String {
    let site = outposts
        .create_site(&json!({ "Name": format!("{name}-site") }))
        .await;
    let outpost = outposts
        .create_outpost(&json!({
            "Name": name,
            "SiteId": text(&site, "SiteId"),
            "SupportedHardwareType": form_factor,
        }))
        .await;
    text(&outpost, "OutpostId")
}

/// The same rule over the hardware an ORDER installed: every asset id a caller reads
/// out of `ListAssets` is offered to `StartConnection`, and only the servers among
/// them start one.
#[tokio::test]
async fn only_the_servers_an_order_installed_start_an_installation_connection() {
    let outposts = Outposts::new();
    // Both form factors a configuration is published for: a rack arrives as a whole
    // rack of units, a server Outpost as the one server it is.
    let mut created = Vec::new();
    for form_factor in ["RACK", "SERVER"] {
        created.push((
            form_factor,
            an_outpost(&outposts, &format!("installed-{form_factor}"), form_factor).await,
        ));
    }
    for (_, outpost_id) in &created {
        install_hardware(&outposts, outpost_id).await;
    }

    let (mut accepted, mut refused, mut row) = (0usize, 0usize, 0usize);
    for (form_factor, outpost_id) in &created {
        let listed = outposts.ok("GET", &assets_route(outpost_id), None).await;
        let units = listed["Assets"].as_array().cloned().unwrap_or_default();
        assert!(
            !units.is_empty(),
            "case {form_factor:?}: the order's fulfilment must have installed hardware, got \
             {listed}"
        );
        for unit in &units {
            let asset_id = text(unit, "AssetId");
            let asset_type = text(unit, "AssetType");
            let case = format!("{form_factor} {asset_type} {asset_id}");
            let key = client_key(row);
            row += 1;
            if asset_type == SERVER_ASSET_TYPE {
                expect_started(&outposts, &asset_id, &key, &case).await;
                accepted += 1;
            } else {
                expect_refused(&outposts, &asset_id, &key, &case).await;
                refused += 1;
            }
        }
    }
    assert!(
        accepted > 0 && refused > 0,
        "the installed configurations must span both server hardware and hardware that is not: \
         {accepted} accepted, {refused} refused"
    );
    assert_eq!(
        held(&outposts),
        accepted,
        "only the servers among the installed units may have started a connection"
    );
}
