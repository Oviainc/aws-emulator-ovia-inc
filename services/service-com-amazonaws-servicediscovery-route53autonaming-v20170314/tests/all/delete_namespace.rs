// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteNamespace` over the wire.
//!
//! The rule under test: `DeleteNamespace` removes the namespace its `Id` names — an id
//! or an ARN, since the member is modelled as `Arn` — files a `DELETE_NAMESPACE`
//! operation record targeting it, and refuses the delete with `ResourceInUse` while the
//! namespace still holds one or more services. A namespace no record exists for is
//! `NamespaceNotFound`, except while a delete already filed for it is still owed its
//! terminal status, which is `DuplicateRequest`.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, the same path the emulator
//! serves in production; the harness lives in `cloud_map_core`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{call, emulator, emulator_with_engine, failure, namespace, ok, service};

/// Which form of the `Arn`-modelled `Id` member the delete names the namespace by.
#[derive(Debug, Clone, Copy)]
enum Reference {
    Id,
    Arn,
}

/// Create an HTTP namespace from `body` and return `(id, arn)`, read back through
/// `GetOperation` and `GetNamespace` so both come from stored state.
async fn created_namespace(router: &MultiServiceRouter, body: Value) -> (String, String) {
    let created = ok(router, "CreateHttpNamespace", body).await;
    let operation_id = created["OperationId"]
        .as_str()
        .expect("CreateHttpNamespace returns an OperationId")
        .to_string();
    let operation = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    let id = operation["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string();
    let read = ok(router, "GetNamespace", json!({ "Id": id })).await;
    let arn = read["Namespace"]["Arn"]
        .as_str()
        .expect("a stored namespace carries its ARN")
        .to_string();
    (id, arn)
}

/// A delete removes the namespace it names, whichever form names it and whatever the
/// namespace carries, and files the operation record that says so.
#[tokio::test]
async fn a_delete_removes_the_namespace_it_names_and_files_its_operation() {
    let router = emulator();

    // Rows differ along the reference form and along what the namespace carries, the
    // two things the rule generalises over. `Tags` and `Description` are
    // `CreateHttpNamespaceRequest` members (smithy_query operation=CreateHttpNamespace).
    let cases: Vec<(&str, Reference, Option<&str>, Value)> = vec![
        ("a bare namespace named by id", Reference::Id, None, Value::Null),
        ("a bare namespace named by ARN", Reference::Arn, None, Value::Null),
        (
            "a described, tagged namespace named by id",
            Reference::Id,
            Some("the one with tags"),
            json!([{ "Key": "team", "Value": "platform" }, { "Key": "env", "Value": "test" }]),
        ),
        (
            "a described, tagged namespace named by ARN",
            Reference::Arn,
            Some("the other one with tags"),
            json!([{ "Key": "team", "Value": "payments" }]),
        ),
    ];

    for (index, (case, reference, description, tags)) in cases.into_iter().enumerate() {
        let name = format!("delete-ns-{index}");
        let mut body = json!({ "Name": name });
        if let Some(description) = description {
            body["Description"] = json!(description);
        }
        if !tags.is_null() {
            body["Tags"] = tags;
        }
        let (namespace_id, namespace_arn) = created_namespace(&router, body).await;

        // A second namespace that the delete must leave untouched.
        let bystander_name = format!("delete-ns-bystander-{index}");
        let (_, bystander_id) = namespace(&router, &bystander_name).await;

        let named = match reference {
            Reference::Id => namespace_id.clone(),
            Reference::Arn => namespace_arn.clone(),
        };
        let deleted = ok(&router, "DeleteNamespace", json!({ "Id": named })).await;
        let delete_operation = deleted["OperationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !delete_operation.is_empty(),
            "case {case:?}: DeleteNamespace must return an OperationId, got {deleted}"
        );

        let operation = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": delete_operation }),
        )
        .await;
        assert_eq!(
            operation["Operation"]["Type"],
            json!("DELETE_NAMESPACE"),
            "case {case:?}: the filed operation is the delete, got {operation}"
        );
        assert_eq!(
            operation["Operation"]["Targets"]["NAMESPACE"],
            json!(namespace_id),
            "case {case:?}: the operation targets the namespace that was deleted, \
             got {operation}"
        );
        assert_eq!(
            operation["Operation"]["Status"],
            json!("SUBMITTED"),
            "case {case:?}: a caller polling straight away still sees the non-terminal \
             status, got {operation}"
        );

        // The record is gone under BOTH the forms that used to reach it.
        for (form, named) in [("id", &namespace_id), ("ARN", &namespace_arn)] {
            let (status, code) = failure(&router, "GetNamespace", json!({ "Id": named })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NamespaceNotFound"),
                "case {case:?}: the deleted namespace must not read back by {form}"
            );
        }

        // The name index was freed with the record, so the name is creatable again and
        // the create is a create, not a conflict.
        let (recreated_id, _) = created_namespace(&router, json!({ "Name": name })).await;
        assert_ne!(
            recreated_id, namespace_id,
            "case {case:?}: re-creating the freed name stores a new namespace, got \
             {recreated_id}"
        );

        // Nothing else moved.
        let bystander = ok(&router, "GetNamespace", json!({ "Id": bystander_id })).await;
        assert_eq!(
            bystander["Namespace"]["Name"],
            json!(bystander_name),
            "case {case:?}: another namespace must survive this delete, got {bystander}"
        );
    }
}

