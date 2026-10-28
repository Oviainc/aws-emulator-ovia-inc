// SPDX-License-Identifier: CC0-1.0

//! The Region restriction on the IPv6 endpoint of an instance's metadata
//! service, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness and the settle poll are the ones `instance_lifecycle.rs`
//! already publishes to this test target.
//!
//! The rule under test: a setting the model documents as available only for the
//! instances of ONE Region is served only for an instance in that Region —
//! `httpProtocolIpv6` is "available only for instances in the Europe (Stockholm)
//! Amazon Web Services Region (`eu-north-1`)" — so
//! `UpdateInstanceMetadataOptions` refuses the member for an instance anywhere
//! else with the modelled `InvalidInputException` (HTTP 400) carrying the
//! documented restriction, and the refused request records nothing at all, not
//! even the other settings it named; every read of an instance outside that
//! Region reports the setting `disabled`, whatever the instance's own
//! `ipAddressType`, because there is no such endpoint there to reach; and inside
//! that Region the member is applied and read back, or — with nobody having set
//! it — follows the instance's own address type.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! the Region the addressed instance lives in (the one the setting is available
//! in, and four others from the modelled `RegionName` enum), the instance's
//! `ipAddressType` (every value of the modelled `IpAddressType` enum) and the
//! value the member is given (every value of the modelled `HttpProtocolIpv6`
//! enum).

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::{
    HTTP_PROTOCOL_IPV6_VALUES, IP_ADDRESS_TYPE_VALUES, REGION_NAME_VALUES,
};

use crate::instance_lifecycle::{error_code, wait_for_state, Emulator};

/// The one Region whose instances have an IPv6 endpoint on their metadata
/// service, from that member's own documented availability note
/// (`smithy_query(query="doc",shape="UpdateInstanceMetadataOptionsRequest",member="httpProtocolIpv6")`):
/// "This parameter is available only for instances in the Europe (Stockholm)
/// Amazon Web Services Region (`eu-north-1`)."
///
/// `pub(crate)` so the sibling suites that read `metadataOptions` expect the
/// setting against the same one Region rather than carrying a second copy of it.
pub(crate) const IPV6_METADATA_REGION: &str = "eu-north-1";

/// The refusal a request that names the member for an instance outside
/// [`IPV6_METADATA_REGION`] is answered with: the restriction as the member's own
/// documentation states it, so its text is a requirement and not an example.
const REFUSAL: &str = "The httpProtocolIpv6 parameter is available only for instances in the \
                       Europe (Stockholm) Amazon Web Services Region (eu-north-1).";

/// The `operationType` an `UpdateInstanceMetadataOptions` log entry carries, the
/// modelled `OperationType` enum's own member.
const OPERATION_TYPE: &str = "UpdateInstanceMetadataOptions";

/// The `InstanceMetadataOptions$state` value a selection with nothing left to put
/// in place reports.
const APPLIED: &str = "applied";

/// The `HttpProtocolIpv6` value that says the endpoint serves, and the one that
/// says it does not, from the modelled enum
/// (`crate::ops::types::HTTP_PROTOCOL_IPV6_VALUES`).
const ENABLED: &str = "enabled";
/// See [`ENABLED`].
const DISABLED: &str = "disabled";

/// The Regions the tables below are driven over: the Region the setting is
/// available in, and four more from the modelled `RegionName` enum — the Region
/// Lightsail serves by default, one on each of two other continents and one in
/// the same part of Europe, so no row is the issue's own `us-east-1` case alone.
const REGIONS: [&str; 5] = [
    IPV6_METADATA_REGION,
    "us-east-1",
    "eu-central-1",
    "ap-southeast-2",
    "sa-east-1",
];

/// Whether an instance in `region` reaches the metadata service's IPv6 endpoint
/// at all.
fn region_has_the_endpoint(region: &str) -> bool {
    region == IPV6_METADATA_REGION
}

/// Whether `ip_address_type` gives the instance an IPv6 address, from the
/// modelled `IpAddressType` enum's own documentation: `ipv4` is IPv4 only,
/// `ipv6` is IPv6 only and `dualstack` is both.
fn has_ipv6(ip_address_type: &str) -> bool {
    matches!(ip_address_type, "ipv6" | "dualstack")
}

