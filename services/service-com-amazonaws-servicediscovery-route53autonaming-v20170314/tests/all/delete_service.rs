// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteService`.
//!
//! The rule under test: deleting a service removes the one stored record the request's
//! id-or-ARN resolves to, and only when that service holds no registered instance —
//! otherwise the modelled `ResourceInUse` is answered and nothing is removed; a
//! reference that resolves to no stored service answers `ServiceNotFound`.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, the same path the
//! emulator serves in production, reusing the harness in `cloud_map_core.rs`.

use serde_json::{json, Value};

use super::cloud_map_core::{discovered, emulator, failure, namespace, ok, service};

/// How a deleted service may be named: the two forms a member modelled as `Arn`
/// accepts, per `DeleteServiceRequest$Id` ("The ID or Amazon Resource Name (ARN) of the
/// service that you want to delete").
#[derive(Debug, Clone, Copy)]
enum Reference {
    Id,
    Arn,
}

/// The service's `Id` or `Arn`, read back from the stored record.
async fn reference_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    service_id: &str,
    form: Reference,
) -> String {
    let read = ok(router, "GetService", json!({ "Id": service_id })).await;
    let member = match form {
        Reference::Id => "Id",
        Reference::Arn => "Arn",
    };
    read["Service"][member]
        .as_str()
        .unwrap_or_else(|| panic!("GetService reports the service's {member}, got {read}"))
        .to_string()
}

/// Register `count` instances in `service_id`.
async fn register(
    router: &smithy_server_runtime::MultiServiceRouter,
    service_id: &str,
    count: usize,
) {
    for index in 0..count {
        ok(
            router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": format!("i-{index}"),
                "Attributes": { "AWS_INSTANCE_IPV4": format!("10.0.0.{}", index + 1) },
            }),
        )
        .await;
    }
}

/// An empty-but-present response body, which is what a `DeleteServiceResponse` with no
/// members serialises to.
fn assert_no_output(operation: &str, output: &Value) {
    let empty = output.is_null() || output.as_object().is_some_and(serde_json::Map::is_empty);
    assert!(
        empty,
        "{operation} models no output members, so it must answer with nothing, got {output}"
    );
}

