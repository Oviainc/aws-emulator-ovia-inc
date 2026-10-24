// SPDX-License-Identifier: CC0-1.0

//! Registering Lightsail instances with a load balancer, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `AttachInstancesToLoadBalancer` records every instance
//! the request names on the ONE stored load balancer of (Region, name), in
//! registration order and at most once, so every later `GetLoadBalancer` reports
//! one `instanceHealthSummary` entry per registered instance, and it answers one
//! operation-log entry per resource the call affected — the load balancer, and
//! each instance registered with it. An attach the two resources' state does not
//! allow — a load balancer or an instance the account does not hold in this
//! Region, an instance that is not running, an instance of another Region, a
//! request naming no instance or the same instance twice — is refused and
//! registers nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check and the instance
// stand-up this test target already exposes: a sibling suite drives the service
// through the same front door rather than standing up a second one.
use crate::disk_attachment::{create_running_instance, wait_for_instance_state};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The `InstanceHealthState` an instance just registered with a load balancer
/// reports, from the modelled enum and the `instanceHealthReason` documentation
/// ("`Lb.RegistrationInProgress` - The target instance is in the process of being
/// registered with the load balancer").
const INITIAL_HEALTH: &str = "initial";

/// See [`INITIAL_HEALTH`].
const REGISTRATION_IN_PROGRESS: &str = "Lb.RegistrationInProgress";

/// The `operationType` every entry this operation logs carries, from the modelled
/// `OperationType` enum.
const OPERATION_TYPE: &str = "AttachInstancesToLoadBalancer";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The most instance names one request may carry
/// (`crate::state::MAX_INSTANCE_NAMES_PER_REQUEST`): an emulator resource guard,
/// because the modelled `ResourceNameList` declares no length.
const MAX_INSTANCE_NAMES_PER_REQUEST: usize = 50;

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