/// "If the namespace still contains one or more services, the request fails" — refused
/// with `ResourceInUse`, and the refusal deletes nothing.
#[tokio::test]
async fn a_namespace_that_still_holds_services_is_refused_and_left_alone() {
    let (router, engine) = emulator_with_engine();
    let store =
        engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();

    // Rows differ along how many services the namespace holds: the rule is "one or
    // more", so one is the boundary and the larger counts are the same refusal.
    for service_count in [1_usize, 2, 5] {
        let namespace_name = format!("inuse-ns-{service_count}");
        let (_, namespace_id) = namespace(&router, &namespace_name).await;
        let mut service_ids = Vec::new();
        for index in 0..service_count {
            service_ids
                .push(service(&router, &namespace_id, &format!("inuse-svc-{index}")).await);
        }

        let (status, code) = failure(
            &router,
            "DeleteNamespace",
            json!({ "Id": namespace_id.clone() }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceInUse"),
            "case {service_count} service(s): the delete must be refused"
        );

        let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id.clone() })).await;
        assert_eq!(
            read["Namespace"]["ServiceCount"],
            json!(service_count),
            "case {service_count} service(s): the refused delete removed nothing, got {read}"
        );
        for service_id in &service_ids {
            let service = ok(&router, "GetService", json!({ "Id": service_id })).await;
            assert_eq!(
                service["Service"]["NamespaceId"],
                json!(namespace_id),
                "case {service_count} service(s): its services are still stored, got {service}"
            );
        }

        // Empty the namespace the only way this slice can: `DeleteService` is still a
        // scaffold stub (its own issue), so the services are dropped from the store
        // directly to reach the state where the delete is allowed.
        {
            let mut store = store.lock();
            store
                .services
                .retain(|_, service| service.namespace_id != namespace_id);
            store
                .service_ids_by_namespace_and_name
                .retain(|(owner, _), _| *owner != namespace_id);
        }

        let deleted = ok(
            &router,
            "DeleteNamespace",
            json!({ "Id": namespace_id.clone() }),
        )
        .await;
        assert!(
            deleted["OperationId"].is_string(),
            "case {service_count} service(s): an emptied namespace deletes, got {deleted}"
        );
        let (status, code) = failure(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NamespaceNotFound"),
            "case {service_count} service(s): the emptied namespace is gone"
        );
    }
}

