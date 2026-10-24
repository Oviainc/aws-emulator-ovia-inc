// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service PROJECTS its persisted records, driven over
//! the wire through the harness in [`super::accelerator_lifecycle`].
//!
//! Three rules, each stated once and asserted over a SET of inputs (a recorded request
//! is one row):
//!
//! 1. Every `IpSet` an accelerator is projected into reports one `IpAddressDetails`
//!    entry per address of that set, in the same order as `IpAddresses`, each naming
//!    that address and the distinct network zone it sits in.
//! 2. A listener reports the `PortRanges`, `Protocol` and `ClientAffinity` last
//!    persisted for it, through every operation that projects a listener.
//! 3. A minted ARN's final segment is a lowercase-hex id of the width that resource's
//!    ARNs carry: 8 for a listener, 12 for an endpoint group.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::accelerator_lifecycle::{arn_of, call, create_accelerator, emulator, error_code, ok};

/// `NetworkZone`'s modelled constraint: `@length(min: 0, max: 24)` on
/// `com.amazonaws.globalaccelerator#NetworkZone` (smithy_query query="shape",
/// shape="com.amazonaws.globalaccelerator#IpAddressDetail").
const NETWORK_ZONE_MAX_LEN: usize = 24;

/// Rule 1, asserted over one projected `Accelerator`.
fn assert_ip_address_details(case: &str, accelerator: &Value) {
    let ip_sets = accelerator["IpSets"].as_array().cloned().unwrap_or_default();
    assert!(
        !ip_sets.is_empty(),
        "case {case}: expected at least one IpSet, got {accelerator}"
    );
    for (set_index, set) in ip_sets.iter().enumerate() {
        let addresses = set["IpAddresses"].as_array().cloned().unwrap_or_default();
        let details = set["IpAddressDetails"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: IpSets[{set_index}] reports no IpAddressDetails, got {set}")
            });
        assert_eq!(
            details.len(),
            addresses.len(),
            "case {case}: IpSets[{set_index}] has {} addresses, so expected as many \
             IpAddressDetails entries, got {details:?}",
            addresses.len()
        );
        let mut zones: Vec<String> = Vec::with_capacity(details.len());
        for (index, detail) in details.iter().enumerate() {
            assert_eq!(
                detail["IpAddress"], addresses[index],
                "case {case}: IpSets[{set_index}].IpAddressDetails[{index}] should carry \
                 address {}, got {detail}",
                addresses[index]
            );
            let zone = detail["NetworkZone"].as_str().unwrap_or_else(|| {
                panic!(
                    "case {case}: IpSets[{set_index}].IpAddressDetails[{index}] carries no \
                     NetworkZone, got {detail}"
                )
            });
            assert!(
                !zone.is_empty() && zone.chars().count() <= NETWORK_ZONE_MAX_LEN,
                "case {case}: a NetworkZone is a non-empty string of at most \
                 {NETWORK_ZONE_MAX_LEN} characters, got {zone:?}"
            );
            assert!(
                !zones.iter().any(|seen| seen == zone),
                "case {case}: IpSets[{set_index}] places two addresses in zone {zone:?}; the \
                 zones of one address set are distinct, got {details:?}"
            );
            zones.push(zone.to_string());
        }
    }
}

