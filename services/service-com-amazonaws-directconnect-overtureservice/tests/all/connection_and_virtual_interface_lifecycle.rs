// SPDX-License-Identifier: CC0-1.0

//! The connection + private virtual interface lifecycle, end to end over the wire.
//!
//! Every assertion goes through `POST /` with `X-Amz-Target: OvertureService.<Op>`
//! (awsJson1_1), the same path a client takes, and every expectation is about what
//! the SERVICE STORED: a create is proved by reading the resource back through
//! `DescribeConnections` / `DescribeVirtualInterfaces`.
//!
//! The tables span the class each rule generalises over (bandwidth unit, tag count,
//! MTU, ASN width, name shape, filter combination, page size), so a row is one
//! instance of a rule rather than the rule itself.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Long enough for a `deleting`/`pending` span the service owes the clock to fall
/// due — the service's own `TRANSITION_DELAY` plus slack.
///
/// Awaited with `std::thread::sleep` rather than `tokio::time::sleep`: the
/// workspace enables tokio's `macros`/`rt` features only, so `tokio::time` is
/// not available here. Nothing else is pending on the test's runtime.
const SETTLE: Duration = Duration::from_millis(900);

/// One emulator: a router carrying only this service, over one shared engine.
///
/// `pub(crate)` so a sibling suite in this same test target (`tests/all/*.rs`)
/// drives the service through the SAME wire path rather than building a second
/// harness.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    region: String,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::in_region("us-east-1")
    }

    pub(crate) fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_directconnect_overtureservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self {
            router,
            region: region.to_string(),
            engine,
        }
    }

    /// The shared engine this emulator's handlers read and write — the one a
    /// sibling suite needs to set up state no operation creates (the Direct
    /// Connect location catalog, which is seeded with the store).
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// `POST /` with the operation's `X-Amz-Target`, returning the status and the
    /// decoded JSON body.
    pub(crate) async fn call(&self, target: &str, body: Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{}/directconnect/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
            self.region
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("https://directconnect.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("OvertureService.{target}"))
            .header("authorization", authorization)
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, body)
    }
}

/// One row of the create-connection table: case, location, bandwidth,
/// `providerName`, `requestMACSec`, `billingMode`, tags.
type ConnectionRow<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<bool>,
    Option<&'a str>,
    &'a [(&'a str, &'a str)],
);

/// One row of the virtual-interface table: case, vlan, extra members, expected
/// `asn`, expected `asnLong`, expected `mtu`.
type VirtualInterfaceRow<'a> = (&'a str, i64, Vec<(&'a str, Value)>, i64, Value, i64);

/// One row of the virtual-interface refusal table: case, connection id, name,
/// vlan, extra members, whether the request is accepted.
type VirtualInterfaceRuleRow<'a> = (&'a str, &'a str, String, i64, Vec<(&'a str, Value)>, bool);

/// One row of the tag table: case, tags, the error code expected (`None` when the
/// request is accepted).
type TagRow<'a> = (&'a str, Vec<(String, String)>, Option<&'a str>);