/// The availability zone a case's instance is created in: the Region's name
/// followed by a zone letter, the form `AvailabilityZone$zoneName` documents
/// ("The format is `us-east-2a` (case-sensitive)").
fn zone_of(region: &str) -> String {
    format!("{region}a")
}

/// Assert the tables are driven over Regions and values the model actually
/// carries, so a catalog that grows cannot leave them behind.
fn assert_tables_follow_the_model() {
    for region in REGIONS {
        assert!(
            REGION_NAME_VALUES.contains(&region),
            "case the Region catalog: expected the modelled RegionName enum to list {region}, \
             got {REGION_NAME_VALUES:?}"
        );
    }
    for value in [ENABLED, DISABLED] {
        assert!(
            HTTP_PROTOCOL_IPV6_VALUES.contains(&value),
            "case the HttpProtocolIpv6 catalog: expected the modelled enum to carry {value}, \
             got {HTTP_PROTOCOL_IPV6_VALUES:?}"
        );
    }
    assert_eq!(
        HTTP_PROTOCOL_IPV6_VALUES.len(),
        2,
        "case the HttpProtocolIpv6 catalog: the table drives every modelled value, got \
         {HTTP_PROTOCOL_IPV6_VALUES:?}"
    );
}

/// Create the instance `name` in `region` with `ip_address_type` and wait until
/// it is running.
async fn running_instance(emulator: &Emulator, name: &str, region: &str, ip_address_type: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": zone_of(region),
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
                "ipAddressType": ip_address_type,
            }),
        )
        .await;
    wait_for_state(emulator, name, "running").await;
}

/// The metadata options of the instance `name`, as `GetInstance` reports them.
async fn metadata_options(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetInstance", json!({"instanceName": name}))
        .await["instance"]["metadataOptions"]
        .clone()
}

/// The `UpdateInstanceMetadataOptions` body naming `instance_name`, the token
/// state and — when one is given — the IPv6 endpoint setting.
fn update_body(
    instance_name: &str,
    http_tokens: Option<&str>,
    http_protocol_ipv6: Option<&str>,
) -> Value {
    let mut body = json!({"instanceName": instance_name});
    if let Some(http_tokens) = http_tokens {
        body["httpTokens"] = json!(http_tokens);
    }
    if let Some(http_protocol_ipv6) = http_protocol_ipv6 {
        body["httpProtocolIpv6"] = json!(http_protocol_ipv6);
    }
    body
}

/// Every `UpdateInstanceMetadataOptions` entry the resource `name`'s operation
/// log carries.
async fn logged_updates(emulator: &Emulator, name: &str) -> usize {
    let body = emulator
        .ok("GetOperationsForResource", json!({"resourceName": name}))
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an operations array, got {body:?}"))
        .iter()
        .filter(|entry| entry["operationType"].as_str() == Some(OPERATION_TYPE))
        .count()
}

