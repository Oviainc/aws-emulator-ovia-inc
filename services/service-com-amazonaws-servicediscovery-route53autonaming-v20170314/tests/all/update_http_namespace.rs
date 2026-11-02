// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateHttpNamespace`: the description a call carries is the
//! description the namespace holds afterwards, the call files an `UPDATE_NAMESPACE`
//! operation `GetOperation` resolves, a retried `UpdaterRequestId` files one operation,
//! and a namespace that is not an HTTP namespace is refused.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` "Wire protocol".
const TARGET_PREFIX: &str = "Route53AutoNaming_v20170314";

/// The region the test requests are signed for.
const REGION: &str = "us-west-2";

/// `ResourceDescription`'s published maximum, from
/// `smithy_query(query="shape", shape="HttpNamespaceChange")`: `Description` targets
/// `ResourceDescription`, `length {min: 0, max: 1024}`.
const DESCRIPTION_MAX: usize = 1024;

/// One emulator with this service mounted on a fresh shared engine, plus that engine so
/// a test can seed a record no implemented operation can create yet.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests drive the router directly rather than signing requests.
    router.set_request_authentication(false);
    router.register_service(cloudmap::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// Dispatch one operation and return its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/servicediscovery/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=unsigned"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("an awsJson1_1 body is JSON")
    };
    (status, json)
}

/// Dispatch an operation that must succeed, returning its output.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, json) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} must succeed, got {status}: {json}");
    json
}

/// Dispatch an operation that must fail, returning its status and wire error code.
async fn failure(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, String) {
    let (status, json) = call(router, operation, body).await;
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    assert_ne!(status, 200, "{operation} must fail, got {json}");
    (status, code)
}

