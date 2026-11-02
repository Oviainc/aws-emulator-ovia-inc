// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DiscoverInstancesRevision`, over the wire through
//! `MultiServiceRouter::dispatch` (the same path the emulator serves).
//!
//! The rule under test: `DiscoverInstancesRevision` reports the `InstancesRevision`
//! the service named by `(NamespaceName, ServiceName)` currently holds — a per-service
//! counter that advances exactly once for each call that changes that service's
//! instance set and for no other call, and an unresolvable namespace or service name
//! is refused with that name's modelled not-found error.
//!
//! The harness (`emulator`, `ok`, `failure`, `namespace`, `service`) is the one
//! `cloud_map_core` already defines; this suite reuses it rather than rebuilding it.

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{emulator, emulator_with_engine, failure, namespace, ok, service};

/// The revision `DiscoverInstancesRevision` reports for one service, by name.
async fn revision(router: &MultiServiceRouter, namespace_name: &str, service_name: &str) -> i64 {
    let answered = ok(
        router,
        "DiscoverInstancesRevision",
        json!({ "NamespaceName": namespace_name, "ServiceName": service_name }),
    )
    .await;
    answered["InstancesRevision"].as_i64().unwrap_or_else(|| {
        panic!("DiscoverInstancesRevision must report an InstancesRevision, got {answered}")
    })
}

