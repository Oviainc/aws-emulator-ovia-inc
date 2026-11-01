// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the worker slice of AWS Deadline Cloud: `CreateWorker` and
//! `GetWorker`.
//!
//! The rules under test:
//!
//! * a worker is registered under the fleet its path names, under a minted
//!   `^worker-[0-9a-f]{32}$` id, and `GetWorker` reports the values that registration
//!   persisted — including the host properties the request carried, with no EC2 instance
//!   members for a host this emulator never launched;
//! * a registered worker is `CREATED` until something reports it onward;
//! * a fleet's reported `workerCount` is the number of workers it holds;
//! * a worker hangs off its fleet and a fleet off its farm, so a call naming an unheld
//!   farm, an unheld fleet, or a resource of another parent is a 404 naming the level that
//!   is missing;
//! * `clientToken` is `@idempotencyToken`, so a create retried with the same token on the
//!   same fleet answers with the worker the first call registered;
//! * the service holds a fleet's worker count at or below the fleet's own
//!   `maxWorkerCount`, so a create into a full fleet is refused and registers nothing.
//!
//! The harness (`Emulator`) and the fleet helpers are the ones the `fleet` suite drives
//! the service with, so these tests go through routing, deserialization, constraint
//! validation, the handler and serialization, exactly as the router does in production.

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, is_resource_id, string_at, Emulator};
use crate::fleet::{create_fleet, create_fleet_path, customer_managed, get_fleet_path, ROLE_ARN};

fn create_worker_path(farm_id: &str, fleet_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers")
}

fn get_worker_path(farm_id: &str, fleet_id: &str, worker_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers/{worker_id}")
}

/// Create a fleet under `farm_id` whose worker-count window tops out at `max`.
async fn create_fleet_with_max(
    emulator: &Emulator,
    farm_id: &str,
    display_name: &str,
    max: i64,
) -> String {
    let (status, body) = emulator
        .post(
            &create_fleet_path(farm_id),
            json!({
                "displayName": display_name,
                "roleArn": ROLE_ARN,
                "minWorkerCount": 0,
                "maxWorkerCount": max,
                "configuration": customer_managed("LINUX", "x86_64", json!({})),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFleet {display_name}: expected 200, got {status} with {body}"
    );
    string_at(&body, "fleetId")
}

/// Register a worker with `fleet_id`, carrying `body`, and return the call's result.
async fn create_worker(
    emulator: &Emulator,
    farm_id: &str,
    fleet_id: &str,
    body: Value,
    client_token: Option<&str>,
) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &create_worker_path(farm_id, fleet_id),
            Some(body),
            client_token,
        )
        .await
}

/// The `workerCount` `GetFleet` reports for `fleet_id`.
async fn fleet_worker_count(emulator: &Emulator, farm_id: &str, fleet_id: &str) -> i64 {
    let (status, body) = emulator.get(&get_fleet_path(farm_id, fleet_id)).await;
    assert_eq!(
        status, 200,
        "GetFleet {fleet_id}: expected 200, got {status} with {body}"
    );
    body.get("workerCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a workerCount in {body}"))
}

/// A host name at the upper bound of the modeled `HostName` pattern
/// (`smithy_query(query="shape", shape="HostName")`: `^[a-zA-Z0-9_\.\-]{0,255}$`).
fn longest_host_name() -> String {
    "w".repeat(255)
}

