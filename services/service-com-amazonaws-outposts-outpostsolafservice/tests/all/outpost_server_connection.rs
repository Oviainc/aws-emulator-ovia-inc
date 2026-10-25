// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the connection an Outpost SERVER is installed over, driven
//! over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses) —
//! `MultiServiceRouter::dispatch` with a restJson1 request, exactly as a signed
//! `aws outposts` call arrives.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `StartConnection` persists the connection a caller started for one hardware
//!    asset — the client end's public key, the dongle serial and the network
//!    interface index it named — mints an opaque `ConnectionId` of the modelled form
//!    and reports an `UnderlayIpAddress` of it.
//! 2. `GetConnection` answers that stored record: the id the start minted and the
//!    tunnel it handed out, field for field, in the forms the model declares, as
//!    often as it is asked. Reporting a connection changes nothing a later call
//!    sees.
//! 3. A connection's own link is its own: two connections of one store never share
//!    a `ConnectionId`, a tunnel address or an underlay address, while both name the
//!    one tunnel server that store reports — the same `ServerPublicKey` and the same
//!    `ServerEndpoint`.
//! 4. A `ConnectionId` that names nothing is 404 `NotFoundException` whose body is
//!    this service's identifier echo, and so is an `AssetId` no Outpost holds; a
//!    request outside a modelled constraint — including one that omits a member the
//!    model marks `@required` — is 400 `ValidationException`. Neither persists
//!    anything.
//! 5. Starting a connection is neither idempotent nor exclusive: nothing in this API
//!    models a second start as a conflict, so EVERY accepted `StartConnection` adds
//!    one connection of its own to the store — whatever the request repeats, and
//!    however many times — and every connection already started stays readable
//!    exactly as it was, under the id it was given.
//!
//! Where the hardware comes from: an asset is a PHYSICAL unit of hardware that
//! arrives with a fulfilled order, and no order this emulator accepts is ever carried
//! as far as being delivered and installed (see [`crate::order_lifecycle`]), so
//! each row RECORDS its asset on the stored Outpost through this crate's store —
//! exactly as [`crate::outpost_assets`] does, whose two recording helpers this suite
//! reuses.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::outpost_assets::{asset, record};
use crate::site_and_outpost_lifecycle::{refusal, segment, text, Outposts, Refusal};

/// The route `StartConnection` is bound to, from
/// `smithy_query(query="operation",operation="StartConnection")`.
const START_ROUTE: &str = "/connections";

/// The route `GetConnection` is bound to, for one `ConnectionId`:
/// `GET /connections/{ConnectionId}`. The label is percent-encoded, because the
/// modelled `ConnectionId` alphabet carries `+`, `/` and `=`.
fn route(connection_id: &str) -> String {
    format!("{START_ROUTE}/{}", segment(connection_id))
}

/// The maximum `NetworkInterfaceDeviceIndex` the model allows, from
/// `smithy_query(query="operation",operation="StartConnection")` (`range(0..1)`),
/// whose minimum is 0.
const MAX_DEVICE_INDEX: i32 = 1;

/// The modelled `AssetId` maximum length (`len 1..100`, pattern `^(\w+)$`).
const MAX_ASSET_ID_LENGTH: usize = 100;

/// The modelled `DeviceSerialNumber` maximum length (`len 1..100`, pattern
/// `^(\w+)$`).
const MAX_DEVICE_SERIAL_LENGTH: usize = 100;

/// The modelled `ConnectionId` maximum length (`len 1..1024`).
const MAX_CONNECTION_ID_LENGTH: usize = 1024;

/// A `WireGuardPublicKey` whose 43 key characters are `filler`, repeated: the
/// modelled form and nothing else fixed, so a row can vary which characters of the
/// alphabet a client key carries.
fn client_key(filler: &str) -> String {
    let body: String = filler.chars().cycle().take(43).collect();
    format!("{body}=")
}

/// The modelled `WireGuardPublicKey` form, from
/// `smithy_query(query="shape",shape="WireGuardPublicKey")`: `len 44..44`, pattern
/// `^[a-zA-Z0-9/+]{43}=$` — 43 characters of the base64 alphabet and the `=` that
/// pads a 32-byte key.
fn is_wireguard_key(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 44
        && bytes[43] == b'='
        && bytes[..43]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'/' || *b == b'+')
}

