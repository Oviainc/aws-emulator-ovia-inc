// SPDX-License-Identifier: CC0-1.0

//! Athena's NOTEBOOK slice, driven end to end over the same router the emulator boots:
//! `ImportNotebook` writes an ipynb file into a Spark enabled workgroup and
//! `ExportNotebook` reads it back.
//!
//! THE RULE UNDER TEST: an import stores the ipynb content of exactly ONE source — the
//! request's own `Payload`, or the object the `NotebookS3LocationUri` names — under a fresh
//! `NotebookId` in a Spark enabled workgroup whose notebook names are unique, and an export
//! of that id answers with that content byte for byte beside the metadata it was stored
//! with. Neither source, or both, is refused with `InvalidRequestException`.
//!
//! Each test asserts that rule over a TABLE rather than one request: the rows differ along
//! what the rule generalises over — the size of the content, the character classes it
//! holds, which source carried it, how an Amazon S3 location is spelled, the shape and
//! length of the notebook name, which part of the request does not resolve, and whether an
//! idempotency token was reused.
//!
//! The object-store rows mount the emulator's own Amazon S3 beside Athena on one engine,
//! exactly the way the assembled emulator runs them, and stage every object with a real
//! `PutObject` — so an import from a location is a round trip through real object bytes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_athena_amazonathena::state::{
    self, MAX_NOTEBOOKS, NOTEBOOK_SOURCE_REQUIRED, NOTEBOOK_TYPE_IPYNB,
};
use service_com_amazonaws_athena_amazonathena::EmulatedService;

use crate::harness::call;

/// The Region both services are addressed in: Amazon S3 places a bucket created with no
/// `LocationConstraint` in us-east-1, which is also the Region the Athena calls below are
/// signed for.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. Request authentication is off in the harness, so nothing here is verified: the
/// scope is what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The bucket the staged notebook files live in.
const BUCKET: &str = "athena-notebooks";

/// The Spark enabled workgroup the imports below go into.
const SPARK_WORK_GROUP: &str = "spark-notebooks";

/// The PySpark engine version a Spark enabled workgroup selects
/// (`state::PYSPARK_ENGINE_VERSION_PREFIX` is the family it belongs to).
const PYSPARK_ENGINE_VERSION: &str = "PySpark engine version 3";

/// One emulator: Athena and Amazon S3 on a single shared engine behind the production
/// router — what the assembled emulator mounts, narrowed to the two services under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so a suite does not depend on how the emulator process was started.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    router.dispatch(request).await.status().as_u16()
}

