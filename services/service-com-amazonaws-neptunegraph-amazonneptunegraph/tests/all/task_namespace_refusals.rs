// SPDX-License-Identifier: CC0-1.0

//! How a task lookup words "this account holds no such task", driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rule: an account-scoped lookup that resolves nothing reports the KIND of resource
//! it searched for and the account it searched in, and then the identifier clause AS THE
//! OPERATION WORDS IT — the operations that READ a task (`GetImportTask`,
//! `GetExportTask`) spell the task's own kind in front of that clause, the operations
//! that CANCEL one (`CancelImportTask`, `CancelExportTask`) carry the bare `task id`.
//! Neither inherits the other's wording, so a client that matches the refusal of one
//! operation recognises it.
//!
//! Import tasks and export tasks share ONE modelled identifier space (`TaskId` and
//! `ExportTaskId` are both `^t-[a-z0-9]{10}$`) while living in two namespaces, which is
//! why the READ spells the kind out: on a read that prefix is the only thing in the
//! message that says which namespace was searched. The table below therefore spans:
//!
//! * the namespace (import, export) and so the kind word each reports;
//! * the operation that resolves the identifier (the read and the cancel of each kind,
//!   over GET and DELETE), and so the identifier clause each words;
//! * the identifier value — the extremes of the shared id space, a mixed one, and the
//!   id of a task that really exists IN THE SIBLING NAMESPACE, which is the one value
//!   that is well formed, stored, and still not found by this lookup.
//!
//! Both fixture tasks are read back at the end, so none of the refusals above came
//! from a lookup that had stopped finding anything.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneTaskRole";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/graphs/{id}")).await;
    id
}

/// Start an import task on its own serving graph and return its minted id.
async fn stored_import_task(router: &MultiServiceRouter, graph_name: &str) -> String {
    let graph_id = graph_in_service(router, graph_name).await;
    let (status, started) = call(
        router,
        "POST",
        &format!("/graphs/{graph_id}/importtasks"),
        Some(json!({"source": "s3://example-bucket/data/", "roleArn": ROLE_ARN})),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture import task: expected 201 from StartImportTask, got {status}: {started}"
    );
    started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("the fixture import task: no taskId in {started}"))
        .to_string()
}

/// Start an export task on its own serving graph and return its minted id.
async fn stored_export_task(router: &MultiServiceRouter, graph_name: &str) -> String {
    let graph_id = graph_in_service(router, graph_name).await;
    let (status, started) = call(
        router,
        "POST",
        "/exporttasks",
        Some(json!({
            "graphIdentifier": graph_id,
            "roleArn": ROLE_ARN,
            "format": "CSV",
            "destination": "s3://export-bucket/namespaces/",
            "kmsKeyIdentifier": KMS_KEY_ARN,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture export task: expected 201 from StartExportTask, got {status}: {started}"
    );
    started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("the fixture export task: no taskId in {started}"))
        .to_string()
}

/// One lookup of a task namespace: the operation, its HTTP method, its path template
/// with `{id}` where the identifier goes, and whether that operation spells the task's
/// kind in front of the identifier clause (the reads do, the cancels do not).
type Lookup = (&'static str, &'static str, &'static str, bool);

#[tokio::test]
async fn a_task_lookup_that_resolves_nothing_words_its_operations_identifier_clause() {
    let router = emulator();

    // One task really stored in each namespace, so the refusals below are about the
    // identifier that was asked for and not about an empty store — and so the
    // cross-namespace rows can ask for an identifier that IS stored, just not here.
    let import_task = stored_import_task(&router, "import-namespace").await;
    let export_task = stored_export_task(&router, "export-namespace").await;

    // The one canonical emulator account every request is served for — read from the
    // framework rather than written out, so the assertion cannot drift from the account
    // the service mints ARNs with.
    let account = engine::emulator_account_id();

    // Well-formed identifiers of the shared task-id space (`^t-[a-z0-9]{10}$`) that no
    // task answers to: the extremes of that space and a mixed value.
    let unminted = ["t-0000000000", "t-zzzzzzzzzz", "t-a1b2c3d4e5"];

    // (the kind as the service names it, its lookups, the identifier of a task stored
    // in the SIBLING namespace)
    let namespaces: Vec<(&str, Vec<Lookup>, &str)> = vec![
        (
            "import task",
            vec![
                ("GetImportTask", "GET", "/importtasks/{id}", true),
                ("CancelImportTask", "DELETE", "/importtasks/{id}", false),
            ],
            export_task.as_str(),
        ),
        (
            "export task",
            vec![
                ("GetExportTask", "GET", "/exporttasks/{id}", true),
                ("CancelExportTask", "DELETE", "/exporttasks/{id}", false),
            ],
            import_task.as_str(),
        ),
    ];

    for (kind, lookups, sibling_id) in namespaces {
        let mut identifiers: Vec<&str> = unminted.to_vec();
        identifiers.push(sibling_id);
        for (operation, method, template, spells_the_kind) in lookups {
            for identifier in &identifiers {
                let case = format!("{operation} for {identifier}");
                let path = template.replace("{id}", identifier);
                let (status, answer) = call(&router, method, &path, None).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected 404, got {status}: {answer}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {answer}"
                );
                let actual = message(&answer);
                let clause = if spells_the_kind {
                    format!("{kind} id")
                } else {
                    "task id".to_string()
                };
                let expected =
                    format!("No {kind} found for customer [{account}] with {clause} [{identifier}]");
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }

    // Each fixture task still reads back through its own namespace, so the refusals
    // above were about the identifier and not about a lookup that stopped resolving.
    for (case, path, task_id) in [
        (
            "the stored import task",
            format!("/importtasks/{import_task}"),
            &import_task,
        ),
        (
            "the stored export task",
            format!("/exporttasks/{export_task}"),
            &export_task,
        ),
    ] {
        let (status, read) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {read}"
        );
        assert_eq!(
            read["taskId"],
            json!(task_id),
            "case {case:?}: expected taskId {task_id:?}, got {}",
            read["taskId"]
        );
    }
}
