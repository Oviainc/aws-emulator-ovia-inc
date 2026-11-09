// SPDX-License-Identifier: CC0-1.0

//! `SetIpAddressType` — setting the IP address type of a Lightsail instance,
//! load balancer or CDN distribution, driven over the wire through the same
//! front door a client uses.
//!
//! The rule under test: the call records the requested `ipAddressType` on the ONE
//! stored resource its `resourceType`/`resourceName` pair addresses, so every
//! later read of that resource reports it; whatever the address type DECIDES
//! follows it — an instance holds an address of exactly the families "ipv4 for
//! IPv4 only, ipv6 for IPv6 only, and dualstack for IPv4 and IPv6" gives it, an
//! address of a family it keeps is left where it is and one of a family it gains
//! is drawn fresh, and a distribution is deploying again — and the change is
//! appended to the operation log under the modelled `SetIpAddressType` type. A
//! resource type this action does not set the address type of, a name this
//! account holds no such resource under, an instance whose bundle the change
//! moves without the caller's agreement, and a distribution addressed from
//! outside the one Region its API is served in are each refused, and a refused
//! request records nothing.
//!
//! WHICH address types each kind of resource may be given is the resource's own
//! documented domain, and a distribution's is narrower than the shared enum, so
//! the distribution cases here move it between the two values its own member
//! documents; `distribution_ip_address_type_domain.rs` is where that domain and
//! the refusal of a value outside it are pinned.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! which address type the resource starts with and which one it is moved to
//! (every ordered pair of the modelled `IpAddressType` values, so each row
//! gains, keeps or gives up each address family), which kind of resource is
//! addressed, and — for a load balancer — the Region the request is made in.
//!
//! The HTTP harness, the address shape checks and the settle poll are the ones
//! `instance_lifecycle.rs` already publishes to this test target.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{
    error_code, is_modelled_ipv4, is_modelled_ipv6, is_uuid_shaped, wait_for_state, Emulator,
};
use crate::instance_metadata_ipv6_region::IPV6_METADATA_REGION;
// The static IP attachment fixture `instance_start_addresses.rs` already
// publishes, which drives `AttachStaticIp` through the same front door: the
// attachment the refusal below is about is made by the operation that makes it.
use crate::instance_start_addresses::attach_static_ip;

/// Every value of the modelled `IpAddressType` enum
/// (`crate::ops::types::IP_ADDRESS_TYPE_VALUES`), which is the domain an
/// INSTANCE and a LOAD BALANCER each document for their own `ipAddressType`.
const IP_ADDRESS_TYPES: [&str; 3] = ["dualstack", "ipv4", "ipv6"];

/// The address types a DISTRIBUTION may carry, from
/// `smithy_query(query="doc",shape="LightsailDistribution",member="ipAddressType")`:
/// "The possible values are `ipv4` for IPv4 only, and `dualstack` for IPv4 and
/// IPv6" — two of the three members the shared enum carries. What a distribution
/// does with the third is `distribution_ip_address_type_domain.rs`.
const DISTRIBUTION_IP_ADDRESS_TYPES: [&str; 2] = ["ipv4", "dualstack"];

/// The `operationType` the log entry of this call carries, the modelled
/// `OperationType` enum's own `SetIpAddressType` member.
const OPERATION_TYPE: &str = "SetIpAddressType";

/// The one Region the distribution APIs are served in, from the modelled
/// `InvalidInputException` documentation ("Domain and distribution APIs are only
/// available in the N. Virginia (us-east-1) Amazon Web Services Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The `LightsailDistribution$status` values a distribution reports while a
/// configuration change is rolling out, and once it has.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const DEPLOYED: &str = "Deployed";

/// Whether `ip_address_type` gives the resource an IPv4 address, from the
/// modelled `IpAddressType` enum's own documentation.
fn has_ipv4(ip_address_type: &str) -> bool {
    matches!(ip_address_type, "ipv4" | "dualstack")
}

/// Whether `ip_address_type` gives the resource an IPv6 address. See
/// [`has_ipv4`].
fn has_ipv6(ip_address_type: &str) -> bool {
    matches!(ip_address_type, "ipv6" | "dualstack")
}