/// A dotted quad: four octets, each one to three digits naming a value an IPv4
/// octet can hold. The modelled `UnderlayIpAddress` pattern is
/// `^([0-9]{1,3}\.){3}[0-9]{1,3}$`; an octet above 255 satisfies that pattern and is
/// still no address, so each one is parsed as well as counted.
fn is_dotted_quad(value: &str) -> bool {
    let octets: Vec<&str> = value.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|octet| {
            (1..=3).contains(&octet.len())
                && octet.bytes().all(|b| b.is_ascii_digit())
                && octet.parse::<u8>().is_ok()
        })
}

/// The modelled `CIDR` form (`len 9..18`, pattern
/// `^([0-9]{1,3}\.){3}[0-9]{1,3}/[0-9]{1,2}$`): a dotted quad and a prefix length no
/// IPv4 prefix can exceed.
fn is_cidr(value: &str) -> bool {
    match value.split_once('/') {
        Some((address, prefix)) => {
            is_dotted_quad(address)
                && (1..=2).contains(&prefix.len())
                && prefix.parse::<u8>().is_ok_and(|bits| bits <= 32)
        }
        None => false,
    }
}

/// The modelled `ServerEndpoint` form (`len 9..21`, pattern
/// `^([0-9]{1,3}\.){3}[0-9]{1,3}:[0-9]{1,5}$`): a dotted quad and a UDP port.
fn is_server_endpoint(value: &str) -> bool {
    match value.rsplit_once(':') {
        Some((address, port)) => {
            is_dotted_quad(address)
                && (1..=5).contains(&port.len())
                && port.parse::<u16>().is_ok()
        }
        None => false,
    }
}