#[tokio::test]
async fn every_ip_set_reports_one_address_detail_per_static_address() {
    let router = emulator();

    // The rows differ along what the rule generalises over: how many addresses the
    // caller supplied (none, one, both) and how many address sets the accelerator has
    // (an IPV4 accelerator has one; a DUAL_STACK one also has an IPv6 set).
    let rows: [(&str, Value, usize); 4] = [
        ("ipv4-allocated", json!({}), 1),
        (
            "ipv4-one-supplied",
            json!({ "IpAddresses": ["198.51.100.10"] }),
            1,
        ),
        (
            "ipv4-two-supplied",
            json!({ "IpAddresses": ["198.51.100.11", "198.51.100.12"] }),
            1,
        ),
        ("dual-stack", json!({ "IpAddressType": "DUAL_STACK" }), 2),
    ];

    for (case, extra, expected_sets) in rows {
        let mut body = json!({
            "Name": format!("detail-{case}"),
            "IdempotencyToken": format!("tok-{case}"),
        });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let created = ok(&router, "CreateAccelerator", body).await["Accelerator"].clone();
        let arn = arn_of(&created);
        assert_eq!(
            created["IpSets"].as_array().map(Vec::len),
            Some(expected_sets),
            "case {case}: expected {expected_sets} IP address set(s), got {created}"
        );
        assert_ip_address_details(&format!("{case}/create"), &created);

        // The same projection out of the reporting call...
        let described = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_ip_address_details(&format!("{case}/describe"), &described);
        assert_eq!(
            described["IpSets"], created["IpSets"],
            "case {case}: DescribeAccelerator should report the persisted address sets {}, \
             got {}",
            created["IpSets"], described["IpSets"]
        );

        // ... out of the list ...
        let listed = ok(&router, "ListAccelerators", json!({})).await["Accelerators"].clone();
        let from_list = listed
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|accelerator| arn_of(accelerator) == arn)
            .unwrap_or_else(|| panic!("case {case}: {arn} is missing from {listed}"));
        assert_ip_address_details(&format!("{case}/list"), &from_list);

        // ... and out of the mutating call's own response.
        let updated = ok(
            &router,
            "UpdateAccelerator",
            json!({ "AcceleratorArn": arn, "Name": format!("detail-{case}-renamed") }),
        )
        .await["Accelerator"]
            .clone();
        assert_ip_address_details(&format!("{case}/update"), &updated);
        assert_eq!(
            updated["IpSets"], created["IpSets"],
            "case {case}: an update that renames the accelerator keeps its address sets {}, \
             got {}",
            created["IpSets"], updated["IpSets"]
        );
    }
}

#[tokio::test]
async fn address_details_follow_the_addresses_an_update_changes() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "detail-update", "tok-detail-update").await;
    let arn = arn_of(&accelerator);

    // Each row updates the address layout and states what the resulting IPv4 set must
    // hold; the rule (one detail per address, in order, distinct zones) is re-asserted
    // after each one.
    let rows: [(&str, Value, Option<&str>); 3] = [
        (
            "replace-both-ipv4-addresses",
            json!({ "IpAddresses": ["198.51.100.20", "198.51.100.21"] }),
            Some("198.51.100.20"),
        ),
        ("add-an-ipv6-set", json!({ "IpAddressType": "DUAL_STACK" }), None),
        ("drop-the-ipv6-set", json!({ "IpAddressType": "IPV4" }), None),
    ];

    for (case, extra, expected_leading) in rows {
        let mut body = json!({ "AcceleratorArn": arn });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let updated = ok(&router, "UpdateAccelerator", body).await["Accelerator"].clone();
        assert_ip_address_details(&format!("{case}/update"), &updated);
        if let Some(expected) = expected_leading {
            assert_eq!(
                updated["IpSets"][0]["IpAddressDetails"][0]["IpAddress"], expected,
                "case {case}: expected the supplied address to lead the IPv4 set's details, \
                 got {}",
                updated["IpSets"][0]
            );
        }

        let described = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_eq!(
            described["IpSets"], updated["IpSets"],
            "case {case}: the describe should report what the update persisted {}, got {}",
            updated["IpSets"], described["IpSets"]
        );
    }
}