/// The Region an availability zone belongs to: the zone name without its trailing
/// zone letter, the form `AvailabilityZone$zoneName` documents ("The format is
/// `us-east-2a` (case-sensitive)").
fn region_of(availability_zone: &str) -> &str {
    &availability_zone[..availability_zone.len() - 1]
}

/// The `SetIpAddressType` body for `resource_type`/`resource_name`, accepting the
/// instance bundle update when `accept_bundle_update` is given.
fn set_body(
    resource_type: &str,
    resource_name: &str,
    ip_address_type: &str,
    accept_bundle_update: Option<bool>,
) -> Value {
    let mut body = json!({
        "resourceType": resource_type,
        "resourceName": resource_name,
        "ipAddressType": ip_address_type,
    });
    if let Some(accept_bundle_update) = accept_bundle_update {
        body["acceptBundleUpdate"] = json!(accept_bundle_update);
    }
    body
}

/// Assert the single operation `operations` carries is the log entry this change
/// appended for `resource_type`/`resource_name`.
fn assert_logged(case: &str, operations: &Value, resource_type: &str, resource_name: &str) {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: the call sets the address type of one resource, so it answers one \
         operation, got {entries:?}"
    );
    let operation = &entries[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected the modelled {OPERATION_TYPE} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(resource_type),
        "case {case}: expected the entry to be about a {resource_type}, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(resource_name),
        "case {case}: expected the entry to name {resource_name}, got {operation:?}"
    );
    assert!(
        operation["id"].as_str().is_some_and(is_uuid_shaped),
        "case {case}: expected a minted operation id, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Started"),
        "case {case}: the change is not finished the instant the call returns, got \
         {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(false),
        "case {case}: an unfinished operation is not terminal, got {operation:?}"
    );
    assert!(
        operation["createdAt"].as_f64().is_some(),
        "case {case}: expected the entry to carry its instant, got {operation:?}"
    );
}

// ---------------------------------------------------------------------------
// Instances
// ---------------------------------------------------------------------------

/// One instance to move between address types. The rows differ in the zone,
/// blueprint and bundle so no two are the same instance, and every ordered pair
/// of address types is driven over each of them in turn.
#[derive(Debug)]
struct InstanceRow {
    availability_zone: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
}

/// The instances the table below is driven over.
const INSTANCE_ROWS: &[InstanceRow] = &[
    InstanceRow {
        availability_zone: "us-east-1a",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
    },
    InstanceRow {
        availability_zone: "eu-west-3b",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
    },
    InstanceRow {
        availability_zone: "ap-northeast-1c",
        blueprint_id: "wordpress",
        bundle_id: "nano_3_0",
    },
    // The one Region whose instances have an IPv6 endpoint on their metadata
    // service, so the setting that follows the address type is observable on
    // both sides of the change (`instance_metadata_ipv6_region.rs`).
    InstanceRow {
        availability_zone: "eu-north-1a",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "small_3_0",
    },
];

/// Every public and private address this suite has seen reported, so an address
/// a change draws can be asserted to be one never handed out before rather than
/// merely different from the one before it.
#[derive(Debug, Default)]
struct SeenAddresses {
    ipv4: BTreeSet<String>,
    ipv6: BTreeSet<String>,
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
}

