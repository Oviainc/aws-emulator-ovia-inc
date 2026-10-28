// SPDX-License-Identifier: CC0-1.0

//! A Resolver endpoint is placed only in subnets the account really holds.
//!
//! The rule under test: a request member that names the VPC subnet one of an
//! endpoint's IP addresses sits in is resolved against the account's own
//! Amazon EC2 subnet inventory, and a subnet id that resolves to nothing is
//! refused with the `InvalidParameterException` the operation models at HTTP
//! 400, whose message names the offending subnet id taken from the request,
//! with nothing written.
//!
//! The members that name such a subnet are `CreateResolverEndpoint`'s
//! `IpAddresses[].SubnetId` (required, `IpAddressesRequest` of length 2..20) and
//! `AssociateResolverEndpointIpAddress`'s `IpAddress.SubnetId`, and both are
//! exercised here. `DisassociateResolverEndpointIpAddress` also takes a
//! `SubnetId`, but there it SELECTS an address the endpoint already holds rather
//! than placing a new one, so it is not part of this rule
//! (`tests/all/resolver_endpoint_ip_address_association.rs` owns it).
//!
//! The rows differ along what the rule generalises over: where the unresolvable
//! id sits in the address list, how many addresses the list carries (both
//! modelled bounds and a size between them), the form and the length of the id
//! (`SubnetId` is length 1..32 with no pattern,
//! `smithy_query(query="shape",shape="SubnetId")`, so both published bounds are
//! covered, as are Amazon EC2's two published id forms and an id of no EC2 form
//! at all), and whether the subnet never existed or was taken out of the
//! account's inventory after the first endpoint was placed in it.
//!
//! Every case goes over the wire the service actually speaks, through the shared
//! [`super::harness`].

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::harness::{call, emulator, error_type, ok, string_member};

/// The two id forms Amazon EC2 publishes for a subnet — "subnet-" and 8 or 17
/// hexadecimal characters — used here for ids no subnet of the account carries.
const LONG_FORM_ID: &str = "subnet-0123456789abcdef0";
const SHORT_FORM_ID: &str = "subnet-0a1b2c3d";

/// The `ResolverEndpoint` of a create/associate/get response.
fn endpoint(body: &Value) -> &Value {
    body.get("ResolverEndpoint")
        .expect("the response carries a ResolverEndpoint")
}

/// A create body placing `count` addresses, the one at `unknown_at` in
/// `unknown_subnet` and the rest in subnets named after `creator`.
fn create_body(creator: &str, count: usize, unknown_at: Option<(usize, &str)>) -> Value {
    let ip_addresses: Vec<Value> = (0..count)
        .map(|index| {
            let subnet = match unknown_at {
                Some((position, unknown)) if position == index => unknown.to_string(),
                _ => format!("subnet-{creator}-{index}"),
            };
            json!({ "SubnetId": subnet, "Ip": format!("10.0.{index}.10") })
        })
        .collect();
    json!({
        "CreatorRequestId": creator,
        "Direction": "OUTBOUND",
        "SecurityGroupIds": ["sg-00000000000000001"],
        "IpAddresses": ip_addresses,
    })
}

/// The subnets `create_body` names for `creator` and `count` addresses, which a
/// served create holds in the account's own Amazon EC2 store.
fn own_subnets(creator: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("subnet-{creator}-{index}"))
        .collect()
}