/// Create an HTTP namespace and return `(namespace id, namespace ARN)`.
async fn namespace(router: &MultiServiceRouter, name: &str, description: Option<&str>) -> (String, String) {
    let mut body = json!({ "Name": name });
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    let created = ok(router, "CreateHttpNamespace", body).await;
    let operation_id = created["OperationId"]
        .as_str()
        .expect("CreateHttpNamespace returns an OperationId")
        .to_string();
    let operation = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    let namespace_id = operation["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string();
    let read = ok(router, "GetNamespace", json!({ "Id": namespace_id })).await;
    let arn = read["Namespace"]["Arn"]
        .as_str()
        .expect("a stored namespace carries its ARN")
        .to_string();
    (namespace_id, arn)
}

/// One `UpdateHttpNamespace` call carrying an `UpdaterRequestId`.
async fn update_with_token(
    router: &MultiServiceRouter,
    id: &str,
    token: &str,
    description: &str,
) -> Value {
    ok(
        router,
        "UpdateHttpNamespace",
        json!({
            "Id": id,
            "UpdaterRequestId": token,
            "Namespace": { "Description": description },
        }),
    )
    .await
}

/// The `Description` a namespace reads back through `GetNamespace`, as JSON (so an
/// absent member is distinguishable from an empty string).
async fn stored_description(router: &MultiServiceRouter, id: &str) -> Value {
    let read = ok(router, "GetNamespace", json!({ "Id": id })).await;
    read["Namespace"]["Description"].clone()
}

/// The rule: the description `Namespace.Description` carries is the description the
/// namespace holds afterwards, whatever that description is. The recorded case is one
/// row; the rows differ in length, content and form.
#[tokio::test]
async fn every_description_a_call_carries_is_what_the_namespace_reads_back() {
    let router = emulator();
    let (namespace_id, _) = namespace(&router, "desc-ns", Some("the original description")).await;

    let at_the_bound = "d".repeat(DESCRIPTION_MAX);
    let cases: Vec<(&str, String)> = vec![
        ("a short description", "the new description".to_string()),
        (
            "a description that replaces the previous one",
            "a second new description".to_string(),
        ),
        ("the empty description", String::new()),
        ("a single character", "x".to_string()),
        (
            "punctuation and whitespace",
            "tier: gold / zone: us-west-2, owner = team-a".to_string(),
        ),
        ("non-ASCII text", "descripción de malla — 网格".to_string()),
        ("at the published 1024 bound", at_the_bound),
    ];

    for (case, description) in cases {
        let updated = ok(
            &router,
            "UpdateHttpNamespace",
            json!({ "Id": namespace_id, "Namespace": { "Description": description } }),
        )
        .await;
        assert!(
            updated["OperationId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: UpdateHttpNamespace must return an OperationId, got {updated}"
        );

        // An empty description is no description: it replaces whatever was there, and
        // the read omits the member — the same projection a create with
        // `Description=""` produces, so a client cannot tell the two paths apart.
        let expected = if description.is_empty() {
            Value::Null
        } else {
            json!(description)
        };
        let read = stored_description(&router, &namespace_id).await;
        assert_eq!(
            read, expected,
            "case {case:?}: expected the namespace to read back the description it was given, \
             got {read}"
        );
    }
}

/// One past the published bound is refused by the modelled constraint, and the stored
/// description is left as it was.
#[tokio::test]
async fn a_description_past_the_published_bound_is_refused_and_changes_nothing() {
    let router = emulator();
    let (namespace_id, _) = namespace(&router, "bound-ns", Some("kept")).await;

    let (status, code) = failure(
        &router,
        "UpdateHttpNamespace",
        json!({
            "Id": namespace_id,
            "Namespace": { "Description": "d".repeat(DESCRIPTION_MAX + 1) },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a description past the 1024-character bound is a client error, got {status} {code}"
    );

    let read = stored_description(&router, &namespace_id).await;
    assert_eq!(
        read,
        json!("kept"),
        "a refused update must leave the stored description alone, got {read}"
    );
}

/// The call files an `UPDATE_NAMESPACE` operation targeting the namespace, which starts
/// non-terminal and reaches `SUCCESS` once the clock owes it.
#[tokio::test]
async fn the_call_files_an_update_namespace_operation_for_the_namespace() {
    let router = emulator();
    let (namespace_id, _) = namespace(&router, "op-ns", None).await;

    let updated = ok(
        &router,
        "UpdateHttpNamespace",
        json!({ "Id": namespace_id, "Namespace": { "Description": "filed" } }),
    )
    .await;
    let operation_id = updated["OperationId"]
        .as_str()
        .expect("UpdateHttpNamespace returns an OperationId")
        .to_string();

    let immediate = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    assert_eq!(
        immediate["Operation"]["Type"],
        json!("UPDATE_NAMESPACE"),
        "the filed operation is an UPDATE_NAMESPACE, got {immediate}"
    );
    assert_eq!(
        immediate["Operation"]["Targets"]["NAMESPACE"],
        json!(namespace_id),
        "the filed operation targets the namespace it updated, got {immediate}"
    );
    assert_eq!(
        immediate["Operation"]["Status"],
        json!("SUBMITTED"),
        "a caller polling straight away still sees the non-terminal status, got {immediate}"
    );

    // Blocking is fine here: the test owns the runtime, and the service never sleeps for
    // this span — it stores the due instant and materialises it on read.
    std::thread::sleep(cloudmap::state::OPERATION_SUCCESS_SPAN + Duration::from_millis(50));
    let settled = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    assert_eq!(
        settled["Operation"]["Status"],
        json!("SUCCESS"),
        "the clock owed this operation its terminal status, got {settled}"
    );
}

/// `Id` is "the ID or Amazon Resource Name (ARN) of the namespace", so both forms reach
/// the one stored record.
#[tokio::test]
async fn a_namespace_is_addressable_by_id_and_by_arn() {
    let router = emulator();
    let (namespace_id, arn) = namespace(&router, "ref-ns", None).await;

    for (case, reference) in [("by id", namespace_id.clone()), ("by ARN", arn.clone())] {
        let description = format!("addressed {case}");
        ok(
            &router,
            "UpdateHttpNamespace",
            json!({ "Id": reference, "Namespace": { "Description": description } }),
        )
        .await;
        let read = stored_description(&router, &namespace_id).await;
        assert_eq!(
            read,
            json!(description),
            "case {case:?}: both reference forms must update the one stored namespace, got {read}"
        );
    }
}

/// A reference no stored namespace answers to is the modelled `NamespaceNotFound`, and
/// nothing is written for it.
#[tokio::test]
async fn an_unknown_namespace_reference_is_namespace_not_found() {
    let router = emulator();
    let (namespace_id, arn) = namespace(&router, "known-ns", Some("untouched")).await;

    let cases = vec![
        ("an id that was never created", "ns-000000000000000".to_string()),
        (
            "an ARN of a namespace that was never created",
            arn.replace(&namespace_id, "ns-000000000000000"),
        ),
        ("a namespace name rather than its id", "known-ns".to_string()),
        ("the empty reference", String::new()),
    ];

    for (case, reference) in cases {
        let (status, code) = failure(
            &router,
            "UpdateHttpNamespace",
            json!({ "Id": reference, "Namespace": { "Description": "never stored" } }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NamespaceNotFound"),
            "case {case:?}: expected 400 NamespaceNotFound, got {status} {code}"
        );
    }

    let read = stored_description(&router, &namespace_id).await;
    assert_eq!(
        read,
        json!("untouched"),
        "a refused update must write nothing, got {read}"
    );
}

/// `UpdaterRequestId` "allows failed UpdateHttpNamespace requests to be retried without
/// the risk of running the operation twice": the same token with the same change replays
/// the first operation, while the same token with a different change is a new request.
#[tokio::test]
async fn a_retried_updater_request_id_files_one_operation() {
    let router = emulator();
    let (namespace_id, _) = namespace(&router, "token-ns", None).await;

    let first = update_with_token(&router, &namespace_id, "token-a", "one").await;
    let retry = update_with_token(&router, &namespace_id, "token-a", "one").await;
    assert_eq!(
        retry["OperationId"], first["OperationId"],
        "a retry of the same change with the same token replays the first operation, got {retry}"
    );

    let changed = update_with_token(&router, &namespace_id, "token-a", "two").await;
    assert_ne!(
        changed["OperationId"], first["OperationId"],
        "the same token asking for a DIFFERENT change is a new request, got {changed}"
    );
    let read = stored_description(&router, &namespace_id).await;
    assert_eq!(
        read,
        json!("two"),
        "the second change must be stored rather than replayed away, got {read}"
    );

    let other_token = update_with_token(&router, &namespace_id, "token-b", "three").await;
    assert_ne!(
        other_token["OperationId"], changed["OperationId"],
        "a different token is a different request, got {other_token}"
    );
}

/// An update only ever touches the namespace it names.
#[tokio::test]
async fn updating_one_namespace_leaves_every_other_namespace_alone() {
    let router = emulator();
    let (first, _) = namespace(&router, "isolated-a", Some("a")).await;
    let (second, _) = namespace(&router, "isolated-b", Some("b")).await;

    let before = ok(&router, "GetNamespace", json!({ "Id": first })).await;
    ok(
        &router,
        "UpdateHttpNamespace",
        json!({ "Id": first, "Namespace": { "Description": "a, updated" } }),
    )
    .await;
    let after = ok(&router, "GetNamespace", json!({ "Id": first })).await;

    for member in ["Id", "Arn", "Name", "Type", "CreateDate", "Properties"] {
        assert_eq!(
            after["Namespace"][member], before["Namespace"][member],
            "case {member:?}: an update changes only the description, got {after}"
        );
    }
    let untouched = stored_description(&router, &second).await;
    assert_eq!(
        untouched,
        json!("b"),
        "the other namespace must be untouched, got {untouched}"
    );
}

/// `UpdateHttpNamespace` updates an HTTP namespace; a namespace of another type is a bad
/// request, not a missing namespace. Only HTTP namespaces can be CREATED in this slice
/// (`CreatePrivateDnsNamespace` is still 501), so the DNS record is seeded.
#[tokio::test]
async fn a_namespace_of_another_type_is_refused_with_invalid_input() {
    let (router, engine) = emulator_with_engine();
    let (http_namespace, _) = namespace(&router, "type-ns", Some("http")).await;
    let state =
        engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();

    let seeded: Vec<(&str, String)> = vec![("DNS_PRIVATE", "ns-dnsprivate".to_string()), ("DNS_PUBLIC", "ns-dnspublic".to_string())];
    {
        let mut store = state.lock();
        let template = store
            .namespaces
            .get(&http_namespace)
            .cloned()
            .expect("the namespace just created");
        for (namespace_type, id) in &seeded {
            let mut record = template.clone();
            record.id = id.clone();
            record.name = format!("type-{namespace_type}");
            record.http_name = None;
            record.hosted_zone_id = Some("Z00000000000000000000".to_string());
            record.namespace_type = (*namespace_type).to_string();
            record.description = Some("seeded".to_string());
            store.namespace_ids_by_name.insert(record.name.clone(), id.clone());
            store.namespaces.insert(id.clone(), record);
        }
    }

    for (namespace_type, id) in &seeded {
        let (status, code) = failure(
            &router,
            "UpdateHttpNamespace",
            json!({ "Id": id, "Namespace": { "Description": "never stored" } }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {namespace_type:?}: expected 400 InvalidInput, got {status} {code}"
        );
        let read = stored_description(&router, id).await;
        assert_eq!(
            read,
            json!("seeded"),
            "case {namespace_type:?}: a refused update must write nothing, got {read}"
        );
    }

    // The HTTP namespace beside them still updates.
    ok(
        &router,
        "UpdateHttpNamespace",
        json!({ "Id": http_namespace, "Namespace": { "Description": "http, updated" } }),
    )
    .await;
    let read = stored_description(&router, &http_namespace).await;
    assert_eq!(
        read,
        json!("http, updated"),
        "the HTTP namespace must still accept an update, got {read}"
    );
}
