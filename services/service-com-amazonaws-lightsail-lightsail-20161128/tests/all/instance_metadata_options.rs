// SPDX-License-Identifier: CC0-1.0

//! `UpdateInstanceMetadataOptions` — configuring the instance metadata service
//! of one Lightsail instance, driven over the wire through the same front door a
//! client uses.
//!
//! The rule under test: the call records the metadata settings the request NAMES
//! on the ONE stored instance its `instanceName` addresses — a setting the
//! request names no value for keeps the value the instance already had, except
//! `httpTokens`, whose absence is itself a value ("If the parameter is not
//! specified in the request, the default state is `optional`") — so every later
//! `GetInstance` and `GetInstances` reports the selection; the options are
//! `pending` while the service is putting a new selection in place on a running
//! instance and `applied` once it has; and the change is appended to the
//! operation log under the modelled `UpdateInstanceMetadataOptions` type, as an
//! entry that settles with the options. A hop limit outside the range the
//! metadata service accepts, and a name this account holds no instance under,
//! are refused and record nothing.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! which of the four settings the request names, and which value each is given —
//! including a hop limit at both published bounds and one past each of them.
//!
//! The HTTP harness, the id shape check and the settle poll are the ones
//! `instance_lifecycle.rs` already publishes to this test target.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The `operationType` the log entry of this call carries, the modelled
/// `OperationType` enum's own `UpdateInstanceMetadataOptions` member.
const OPERATION_TYPE: &str = "UpdateInstanceMetadataOptions";

/// The `InstanceMetadataState` values an instance's metadata options report:
/// "`pending` - The metadata options are being updated […] `applied` - The
/// metadata options have been successfully applied to the instance".
const PENDING: &str = "pending";
/// See [`PENDING`].
const APPLIED: &str = "applied";

/// The smallest and largest `httpPutResponseHopLimit` the instance metadata
/// service accepts ("Possible values: Integers from 1 to 64", the hop limit of
/// the instance metadata service a Lightsail instance runs).
const MIN_HOP_LIMIT: i64 = 1;
/// See [`MIN_HOP_LIMIT`].
const MAX_HOP_LIMIT: i64 = 64;

/// The selection every row's instance is seeded with before the row's own
/// request, so a setting the row leaves out is observably MAINTAINED rather than
/// merely equal to what a new instance is created with. Every value differs from
/// the creation default (`optional`, `enabled`, 1, and — on a dual-stack
/// instance — `enabled`).
const SEED_TOKENS: &str = "required";
const SEED_ENDPOINT: &str = "disabled";
const SEED_HOP_LIMIT: i64 = 9;
const SEED_PROTOCOL_IPV6: &str = "disabled";

/// One `UpdateInstanceMetadataOptions` request against a seeded instance, and
/// what the instance must then report.
struct Row {
    case: &'static str,
    /// What the request carries; `None` leaves the member out of the body
    /// entirely.
    http_tokens: Option<&'static str>,
    http_endpoint: Option<&'static str>,
    http_put_response_hop_limit: Option<i64>,
    http_protocol_ipv6: Option<&'static str>,
    /// What `Instance$metadataOptions` must report afterwards.
    expect_http_tokens: &'static str,
    expect_http_endpoint: &'static str,
    expect_http_put_response_hop_limit: i64,
    expect_http_protocol_ipv6: &'static str,
}