/// How many endpoints the store holds, read over the wire.
async fn stored_endpoints(router: &super::harness::Emulator) -> usize {
    ok(router, "ListResolverEndpoints", json!({}))
        .await
        .get("ResolverEndpoints")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// One row of the create table: the address list the request carries, and the
/// subnet id in it that the account holds no subnet under.
#[derive(Debug)]
struct RefusedCase {
    case: &'static str,
    /// How many addresses the create places.
    count: usize,
    /// Which address of the list names the unresolvable subnet.
    unknown_at: usize,
    /// The subnet id that resolves to nothing.
    unknown: String,
}

/// A create naming a subnet the account does not hold is refused with
/// `InvalidParameterException` at HTTP 400, the offending id named in the
/// message, and stores nothing.
#[tokio::test]
async fn a_create_naming_a_subnet_the_account_does_not_hold_is_refused() {
    let cases = vec![
        RefusedCase {
            case: "the first of two addresses, an EC2 long-form id",
            count: 2,
            unknown_at: 0,
            unknown: LONG_FORM_ID.to_string(),
        },
        RefusedCase {
            case: "the last of two addresses, an EC2 short-form id",
            count: 2,
            unknown_at: 1,
            unknown: SHORT_FORM_ID.to_string(),
        },
        RefusedCase {
            case: "the middle of five addresses",
            count: 5,
            unknown_at: 2,
            unknown: "subnet-not-in-this-account".to_string(),
        },
        RefusedCase {
            case: "the last of twenty addresses, the modelled maximum",
            count: 20,
            unknown_at: 19,
            unknown: LONG_FORM_ID.to_string(),
        },
        RefusedCase {
            case: "the shortest SubnetId (length 1)",
            count: 2,
            unknown_at: 0,
            unknown: "s".to_string(),
        },
        RefusedCase {
            case: "the longest SubnetId (length 32)",
            count: 3,
            unknown_at: 1,
            unknown: "s".repeat(32),
        },
        RefusedCase {
            case: "an id that differs from one the account holds only by case",
            count: 2,
            unknown_at: 1,
            unknown: "SUBNET-REFUSED-1".to_string(),
        },
    ];

    let mut messages: Vec<String> = Vec::new();
    for RefusedCase {
        case,
        count,
        unknown_at,
        unknown,
    } in &cases
    {
        let router = emulator();
        // Every other subnet of the request IS the account's, so each case is
        // about the one id that resolves to nothing.
        router.hold_subnets(own_subnets("refused", *count));

        let body = create_body("refused", *count, Some((*unknown_at, unknown)));
        let (status, parsed) = call(&router, "CreateResolverEndpoint", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {parsed}"
        );
        let message = string_member(&parsed, "Message");
        assert_eq!(
            message,
            format!("The subnet ID '{unknown}' does not exist"),
            "case {case:?}: unexpected Message, got {parsed}"
        );
        messages.push(message);

        assert_eq!(
            stored_endpoints(&router).await,
            0,
            "case {case:?}: a refused create must store no endpoint"
        );
    }

    // The message is built from the request and not from fixed text: two
    // different unresolvable ids give two different messages.
    let distinct_ids: BTreeSet<&String> = cases.iter().map(|row| &row.unknown).collect();
    let distinct_messages: BTreeSet<&String> = messages.iter().collect();
    assert_eq!(
        distinct_messages.len(),
        distinct_ids.len(),
        "every distinct subnet id must give a distinct Message, got {messages:?}"
    );
}

/// A create every one of whose subnets the account holds is served, whatever the
/// size of the address list and however many addresses share one subnet, and the
/// endpoint reads back with the subnets the request named.
#[tokio::test]
async fn a_create_whose_subnets_the_account_holds_is_served() {
    // Each row: how many addresses, and how many distinct subnets they sit in —
    // "the subnet ID uniquely identifies a VPC", so several addresses of one
    // endpoint may share a subnet.
    for (case, count, distinct) in [
        ("the modelled minimum, one subnet each", 2usize, 2usize),
        ("two addresses in ONE subnet", 2, 1),
        ("five addresses across two subnets", 5, 2),
        ("the modelled maximum, one subnet each", 20, 20),
    ] {
        let router = emulator();
        let subnets = own_subnets("served", distinct);
        router.hold_subnets(&subnets);

        let ip_addresses: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "SubnetId": subnets[index % distinct],
                    "Ip": format!("10.1.{index}.10"),
                })
            })
            .collect();
        let created = ok(
            &router,
            "CreateResolverEndpoint",
            json!({
                "CreatorRequestId": format!("req-served-{count}-{distinct}"),
                "Direction": "OUTBOUND",
                "SecurityGroupIds": ["sg-00000000000000001"],
                "IpAddresses": ip_addresses.clone(),
            }),
        )
        .await;
        let id = string_member(endpoint(&created), "Id");

        // The round trip: the stored addresses sit in the subnets the request
        // named, in order.
        let listed = ok(
            &router,
            "ListResolverEndpointIpAddresses",
            json!({ "ResolverEndpointId": id }),
        )
        .await;
        let stored: Vec<String> = listed
            .get("IpAddresses")
            .and_then(Value::as_array)
            .map(|addresses| {
                addresses
                    .iter()
                    .map(|address| string_member(address, "SubnetId"))
                    .collect()
            })
            .unwrap_or_default();
        let requested: Vec<String> = ip_addresses
            .iter()
            .map(|address| string_member(address, "SubnetId"))
            .collect();
        assert_eq!(
            stored, requested,
            "case {case:?}: the stored addresses must sit in the subnets the request named, \
             got {listed}"
        );
    }
}

