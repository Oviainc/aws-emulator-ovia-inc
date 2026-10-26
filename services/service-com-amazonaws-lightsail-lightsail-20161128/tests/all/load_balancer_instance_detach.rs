// SPDX-License-Identifier: CC0-1.0

//! `DetachInstancesFromLoadBalancer` — taking Lightsail instances out of service
//! behind a load balancer — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness and the registration each case releases are the sibling
//! suites' own ([`crate::instance_lifecycle::Emulator`],
//! `AttachInstancesToLoadBalancer`) — no case reaches into the store.
//!
//! The rule under test: `DetachInstancesFromLoadBalancer` begins the
//! deregistration of every instance the request names from the ONE stored load
//! balancer of (Region, name) — the registration is reported `draining` until the
//! removal the clock owes it is due and is gone from `instanceHealthSummary` from
//! that instant on, the registrations the request does not name keep their place
//! and their health, and an instance the load balancer holds no registration for
//! leaves it as it is — and the call answers one operation-log entry per resource
//! it names: the load balancer, then each instance. A detach the resources'
//! state does not allow — a load balancer the account does not hold in this
//! Region, an instance it does not hold, a request naming no instance or the same
//! instance twice — is refused and releases nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::disk_attachment::{create_running_instance, wait_for_instance_state};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
// The registration this suite releases is made the way a client makes it, with
// the attach suite's own helpers.
use crate::load_balancer_instance_attachment::{
    create_load_balancer, health_summary, registered_instances, wait_for_load_balancer_state,
    wait_for_operation,
};

/// The `InstanceHealthState` a registration whose deregistration is outstanding
/// reports, and the `InstanceHealthReason` for it — both from the modelled enums
/// (`smithy_query(query="shape",shape="InstanceHealthState,InstanceHealthReason")`).
const DRAINING_HEALTH: &str = "draining";
/// See [`DRAINING_HEALTH`].
const DEREGISTRATION_IN_PROGRESS: &str = "Instance.DeregistrationInProgress";

/// The `InstanceHealthState` a registration that is NOT being deregistered
/// reports, from the same modelled enum.
const INITIAL_HEALTH: &str = "initial";
/// See [`INITIAL_HEALTH`].
const REGISTRATION_IN_PROGRESS: &str = "Lb.RegistrationInProgress";

/// The `operationType` every entry this operation logs carries, from the modelled
/// `OperationType` enum.
const OPERATION_TYPE: &str = "DetachInstancesFromLoadBalancer";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The most instance names one request may carry
/// (`crate::state::MAX_INSTANCE_NAMES_PER_REQUEST`): an emulator resource guard,
/// because the modelled `ResourceNameList` declares no length.
const MAX_INSTANCE_NAMES_PER_REQUEST: usize = 50;

/// Longer than the span the clock takes to remove a drained registration
/// (`crate::state::STATE_TRANSITION_MILLIS`), so a case that has to prove a
/// registration SURVIVED can wait the whole span out.
const PAST_THE_DRAIN: Duration = Duration::from_millis(900);

/// The health entry the load balancer `name` reports for the instance
/// `instance_name` in `region`, or `None` when it holds no registration for it.
async fn health_of(
    emulator: &Emulator,
    region: &str,
    name: &str,
    instance_name: &str,
) -> Option<Value> {
    health_summary(emulator, region, name)
        .await
        .into_iter()
        .find(|entry| entry["instanceName"].as_str() == Some(instance_name))
}

/// Register `names` with the load balancer `name` in `region`.
async fn attach(emulator: &Emulator, region: &str, name: &str, names: &[&str]) {
    emulator
        .ok_at(
            None,
            Some(region),
            "AttachInstancesToLoadBalancer",
            json!({"loadBalancerName": name, "instanceNames": names}),
        )
        .await;
}

/// Begin the deregistration of `names` from the load balancer `name` in
/// `region`, answering the `operations` the call logged.
async fn detach(emulator: &Emulator, region: &str, name: &str, names: &[&str]) -> Vec<Value> {
    let answered = emulator
        .ok_at(
            None,
            Some(region),
            "DetachInstancesFromLoadBalancer",
            json!({"loadBalancerName": name, "instanceNames": names}),
        )
        .await;
    answered["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an operations array, got {answered:?}"))
        .clone()
}