/// A service holding no instance is deleted whichever of its two references names it,
/// and the deletion is visible to every later reader.
#[tokio::test]
async fn an_empty_service_is_deleted_by_id_or_by_arn() {
    for (case, form) in [("by id", Reference::Id), ("by arn", Reference::Arn)] {
        let router = emulator();
        let (_, namespace_id) = namespace(&router, "delete-ns").await;
        let service_id = service(&router, &namespace_id, "delete-svc").await;
        let other_id = service(&router, &namespace_id, "keeper-svc").await;
        let reference = reference_of(&router, &service_id, form).await;

        let deleted = ok(&router, "DeleteService", json!({ "Id": reference })).await;
        assert_no_output("DeleteService", &deleted);

        let (status, code) = failure(&router, "GetService", json!({ "Id": service_id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ServiceNotFound"),
            "case {case:?}: a deleted service is gone from the store"
        );

        // The namespace's derived ServiceCount follows the records that remain, and the
        // service that was not named is untouched.
        let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
        assert_eq!(
            read["Namespace"]["ServiceCount"],
            json!(1),
            "case {case:?}: deleting one of two services leaves the other, got {read}"
        );
        let read = ok(&router, "GetService", json!({ "Id": other_id })).await;
        assert_eq!(
            read["Service"]["Name"],
            json!("keeper-svc"),
            "case {case:?}: only the named service is deleted, got {read}"
        );

        // The (namespace, name) index entry went with the record: the name is free, and
        // the re-created service is a NEW resource, not the deleted one.
        let recreated = service(&router, &namespace_id, "delete-svc").await;
        assert_ne!(
            recreated, service_id,
            "case {case:?}: re-creating a deleted service mints a fresh id"
        );
        let read = ok(&router, "GetService", json!({ "Id": recreated })).await;
        assert_eq!(
            read["Service"]["InstanceCount"],
            json!(0),
            "case {case:?}: the re-created service starts empty, got {read}"
        );
    }
}

/// A service that still holds registered instances is refused, and the refusal changes
/// nothing: the service, its instances and their discoverability all survive. Once every
/// instance is deregistered the same request succeeds.
#[tokio::test]
async fn a_service_holding_instances_is_refused_until_they_are_deregistered() {
    for count in [1_usize, 2, 7] {
        let router = emulator();
        let (_, namespace_id) = namespace(&router, "busy-ns").await;
        let service_id = service(&router, &namespace_id, "busy-svc").await;
        register(&router, &service_id, count).await;

        let (status, code) = failure(&router, "DeleteService", json!({ "Id": service_id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceInUse"),
            "case {count} instance(s): a service with instances cannot be deleted"
        );

        // Nothing was removed by the refused delete.
        let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
        assert_eq!(
            read["Service"]["InstanceCount"],
            json!(count),
            "case {count} instance(s): a refused delete keeps every instance, got {read}"
        );
        let found = discovered(
            &router,
            json!({ "NamespaceName": "busy-ns", "ServiceName": "busy-svc" }),
        )
        .await;
        assert_eq!(
            found.len(),
            count,
            "case {count} instance(s): a refused delete leaves the service discoverable, \
             got {found:?}"
        );

        // Deregistering all but one is still not enough.
        for index in 0..count - 1 {
            ok(
                &router,
                "DeregisterInstance",
                json!({ "ServiceId": service_id, "InstanceId": format!("i-{index}") }),
            )
            .await;
            let (status, code) =
                failure(&router, "DeleteService", json!({ "Id": service_id })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceInUse"),
                "case {count} instance(s): still refused with {} instance(s) left",
                count - index - 1
            );
        }

        ok(
            &router,
            "DeregisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": format!("i-{}", count - 1) }),
        )
        .await;
        let deleted = ok(&router, "DeleteService", json!({ "Id": service_id })).await;
        assert_no_output("DeleteService", &deleted);
        let (status, code) = failure(&router, "GetService", json!({ "Id": service_id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ServiceNotFound"),
            "case {count} instance(s): an emptied service is deletable"
        );
    }
}

/// Every reference that resolves to no stored service answers the modelled
/// `ServiceNotFound`, whatever form it takes.
#[tokio::test]
async fn a_reference_naming_no_service_answers_service_not_found() {
    let router = emulator();
    let (operation_id, namespace_id) = namespace(&router, "unknown-ns").await;
    let service_id = service(&router, &namespace_id, "unknown-svc").await;
    let service_arn = reference_of(&router, &service_id, Reference::Arn).await;
    let namespace_arn = ok(&router, "GetNamespace", json!({ "Id": namespace_id }))
        .await["Namespace"]["Arn"]
        .as_str()
        .expect("GetNamespace reports the namespace's Arn")
        .to_string();
    ok(&router, "DeleteService", json!({ "Id": service_id })).await;

    // `Id` is `len 0..255`, so the empty string reaches the handler rather than being
    // refused by a modelled constraint.
    let cases: Vec<(&str, String)> = vec![
        ("the id of a service already deleted", service_id.clone()),
        ("the ARN of a service already deleted", service_arn),
        ("an id no service ever had", "srv-000000000000zzzz".to_string()),
        (
            "an ARN no service ever had",
            format!(
                "arn:aws:servicediscovery:us-west-2:{}:service/srv-0000000000001111",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("a namespace id", namespace_id.clone()),
        ("a namespace ARN", namespace_arn),
        ("an operation id", operation_id),
        ("the empty string", String::new()),
    ];
    for (case, reference) in cases {
        let (status, code) = failure(&router, "DeleteService", json!({ "Id": reference })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ServiceNotFound"),
            "case {case:?} ({reference:?}): no service exists with that reference"
        );
    }
}

/// Once a service is deleted, every call that would read or mutate it is rejected for the
/// resource that no longer exists — including discovery, which resolves it by name.
#[tokio::test]
async fn mutating_a_deleted_service_is_rejected() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "gone-ns").await;
    let service_id = service(&router, &namespace_id, "gone-svc").await;
    register(&router, &service_id, 1).await;
    ok(
        &router,
        "DeregisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-0" }),
    )
    .await;
    ok(&router, "DeleteService", json!({ "Id": service_id })).await;

    // Every call is refused for the resource that is no longer there. `GetInstance`
    // resolves the (service, instance) pair in one lookup, so the pair it cannot find
    // is an instance miss rather than a service miss.
    let rejected: Vec<(&str, Value, &str)> = vec![
        ("GetService", json!({ "Id": service_id }), "ServiceNotFound"),
        (
            "DeleteService",
            json!({ "Id": service_id }),
            "ServiceNotFound",
        ),
        (
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": "i-0",
                "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" },
            }),
            "ServiceNotFound",
        ),
        (
            "DeregisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-0" }),
            "ServiceNotFound",
        ),
        (
            "GetInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-0" }),
            "InstanceNotFound",
        ),
        (
            "DiscoverInstances",
            json!({ "NamespaceName": "gone-ns", "ServiceName": "gone-svc" }),
            "ServiceNotFound",
        ),
    ];
    for (operation, body, expected_code) in rejected {
        let (status, code) = failure(&router, operation, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {operation:?}: a deleted service cannot be read or mutated"
        );
    }
}