/// `CreateListener` with the members a row names, returning the created `Listener`.
async fn create_listener(
    router: &MultiServiceRouter,
    accelerator_arn: &str,
    token: &str,
    extra: &Value,
) -> Value {
    let mut body = json!({
        "AcceleratorArn": accelerator_arn,
        "IdempotencyToken": token,
        "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
        "Protocol": "TCP",
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    ok(router, "CreateListener", body).await["Listener"].clone()
}

#[tokio::test]
async fn a_listener_reports_the_configuration_last_persisted_for_it() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "listener-cfg", "tok-listener-cfg").await;
    let accelerator_arn = arn_of(&accelerator);

    // The rows differ along every member an update can change and along the SHAPE of
    // the port-range list (one range, two, the modelled maximum of ten — `PortRanges`
    // is @length(1..10)), plus a row whose update names nothing but the protocol, so an
    // omitted member must keep the persisted value rather than fall back to a default.
    let ten_ranges: Vec<Value> = (0..10)
        .map(|index| json!({ "FromPort": 1000 + index * 10, "ToPort": 1000 + index * 10 + 5 }))
        .collect();
    let rows: [(&str, Value, Value, Value); 5] = [
        (
            "source-ip-and-two-ranges",
            json!({}),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "PortRanges": [
                    { "FromPort": 443, "ToPort": 443 },
                    { "FromPort": 8080, "ToPort": 8081 },
                ],
            }),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "TCP",
                "PortRanges": [
                    { "FromPort": 443, "ToPort": 443 },
                    { "FromPort": 8080, "ToPort": 8081 },
                ],
            }),
        ),
        (
            "protocol-only-keeps-the-rest",
            json!({ "ClientAffinity": "SOURCE_IP" }),
            json!({ "Protocol": "UDP" }),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "UDP",
                "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            }),
        ),
        (
            "ten-ranges",
            json!({}),
            json!({ "PortRanges": ten_ranges.clone() }),
            json!({
                "ClientAffinity": "NONE",
                "Protocol": "TCP",
                "PortRanges": ten_ranges,
            }),
        ),
        (
            "affinity-back-to-none",
            json!({ "ClientAffinity": "SOURCE_IP" }),
            json!({ "ClientAffinity": "NONE" }),
            json!({
                "ClientAffinity": "NONE",
                "Protocol": "TCP",
                "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            }),
        ),
        (
            "every-member-at-once",
            json!({}),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "UDP",
                "PortRanges": [{ "FromPort": 65535, "ToPort": 65535 }],
            }),
            json!({
                "ClientAffinity": "SOURCE_IP",
                "Protocol": "UDP",
                "PortRanges": [{ "FromPort": 65535, "ToPort": 65535 }],
            }),
        ),
    ];

    // ARN -> the configuration that row expects, re-checked through ListListeners once
    // every row has run, so one listener's update cannot be reported for another.
    let mut expected_by_arn: Vec<(String, &str, Value)> = Vec::new();

    for (case, create_extra, update_extra, expected) in &rows {
        let created =
            create_listener(&router, &accelerator_arn, &format!("lsn-{case}"), create_extra).await;
        let listener_arn = created["ListenerArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: ListenerArn missing from {created}"))
            .to_string();

        let mut body = json!({ "ListenerArn": listener_arn });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), update_extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let updated = ok(&router, "UpdateListener", body).await["Listener"].clone();

        for (member, want) in expected.as_object().expect("an object of expectations") {
            assert_eq!(
                updated.get(member).unwrap_or(&Value::Null),
                want,
                "case {case}/update: expected {member} {want}, got {updated}"
            );
        }

        // The mutating call's own response, the describe and the list all project the
        // same stored record.
        let described = ok(
            &router,
            "DescribeListener",
            json!({ "ListenerArn": listener_arn }),
        )
        .await["Listener"]
            .clone();
        assert_eq!(
            described, updated,
            "case {case}/describe: expected the persisted listener {updated}, got {described}"
        );

        expected_by_arn.push((listener_arn, case, expected.clone()));
    }

    let listeners = ok(
        &router,
        "ListListeners",
        json!({ "AcceleratorArn": accelerator_arn }),
    )
    .await["Listeners"]
        .clone();
    let listed = listeners.as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        rows.len(),
        "case list: expected {} listeners, got {listeners}",
        rows.len()
    );
    for (listener_arn, case, expected) in &expected_by_arn {
        let found = listed
            .iter()
            .find(|listener| listener["ListenerArn"] == Value::String(listener_arn.clone()))
            .unwrap_or_else(|| panic!("case {case}/list: {listener_arn} is missing from {listeners}"));
        for (member, want) in expected.as_object().expect("an object of expectations") {
            assert_eq!(
                found.get(member).unwrap_or(&Value::Null),
                want,
                "case {case}/list: expected {member} {want}, got {found}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_listener_update_changes_nothing() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "listener-refuse", "tok-listener-refuse").await;
    let accelerator_arn = arn_of(&accelerator);
    let created = create_listener(
        &router,
        &accelerator_arn,
        "lsn-refuse",
        &json!({ "ClientAffinity": "SOURCE_IP" }),
    )
    .await;
    let listener_arn = created["ListenerArn"].as_str().unwrap_or_default().to_string();

    // A port range names the inclusive bounds of a range of ports, so a range that runs
    // backwards names none: the update path enforces the same rule as the create path.
    // An ARN the store does not hold is the modelled not-found refusal.
    let rows: [(&str, Value, &str); 4] = [
        (
            "reversed-range",
            json!({
                "ListenerArn": listener_arn,
                "PortRanges": [{ "FromPort": 443, "ToPort": 80 }],
            }),
            "InvalidPortRangeException",
        ),
        (
            "second-range-reversed",
            json!({
                "ListenerArn": listener_arn,
                "PortRanges": [
                    { "FromPort": 80, "ToPort": 81 },
                    { "FromPort": 9000, "ToPort": 8999 },
                ],
            }),
            "InvalidPortRangeException",
        ),
        (
            "range-missing-an-end",
            json!({
                "ListenerArn": listener_arn,
                "PortRanges": [{ "FromPort": 80 }],
            }),
            "InvalidPortRangeException",
        ),
        (
            "unknown-listener",
            json!({
                "ListenerArn": format!("{accelerator_arn}/listener/deadbeef"),
                "ClientAffinity": "NONE",
            }),
            "ListenerNotFoundException",
        ),
    ];

    for (case, body, expected_code) in rows {
        let (status, response) = call(&router, "UpdateListener", body).await;
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case}: expected {expected_code}, got {status} {response}"
        );
        let described = ok(
            &router,
            "DescribeListener",
            json!({ "ListenerArn": listener_arn }),
        )
        .await["Listener"]
            .clone();
        assert_eq!(
            described, created,
            "case {case}: a refused update must leave the listener at {created}, got {described}"
        );
    }
}