/// Poll `GetLoadBalancer` until the load balancer `name` in `region` reports
/// exactly `want` as its registered instances, in that order.
///
/// The removal is owed by the clock, not by the poll: it arrives without anyone
/// asking more than once.
async fn wait_for_registrations(emulator: &Emulator, region: &str, name: &str, want: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let registered = registered_instances(emulator, region, name).await;
        if registered == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the registrations {want:?} on {name}: expected them within 5s, got \
             {registered:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The detach table
// ---------------------------------------------------------------------------

/// One load balancer, the instances registered with it, and the successive
/// `DetachInstancesFromLoadBalancer` calls made against it.
///
/// The rows differ along everything the rule generalises over: the Region (and so
/// the Availability Zone the instances run in), how many of the registered
/// instances one call releases, the order the names come in against the
/// registration order, how many calls the release is spread over, whether a name
/// the load balancer holds no registration for is among them, whether the same
/// name is detached twice, and what state the instance itself is in.
struct Row {
    case: &'static str,
    region: &'static str,
    availability_zone: &'static str,
    load_balancer_name: &'static str,
    /// The instances registered before the first detach, in registration order.
    attached: &'static [&'static str],
    /// Instances stood up for this row but never registered — what a detach that
    /// releases nothing names.
    unregistered: &'static [&'static str],
    /// Instances stopped after the registration and before the first detach.
    stopped: &'static [&'static str],
    /// The successive calls, each the `instanceNames` list of one request.
    calls: &'static [&'static [&'static str]],
    /// The registration order the rule says the load balancer reports once every
    /// call's removal has fallen due.
    remaining: &'static [&'static str],
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "one of two registrations",
        region: "us-east-1",
        availability_zone: "us-east-1a",
        load_balancer_name: "one-off-lb",
        attached: &["keep-1", "drop-2"],
        unregistered: &[],
        stopped: &[],
        calls: &[&["drop-2"]],
        remaining: &["keep-1"],
    },
    Row {
        case: "every registration in one call, in another Region",
        region: "us-west-2",
        availability_zone: "us-west-2b",
        load_balancer_name: "emptied-lb",
        attached: &["gone_1", "gone-2"],
        unregistered: &[],
        stopped: &[],
        calls: &[&["gone_1", "gone-2"]],
        remaining: &[],
    },
    Row {
        case: "two of three, named against the registration order: the survivor keeps its place",
        region: "eu-west-1",
        availability_zone: "eu-west-1c",
        load_balancer_name: "middle-lb",
        attached: &["alpha-1", "mike-2", "zulu-3"],
        unregistered: &[],
        stopped: &[],
        calls: &[&["zulu-3", "alpha-1"]],
        remaining: &["mike-2"],
    },
    Row {
        case: "two calls each releasing one",
        region: "ap-southeast-2",
        availability_zone: "ap-southeast-2a",
        load_balancer_name: "stepwise-lb",
        attached: &["step-1", "step-2", "step-3"],
        unregistered: &[],
        stopped: &[],
        calls: &[&["step-1"], &["step-3"]],
        remaining: &["step-2"],
    },
    Row {
        case: "an instance the load balancer holds no registration for",
        region: "us-east-2",
        availability_zone: "us-east-2b",
        load_balancer_name: "untouched-lb",
        attached: &["held-1"],
        unregistered: &["spare-2"],
        stopped: &[],
        calls: &[&["spare-2"]],
        remaining: &["held-1"],
    },
    Row {
        case: "the same name detached twice: the second call releases nothing",
        region: "eu-central-1",
        availability_zone: "eu-central-1a",
        load_balancer_name: "repeated-lb",
        attached: &["twice-1", "twice-2"],
        unregistered: &[],
        stopped: &[],
        calls: &[&["twice-1"], &["twice-1"]],
        remaining: &["twice-2"],
    },
    Row {
        case: "an instance stopped after it was registered is still detachable",
        region: "ca-central-1",
        availability_zone: "ca-central-1a",
        load_balancer_name: "stopped-member-lb",
        attached: &["halt-1", "halt-2"],
        unregistered: &[],
        stopped: &["halt-1"],
        calls: &[&["halt-1"]],
        remaining: &["halt-2"],
    },
];