/// A server-minted id: `<prefix><8 lowercase hex digits>`.
fn is_minted_id(prefix: &str, id: &Value) -> bool {
    id.as_str()
        .and_then(|id| id.strip_prefix(prefix))
        .is_some_and(|suffix| {
            suffix.len() == 8
                && suffix
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
}

/// The error code an awsJson1_1 error body carries in `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A `CreateConnection` body with only the required members set.
fn create_connection_body(name: &str, location: &str, bandwidth: &str) -> Value {
    json!({
        "connectionName": name,
        "location": location,
        "bandwidth": bandwidth,
    })
}

/// A `CreatePrivateVirtualInterface` body over `connection_id`, with `extra`
/// members merged into `newPrivateVirtualInterface`.
fn create_vif_body(connection_id: &str, name: &str, vlan: i64, extra: &[(&str, Value)]) -> Value {
    let mut requested = Map::new();
    requested.insert("virtualInterfaceName".to_string(), json!(name));
    requested.insert("vlan".to_string(), json!(vlan));
    requested.insert("virtualGatewayId".to_string(), json!("vgw-0123456789abcdef0"));
    for (member, value) in extra {
        requested.insert((*member).to_string(), value.clone());
    }
    json!({
        "connectionId": connection_id,
        "newPrivateVirtualInterface": Value::Object(requested),
    })
}

/// Create one connection and return its id, failing the test if it was refused.
async fn connection(dx: &Emulator, name: &str, location: &str, bandwidth: &str) -> String {
    let (status, body) = dx
        .call(
            "CreateConnection",
            create_connection_body(name, location, bandwidth),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateConnection to succeed, got {status} with {body}"
    );
    body["connectionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_created_connection_is_minted_stored_and_read_back_field_for_field() {
    let dx = Emulator::new();

    // (case, location, bandwidth, providerName, requestMACSec, billingMode, tags)
    let rows: &[ConnectionRow<'_>] = &[
        (
            "dedicated gigabit port, nothing optional set",
            "EqDC2",
            "1Gbps",
            None,
            None,
            None,
            &[],
        ),
        (
            "ten gigabit port with MACsec, a provider and two tags",
            "CSOW",
            "10Gbps",
            Some("Example Provider"),
            Some(true),
            Some("PayAsYouGo"),
            &[("env", "prod"), ("team", "network")],
        ),
        (
            "sub-gigabit bandwidth quoted in Mbps",
            "TIVIT",
            "200Mbps",
            None,
            Some(false),
            None,
            &[("only", "one-tag")],
        ),
        (
            "hundred gigabit port on a flat-rate billing tier",
            "EqDA2",
            "100Gbps",
            None,
            None,
            Some("FlatRateTier5"),
            &[],
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, location, bandwidth, provider, request_mac_sec, billing_mode, tags) in rows {
        let mut body = Map::new();
        body.insert(
            "connectionName".to_string(),
            json!(format!("conn-{}", minted.len())),
        );
        body.insert("location".to_string(), json!(location));
        body.insert("bandwidth".to_string(), json!(bandwidth));
        if let Some(provider) = provider {
            body.insert("providerName".to_string(), json!(provider));
        }
        if let Some(request_mac_sec) = request_mac_sec {
            body.insert("requestMACSec".to_string(), json!(request_mac_sec));
        }
        if let Some(billing_mode) = billing_mode {
            body.insert("billingMode".to_string(), json!(billing_mode));
        }
        if !tags.is_empty() {
            body.insert(
                "tags".to_string(),
                Value::Array(
                    tags.iter()
                        .map(|(key, value)| json!({"key": key, "value": value}))
                        .collect(),
                ),
            );
        }

        let (status, created) = dx.call("CreateConnection", Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {created}"
        );
        assert!(
            is_minted_id("dxcon-", &created["connectionId"]),
            "case {case:?}: expected a server-minted dxcon-xxxxxxxx id, got {:?}",
            created["connectionId"]
        );
        assert_eq!(
            created["connectionState"],
            json!("requested"),
            "case {case:?}: expected connectionState requested, got {:?}",
            created["connectionState"]
        );
        assert_eq!(
            created["vlan"],
            json!(0),
            "case {case:?}: a dedicated port carries no VLAN tag, got {:?}",
            created["vlan"]
        );
        assert_eq!(
            created["ownerAccount"],
            json!(engine::EMULATOR_ACCOUNT_ID),
            "case {case:?}: expected the emulator account, got {:?}",
            created["ownerAccount"]
        );
        assert_eq!(
            created["region"],
            json!("us-east-1"),
            "case {case:?}: expected the signed Region, got {:?}",
            created["region"]
        );
        assert_eq!(
            created["location"],
            json!(location),
            "case {case:?}: expected the requested location, got {:?}",
            created["location"]
        );
        assert_eq!(
            created["bandwidth"],
            json!(bandwidth),
            "case {case:?}: expected the requested bandwidth, got {:?}",
            created["bandwidth"]
        );
        // Jumbo frames belong to a dedicated port, whose bandwidth AWS quotes in
        // Gbps; a sub-gigabit bandwidth is quoted in Mbps and is not capable.
        assert_eq!(
            created["jumboFrameCapable"],
            json!(bandwidth.ends_with("Gbps")),
            "case {case:?}: jumboFrameCapable must follow the bandwidth unit, got {:?}",
            created["jumboFrameCapable"]
        );
        assert_eq!(
            created["macSecCapable"],
            json!(request_mac_sec.unwrap_or(false)),
            "case {case:?}: macSecCapable must follow requestMACSec, got {:?}",
            created["macSecCapable"]
        );
        assert_eq!(
            created["billingMode"],
            billing_mode.map_or(Value::Null, |mode| json!(mode)),
            "case {case:?}: expected the requested billing mode, got {:?}",
            created["billingMode"]
        );
        assert_eq!(
            created["tags"].as_array().map_or(0, Vec::len),
            tags.len(),
            "case {case:?}: expected {} tags, got {:?}",
            tags.len(),
            created["tags"]
        );
        for device in ["awsDevice", "awsDeviceV2", "awsLogicalDeviceId"] {
            let reported = created[device].as_str().unwrap_or_default();
            assert!(
                reported.starts_with(&format!("{location}-")),
                "case {case:?}: {device} must name the location's endpoint, got {reported:?}"
            );
        }

        // The stored record, read back: field for field what the create returned.
        let id = created["connectionId"].as_str().unwrap_or_default();
        let (status, described) = dx
            .call("DescribeConnections", json!({"connectionId": id}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeConnections 200, got {status} with {described}"
        );
        assert_eq!(
            described["connections"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected exactly the named connection, got {:?}",
            described["connections"]
        );
        assert_eq!(
            described["connections"][0], created,
            "case {case:?}: the describe must render the stored record, got {:?}",
            described["connections"][0]
        );

        minted.push(id.to_string());
    }

    // Ids are unique, and a describe with no filter answers every stored one.
    let (status, all) = dx.call("DescribeConnections", json!({})).await;
    assert_eq!(status, 200, "expected 200 listing all, got {status}: {all}");
    let listed: Vec<String> = all["connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|connection| connection["connectionId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed, minted,
        "expected every stored connection in creation order, got {listed:?}"
    );
}

#[tokio::test]
async fn the_stored_region_comes_from_the_requests_credential_scope() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let dx = Emulator::in_region(region);
        let (status, created) = dx
            .call(
                "CreateConnection",
                create_connection_body("regional", "EqDC2", "1Gbps"),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} with {created}"
        );
        assert_eq!(
            created["region"],
            json!(region),
            "case {region:?}: expected the signed Region, got {:?}",
            created["region"]
        );
    }
}

#[tokio::test]
async fn a_private_virtual_interface_stores_what_was_supplied_and_mints_its_id() {
    let dx = Emulator::new();
    let connection_id = connection(&dx, "vif-host", "EqDC2", "10Gbps").await;

    // (case, vlan, extra members, expected asn, expected asnLong, expected mtu)
    let rows: &[VirtualInterfaceRow<'_>] = &[
        (
            "two-byte asn, defaults elsewhere",
            101,
            vec![("asn", json!(65_000))],
            65_000,
            json!(65_000),
            1500,
        ),
        (
            "jumbo MTU on a jumbo-capable port",
            102,
            vec![("asn", json!(64_512)), ("mtu", json!(8500))],
            64_512,
            json!(64_512),
            8500,
        ),
        (
            "largest documented asn",
            103,
            vec![("asn", json!(2_147_483_646i64))],
            2_147_483_646,
            json!(2_147_483_646i64),
            1500,
        ),
        (
            "four-byte asnLong reports 0 in the legacy asn member",
            104,
            vec![("asnLong", json!(4_294_967_294i64))],
            0,
            json!(4_294_967_294i64),
            1500,
        ),
        (
            "asnLong wins when both are supplied",
            105,
            vec![("asn", json!(65_000)), ("asnLong", json!(131_072))],
            131_072,
            json!(131_072),
            1500,
        ),
        (
            "explicit addresses, auth key, site link and ipv6 family",
            106,
            vec![
                ("asn", json!(65_001)),
                ("addressFamily", json!("ipv6")),
                ("authKey", json!("secret-key")),
                ("amazonAddress", json!("2600:1f16::1/125")),
                ("customerAddress", json!("2600:1f16::2/125")),
                ("enableSiteLink", json!(true)),
            ],
            65_001,
            json!(65_001),
            1500,
        ),
    ];

    for (case, vlan, extra, expected_asn, expected_asn_long, expected_mtu) in rows {
        let name = format!("vif-{vlan}");
        let (status, created) = dx
            .call(
                "CreatePrivateVirtualInterface",
                create_vif_body(&connection_id, &name, *vlan, extra),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {created}"
        );
        assert!(
            is_minted_id("dxvif-", &created["virtualInterfaceId"]),
            "case {case:?}: expected a server-minted dxvif-xxxxxxxx id, got {:?}",
            created["virtualInterfaceId"]
        );
        assert_eq!(
            created["virtualInterfaceState"],
            json!("pending"),
            "case {case:?}: expected virtualInterfaceState pending, got {:?}",
            created["virtualInterfaceState"]
        );
        assert_eq!(
            created["virtualInterfaceType"],
            json!("private"),
            "case {case:?}: expected virtualInterfaceType private, got {:?}",
            created["virtualInterfaceType"]
        );
        assert_eq!(
            created["vlan"],
            json!(vlan),
            "case {case:?}: expected the supplied vlan, got {:?}",
            created["vlan"]
        );
        assert_eq!(
            created["asn"],
            json!(expected_asn),
            "case {case:?}: expected asn {expected_asn}, got {:?}",
            created["asn"]
        );
        assert_eq!(
            &created["asnLong"], expected_asn_long,
            "case {case:?}: expected asnLong {expected_asn_long}, got {:?}",
            created["asnLong"]
        );
        assert_eq!(
            created["mtu"],
            json!(expected_mtu),
            "case {case:?}: expected mtu {expected_mtu}, got {:?}",
            created["mtu"]
        );
        assert_eq!(
            created["connectionId"],
            json!(connection_id),
            "case {case:?}: expected the hosting connection, got {:?}",
            created["connectionId"]
        );
        assert_eq!(
            created["virtualInterfaceName"],
            json!(name),
            "case {case:?}: expected the supplied name, got {:?}",
            created["virtualInterfaceName"]
        );
        assert!(
            created["amazonAddress"].is_string() && created["customerAddress"].is_string(),
            "case {case:?}: both peering addresses must be reported, got {:?} and {:?}",
            created["amazonAddress"],
            created["customerAddress"]
        );
        for (member, value) in extra {
            if *member == "amazonAddress" || *member == "customerAddress" {
                assert_eq!(
                    &created[*member], value,
                    "case {case:?}: expected the supplied {member}, got {:?}",
                    created[*member]
                );
            }
        }

        // Read it back: the stored record, not a rebuilt one.
        let id = created["virtualInterfaceId"].as_str().unwrap_or_default();
        let (status, described) = dx
            .call(
                "DescribeVirtualInterfaces",
                json!({"virtualInterfaceId": id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeVirtualInterfaces 200, got {status} with {described}"
        );
        assert_eq!(
            described["virtualInterfaces"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected exactly the named interface, got {:?}",
            described["virtualInterfaces"]
        );
        let read_back = &described["virtualInterfaces"][0];
        for member in [
            "virtualInterfaceId",
            "connectionId",
            "virtualInterfaceName",
            "vlan",
            "asn",
            "asnLong",
            "mtu",
            "addressFamily",
            "amazonAddress",
            "customerAddress",
            "authKey",
            "siteLinkEnabled",
            "virtualGatewayId",
            "ownerAccount",
            "location",
            "region",
        ] {
            assert_eq!(
                read_back[member], created[member],
                "case {case:?}: {member} must read back as stored, got {:?}",
                read_back[member]
            );
        }
    }
}

#[tokio::test]
async fn a_private_virtual_interface_request_outside_a_documented_rule_is_refused() {
    let dx = Emulator::new();
    let gigabit = connection(&dx, "jumbo-capable", "EqDC2", "10Gbps").await;
    let sub_gigabit = connection(&dx, "not-jumbo-capable", "TIVIT", "200Mbps").await;

    // (case, connection, name, vlan, extra, accepted)
    // Bounds come from the model's own docs: mtu "supported values are 1500 and
    // 8500", asn "valid values are 1-2147483646", asnLong "from 1 to 4294967294",
    // and the name "has a maximum of 100 characters ... a-z, 0-9 and a hyphen (-)".
    let rows: &[VirtualInterfaceRuleRow<'_>] = &[
        (
            "default MTU",
            &gigabit,
            "mtu-default".to_string(),
            201,
            vec![("asn", json!(65_000))],
            true,
        ),
        (
            "jumbo MTU on a jumbo-capable port",
            &gigabit,
            "mtu-jumbo".to_string(),
            202,
            vec![("asn", json!(65_000)), ("mtu", json!(8500))],
            true,
        ),
        (
            "jumbo MTU on a sub-gigabit port",
            &sub_gigabit,
            "mtu-jumbo-sub".to_string(),
            203,
            vec![("asn", json!(65_000)), ("mtu", json!(8500))],
            false,
        ),
        (
            "an MTU the model does not list",
            &gigabit,
            "mtu-9001".to_string(),
            204,
            vec![("asn", json!(65_000)), ("mtu", json!(9001))],
            false,
        ),
        (
            "one past the smallest listed MTU",
            &gigabit,
            "mtu-1501".to_string(),
            205,
            vec![("asn", json!(65_000)), ("mtu", json!(1501))],
            false,
        ),
        (
            "smallest valid asn",
            &gigabit,
            "asn-min".to_string(),
            206,
            vec![("asn", json!(1))],
            true,
        ),
        (
            "no asn at all",
            &gigabit,
            "asn-absent".to_string(),
            207,
            vec![],
            false,
        ),
        (
            "one past the largest asn",
            &gigabit,
            "asn-over".to_string(),
            208,
            vec![("asn", json!(2_147_483_647i64))],
            false,
        ),
        (
            "largest valid asnLong",
            &gigabit,
            "asnlong-max".to_string(),
            209,
            vec![("asnLong", json!(4_294_967_294i64))],
            true,
        ),
        (
            "one past the largest asnLong",
            &gigabit,
            "asnlong-over".to_string(),
            210,
            vec![("asnLong", json!(4_294_967_295i64))],
            false,
        ),
        (
            "a name at the 100 character maximum",
            &gigabit,
            "n".repeat(100),
            211,
            vec![("asn", json!(65_000))],
            true,
        ),
        (
            "a name one character past the maximum",
            &gigabit,
            "n".repeat(101),
            212,
            vec![("asn", json!(65_000))],
            false,
        ),
        (
            "a name carrying characters outside a-z, 0-9 and a hyphen",
            &gigabit,
            "Not_A_Valid_Name".to_string(),
            213,
            vec![("asn", json!(65_000))],
            false,
        ),
    ];

    for (case, connection_id, name, vlan, extra, accepted) in rows {
        let (status, body) = dx
            .call(
                "CreatePrivateVirtualInterface",
                create_vif_body(connection_id, name, *vlan, extra),
            )
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("DirectConnectClientException"),
                "case {case:?}: expected DirectConnectClientException, got {:?}",
                error_code(&body)
            );
        }
    }

    // A gateway is required, and exactly one of the two members names it.
    let gateway_rows: &[(&str, Vec<(&str, Value)>)] = &[
        ("neither gateway member", vec![]),
        (
            "both gateway members",
            vec![
                ("virtualGatewayId", json!("vgw-0123456789abcdef0")),
                (
                    "directConnectGatewayId",
                    json!("11460968-4ac1-4fd3-bdb2-00599e4b0b41"),
                ),
            ],
        ),
        (
            "a Direct Connect gateway this store does not hold",
            vec![(
                "directConnectGatewayId",
                json!("11460968-4ac1-4fd3-bdb2-00599e4b0b41"),
            )],
        ),
    ];
    for (case, members) in gateway_rows {
        let mut requested = Map::new();
        requested.insert("virtualInterfaceName".to_string(), json!("gateway-case"));
        requested.insert("vlan".to_string(), json!(301));
        requested.insert("asn".to_string(), json!(65_000));
        for (member, value) in members {
            requested.insert((*member).to_string(), value.clone());
        }
        let (status, body) = dx
            .call(
                "CreatePrivateVirtualInterface",
                json!({
                    "connectionId": gigabit,
                    "newPrivateVirtualInterface": Value::Object(requested),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn a_vlan_is_unique_among_the_live_interfaces_of_one_connection() {
    let dx = Emulator::new();
    let first = connection(&dx, "vlan-first", "EqDC2", "1Gbps").await;
    let second = connection(&dx, "vlan-second", "EqDC2", "1Gbps").await;

    for vlan in [1, 101, 4094] {
        let (status, body) = dx
            .call(
                "CreatePrivateVirtualInterface",
                create_vif_body(&first, &format!("first-{vlan}"), vlan, &[("asn", json!(65_000))]),
            )
            .await;
        assert_eq!(
            status, 200,
            "case vlan {vlan} on an empty connection: expected 200, got {status} with {body}"
        );

        let (status, body) = dx
            .call(
                "CreatePrivateVirtualInterface",
                create_vif_body(
                    &first,
                    &format!("clash-{vlan}"),
                    vlan,
                    &[("asn", json!(65_000))],
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case vlan {vlan} reused on the same connection: expected a refusal, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("DirectConnectClientException"),
            "case vlan {vlan} reused on the same connection: expected DirectConnectClientException, got {:?}",
            error_code(&body)
        );

        // The same VLAN on a DIFFERENT port is a different 802.1Q tag.
        let (status, body) = dx
            .call(
                "CreatePrivateVirtualInterface",
                create_vif_body(
                    &second,
                    &format!("second-{vlan}"),
                    vlan,
                    &[("asn", json!(65_000))],
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case vlan {vlan} on another connection: expected 200, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn describe_virtual_interfaces_filters_the_stored_set() {
    let dx = Emulator::new();
    let first = connection(&dx, "filter-first", "EqDC2", "1Gbps").await;
    let second = connection(&dx, "filter-second", "CSOW", "1Gbps").await;

    let mut created: Vec<(String, String)> = Vec::new();
    for (connection_id, vlan) in [(&first, 11), (&first, 12), (&second, 13)] {
        let (status, body) = dx
            .call(
                "CreatePrivateVirtualInterface",
                create_vif_body(
                    connection_id,
                    &format!("vif-{vlan}"),
                    vlan,
                    &[("asn", json!(65_000))],
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup vlan {vlan}: expected 200, got {status} with {body}"
        );
        created.push((
            connection_id.clone(),
            body["virtualInterfaceId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }
    let all: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();
    let of_first: Vec<String> = created
        .iter()
        .filter(|(connection_id, _)| connection_id == &first)
        .map(|(_, id)| id.clone())
        .collect();

    // (case, filter, expected ids)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter", json!({}), all.clone()),
        (
            "by connectionId",
            json!({"connectionId": first}),
            of_first.clone(),
        ),
        (
            "by the other connectionId",
            json!({"connectionId": second}),
            vec![all[2].clone()],
        ),
        (
            "by virtualInterfaceId",
            json!({"virtualInterfaceId": all[1]}),
            vec![all[1].clone()],
        ),
        (
            "by both, consistently",
            json!({"connectionId": first, "virtualInterfaceId": all[0]}),
            vec![all[0].clone()],
        ),
        (
            "by both, inconsistently",
            json!({"connectionId": second, "virtualInterfaceId": all[0]}),
            vec![],
        ),
    ];

    for (case, filter, expected) in rows {
        let (status, body) = dx.call("DescribeVirtualInterfaces", filter).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let listed: Vec<String> = body["virtualInterfaces"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|interface| {
                interface["virtualInterfaceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn deleting_moves_the_resource_through_deleting_to_deleted() {
    let dx = Emulator::new();
    let connection_id = connection(&dx, "delete-me", "EqDC2", "1Gbps").await;
    let (status, created) = dx
        .call(
            "CreatePrivateVirtualInterface",
            create_vif_body(&connection_id, "doomed-vif", 401, &[("asn", json!(65_000))]),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected the virtual interface to be created, got {status} with {created}"
    );
    let interface_id = created["virtualInterfaceId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, deleted) = dx
        .call(
            "DeleteVirtualInterface",
            json!({"virtualInterfaceId": interface_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case delete a live interface: expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        deleted["virtualInterfaceState"],
        json!("deleting"),
        "case delete a live interface: the model documents `deleting` immediately after the call, got {:?}",
        deleted["virtualInterfaceState"]
    );

    let (_, described) = dx
        .call(
            "DescribeVirtualInterfaces",
            json!({"virtualInterfaceId": interface_id}),
        )
        .await;
    assert_eq!(
        described["virtualInterfaces"][0]["virtualInterfaceState"],
        json!("deleting"),
        "case read straight back: the transient state must be observable, got {:?}",
        described["virtualInterfaces"][0]["virtualInterfaceState"]
    );

    std::thread::sleep(SETTLE);
    let (_, described) = dx
        .call(
            "DescribeVirtualInterfaces",
            json!({"virtualInterfaceId": interface_id}),
        )
        .await;
    assert_eq!(
        described["virtualInterfaces"][0]["virtualInterfaceState"],
        json!("deleted"),
        "case read after the span: expected deleted, got {:?}",
        described["virtualInterfaces"][0]["virtualInterfaceState"]
    );

    let (status, body) = dx
        .call(
            "DeleteVirtualInterface",
            json!({"virtualInterfaceId": interface_id}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case delete twice: expected a refusal, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("DirectConnectClientException"),
        "case delete twice: expected DirectConnectClientException, got {:?}",
        error_code(&body)
    );

    // A fresh interface on the same port, then delete the PORT: the interface
    // cannot outlive the connection it is provisioned on.
    let (status, survivor) = dx
        .call(
            "CreatePrivateVirtualInterface",
            create_vif_body(&connection_id, "cascade-vif", 402, &[("asn", json!(65_000))]),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected a second virtual interface, got {status} with {survivor}"
    );
    let survivor_id = survivor["virtualInterfaceId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, deleted) = dx
        .call("DeleteConnection", json!({"connectionId": connection_id}))
        .await;
    assert_eq!(
        status, 200,
        "case delete a live connection: expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        deleted["connectionState"],
        json!("deleting"),
        "case delete a live connection: the model documents `deleting` while it is being deleted, got {:?}",
        deleted["connectionState"]
    );
    assert_eq!(
        deleted["connectionId"],
        json!(connection_id),
        "case delete a live connection: expected the whole stored record, got {:?}",
        deleted["connectionId"]
    );
    assert_eq!(
        deleted["connectionName"],
        json!("delete-me"),
        "case delete a live connection: expected the stored name, got {:?}",
        deleted["connectionName"]
    );

    let (_, described) = dx
        .call(
            "DescribeVirtualInterfaces",
            json!({"virtualInterfaceId": survivor_id}),
        )
        .await;
    assert_eq!(
        described["virtualInterfaces"][0]["virtualInterfaceState"],
        json!("deleting"),
        "case delete the hosting port: its interface must be going too, got {:?}",
        described["virtualInterfaces"][0]["virtualInterfaceState"]
    );

    std::thread::sleep(SETTLE);
    let (status, described) = dx
        .call("DescribeConnections", json!({"connectionId": connection_id}))
        .await;
    assert_eq!(
        status, 200,
        "case read the deleted connection: expected 200, got {status} with {described}"
    );
    assert_eq!(
        described["connections"][0]["connectionState"],
        json!("deleted"),
        "case read after the span: expected deleted, got {:?}",
        described["connections"][0]["connectionState"]
    );

    let (status, body) = dx
        .call("DeleteConnection", json!({"connectionId": connection_id}))
        .await;
    assert_eq!(
        status, 400,
        "case delete the connection twice: expected a refusal, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("DirectConnectClientException"),
        "case delete the connection twice: expected DirectConnectClientException, got {:?}",
        error_code(&body)
    );
}

#[tokio::test]
async fn an_identifier_that_names_nothing_is_refused_not_answered_with_an_empty_page() {
    let dx = Emulator::new();
    let live = connection(&dx, "a-real-connection", "EqDC2", "1Gbps").await;

    // (case, target, body)
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "DescribeConnections with an unknown connection",
            "DescribeConnections",
            json!({"connectionId": "dxcon-ffffffff"}),
        ),
        (
            "DeleteConnection with an unknown connection",
            "DeleteConnection",
            json!({"connectionId": "dxcon-ffffffff"}),
        ),
        (
            "DeleteVirtualInterface with an unknown interface",
            "DeleteVirtualInterface",
            json!({"virtualInterfaceId": "dxvif-ffffffff"}),
        ),
        (
            "DescribeVirtualInterfaces with an unknown interface",
            "DescribeVirtualInterfaces",
            json!({"virtualInterfaceId": "dxvif-ffffffff"}),
        ),
        (
            "DescribeVirtualInterfaces with an unknown connection",
            "DescribeVirtualInterfaces",
            json!({"connectionId": "dxcon-ffffffff"}),
        ),
        (
            "CreatePrivateVirtualInterface on an unknown connection",
            "CreatePrivateVirtualInterface",
            create_vif_body("dxcon-ffffffff", "orphan", 501, &[("asn", json!(65_000))]),
        ),
        (
            "CreateConnection naming a LAG this store does not hold",
            "CreateConnection",
            json!({
                "connectionName": "on-a-lag",
                "location": "EqDC2",
                "bandwidth": "1Gbps",
                "lagId": "dxlag-ffffffff",
            }),
        ),
        (
            "DescribeConnections with a pagination token naming nothing",
            "DescribeConnections",
            json!({"nextToken": "dxcon-ffffffff"}),
        ),
    ];

    for (case, target, body) in rows {
        let (status, response) = dx.call(target, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
    }

    // And the live resource is untouched by all of that.
    let (status, described) = dx
        .call("DescribeConnections", json!({"connectionId": live}))
        .await;
    assert_eq!(
        status, 200,
        "case the live connection still reads: expected 200, got {status} with {described}"
    );
}

#[tokio::test]
async fn tag_rules_are_enforced_on_create() {
    let dx = Emulator::new();

    // (case, tags, expected error fragment or None for accepted)
    let rows: Vec<TagRow<'_>> = vec![
        ("no tags", vec![], None),
        (
            "one tag",
            vec![("env".to_string(), "prod".to_string())],
            None,
        ),
        (
            "the largest accepted tag set",
            (0..50)
                .map(|n| (format!("key-{n}"), format!("value-{n}")))
                .collect(),
            None,
        ),
        (
            "one tag past the limit",
            (0..51)
                .map(|n| (format!("key-{n}"), format!("value-{n}")))
                .collect(),
            Some("TooManyTagsException"),
        ),
        (
            "a key repeated",
            vec![
                ("env".to_string(), "prod".to_string()),
                ("env".to_string(), "staging".to_string()),
            ],
            Some("DuplicateTagKeysException"),
        ),
        (
            // The count ceiling is checked first, so it bounds the per-tag work
            // the duplicate scan does on a caller-chosen list length.
            "past the limit AND repeating a key: the count ceiling dominates",
            (0..51).map(|_| ("env".to_string(), "prod".to_string())).collect(),
            Some("TooManyTagsException"),
        ),
    ];

    for (case, tags, expected_error) in rows {
        let mut body = Map::new();
        body.insert("connectionName".to_string(), json!("tagged"));
        body.insert("location".to_string(), json!("EqDC2"));
        body.insert("bandwidth".to_string(), json!("1Gbps"));
        // `TagList` has a minimum length of 1, so an empty set is the ABSENT
        // member, not an empty array.
        if !tags.is_empty() {
            body.insert(
                "tags".to_string(),
                Value::Array(
                    tags.iter()
                        .map(|(key, value)| json!({"key": key, "value": value}))
                        .collect(),
                ),
            );
        }
        let (status, response) = dx.call("CreateConnection", Value::Object(body)).await;
        match expected_error {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} with {response}"
                );
                assert_eq!(
                    response["tags"].as_array().map_or(0, Vec::len),
                    tags.len(),
                    "case {case:?}: expected {} tags stored, got {:?}",
                    tags.len(),
                    response["tags"]
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a refusal, got {status} with {response}"
                );
                assert!(
                    error_code(&response).contains(expected),
                    "case {case:?}: expected {expected}, got {:?}",
                    error_code(&response)
                );
            }
        }
    }
}

#[tokio::test]
async fn list_operations_page_with_max_results_and_next_token() {
    let dx = Emulator::new();
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(connection(&dx, &format!("page-{n}"), "EqDC2", "1Gbps").await);
    }

    // (case, page size, expected page lengths)
    let rows: &[(&str, i64, &[usize])] = &[
        ("one at a time", 1, &[1, 1, 1, 1, 1]),
        ("two at a time", 2, &[2, 2, 1]),
        ("exactly the whole set", 5, &[5]),
        ("more than the whole set", 50, &[5]),
    ];

    for (case, page_size, expected_lengths) in rows {
        let mut token: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        for (index, expected_length) in expected_lengths.iter().enumerate() {
            let mut body = Map::new();
            body.insert("maxResults".to_string(), json!(page_size));
            if let Some(token) = &token {
                body.insert("nextToken".to_string(), json!(token));
            }
            let (status, page) = dx
                .call("DescribeConnections", Value::Object(body))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} page {index}: expected 200, got {status} with {page}"
            );
            let listed = page["connections"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                listed.len(),
                *expected_length,
                "case {case:?} page {index}: expected {expected_length} items, got {}",
                listed.len()
            );
            seen.extend(
                listed
                    .iter()
                    .map(|c| c["connectionId"].as_str().unwrap_or_default().to_string()),
            );
            let is_last = index + 1 == expected_lengths.len();
            assert_eq!(
                page["nextToken"].is_null(),
                is_last,
                "case {case:?} page {index}: nextToken must be null only on the last page, got {:?}",
                page["nextToken"]
            );
            token = page["nextToken"].as_str().map(ToString::to_string);
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: paging must visit every stored connection exactly once, got {seen:?}"
        );
    }
}

/// The emulator's per-store memory ceiling: a create past it is refused with a
/// modelled error, and a caller that deletes what it created gets the room back.
#[tokio::test]
async fn each_store_refuses_to_grow_past_the_emulator_memory_ceiling() {
    // Mirrors `crate::state::MAX_RECORDS_PER_STORE`.
    const CEILING: usize = 1_000;
    let dx = Emulator::new();

    let mut first_id = String::new();
    for n in 0..CEILING {
        let (status, created) = dx
            .call(
                "CreateConnection",
                create_connection_body(&format!("bulk-{n}"), "EqDC2", "1Gbps"),
            )
            .await;
        assert_eq!(
            status, 200,
            "case connection {n} of the ceiling: expected 200, got {status} with {created}"
        );
        if n == 0 {
            first_id = created["connectionId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
    }

    let (status, refused) = dx
        .call(
            "CreateConnection",
            create_connection_body("one-too-many", "EqDC2", "1Gbps"),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one past the ceiling: expected a refusal, got {status} with {refused}"
    );
    assert!(
        error_code(&refused).contains("DirectConnectClientException"),
        "case one past the ceiling: expected DirectConnectClientException, got {:?}",
        error_code(&refused)
    );
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains(&CEILING.to_string()),
        "case one past the ceiling: the refusal must name the ceiling, got {:?}",
        refused["message"]
    );

    // Deleting one, and letting the clock finish the teardown, returns the room.
    let (status, deleted) = dx
        .call("DeleteConnection", json!({"connectionId": first_id}))
        .await;
    assert_eq!(
        status, 200,
        "case delete to make room: expected 200, got {status} with {deleted}"
    );
    std::thread::sleep(SETTLE);
    let (status, created) = dx
        .call(
            "CreateConnection",
            create_connection_body("after-reclaim", "EqDC2", "1Gbps"),
        )
        .await;
    assert_eq!(
        status, 200,
        "case create after reclaiming a torn-down record: expected 200, got {status} with {created}"
    );

    // The virtual-interface store carries the same ceiling, refused through the
    // limit error that operation models. A fresh emulator, so the connection
    // store's own ceiling is not in the way.
    let dx = Emulator::new();
    let host = connection(&dx, "vif-ceiling-host", "EqDC2", "1Gbps").await;
    for n in 0..CEILING {
        let vlan = i64::try_from(n).unwrap_or_default() + 1;
        let (status, body) = dx
            .call(
                "CreatePrivateVirtualInterface",
                create_vif_body(
                    &host,
                    &format!("bulk-vif-{n}"),
                    vlan,
                    &[("asn", json!(65_000))],
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case virtual interface {n} of the ceiling: expected 200, got {status} with {body}"
        );
    }
    let (status, refused) = dx
        .call(
            "CreatePrivateVirtualInterface",
            create_vif_body(&host, "one-too-many-vif", 4094, &[("asn", json!(65_000))]),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one virtual interface past the ceiling: expected a refusal, got {status} with {refused}"
    );
    assert!(
        error_code(&refused).contains("LimitExceededException"),
        "case one virtual interface past the ceiling: expected LimitExceededException, got {:?}",
        error_code(&refused)
    );
}