/// The modelled `ConnectionId` form (`len 1..1024`, pattern
/// `^[a-zA-Z0-9+/=]{1,1024}$`): an opaque, base64-shaped identifier.
fn is_connection_id(value: &str) -> bool {
    (1..=MAX_CONNECTION_ID_LENGTH).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// A `ConnectionId` of the modelled form that this emulator never minted: every id
/// it mints is the base64 of a digest.
fn absent_connection_id() -> String {
    client_key("A")
}

/// An `AssetId` of the modelled form that no Outpost in a store holds.
const ABSENT_ASSET_ID: &str = "asset_nothing_holds";

/// A `StartConnection` payload carrying exactly the members a row names: `AssetId`
/// and `ClientPublicKey` always, the dongle serial and the device index only where
/// the row names them — the model marks `NetworkInterfaceDeviceIndex` `@required`,
/// so a row that omits it is a row about a refusal.
fn start_request(
    asset_id: &str,
    client_public_key: &str,
    device_serial_number: Option<&str>,
    index: Option<i32>,
) -> Value {
    let mut request = json!({
        "AssetId": asset_id,
        "ClientPublicKey": client_public_key,
    });
    let object = request.as_object_mut().expect("a JSON object");
    if let Some(serial) = device_serial_number {
        object.insert("DeviceSerialNumber".to_string(), json!(serial));
    }
    if let Some(index) = index {
        object.insert("NetworkInterfaceDeviceIndex".to_string(), json!(index));
    }
    request
}

/// One row's `StartConnection` request.
struct Case {
    case: &'static str,
    asset_id: String,
    /// The dongle serial the request names, or `None` for a request that names
    /// none — which must be persisted and reported as absent, never as a value.
    device_serial_number: Option<String>,
    /// The `NetworkInterfaceDeviceIndex` the request names: the model declares the
    /// member `@required`, so every request carries one.
    index: i32,
    client_public_key: String,
}

/// The payload a row sends.
fn body(case: &Case) -> Value {
    start_request(
        &case.asset_id,
        &case.client_public_key,
        case.device_serial_number.as_deref(),
        Some(case.index),
    )
}

/// A fresh Outpost on a fresh site with `asset_id` recorded as its hardware,
/// returning the Outpost's id. See this module's header.
async fn outpost_holding(outposts: &Outposts, name: &str, asset_id: &str) -> String {
    outpost_holding_all(outposts, name, &[asset_id]).await
}

/// The same, for an Outpost holding every asset in `asset_ids` — a rack carries more
/// than one unit of hardware, and each unit is its own installation.
async fn outpost_holding_all(outposts: &Outposts, name: &str, asset_ids: &[&str]) -> String {
    let (outpost_id, _) = outposts.site_and_outpost(name).await;
    let hardware: Vec<_> = asset_ids
        .iter()
        .map(|asset_id| asset(asset_id, "rack-1", None, None, None))
        .collect();
    record(outposts, &outpost_id, &hardware, name);
    outpost_id
}

/// The `ConnectionDetails` member of a `GetConnection` output.
fn details_of(output: &Value, case: &str) -> Value {
    output
        .get("ConnectionDetails")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectionDetails member, got {output}"))
}

/// The `AllowedIps` list of a connection's details.
fn allowed_ips(details: &Value, case: &str) -> Vec<String> {
    details
        .get("AllowedIps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an AllowedIps list, got {details}"))
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Rules 1, 2 and 3: every shape of request persists its connection, reads back
/// identically through `GetConnection`, and is numbered on a link of its own while
/// naming the one tunnel server the store reports.
///
/// The rows differ along everything the operations generalise over: whether the
/// dongle serial is present (and how long it is), which
/// `NetworkInterfaceDeviceIndex` the request names (both modelled bounds), which
/// characters of the key alphabet the client key carries, and which shape of
/// `AssetId` names the hardware (letters, digits, underscores and the modelled
/// length maximum).
#[tokio::test]
async fn a_started_connection_is_reported_from_what_the_start_persisted() {
    let outposts = Outposts::new();
    let cases: Vec<Case> = vec![
        Case {
            case: "no dongle serial, the minimum device index",
            asset_id: "asset_one".to_string(),
            device_serial_number: None,
            index: 0,
            client_public_key: client_key("B"),
        },
        Case {
            case: "a dongle serial and the maximum device index",
            asset_id: "asset_two".to_string(),
            device_serial_number: Some("DNG0000001".to_string()),
            index: MAX_DEVICE_INDEX,
            client_public_key: client_key("ab+/09Z"),
        },
        Case {
            case: "an asset id of digits only and a one-character serial",
            asset_id: "9".to_string(),
            device_serial_number: Some("0".to_string()),
            index: 0,
            client_public_key: client_key("0123456789"),
        },
        Case {
            case: "an asset id at the modelled length maximum",
            asset_id: "a".repeat(MAX_ASSET_ID_LENGTH),
            device_serial_number: None,
            index: MAX_DEVICE_INDEX,
            client_public_key: client_key("+"),
        },
        Case {
            case: "a dongle serial at the modelled length maximum",
            asset_id: "ASSET_with_mixed_Case_0009".to_string(),
            device_serial_number: Some("S".repeat(MAX_DEVICE_SERIAL_LENGTH)),
            index: 0,
            client_public_key: client_key("/"),
        },
    ];

    // Every row's connection id and the details it was answered with, re-read after
    // the whole table has run: rule 2's isolation, and rule 3's uniqueness.
    let mut started: Vec<(&str, String, Value)> = Vec::new();
    let mut underlays: Vec<(&str, String)> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let name = format!("connection-{index}");
        outpost_holding(&outposts, &name, &case.asset_id).await;

        let start = outposts.ok("POST", START_ROUTE, Some(&body(case))).await;
        let connection_id = text(&start, "ConnectionId");
        assert!(
            is_connection_id(&connection_id),
            "case {:?}: StartConnection must mint a ConnectionId of the modelled form, got {:?}",
            case.case,
            connection_id
        );
        let underlay = text(&start, "UnderlayIpAddress");
        assert!(
            is_dotted_quad(&underlay),
            "case {:?}: expected an UnderlayIpAddress of the modelled form, got {underlay:?}",
            case.case
        );

        // Rule 2: the read answers the stored record.
        let read = outposts.ok("GET", &route(&connection_id), None).await;
        assert_eq!(
            text(&read, "ConnectionId"),
            connection_id,
            "case {:?}: GetConnection must report the id the start minted, got {read}",
            case.case
        );
        let details = details_of(&read, case.case);
        assert_eq!(
            text(&details, "ClientPublicKey"),
            case.client_public_key,
            "case {:?}: the connection must report the client key it was started with, got \
             {details}",
            case.case
        );
        let server_key = text(&details, "ServerPublicKey");
        assert!(
            is_wireguard_key(&server_key),
            "case {:?}: expected a ServerPublicKey of the modelled form, got {server_key:?}",
            case.case
        );
        let endpoint = text(&details, "ServerEndpoint");
        assert!(
            is_server_endpoint(&endpoint),
            "case {:?}: expected a ServerEndpoint of the modelled form, got {endpoint:?}",
            case.case
        );
        let client_tunnel = text(&details, "ClientTunnelAddress");
        let server_tunnel = text(&details, "ServerTunnelAddress");
        for (member, value) in [
            ("ClientTunnelAddress", &client_tunnel),
            ("ServerTunnelAddress", &server_tunnel),
        ] {
            assert!(
                is_cidr(value),
                "case {:?}: expected a {member} of the modelled CIDR form, got {value:?}",
                case.case
            );
        }
        assert_ne!(
            client_tunnel, server_tunnel,
            "case {:?}: the two ends of one link are two different addresses",
            case.case
        );
        let allowed = allowed_ips(&details, case.case);
        assert!(
            !allowed.is_empty() && allowed.iter().all(|entry| is_cidr(entry)),
            "case {:?}: every AllowedIps entry must be of the modelled CIDR form, got {allowed:?}",
            case.case
        );

        // Rule 2's second half: a report changes nothing, so a second read answers
        // the same connection.
        let again = outposts.ok("GET", &route(&connection_id), None).await;
        assert_eq!(
            again, read,
            "case {:?}: a second GetConnection must answer the same connection, got {again}",
            case.case
        );

        // What the start persisted that no operation in this slice reports: the
        // asset the connection installs, the dongle serial, the device index the
        // request named and the underlay address the start answered with. Read off
        // the record, because the modelled output shapes carry no member for them.
        {
            let store = outposts.store();
            let store = store.lock();
            let persisted = store
                .connections
                .get(&connection_id)
                .unwrap_or_else(|| panic!("case {:?}: the start persisted it", case.case));
            assert_eq!(
                persisted.asset_id, case.asset_id,
                "case {:?}: the record must name the asset the connection installs",
                case.case
            );
            assert_eq!(
                persisted.device_serial_number.as_deref(),
                case.device_serial_number.as_deref(),
                "case {:?}: the record must keep the dongle serial the request named, and none \
                 where it named none",
                case.case
            );
            assert_eq!(
                persisted.network_interface_device_index, case.index,
                "case {:?}: the record must keep the device index the request named",
                case.case
            );
            assert_eq!(
                persisted.underlay_ip_address, underlay,
                "case {:?}: the record must keep the underlay address the start reported",
                case.case
            );
        }

        started.push((case.case, connection_id, details));
        underlays.push((case.case, underlay));
    }

    // Rule 3: one store reports ONE tunnel server...
    for member in ["ServerPublicKey", "ServerEndpoint"] {
        let reported: BTreeSet<String> = started
            .iter()
            .map(|(_, _, details)| text(details, member))
            .collect();
        assert_eq!(
            reported.len(),
            1,
            "every connection of one store must name the same {member}, got {reported:?}"
        );
    }
    // ...and every connection its own id, its own link and its own underlay address.
    for (member, values) in [
        (
            "ConnectionId",
            started
                .iter()
                .map(|(_, id, _)| id.clone())
                .collect::<Vec<String>>(),
        ),
        (
            "ClientTunnelAddress",
            started
                .iter()
                .map(|(_, _, details)| text(details, "ClientTunnelAddress"))
                .collect(),
        ),
        (
            "ServerTunnelAddress",
            started
                .iter()
                .map(|(_, _, details)| text(details, "ServerTunnelAddress"))
                .collect(),
        ),
        (
            "UnderlayIpAddress",
            underlays
                .iter()
                .map(|(_, underlay)| underlay.clone())
                .collect(),
        ),
    ] {
        let distinct: BTreeSet<&String> = values.iter().collect();
        assert_eq!(
            distinct.len(),
            values.len(),
            "no two connections may share a {member}, got {values:?}"
        );
    }

    // Rule 2 again, after every other row has started its own connection: each one
    // still answers its OWN record.
    for (case, connection_id, details) in &started {
        let read = outposts.ok("GET", &route(connection_id), None).await;
        assert_eq!(
            details_of(&read, case),
            *details,
            "case {case:?}: every connection keeps its own details after the others started, \
             got {read}"
        );
    }
}