/// RULE: `DetachInstancesFromLoadBalancer` releases the registration the stored
/// load balancer holds for every instance the request names — leaving every other
/// registration where it is — and answers one operation-log entry per resource it
/// names, non-terminal while a release is still owed by the clock.
#[tokio::test]
async fn detaching_instances_releases_their_registrations() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let mut created: BTreeSet<&str> = BTreeSet::new();
        for name in row
            .attached
            .iter()
            .chain(row.unregistered)
            .chain(row.calls.iter().copied().flatten())
        {
            if created.insert(name) {
                create_running_instance(&emulator, name, row.availability_zone).await;
            }
        }
        create_load_balancer(&emulator, row.region, row.load_balancer_name, 80).await;
        wait_for_load_balancer_state(&emulator, row.region, row.load_balancer_name, "active").await;
        attach(&emulator, row.region, row.load_balancer_name, row.attached).await;
        let registered =
            registered_instances(&emulator, row.region, row.load_balancer_name).await;
        assert_eq!(
            registered, row.attached,
            "case {case}: expected the registrations {:?} to start with, got {registered:?}",
            row.attached
        );
        for name in row.stopped {
            emulator
                .ok("StopInstance", json!({"instanceName": name}))
                .await;
            wait_for_instance_state(&emulator, name, "stopped").await;
        }

        // What the load balancer still holds, in registration order — the state
        // each call is judged against.
        let mut held: Vec<&str> = row.attached.to_vec();
        for names in row.calls {
            // Which of the named instances this call actually releases, and so
            // whether the clock owes the call anything at all.
            let releases = names.iter().any(|name| held.contains(name));
            let operations = detach(&emulator, row.region, row.load_balancer_name, names).await;
            // One entry per resource the call names: the load balancer first,
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
                // A call that released a registration is finished when the clock
                // removes it; a call that released none was finished when it
                // returned.
                let (want_status, want_terminal) = if releases {
                    ("Started", false)
                } else {
                    ("Succeeded", true)
                };
                assert_eq!(
                    operation["status"].as_str(),
                    Some(want_status),
                    "case {case}: expected a {want_status} operation at {position}, got \
                     {operation:?}"
                );
                assert_eq!(
                    operation["isTerminal"].as_bool(),
                    Some(want_terminal),
                    "case {case}: expected isTerminal {want_terminal} at {position}, got \
                     {operation:?}"
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
                // The clock owes an outstanding release its terminal status, and
                // the entry settles into it without anyone asking twice.
                let settled = wait_for_operation(&emulator, &id).await;
                assert_eq!(
                    settled["isTerminal"].as_bool(),
                    Some(true),
                    "case {case}: expected a settled entry to be terminal, got {settled:?}"
                );
            }

            held.retain(|name| !names.contains(name));
            wait_for_registrations(&emulator, row.region, row.load_balancer_name, &held).await;
        }

        // The registrations the calls did not name are untouched, health and all
        // — nothing about them was deregistered.
        wait_for_registrations(&emulator, row.region, row.load_balancer_name, row.remaining).await;
        for entry in health_summary(&emulator, row.region, row.load_balancer_name).await {
            assert_eq!(
                (
                    entry["instanceHealth"].as_str(),
                    entry["instanceHealthReason"].as_str()
                ),
                (Some(INITIAL_HEALTH), Some(REGISTRATION_IN_PROGRESS)),
                "case {case}: expected a surviving registration to be untouched, got {entry:?}"
            );
        }
        // Detaching instances changes nothing else about the load balancer: it
        // still serves plain HTTP on the instance port the create asked for.
        let balancer =
            crate::load_balancer_instance_attachment::load_balancer(
                &emulator,
                row.region,
                row.load_balancer_name,
            )
            .await;
        assert_eq!(
            (
                balancer["state"].as_str(),
                balancer["instancePort"].as_i64(),
                balancer["protocol"].as_str()
            ),
            (Some("active"), Some(80), Some("HTTP")),
            "case {case}: expected the load balancer itself untouched, got {balancer:?}"
        );
    }
}