/// `PUT /<bucket>` — `CreateBucket` — so the staged objects have a bucket to live in.
async fn given_bucket(router: &MultiServiceRouter) {
    let status = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status, 200,
        "the notebook bucket must be created first, got {status}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — storing `body` as the object's own bytes.
async fn given_object(router: &MultiServiceRouter, key: &str, body: &[u8]) {
    let status = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert_eq!(
        status, 200,
        "case {key:?}: the notebook file must be stored, got {status}"
    );
}

/// The wire error code (`__type`) of an error body, with the shape-id prefix the awsJson
/// protocols may carry stripped.
fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or("<no __type>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// The `Message` of an error body.
fn error_message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("<no Message>")
        .to_string()
}

/// Create a Spark enabled workgroup called `name`, selecting the PySpark engine.
async fn given_spark_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(
        router,
        "CreateWorkGroup",
        &json!({
            "Name": name,
            "Configuration": {
                "EngineVersion": { "SelectedEngineVersion": PYSPARK_ENGINE_VERSION },
                "ExecutionRole": "arn:aws:iam::000000000000:role/AthenaSparkExecution"
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the Spark workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// Create an Athena SQL workgroup called `name`: no Spark member set, so the engine version
/// resolves to the Athena SQL default.
async fn given_sql_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(router, "CreateWorkGroup", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "the SQL workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// Import `payload` as the notebook `name` of `work_group` and hand back the whole answer.
async fn import(
    router: &MultiServiceRouter,
    work_group: &str,
    name: &str,
    payload: &str,
) -> (u16, Value) {
    call(
        router,
        "ImportNotebook",
        &json!({
            "WorkGroup": work_group,
            "Name": name,
            "Type": NOTEBOOK_TYPE_IPYNB,
            "Payload": payload,
        }),
    )
    .await
}

/// Export the notebook `notebook_id` names and hand back the whole answer.
async fn export(router: &MultiServiceRouter, notebook_id: &str) -> (u16, Value) {
    call(
        router,
        "ExportNotebook",
        &json!({ "NotebookId": notebook_id }),
    )
    .await
}

/// Whether `id` is of the form the modelled `NotebookId` admits
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), checked segment by
/// segment so no pattern engine is needed.
fn is_notebook_id(id: &str) -> bool {
    let segments: Vec<&str> = id.split('-').collect();
    segments.len() == 5
        && segments
            .iter()
            .map(|segment| segment.len())
            .eq([8usize, 4, 4, 4, 12])
        && segments
            .iter()
            .all(|segment| segment.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

/// How `actual` differs from `expected`, as a sentence an assertion can carry for content
/// far too large to print: the two byte lengths and the first byte that differs.
fn difference(expected: &str, actual: &str) -> String {
    let first = expected
        .as_bytes()
        .iter()
        .zip(actual.as_bytes())
        .position(|(left, right)| left != right);
    format!(
        "expected {} bytes, got {} bytes; first differing byte at {:?}",
        expected.len(),
        actual.len(),
        first
    )
}

/// An ipynb document of `cells`, the JSON shape a Jupyter notebook file really is — so the
/// content imported below is a notebook rather than an opaque blob.
fn ipynb(cells: &[&str]) -> String {
    let cells: Vec<Value> = cells
        .iter()
        .map(|source| {
            json!({
                "cell_type": "code",
                "execution_count": Value::Null,
                "metadata": {},
                "outputs": [],
                "source": [source],
            })
        })
        .collect();
    serde_json::to_string(&json!({
        "cells": cells,
        "metadata": { "kernelspec": { "display_name": "PySpark", "name": "pysparkkernel" } },
        "nbformat": 4,
        "nbformat_minor": 5,
    }))
    .expect("the notebook serializes")
}

/// One row of the import/export table: the case, the notebook name, and the content.
struct ContentCase {
    case: &'static str,
    name: String,
    payload: String,
}

/// The rows: the content differs from the next along its size (one character up to a
/// megabyte), the character classes it holds (ASCII JSON, multi-byte UTF-8, control
/// characters, a long base64 run), and the notebook name differs along its shape and its
/// length (one character up to the modelled 255).
fn content_cases() -> Vec<ContentCase> {
    // `NotebookName` is 1 to 255 characters and excludes `/`, `:` and `\`.
    let longest_name: String = "nb.0_ "
        .chars()
        .cycle()
        .take(255)
        .collect();
    vec![
        ContentCase {
            case: "the shortest content the model admits",
            name: "a".to_string(),
            payload: "{".to_string(),
        },
        ContentCase {
            case: "a one-cell ipynb document",
            name: "analysis.ipynb".to_string(),
            payload: ipynb(&["spark.sql('SELECT 1').show()"]),
        },
        ContentCase {
            case: "multi-byte UTF-8 in the cell sources",
            name: "ノートブック 1.ipynb".to_string(),
            payload: ipynb(&["print('データ分析 🚀')", "print('αβγ δεζ')"]),
        },
        ContentCase {
            case: "control characters and CRLF line endings",
            name: "with\ttab.ipynb".to_string(),
            payload: "{\"cells\": [],\r\n\t\"nbformat\": 4,\r\n\t\"blob\": \"\u{7}\u{1}\"}"
                .to_string(),
        },
        ContentCase {
            case: "an embedded base64 image output, one long line",
            name: "plots.ipynb".to_string(),
            payload: ipynb(&["display(figure)"]).replace(
                "\"outputs\": []",
                &format!("\"outputs\": [\"{}\"]", "iVBORw0KGgoAAAANSU".repeat(512)),
            ),
        },
        ContentCase {
            case: "a megabyte of content under the longest name the model admits",
            name: longest_name,
            payload: ipynb(&["x = 1"]).replace("x = 1", &"x = 1; ".repeat(150_000)),
        },
    ]
}

#[tokio::test]
async fn an_imported_notebook_exports_byte_identical_content() {
    let router = emulator();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;

    let mut minted: Vec<String> = Vec::new();
    for case in content_cases() {
        let (status, imported) = import(&router, SPARK_WORK_GROUP, &case.name, &case.payload).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ImportNotebook to answer 200, got {status} with {imported:?}",
            case.case
        );
        let notebook_id = imported["NotebookId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_notebook_id(&notebook_id),
            "case {:?}: expected a NotebookId of the modelled form, got {notebook_id:?}",
            case.case
        );
        assert!(
            !minted.contains(&notebook_id),
            "case {:?}: expected a NotebookId no earlier import minted, got {notebook_id:?} \
             among {minted:?}",
            case.case
        );
        minted.push(notebook_id.clone());

        let (status, exported) = export(&router, &notebook_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ExportNotebook to answer 200, got {status} with {exported:?}",
            case.case
        );
        let payload = exported["Payload"].as_str().unwrap_or_default();
        assert!(
            payload == case.payload,
            "case {:?}: the exported content must be the imported content byte for byte; {}",
            case.case,
            difference(&case.payload, payload)
        );

        let metadata = &exported["NotebookMetadata"];
        assert_eq!(
            metadata["NotebookId"].as_str(),
            Some(notebook_id.as_str()),
            "case {:?}: expected the metadata to carry the imported id, got {metadata:?}",
            case.case
        );
        assert_eq!(
            metadata["Name"].as_str(),
            Some(case.name.as_str()),
            "case {:?}: expected the metadata to carry the imported name, got {metadata:?}",
            case.case
        );
        assert_eq!(
            metadata["WorkGroup"].as_str(),
            Some(SPARK_WORK_GROUP),
            "case {:?}: expected the metadata to name the workgroup imported into, got \
             {metadata:?}",
            case.case
        );
        assert_eq!(
            metadata["Type"].as_str(),
            Some(NOTEBOOK_TYPE_IPYNB),
            "case {:?}: expected the metadata to carry the imported type, got {metadata:?}",
            case.case
        );
        let created = metadata["CreationTime"].as_f64();
        assert!(
            created.is_some_and(|instant| instant > 0.0),
            "case {:?}: expected a CreationTime, got {metadata:?}",
            case.case
        );
        assert_eq!(
            metadata["LastModifiedTime"].as_f64(),
            created,
            "case {:?}: a notebook nothing has updated was last modified when it was \
             created, got {metadata:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_notebook_imported_from_an_object_store_location_is_that_object() {
    let router = emulator();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    given_bucket(&router).await;

    let key = "staged/notebooks/demo-1.ipynb";
    let content = ipynb(&["spark.range(10).show()", "print('ステージ')"]);
    given_object(&router, key, content.as_bytes()).await;

    // Every spelling of the same object the modelled `S3Uri` pattern admits
    // (`^(https|s3|S3)://([^/]+)/?(.*)$`): the two S3 schemes, and the virtual-hosted and
    // path-style HTTPS addressings.
    let locations = [
        (
            "the s3 scheme",
            format!("s3://{BUCKET}/{key}"),
        ),
        (
            "the S3 scheme in upper case",
            format!("S3://{BUCKET}/{key}"),
        ),
        (
            "a virtual-hosted HTTPS address",
            format!("https://{BUCKET}.s3.{REGION}.amazonaws.com/{key}"),
        ),
        (
            "a path-style HTTPS address",
            format!("https://s3.{REGION}.amazonaws.com/{BUCKET}/{key}"),
        ),
    ];

    for (index, (case, uri)) in locations.iter().enumerate() {
        let name = format!("staged-{index}.ipynb");
        let (status, imported) = call(
            &router,
            "ImportNotebook",
            &json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": name,
                "Type": NOTEBOOK_TYPE_IPYNB,
                "NotebookS3LocationUri": uri,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the import from {uri} to answer 200, got {status} with \
             {imported:?}"
        );
        let notebook_id = imported["NotebookId"].as_str().unwrap_or_default().to_string();
        let (status, exported) = export(&router, &notebook_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ExportNotebook to answer 200, got {status} with {exported:?}"
        );
        let payload = exported["Payload"].as_str().unwrap_or_default();
        assert!(
            payload == content,
            "case {case:?}: the exported content must be the staged object's bytes; {}",
            difference(&content, payload)
        );
    }
}

#[tokio::test]
async fn an_import_names_exactly_one_source() {
    let router = emulator();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    given_bucket(&router).await;
    given_object(&router, "held.ipynb", ipynb(&["pass"]).as_bytes()).await;
    given_object(&router, "empty.ipynb", &[]).await;
    // Bytes no UTF-8 decoder accepts: a lone continuation byte and a bare 0x80.
    given_object(&router, "binary.ipynb", &[0x80, 0xff, 0xfe, 0x00]).await;

    // Rows differ along WHICH source the request names, and along which part of a named
    // Amazon S3 location does not resolve.
    let refusals: Vec<(&str, Value, String)> = vec![
        (
            "neither Payload nor NotebookS3LocationUri",
            json!({ "WorkGroup": SPARK_WORK_GROUP, "Name": "neither.ipynb", "Type": NOTEBOOK_TYPE_IPYNB }),
            NOTEBOOK_SOURCE_REQUIRED.to_string(),
        ),
        (
            "both Payload and NotebookS3LocationUri",
            json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": "both.ipynb",
                "Type": NOTEBOOK_TYPE_IPYNB,
                "Payload": "{}",
                "NotebookS3LocationUri": format!("s3://{BUCKET}/held.ipynb"),
            }),
            NOTEBOOK_SOURCE_REQUIRED.to_string(),
        ),
        (
            "a location in a bucket the store does not hold",
            json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": "absent-bucket.ipynb",
                "Type": NOTEBOOK_TYPE_IPYNB,
                "NotebookS3LocationUri": "s3://athena-notebooks-elsewhere/held.ipynb",
            }),
            state::notebook_file_not_found("s3://athena-notebooks-elsewhere/held.ipynb"),
        ),
        (
            "a location at a key the bucket does not hold",
            json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": "absent-key.ipynb",
                "Type": NOTEBOOK_TYPE_IPYNB,
                "NotebookS3LocationUri": format!("s3://{BUCKET}/never-written.ipynb"),
            }),
            state::notebook_file_not_found(&format!("s3://{BUCKET}/never-written.ipynb")),
        ),
        (
            "a location naming a bucket and no file under it",
            json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": "bucket-only.ipynb",
                "Type": NOTEBOOK_TYPE_IPYNB,
                "NotebookS3LocationUri": format!("s3://{BUCKET}/"),
            }),
            state::notebook_file_not_found(&format!("s3://{BUCKET}/")),
        ),
        (
            "a location whose object holds no ipynb content at all",
            json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": "empty-file.ipynb",
                "Type": NOTEBOOK_TYPE_IPYNB,
                "NotebookS3LocationUri": format!("s3://{BUCKET}/empty.ipynb"),
            }),
            state::notebook_file_not_ipynb(&format!("s3://{BUCKET}/empty.ipynb")),
        ),
        (
            "a location whose object holds bytes that are no text",
            json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": "binary-file.ipynb",
                "Type": NOTEBOOK_TYPE_IPYNB,
                "NotebookS3LocationUri": format!("s3://{BUCKET}/binary.ipynb"),
            }),
            state::notebook_file_not_ipynb(&format!("s3://{BUCKET}/binary.ipynb")),
        ),
    ];

    for (case, request, message) in refusals {
        let (status, refused) = call(&router, "ImportNotebook", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            message,
            "case {case:?}: expected the documented message, got {refused:?}"
        );
        let (status, listed) = export(&router, "00000000-0000-4000-8000-000000000000").await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused import must have stored nothing to export, got \
             {status} with {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_import_needs_a_spark_enabled_workgroup_that_holds_the_name_free() {
    let router = emulator();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;
    given_spark_work_group(&router, "spark-notebooks-two").await;
    given_sql_work_group(&router, "sql-workgroup").await;

    let (status, imported) = import(&router, SPARK_WORK_GROUP, "taken.ipynb", "{}").await;
    assert_eq!(
        status, 200,
        "the first import of a name must be accepted, got {status} with {imported:?}"
    );

    // Rows differ along WHICH precondition of the target workgroup fails: it does not
    // exist, it runs Athena SQL (configured by name, or by default), or it already holds a
    // notebook of that name.
    let refusals: Vec<(&str, &str, &str, String)> = vec![
        (
            "a workgroup this account does not hold",
            "never-created",
            "new.ipynb",
            state::work_group_not_found("never-created"),
        ),
        (
            "Athena's own default workgroup, which runs SQL",
            state::DEFAULT_WORK_GROUP,
            "new.ipynb",
            state::work_group_not_spark_enabled(state::DEFAULT_WORK_GROUP),
        ),
        (
            "an Athena SQL workgroup",
            "sql-workgroup",
            "new.ipynb",
            state::work_group_not_spark_enabled("sql-workgroup"),
        ),
        (
            "a name the workgroup already holds a notebook under",
            SPARK_WORK_GROUP,
            "taken.ipynb",
            state::notebook_name_exists(SPARK_WORK_GROUP, "taken.ipynb"),
        ),
    ];

    for (case, work_group, name, message) in refusals {
        let (status, refused) = import(&router, work_group, name, "{}").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            message,
            "case {case:?}: expected the refusal to state the reason, got {refused:?}"
        );
    }

    // The name is unique within ONE workgroup: the same name in another Spark enabled
    // workgroup is another notebook.
    let (status, elsewhere) = import(&router, "spark-notebooks-two", "taken.ipynb", "{}").await;
    assert_eq!(
        status, 200,
        "the same name in another workgroup must be accepted, got {status} with {elsewhere:?}"
    );
    assert_ne!(
        elsewhere["NotebookId"], imported["NotebookId"],
        "two notebooks of the same name in two workgroups must carry different ids, got \
         {elsewhere:?} and {imported:?}"
    );
}

#[tokio::test]
async fn one_client_request_token_imports_one_notebook() {
    let router = emulator();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;

    // The modelled `ClientRequestToken` is a hyphenated hex token
    // (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`); the rows differ
    // along the content imported under each.
    let tokens = [
        ("a small document", "11111111-1111-4111-8111-111111111111", "{}"),
        (
            "a multi-byte document",
            "22222222-2222-4222-8222-222222222222",
            "{\"µ\": \"ü\"}",
        ),
    ];

    for (index, (case, token, payload)) in tokens.iter().enumerate() {
        let name = format!("idempotent-{index}.ipynb");
        let request = json!({
            "WorkGroup": SPARK_WORK_GROUP,
            "Name": name,
            "Type": NOTEBOOK_TYPE_IPYNB,
            "Payload": payload,
            "ClientRequestToken": token,
        });
        let (status, first) = call(&router, "ImportNotebook", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first import to answer 200, got {status} with {first:?}"
        );
        let (status, again) = call(&router, "ImportNotebook", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an import repeated under one token executes once and answers \
             the same way, got {status} with {again:?}"
        );
        assert_eq!(
            again["NotebookId"], first["NotebookId"],
            "case {case:?}: the repeat must answer with the id the token already minted, \
             got {again:?} after {first:?}"
        );

        // The same token for OTHER parameters describes another import, which is not the
        // one it already minted.
        let (status, mismatched) = call(
            &router,
            "ImportNotebook",
            &json!({
                "WorkGroup": SPARK_WORK_GROUP,
                "Name": format!("changed-{index}.ipynb"),
                "Type": NOTEBOOK_TYPE_IPYNB,
                "Payload": payload,
                "ClientRequestToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a reused token under changed parameters to be refused, \
             got {status} with {mismatched:?}"
        );
        assert_eq!(
            error_message(&mismatched),
            state::IDEMPOTENT_PARAMETERS_DO_NOT_MATCH,
            "case {case:?}: expected the idempotency refusal, got {mismatched:?}"
        );
    }
}

#[tokio::test]
async fn exporting_a_notebook_no_import_minted_is_refused() {
    let router = emulator();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;

    let (status, imported) = import(&router, SPARK_WORK_GROUP, "present.ipynb", "{}").await;
    assert_eq!(
        status, 200,
        "the notebook under test must be imported first, got {status} with {imported:?}"
    );
    let present = imported["NotebookId"].as_str().unwrap_or_default().to_string();

    // Rows differ along the shape of the id that names no notebook: every nibble zero,
    // every nibble f, and one that differs from a held id in a single character.
    let mut absent = vec![
        "00000000-0000-4000-8000-000000000000".to_string(),
        "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string(),
    ];
    let flipped = format!(
        "{}{}",
        if present.starts_with('a') { "b" } else { "a" },
        &present[1..]
    );
    absent.push(flipped);

    for id in absent {
        let (status, refused) = export(&router, &id).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {id:?}: expected InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            state::notebook_not_found(&id),
            "case {id:?}: expected the not-found message to name the id asked for, got \
             {refused:?}"
        );
    }

    // The held notebook is untouched by every refusal above: a read reports, it never
    // mutates.
    let (status, exported) = export(&router, &present).await;
    assert_eq!(
        status, 200,
        "the held notebook must still export, got {status} with {exported:?}"
    );
    assert_eq!(
        exported["Payload"].as_str(),
        Some("{}"),
        "the held notebook's content must be unchanged, got {exported:?}"
    );
}

#[tokio::test]
async fn the_emulator_holds_at_most_its_notebook_bound() {
    let router = emulator();
    given_spark_work_group(&router, SPARK_WORK_GROUP).await;

    // The emulator's own bound (`state::MAX_NOTEBOOKS`), filled to the brim and then one
    // past it: the one past must be refused rather than evicting a notebook a caller can
    // still name.
    for index in 0..MAX_NOTEBOOKS {
        let (status, imported) =
            import(&router, SPARK_WORK_GROUP, &format!("bulk-{index}.ipynb"), "{}").await;
        assert_eq!(
            status, 200,
            "notebook {index} of the bound must be accepted, got {status} with {imported:?}"
        );
    }
    let (status, refused) = import(&router, SPARK_WORK_GROUP, "one-too-many.ipynb", "{}").await;
    assert_eq!(
        status, 400,
        "the import past the bound must be refused, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "expected InvalidRequestException past the bound, got {refused:?}"
    );

    // And the first notebook is still there: nothing was evicted to make room.
    let (status, listed) = import(&router, SPARK_WORK_GROUP, "bulk-0.ipynb", "{}").await;
    assert_eq!(
        status, 400,
        "the first notebook must still hold its name, got {status} with {listed:?}"
    );
    assert_eq!(
        error_message(&listed),
        state::notebook_name_exists(SPARK_WORK_GROUP, "bulk-0.ipynb"),
        "expected the name to still be taken, got {listed:?}"
    );
}
