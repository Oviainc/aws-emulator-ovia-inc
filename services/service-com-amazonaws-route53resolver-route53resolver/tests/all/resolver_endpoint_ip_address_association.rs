// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the endpoint IP-address write path:
//! `AssociateResolverEndpointIpAddress` and
//! `DisassociateResolverEndpointIpAddress`, read back through
//! `GetResolverEndpoint` and `ListResolverEndpointIpAddresses`.
//!
//! The rule under test is that one call adds, or removes, exactly one address
//! in the named endpoint's persisted address set, that the set stays inside the
//! bounds the model puts on the address list an endpoint is created with, and
//! that a refused call writes nothing. Every case goes over the wire the
//! service actually speaks, through the shared [`super::harness`].

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    MAX_ENDPOINT_IP_ADDRESSES, MIN_ENDPOINT_IP_ADDRESSES, PROVISIONING_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, expect_error, ok, parse_time, string_member};

/// A create request body with `count` IP addresses, one per subnet named
/// `subnet-<creator>-<index>`.
fn create_body(creator: &str, count: usize) -> Value {
    let ip_addresses: Vec<Value> = (0..count)
        .map(|index| {
            json!({ "SubnetId": format!("subnet-{creator}-{index}"), "Ip": format!("10.0.{index}.10") })
        })
        .collect();
    json!({
        "CreatorRequestId": creator,
        "Direction": "OUTBOUND",
        "SecurityGroupIds": ["sg-00000000000000001"],
        "IpAddresses": ip_addresses,
    })
}

/// The `ResolverEndpoint` of an associate/disassociate/create/get response.
fn endpoint(body: &Value) -> &Value {
    body.get("ResolverEndpoint")
        .expect("the response carries a ResolverEndpoint")
}

/// Create an endpoint with `count` addresses and return its `Id`.
async fn seed_endpoint(router: &MultiServiceRouter, creator: &str, count: usize) -> String {
    let created = ok(router, "CreateResolverEndpoint", create_body(creator, count)).await;
    string_member(endpoint(&created), "Id")
}

