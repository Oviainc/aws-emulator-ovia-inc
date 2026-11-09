// SPDX-License-Identifier: CC0-1.0

//! The value domain of a resource's `ipAddressType`, driven over the wire
//! through the same front door a client uses.
//!
//! The rule under test: a resource may only be given an `ipAddressType` its OWN
//! modelled member documents as a possible value, whichever door writes it — the
//! create or `SetIpAddressType` — and a value outside that domain is refused
//! with the modelled `InvalidInputException` and records nothing. A
//! distribution's member documents two values
//! (`smithy_query(query="doc",shape="LightsailDistribution",member="ipAddressType")`:
//! "The possible values are `ipv4` for IPv4 only, and `dualstack` for IPv4 and
//! IPv6"), narrower than the shared `IpAddressType` enum the input members
//! target, while an instance's and a load balancer's each document all three
//! ("`ipv4` for IPv4 only, `ipv6` for IPv6 only, and `dualstack` for IPv4 and
//! IPv6"), so the same value is legitimate on those resources.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! which address type is asked for (every member of the enum, and no member at
//! all, which takes the documented default), which kind of resource is asked of
//! it, and which door asks — the create or the later set.
//!
//! The HTTP harness and the settle poll are the ones `instance_lifecycle.rs`
//! already publishes to this test target.

use serde_json::{json, Value};

// The distribution stand-up, the deployed status and the read-back this target
// already publishes: the origin creates and the `GetDistributions` poll are the
// ones every distribution suite here drives.
use crate::distribution_cache_reset::{create_origin, OriginKind, DEPLOYED};
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::{error_code, wait_for_state, Emulator};

/// The one Region the distribution APIs are served in, from the modelled
/// `InvalidInputException` documentation ("Domain and distribution APIs are only
/// available in the N. Virginia (us-east-1) Amazon Web Services Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// Every member of the modelled `IpAddressType` enum
/// (`smithy_query(query="shape",shape="IpAddressType")`), which is the domain
/// `Instance$ipAddressType` and `LoadBalancer$ipAddressType` each document.
const IP_ADDRESS_TYPES: [&str; 3] = ["dualstack", "ipv4", "ipv6"];

/// The address type a request that names none gets, from
/// `CreateDistributionRequest$ipAddressType` ("The default value is
/// `dualstack`") — the same default the instance and load balancer creates
/// document.
const DEFAULT_IP_ADDRESS_TYPE: &str = "dualstack";

/// One row of the distribution domain table: an address type, and whether a
/// distribution's own modelled member documents it as a possible value.
struct Row {
    /// What the request sends as `ipAddressType`; `None` sends none at all.
    asked: Option<&'static str>,
    /// Whether a DISTRIBUTION may carry it, from the two values its member
    /// documents.
    admitted: bool,
}

/// Every member of the enum, plus the request that names no member at all.
const ROWS: &[Row] = &[
    Row {
        asked: Some("ipv4"),
        admitted: true,
    },
    Row {
        asked: Some("dualstack"),
        admitted: true,
    },
    Row {
        asked: Some("ipv6"),
        admitted: false,
    },
    Row {
        asked: None,
        admitted: true,
    },
];

impl Row {
    /// The address type the distribution must report when the row is served.
    fn expected(&self) -> &'static str {
        self.asked.unwrap_or(DEFAULT_IP_ADDRESS_TYPE)
    }

    /// This row's case name.
    fn case(&self) -> String {
        match self.asked {
            Some(asked) => format!("ipAddressType {asked}"),
            None => "no ipAddressType at all".to_string(),
        }
    }
}

/// The `CreateDistribution` body for `name` on `origin`, sending
/// `ip_address_type` only when the row named one.
///
/// The create this target publishes (`distribution_cache_reset.rs`) carries no
/// `ipAddressType` and requires success, and the member under test is exactly
/// what each row varies, so the body is built here.
fn create_body(name: &str, origin: &str, ip_address_type: Option<&str>) -> Value {
    let mut body = json!({
        "distributionName": name,
        "bundleId": "small_1_0",
        "origin": {"name": origin, "regionName": DISTRIBUTION_REGION,
                   "protocolPolicy": "http-only"},
        "defaultCacheBehavior": {"behavior": "cache"},
    });
    if let Some(ip_address_type) = ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    body
}

