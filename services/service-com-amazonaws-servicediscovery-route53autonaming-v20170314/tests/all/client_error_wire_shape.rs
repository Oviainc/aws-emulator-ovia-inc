// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wire shape of this service's CLIENT-class refusals.
//!
//! Two rules are under test, over tables that span the error classes, the operations
//! and the reference forms each rule covers:
//!
//! 1. Every client-class refusal answers HTTP 400 and carries the class only in the
//!    body's error code, whatever status the model's `@httpError` pins on the shape
//!    (404 on the not-found shapes, 409 on `ResourceInUse`/`DuplicateRequest`, 400 on
//!    the rest). A caller keyed on the status sees one status for the whole class.
//! 2. A refusal's `Message` carries only what the request named — an identifier, that
//!    identifier as an ARN, or nothing at all — or the one fixed sentence the service
//!    words a conflict with. No refusal composes prose of its own around the value.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, the same path the
//! emulator serves in production, reusing the harness in `cloud_map_core.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cloud_map_core::{call, emulator, namespace, ok, service, REGION};

/// What a row expects of the refusal's `Message`.
#[derive(Debug, Clone)]
enum Message {
    /// The member is absent: this refusal reports its class and echoes nothing.
    Absent,
    /// Exactly this value, and nothing around it.
    Exactly(String),
    /// This row pins the status and the class only.
    Any,
}

/// The sentence a service that still holds instances is refused with. Fixed text: it
/// names neither the service nor how many instances it holds.
const SERVICE_CONTAINS_INSTANCES: &str =
    "Service contains registered instances; delete the instances before deleting the service";

/// The ARN form of a service reference, which is what the refusals that echo a service
/// carry however the request named it.
fn service_arn_of(reference: &str) -> String {
    if reference.starts_with("arn:") {
        return reference.to_string();
    }
    format!(
        "arn:aws:servicediscovery:{REGION}:{}:service/{reference}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The error code and `Message` of one refused call, plus whether a `Message` member
/// was present at all.
async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Option<String>) {
    let (status, json) = call(router, operation, body).await;
    // The awsJson error envelope names the class in `__type`, qualified by namespace.
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = json
        .get("Message")
        .or_else(|| json.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, message)
}

/// Assert one row: the status is 400, the class is the expected one, and the `Message`
/// follows that operation's rule.
async fn assert_refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    expected_code: &str,
    expected_message: &Message,
) {
    let (status, code, message) = refusal(router, operation, body.clone()).await;
    assert_eq!(
        (status, code.as_str()),
        (400, expected_code),
        "case {case:?} on {operation} with {body}: expected 400 {expected_code}, got \
         {status} {code} {message:?}"
    );
    match expected_message {
        Message::Absent => assert!(
            message.is_none(),
            "case {case:?} on {operation}: this refusal carries no Message, got {message:?}"
        ),
        Message::Exactly(expected) => assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?} on {operation}: expected the Message {expected:?}, got {message:?}"
        ),
        Message::Any => {}
    }
}

/// One namespace holding one service with one instance, plus a second service holding
/// three: enough state for every conflict and not-found row below.
async fn seeded(router: &MultiServiceRouter) -> (String, String, String) {
    let (_, namespace_id) = namespace(router, "errors-ns").await;
    let service_id = service(router, &namespace_id, "errors-svc").await;
    register(router, &service_id, 1).await;
    let busy_id = service(router, &namespace_id, "busy-svc").await;
    register(router, &busy_id, 3).await;
    (namespace_id, service_id, busy_id)
}