/// The `instanceHealthSummary` entries the load balancer `name` reports in
/// `region`, in the order it holds them.
async fn health_summary(emulator: &Emulator, region: &str, name: &str) -> Vec<Value> {
    load_balancer(emulator, region, name).await["instanceHealthSummary"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The instance names the load balancer `name` reports as registered in
/// `region`, in registration order.
async fn registered_instances(emulator: &Emulator, region: &str, name: &str) -> Vec<String> {
    health_summary(emulator, region, name)
        .await
        .iter()
        .map(|entry| {
            entry["instanceName"]
                .as_str()
                .unwrap_or("<no instanceName>")
                .to_string()
        })
        .collect()
}

/// Create the load balancer `name` in `region` on `instance_port`.
async fn create_load_balancer(emulator: &Emulator, region: &str, name: &str, instance_port: i64) {
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancer",
            json!({"loadBalancerName": name, "instancePort": instance_port}),
        )
        .await;
}

/// Poll `GetLoadBalancer` until the load balancer `name` reports `want` in
/// `region`, answering the whole record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_load_balancer_state(
    emulator: &Emulator,
    region: &str,
    name: &str,
    want: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let balancer = load_balancer(emulator, region, name).await;
        if balancer["state"].as_str() == Some(want) {
            return balancer;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for load balancer '{want}' on {name}: expected it within 5s, got {:?}",
            balancer["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll `GetOperation` until the log entry `id` reports the terminal
/// `Succeeded`, answering it.
async fn wait_for_operation(emulator: &Emulator, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let operation = emulator
            .ok("GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        if operation["status"].as_str() == Some("Succeeded") {
            return operation;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for operation {id}: expected Succeeded within 5s, got {operation:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The attach table
// ---------------------------------------------------------------------------

/// One load balancer and the successive `AttachInstancesToLoadBalancer` calls
/// made against it.
///
/// The rows differ along everything the rule generalises over: the Region (and so
/// the Availability Zone the instances run in), the instance port the load
/// balancer was created on, how many instances one call names, the order the
/// names come in, how many calls the registration is spread over, whether a name
/// is registered twice, and whether the load balancer has finished provisioning
/// when the attach is made.
struct Row {
    case: &'static str,
    region: &'static str,
    availability_zone: &'static str,
    load_balancer_name: &'static str,
    instance_port: i64,
    /// Whether the load balancer is waited into `active` before the first
    /// attach: the operation's documentation makes the attach conditional on the
    /// INSTANCE being running, not on the load balancer having settled.
    wait_active: bool,
    /// The successive calls, each the `instanceNames` list of one request.
    calls: &'static [&'static [&'static str]],
    /// The registration order the rule says the load balancer then reports.
    registered: &'static [&'static str],
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "one instance in one call",
        region: "us-east-1",
        availability_zone: "us-east-1a",
        load_balancer_name: "single-lb",
        instance_port: 80,
        wait_active: true,
        calls: &[&["web-1"]],
        registered: &["web-1"],
    },
    Row {
        case: "two instances in one call, in another Region",
        region: "us-west-2",
        availability_zone: "us-west-2b",
        load_balancer_name: "pair-lb",
        instance_port: 8080,
        wait_active: true,
        calls: &[&["app_1", "app-2"]],
        registered: &["app_1", "app-2"],
    },
    Row {
        case: "three instances in one call: the request's order is the registration order",
        region: "eu-west-1",
        availability_zone: "eu-west-1c",
        load_balancer_name: "trio-lb",
        instance_port: 3000,
        wait_active: true,
        calls: &[&["zulu-3", "mike-2", "alpha-1"]],
        registered: &["zulu-3", "mike-2", "alpha-1"],
    },
    Row {
        case: "two calls append to the registration the first one made",
        region: "ap-southeast-2",
        availability_zone: "ap-southeast-2a",
        load_balancer_name: "growing-lb",
        instance_port: 443,
        wait_active: true,
        calls: &[&["first-1"], &["second-2", "third-3"]],
        registered: &["first-1", "second-2", "third-3"],
    },
    Row {
        case: "registering an already registered instance reports it once",
        region: "us-east-2",
        availability_zone: "us-east-2b",
        load_balancer_name: "idempotent-lb",
        instance_port: 80,
        wait_active: true,
        calls: &[&["dup-1", "dup-2"], &["dup-1"], &["dup-2", "dup-1"]],
        registered: &["dup-1", "dup-2"],
    },
    Row {
        case: "attached while the load balancer is still provisioning",
        region: "eu-central-1",
        availability_zone: "eu-central-1a",
        load_balancer_name: "provisioning-lb",
        instance_port: 65535,
        wait_active: false,
        calls: &[&["early-1"]],
        registered: &["early-1"],
    },
];

/// RULE: `AttachInstancesToLoadBalancer` registers every instance the request
/// names with the stored load balancer — once each, in registration order — so
/// every later `GetLoadBalancer` reports one `instanceHealthSummary` entry per
/// registered instance, and the call answers one operation-log entry per resource
/// it affected.
#[tokio::test]
async fn attaching_instances_registers_them_with_the_load_balancer() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        // The instances come first, so a row about a load balancer that has not
        // finished provisioning does not spend that span standing instances up.
        let mut created: BTreeSet<&str> = BTreeSet::new();
        for name in row.calls.iter().copied().flatten() {
            if created.insert(name) {
                create_running_instance(&emulator, name, row.availability_zone).await;
            }
        }
        create_load_balancer(
            &emulator,
            row.region,
            row.load_balancer_name,
            row.instance_port,
        )
        .await;
        if row.wait_active {
            wait_for_load_balancer_state(&emulator, row.region, row.load_balancer_name, "active")
                .await;
        }

        for names in row.calls {
            let answered = emulator
                .ok_at(
                    None,
                    Some(row.region),
                    "AttachInstancesToLoadBalancer",
                    json!({
                        "loadBalancerName": row.load_balancer_name,
                        "instanceNames": names,
                    }),
                )
                .await;
            let operations = answered["operations"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected an operations array, got {answered:?}")
                })
                .clone();
            // One entry per resource the call affected: the load balancer first,
            // then one per instance in the order the request named them.
            assert_eq!(
                operations.len(),
                names.len() + 1,
                "case {case}: expected {} operations for {names:?}, got {operations:?}",
                names.len() + 1
            );
            for (position, operation) in operations.iter().enumerate() {
                let (want_type, want_name) = if position == 0 {
                    ("LoadBalancer", row.load_balancer_name)
                } else {
                    ("Instance", names[position - 1])
                };
                assert_eq!(
                    operation["operationType"].as_str(),
                    Some(OPERATION_TYPE),
                    "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
                );
                assert_eq!(
                    operation["resourceType"].as_str(),
                    Some(want_type),
                    "case {case}: expected the {want_type} resource type at {position}, got \
                     {operation:?}"
                );
                assert_eq!(
                    operation["resourceName"].as_str(),
                    Some(want_name),
                    "case {case}: expected the entry at {position} to name {want_name}, got \
                     {operation:?}"
                );
                assert_eq!(
                    operation["status"].as_str(),
                    Some("Started"),
                    "case {case}: expected a Started operation, got {operation:?}"
                );
                assert_eq!(
                    operation["isTerminal"].as_bool(),
                    Some(false),
                    "case {case}: expected a non-terminal operation, got {operation:?}"
                );
                // Each entry is located where its OWN resource is: the load
                // balancer across the Region's zones (so it names none of them),
                // each instance in the zone it runs in.
                assert_eq!(
                    operation["location"]["regionName"].as_str(),
                    Some(row.region),
                    "case {case}: expected the entry at {position} in {}, got {operation:?}",
                    row.region
                );
                let want_zone = (position > 0).then_some(row.availability_zone);
                assert_eq!(
                    operation["location"]["availabilityZone"].as_str(),
                    want_zone,
                    "case {case}: expected availabilityZone {want_zone:?} at {position}, got \
                     {operation:?}"
                );
                let id = operation["id"].as_str().unwrap_or_default().to_string();
                assert!(
                    is_uuid_shaped(&id),
                    "case {case}: expected a uuid-shaped operation id, got {id:?}"
                );
                assert!(
                    operation_ids.insert(id.clone()),
                    "case {case}: expected a fresh operation id, got the already-used {id:?}"
                );
                // The clock owes the registration its terminal status, and the
                // entry settles into it without anyone asking twice.
                let settled = wait_for_operation(&emulator, &id).await;
                assert_eq!(
                    settled["isTerminal"].as_bool(),
                    Some(true),
                    "case {case}: expected a settled entry to be terminal, got {settled:?}"
                );
            }
        }

        // The registration, as the LOAD BALANCER reports it — after the state
        // the clock owed it has been applied, so the settle did not drop it.
        let balancer =
            wait_for_load_balancer_state(&emulator, row.region, row.load_balancer_name, "active")
                .await;
        let summary = balancer["instanceHealthSummary"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let names: Vec<&str> = summary
            .iter()
            .map(|entry| entry["instanceName"].as_str().unwrap_or("<no instanceName>"))
            .collect();
        assert_eq!(
            names, row.registered,
            "case {case}: expected the registration order {:?}, got {names:?}",
            row.registered
        );
        for entry in &summary {
            assert_eq!(
                entry["instanceHealth"].as_str(),
                Some(INITIAL_HEALTH),
                "case {case}: expected a newly registered instance to be {INITIAL_HEALTH}, got \
                 {entry:?}"
            );
            assert_eq!(
                entry["instanceHealthReason"].as_str(),
                Some(REGISTRATION_IN_PROGRESS),
                "case {case}: expected the reason {REGISTRATION_IN_PROGRESS}, got {entry:?}"
            );
        }
        // Registering instances changes nothing else about the load balancer: it
        // has no attached certificate, so it still serves plain HTTP on port 80,
        // and the instance port the create asked for is untouched.
        assert_eq!(
            balancer["instancePort"].as_i64(),
            Some(row.instance_port),
            "case {case}: expected the created instance port to be untouched, got {balancer:?}"
        );
        assert_eq!(
            balancer["protocol"].as_str(),
            Some("HTTP"),
            "case {case}: expected a load balancer with no attached certificate to serve HTTP, \
             got {balancer:?}"
        );
        assert_eq!(
            balancer["publicPorts"].as_array().map(Vec::len),
            Some(1),
            "case {case}: expected the single HTTP public port, got {balancer:?}"
        );
    }
}

/// RULE: a load balancer is addressed within the Region the request is made in,
/// so a registration made in one Region is invisible in another and the instances
/// of one load balancer are not reported by another.
#[tokio::test]
async fn registrations_are_per_load_balancer_and_per_region() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "shared-instance", "us-east-1a").await;
    create_running_instance(&emulator, "other-instance", "us-east-1a").await;
    // The SAME load balancer name in two Regions: two records, so a registration
    // against one is not a registration against the other.
    for region in ["us-east-1", "us-west-2"] {
        create_load_balancer(&emulator, region, "twin-lb", 80).await;
    }
    create_load_balancer(&emulator, "us-east-1", "neighbour-lb", 80).await;

    emulator
        .ok_at(
            None,
            Some("us-east-1"),
            "AttachInstancesToLoadBalancer",
            json!({"loadBalancerName": "twin-lb", "instanceNames": ["shared-instance"]}),
        )
        .await;

    let cases: &[(&str, &str, &[&str])] = &[
        ("the load balancer attached to", "us-east-1", &[
            "shared-instance"
        ]),
        ("its namesake in another Region", "us-west-2", &[]),
        ("another load balancer of the same Region", "us-east-1", &[]),
    ];
    for (position, (case, region, want)) in cases.iter().enumerate() {
        let name = if position == 2 { "neighbour-lb" } else { "twin-lb" };
        let registered = registered_instances(&emulator, region, name).await;
        assert_eq!(
            registered, *want,
            "case {case}: expected {name} in {region} to report {want:?}, got {registered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `AttachInstancesToLoadBalancer` request the service must refuse, and the
/// error envelope it must refuse it with.
struct Refusal {
    case: &'static str,
    /// The Region the call is made in.
    region: &'static str,
    load_balancer_name: &'static str,
    instance_names: &'static [&'static str],
    /// The wire error code (`__type`).
    code: &'static str,
    /// The body's `code` member, for the envelope that carries one.
    body_code: Option<&'static str>,
}

/// The table. Every row is refused with HTTP 400: Lightsail answers its
/// `NotFoundException` at 400 with `DoesNotExist` in the body, and both
/// `InvalidInputException` and `OperationFailureException` are modelled
/// `@httpError(400)`.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a load balancer name the account does not hold",
        region: "us-east-1",
        load_balancer_name: "ghost-lb",
        instance_names: &["guard-running"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "the load balancer exists, but in another Region",
        region: "eu-west-1",
        load_balancer_name: "guard-lb",
        instance_names: &["guard-running"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "an instance name the account does not hold",
        region: "us-east-1",
        load_balancer_name: "guard-lb",
        instance_names: &["ghost-instance"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "one name of several names nothing: none of the others is registered",
        region: "us-east-1",
        load_balancer_name: "guard-lb",
        instance_names: &["guard-second", "ghost-instance"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "an instance that is stopped rather than running",
        region: "us-east-1",
        load_balancer_name: "guard-lb",
        instance_names: &["guard-stopped"],
        code: "OperationFailureException",
        body_code: None,
    },
    Refusal {
        case: "a running instance of another Region",
        region: "us-east-1",
        load_balancer_name: "guard-lb",
        instance_names: &["guard-elsewhere"],
        code: "OperationFailureException",
        body_code: None,
    },
    Refusal {
        case: "a stopped instance beside a registrable one: neither is registered",
        region: "us-east-1",
        load_balancer_name: "guard-lb",
        instance_names: &["guard-second", "guard-stopped"],
        code: "OperationFailureException",
        body_code: None,
    },
    Refusal {
        case: "no instance named at all",
        region: "us-east-1",
        load_balancer_name: "guard-lb",
        instance_names: &[],
        code: "InvalidInputException",
        body_code: None,
    },
    Refusal {
        case: "the same instance named twice in one request",
        region: "us-east-1",
        load_balancer_name: "guard-lb",
        instance_names: &["guard-second", "guard-second"],
        code: "InvalidInputException",
        body_code: None,
    },
];

/// RULE: an attach the two resources' state does not allow is refused with the
/// modelled error for its cause and registers NOTHING — not even the names of the
/// same request that were registrable.
#[tokio::test]
async fn a_refused_attach_registers_nothing() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "guard-running", "us-east-1a").await;
    create_running_instance(&emulator, "guard-second", "us-east-1b").await;
    create_running_instance(&emulator, "guard-stopped", "us-east-1a").await;
    create_running_instance(&emulator, "guard-elsewhere", "us-west-2a").await;
    emulator
        .ok("StopInstance", json!({"instanceName": "guard-stopped"}))
        .await;
    wait_for_instance_state(&emulator, "guard-stopped", "stopped").await;
    create_load_balancer(&emulator, "us-east-1", "guard-lb", 80).await;
    // One registration the refusals must leave exactly as it is.
    emulator
        .ok_at(
            None,
            Some("us-east-1"),
            "AttachInstancesToLoadBalancer",
            json!({"loadBalancerName": "guard-lb", "instanceNames": ["guard-running"]}),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call_in(
                Some(refusal.region),
                "AttachInstancesToLoadBalancer",
                json!({
                    "loadBalancerName": refusal.load_balancer_name,
                    "instanceNames": refusal.instance_names,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, refusal.code,
            "case {case}: expected the wire error code {}, got {code} {body:?}",
            refusal.code
        );
        assert_eq!(
            body["code"].as_str(),
            refusal.body_code,
            "case {case}: expected the body code {:?}, got {body:?}",
            refusal.body_code
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case}: expected a message saying what was refused, got {body:?}"
        );
        let registered = registered_instances(&emulator, "us-east-1", "guard-lb").await;
        assert_eq!(
            registered,
            vec!["guard-running".to_string()],
            "case {case}: expected the refusal to register nothing, got {registered:?}"
        );
    }
}

/// RULE: a caller-sized list is bounded, so a request naming more instances than
/// one request may carry is refused with the modelled `InvalidInputException`
/// rather than walked.
#[tokio::test]
async fn more_names_than_one_request_may_carry_is_refused() {
    let emulator = Emulator::new();
    create_load_balancer(&emulator, "us-east-1", "bounded-lb", 80).await;
    let names: Vec<String> = (0..=MAX_INSTANCE_NAMES_PER_REQUEST)
        .map(|index| format!("bulk-{index}"))
        .collect();
    let case = "one more name than the bound";
    let (status, body) = emulator
        .call_in(
            Some("us-east-1"),
            "AttachInstancesToLoadBalancer",
            json!({"loadBalancerName": "bounded-lb", "instanceNames": names}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "InvalidInputException",
        "case {case}: expected InvalidInputException, got {code} {body:?}"
    );
    let registered = registered_instances(&emulator, "us-east-1", "bounded-lb").await;
    assert!(
        registered.is_empty(),
        "case {case}: expected the refusal to register nothing, got {registered:?}"
    );
}