/// Assert `body` is the modelled invalid-input refusal naming the member and the
/// value the request carried.
fn assert_refused(case: &str, status: u16, body: &Value, asked: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "InvalidInputException"),
        "case {case}: expected the modelled invalid-input refusal, got {status} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("ipAddressType") && message.contains(asked),
        "case {case}: expected the message to name the member and the value it carried, got \
         {message:?}"
    );
}

/// RULE: `CreateDistribution` takes only the address types a distribution's own
/// member documents; one outside that domain is refused as invalid input and
/// creates nothing.
#[tokio::test]
async fn creating_a_distribution_takes_only_the_address_types_it_can_have() {
    let emulator = Emulator::new();
    let origin = "domain-origin";
    create_origin(&emulator, OriginKind::Instance, origin).await;

    for (position, row) in ROWS.iter().enumerate() {
        let case = row.case();
        let name = format!("domain-cdn-{position}");
        let (status, body) = emulator
            .call_in(
                Some(DISTRIBUTION_REGION),
                "CreateDistribution",
                create_body(&name, origin, row.asked),
            )
            .await;
        if row.admitted {
            assert_eq!(
                status, 200,
                "case {case}: a distribution may carry it, got {status} {body:?}"
            );
            assert_eq!(
                body["distribution"]["ipAddressType"].as_str(),
                Some(row.expected()),
                "case {case}: the create answers the address type the distribution was \
                 given, got {body:?}"
            );
            let stored = distribution(&emulator, &name).await;
            assert_eq!(
                stored["ipAddressType"].as_str(),
                Some(row.expected()),
                "case {case}: the next read reports the stored address type, got {stored:?}"
            );
        } else {
            assert_refused(&case, status, &body, row.asked.unwrap_or_default());
            // A name this account holds no distribution under is the service's
            // `DoesNotExist` envelope, so a refused create is observable as the
            // absence of the record rather than as a stored one.
            let (read_status, read_body) = emulator
                .call("GetDistributions", json!({"distributionName": name}))
                .await;
            assert_eq!(
                (read_status, error_code(&read_body).as_str()),
                (400, "NotFoundException"),
                "case {case}: a refused create stores no distribution, got {read_status} \
                 {read_body:?}"
            );
        }
    }
}

/// RULE: `SetIpAddressType` moves a distribution between the address types its
/// own member documents, and refuses one outside that domain without recording
/// anything.
#[tokio::test]
async fn setting_a_distributions_address_type_stays_inside_its_domain() {
    let emulator = Emulator::new();
    let origin = "set-domain-origin";
    create_origin(&emulator, OriginKind::Instance, origin).await;

    for (position, row) in ROWS.iter().enumerate() {
        // A set names the address type, so the row that names none has nothing
        // to ask for here: the create case above already covered the default.
        let Some(asked) = row.asked else {
            continue;
        };
        // Each row starts from the value the other admitted row does not carry,
        // so a served row is a real change rather than a repeat.
        let from = if row.expected() == "ipv4" {
            "dualstack"
        } else {
            "ipv4"
        };
        let case = format!("{} from {from}", row.case());
        let name = format!("set-domain-cdn-{position}");
        emulator
            .ok_at(
                None,
                Some(DISTRIBUTION_REGION),
                "CreateDistribution",
                create_body(&name, origin, Some(from)),
            )
            .await;
        let before = wait_for_distribution_status(&emulator, &name, DEPLOYED).await;

        let (status, body) = emulator
            .call_in(
                Some(DISTRIBUTION_REGION),
                "SetIpAddressType",
                json!({
                    "resourceType": "Distribution",
                    "resourceName": name,
                    "ipAddressType": asked,
                }),
            )
            .await;
        if row.admitted {
            assert_eq!(
                status, 200,
                "case {case}: a distribution may carry it, got {status} {body:?}"
            );
            let after = wait_for_distribution_status(&emulator, &name, DEPLOYED).await;
            assert_eq!(
                after["ipAddressType"].as_str(),
                Some(asked),
                "case {case}: the next read reports the address type that was set, got \
                 {after:?}"
            );
        } else {
            assert_refused(&case, status, &body, asked);
            assert_eq!(
                wait_for_distribution_status(&emulator, &name, DEPLOYED).await,
                before,
                "case {case}: a refused set records nothing"
            );
        }
    }
}