/// A `CreatorRequestId` is only a replay while the namespace it created is still
/// stored: once the namespace has been deleted, the same token must create a new one
/// rather than report success for a create it never made.
#[tokio::test]
async fn a_create_token_is_not_replayed_once_its_namespace_is_deleted() {
    let router = emulator();
    let token = "11111111-2222-3333-4444-555555555555";
    let name = "token-ns";

    let first = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": name, "CreatorRequestId": token }),
    )
    .await;
    let first_operation = first["OperationId"].as_str().expect("an id").to_string();
    let replay = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": name, "CreatorRequestId": token }),
    )
    .await;
    assert_eq!(
        replay["OperationId"],
        json!(first_operation),
        "case \"token replayed while the namespace is stored\": the retry replays the \
         first create, got {replay}"
    );

    let namespace_id = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": first_operation }),
    )
    .await["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the create targets its namespace")
        .to_string();
    ok(
        &router,
        "DeleteNamespace",
        json!({ "Id": namespace_id.clone() }),
    )
    .await;

    let recreated = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": name, "CreatorRequestId": token }),
    )
    .await;
    let recreated_operation = recreated["OperationId"].as_str().expect("an id").to_string();
    assert_ne!(
        recreated_operation, first_operation,
        "case \"token replayed once the namespace is deleted\": a stale token must not \
         replay, got {recreated}"
    );
    let recreated_id = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": recreated_operation }),
    )
    .await["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the create targets its namespace")
        .to_string();
    assert_ne!(
        recreated_id, namespace_id,
        "case \"token replayed once the namespace is deleted\": the second create stores \
         a new namespace, got {recreated_id}"
    );
    let read = ok(&router, "GetNamespace", json!({ "Id": recreated_id })).await;
    assert_eq!(
        read["Namespace"]["Name"],
        json!(name),
        "case \"token replayed once the namespace is deleted\": the new namespace reads \
         back, got {read}"
    );
}

/// A namespace no record exists for is `NamespaceNotFound` — except while the delete
/// that removed it is still in progress, which is `DuplicateRequest` naming that
/// operation.
#[tokio::test]
async fn a_delete_in_progress_is_a_duplicate_and_a_settled_one_is_not_found() {
    let router = emulator();

    for case in ["nothing was ever created under this id", "an ARN nothing minted"] {
        let named = if case.contains("ARN") {
            // The same helper the handlers mint ARNs with, so the test names an ARN of
            // exactly the shape this service stores.
            cloudmap::state::service_arn("us-west-2", "namespace/ns-nosuchnamespace")
        } else {
            "ns-nosuchnamespace".to_string()
        };
        let (status, code) = failure(&router, "DeleteNamespace", json!({ "Id": named })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NamespaceNotFound"),
            "case {case:?}: an unknown namespace is refused as not found"
        );
    }

    let (_, namespace_id) = namespace(&router, "duplicate-ns").await;
    let started = Instant::now();
    let deleted = ok(
        &router,
        "DeleteNamespace",
        json!({ "Id": namespace_id.clone() }),
    )
    .await;
    let delete_operation = deleted["OperationId"]
        .as_str()
        .expect("an OperationId")
        .to_string();

    let (status, retry) = call(
        &router,
        "DeleteNamespace",
        json!({ "Id": namespace_id.clone() }),
    )
    .await;
    let code = retry["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    if started.elapsed() < cloudmap::state::OPERATION_SUCCESS_SPAN {
        // The first delete is still owed its terminal status, so the retry is the
        // duplicate — and it names the operation already running.
        assert_eq!(
            (status, code.as_str()),
            (400, "DuplicateRequest"),
            "a retry inside the operation's span is a duplicate, got {retry}"
        );
        assert_eq!(
            retry["DuplicateOperationId"],
            json!(delete_operation),
            "DuplicateRequest names the delete already in progress, got {retry}"
        );
    } else {
        assert_eq!(
            (status, code.as_str()),
            (400, "NamespaceNotFound"),
            "past the span the retry is a plain not-found, got {retry}"
        );
    }

    // Blocking is fine here: the test owns the runtime, and the service never sleeps for
    // this span — it stores the due instant and materialises it on read.
    std::thread::sleep(cloudmap::state::OPERATION_SUCCESS_SPAN + Duration::from_millis(50));

    let settled = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": delete_operation }),
    )
    .await;
    assert_eq!(
        settled["Operation"]["Status"],
        json!("SUCCESS"),
        "the clock owed the delete its terminal status, got {settled}"
    );
    let (status, code) = failure(&router, "DeleteNamespace", json!({ "Id": namespace_id })).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NamespaceNotFound"),
        "once the delete has settled there is nothing in progress to duplicate"
    );
}