/// The table. The seeded selection is `required` / `disabled` / 9 / `disabled`,
/// so every expectation below is either the row's own value or the seed's.
const ROWS: &[Row] = &[
    Row {
        case: "every setting named",
        http_tokens: Some("optional"),
        http_endpoint: Some("enabled"),
        http_put_response_hop_limit: Some(32),
        http_protocol_ipv6: Some("enabled"),
        expect_http_tokens: "optional",
        expect_http_endpoint: "enabled",
        expect_http_put_response_hop_limit: 32,
        expect_http_protocol_ipv6: "enabled",
    },
    Row {
        case: "only httpEndpoint named",
        http_tokens: None,
        http_endpoint: Some("enabled"),
        http_put_response_hop_limit: None,
        http_protocol_ipv6: None,
        // An unnamed `httpTokens` is `optional`, not the seeded `required`.
        expect_http_tokens: "optional",
        expect_http_endpoint: "enabled",
        expect_http_put_response_hop_limit: SEED_HOP_LIMIT,
        expect_http_protocol_ipv6: SEED_PROTOCOL_IPV6,
    },
    Row {
        case: "only httpTokens named",
        http_tokens: Some("required"),
        http_endpoint: None,
        http_put_response_hop_limit: None,
        http_protocol_ipv6: None,
        expect_http_tokens: "required",
        expect_http_endpoint: SEED_ENDPOINT,
        expect_http_put_response_hop_limit: SEED_HOP_LIMIT,
        expect_http_protocol_ipv6: SEED_PROTOCOL_IPV6,
    },
    Row {
        case: "only httpProtocolIpv6 named",
        http_tokens: None,
        http_endpoint: None,
        http_put_response_hop_limit: None,
        http_protocol_ipv6: Some("enabled"),
        expect_http_tokens: "optional",
        expect_http_endpoint: SEED_ENDPOINT,
        expect_http_put_response_hop_limit: SEED_HOP_LIMIT,
        expect_http_protocol_ipv6: "enabled",
    },
    Row {
        case: "the hop limit at the published minimum",
        http_tokens: None,
        http_endpoint: None,
        http_put_response_hop_limit: Some(MIN_HOP_LIMIT),
        http_protocol_ipv6: None,
        expect_http_tokens: "optional",
        expect_http_endpoint: SEED_ENDPOINT,
        expect_http_put_response_hop_limit: MIN_HOP_LIMIT,
        expect_http_protocol_ipv6: SEED_PROTOCOL_IPV6,
    },
    Row {
        case: "the hop limit at the published maximum",
        http_tokens: None,
        http_endpoint: None,
        http_put_response_hop_limit: Some(MAX_HOP_LIMIT),
        http_protocol_ipv6: None,
        expect_http_tokens: "optional",
        expect_http_endpoint: SEED_ENDPOINT,
        expect_http_put_response_hop_limit: MAX_HOP_LIMIT,
        expect_http_protocol_ipv6: SEED_PROTOCOL_IPV6,
    },
    Row {
        case: "no setting named at all",
        http_tokens: None,
        http_endpoint: None,
        http_put_response_hop_limit: None,
        http_protocol_ipv6: None,
        expect_http_tokens: "optional",
        expect_http_endpoint: SEED_ENDPOINT,
        expect_http_put_response_hop_limit: SEED_HOP_LIMIT,
        expect_http_protocol_ipv6: SEED_PROTOCOL_IPV6,
    },
];

/// The `UpdateInstanceMetadataOptions` body naming `instance_name` and whichever
/// of the four settings are given.
fn update_body(
    instance_name: &str,
    http_tokens: Option<&str>,
    http_endpoint: Option<&str>,
    http_put_response_hop_limit: Option<i64>,
    http_protocol_ipv6: Option<&str>,
) -> Value {
    let mut body = json!({"instanceName": instance_name});
    if let Some(http_tokens) = http_tokens {
        body["httpTokens"] = json!(http_tokens);
    }
    if let Some(http_endpoint) = http_endpoint {
        body["httpEndpoint"] = json!(http_endpoint);
    }
    if let Some(hop_limit) = http_put_response_hop_limit {
        body["httpPutResponseHopLimit"] = json!(hop_limit);
    }
    if let Some(http_protocol_ipv6) = http_protocol_ipv6 {
        body["httpProtocolIpv6"] = json!(http_protocol_ipv6);
    }
    body
}

/// Create the dual-stack instance `name` and wait until it is running — the
/// state the documented `pending` → `applied` span belongs to.
async fn running_instance(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
                "ipAddressType": "dualstack",
            }),
        )
        .await;
    wait_for_state(emulator, name, "running").await;
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetInstance", json!({"instanceName": name}))
        .await["instance"]
        .clone()
}

/// The metadata options of the instance `name`.
async fn metadata_options(emulator: &Emulator, name: &str) -> Value {
    instance(emulator, name).await["metadataOptions"].clone()
}

/// Put the seeded selection on `name` and wait until it is applied, so the row's
/// own request starts from a known, non-default configuration.
async fn seed(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "UpdateInstanceMetadataOptions",
            update_body(
                name,
                Some(SEED_TOKENS),
                Some(SEED_ENDPOINT),
                Some(SEED_HOP_LIMIT),
                Some(SEED_PROTOCOL_IPV6),
            ),
        )
        .await;
    wait_for_applied(emulator, name).await;
}