#[tokio::test]
async fn create_worker_then_get_worker_returns_what_was_registered() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("workers").await;
    let fleet_id = create_fleet_with_max(&emulator, &farm_id, "registering", 8).await;

    // The rule: GetWorker reports what CreateWorker persisted under that fleet. The rows
    // differ along the host properties the request carries — none at all, a host name
    // only, each address family on its own, several addresses of one family, both
    // families together with the host name at its published length bound — and along the
    // tags the request carries.
    let cases: Vec<(&str, Value)> = vec![
        ("no host properties and no tags", json!({})),
        (
            "a host name only",
            json!({"hostProperties": {"hostName": "render-01"}}),
        ),
        (
            "one IPv4 address",
            json!({"hostProperties": {"ipAddresses": {"ipV4Addresses": ["10.0.0.4"]}}}),
        ),
        (
            "several IPv4 addresses",
            json!({
                "hostProperties": {
                    "ipAddresses": {"ipV4Addresses": ["10.0.0.5", "192.168.1.200", "127.0.0.1"]},
                },
            }),
        ),
        (
            "one IPv6 address",
            json!({
                "hostProperties": {
                    "ipAddresses": {"ipV6Addresses": ["FE80:0000:0000:0000:0202:B3FF:FE1E:8329"]},
                },
            }),
        ),
        (
            "both address families and the longest host name",
            json!({
                "hostProperties": {
                    "hostName": longest_host_name(),
                    "ipAddresses": {
                        "ipV4Addresses": ["10.0.0.6"],
                        "ipV6Addresses": ["2001:0DB8:0000:0000:0000:0000:0000:0001"],
                    },
                },
                "tags": {"team": "lighting", "shot": ""},
            }),
        ),
        (
            "tags without host properties",
            json!({"tags": {"team": "layout"}}),
        ),
    ];

    let mut registered: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let before = fleet_worker_count(&emulator, &farm_id, &fleet_id).await;
        let (status, body) = create_worker(&emulator, &farm_id, &fleet_id, request.clone(), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorker to answer 200, got {status} with {body}"
        );
        let worker_id = string_at(&body, "workerId");
        assert!(
            is_resource_id(&worker_id, "worker"),
            "case {case:?}: expected a minted ^worker-[0-9a-f]{{32}}$ id, got {worker_id:?}"
        );
        assert!(
            !registered.contains(&worker_id),
            "case {case:?}: expected a worker id no earlier registration used, got {worker_id:?}"
        );
        registered.push(worker_id.clone());

        let (status, worker) = emulator
            .get(&get_worker_path(&farm_id, &fleet_id, &worker_id))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorker to answer 200, got {status} with {worker}"
        );
        assert_eq!(
            (
                string_at(&worker, "farmId"),
                string_at(&worker, "fleetId"),
                string_at(&worker, "workerId"),
                string_at(&worker, "status"),
            ),
            (
                farm_id.clone(),
                fleet_id.clone(),
                worker_id.clone(),
                "CREATED".to_string(),
            ),
            "case {case:?}: expected the worker read back as registered, got {worker}"
        );

        // The host properties come back as sent, with the EC2 members of a host nothing
        // launched left out rather than invented.
        let sent = request.get("hostProperties");
        let read = worker.get("hostProperties");
        match sent {
            None => assert!(
                read.is_none(),
                "case {case:?}: expected no hostProperties for a worker registered without any, got {worker}"
            ),
            Some(sent) => {
                assert_eq!(
                    read.and_then(|read| read.get("hostName")),
                    sent.get("hostName"),
                    "case {case:?}: expected the registered hostName, got {worker}"
                );
                assert_eq!(
                    read.and_then(|read| read.get("ipAddresses")),
                    sent.get("ipAddresses"),
                    "case {case:?}: expected the registered ipAddresses, got {worker}"
                );
                assert!(
                    read.is_some_and(|read| read.get("ec2InstanceArn").is_none()
                        && read.get("ec2InstanceType").is_none()),
                    "case {case:?}: expected no EC2 instance members for a host this emulator never launched, got {worker}"
                );
            }
        }

        assert!(
            !string_at(&worker, "createdBy").is_empty(),
            "case {case:?}: expected a createdBy principal, got {worker}"
        );
        assert!(
            worker.get("createdAt").and_then(Value::as_str).is_some(),
            "case {case:?}: expected createdAt as an RFC 3339 string, got {worker}"
        );
        assert!(
            worker.get("updatedAt").is_none() && worker.get("updatedBy").is_none(),
            "case {case:?}: expected no updatedAt/updatedBy before an update, got {worker}"
        );
        assert!(
            worker.get("log").is_none(),
            "case {case:?}: expected no log configuration for a worker nothing logs for, got {worker}"
        );

        // A read does not change what the next reader sees.
        let (_, again) = emulator
            .get(&get_worker_path(&farm_id, &fleet_id, &worker_id))
            .await;
        assert_eq!(
            again, worker,
            "case {case:?}: two reads of one worker disagreed: {worker} then {again}"
        );

        // The fleet's reported worker count is the number of workers it now holds.
        let after = fleet_worker_count(&emulator, &farm_id, &fleet_id).await;
        assert_eq!(
            after,
            before + 1,
            "case {case:?}: expected the fleet's workerCount to count the registration, got {after} after {before}"
        );
    }

    // Every worker of the run is still readable, so no registration overwrote another.
    for worker_id in &registered {
        let (status, worker) = emulator
            .get(&get_worker_path(&farm_id, &fleet_id, worker_id))
            .await;
        assert_eq!(
            status, 200,
            "after the registrations: expected {worker_id} to still be readable, got {status} with {worker}"
        );
    }
}

