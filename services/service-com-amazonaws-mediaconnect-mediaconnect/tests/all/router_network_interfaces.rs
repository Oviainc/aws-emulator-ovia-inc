// SPDX-License-Identifier: CC0-1.0

//! Router network interfaces end to end over HTTP: CreateRouterNetworkInterface ->
//! BatchGetRouterNetworkInterface.
//!
//! The subject is BatchGetRouterNetworkInterface: it must report, for the ARNs one request
//! names, the router network interfaces the STORE HOLDS — computed from the stored record,
//! each requested resource once — and one `Errors` entry for every named ARN the store holds
//! no interface for, rather than a value fixed in the handler. Its write side
//! (CreateRouterNetworkInterface) is implemented in the same change, because a reader with
//! nothing to read proves nothing.
//!
//! The cases below vary what those rules generalise over: which configuration union variant
//! (and so which network interface type) the interface carries, how many allow rules a
//! PUBLIC one lists (none, one, the modelled maximum) and how many security groups a VPC one
//! lists (the modelled minimum and maximum), whether the request names a Region / tags /
//! client token or lets the service derive them, how many ARNs a batch read names, in what
//! order, and whether each resolves, repeats or resolves nothing — and, for the two
//! association counts, how many router inputs and outputs name the interface.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.
//!
//! The rows also cover a PUBLIC configuration that repeats a CIDR: the model puts no
//! uniqueness rule on `AllowRules`, so it is STORED as the request gave it rather than
//! refused — only the modelled constraints refuse a configuration.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::{union, union_member};

/// An ARN satisfying the modelled `RouterNetworkInterfaceArn` pattern that no create minted,
/// so no store holds a router network interface for it.
///
/// `pub(crate)` because `update_router_network_interface` addresses the same absent interface.
pub(crate) const UNKNOWN_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:ffffffffffff";
/// A second one, for the cases that name more than one absent interface.
const OTHER_UNKNOWN_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:eeeeeeeeeeee";

/// A `FlowArn` matching the modelled pattern — an ARN of this service that is NOT a router
/// network interface ARN, so the batch read's modelled `Arns` pattern refuses it.
pub(crate) const FLOW_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000000a:rni-target";

/// `pub(crate)` because `update_router_network_interface` mints its subjects the same way,
/// and `crate::create_router_network_interface_rules` drives the same front door for the
/// create rules this suite does not cover.
pub(crate) async fn create_network_interface(
    emulator: &Emulator,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", "/v1/routerNetworkInterface", Some(body))
        .await
}

/// BatchGetRouterNetworkInterface binds its `Arns` member to the `arns` query parameter, one
/// parameter per ARN
/// (`smithy_query(query="operation",operation="BatchGetRouterNetworkInterface")`:
/// `GET /v1/routerNetworkInterfaces`, `Arns` `@httpQuery("arns")`).
///
/// `pub(crate)` because `update_router_network_interface` reads its subjects back through the
/// same implemented reader, as does `crate::create_router_network_interface_rules`.
pub(crate) async fn batch_get_network_interfaces(
    emulator: &Emulator,
    arns: &[&str],
) -> (u16, Option<String>, Value) {
    let query = arns
        .iter()
        .map(|arn| format!("arns={arn}"))
        .collect::<Vec<String>>()
        .join("&");
    let uri = if query.is_empty() {
        "/v1/routerNetworkInterfaces".to_string()
    } else {
        format!("/v1/routerNetworkInterfaces?{query}")
    };
    emulator.send("GET", &uri, None).await
}