/// The ids in a `.../accelerator/<id>/listener/<id>/endpoint-group/<id>` ARN, outermost
/// first.
fn arn_segment_ids(arn: &str) -> Vec<&str> {
    arn.split('/').skip(1).step_by(2).collect()
}

#[tokio::test]
async fn a_minted_arn_segment_has_the_width_that_resources_arns_carry() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "arn-widths", "tok-arn-widths").await;
    let accelerator_arn = arn_of(&accelerator);

    // The rows differ along the parent the group hangs off and the Region it covers (a
    // listener holds one endpoint group per Region), so the widths are asserted over
    // several minted ids rather than one.
    let rows: [(&str, &str); 4] = [
        ("first-listener", "us-west-2"),
        ("first-listener", "eu-west-1"),
        ("second-listener", "us-west-2"),
        ("second-listener", "ap-south-1"),
    ];

    let mut listener_arns: Vec<(String, String)> = Vec::new();
    let mut seen_group_ids: Vec<String> = Vec::new();

    for (listener_case, region) in rows {
        let listener_arn = match listener_arns
            .iter()
            .find(|(case, _)| case == listener_case)
            .map(|(_, arn)| arn.clone())
        {
            Some(arn) => arn,
            None => {
                let created = create_listener(
                    &router,
                    &accelerator_arn,
                    &format!("lsn-{listener_case}"),
                    &json!({}),
                )
                .await;
                let arn = created["ListenerArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                listener_arns.push((listener_case.to_string(), arn.clone()));
                arn
            }
        };

        let case = format!("{listener_case}/{region}");
        let group = ok(
            &router,
            "CreateEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": region,
                "IdempotencyToken": format!("eg-{case}"),
            }),
        )
        .await["EndpointGroup"]
            .clone();
        let group_arn = group["EndpointGroupArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: EndpointGroupArn missing from {group}"))
            .to_string();
        assert!(
            group_arn.starts_with(&format!("{listener_arn}/endpoint-group/")),
            "case {case}: an endpoint group ARN hangs off its listener {listener_arn:?}, got \
             {group_arn:?}"
        );

        // The id of each segment, with the width that segment carries: an accelerator
        // id is a 36-character UUID, a listener id 8 lowercase hex characters and an
        // endpoint-group id 12.
        let ids = arn_segment_ids(&group_arn);
        assert_eq!(
            ids.len(),
            3,
            "case {case}: expected accelerator/listener/endpoint-group ids, got {group_arn:?}"
        );
        for (segment, id, width) in [
            ("accelerator", ids[0], 36),
            ("listener", ids[1], 8),
            ("endpoint-group", ids[2], 12),
        ] {
            assert_eq!(
                id.chars().count(),
                width,
                "case {case}: a {segment} id is {width} characters wide, got {id:?} in \
                 {group_arn:?}"
            );
            assert!(
                id.chars()
                    .all(|c| (c.is_ascii_hexdigit() || c == '-') && !c.is_ascii_uppercase()),
                "case {case}: a {segment} id is lowercase hex, got {id:?}"
            );
        }
        assert!(
            !seen_group_ids.iter().any(|seen| seen == ids[2]),
            "case {case}: endpoint-group id {:?} was minted twice, seen {seen_group_ids:?}",
            ids[2]
        );
        seen_group_ids.push(ids[2].to_string());

        // The minted ARN is the one the store answers for.
        let described = ok(
            &router,
            "DescribeEndpointGroup",
            json!({ "EndpointGroupArn": group_arn }),
        )
        .await["EndpointGroup"]
            .clone();
        assert_eq!(
            described, group,
            "case {case}: expected the created endpoint group {group}, got {described}"
        );
    }
}