/// The IPv6 addresses `instance` reports.
fn ipv6_addresses(instance: &Value) -> Vec<String> {
    instance["ipv6Addresses"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Assert the addresses `after` reports are the ones `ip_address_type` gives the
/// instance: an address of a family it keeps is the one `before` reported, one of
/// a family it gains is well formed and was never handed out before, and a family
/// it does not give the instance is reported by no address at all.
fn assert_addresses_follow_type(
    case: &str,
    before: &Value,
    after: &Value,
    ip_address_type: &str,
    seen: &mut SeenAddresses,
) {
    let wants_ipv4 = has_ipv4(ip_address_type);
    let wants_ipv6 = has_ipv6(ip_address_type);

    for member in ["publicIpAddress", "privateIpAddress"] {
        match after[member].as_str() {
            Some(address) => {
                assert!(
                    wants_ipv4,
                    "case {case}: ipAddressType {ip_address_type} gives the instance no IPv4 \
                     address, got {member} {address:?}"
                );
                assert!(
                    is_modelled_ipv4(address),
                    "case {case}: expected a dotted-quad {member}, got {address:?}"
                );
                match before[member].as_str() {
                    // A family the instance keeps keeps its address: the
                    // instance is reachable on it.
                    Some(held) => assert_eq!(
                        address, held,
                        "case {case}: expected {member} to be left where it is, got \
                         {address:?}"
                    ),
                    // A family the instance gains is addressed freshly.
                    None => assert!(
                        seen.ipv4.insert(address.to_string()),
                        "case {case}: expected a {member} never handed out before, got \
                         {address:?} again (already seen {:?})",
                        seen.ipv4
                    ),
                }
            }
            None => assert!(
                !wants_ipv4,
                "case {case}: ipAddressType {ip_address_type} gives the instance an IPv4 \
                 address, so {member} must be reported, got {after:?}"
            ),
        }
    }

    let reported = ipv6_addresses(after);
    assert_eq!(
        reported.len(),
        usize::from(wants_ipv6),
        "case {case}: an instance with ipAddressType {ip_address_type} reports exactly the \
         IPv6 addresses it has, got {reported:?}"
    );
    let held = ipv6_addresses(before);
    for address in &reported {
        assert!(
            is_modelled_ipv6(address),
            "case {case}: expected a modelled IPv6 address, got {address:?}"
        );
        if held.is_empty() {
            assert!(
                seen.ipv6.insert(address.clone()),
                "case {case}: expected an IPv6 address never handed out before, got \
                 {address:?} again (already seen {:?})",
                seen.ipv6
            );
        } else {
            assert_eq!(
                &reported, &held,
                "case {case}: expected the IPv6 addresses the instance keeps to be left \
                 where they are, got {reported:?}"
            );
        }
    }
}

/// Assert everything the change does NOT touch reads back identical.
fn assert_everything_else_unchanged(case: &str, before: &Value, after: &Value) {
    for member in [
        "name",
        "arn",
        "supportCode",
        "createdAt",
        "location",
        "resourceType",
        "blueprintId",
        "blueprintName",
        "bundleId",
        "hardware",
        // The firewall is the caller's own configuration, kept verbatim: the
        // operations that open and close ports are what change it.
        "networking",
        "addOns",
        "tags",
        "sshKeyName",
        "username",
        "isStaticIp",
        "state",
    ] {
        assert_eq!(
            after[member], before[member],
            "case {case}: setting the IP address type must leave {member} alone, expected \
             {:?}, got {:?}",
            before[member], after[member]
        );
    }
}

/// RULE: setting an instance's IP address type records it on the one stored
/// instance, gives it an address of exactly the families that type gives it —
/// keeping the address of every family it keeps, drawing one for every family it
/// gains — follows it in the instance metadata service's IPv6 setting, and
/// appends one operation-log entry; everything else the record holds is left
/// alone.
#[tokio::test]
async fn setting_an_instances_ip_address_type_reprovisions_its_addresses() {
    // ONE emulator for every row, so its address allocator is shared and "an
    // address never handed out before" is a claim over the whole suite.
    let emulator = Emulator::new();
    let mut seen = SeenAddresses::default();

    for (position, row) in INSTANCE_ROWS.iter().enumerate() {
        for from in IP_ADDRESS_TYPES {
            for to in IP_ADDRESS_TYPES {
                let case = format!("instance {position} {from} -> {to}");
                let name = format!("typed-{position}-{from}-{to}");
                emulator
                    .ok(
                        "CreateInstances",
                        json!({
                            "instanceNames": [name],
                            "availabilityZone": row.availability_zone,
                            "blueprintId": row.blueprint_id,
                            "bundleId": row.bundle_id,
                            "ipAddressType": from,
                        }),
                    )
                    .await;
                wait_for_state(&emulator, &name, "running").await;
                let before = instance(&emulator, &name).await;
                assert_eq!(
                    before["ipAddressType"].as_str(),
                    Some(from),
                    "case {case}: the create stored the address type it was asked for, got \
                     {before:?}"
                );
                for address in [
                    before["publicIpAddress"].as_str(),
                    before["privateIpAddress"].as_str(),
                ]
                .into_iter()
                .flatten()
                {
                    seen.ipv4.insert(address.to_string());
                }
                seen.ipv6.extend(ipv6_addresses(&before));

                // A change that takes the instance's IPv4 addressing away, or
                // gives it back, moves the instance bundle, so it is accepted.
                let accept = (has_ipv4(from) != has_ipv4(to)).then_some(true);
                let answered = emulator
                    .ok(
                        "SetIpAddressType",
                        set_body("Instance", &name, to, accept),
                    )
                    .await;
                assert_logged(&case, &answered["operations"], "Instance", &name);

                let after = instance(&emulator, &name).await;
                assert_eq!(
                    after["ipAddressType"].as_str(),
                    Some(to),
                    "case {case}: the next read reports the address type that was set, got \
                     {after:?}"
                );
                assert_addresses_follow_type(&case, &before, &after, to, &mut seen);
                assert_everything_else_unchanged(&case, &before, &after);
                // "`httpProtocolIpv6` […] enables or disables the IPv6 endpoint
                // for the instance metadata service": an instance without an
                // IPv6 address has no IPv6 metadata endpoint to reach — and
                // neither has an instance outside the one Region that endpoint
                // is available in.
                let endpoint_available =
                    region_of(row.availability_zone) == IPV6_METADATA_REGION;
                assert_eq!(
                    after["metadataOptions"]["httpProtocolIpv6"].as_str(),
                    Some(if has_ipv6(to) && endpoint_available {
                        "enabled"
                    } else {
                        "disabled"
                    }),
                    "case {case}: the metadata service's IPv6 endpoint follows the \
                     instance's own address type where it has that endpoint at all, got \
                     {after:?}"
                );

                // The entry is in the log the operation reads answer from, too.
                let logged = emulator
                    .ok(
                        "GetOperationsForResource",
                        json!({"resourceName": name}),
                    )
                    .await;
                let types: Vec<&str> = logged["operations"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: expected operations, got {logged:?}"))
                    .iter()
                    .filter_map(|operation| operation["operationType"].as_str())
                    .collect();
                assert!(
                    types.contains(&OPERATION_TYPE),
                    "case {case}: expected the resource's log to carry the \
                     {OPERATION_TYPE} entry, got {types:?}"
                );
            }
        }
    }
}

/// RULE: the change is idempotent in its effect — setting the address type an
/// instance already has is accepted, needs no bundle update accepted (the bundle
/// does not move), leaves every address where it is, and is logged like any other
/// change.
#[tokio::test]
async fn setting_the_address_type_an_instance_already_has_changes_no_address() {
    let emulator = Emulator::new();
    for ip_address_type in IP_ADDRESS_TYPES {
        let case = format!("already {ip_address_type}");
        let name = format!("unchanged-{ip_address_type}");
        emulator
            .ok(
                "CreateInstances",
                json!({
                    "instanceNames": [name],
                    "availabilityZone": "us-west-2a",
                    "blueprintId": "ubuntu_22_04",
                    "bundleId": "small_3_0",
                    "ipAddressType": ip_address_type,
                }),
            )
            .await;
        wait_for_state(&emulator, &name, "running").await;
        let before = instance(&emulator, &name).await;

        // No `acceptBundleUpdate` at all: nothing crosses the IPv6-only
        // boundary, so none is needed.
        let answered = emulator
            .ok(
                "SetIpAddressType",
                set_body("Instance", &name, ip_address_type, None),
            )
            .await;
        assert_logged(&case, &answered["operations"], "Instance", &name);

        let after = instance(&emulator, &name).await;
        assert_eq!(
            after, before,
            "case {case}: setting the address type it already has changes nothing about the \
             record, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Load balancers
// ---------------------------------------------------------------------------

/// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports it.
async fn load_balancer(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await["loadBalancer"]
        .clone()
}

/// RULE: a load balancer is addressed by name in the Region the request is made
/// in, so the change lands on that Region's record alone and `GetLoadBalancer`
/// reports it there from then on.
#[tokio::test]
async fn setting_a_load_balancers_ip_address_type_is_reported_by_the_next_read() {
    let emulator = Emulator::new();
    // One load balancer of the same name in each of two Regions, so a change in
    // one Region is observable as a change to THAT record only.
    let regions = ["us-east-1", "ap-southeast-2"];
    let name = "typed-lb";
    for region in regions {
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateLoadBalancer",
                json!({
                    "loadBalancerName": name,
                    "instancePort": 80,
                    "ipAddressType": "ipv4",
                }),
            )
            .await;
    }

    for to in IP_ADDRESS_TYPES {
        for region in regions {
            let case = format!("load balancer in {region} -> {to}");
            let other = regions
                .iter()
                .find(|candidate| **candidate != region)
                .expect("the suite drives two regions");
            let before = load_balancer(&emulator, region, name).await;
            let elsewhere_before = load_balancer(&emulator, other, name).await;

            let answered = emulator
                .ok_at(
                    None,
                    Some(region),
                    "SetIpAddressType",
                    set_body("LoadBalancer", name, to, None),
                )
                .await;
            assert_logged(&case, &answered["operations"], "LoadBalancer", name);

            let after = load_balancer(&emulator, region, name).await;
            assert_eq!(
                after["ipAddressType"].as_str(),
                Some(to),
                "case {case}: the next read reports the address type that was set, got \
                 {after:?}"
            );
            for member in [
                "name",
                "arn",
                "supportCode",
                "createdAt",
                "location",
                "dnsName",
                "instancePort",
                "healthCheckPath",
                "protocol",
                "publicPorts",
                "configurationOptions",
                "tlsCertificateSummaries",
                "instanceHealthSummary",
            ] {
                assert_eq!(
                    after[member], before[member],
                    "case {case}: setting the address type must leave {member} alone, \
                     expected {:?}, got {:?}",
                    before[member], after[member]
                );
            }
            assert_eq!(
                load_balancer(&emulator, other, name).await,
                elsewhere_before,
                "case {case}: the load balancer of the same name in {other} is a different \
                 record and is untouched"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Distributions
// ---------------------------------------------------------------------------

/// The stored distribution `name`, as `GetDistributions` reports it.
async fn distribution(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            "GetDistributions",
            json!({"distributionName": name}),
        )
        .await["distributions"][0]
        .clone()
}

/// Poll `GetDistributions` until `name` reports `DEPLOYED`, answering the record.
///
/// The deployment is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_deployed(emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let record = distribution(emulator, name).await;
        if record["status"].as_str() == Some(DEPLOYED) {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {DEPLOYED} on {name}: expected it within 5s, got {:?}",
            record["status"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: setting a distribution's IP address type records it and rolls the change
/// out — the distribution is `InProgress` again until the clock owes it the
/// deployment — and the origin, bundle and published address it serves through
/// are left alone.
#[tokio::test]
async fn setting_a_distributions_ip_address_type_redeploys_it() {
    let emulator = Emulator::new();
    // An origin the distribution can pull content from: "An origin can be a
    // Lightsail instance, bucket, container service, or load balancer."
    let origin = "cdn-origin";
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [origin],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
        )
        .await;
    wait_for_state(&emulator, origin, "running").await;

    for (position, to) in DISTRIBUTION_IP_ADDRESS_TYPES.iter().enumerate() {
        let case = format!("distribution -> {to}");
        let name = format!("typed-cdn-{position}");
        emulator
            .ok_at(
                None,
                Some(DISTRIBUTION_REGION),
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "bundleId": "small_1_0",
                    "origin": {"name": origin, "regionName": DISTRIBUTION_REGION,
                               "protocolPolicy": "http-only"},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "ipAddressType": "ipv4",
                }),
            )
            .await;
        let before = wait_for_deployed(&emulator, &name).await;

        let answered = emulator
            .ok_at(
                None,
                Some(DISTRIBUTION_REGION),
                "SetIpAddressType",
                set_body("Distribution", &name, to, None),
            )
            .await;
        assert_logged(&case, &answered["operations"], "Distribution", &name);

        let rolling_out = distribution(&emulator, &name).await;
        assert_eq!(
            rolling_out["ipAddressType"].as_str(),
            Some(*to),
            "case {case}: the next read reports the address type that was set, got \
             {rolling_out:?}"
        );
        assert_eq!(
            rolling_out["status"].as_str(),
            Some(IN_PROGRESS),
            "case {case}: a configuration change is rolling out, so the distribution is \
             {IN_PROGRESS} again, got {rolling_out:?}"
        );
        for member in [
            "name",
            "arn",
            "supportCode",
            "createdAt",
            "location",
            "domainName",
            "bundleId",
            "origin",
            "originPublicDNS",
            "defaultCacheBehavior",
            "isEnabled",
            "certificateName",
            "alternativeDomainNames",
        ] {
            assert_eq!(
                rolling_out[member], before[member],
                "case {case}: setting the address type must leave {member} alone, expected \
                 {:?}, got {:?}",
                before[member], rolling_out[member]
            );
        }

        let deployed = wait_for_deployed(&emulator, &name).await;
        assert_eq!(
            deployed["ipAddressType"].as_str(),
            Some(*to),
            "case {case}: the address type does not move as the deployment settles, got \
             {deployed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: the resource values this action accepts are Distribution, Instance and
/// LoadBalancer; every other member of the modelled `ResourceType` enum names a
/// kind of resource with no IP address type to set, and is refused as invalid
/// input naming the member.
#[tokio::test]
async fn a_resource_type_without_an_ip_address_type_is_refused() {
    let emulator = Emulator::new();
    // A resource of the first kind really exists, so what is refused is the
    // resource TYPE rather than the name.
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "reserved"}))
        .await;

    for (resource_type, resource_name) in [
        ("StaticIp", "reserved"),
        ("Domain", "a-domain"),
        ("Bucket", "a-bucket"),
        ("Disk", "a-disk"),
        ("KeyPair", "a-key-pair"),
        ("RelationalDatabase", "a-database"),
        ("Alarm", "an-alarm"),
        ("ContainerService", "a-container-service"),
    ] {
        let case = format!("resourceType {resource_type}");
        let (status, body) = emulator
            .call(
                "SetIpAddressType",
                set_body(resource_type, resource_name, "dualstack", Some(true)),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected the modelled invalid-input refusal, got {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("resourceType") && message.contains(resource_type),
            "case {case}: expected the message to name the member and the value it \
             carried, got {message:?}"
        );
    }
}

/// RULE: a name this account holds no such resource under is the service's
/// `DoesNotExist` rejection, naming the resource type and the name the request
/// carried.
#[tokio::test]
async fn a_resource_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    for resource_type in ["Instance", "LoadBalancer", "Distribution"] {
        for resource_name in ["never-created", "typed-but-absent"] {
            let case = format!("{resource_type} {resource_name}");
            let (status, body) = emulator
                .call(
                    "SetIpAddressType",
                    set_body(resource_type, resource_name, "ipv4", Some(true)),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "NotFoundException"),
                "case {case}: expected the DoesNotExist envelope, got {status} {body:?}"
            );
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case {case}: expected the body's code to say what went wrong, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(resource_type) && message.contains(resource_name),
                "case {case}: expected the message to name the resource type and the \
                 requested name, got {message:?}"
            );
        }
    }
}

/// RULE: an instance whose IPv4 addressing the change takes away or gives back
/// moves the instance bundle, so the call is refused unless the caller accepted
/// that update — and a refused call changes nothing about the record. A change
/// that keeps the instance's IPv4 addressing moves no bundle and needs no
/// acceptance.
#[tokio::test]
async fn an_instance_bundle_change_needs_the_caller_to_accept_it() {
    let emulator = Emulator::new();
    for from in IP_ADDRESS_TYPES {
        for to in IP_ADDRESS_TYPES {
            for accept in [None, Some(false)] {
                let crosses = has_ipv4(from) != has_ipv4(to);
                let case = format!("{from} -> {to}, acceptBundleUpdate {accept:?}");
                let name = format!(
                    "bundle-{from}-{to}-{}",
                    if accept.is_some() { "false" } else { "absent" }
                );
                emulator
                    .ok(
                        "CreateInstances",
                        json!({
                            "instanceNames": [name],
                            "availabilityZone": "us-east-2a",
                            "blueprintId": "ubuntu_22_04",
                            "bundleId": "medium_3_0",
                            "ipAddressType": from,
                        }),
                    )
                    .await;
                wait_for_state(&emulator, &name, "running").await;
                let before = instance(&emulator, &name).await;

                let (status, body) = emulator
                    .call("SetIpAddressType", set_body("Instance", &name, to, accept))
                    .await;
                let after = instance(&emulator, &name).await;
                if crosses {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "InvalidInputException"),
                        "case {case}: expected the modelled invalid-input refusal, got \
                         {status} {body:?}"
                    );
                    let message = body["message"].as_str().unwrap_or_default();
                    assert!(
                        message.contains("acceptBundleUpdate"),
                        "case {case}: expected the message to name the member to send, got \
                         {message:?}"
                    );
                    assert_eq!(
                        after, before,
                        "case {case}: a refused call records nothing, got {after:?}"
                    );
                } else {
                    assert_eq!(
                        status, 200,
                        "case {case}: no bundle moves, so nothing has to be accepted, got \
                         {status} {body:?}"
                    );
                    assert_eq!(
                        after["ipAddressType"].as_str(),
                        Some(to),
                        "case {case}: the change was applied, got {after:?}"
                    );
                }
            }
        }
    }
}

/// RULE: a static IP is a public IPv4 address the account has reserved and
/// attached to the instance, so an address type that gives the instance no public
/// IPv4 address is refused while the attachment stands — and the instance keeps
/// the address type and the address it had.
#[tokio::test]
async fn an_instance_with_a_static_ip_attached_cannot_be_moved_to_ipv6_only() {
    let emulator = Emulator::new();
    let name = "pinned";
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": "us-east-1b",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
                "ipAddressType": "dualstack",
            }),
        )
        .await;
    wait_for_state(&emulator, name, "running").await;
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "reserved"}))
        .await;
    let reserved = attach_static_ip(&emulator, "reserved", name).await;
    let before = instance(&emulator, name).await;

    let (status, body) = emulator
        .call(
            "SetIpAddressType",
            set_body("Instance", name, "ipv6", Some(true)),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "OperationFailureException"),
        "case the attached static IP: expected the modelled operation-failure refusal, got \
         {status} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("reserved") && message.contains(name),
        "case the attached static IP: expected the message to name the static IP in the way \
         and the instance, got {message:?}"
    );
    let after = instance(&emulator, name).await;
    assert_eq!(
        after, before,
        "case the attached static IP: a refused call records nothing, got {after:?}"
    );
    assert_eq!(
        after["publicIpAddress"].as_str(),
        Some(reserved.as_str()),
        "case the attached static IP: the instance still serves on the reserved address, \
         got {after:?}"
    );

    // The same change on an address type that keeps the instance's public IPv4
    // address is served: the attachment is not in the way of it.
    emulator
        .ok(
            "SetIpAddressType",
            set_body("Instance", name, "ipv4", None),
        )
        .await;
    let narrowed = instance(&emulator, name).await;
    assert_eq!(
        (
            narrowed["ipAddressType"].as_str(),
            narrowed["publicIpAddress"].as_str(),
            ipv6_addresses(&narrowed).len()
        ),
        (Some("ipv4"), Some(reserved.as_str()), 0),
        "case the attached static IP, narrowed to ipv4: expected the reserved address kept \
         and the IPv6 address given up, got {narrowed:?}"
    );
}