/// RULE (the span is never zero): the removal is work the clock owes, so a load
/// balancer read straight after an accepted detach still reports the instance —
/// in the modelled `draining` health, the transient state AWS shows too — and
/// registering it again while that release is in flight keeps it in service.
#[tokio::test]
async fn a_detach_in_flight_reports_the_instance_as_draining() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "drain-1", "us-east-2a").await;
    create_running_instance(&emulator, "drain-2", "us-east-2a").await;
    create_load_balancer(&emulator, "us-east-2", "draining-lb", 8080).await;
    attach(
        &emulator,
        "us-east-2",
        "draining-lb",
        &["drain-1", "drain-2"],
    )
    .await;

    detach(&emulator, "us-east-2", "draining-lb", &["drain-1"]).await;
    let case = "detach in flight";
    let draining = health_of(&emulator, "us-east-2", "draining-lb", "drain-1")
        .await
        .unwrap_or_else(|| panic!("case {case}: expected drain-1 still registered"));
    assert_eq!(
        (
            draining["instanceHealth"].as_str(),
            draining["instanceHealthReason"].as_str()
        ),
        (Some(DRAINING_HEALTH), Some(DEREGISTRATION_IN_PROGRESS)),
        "case {case}: expected the registration to be draining, got {draining:?}"
    );
    let untouched = health_of(&emulator, "us-east-2", "draining-lb", "drain-2")
        .await
        .unwrap_or_else(|| panic!("case {case}: expected drain-2 still registered"));
    assert_eq!(
        untouched["instanceHealth"].as_str(),
        Some(INITIAL_HEALTH),
        "case {case}: expected the registration nobody detached to be untouched, got {untouched:?}"
    );

    // Registering the draining instance again asks the load balancer to serve it,
    // so the release the clock owed has nothing left to do: the registration is
    // back out of `draining` and is still there once the span has passed.
    let case = "re-registered while draining";
    attach(&emulator, "us-east-2", "draining-lb", &["drain-1"]).await;
    let served = health_of(&emulator, "us-east-2", "draining-lb", "drain-1")
        .await
        .unwrap_or_else(|| panic!("case {case}: expected drain-1 still registered"));
    assert_eq!(
        (
            served["instanceHealth"].as_str(),
            served["instanceHealthReason"].as_str()
        ),
        (Some(INITIAL_HEALTH), Some(REGISTRATION_IN_PROGRESS)),
        "case {case}: expected the registration back in service, got {served:?}"
    );
    tokio::time::sleep(PAST_THE_DRAIN).await;
    let registered = registered_instances(&emulator, "us-east-2", "draining-lb").await;
    assert_eq!(
        registered,
        vec!["drain-1".to_string(), "drain-2".to_string()],
        "case {case}: expected both registrations past the drain, got {registered:?}"
    );

    // And a detach made after that is released on its own clock, so the
    // registration goes for good.
    let case = "detached again";
    detach(&emulator, "us-east-2", "draining-lb", &["drain-1"]).await;
    wait_for_registrations(&emulator, "us-east-2", "draining-lb", &["drain-2"]).await;
    let gone = health_of(&emulator, "us-east-2", "draining-lb", "drain-1").await;
    assert!(
        gone.is_none(),
        "case {case}: expected no registration for drain-1, got {gone:?}"
    );
}