/// Poll `GetInstance` until the metadata options of `name` report `applied`,
/// answering them.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_applied(emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let options = metadata_options(emulator, name).await;
        if options["state"].as_str() == Some(APPLIED) {
            return options;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {APPLIED} metadata options on {name}: expected them within 5s, \
             got {options:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Assert `options` reports exactly the four settings `row` expects.
fn assert_settings(case: &str, options: &Value, row: &Row) {
    assert_eq!(
        options["httpTokens"].as_str(),
        Some(row.expect_http_tokens),
        "case {case}: expected httpTokens {:?}, got {options:?}",
        row.expect_http_tokens
    );
    assert_eq!(
        options["httpEndpoint"].as_str(),
        Some(row.expect_http_endpoint),
        "case {case}: expected httpEndpoint {:?}, got {options:?}",
        row.expect_http_endpoint
    );
    assert_eq!(
        options["httpPutResponseHopLimit"].as_i64(),
        Some(row.expect_http_put_response_hop_limit),
        "case {case}: expected httpPutResponseHopLimit {}, got {options:?}",
        row.expect_http_put_response_hop_limit
    );
    assert_eq!(
        options["httpProtocolIpv6"].as_str(),
        Some(row.expect_http_protocol_ipv6),
        "case {case}: expected httpProtocolIpv6 {:?}, got {options:?}",
        row.expect_http_protocol_ipv6
    );
}

/// Assert the ONE operation the call answered is the log entry it appended for
/// `resource_name`, answering its id.
fn assert_logged(case: &str, answered: &Value, resource_name: &str, terminal: bool) -> String {
    let operation = &answered["operation"];
    assert!(
        operation.is_object(),
        "case {case}: the call acts on one instance, so it answers one operation, got \
         {answered:?}"
    );
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected the modelled {OPERATION_TYPE} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("Instance"),
        "case {case}: expected the entry to be about an Instance, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(resource_name),
        "case {case}: expected the entry to name {resource_name}, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(terminal),
        "case {case}: expected isTerminal {terminal}, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some(if terminal { "Succeeded" } else { "Started" }),
        "case {case}: expected the status that goes with isTerminal {terminal}, got \
         {operation:?}"
    );
    let id = operation["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an operation id, got {operation:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a minted operation id, got {id:?}"
    );
    id.to_string()
}

#[tokio::test]
async fn the_named_settings_are_recorded_and_the_rest_are_maintained() {
    // ONE emulator for every row: each row addresses its own instance, so the
    // rule is also asserted to touch only the instance its request names.
    let emulator = Emulator::new();

    for (position, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("metadata-{position}");
        running_instance(&emulator, &name).await;
        seed(&emulator, &name).await;

        let answered = emulator
            .ok(
                "UpdateInstanceMetadataOptions",
                update_body(
                    &name,
                    row.http_tokens,
                    row.http_endpoint,
                    row.http_put_response_hop_limit,
                    row.http_protocol_ipv6,
                ),
            )
            .await;
        // The instance is running, so the service is still putting the selection
        // in place: the entry is non-terminal until it has.
        let operation_id = assert_logged(case, &answered, &name, false);

        // The record carries the new selection from the moment the call
        // returned, and reports it as not yet in place.
        let putting_in_place = metadata_options(&emulator, &name).await;
        assert_eq!(
            putting_in_place["state"].as_str(),
            Some(PENDING),
            "case {case}: a running instance is not ready to process metadata traffic with \
             the new selection yet, got {putting_in_place:?}"
        );
        assert_settings(case, &putting_in_place, row);

        // And once the clock has applied them, the same settings are reported
        // with nothing outstanding.
        let applied = wait_for_applied(&emulator, &name).await;
        assert_settings(case, &applied, row);

        // The listing reads the one record the same way the single read does.
        let listed = emulator.ok("GetInstances", json!({})).await;
        let entry = listed["instances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an instances array, got {listed:?}"))
            .iter()
            .find(|entry| entry["name"].as_str() == Some(name.as_str()))
            .unwrap_or_else(|| panic!("case {case}: expected {name} to be listed, got {listed:?}"))
            .clone();
        assert_settings(case, &entry["metadataOptions"], row);

        // The log entry the call answered is in the log, and it has settled with
        // the options it recorded.
        let read_back = emulator
            .ok("GetOperation", json!({"operationId": operation_id}))
            .await;
        assert_eq!(
            read_back["operation"]["status"].as_str(),
            Some("Succeeded"),
            "case {case}: the entry settles once the options are applied, got {read_back:?}"
        );
        let for_resource = emulator
            .ok("GetOperationsForResource", json!({"resourceName": name}))
            .await;
        let types: Vec<&str> = for_resource["operations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an operations array, got {for_resource:?}")
            })
            .iter()
            .filter_map(|entry| entry["operationType"].as_str())
            .collect();
        assert!(
            types.contains(&OPERATION_TYPE),
            "case {case}: expected the resource's log to carry a {OPERATION_TYPE} entry, got \
             {types:?}"
        );
    }
}