#[tokio::test]
async fn a_worker_is_reached_only_through_its_own_fleet() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("owner").await;
    let other_farm_id = emulator.farm("stranger").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "owned").await;
    let sibling_fleet_id = create_fleet(&emulator, &farm_id, "sibling").await;
    let other_fleet_id = create_fleet(&emulator, &other_farm_id, "elsewhere").await;
    let absent_farm = absent_id("farm", "00000000000000000000000000000031");
    let absent_fleet = absent_id("fleet", "00000000000000000000000000000032");
    let absent_worker = absent_id("worker", "00000000000000000000000000000033");

    let (status, body) = create_worker(&emulator, &farm_id, &fleet_id, json!({}), None).await;
    assert_eq!(
        status, 200,
        "the first registration: expected 200, got {status} with {body}"
    );
    let worker_id = string_at(&body, "workerId");

    // The rule: a worker hangs off its fleet and a fleet off its farm, so a create naming
    // a parent this emulator does not hold at that level registers nothing and answers a
    // 404 naming the level that is missing. The rows differ along which level is wrong.
    let create_cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "an unheld farm",
            absent_farm.clone(),
            fleet_id.clone(),
            "farm",
            absent_farm.clone(),
        ),
        (
            "an unheld fleet",
            farm_id.clone(),
            absent_fleet.clone(),
            "fleet",
            absent_fleet.clone(),
        ),
        (
            "a fleet of another farm",
            farm_id.clone(),
            other_fleet_id.clone(),
            "fleet",
            other_fleet_id.clone(),
        ),
    ];
    for (case, farm, fleet, resource_type, resource_id) in create_cases {
        let (status, body) = create_worker(&emulator, &farm, &fleet, json!({}), None).await;
        assert_eq!(
            (
                status,
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                resource_type.to_string(),
                resource_id.clone(),
            ),
            "case {case:?}: expected CreateWorker to answer a 404 naming what is missing, got {status} with {body}"
        );
    }

    // The same rule on the read: only the worker's own farm/fleet path reaches it.
    let get_cases: Vec<(&str, String, String, String, &str, String)> = vec![
        (
            "an unheld farm",
            absent_farm.clone(),
            fleet_id.clone(),
            worker_id.clone(),
            "farm",
            absent_farm.clone(),
        ),
        (
            "an unheld fleet",
            farm_id.clone(),
            absent_fleet.clone(),
            worker_id.clone(),
            "fleet",
            absent_fleet.clone(),
        ),
        (
            "another fleet of the same farm",
            farm_id.clone(),
            sibling_fleet_id.clone(),
            worker_id.clone(),
            "worker",
            worker_id.clone(),
        ),
        (
            "a fleet of another farm",
            other_farm_id.clone(),
            other_fleet_id.clone(),
            worker_id.clone(),
            "worker",
            worker_id.clone(),
        ),
        (
            "an unheld worker",
            farm_id.clone(),
            fleet_id.clone(),
            absent_worker.clone(),
            "worker",
            absent_worker.clone(),
        ),
    ];
    for (case, farm, fleet, worker, resource_type, resource_id) in get_cases {
        let (status, body) = emulator.get(&get_worker_path(&farm, &fleet, &worker)).await;
        assert_eq!(
            (
                status,
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                resource_type.to_string(),
                resource_id.clone(),
            ),
            "case {case:?}: expected GetWorker to answer a 404 naming what is missing, got {status} with {body}"
        );
    }

    // None of those refusals registered or removed anything: the worker is still readable
    // through its own path, and the fleets that were named wrongly hold nothing.
    let (status, worker) = emulator
        .get(&get_worker_path(&farm_id, &fleet_id, &worker_id))
        .await;
    assert_eq!(
        status, 200,
        "after the refusals: expected the worker to still be held, got {status} with {worker}"
    );
    for (fleet_farm, fleet) in [
        (&farm_id, &sibling_fleet_id),
        (&other_farm_id, &other_fleet_id),
    ] {
        let count = fleet_worker_count(&emulator, fleet_farm, fleet).await;
        assert_eq!(
            count, 0,
            "after the refusals: expected fleet {fleet} to hold no worker, got {count}"
        );
    }
}