/// Rule 4: an identifier that names nothing is 404 with this service's identifier
/// echo, a request outside a modelled constraint is 400, and neither persists
/// anything.
#[tokio::test]
async fn refusals_are_classified_and_persist_nothing() {
    let outposts = Outposts::new();
    let held_asset = "asset_held_by_an_outpost";
    outpost_holding(&outposts, "refusal-target", held_asset).await;
    let key = client_key("C");
    let valid = start_request(held_asset, &key, Some("DNG0000002"), Some(0));

    // One connection that exists, so the 404 rows below are about the identifier
    // they name and not about an empty store.
    let started = outposts.ok("POST", START_ROUTE, Some(&valid)).await;
    let connection_id = text(&started, "ConnectionId");
    let before = outposts.ok("GET", &route(&connection_id), None).await;
    let absent = absent_connection_id();

    let cases: Vec<Refusal> = vec![
        refusal(
            "GetConnection on a ConnectionId nothing holds",
            "GET",
            route(&absent),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetConnection on a one-character ConnectionId nothing holds",
            "GET",
            route("x"),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetConnection on a ConnectionId at the modelled maximum length that nothing holds",
            "GET",
            route(&"Z".repeat(MAX_CONNECTION_ID_LENGTH)),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetConnection on a label carrying a character the alphabet excludes",
            "GET",
            route("connection-id"),
            400,
            "ValidationException",
        ),
        refusal(
            "GetConnection on a label one character past the modelled maximum",
            "GET",
            route(&"Z".repeat(MAX_CONNECTION_ID_LENGTH + 1)),
            400,
            "ValidationException",
        ),
        refusal(
            "StartConnection for an asset no Outpost holds",
            "POST",
            START_ROUTE.to_string(),
            404,
            "NotFoundException",
        )
        .with_body(start_request(ABSENT_ASSET_ID, &key, None, Some(0))),
        refusal(
            "StartConnection with no AssetId",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "ClientPublicKey": key })),
        refusal(
            "StartConnection with no ClientPublicKey",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "AssetId": held_asset })),
        refusal(
            "StartConnection with no NetworkInterfaceDeviceIndex",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "AssetId": held_asset, "ClientPublicKey": key })),
        refusal(
            "StartConnection with a client key one character short",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(held_asset, &key[..43], None, Some(0))),
        refusal(
            "StartConnection with a client key carrying a character the alphabet excludes",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(
            held_asset,
            &client_key("-"),
            None,
            Some(0),
        )),
        refusal(
            "StartConnection with a client key that is not padded",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(held_asset, &"D".repeat(44), None, Some(0))),
        refusal(
            "StartConnection with a device index one past the modelled maximum",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(
            held_asset,
            &key,
            None,
            Some(MAX_DEVICE_INDEX + 1),
        )),
        refusal(
            "StartConnection with a device index below the modelled minimum",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(held_asset, &key, None, Some(-1))),
        refusal(
            "StartConnection with an AssetId outside its pattern",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request("asset-one", &key, None, Some(0))),
        refusal(
            "StartConnection with an AssetId one character past the modelled maximum",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(
            &"a".repeat(MAX_ASSET_ID_LENGTH + 1),
            &key,
            None,
            Some(0),
        )),
        refusal(
            "StartConnection with a DeviceSerialNumber outside its pattern",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(held_asset, &key, Some("DNG 0001"), Some(0))),
        refusal(
            "StartConnection with an empty DeviceSerialNumber",
            "POST",
            START_ROUTE.to_string(),
            400,
            "ValidationException",
        )
        .with_body(start_request(held_asset, &key, Some(""), Some(0))),
    ];

    outposts.expect_refusals(&cases).await;

    // The 404 bodies are this service's identifier echo: the member the identifier
    // arrived in and the value exactly as supplied.
    for (case, method, path, member, supplied, payload) in [
        (
            "an absent ConnectionId",
            "GET",
            route(&absent),
            "connectionId",
            absent.clone(),
            None,
        ),
        (
            "an asset no Outpost holds",
            "POST",
            START_ROUTE.to_string(),
            "assetId",
            ABSENT_ASSET_ID.to_string(),
            Some(start_request(ABSENT_ASSET_ID, &key, None, Some(0))),
        ),
    ] {
        let reply = outposts.call(method, &path, payload.as_ref()).await;
        assert_eq!(
            text(&reply.body, "Message"),
            format!("{member}={supplied}"),
            "case {case:?}: the 404 body must echo the identifier as supplied, got {}",
            reply.body
        );
    }

    // Nothing any refused call touched was persisted: the store still holds exactly
    // the one connection the valid start made, and it reads back unchanged.
    let held = outposts.store().lock().connections.len();
    assert_eq!(
        held, 1,
        "a refused call must persist no connection: expected 1 held, got {held}"
    );
    let after = outposts.ok("GET", &route(&connection_id), None).await;
    assert_eq!(
        after, before,
        "a refused call must leave the stored connection as it was, got {after}"
    );
}