#[tokio::test]
async fn a_stopped_instance_has_nothing_left_to_put_in_place() {
    let emulator = Emulator::new();
    let name = "metadata-stopped";
    running_instance(&emulator, name).await;
    emulator
        .ok("StopInstance", json!({"instanceName": name}))
        .await;
    wait_for_state(&emulator, name, "stopped").await;

    let row = Row {
        case: "a stopped instance",
        http_tokens: Some("required"),
        http_endpoint: Some("disabled"),
        http_put_response_hop_limit: Some(4),
        http_protocol_ipv6: Some("enabled"),
        expect_http_tokens: "required",
        expect_http_endpoint: "disabled",
        expect_http_put_response_hop_limit: 4,
        expect_http_protocol_ipv6: "enabled",
    };
    let answered = emulator
        .ok(
            "UpdateInstanceMetadataOptions",
            update_body(
                name,
                row.http_tokens,
                row.http_endpoint,
                row.http_put_response_hop_limit,
                row.http_protocol_ipv6,
            ),
        )
        .await;
    // An instance that is not running processes no metadata traffic, so there is
    // nothing to put in place: the selection — and the entry for it — is
    // complete when the call returns.
    assert_logged(row.case, &answered, name, true);

    let options = metadata_options(&emulator, name).await;
    assert_eq!(
        options["state"].as_str(),
        Some(APPLIED),
        "case {}: a stopped instance has the new selection already, got {options:?}",
        row.case
    );
    assert_settings(row.case, &options, &row);
}

#[tokio::test]
async fn a_hop_limit_the_metadata_service_does_not_accept_is_refused() {
    let emulator = Emulator::new();
    let name = "metadata-hop-limit";
    running_instance(&emulator, name).await;
    seed(&emulator, name).await;
    let seeded = metadata_options(&emulator, name).await;

    // One past each published bound, and the two values a caller most easily
    // sends by mistake.
    for hop_limit in [MIN_HOP_LIMIT - 1, MAX_HOP_LIMIT + 1, -1, i64::from(i32::MIN)] {
        let case = format!("hop limit {hop_limit}");
        let (status, body) = emulator
            .call(
                "UpdateInstanceMetadataOptions",
                update_body(name, None, None, Some(hop_limit), None),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled input refusal, got {body:?}"
        );
        // A refused request changes nothing: the seeded selection still stands.
        let after = metadata_options(&emulator, name).await;
        assert_eq!(
            after, seeded,
            "case {case}: a refused request must leave the options alone, expected {seeded:?}, \
             got {after:?}"
        );
    }

    // The log carries only the seed's entry, so nothing was recorded for the
    // refused requests.
    let for_resource = emulator
        .ok("GetOperationsForResource", json!({"resourceName": name}))
        .await;
    let entries: Vec<&str> = for_resource["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an operations array, got {for_resource:?}"))
        .iter()
        .filter_map(|entry| entry["operationType"].as_str())
        .filter(|operation_type| *operation_type == OPERATION_TYPE)
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected only the seed's {OPERATION_TYPE} entry, got {entries:?}"
    );
}

#[tokio::test]
async fn an_instance_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    // A name that was never created, and one that was created and then deleted:
    // neither addresses an instance this account holds.
    let deleted = "metadata-deleted";
    running_instance(&emulator, deleted).await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": deleted}))
        .await;

    for name in ["metadata-never-created", deleted] {
        let case = format!("instance {name}");
        let (status, body) = emulator
            .call(
                "UpdateInstanceMetadataOptions",
                update_body(name, Some("required"), None, None, None),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the service's not-found envelope, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled not-found error, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist code a client branches on, got {body:?}"
        );
    }
}