/// Every stored IP address of one endpoint, as `ListResolverEndpointIpAddresses`
/// reports them.
async fn stored_addresses(router: &MultiServiceRouter, endpoint_id: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": endpoint_id }),
    )
    .await;
    listed
        .get("IpAddresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `IpAddressCount` of one endpoint as `GetResolverEndpoint` reports it.
async fn reported_count(router: &MultiServiceRouter, endpoint_id: &str) -> i64 {
    let fetched = ok(
        router,
        "GetResolverEndpoint",
        json!({ "ResolverEndpointId": endpoint_id }),
    )
    .await;
    endpoint(&fetched)
        .get("IpAddressCount")
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

/// One row of the associate table: the `IpAddress` a request carries.
#[derive(Debug)]
struct AssociateCase {
    case: &'static str,
    ip_address: Value,
}

/// An associated address is persisted: the response's `IpAddressCount` counts
/// it, `GetResolverEndpoint` reports the same count and
/// `ListResolverEndpointIpAddresses` reports the new address with an
/// emulator-minted `IpId`, the subnet the request named and the address
/// literals it carried.
///
/// The table spans what the rule generalises over: the address family (IPv4,
/// IPv6, both), an address the request leaves to Resolver to choose, and a
/// subnet the endpoint already holds an address in.
#[tokio::test]
async fn an_associated_ip_address_is_read_back_from_state() {
    let router = emulator();
    let id = seed_endpoint(&router, "assoc", 2).await;

    let cases = [
        AssociateCase {
            case: "IPv4 address in a new subnet",
            ip_address: json!({ "SubnetId": "subnet-assoc-new-a", "Ip": "10.9.0.10" }),
        },
        AssociateCase {
            case: "IPv6 address only",
            ip_address: json!({ "SubnetId": "subnet-assoc-new-b", "Ipv6": "2001:db8::50" }),
        },
        AssociateCase {
            case: "both address families",
            ip_address: json!({
                "SubnetId": "subnet-assoc-new-c",
                "Ip": "10.9.0.11",
                "Ipv6": "2001:db8::51",
            }),
        },
        AssociateCase {
            case: "subnet only, so Resolver chooses the address",
            ip_address: json!({ "SubnetId": "subnet-assoc-new-d" }),
        },
        AssociateCase {
            case: "a second address in a subnet the endpoint already uses",
            ip_address: json!({ "SubnetId": "subnet-assoc-0", "Ip": "10.0.0.11" }),
        },
    ];

    let mut expected_count = 2;
    for AssociateCase { case, ip_address } in &cases {
        let before: Vec<String> = stored_addresses(&router, &id)
            .await
            .iter()
            .map(|address| string_member(address, "IpId"))
            .collect();

        let associated = ok(
            &router,
            "AssociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": id, "IpAddress": ip_address }),
        )
        .await;
        let reported = endpoint(&associated);
        expected_count += 1;

        assert_eq!(
            reported.get("IpAddressCount").and_then(Value::as_i64),
            Some(expected_count),
            "case {case:?}: the response must count the added address, got {associated}"
        );
        assert_eq!(
            string_member(reported, "Status"),
            "UPDATING",
            "case {case:?}: attaching an address moves the endpoint to UPDATING, got {associated}"
        );
        assert_eq!(
            string_member(reported, "Id"),
            id,
            "case {case:?}: the response must describe the endpoint that was asked for, got {associated}"
        );
        assert_eq!(
            reported_count(&router, &id).await,
            expected_count,
            "case {case:?}: GetResolverEndpoint must report the same count as the write"
        );

        let after = stored_addresses(&router, &id).await;
        assert_eq!(
            after.len(),
            usize::try_from(expected_count).expect("a small count"),
            "case {case:?}: the stored set must hold one more address, got {after:?}"
        );
        let added: Vec<&Value> = after
            .iter()
            .filter(|address| !before.contains(&string_member(address, "IpId")))
            .collect();
        assert_eq!(
            added.len(),
            1,
            "case {case:?}: exactly one address must be new, got {added:?}"
        );
        let added = added[0];

        let ip_id = string_member(added, "IpId");
        assert!(
            ip_id.starts_with("rni-"),
            "case {case:?}: the added address must carry a minted IpId, got {ip_id:?}"
        );
        assert_eq!(
            added.get("SubnetId"),
            ip_address.get("SubnetId"),
            "case {case:?}: the stored address must keep the subnet the request named, got {added}"
        );
        assert_eq!(
            added.get("Ip"),
            ip_address.get("Ip"),
            "case {case:?}: the stored address must keep the IPv4 address the request carried, got {added}"
        );
        assert_eq!(
            added.get("Ipv6"),
            ip_address.get("Ipv6"),
            "case {case:?}: the stored address must keep the IPv6 address the request carried, got {added}"
        );
        for member in ["CreationTime", "ModificationTime"] {
            parse_time(case, member, &string_member(added, member));
        }
    }
}

/// The attach the clock owes is observable: an address reports its transient
/// status straight after the associate and `ATTACHED` once the owed move falls
/// due, with the endpoint back in `OPERATIONAL`.
#[tokio::test]
async fn an_associated_address_attaches_once_the_clock_owes_it() {
    let router = emulator();
    let id = seed_endpoint(&router, "assoc-clock", 2).await;

    let associated = ok(
        &router,
        "AssociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": id,
            "IpAddress": { "SubnetId": "subnet-assoc-clock-new", "Ip": "10.9.1.10" },
        }),
    )
    .await;
    assert_eq!(
        string_member(endpoint(&associated), "Status"),
        "UPDATING",
        "the associate response must report the transient status, got {associated}"
    );

    let immediate = stored_addresses(&router, &id).await;
    let added = immediate
        .iter()
        .find(|address| string_member(address, "Ip") == "10.9.1.10")
        .expect("the added address is stored");
    assert_eq!(
        string_member(added, "Status"),
        "CREATING",
        "case \"immediately after the associate\": the added address must still be CREATING, got {added}"
    );

    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    let settled = stored_addresses(&router, &id).await;
    for address in &settled {
        assert_eq!(
            string_member(address, "Status"),
            "ATTACHED",
            "case \"after the owed transition\": every address must be ATTACHED, got {address}"
        );
    }
    let fetched = ok(
        &router,
        "GetResolverEndpoint",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    assert_eq!(
        string_member(endpoint(&fetched), "Status"),
        "OPERATIONAL",
        "once the owed transition falls due the endpoint must report OPERATIONAL, got {fetched}"
    );
}