/// One row of rule 5: the hardware the row's Outpost holds, and the sequence of
/// `StartConnection` payloads it sends against that hardware, in the order it sends
/// them.
struct Repeat {
    case: &'static str,
    /// Every `AssetId` the row's Outpost holds, in installation order.
    assets: &'static [&'static str],
    /// The starts the row sends, each naming one of `assets`.
    starts: Vec<Value>,
}

/// The rows of rule 5, differing along everything a repeat can repeat: how many
/// starts a row sends, whether the payload is byte-for-byte the one before it, which
/// member changes between them (the client key, the device index, the dongle serial)
/// and whether two starts name one asset or two assets of one Outpost.
fn repeats() -> Vec<Repeat> {
    let key = client_key("E");
    let other_key = client_key("F");
    vec![
        Repeat {
            case: "the same payload sent twice",
            assets: &["asset_repeat_same"],
            starts: vec![
                start_request("asset_repeat_same", &key, Some("DNG0000010"), Some(0)),
                start_request("asset_repeat_same", &key, Some("DNG0000010"), Some(0)),
            ],
        },
        Repeat {
            case: "one asset started three times, a new client key each time",
            assets: &["asset_repeat_rekeyed"],
            starts: vec![
                start_request("asset_repeat_rekeyed", &client_key("0"), None, Some(0)),
                start_request("asset_repeat_rekeyed", &client_key("1"), None, Some(0)),
                start_request("asset_repeat_rekeyed", &client_key("2"), None, Some(0)),
            ],
        },
        Repeat {
            case: "one asset started on each of the two modelled device indices",
            assets: &["asset_repeat_indices"],
            starts: vec![
                start_request("asset_repeat_indices", &other_key, None, Some(0)),
                start_request(
                    "asset_repeat_indices",
                    &other_key,
                    None,
                    Some(MAX_DEVICE_INDEX),
                ),
            ],
        },
        Repeat {
            case: "one asset started with and then without a dongle serial",
            assets: &["asset_repeat_serial"],
            starts: vec![
                start_request("asset_repeat_serial", &key, Some("DNG0000011"), Some(0)),
                start_request("asset_repeat_serial", &key, None, Some(0)),
            ],
        },
        Repeat {
            case: "each of the two assets one Outpost holds, started once",
            assets: &["asset_repeat_first", "asset_repeat_second"],
            starts: vec![
                start_request("asset_repeat_first", &key, None, Some(0)),
                start_request("asset_repeat_second", &key, None, Some(MAX_DEVICE_INDEX)),
            ],
        },
    ]
}