/// Poll `GetInstance` until the metadata options of `name` report `applied`,
/// answering them.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_applied(emulator: &Emulator, name: &str) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let options = metadata_options(emulator, name).await;
        if options["state"].as_str() == Some(APPLIED) {
            return options;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case waiting for {APPLIED} metadata options on {name}: expected them within 5s, \
             got {options:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// RULE: with nobody having set it, the IPv6 metadata endpoint is reported as
/// serving only where an instance has one at all — inside the Region the setting
/// is available in, following the instance's own address type — and `disabled`
/// everywhere else, whatever that address type is.
#[tokio::test]
async fn the_reported_setting_follows_the_region_and_then_the_address_type() {
    assert_tables_follow_the_model();
    let emulator = Emulator::new();

    for region in REGIONS {
        for ip_address_type in IP_ADDRESS_TYPE_VALUES.iter().copied() {
            let case = format!("a fresh {ip_address_type} instance in {region}");
            let name = format!("ipv6meta-{region}-{ip_address_type}");
            running_instance(&emulator, &name, region, ip_address_type).await;

            let expected = if region_has_the_endpoint(region) && has_ipv6(ip_address_type) {
                ENABLED
            } else {
                DISABLED
            };
            let options = metadata_options(&emulator, &name).await;
            assert_eq!(
                options["httpProtocolIpv6"].as_str(),
                Some(expected),
                "case {case}: expected httpProtocolIpv6 {expected:?}, got {options:?}"
            );
        }
    }
}

/// RULE: `UpdateInstanceMetadataOptions` serves `httpProtocolIpv6` only for an
/// instance in the Region the setting is available in — where either modelled
/// value is recorded and read back — and refuses it for an instance in any other
/// Region with the modelled `InvalidInputException` (HTTP 400) and the documented
/// restriction, recording nothing: not the member, not the other settings the
/// same request named, and not a log entry.
#[tokio::test]
async fn setting_the_ipv6_metadata_endpoint_is_served_only_in_its_own_region() {
    assert_tables_follow_the_model();
    let emulator = Emulator::new();

    for region in REGIONS {
        for value in HTTP_PROTOCOL_IPV6_VALUES.iter().copied() {
            let case = format!("httpProtocolIpv6 {value} for an instance in {region}");
            let name = format!("ipv6set-{region}-{value}");
            // Dual-stack, so the instance has IPv6 addressing of its own: what
            // decides is the Region, not the address type.
            running_instance(&emulator, &name, region, "dualstack").await;
            let before = metadata_options(&emulator, &name).await;

            // The request names the token state too, so a refusal can be shown
            // to record NONE of what it carried.
            let (status, body) = emulator
                .call(
                    "UpdateInstanceMetadataOptions",
                    update_body(&name, Some("required"), Some(value)),
                )
                .await;

            if region_has_the_endpoint(region) {
                assert_eq!(
                    status, 200,
                    "case {case}: the setting is available here, so the call must be served, \
                     got {status} {body:?}"
                );
                let options = wait_for_applied(&emulator, &name).await;
                assert_eq!(
                    options["httpProtocolIpv6"].as_str(),
                    Some(value),
                    "case {case}: expected the recorded setting to be read back, got {options:?}"
                );
                assert_eq!(
                    options["httpTokens"].as_str(),
                    Some("required"),
                    "case {case}: expected the token state the same request named, got \
                     {options:?}"
                );
                assert_eq!(
                    logged_updates(&emulator, &name).await,
                    1,
                    "case {case}: expected the served call to append one {OPERATION_TYPE} entry"
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case}: expected the request to be refused, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled input refusal, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(REFUSAL),
                "case {case}: expected the documented restriction, got {:?}",
                body["message"]
            );
            let after = metadata_options(&emulator, &name).await;
            assert_eq!(
                after, before,
                "case {case}: a refused request records nothing, expected {before:?}, got \
                 {after:?}"
            );
            assert_eq!(
                logged_updates(&emulator, &name).await,
                0,
                "case {case}: a refused request appends no {OPERATION_TYPE} entry"
            );
        }
    }
}

/// RULE: the restriction is on the ONE member it is documented for, so a request
/// that names no value for it is served wherever the instance lives, and the
/// setting the request said nothing about keeps the value that Region's instances
/// have.
#[tokio::test]
async fn an_update_that_names_no_ipv6_setting_is_served_in_every_region() {
    let emulator = Emulator::new();

    for region in REGIONS {
        let case = format!("an update naming only httpTokens in {region}");
        let name = format!("ipv6other-{region}");
        running_instance(&emulator, &name, region, "dualstack").await;

        let (status, body) = emulator
            .call(
                "UpdateInstanceMetadataOptions",
                update_body(&name, Some("required"), None),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: the request names no restricted member, so it must be served, got \
             {status} {body:?}"
        );
        let options = wait_for_applied(&emulator, &name).await;
        assert_eq!(
            options["httpTokens"].as_str(),
            Some("required"),
            "case {case}: expected the recorded token state, got {options:?}"
        );
        let expected = if region_has_the_endpoint(region) {
            ENABLED
        } else {
            DISABLED
        };
        assert_eq!(
            options["httpProtocolIpv6"].as_str(),
            Some(expected),
            "case {case}: expected httpProtocolIpv6 {expected:?} for a dual-stack instance \
             there, got {options:?}"
        );
    }
}