/// RULE: the distribution APIs are served by the N. Virginia endpoint alone, so a
/// request for a distribution signed for any other Region is refused as invalid
/// input and changes nothing.
#[tokio::test]
async fn a_distribution_is_addressed_only_from_the_region_its_api_is_served_in() {
    let emulator = Emulator::new();
    let origin = "region-origin";
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [origin],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
        )
        .await;
    wait_for_state(&emulator, origin, "running").await;
    let name = "region-cdn";
    emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            "CreateDistribution",
            json!({
                "distributionName": name,
                "bundleId": "medium_1_0",
                "origin": {"name": origin, "protocolPolicy": "https-only"},
                "defaultCacheBehavior": {"behavior": "dont-cache"},
            }),
        )
        .await;
    let before = wait_for_deployed(&emulator, name).await;

    for region in ["us-west-2", "eu-central-1", "ap-south-1"] {
        let case = format!("distribution addressed from {region}");
        let (status, body) = emulator
            .call_in(
                Some(region),
                "SetIpAddressType",
                set_body("Distribution", name, "ipv4", None),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected the modelled invalid-input refusal, got {status} {body:?}"
        );
        assert_eq!(
            distribution(&emulator, name).await,
            before,
            "case {case}: a refused call records nothing"
        );
    }

    // The same call in the Region the API is served in is served.
    emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            "SetIpAddressType",
            set_body("Distribution", name, "ipv4", None),
        )
        .await;
    assert_eq!(
        distribution(&emulator, name).await["ipAddressType"].as_str(),
        Some("ipv4"),
        "case distribution addressed from {DISTRIBUTION_REGION}: the change is served there"
    );
}