/// One step of the sequence in
/// [`the_revision_advances_once_per_change_to_the_instance_set`]: what is called
/// against one service, paired there with the revision delta the rule says it owes.
#[derive(Debug)]
enum Step {
    /// `RegisterInstance` — a change to the instance set.
    Register {
        instance: &'static str,
        attributes: Value,
    },
    /// `DeregisterInstance` — a change to the instance set.
    Deregister { instance: &'static str },
    /// Any call that only reports: by the rule it leaves the revision where it is.
    Read {
        operation: &'static str,
        body: Value,
    },
    /// A health-status change, written straight into the store because
    /// `UpdateInstanceCustomHealthStatus` is still a scaffold stub with its own issue.
    /// `DiscoverInstancesRevisionResponse$InstancesRevision` states this case: "The
    /// health status updates don't update InstancesRevision."
    SetHealth {
        instance: &'static str,
        status: &'static str,
    },
}

/// The revision advances once per change to the instance set, and a read of any shape
/// — including `DiscoverInstancesRevision` itself — leaves it untouched.
#[tokio::test]
async fn the_revision_advances_once_per_change_to_the_instance_set() {
    let (router, engine) = emulator_with_engine();
    let (operation_id, namespace_id) = namespace(&router, "revision-ns").await;
    let service_id = service(&router, &namespace_id, "revision-svc").await;

    let start = revision(&router, "revision-ns", "revision-svc").await;
    assert_eq!(
        start, 0,
        "case fresh service: a service that holds no instance starts at revision 0, got {start}"
    );

    let discover = json!({ "NamespaceName": "revision-ns", "ServiceName": "revision-svc" });
    // Each row differs in the KIND of call made (register, deregister, a read of a
    // different resource, a stored health change) and in the instance it names.
    let steps: Vec<(Step, i64)> = vec![
        (
            Step::Read {
                operation: "DiscoverInstancesRevision",
                body: discover.clone(),
            },
            0,
        ),
        (
            Step::Register {
                instance: "i-1",
                attributes: json!({ "AWS_INSTANCE_IPV4": "10.0.0.1" }),
            },
            1,
        ),
        (
            Step::Register {
                instance: "i-2",
                attributes: json!({ "AWS_INSTANCE_IPV4": "10.0.0.2", "stage": "prod" }),
            },
            1,
        ),
        // Re-registering an id replaces that instance, which is still a change to the
        // set the response list is built from.
        (
            Step::Register {
                instance: "i-1",
                attributes: json!({ "AWS_INSTANCE_IPV4": "10.0.0.3" }),
            },
            1,
        ),
        (
            Step::Read {
                operation: "DiscoverInstances",
                body: discover.clone(),
            },
            0,
        ),
        (
            Step::Read {
                operation: "GetInstance",
                body: json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
            },
            0,
        ),
        (
            Step::Read {
                operation: "GetService",
                body: json!({ "Id": service_id }),
            },
            0,
        ),
        (
            Step::Read {
                operation: "GetNamespace",
                body: json!({ "Id": namespace_id }),
            },
            0,
        ),
        (
            Step::Read {
                operation: "GetOperation",
                body: json!({ "OperationId": operation_id }),
            },
            0,
        ),
        (
            Step::SetHealth {
                instance: "i-1",
                status: cloudmap::state::HEALTH_STATUS_HEALTHY,
            },
            0,
        ),
        (
            Step::SetHealth {
                instance: "i-2",
                status: cloudmap::state::HEALTH_STATUS_UNHEALTHY,
            },
            0,
        ),
        (Step::Deregister { instance: "i-2" }, 1),
        (Step::Deregister { instance: "i-1" }, 1),
    ];

    let mut expected = start;
    for (index, (step, delta)) in steps.into_iter().enumerate() {
        let case = format!("{index} {step:?}");
        match step {
            Step::Register {
                instance,
                attributes,
            } => {
                ok(
                    &router,
                    "RegisterInstance",
                    json!({
                        "ServiceId": service_id,
                        "InstanceId": instance,
                        "Attributes": attributes,
                    }),
                )
                .await;
            }
            Step::Deregister { instance } => {
                ok(
                    &router,
                    "DeregisterInstance",
                    json!({ "ServiceId": service_id, "InstanceId": instance }),
                )
                .await;
            }
            Step::Read { operation, body } => {
                ok(&router, operation, body).await;
            }
            Step::SetHealth { instance, status } => {
                let state = engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
                let mut store = state.lock();
                let stored = store
                    .services
                    .get_mut(&service_id)
                    .expect("the service this test created");
                stored
                    .instances
                    .get_mut(instance)
                    .expect("an instance this test registered")
                    .health_status = status.to_string();
            }
        }

        expected += delta;
        let actual = revision(&router, "revision-ns", "revision-svc").await;
        assert_eq!(
            actual, expected,
            "case {case}: expected revision {expected}, got {actual}"
        );

        // The same number both discovery calls report, since both read the one record.
        let found = ok(&router, "DiscoverInstances", discover.clone()).await;
        assert_eq!(
            found["InstancesRevision"].as_i64(),
            Some(actual),
            "case {case}: DiscoverInstances must report the same revision as \
             DiscoverInstancesRevision ({actual}), got {found}"
        );
    }
}

/// Every service carries its OWN revision, counting only the changes to its own
/// instance set — including two services of the same name in different namespaces.
#[tokio::test]
async fn each_service_carries_its_own_revision() {
    let router = emulator();
    let (_, namespace_a) = namespace(&router, "rev-ns-a").await;
    let (_, namespace_b) = namespace(&router, "rev-ns-b").await;

    // (namespace name, namespace id, service name, registers, deregisters)
    let cases: Vec<(&str, &str, &str, usize, usize)> = vec![
        ("rev-ns-a", namespace_a.as_str(), "svc-untouched", 0, 0),
        ("rev-ns-a", namespace_a.as_str(), "svc-one", 1, 0),
        ("rev-ns-a", namespace_a.as_str(), "svc-churn", 3, 2),
        // The same service NAME in another namespace is another service entirely.
        ("rev-ns-b", namespace_b.as_str(), "svc-one", 2, 0),
        ("rev-ns-b", namespace_b.as_str(), "svc-emptied", 5, 5),
    ];

    let mut service_ids = Vec::new();
    for (_, namespace_id, service_name, registers, deregisters) in &cases {
        let service_id = service(&router, namespace_id, service_name).await;
        for index in 0..*registers {
            ok(
                &router,
                "RegisterInstance",
                json!({
                    "ServiceId": service_id,
                    "InstanceId": format!("i-{index}"),
                    "Attributes": { "AWS_INSTANCE_IPV4": format!("10.0.0.{}", index + 1) },
                }),
            )
            .await;
        }
        for index in 0..*deregisters {
            ok(
                &router,
                "DeregisterInstance",
                json!({ "ServiceId": service_id, "InstanceId": format!("i-{index}") }),
            )
            .await;
        }
        service_ids.push(service_id);
    }

    for (namespace_name, _, service_name, registers, deregisters) in &cases {
        let expected = i64::try_from(registers + deregisters).expect("a small count");
        let actual = revision(&router, namespace_name, service_name).await;
        assert_eq!(
            actual, expected,
            "case {namespace_name}/{service_name} ({registers} registers, \
             {deregisters} deregisters): expected revision {expected}, got {actual}"
        );
    }

    // `OwnerAccount` (AWSAccountId, len 12..12, per the model) names the owner of a
    // SHARED namespace. This emulator holds one account's resources — every namespace
    // reports no `ResourceOwner` — so naming that account resolves the same record.
    let owned = ok(
        &router,
        "DiscoverInstancesRevision",
        json!({
            "NamespaceName": "rev-ns-a",
            "ServiceName": "svc-churn",
            "OwnerAccount": engine::EMULATOR_ACCOUNT_ID,
        }),
    )
    .await;
    assert_eq!(
        owned["InstancesRevision"].as_i64(),
        Some(5),
        "case OwnerAccount of this emulator's account: expected revision 5, got {owned}"
    );
}

/// A name that resolves to no stored record is refused with that name's modelled
/// not-found error, never with a revision for a service the caller did not name.
#[tokio::test]
async fn an_unresolvable_namespace_or_service_name_is_refused() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "missing-ns").await;
    let service_id = service(&router, &namespace_id, "missing-svc").await;
    // A service of this name exists, but in ANOTHER namespace.
    let (_, other_namespace_id) = namespace(&router, "other-ns").await;
    service(&router, &other_namespace_id, "elsewhere-svc").await;

    // (case, request, expected status, expected wire error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown namespace name",
            json!({ "NamespaceName": "no-such-ns", "ServiceName": "missing-svc" }),
            400,
            "NamespaceNotFound",
        ),
        (
            "an unknown service name in a known namespace",
            json!({ "NamespaceName": "missing-ns", "ServiceName": "no-such-svc" }),
            400,
            "ServiceNotFound",
        ),
        (
            // `NamespaceName` is documented as the namespace's HttpName, so its id is
            // not a name this operation resolves.
            "the namespace id instead of its name",
            json!({ "NamespaceName": namespace_id, "ServiceName": "missing-svc" }),
            400,
            "NamespaceNotFound",
        ),
        (
            "the service id instead of its name",
            json!({ "NamespaceName": "missing-ns", "ServiceName": service_id }),
            400,
            "ServiceNotFound",
        ),
        (
            // A namespace exists under this name, but the service belongs to another.
            "a service name from a different namespace",
            json!({ "NamespaceName": "missing-ns", "ServiceName": "elsewhere-svc" }),
            400,
            "ServiceNotFound",
        ),
    ];

    for (case, body, expected_status, expected_code) in cases {
        let (status, code) = failure(&router, "DiscoverInstancesRevision", body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case}: expected ({expected_status}, {expected_code}), got ({status}, {code})"
        );
    }
}