/// RULE: the two-value domain belongs to the distribution alone — an instance
/// and a load balancer each document all three members of the enum, so every one
/// of them is served on those resources, by the create and by the set alike.
#[tokio::test]
async fn an_instance_and_a_load_balancer_take_every_member_of_the_enum() {
    let emulator = Emulator::new();

    for (position, asked) in IP_ADDRESS_TYPES.iter().enumerate() {
        let case = format!("instance ipAddressType {asked}");
        let instance = format!("typed-host-{position}");
        emulator
            .ok(
                "CreateInstances",
                json!({
                    "instanceNames": [instance],
                    "availabilityZone": "us-east-2a",
                    "blueprintId": "amazon_linux_2023",
                    "bundleId": "micro_3_0",
                    "ipAddressType": asked,
                }),
            )
            .await;
        wait_for_state(&emulator, &instance, "running").await;
        let created = emulator
            .ok("GetInstance", json!({"instanceName": instance}))
            .await["instance"]
            .clone();
        assert_eq!(
            created["ipAddressType"].as_str(),
            Some(*asked),
            "case {case}: an instance's member documents every member of the enum, got \
             {created:?}"
        );

        let balancer = format!("typed-lb-{position}");
        emulator
            .ok(
                "CreateLoadBalancer",
                json!({
                    "loadBalancerName": balancer,
                    "instancePort": 80,
                    "ipAddressType": asked,
                }),
            )
            .await;
        let stored = emulator
            .ok("GetLoadBalancer", json!({"loadBalancerName": balancer}))
            .await["loadBalancer"]
            .clone();
        assert_eq!(
            stored["ipAddressType"].as_str(),
            Some(*asked),
            "case load balancer ipAddressType {asked}: a load balancer's member documents \
             every member of the enum, got {stored:?}"
        );
    }

    // And the set door agrees with the create door on both resource kinds: the
    // IPv6-only value a distribution cannot carry is served here, with the
    // instance bundle update the move to IPv6-only needs accepted.
    let instance = "typed-host-1";
    emulator
        .ok(
            "SetIpAddressType",
            json!({
                "resourceType": "Instance",
                "resourceName": instance,
                "ipAddressType": "ipv6",
                "acceptBundleUpdate": true,
            }),
        )
        .await;
    let moved = emulator
        .ok("GetInstance", json!({"instanceName": instance}))
        .await["instance"]
        .clone();
    assert_eq!(
        moved["ipAddressType"].as_str(),
        Some("ipv6"),
        "case instance set to ipv6: an instance may be IPv6 only, got {moved:?}"
    );

    let balancer = "typed-lb-1";
    emulator
        .ok(
            "SetIpAddressType",
            json!({
                "resourceType": "LoadBalancer",
                "resourceName": balancer,
                "ipAddressType": "ipv6",
            }),
        )
        .await;
    let moved = emulator
        .ok("GetLoadBalancer", json!({"loadBalancerName": balancer}))
        .await["loadBalancer"]
        .clone();
    assert_eq!(
        moved["ipAddressType"].as_str(),
        Some("ipv6"),
        "case load balancer set to ipv6: a load balancer may be IPv6 only, got {moved:?}"
    );
}