/// The refusal is keyed on the account's inventory and not on the shape of the
/// id: the very same create is served while the subnet is held and refused once
/// it is gone.
#[tokio::test]
async fn a_subnet_taken_out_of_the_account_stops_resolving() {
    let router = emulator();
    let subnets = own_subnets("released", 2);
    router.hold_subnets(&subnets);

    let first = ok(
        &router,
        "CreateResolverEndpoint",
        create_body("released", 2, None),
    )
    .await;
    assert!(
        !string_member(endpoint(&first), "Id").is_empty(),
        "a create into subnets the account holds must answer an endpoint, got {first}"
    );

    router.release_subnet(&subnets[1]);

    let mut second = create_body("released", 2, None);
    second["CreatorRequestId"] = json!("released-again");
    let (status, parsed) = call(&router, "CreateResolverEndpoint", second).await;
    assert_eq!(
        status, 400,
        "a create into a subnet the account no longer holds must answer HTTP 400, \
         got {status}: {parsed}"
    );
    assert_eq!(
        string_member(&parsed, "Message"),
        format!("The subnet ID '{}' does not exist", subnets[1]),
        "the refusal must name the subnet that stopped resolving, got {parsed}"
    );
    assert_eq!(
        stored_endpoints(&router).await,
        1,
        "the refused create must leave only the endpoint the first create made, got {parsed}"
    );
}

/// `AssociateResolverEndpointIpAddress` places one more address in a subnet, so
/// it resolves that subnet the same way: an id the account holds no subnet under
/// is refused and the endpoint's address set is left alone.
#[tokio::test]
async fn an_added_address_is_placed_only_in_a_subnet_the_account_holds() {
    let router = emulator();
    let subnets = own_subnets("assoc", 2);
    router.hold_subnets(&subnets);
    let created = ok(
        &router,
        "CreateResolverEndpoint",
        create_body("assoc", 2, None),
    )
    .await;
    let id = string_member(endpoint(&created), "Id");

    let addresses = |listed: &Value| -> Vec<Value> {
        listed
            .get("IpAddresses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let before = addresses(
        &ok(
            &router,
            "ListResolverEndpointIpAddresses",
            json!({ "ResolverEndpointId": id }),
        )
        .await,
    );

    // Each row: an addition naming a subnet that resolves to nothing, varying
    // the id's form and whether the address literal is supplied.
    for (case, unknown, ip) in [
        ("an EC2 long-form id", LONG_FORM_ID, Some("10.9.0.10")),
        ("an EC2 short-form id", SHORT_FORM_ID, None),
        (
            "an id of no EC2 form",
            "subnet-somewhere-else",
            Some("10.9.0.11"),
        ),
    ] {
        let mut address = json!({ "SubnetId": unknown });
        if let Some(ip) = ip {
            address["Ip"] = json!(ip);
        }
        let (status, parsed) = call(
            &router,
            "AssociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": id, "IpAddress": address }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {parsed}"
        );
        assert_eq!(
            string_member(&parsed, "Message"),
            format!("The subnet ID '{unknown}' does not exist"),
            "case {case:?}: unexpected Message, got {parsed}"
        );
    }

    let after = addresses(
        &ok(
            &router,
            "ListResolverEndpointIpAddresses",
            json!({ "ResolverEndpointId": id }),
        )
        .await,
    );
    assert_eq!(
        after, before,
        "a refused addition must leave every stored address alone"
    );

    // The endpoint is resolved before the subnet: a request that names an
    // endpoint the account does not hold is answered for THAT, even when its
    // subnet would not resolve either.
    let (status, parsed) = call(
        &router,
        "AssociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": "rslvr-out-0000000000000000",
            "IpAddress": { "SubnetId": LONG_FORM_ID, "Ip": "10.9.0.12" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an addition to an endpoint that was never created must answer HTTP 400, \
         got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains("ResourceNotFoundException"),
        "the missing endpoint is what the request is refused for, got {parsed}"
    );

    // And an addition into a subnet the account DOES hold is served, so the
    // refusals above are about the subnet and not about the operation.
    router.hold_subnets(["subnet-assoc-added"]);
    let associated = ok(
        &router,
        "AssociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": id,
            "IpAddress": { "SubnetId": "subnet-assoc-added", "Ip": "10.9.1.10" },
        }),
    )
    .await;
    assert_eq!(
        endpoint(&associated)
            .get("IpAddressCount")
            .and_then(Value::as_i64),
        Some(3),
        "the served addition must be counted, got {associated}"
    );
}