/// RULE: a load balancer is addressed within the Region the request is made in,
/// so a detach made in one Region releases nothing from its namesake in another
/// and nothing from another load balancer of its own Region.
#[tokio::test]
async fn a_detach_releases_only_the_load_balancer_of_its_own_region() {
    let emulator = Emulator::new();
    // An instance is registered only with a load balancer of its OWN Region, so
    // each Region's load balancer holds its own member.
    create_running_instance(&emulator, "east-member", "us-east-1a").await;
    create_running_instance(&emulator, "west-member", "us-west-2a").await;
    // The SAME load balancer name in two Regions: two records, each holding its
    // own registration.
    for (region, member) in [("us-east-1", "east-member"), ("us-west-2", "west-member")] {
        create_load_balancer(&emulator, region, "twin-detach-lb", 80).await;
        attach(&emulator, region, "twin-detach-lb", &[member]).await;
    }
    create_load_balancer(&emulator, "us-east-1", "neighbour-detach-lb", 80).await;
    attach(
        &emulator,
        "us-east-1",
        "neighbour-detach-lb",
        &["east-member"],
    )
    .await;

    detach(&emulator, "us-east-1", "twin-detach-lb", &["east-member"]).await;
    wait_for_registrations(&emulator, "us-east-1", "twin-detach-lb", &[]).await;

    let cases: &[(&str, &str, &str, &[&str])] = &[
        (
            "the load balancer detached from",
            "us-east-1",
            "twin-detach-lb",
            &[],
        ),
        (
            "its namesake in another Region",
            "us-west-2",
            "twin-detach-lb",
            &["west-member"],
        ),
        (
            "another load balancer of the same Region",
            "us-east-1",
            "neighbour-detach-lb",
            &["east-member"],
        ),
    ];
    for (case, region, name, want) in cases {
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

/// One `DetachInstancesFromLoadBalancer` request the service must refuse, and the
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
/// `NotFoundException` at 400 with `DoesNotExist` in the body, and
/// `InvalidInputException` is modelled `@httpError(400)`.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a load balancer name the account does not hold",
        region: "us-east-1",
        load_balancer_name: "ghost-lb",
        instance_names: &["guard-member"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "the load balancer exists, but in another Region",
        region: "eu-west-1",
        load_balancer_name: "guard-detach-lb",
        instance_names: &["guard-member"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "an instance name the account does not hold",
        region: "us-east-1",
        load_balancer_name: "guard-detach-lb",
        instance_names: &["ghost-instance"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "one name of several names nothing: none of the others is released",
        region: "us-east-1",
        load_balancer_name: "guard-detach-lb",
        instance_names: &["guard-member", "ghost-instance"],
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "no instance named at all",
        region: "us-east-1",
        load_balancer_name: "guard-detach-lb",
        instance_names: &[],
        code: "InvalidInputException",
        body_code: None,
    },
    Refusal {
        case: "the same instance named twice in one request",
        region: "us-east-1",
        load_balancer_name: "guard-detach-lb",
        instance_names: &["guard-member", "guard-member"],
        code: "InvalidInputException",
        body_code: None,
    },
];

/// RULE: a detach the two resources' state does not allow is refused with the
/// modelled error for its cause and releases NOTHING — not even the names of the
/// same request that were registered.
#[tokio::test]
async fn a_refused_detach_releases_nothing() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "guard-member", "us-east-1a").await;
    create_running_instance(&emulator, "guard-second", "us-east-1b").await;
    create_load_balancer(&emulator, "us-east-1", "guard-detach-lb", 80).await;
    attach(
        &emulator,
        "us-east-1",
        "guard-detach-lb",
        &["guard-member", "guard-second"],
    )
    .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call_in(
                Some(refusal.region),
                "DetachInstancesFromLoadBalancer",
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
        // Not even a release the clock could still be owing: the registrations
        // are all there, and all in service.
        let registered = registered_instances(&emulator, "us-east-1", "guard-detach-lb").await;
        assert_eq!(
            registered,
            vec!["guard-member".to_string(), "guard-second".to_string()],
            "case {case}: expected the refusal to release nothing, got {registered:?}"
        );
        for entry in health_summary(&emulator, "us-east-1", "guard-detach-lb").await {
            assert_eq!(
                entry["instanceHealth"].as_str(),
                Some(INITIAL_HEALTH),
                "case {case}: expected no registration to be draining, got {entry:?}"
            );
        }
    }
}

/// RULE: a caller-sized list is bounded, so a request naming more instances than
/// one request may carry is refused with the modelled `InvalidInputException`
/// rather than walked.
#[tokio::test]
async fn more_names_than_one_request_may_carry_is_refused() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "bounded-member", "us-east-1a").await;
    create_load_balancer(&emulator, "us-east-1", "bounded-detach-lb", 80).await;
    attach(
        &emulator,
        "us-east-1",
        "bounded-detach-lb",
        &["bounded-member"],
    )
    .await;

    // One name past the bound, and the bound itself: the first is refused before
    // the list is walked, the second is a list this service accepts (every name
    // but the registered one is unknown, so it is refused for THAT instead).
    let cases: &[(&str, usize, &str)] = &[
        ("one more name than the bound", MAX_INSTANCE_NAMES_PER_REQUEST + 1, "InvalidInputException"),
        ("the bound itself", MAX_INSTANCE_NAMES_PER_REQUEST, "NotFoundException"),
    ];
    for (case, count, want_code) in cases {
        let names: Vec<String> = (0..*count).map(|index| format!("bulk-{index}")).collect();
        let (status, body) = emulator
            .call_in(
                Some("us-east-1"),
                "DetachInstancesFromLoadBalancer",
                json!({"loadBalancerName": "bounded-detach-lb", "instanceNames": names}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, *want_code,
            "case {case}: expected {want_code}, got {code} {body:?}"
        );
        let registered = registered_instances(&emulator, "us-east-1", "bounded-detach-lb").await;
        assert_eq!(
            registered,
            vec!["bounded-member".to_string()],
            "case {case}: expected the refusal to release nothing, got {registered:?}"
        );
    }
}