#[tokio::test]
async fn one_client_token_registers_one_worker() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("idempotent registrations").await;
    let fleet_id = create_fleet_with_max(&emulator, &farm_id, "retried", 8).await;
    let other_fleet_id = create_fleet_with_max(&emulator, &farm_id, "another fleet", 8).await;

    let (status, body) = create_worker(
        &emulator,
        &farm_id,
        &fleet_id,
        json!({"hostProperties": {"hostName": "first"}}),
        Some("register-one"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first registration: expected 200, got {status} with {body}"
    );
    let first = string_at(&body, "workerId");

    // The rule: `clientToken` lets the server recognize a retry of the SAME request, so
    // only a retry naming that token on that fleet answers with the worker the first call
    // registered. The rows differ along what changes between the two calls: nothing, the
    // body, the token, or the fleet the create names.
    let cases: Vec<(&str, String, Value, Option<&str>, bool)> = vec![
        (
            "the same token and the same body",
            fleet_id.clone(),
            json!({"hostProperties": {"hostName": "first"}}),
            Some("register-one"),
            true,
        ),
        (
            "the same token with another body",
            fleet_id.clone(),
            json!({"hostProperties": {"hostName": "second"}}),
            Some("register-one"),
            true,
        ),
        (
            "another token on the same fleet",
            fleet_id.clone(),
            json!({}),
            Some("register-two"),
            false,
        ),
        ("no token at all", fleet_id.clone(), json!({}), None, false),
        (
            "the same token on another fleet",
            other_fleet_id.clone(),
            json!({}),
            Some("register-one"),
            false,
        ),
    ];

    for (case, fleet, request, token, same) in cases {
        let (status, body) = create_worker(&emulator, &farm_id, &fleet, request, token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorker to answer 200, got {status} with {body}"
        );
        let worker_id = string_at(&body, "workerId");
        if same {
            assert_eq!(
                worker_id, first,
                "case {case:?}: expected the first registration's worker {first}, got {worker_id}"
            );
        } else {
            assert_ne!(
                worker_id, first,
                "case {case:?}: expected a newly registered worker, got the first one again"
            );
        }
    }

    // A recognized retry registered nothing: the fleet holds the first worker plus the two
    // the un-recognized calls registered, and the first worker still carries what the call
    // that made it sent.
    let count = fleet_worker_count(&emulator, &farm_id, &fleet_id).await;
    assert_eq!(
        count, 3,
        "after the retries: expected the fleet to hold 3 workers, got {count}"
    );
    let (status, worker) = emulator
        .get(&get_worker_path(&farm_id, &fleet_id, &first))
        .await;
    assert_eq!(
        (
            status,
            worker
                .get("hostProperties")
                .and_then(|properties| properties.get("hostName"))
                .cloned(),
        ),
        (200, Some(json!("first"))),
        "after the retries: expected the first worker untouched, got {status} with {worker}"
    );
}

#[tokio::test]
async fn a_fleet_holds_no_more_workers_than_its_maximum_worker_count() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("capacity").await;

    // The rule: the service holds a fleet's worker count at or below the fleet's own
    // maxWorkerCount, so the create that would pass it is refused and registers nothing.
    // The rows differ along the window the fleet was created with, including a fleet that
    // can hold no worker at all and the first value past each window.
    let cases: Vec<(&str, i64)> = vec![
        ("a fleet that holds no worker", 0),
        ("a fleet that holds one worker", 1),
        ("a fleet that holds three workers", 3),
    ];

    for (case, max) in cases {
        let fleet_id = create_fleet_with_max(&emulator, &farm_id, case, max).await;
        for index in 0..max {
            let (status, body) = create_worker(&emulator, &farm_id, &fleet_id, json!({}), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected worker {index} to be registered, got {status} with {body}"
            );
        }

        let (status, body) = create_worker(&emulator, &farm_id, &fleet_id, json!({}), None).await;
        assert_eq!(
            (
                status,
                error_code(&body),
                string_at(&body, "reason"),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (
                409,
                "ConflictException".to_string(),
                "RESOURCE_IN_USE".to_string(),
                "fleet".to_string(),
                fleet_id.clone(),
            ),
            "case {case:?}: expected the create past the window to be refused, got {status} with {body}"
        );

        // The refusal registered nothing: the fleet still holds exactly its maximum.
        let count = fleet_worker_count(&emulator, &farm_id, &fleet_id).await;
        assert_eq!(
            count, max,
            "case {case:?}: expected the fleet to still hold {max} workers, got {count}"
        );
    }
}