/// One row of the disassociate table: how the request names the address to
/// remove.
#[derive(Debug)]
struct DisassociateCase {
    case: &'static str,
    /// `IpAddress` as the request carries it, `None` when the row names the
    /// address by the `IpId` the emulator minted (filled in at run time).
    ip_address: Option<Value>,
    /// The `Ip` of the address the row must remove.
    removes_ip: &'static str,
}

/// A disassociated address is gone from state: the response's
/// `IpAddressCount`, `GetResolverEndpoint` and
/// `ListResolverEndpointIpAddresses` all stop counting it, and every other
/// address is left alone.
///
/// The table spans the ways `IpAddressUpdate` names one address: by `IpId`, by
/// the IPv4 address, by the IPv6 address, by address and subnet together, and
/// by subnet alone.
#[tokio::test]
async fn a_disassociated_ip_address_is_removed_from_state() {
    let router = emulator();
    let id = seed_endpoint(&router, "disassoc", 2).await;

    // Headroom, so each removal below is well clear of the minimum.
    for (index, ipv6) in [
        (0, Some("2001:db8::60")),
        (1, None),
        (2, None),
        (3, None),
        (4, None),
    ] {
        let mut address = json!({
            "SubnetId": format!("subnet-disassoc-extra-{index}"),
            "Ip": format!("10.8.{index}.10"),
        });
        if let Some(ipv6) = ipv6 {
            address["Ipv6"] = json!(ipv6);
        }
        ok(
            &router,
            "AssociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": id, "IpAddress": address }),
        )
        .await;
    }

    let cases = [
        DisassociateCase {
            case: "by the minted IpId",
            ip_address: None,
            removes_ip: "10.8.1.10",
        },
        DisassociateCase {
            case: "by the IPv4 address",
            ip_address: Some(json!({ "Ip": "10.8.2.10" })),
            removes_ip: "10.8.2.10",
        },
        DisassociateCase {
            case: "by the IPv6 address",
            ip_address: Some(json!({ "Ipv6": "2001:db8::60" })),
            removes_ip: "10.8.0.10",
        },
        DisassociateCase {
            case: "by address and subnet together",
            ip_address: Some(json!({
                "SubnetId": "subnet-disassoc-extra-3",
                "Ip": "10.8.3.10",
            })),
            removes_ip: "10.8.3.10",
        },
        DisassociateCase {
            case: "by the subnet that holds it",
            ip_address: Some(json!({ "SubnetId": "subnet-disassoc-extra-4" })),
            removes_ip: "10.8.4.10",
        },
    ];

    for DisassociateCase {
        case,
        ip_address,
        removes_ip,
    } in &cases
    {
        let before = stored_addresses(&router, &id).await;
        let doomed = before
            .iter()
            .find(|address| string_member(address, "Ip") == *removes_ip)
            .unwrap_or_else(|| panic!("case {case:?}: the address to remove is stored"));
        let doomed_ip_id = string_member(doomed, "IpId");
        let request = ip_address
            .clone()
            .unwrap_or_else(|| json!({ "IpId": doomed_ip_id.clone() }));

        let removed = ok(
            &router,
            "DisassociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": id, "IpAddress": request }),
        )
        .await;
        let expected_count = i64::try_from(before.len() - 1).expect("a small count");
        assert_eq!(
            endpoint(&removed).get("IpAddressCount").and_then(Value::as_i64),
            Some(expected_count),
            "case {case:?}: the response must stop counting the removed address, got {removed}"
        );
        assert_eq!(
            string_member(endpoint(&removed), "Status"),
            "UPDATING",
            "case {case:?}: detaching an address moves the endpoint to UPDATING, got {removed}"
        );
        assert_eq!(
            reported_count(&router, &id).await,
            expected_count,
            "case {case:?}: GetResolverEndpoint must report the same count as the write"
        );

        let after = stored_addresses(&router, &id).await;
        assert!(
            !after
                .iter()
                .any(|address| string_member(address, "IpId") == doomed_ip_id),
            "case {case:?}: the removed address must be gone from state, got {after:?}"
        );
        let kept: Vec<String> = after
            .iter()
            .map(|address| string_member(address, "IpId"))
            .collect();
        let expected_kept: Vec<String> = before
            .iter()
            .map(|address| string_member(address, "IpId"))
            .filter(|ip_id| ip_id != &doomed_ip_id)
            .collect();
        assert_eq!(
            kept, expected_kept,
            "case {case:?}: every other address must be left alone and in order"
        );
    }
}