/// Register `count` instances, named `i-0`..`i-<count-1>`, in `service_id`.
async fn register(router: &MultiServiceRouter, service_id: &str, count: usize) {
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

/// Every client-class refusal this service serves answers HTTP 400, whatever status the
/// model pins on its shape: the table spans the shapes carrying `@httpError` 404 (the
/// four not-found classes), 409 (`ResourceInUse`, `DuplicateRequest`) and 400
/// (`InvalidInput`, `ServiceAlreadyExists`), reached through ten different operations,
/// plus a boundary refusal the framework answers before any handler runs.
#[tokio::test]
async fn every_client_class_refusal_answers_four_hundred() {
    let router = emulator();
    let (namespace_id, service_id, _) = seeded(&router).await;

    // (case, operation, request, expected class, expected Message rule)
    let cases: Vec<(&str, &str, Value, &str, Message)> = vec![
        (
            "a service id nothing created (modelled 404)",
            "GetService",
            json!({ "Id": "srv-nosuchservice000" }),
            "ServiceNotFound",
            Message::Absent,
        ),
        (
            "an instance nothing registered (modelled 404)",
            "GetInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-never" }),
            "InstanceNotFound",
            Message::Exactly("i-never".to_string()),
        ),
        (
            "a namespace id nothing created (modelled 404)",
            "GetNamespace",
            json!({ "Id": "ns-nosuchnamespace" }),
            "NamespaceNotFound",
            Message::Any,
        ),
        (
            "an operation id nothing filed (modelled 404)",
            "GetOperation",
            json!({ "OperationId": "nosuchoperation" }),
            "OperationNotFound",
            Message::Any,
        ),
        (
            "a resource ARN nothing minted (modelled 404)",
            "ListTagsForResource",
            json!({
                "ResourceARN": format!(
                    "arn:aws:servicediscovery:{REGION}:{}:namespace/ns-nothinghere",
                    engine::EMULATOR_ACCOUNT_ID
                ),
            }),
            "ResourceNotFoundException",
            Message::Any,
        ),
        (
            "a service that still holds instances (modelled 409)",
            "DeleteService",
            json!({ "Id": service_id }),
            "ResourceInUse",
            Message::Exactly(SERVICE_CONTAINS_INSTANCES.to_string()),
        ),
        (
            "a namespace that still holds services (modelled 409)",
            "DeleteNamespace",
            json!({ "Id": namespace_id }),
            "ResourceInUse",
            Message::Any,
        ),
        (
            "a namespace name already created (modelled 409)",
            "CreateHttpNamespace",
            json!({ "Name": "errors-ns" }),
            "DuplicateRequest",
            Message::Any,
        ),
        (
            "a service name already taken in the namespace (modelled 400)",
            "CreateService",
            json!({ "Name": "errors-svc", "NamespaceId": namespace_id }),
            "ServiceAlreadyExists",
            Message::Any,
        ),
        (
            "a create naming no namespace at all (modelled 400)",
            "CreateService",
            json!({ "Name": "orphan-svc" }),
            "InvalidInput",
            Message::Any,
        ),
        (
            // `MaxResults` is modelled `range 1..100`, so this is refused at the
            // boundary, before the handler runs.
            "a MaxResults under the modelled minimum (a boundary refusal)",
            "ListInstances",
            json!({ "ServiceId": service_id, "MaxResults": 0 }),
            "InvalidInput",
            Message::Any,
        ),
    ];

    for (case, operation, body, expected_code, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            operation,
            body,
            expected_code,
            &expected_message,
        )
        .await;
    }
}

/// A service reference that resolves to nothing is refused on every operation that
/// takes one, and each refusal's `Message` carries only what the request named, in that
/// operation's form: nothing at all, the reference as given, or the reference as an ARN
/// — the ARN rows proving the form does not depend on how the caller named the service.
#[tokio::test]
async fn an_unresolvable_service_reference_echoes_only_what_the_request_named() {
    let router = emulator();
    seeded(&router).await;

    // The forms a member modelled `Arn` accepts ("the ID or Amazon Resource Name
    // (ARN)"), plus the two references that resolve to no service at all: `Arn` is
    // `length 0..255`, so the empty string reaches the handler.
    let references: Vec<(&str, String)> = vec![
        ("an id no service carries", "srv-nosuchservice000".to_string()),
        (
            "an ARN no service carries",
            service_arn_of("srv-0000000000000001"),
        ),
        ("the empty reference", String::new()),
        (
            "a namespace id in the service slot",
            "ns-nosuchnamespace".to_string(),
        ),
    ];

    for (case, reference) in references {
        let arn = service_arn_of(&reference);
        // (operation, request, expected class, expected Message rule)
        let cases: Vec<(&str, Value, &str, Message)> = vec![
            (
                "GetService",
                json!({ "Id": reference }),
                "ServiceNotFound",
                Message::Absent,
            ),
            (
                "GetServiceAttributes",
                json!({ "ServiceId": reference }),
                "ServiceNotFound",
                Message::Absent,
            ),
            (
                "UpdateServiceAttributes",
                json!({ "ServiceId": reference, "Attributes": { "stage": "prod" } }),
                "ServiceNotFound",
                Message::Absent,
            ),
            (
                "DeleteServiceAttributes",
                json!({ "ServiceId": reference, "Attributes": ["stage"] }),
                "ServiceNotFound",
                Message::Absent,
            ),
            (
                "DeleteService",
                json!({ "Id": reference }),
                "ServiceNotFound",
                Message::Exactly(reference.clone()),
            ),
            (
                "UpdateService",
                json!({
                    "Id": reference,
                    "Service": { "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 60 }] } },
                }),
                "ServiceNotFound",
                Message::Exactly(arn.clone()),
            ),
            (
                "ListInstances",
                json!({ "ServiceId": reference }),
                "ServiceNotFound",
                Message::Exactly(arn.clone()),
            ),
            (
                "RegisterInstance",
                json!({
                    "ServiceId": reference,
                    "InstanceId": "i-9",
                    "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.9" },
                }),
                "ServiceNotFound",
                Message::Exactly(arn.clone()),
            ),
            (
                "DeregisterInstance",
                json!({ "ServiceId": reference, "InstanceId": "i-9" }),
                "ServiceNotFound",
                Message::Exactly(arn.clone()),
            ),
            (
                // This read resolves the (service, instance) pair in one lookup, so a
                // service it cannot find has no instance to report either.
                "GetInstance",
                json!({ "ServiceId": reference, "InstanceId": "i-9" }),
                "InstanceNotFound",
                Message::Exactly("i-9".to_string()),
            ),
        ];
        for (operation, body, expected_code, expected_message) in cases {
            assert_refusal(
                &router,
                case,
                operation,
                body,
                expected_code,
                &expected_message,
            )
            .await;
        }
    }
}