/// Every ARN CreateRouterNetworkInterface mints must satisfy the modelled
/// `RouterNetworkInterfaceArn` constraint
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerNetworkInterface:[a-z0-9]{12}$`),
/// because GetRouterNetworkInterface / UpdateRouterNetworkInterface /
/// DeleteRouterNetworkInterface take it as an `@httpLabel` and BatchGetRouterNetworkInterface's
/// `Arns` member carries that same pattern.
///
/// The check runs through the crate's own encoding of the pattern and then re-asserts, field
/// by field, the parts it fixes, so a bug in that function cannot hide here.
fn assert_minted_interface_arn(case: &str, arn: &str, id: &str) {
    assert!(
        mc::is_mintable_router_network_interface_arn(arn),
        "case {case:?}: a minted RouterNetworkInterfaceArn must satisfy the modelled pattern, \
         got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        7,
        "case {case:?}: a router network interface ARN has 7 colon-separated segments, got \
         {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "routerNetworkInterface"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert_eq!(
        segments[6], id,
        "case {case:?}: the last ARN segment is the interface's own Id, got {arn:?}"
    );
    assert!(
        id.len() == 12
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "case {case:?}: the interface Id is 12 [a-z0-9] characters, got {id:?}"
    );
}

/// A timestamp the service reports, in seconds since the epoch.
///
/// `RouterNetworkInterface$CreatedAt` / `$UpdatedAt` carry `@timestampFormat("date-time")` in
/// the model, but this emulator's codec renders every timestamp in restJson1's DEFAULT
/// epoch-seconds form — the framework's generated member schemas carry no
/// `@timestampFormat`, for every service alike — so that is the form asserted here. The
/// value is read from the clock either way, which is what these cases are about.
pub(crate) fn reported_timestamp(case: &str, member: &str, value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be reported as a timestamp, got {value}")
    })
}

/// Now, in seconds since the epoch — what a reported timestamp is compared against.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The members of a `RouterNetworkInterface` that a read must report exactly as the create
/// stored them. `state` is excluded on purpose: it is the one member the CLOCK moves (see
/// [`the_clock_settles_a_new_router_network_interface_into_active`]).
const PERSISTED_MEMBERS: [&str; 10] = [
    "name",
    "arn",
    "id",
    "networkInterfaceType",
    "configuration",
    "associatedInputCount",
    "associatedOutputCount",
    "regionName",
    "createdAt",
    "updatedAt",
];

/// A PUBLIC configuration listing `count` distinct allow rules.
///
/// The CIDR of each rule satisfies the modelled `PublicRouterNetworkInterfaceRule$Cidr`
/// pattern (`smithy_query(query="shape",shape="PublicRouterNetworkInterfaceRule")`:
/// `^(?:[0-9]{1,3}\.){3}[0-9]{1,3}/([0-9]|[12][0-9]|3[0-2])$`), from the RFC 5737
/// documentation range.
pub(crate) fn public_configuration(count: usize) -> Value {
    let allow_rules: Vec<Value> = (0..count)
        .map(|index| json!({ "cidr": format!("198.51.{index}.0/24") }))
        .collect();
    union("Public", "public", json!({ "allowRules": allow_rules }))
}

/// A VPC configuration listing `count` security groups, in this suite's one subnet.
pub(crate) fn vpc_configuration(count: usize) -> Value {
    vpc_configuration_in_subnet(count, "subnet-0123456789abcdef0")
}

/// A VPC configuration listing `count` security groups in the subnet `subnet`.
///
/// The one definition of the VPC configuration shape in this test tree: `update_router_
/// network_interface` needs a row whose subnet DIFFERS from the stored one, which is the
/// only thing it varies.
pub(crate) fn vpc_configuration_in_subnet(count: usize, subnet: &str) -> Value {
    let security_group_ids: Vec<Value> = (0..count)
        .map(|index| json!(format!("sg-{index:017x}")))
        .collect();
    union(
        "Vpc",
        "vpc",
        json!({
            "securityGroupIds": security_group_ids,
            "subnetId": subnet,
        }),
    )
}