/// The `NetworkInterfaceDeviceIndex` a row's payload names. Every row carries one:
/// the model marks the member `@required`.
fn requested_index(payload: &Value, case: &str) -> i32 {
    let index = payload
        .get("NetworkInterfaceDeviceIndex")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: every row names a device index, got {payload}"));
    i32::try_from(index)
        .unwrap_or_else(|_| panic!("case {case:?}: the device index is a modelled integer"))
}

/// Rule 5: every accepted start mints its own connection, and leaves the ones
/// already started exactly as they were.
#[tokio::test]
async fn every_accepted_start_mints_a_connection_of_its_own() {
    let outposts = Outposts::new();
    // Every connection id the whole table started, so no two rows may collide
    // either.
    let mut minted: Vec<(&str, String)> = Vec::new();

    for (row, repeat) in repeats().iter().enumerate() {
        outpost_holding_all(&outposts, &format!("repeat-{row}"), repeat.assets).await;
        // The id of each start of THIS row and the body `GetConnection` answered it
        // with, so every later start of the row can be checked against them.
        let mut answered: Vec<(String, Value)> = Vec::new();

        for (nth, payload) in repeat.starts.iter().enumerate() {
            let before = minted.len();
            let start = outposts.ok("POST", START_ROUTE, Some(payload)).await;
            let connection_id = text(&start, "ConnectionId");
            assert!(
                is_connection_id(&connection_id),
                "case {:?} start {nth}: expected a ConnectionId of the modelled form, got \
                 {connection_id:?}",
                repeat.case
            );
            let held = outposts.store().lock().connections.len();
            assert_eq!(
                held,
                before + 1,
                "case {:?} start {nth}: an accepted start must add exactly one connection, \
                 expected {} held, got {held}",
                repeat.case,
                before + 1
            );

            // The new connection reports THIS start's request, not the one before it.
            let read = outposts.ok("GET", &route(&connection_id), None).await;
            let details = details_of(&read, repeat.case);
            assert_eq!(
                text(&details, "ClientPublicKey"),
                text(payload, "ClientPublicKey"),
                "case {:?} start {nth}: the connection must report the client key this start \
                 sent, got {details}",
                repeat.case
            );
            {
                let store = outposts.store();
                let store = store.lock();
                let persisted = store
                    .connections
                    .get(&connection_id)
                    .unwrap_or_else(|| panic!("case {:?} start {nth}: it persisted", repeat.case));
                assert_eq!(
                    persisted.asset_id,
                    text(payload, "AssetId"),
                    "case {:?} start {nth}: the record must name the asset this start installs",
                    repeat.case
                );
                assert_eq!(
                    persisted.device_serial_number.as_deref(),
                    payload.get("DeviceSerialNumber").and_then(Value::as_str),
                    "case {:?} start {nth}: the record must keep the dongle serial this start \
                     named, and none where it named none",
                    repeat.case
                );
                assert_eq!(
                    persisted.network_interface_device_index,
                    requested_index(payload, repeat.case),
                    "case {:?} start {nth}: the record must keep the device index this start \
                     named",
                    repeat.case
                );
            }

            // Every connection already started is its own, and unchanged.
            for (earlier, answer) in &answered {
                assert_ne!(
                    *earlier, connection_id,
                    "case {:?} start {nth}: a repeated start must mint a new ConnectionId, got \
                     the earlier one",
                    repeat.case
                );
                let again = outposts.ok("GET", &route(earlier), None).await;
                assert_eq!(
                    &again, answer,
                    "case {:?} start {nth}: a later start must leave the earlier connection as \
                     it was, got {again}",
                    repeat.case
                );
            }

            answered.push((connection_id.clone(), read));
            minted.push((repeat.case, connection_id));
        }
    }

    // One connection per accepted start, each under an id of its own, and nothing
    // else in the store.
    let distinct: BTreeSet<&String> = minted.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "no two accepted starts may share a ConnectionId, got {minted:?}"
    );
    let held = outposts.store().lock().connections.len();
    assert_eq!(
        held,
        minted.len(),
        "the store must hold one connection per accepted start: expected {} held, got {held}",
        minted.len()
    );
}