/// An instance the service never registered is refused with the instance id the request
/// named and nothing else — whichever form named the service, and whatever the id looks
/// like (`ResourceId` is `length 0..64` with no pattern, so the empty id and a long one
/// both reach the handler).
#[tokio::test]
async fn an_unregistered_instance_echoes_only_the_requested_instance_id() {
    let router = emulator();
    let (_, service_id, _) = seeded(&router).await;
    let service_arn = ok(&router, "GetService", json!({ "Id": service_id }))
        .await["Service"]["Arn"]
        .as_str()
        .expect("GetService reports the service's Arn")
        .to_string();

    let instance_ids: Vec<(&str, String)> = vec![
        ("a plain id", "i-never".to_string()),
        ("the empty id", String::new()),
        ("an id at the modelled 64-character maximum", "i".repeat(64)),
        (
            "an id carrying the punctuation the registry allows",
            "host.example_1/9:0@x-y".to_string(),
        ),
    ];

    for (named, reference) in [("by id", &service_id), ("by ARN", &service_arn)] {
        for (case, instance_id) in &instance_ids {
            for operation in ["GetInstance", "DeregisterInstance"] {
                assert_refusal(
                    &router,
                    &format!("{case} named {named}"),
                    operation,
                    json!({ "ServiceId": reference, "InstanceId": instance_id }),
                    "InstanceNotFound",
                    &Message::Exactly(instance_id.clone()),
                )
                .await;
            }
        }
    }
}

/// An instance-status read reports whether instances have a status, so a request it
/// cannot resolve to any instance is an instance miss carrying this operation's own
/// template: the fixed text, the service as its single-quoted ARN, and the scope the
/// request asked about — the ids it listed, or the whole service when it listed none.
#[tokio::test]
async fn an_instance_status_read_reports_an_instance_miss_with_the_scope_it_was_asked() {
    let router = emulator();
    let (_, service_id, _) = seeded(&router).await;
    let service_arn = service_arn_of(&service_id);
    let unknown = "srv-nosuchservice000";

    // (case, request, the service the message names, the scope it reports)
    let cases: Vec<(&str, Value, String, &str)> = vec![
        (
            "a service nothing created, no Instances named",
            json!({ "ServiceId": unknown }),
            service_arn_of(unknown),
            "All instances",
        ),
        (
            "a service nothing created, named by its would-be ARN",
            json!({ "ServiceId": service_arn_of(unknown) }),
            service_arn_of(unknown),
            "All instances",
        ),
        (
            "a service nothing created, one instance named",
            json!({ "ServiceId": unknown, "Instances": ["i-1"] }),
            service_arn_of(unknown),
            "i-1",
        ),
        (
            "a service nothing created, two instances named",
            json!({ "ServiceId": unknown, "Instances": ["i-1", "i-2"] }),
            service_arn_of(unknown),
            "i-1, i-2",
        ),
        (
            "a stored service, one instance it never registered",
            json!({ "ServiceId": service_id, "Instances": ["i-never"] }),
            service_arn.clone(),
            "i-never",
        ),
        (
            "a stored service named by ARN, one known and one unknown instance",
            json!({ "ServiceId": service_arn.clone(), "Instances": ["i-0", "i-never"] }),
            service_arn.clone(),
            "i-0, i-never",
        ),
    ];

    for (case, body, named_service, scope) in cases {
        assert_refusal(
            &router,
            case,
            "GetInstancesHealthStatus",
            body,
            "InstanceNotFound",
            &Message::Exactly(format!(
                "Instance(s) not found for service '{named_service}' when trying to retreive: \
                 {scope}"
            )),
        )
        .await;
    }
}

/// The conflict a service still holding instances is refused with is ONE fixed
/// sentence: it varies with neither the instance count nor the service, and the refusal
/// removes nothing. The row that empties the service proves the same request then
/// succeeds.
#[tokio::test]
async fn the_in_use_conflict_is_one_fixed_sentence_whatever_the_service_holds() {
    for count in [1_usize, 2, 7] {
        let router = emulator();
        let (_, namespace_id) = namespace(&router, "in-use-ns").await;
        let service_id = service(&router, &namespace_id, "in-use-svc").await;
        register(&router, &service_id, count).await;

        assert_refusal(
            &router,
            &format!("{count} registered instance(s)"),
            "DeleteService",
            json!({ "Id": service_id }),
            "ResourceInUse",
            &Message::Exactly(SERVICE_CONTAINS_INSTANCES.to_string()),
        )
        .await;

        // Nothing was removed by the refused delete.
        let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
        assert_eq!(
            read["Service"]["InstanceCount"],
            json!(count),
            "case {count} instance(s): a refused delete keeps every instance, got {read}"
        );

        for index in 0..count {
            ok(
                &router,
                "DeregisterInstance",
                json!({ "ServiceId": service_id, "InstanceId": format!("i-{index}") }),
            )
            .await;
        }
        ok(&router, "DeleteService", json!({ "Id": service_id })).await;
    }
}