/// The address set stays inside the bounds the model puts on the address list
/// an endpoint is created with: a removal that would take it below the minimum,
/// and an addition past the maximum, are both refused and write nothing.
#[tokio::test]
async fn the_address_set_stays_within_its_modelled_bounds() {
    let router = emulator();

    // The minimum: an endpoint holding exactly the fewest addresses it could be
    // created with cannot give one up, however the request names it.
    let smallest = seed_endpoint(&router, "bounds-min", MIN_ENDPOINT_IP_ADDRESSES).await;
    let before = stored_addresses(&router, &smallest).await;
    let ip_id = string_member(&before[0], "IpId");
    for (case, ip_address) in [
        ("named by IpId", json!({ "IpId": ip_id })),
        ("named by its IPv4 address", json!({ "Ip": "10.0.0.10" })),
        (
            "named by its subnet",
            json!({ "SubnetId": "subnet-bounds-min-1" }),
        ),
    ] {
        let (status, parsed) = call(
            &router,
            "DisassociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": smallest, "IpAddress": ip_address }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a removal below the minimum must answer HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("InvalidRequestException"),
            "case {case:?}: a removal below the minimum must answer InvalidRequestException, got {parsed}"
        );
        // The fault carries its Resolver service error code and states the
        // minimum, which is what tells it apart from the other faults of this
        // operation.
        assert_eq!(
            string_member(&parsed, "Message"),
            format!(
                "[RSLVR-00505] Can only remove IP from resolver endpoints with more than \
                 {MIN_ENDPOINT_IP_ADDRESSES} IP addresses."
            ),
            "case {case:?}: unexpected message, got {parsed}"
        );
    }
    assert_eq!(
        stored_addresses(&router, &smallest).await,
        before,
        "a refused removal must leave every stored address alone"
    );

    // One above the minimum: the removal is served, and the next one is refused
    // — so the bound is on the resulting set and not on the operation.
    let id = seed_endpoint(&router, "bounds-edge", MIN_ENDPOINT_IP_ADDRESSES + 1).await;
    ok(
        &router,
        "DisassociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": id,
            "IpAddress": { "SubnetId": "subnet-bounds-edge-2" },
        }),
    )
    .await;
    assert_eq!(
        reported_count(&router, &id).await,
        i64::try_from(MIN_ENDPOINT_IP_ADDRESSES).expect("a small count"),
        "the served removal must leave the endpoint at the minimum"
    );
    expect_error(
        &router,
        "DisassociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": id,
            "IpAddress": { "SubnetId": "subnet-bounds-edge-1" },
        }),
        "one removal past the minimum",
        "InvalidRequestException",
    )
    .await;

    // The maximum: an endpoint already holding the most addresses it could be
    // created with takes no more.
    let largest = seed_endpoint(&router, "bounds-max", MAX_ENDPOINT_IP_ADDRESSES).await;
    let full = stored_addresses(&router, &largest).await;
    expect_error(
        &router,
        "AssociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": largest,
            "IpAddress": { "SubnetId": "subnet-bounds-max-new", "Ip": "10.7.0.10" },
        }),
        "one address past the maximum",
        "LimitExceededException",
    )
    .await;
    assert_eq!(
        stored_addresses(&router, &largest).await,
        full,
        "a refused addition must leave every stored address alone"
    );

    // Freeing a slot makes room again, so the cap is on the stored set.
    ok(
        &router,
        "DisassociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": largest,
            "IpAddress": { "SubnetId": "subnet-bounds-max-0" },
        }),
    )
    .await;
    ok(
        &router,
        "AssociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": largest,
            "IpAddress": { "SubnetId": "subnet-bounds-max-new", "Ip": "10.7.0.10" },
        }),
    )
    .await;
    assert_eq!(
        reported_count(&router, &largest).await,
        i64::try_from(MAX_ENDPOINT_IP_ADDRESSES).expect("a small count"),
        "the endpoint must be back at the maximum"
    );
}