/// One create, and everything the stored router network interface must report for it.
struct CreateCase {
    case: &'static str,
    body: Value,
    /// `RouterNetworkInterface$NetworkInterfaceType`, which the service derives from the
    /// configuration's union variant rather than reading from the request.
    network_interface_type: &'static str,
    /// The union member the stored configuration must carry, as
    /// `(Smithy member name, @jsonName)`.
    configuration_member: (&'static str, &'static str),
    /// The payload the stored configuration's union member must carry.
    configuration_payload: Value,
    tags: Value,
}

/// The rows span the class the rules generalise over: both configuration variants (so both
/// network interface types), a PUBLIC interface with no allow rules, with one, and with the
/// modelled maximum of ten (`PublicRouterNetworkInterfaceConfiguration$AllowRules`, len
/// 0..10), a VPC interface at the modelled minimum and maximum security group count
/// (`VpcRouterNetworkInterfaceConfiguration$SecurityGroupIds`, len 1..5), a request that
/// names nothing optional beside ones that name the Region and the tags, and names of
/// different forms the modelled `Name` pattern accepts.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "public with no allow rules, nothing optional named",
            body: json!({
                "name": "rni-public-open",
                "configuration": public_configuration(0),
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            configuration_member: ("Public", "public"),
            configuration_payload: json!({ "allowRules": [] }),
            tags: json!({}),
        },
        CreateCase {
            case: "public with one allow rule, Region named explicitly",
            body: json!({
                "name": "rni-public-one",
                "configuration": public_configuration(1),
                "regionName": REGION,
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            configuration_member: ("Public", "public"),
            configuration_payload: json!({ "allowRules": [{ "cidr": "198.51.0.0/24" }] }),
            tags: json!({}),
        },
        CreateCase {
            case: "public at the modelled maximum of ten allow rules, with tags",
            body: json!({
                "name": "rni_public-ten",
                "configuration": public_configuration(10),
                "tags": { "env": "test", "team": "media" },
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            configuration_member: ("Public", "public"),
            configuration_payload: json!({
                "allowRules": (0..10)
                    .map(|index| json!({ "cidr": format!("198.51.{index}.0/24") }))
                    .collect::<Vec<Value>>(),
            }),
            tags: json!({ "env": "test", "team": "media" }),
        },
        CreateCase {
            case: "public repeating one allow rule CIDR, stored as the request gave it",
            body: json!({
                "name": "rni-public-repeated",
                "configuration": union(
                    "Public",
                    "public",
                    json!({"allowRules": [
                        {"cidr": "198.51.100.0/24"},
                        {"cidr": "198.51.100.0/24"},
                    ]}),
                ),
            }),
            // The model puts no uniqueness rule on `AllowRules`, so a repeated CIDR is
            // stored as given rather than refused or silently deduplicated: only the
            // modelled constraints (the CIDR pattern and the 0..10 bound) refuse a list.
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_PUBLIC,
            configuration_member: ("Public", "public"),
            configuration_payload: json!({"allowRules": [
                {"cidr": "198.51.100.0/24"},
                {"cidr": "198.51.100.0/24"},
            ]}),
            tags: json!({}),
        },
        CreateCase {
            case: "vpc at the modelled minimum of one security group",
            body: json!({
                "name": "rni-vpc-one",
                "configuration": vpc_configuration(1),
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC,
            configuration_member: ("Vpc", "vpc"),
            configuration_payload: json!({
                "securityGroupIds": ["sg-00000000000000000"],
                "subnetId": "subnet-0123456789abcdef0",
            }),
            tags: json!({}),
        },
        CreateCase {
            case: "vpc at the modelled maximum of five security groups, with one tag",
            body: json!({
                "name": "RNI0",
                "configuration": vpc_configuration(5),
                "tags": { "env": "test" },
            }),
            network_interface_type: mc::ROUTER_NETWORK_INTERFACE_TYPE_VPC,
            configuration_member: ("Vpc", "vpc"),
            configuration_payload: json!({
                "securityGroupIds": (0..5)
                    .map(|index| json!(format!("sg-{index:017x}")))
                    .collect::<Vec<Value>>(),
                "subnetId": "subnet-0123456789abcdef0",
            }),
            tags: json!({ "env": "test" }),
        },
    ]
}

#[tokio::test]
async fn create_router_network_interface_persists_what_batch_get_reports() {
    let emulator = Emulator::new();

    for CreateCase {
        case,
        body,
        network_interface_type,
        configuration_member,
        configuration_payload,
        tags,
    } in create_cases()
    {
        let requested_name = body["name"].as_str().unwrap_or_default().to_string();

        let (status, code, created) = create_network_interface(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {created}"
        );

        let interface = created["routerNetworkInterface"].clone();
        let arn = interface["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();
        let id = interface["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an Id: {created}"))
            .to_string();
        assert_minted_interface_arn(case, &arn, &id);

        for (member, expected) in [
            ("name", json!(requested_name)),
            ("networkInterfaceType", json!(network_interface_type)),
            ("regionName", json!(REGION)),
            ("state", json!(mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING)),
            ("tags", tags.clone()),
            // Nothing names a brand-new interface yet, so both association counts are
            // counted as zero rather than reported absent.
            ("associatedInputCount", json!(0)),
            ("associatedOutputCount", json!(0)),
        ] {
            assert_eq!(
                interface[member], expected,
                "case {case:?}: the created interface's {member} must be {expected}, got {:?}",
                interface[member]
            );
        }

        let (member_name, json_name) = configuration_member;
        assert_eq!(
            union_member(&interface["configuration"], member_name, json_name),
            &configuration_payload,
            "case {case:?}: the stored Configuration is the {member_name} member the request \
             carried, got {:?}",
            interface["configuration"]
        );

        let created_at = reported_timestamp(case, "createdAt", &interface["createdAt"]);
        let updated_at = reported_timestamp(case, "updatedAt", &interface["updatedAt"]);
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: nothing has updated a new interface, so UpdatedAt equals CreatedAt, \
             got {created_at} and {updated_at}"
        );
        assert!(
            (now_epoch_seconds() - created_at).abs() < 300.0,
            "case {case:?}: CreatedAt is read from the clock, got {created_at}"
        );

        // And the batch read reports the record the create stored, for that ARN alone.
        let (status, code, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: batch get expected 200, got {status} {code:?}: {read}"
        );
        assert_eq!(
            read["errors"],
            json!([]),
            "case {case:?}: an ARN the store holds yields no error entry, got {read}"
        );
        let reported = read["routerNetworkInterfaces"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: one requested ARN is reported once, got {read}"
        );
        for member in PERSISTED_MEMBERS {
            assert_eq!(
                reported[0][member], interface[member],
                "case {case:?}: BatchGetRouterNetworkInterface must report the created {member}, \
                 got {:?}",
                reported[0][member]
            );
        }
        let reported_state = reported[0]["state"].as_str().unwrap_or_default().to_string();
        assert!(
            [
                mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING,
                mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE
            ]
            .contains(&reported_state.as_str()),
            "case {case:?}: a read interface is CREATING or ACTIVE, got {reported_state:?}"
        );
    }
}

/// How one batch read's request list maps onto its two response lists.
struct BatchCase {
    case: &'static str,
    /// Indexes into the created interfaces, or `None` for an ARN nothing holds.
    requested: Vec<Option<usize>>,
    /// The created interfaces the response must report, in order.
    expected_interfaces: Vec<usize>,
    /// How many `Errors` entries the response must carry.
    expected_errors: usize,
}

/// Mint `count` interfaces over the wire, like any client's, and answer their ARNs.
async fn seed_interfaces(emulator: &Emulator, count: usize) -> Vec<String> {
    let mut arns = Vec::new();
    for index in 0..count {
        let (status, _, created) = create_network_interface(
            emulator,
            json!({
                "name": format!("rni-batch-{index}"),
                "configuration": public_configuration(1),
            }),
        )
        .await;
        assert_eq!(status, 201, "seeding create expected 201, got {created}");
        arns.push(
            created["routerNetworkInterface"]["arn"]
                .as_str()
                .expect("the create answers an ARN")
                .to_string(),
        );
    }
    arns
}

#[tokio::test]
async fn batch_get_reports_each_requested_arn_once_and_errors_for_the_rest() {
    let emulator = Emulator::new();
    let arns = seed_interfaces(&emulator, 3).await;

    let cases = vec![
        BatchCase {
            case: "one stored ARN",
            requested: vec![Some(1)],
            expected_interfaces: vec![1],
            expected_errors: 0,
        },
        BatchCase {
            case: "every stored ARN, in the order the request names them",
            requested: vec![Some(2), Some(0), Some(1)],
            expected_interfaces: vec![2, 0, 1],
            expected_errors: 0,
        },
        BatchCase {
            case: "a stored ARN either side of one nothing holds",
            requested: vec![Some(0), None, Some(2)],
            expected_interfaces: vec![0, 2],
            expected_errors: 1,
        },
        BatchCase {
            case: "only ARNs nothing holds",
            requested: vec![None, None],
            expected_interfaces: vec![],
            expected_errors: 2,
        },
        BatchCase {
            case: "a repeated ARN is one resource, reported once",
            requested: vec![Some(1), Some(1), Some(0)],
            expected_interfaces: vec![1, 0],
            expected_errors: 0,
        },
    ];

    for BatchCase {
        case,
        requested,
        expected_interfaces,
        expected_errors,
    } in cases
    {
        // Two distinct absent ARNs, so a case naming both gets two error entries.
        let mut unknown_arns = [UNKNOWN_ARN, OTHER_UNKNOWN_ARN].into_iter();
        let requested_arns: Vec<&str> = requested
            .iter()
            .map(|slot| match slot {
                Some(index) => arns[*index].as_str(),
                None => unknown_arns.next().unwrap_or(UNKNOWN_ARN),
            })
            .collect();

        let (status, code, read) = batch_get_network_interfaces(&emulator, &requested_arns).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: a batch read answers 200 whatever its ARNs resolve, got \
             {status} {code:?}: {read}"
        );

        let reported: Vec<String> = read["routerNetworkInterfaces"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|interface| interface["arn"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = expected_interfaces
            .iter()
            .map(|index| arns[*index].clone())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: RouterNetworkInterfaces must report the stored interfaces in \
             request order, got {read}"
        );

        let errors = read["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            expected_errors,
            "case {case:?}: expected {expected_errors} Errors entries, got {read}"
        );
        for error in &errors {
            let arn = error["arn"].as_str().unwrap_or_default().to_string();
            assert!(
                requested_arns.contains(&arn.as_str()) && !reported.contains(&arn),
                "case {case:?}: an error entry names a requested ARN that was not reported, \
                 got {error}"
            );
            assert_eq!(
                error["code"].as_str(),
                Some("NotFoundException"),
                "case {case:?}: an ARN the store holds nothing for carries the not-found code, \
                 got {error}"
            );
            assert_eq!(
                error["message"].as_str(),
                Some(mc::not_found_message(&arn).as_str()),
                "case {case:?}: the error message is this service's not-found envelope, got \
                 {error}"
            );
        }
    }

    // A read changes nothing a later caller sees: the same request answers the same body,
    // and the store still holds exactly the three interfaces the creates made.
    let every_arn: Vec<&str> = arns.iter().map(String::as_str).collect();
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    let (_, _, first) = batch_get_network_interfaces(&emulator, &every_arn).await;
    let (_, _, second) = batch_get_network_interfaces(&emulator, &every_arn).await;
    assert_eq!(
        first, second,
        "a batch read must not change what the next one sees, got {first} then {second}"
    );
    assert_eq!(
        stored_count(&emulator),
        3,
        "a read stores no router network interface, got {} stored",
        stored_count(&emulator)
    );
}

/// How many router network interfaces the store holds.
pub(crate) fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .router_network_interfaces
        .len()
}

#[tokio::test]
async fn the_clock_settles_a_new_router_network_interface_into_active() {
    let emulator = Emulator::new();
    let (status, _, created) = create_network_interface(
        &emulator,
        json!({
            "name": "rni-lifecycle",
            "configuration": vpc_configuration(2),
        }),
    )
    .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // A caller reading straight after the create still observes the transient state.
    let (_, _, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
    assert_eq!(
        read["routerNetworkInterfaces"][0]["state"].as_str(),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING),
        "an interface read straight after the create is still CREATING, got {read}"
    );

    // Wait out the transition the clock owes, and the settled state is reported.
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    let (_, _, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
    assert_eq!(
        read["routerNetworkInterfaces"][0]["state"].as_str(),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
        "the clock settles a new interface into ACTIVE, got {read}"
    );
    for member in PERSISTED_MEMBERS {
        assert_eq!(
            read["routerNetworkInterfaces"][0][member], created["routerNetworkInterface"][member],
            "settling the state leaves {member} as the create stored it, got {:?}",
            read["routerNetworkInterfaces"][0][member]
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_mints_no_second_router_network_interface() {
    let emulator = Emulator::new();
    let body = |name: &str, token: &str| {
        json!({
            "name": name,
            "configuration": public_configuration(2),
            "clientToken": token,
        })
    };

    let (status, _, first) = create_network_interface(&emulator, body("rni-token", "token-1")).await;
    assert_eq!(status, 201, "create expected 201, got {status}: {first}");
    let arn = first["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // The same token again — even carrying a different name — answers with the interface
    // that token already minted, and mints nothing.
    let (status, _, replay) =
        create_network_interface(&emulator, body("rni-token-renamed", "token-1")).await;
    assert_eq!(status, 201, "replay expected 201, got {status}: {replay}");
    assert_eq!(
        replay["routerNetworkInterface"]["arn"].as_str(),
        Some(arn.as_str()),
        "a replayed ClientToken answers with the interface it minted, got {replay}"
    );
    assert_eq!(
        stored_count(&emulator),
        1,
        "a replayed ClientToken mints no second interface, got {} stored",
        stored_count(&emulator)
    );

    // A DIFFERENT token is a different request: it mints its own interface.
    let (status, _, second) = create_network_interface(&emulator, body("rni-token", "token-2")).await;
    assert_eq!(status, 201, "create expected 201, got {status}: {second}");
    let other_arn = second["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();
    assert_ne!(
        other_arn, arn,
        "a different ClientToken mints a distinct interface, got {other_arn}"
    );
    let (_, _, read) = batch_get_network_interfaces(&emulator, &[&arn, &other_arn]).await;
    assert_eq!(
        read["routerNetworkInterfaces"]
            .as_array()
            .map(|interfaces| interfaces.len()),
        Some(2),
        "both minted interfaces are readable, got {read}"
    );
}

/// A CreateRouterInput body carrying `configuration`, with the required members every
/// create needs beside it.
fn router_input_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "configuration": configuration,
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "INPUT_20",
    })
}

/// Create one router input over the wire and answer its ARN.
async fn create_router_input(emulator: &Emulator, name: &str, configuration: Value) -> String {
    let (status, code, created) = emulator
        .send(
            "POST",
            "/v1/routerInput",
            Some(router_input_body(name, configuration)),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateRouterInput answers 201 for {name:?}, got {status} {code:?}: {created}"
    );
    created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create one router output over the wire on the network interface `interface_arn`.
async fn create_router_output(emulator: &Emulator, name: &str, interface_arn: &str, port: i64) {
    let body = json!({
        "name": name,
        "configuration": union(
            "Standard",
            "standard",
            json!({
                "networkInterfaceArn": interface_arn,
                "protocolConfiguration": union(
                    "Rtp",
                    "rtp",
                    json!({ "destinationAddress": "198.51.100.10", "destinationPort": port }),
                ),
            }),
        ),
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    });
    let (status, code, created) = emulator.send("POST", "/v1/routerOutput", Some(body)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateRouterOutput answers 201 for {name:?}, got {status} {code:?}: {created}"
    );
}

/// The two association counts the batch read reports for `arn`.
async fn association_counts(emulator: &Emulator, arn: &str) -> (i64, i64) {
    let (status, _, read) = batch_get_network_interfaces(emulator, &[arn]).await;
    assert_eq!(status, 200, "batch get expected 200, got {read}");
    let interface = read["routerNetworkInterfaces"][0].clone();
    (
        interface["associatedInputCount"].as_i64().unwrap_or(-1),
        interface["associatedOutputCount"].as_i64().unwrap_or(-1),
    )
}

#[tokio::test]
async fn association_counts_come_from_the_inputs_and_outputs_that_name_the_interface() {
    let emulator = Emulator::new();
    let arns = seed_interfaces(&emulator, 2).await;
    let (first, second) = (arns[0].clone(), arns[1].clone());

    assert_eq!(
        association_counts(&emulator, &first).await,
        (0, 0),
        "an interface nothing names is associated with no input and no output"
    );

    // One row per kind of configuration that NAMES a network interface — STANDARD, FAILOVER
    // and MERGE all carry `NetworkInterfaceArn` — plus one that names none, so the counts
    // are asserted over the whole class rather than over one configuration.
    let standard = |interface_arn: &str, port: i64| {
        union(
            "Standard",
            "standard",
            json!({
                "networkInterfaceArn": interface_arn,
                "protocolConfiguration": union("Rtp", "rtp", json!({ "port": port })),
            }),
        )
    };
    let failover = |interface_arn: &str, port: i64| {
        union(
            "Failover",
            "failover",
            json!({
                "networkInterfaceArn": interface_arn,
                "sourcePriorityMode": "PRIMARY_SECONDARY",
                "primarySourceIndex": 0,
                "protocolConfigurations": [
                    union("Rtp", "rtp", json!({ "port": port })),
                    union("Rtp", "rtp", json!({ "port": port + 2 })),
                ],
            }),
        )
    };
    let merge = |interface_arn: &str, port: i64| {
        union(
            "Merge",
            "merge",
            json!({
                "networkInterfaceArn": interface_arn,
                "mergeRecoveryWindowMilliseconds": 500,
                "protocolConfigurations": [
                    union("Rtp", "rtp", json!({ "port": port })),
                    union("Rtp", "rtp", json!({ "port": port + 2 })),
                ],
            }),
        )
    };

    let standard_input = create_router_input(&emulator, "rni-assoc-standard", standard(&first, 5000)).await;
    assert_eq!(
        association_counts(&emulator, &first).await,
        (1, 0),
        "a STANDARD input naming the interface is counted once"
    );

    create_router_input(&emulator, "rni-assoc-failover", failover(&first, 5010)).await;
    assert_eq!(
        association_counts(&emulator, &first).await,
        (2, 0),
        "a FAILOVER input naming the interface is counted too"
    );

    create_router_input(&emulator, "rni-assoc-merge", merge(&second, 5020)).await;
    assert_eq!(
        association_counts(&emulator, &second).await,
        (1, 0),
        "a MERGE input is counted against the interface IT names"
    );
    assert_eq!(
        association_counts(&emulator, &first).await,
        (2, 0),
        "an input naming another interface does not change this one's count"
    );

    // A configuration that takes its content from another AWS resource names no interface,
    // so it is counted against none.
    create_router_input(
        &emulator,
        "rni-assoc-flow",
        union(
            "MediaConnectFlow",
            "mediaConnectFlow",
            json!({
                "flowArn": FLOW_ARN,
                "sourceTransitDecryption": {
                    "encryptionKeyType": "AUTOMATIC",
                    "encryptionKeyConfiguration": union("Automatic", "automatic", json!({})),
                },
            }),
        ),
    )
    .await;
    assert_eq!(
        association_counts(&emulator, &first).await,
        (2, 0),
        "an input that names no interface is counted against none"
    );

    create_router_output(&emulator, "rni-assoc-output", &first, 6000).await;
    create_router_output(&emulator, "rni-assoc-output-2", &first, 6002).await;
    create_router_output(&emulator, "rni-assoc-output-3", &second, 6004).await;
    assert_eq!(
        association_counts(&emulator, &first).await,
        (2, 2),
        "two outputs naming the interface are counted on the output side"
    );
    assert_eq!(
        association_counts(&emulator, &second).await,
        (1, 1),
        "the other interface counts only what names it"
    );

    // The counts are COUNTED, not stored: deleting an input lowers the count of the
    // interface it named, once the clock has taken the record away.
    let (status, _, deleted) = emulator
        .send("DELETE", &format!("/v1/routerInput/{standard_input}"), None)
        .await;
    assert_eq!(status, 200, "DeleteRouterInput expected 200, got {deleted}");
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    assert_eq!(
        association_counts(&emulator, &first).await,
        (1, 2),
        "a deleted input is no longer associated with the interface it named"
    );
}

/// One request this service must refuse, and the modelled failure it answers with.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();
    let twenty_one_arns: Vec<String> = (0..21)
        .map(|index| {
            format!(
                "arns=arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:{index:012x}"
            )
        })
        .collect();

    let cases = vec![
        RefusalCase {
            case: "a RegionName that is not a Region name",
            method: "POST",
            uri: "/v1/routerNetworkInterface".to_string(),
            body: Some(json!({
                "name": "rni-bad-region",
                "configuration": public_configuration(1),
                "regionName": "US-EAST-1",
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a Name outside the modelled pattern",
            method: "POST",
            uri: "/v1/routerNetworkInterface".to_string(),
            body: Some(json!({
                "name": "-rni-leading-dash",
                "configuration": public_configuration(1),
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a PUBLIC configuration past the modelled ten allow rules",
            method: "POST",
            uri: "/v1/routerNetworkInterface".to_string(),
            body: Some(json!({
                "name": "rni-eleven-rules",
                "configuration": public_configuration(11),
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a VPC configuration with no security group at all",
            method: "POST",
            uri: "/v1/routerNetworkInterface".to_string(),
            body: Some(json!({
                "name": "rni-no-security-group",
                "configuration": vpc_configuration(0),
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a VPC configuration past the modelled five security groups",
            method: "POST",
            uri: "/v1/routerNetworkInterface".to_string(),
            body: Some(json!({
                "name": "rni-six-security-groups",
                "configuration": vpc_configuration(6),
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a batch read naming an ARN that is not an ARN at all",
            method: "GET",
            uri: "/v1/routerNetworkInterfaces?arns=".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a batch read past the modelled 20-ARN maximum",
            method: "GET",
            uri: format!("/v1/routerNetworkInterfaces?{}", twenty_one_arns.join("&")),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a batch read naming an ARN that is not a network interface ARN",
            method: "GET",
            uri: format!("/v1/routerNetworkInterfaces?arns={FLOW_ARN}"),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
    ];

    // The valid shape of the same create is accepted, so each refusal below is about the one
    // thing the case changed.
    let (status, code, created) = create_network_interface(
        &emulator,
        json!({
            "name": "rni-refusals",
            "configuration": public_configuration(1),
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the control request must be accepted, got {status} {code:?}: {created}"
    );

    for RefusalCase {
        case,
        method,
        uri,
        body,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, answer) = emulator.send(method, &uri, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {answer}"
        );
        assert!(
            answer["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal carries this service's validation envelope, got {answer}"
        );
    }

    // Nothing the refusals asked for was stored: only the control request's interface is.
    assert_eq!(
        stored_count(&emulator),
        1,
        "a refused create stores nothing, got {} stored",
        stored_count(&emulator)
    );
}