/// Every refused request on this path answers the modelled error for its fault
/// and writes nothing.
#[tokio::test]
async fn a_refused_request_writes_nothing() {
    let router = emulator();
    let id = seed_endpoint(&router, "refusals", 3).await;
    let before = stored_addresses(&router, &id).await;
    let stored_ip_id = string_member(&before[0], "IpId");

    let cases = [
        (
            "an endpoint id that was never created",
            "AssociateResolverEndpointIpAddress",
            json!({
                "ResolverEndpointId": "rslvr-out-0000000000000000",
                "IpAddress": { "SubnetId": "subnet-refusals-new", "Ip": "10.6.0.10" },
            }),
            "ResourceNotFoundException",
        ),
        (
            "an endpoint id that was never created, on the removal",
            "DisassociateResolverEndpointIpAddress",
            json!({
                "ResolverEndpointId": "rslvr-out-0000000000000000",
                "IpAddress": { "Ip": "10.0.0.10" },
            }),
            "ResourceNotFoundException",
        ),
        (
            "an IPv4 address the endpoint already holds",
            "AssociateResolverEndpointIpAddress",
            json!({
                "ResolverEndpointId": id,
                "IpAddress": { "SubnetId": "subnet-refusals-new", "Ip": "10.0.1.10" },
            }),
            "ResourceExistsException",
        ),
        (
            "an IpId on the addition, which only a removal may name",
            "AssociateResolverEndpointIpAddress",
            json!({
                "ResolverEndpointId": id,
                "IpAddress": { "IpId": stored_ip_id.clone(), "SubnetId": "subnet-refusals-new" },
            }),
            "InvalidParameterException",
        ),
        (
            "an addition that names no subnet",
            "AssociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": id, "IpAddress": { "Ip": "10.6.0.11" } }),
            "InvalidParameterException",
        ),
        (
            "an IpId the endpoint does not hold",
            "DisassociateResolverEndpointIpAddress",
            json!({
                "ResolverEndpointId": id,
                "IpAddress": { "IpId": "rni-0000000000000000" },
            }),
            "ResourceNotFoundException",
        ),
        (
            "an address the endpoint does not hold",
            "DisassociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": id, "IpAddress": { "Ip": "10.6.9.9" } }),
            "ResourceNotFoundException",
        ),
        (
            "an address in a subnet the endpoint does not use",
            "DisassociateResolverEndpointIpAddress",
            json!({
                "ResolverEndpointId": id,
                "IpAddress": { "SubnetId": "subnet-refusals-elsewhere", "Ip": "10.0.0.10" },
            }),
            "ResourceNotFoundException",
        ),
        (
            "a removal that names no address at all",
            "DisassociateResolverEndpointIpAddress",
            json!({ "ResolverEndpointId": id, "IpAddress": {} }),
            "InvalidParameterException",
        ),
    ];

    for (case, operation, body, expected) in cases {
        expect_error(&router, operation, body, case, expected).await;
    }

    assert_eq!(
        stored_addresses(&router, &id).await,
        before,
        "no refused request may change the stored address set"
    );

    // And a well-formed call on the same endpoint is still served, so the
    // refusals above are about the requests and not about the endpoint.
    let (status, parsed) = call(
        &router,
        "AssociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": id,
            "IpAddress": { "SubnetId": "subnet-refusals-new", "Ip": "10.6.0.10" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a well-formed addition must still be served, got {status}: {parsed}"
    );
}
