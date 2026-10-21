// SPDX-License-Identifier: CC0-1.0

//! The Data Pipeline pipeline lifecycle, driven end-to-end over the router.
//!
//! Every call here goes through the real wire path — `POST /` with
//! `X-Amz-Target: DataPipeline.<Operation>` and an `application/x-amz-json-1.1` body,
//! dispatched by the same [`MultiServiceRouter`] the emulator binary mounts — so what is
//! asserted is the emulator's HTTP behaviour, not a handler called directly.
//!
//! Each test is a TABLE: the rows differ along whatever the rule under test generalises
//! over (tag count, object count, whether parameters are supplied, how many pipelines
//! exist, which `version` is asked for, which pipeline id is named), and the single case
//! the bootstrap issue cited is just one row.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_datapipeline_datapipeline::ops::describe_pipelines::{
    MAX_PIPELINE_IDS, METADATA_FIELD_KEYS,
};
use service_com_amazonaws_datapipeline_datapipeline::ops::list_pipelines::PAGE_SIZE;
use service_com_amazonaws_datapipeline_datapipeline::ops::put_pipeline_definition::{
    MAX_FIELDS_PER_OBJECT, MAX_PIPELINE_OBJECTS,
};
use service_com_amazonaws_datapipeline_datapipeline::state::MAX_PIPELINES;
use service_com_amazonaws_datapipeline_datapipeline::EmulatedService;

/// A router with only this service mounted, over a fresh shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests drive the wire path, not SigV4; signing is the framework's own concern.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One awsJson1_1 call: returns the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://datapipeline.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("DataPipeline.{operation}"))
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation}: response body is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// A successful call, or a panic naming the operation and what came back instead.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The awsJson1_1 error code of a failed call: the trailing segment of `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Create a pipeline and return its `pipelineId`.
async fn create(router: &MultiServiceRouter, name: &str, unique_id: &str) -> String {
    let created = ok(
        router,
        "CreatePipeline",
        &json!({"name": name, "uniqueId": unique_id}),
    )
    .await;
    created
        .get("pipelineId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreatePipeline({name}): no pipelineId in {created}"))
        .to_string()
}

/// A pipeline id that was never assigned, for the not-found rows.
const UNKNOWN_PIPELINE_ID: &str = "df-NOSUCHPIPELINE00000";

/// One row of the lifecycle table.
struct LifecycleCase {
    label: &'static str,
    name: &'static str,
    unique_id: &'static str,
    description: Option<&'static str>,
    /// A `tagList` (0..10 entries), as it goes on the wire.
    tags: Value,
    /// A `PipelineObjectList`, as it goes on the wire.
    pipeline_objects: Value,
    /// A `ParameterObjectList`, or `Value::Null` for a definition with no parameters.
    parameter_objects: Value,
    /// A `ParameterValueList`, or `Value::Null`.
    parameter_values: Value,
}

/// The lifecycle rows: they differ in whether a description is set, in tag count (0, 1,
/// 3, 10 — the modelled maximum), in object and field count, in whether a field is a
/// `refValue` rather than a `stringValue`, and in whether the definition carries
/// parameters.
fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        LifecycleCase {
            // The row the bootstrap issue cited.
            label: "no description, one tag, one object",
            name: "demo",
            unique_id: "demo-1",
            description: Some("bootstrap"),
            tags: json!([{"key": "env", "value": "test"}]),
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "scheduleType", "stringValue": "ondemand"}]}
            ]),
            parameter_objects: Value::Null,
            parameter_values: Value::Null,
        },
        LifecycleCase {
            label: "no description, no tags, two objects with a reference",
            name: "bare",
            unique_id: "bare-1",
            description: None,
            tags: json!([]),
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "workerGroup", "stringValue": "workerGroup"}]},
                {"id": "SayHello", "name": "SayHello",
                 "fields": [
                    {"key": "type", "stringValue": "ShellCommandActivity"},
                    {"key": "command", "stringValue": "echo hello"},
                    {"key": "parent", "refValue": "Default"}
                 ]}
            ]),
            parameter_objects: Value::Null,
            parameter_values: Value::Null,
        },
        LifecycleCase {
            label: "described, three tags, parameterised definition",
            name: "parameterised",
            unique_id: "parameterised-1",
            description: Some("uses parameters"),
            tags: json!([
                {"key": "env", "value": "test"},
                {"key": "owner", "value": "team"},
                {"key": "empty-value", "value": ""}
            ]),
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "scheduleType", "stringValue": "cron"}]},
                {"id": "Copy", "name": "Copy",
                 "fields": [
                    {"key": "type", "stringValue": "CopyActivity"},
                    {"key": "input", "refValue": "myInputBucket"},
                    {"key": "parent", "refValue": "Default"}
                 ]}
            ]),
            parameter_objects: json!([
                {"id": "myInputBucket",
                 "attributes": [{"key": "type", "stringValue": "String"}]}
            ]),
            parameter_values: json!([{"id": "myInputBucket", "stringValue": "s3://in/"}]),
        },
        LifecycleCase {
            label: "ten tags (the modelled maximum), five objects",
            name: "wide",
            unique_id: "wide-1",
            description: Some("the widest row"),
            tags: json!([
                {"key": "t0", "value": "0"}, {"key": "t1", "value": "1"},
                {"key": "t2", "value": "2"}, {"key": "t3", "value": "3"},
                {"key": "t4", "value": "4"}, {"key": "t5", "value": "5"},
                {"key": "t6", "value": "6"}, {"key": "t7", "value": "7"},
                {"key": "t8", "value": "8"}, {"key": "t9", "value": "9"}
            ]),
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "scheduleType", "stringValue": "ondemand"}]},
                {"id": "A", "name": "A", "fields": [{"key": "parent", "refValue": "Default"}]},
                {"id": "B", "name": "B", "fields": [{"key": "parent", "refValue": "A"}]},
                {"id": "C", "name": "C", "fields": [{"key": "parent", "refValue": "B"}]},
                {"id": "D", "name": "D", "fields": [
                    {"key": "parent", "refValue": "C"},
                    {"key": "type", "stringValue": "ShellCommandActivity"},
                    {"key": "command", "stringValue": "echo done"}
                ]}
            ]),
            parameter_objects: Value::Null,
            parameter_values: Value::Null,
        },
    ]
}

/// The `CreatePipeline` request body for a row.
fn create_body(case: &LifecycleCase) -> Value {
    let mut body = json!({"name": case.name, "uniqueId": case.unique_id});
    if let Some(description) = case.description {
        body["description"] = json!(description);
    }
    body["tags"] = case.tags.clone();
    body
}

/// The `PutPipelineDefinition` request body for a row.
fn put_body(case: &LifecycleCase, pipeline_id: &str) -> Value {
    let mut body = json!({
        "pipelineId": pipeline_id,
        "pipelineObjects": case.pipeline_objects.clone(),
    });
    if !case.parameter_objects.is_null() {
        body["parameterObjects"] = case.parameter_objects.clone();
    }
    if !case.parameter_values.is_null() {
        body["parameterValues"] = case.parameter_values.clone();
    }
    body
}

/// The whole happy path, per row: create, re-create idempotently, commit a definition,
/// read it back, describe, list, delete, and confirm the id stops resolving.
#[tokio::test]
async fn pipeline_lifecycle_round_trips_for_every_shape_of_pipeline() {
    for case in lifecycle_cases() {
        let label = case.label;
        let router = emulator();

        // 1. CreatePipeline mints an id of the documented `df-…` form.
        let created = ok(&router, "CreatePipeline", &create_body(&case)).await;
        let pipeline_id = created
            .get("pipelineId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: no pipelineId in {created}"))
            .to_string();
        assert!(
            pipeline_id.starts_with("df-") && pipeline_id.len() == 22,
            "case {label:?}: expected a df- id of 22 characters, got {pipeline_id:?}"
        );

        // 2. The identical call is idempotent on `uniqueId`.
        let again = ok(&router, "CreatePipeline", &create_body(&case)).await;
        assert_eq!(
            again.get("pipelineId").and_then(Value::as_str),
            Some(pipeline_id.as_str()),
            "case {label:?}: a repeat CreatePipeline must return the same id, got {again}"
        );
        let listed = ok(&router, "ListPipelines", &json!({})).await;
        assert_eq!(
            listed["pipelineIdList"].as_array().map(Vec::len),
            Some(1),
            "case {label:?}: the repeat call must not create a second pipeline, got {listed}"
        );

        // 3. PutPipelineDefinition commits a well-formed definition.
        let put = ok(&router, "PutPipelineDefinition", &put_body(&case, &pipeline_id)).await;
        assert_eq!(
            put["errored"],
            json!(false),
            "case {label:?}: a well-formed definition must not be errored, got {put}"
        );
        assert!(
            put.get("validationErrors").is_none(),
            "case {label:?}: expected no validationErrors, got {put}"
        );

        // 4. GetPipelineDefinition reads back exactly what was stored.
        let fetched = ok(
            &router,
            "GetPipelineDefinition",
            &json!({"pipelineId": pipeline_id}),
        )
        .await;
        assert_eq!(
            fetched["pipelineObjects"], case.pipeline_objects,
            "case {label:?}: expected the stored pipelineObjects, got {fetched}"
        );
        assert_eq!(
            fetched.get("parameterObjects").cloned(),
            (!case.parameter_objects.is_null()).then(|| case.parameter_objects.clone()),
            "case {label:?}: expected the stored parameterObjects, got {fetched}"
        );
        assert_eq!(
            fetched.get("parameterValues").cloned(),
            (!case.parameter_values.is_null()).then(|| case.parameter_values.clone()),
            "case {label:?}: expected the stored parameterValues, got {fetched}"
        );

        // 5. DescribePipelines reports the record and the documented metadata fields.
        let described = ok(
            &router,
            "DescribePipelines",
            &json!({"pipelineIds": [pipeline_id]}),
        )
        .await;
        let description = &described["pipelineDescriptionList"][0];
        assert_eq!(
            description["pipelineId"],
            json!(pipeline_id),
            "case {label:?}: wrong pipelineId in {described}"
        );
        assert_eq!(
            description["name"],
            json!(case.name),
            "case {label:?}: wrong name in {described}"
        );
        assert_eq!(
            description.get("description").and_then(Value::as_str),
            case.description,
            "case {label:?}: wrong description in {described}"
        );
        let expected_tags = if case.tags.as_array().is_some_and(Vec::is_empty) {
            Value::Null
        } else {
            case.tags.clone()
        };
        assert_eq!(
            description.get("tags").cloned().unwrap_or(Value::Null),
            expected_tags,
            "case {label:?}: wrong tags in {described}"
        );
        assert_metadata_fields(label, description, &pipeline_id, &case);

        // 6. ListPipelines pages off the store.
        let listed = ok(&router, "ListPipelines", &json!({})).await;
        assert_eq!(
            listed["pipelineIdList"],
            json!([{"id": pipeline_id, "name": case.name}]),
            "case {label:?}: wrong pipelineIdList in {listed}"
        );
        assert_eq!(
            listed["hasMoreResults"],
            json!(false),
            "case {label:?}: a single page must report hasMoreResults false, got {listed}"
        );
        assert!(
            listed.get("marker").is_none(),
            "case {label:?}: the last page must carry no marker, got {listed}"
        );

        // 7. DeletePipeline removes the pipeline and its definition.
        let deleted = ok(&router, "DeletePipeline", &json!({"pipelineId": pipeline_id})).await;
        assert!(
            deleted.is_null() || deleted == json!({}),
            "case {label:?}: DeletePipeline returns no members, got {deleted}"
        );

        // 8. The id stops resolving, and the pipeline stops being listed.
        for (operation, body) in [
            (
                "DescribePipelines",
                json!({"pipelineIds": [pipeline_id.clone()]}),
            ),
            (
                "GetPipelineDefinition",
                json!({"pipelineId": pipeline_id.clone()}),
            ),
            (
                "PutPipelineDefinition",
                put_body(&case, &pipeline_id.clone()),
            ),
        ] {
            let (status, body) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {label:?}: {operation} on a deleted pipeline must be HTTP 400, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "PipelineDeletedException",
                "case {label:?}: {operation} on a deleted pipeline, got {body}"
            );
        }
        let listed = ok(&router, "ListPipelines", &json!({})).await;
        assert_eq!(
            listed["pipelineIdList"],
            json!([]),
            "case {label:?}: a deleted pipeline must not be listed, got {listed}"
        );
    }
}

/// `PipelineDescription$fields` carries exactly the read-only metadata keys the
/// `DescribePipelines` response example in the Smithy model documents, with values taken
/// from the stored record.
fn assert_metadata_fields(
    label: &str,
    description: &Value,
    pipeline_id: &str,
    case: &LifecycleCase,
) {
    let fields = description["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: fields is not a list in {description}"));
    let by_key: std::collections::BTreeMap<&str, &str> = fields
        .iter()
        .filter_map(|field| {
            Some((
                field.get("key")?.as_str()?,
                field.get("stringValue")?.as_str()?,
            ))
        })
        .collect();

    let expected_keys: BTreeSet<&str> = METADATA_FIELD_KEYS
        .iter()
        .copied()
        // `description` is only reported when the pipeline has one.
        .filter(|key| *key != "description" || case.description.is_some())
        .collect();
    let got_keys: BTreeSet<&str> = by_key.keys().copied().collect();
    assert_eq!(
        got_keys, expected_keys,
        "case {label:?}: wrong metadata field keys in {description}"
    );

    for (key, expected) in [
        ("@pipelineState", "PENDING"),
        ("@id", pipeline_id),
        ("@sphere", "PIPELINE"),
        ("@version", "1"),
        ("@userId", engine::EMULATOR_ACCOUNT_ID),
        ("@accountId", engine::EMULATOR_ACCOUNT_ID),
        ("uniqueId", case.unique_id),
        ("name", case.name),
    ] {
        assert_eq!(
            by_key.get(key).copied(),
            Some(expected),
            "case {label:?}: field {key} in {description}"
        );
    }

    // `@creationTime` is a per-run value, so only its documented shape is asserted:
    // `2012-12-13T01:24:06` — ISO-8601 UTC to the second, no zone suffix.
    let creation_time = by_key
        .get("@creationTime")
        .copied()
        .unwrap_or_else(|| panic!("case {label:?}: no @creationTime in {description}"));
    assert!(
        creation_time.len() == 19
            && creation_time.as_bytes()[10] == b'T'
            && creation_time
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-' || b == b':' || b == b'T'),
        "case {label:?}: expected @creationTime like 2012-12-13T01:24:06, got \
         {creation_time:?}"
    );
}

/// A `uniqueId` already claimed is honoured for the name that claimed it and refused for
/// any other name; deleting the pipeline releases the claim.
#[tokio::test]
async fn create_pipeline_keys_idempotency_on_the_unique_id() {
    let router = emulator();
    let first = create(&router, "alpha", "shared-unique-id").await;

    // Same uniqueId, same name: the id the first call assigned.
    let repeat = create(&router, "alpha", "shared-unique-id").await;
    assert_eq!(
        repeat, first,
        "same (name, uniqueId): expected {first}, got {repeat}"
    );

    // Same uniqueId, a different name: refused. The rows vary the colliding name.
    for other_name in ["beta", "ALPHA", "alpha ", "a"] {
        let (status, body) = call(
            &router,
            "CreatePipeline",
            &json!({"name": other_name, "uniqueId": "shared-unique-id"}),
        )
        .await;
        assert_eq!(
            status, 400,
            "name {other_name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "name {other_name:?}: got {body}"
        );
    }

    // A different uniqueId under the same name is a different pipeline.
    let sibling = create(&router, "alpha", "another-unique-id").await;
    assert_ne!(
        sibling, first,
        "a fresh uniqueId must mint a new pipeline, got {sibling}"
    );

    // Deleting releases the claim: the same pair creates a NEW pipeline.
    ok(&router, "DeletePipeline", &json!({"pipelineId": first})).await;
    let recreated = create(&router, "alpha", "shared-unique-id").await;
    assert_ne!(
        recreated, first,
        "after a delete the pair must mint a new pipeline, got {recreated}"
    );
}

/// `ListPipelines` walks every live pipeline exactly once, for pipeline counts either
/// side of the page boundary, and refuses a marker it did not issue.
#[tokio::test]
async fn list_pipelines_pages_over_every_live_pipeline_exactly_once() {
    for count in [
        0,
        1,
        PAGE_SIZE - 1,
        PAGE_SIZE,
        PAGE_SIZE + 1,
        2 * PAGE_SIZE + 3,
    ] {
        let router = emulator();
        let mut created = BTreeSet::new();
        for index in 0..count {
            created.insert(create(&router, &format!("p{index}"), &format!("u{index}")).await);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = match &marker {
                Some(marker) => json!({"marker": marker}),
                None => json!({}),
            };
            let page = ok(&router, "ListPipelines", &body).await;
            pages += 1;
            let ids = page["pipelineIdList"]
                .as_array()
                .unwrap_or_else(|| panic!("count {count}: pipelineIdList missing in {page}"));
            assert!(
                ids.len() <= PAGE_SIZE,
                "count {count}: a page may hold at most {PAGE_SIZE} entries, got {page}"
            );
            for entry in ids {
                seen.push(
                    entry["id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("count {count}: no id in {entry}"))
                        .to_string(),
                );
            }
            if page["hasMoreResults"] == json!(true) {
                marker = Some(
                    page["marker"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("count {count}: hasMoreResults with no marker in {page}")
                        })
                        .to_string(),
                );
            } else {
                assert!(
                    page.get("marker").is_none(),
                    "count {count}: the last page must carry no marker, got {page}"
                );
                break;
            }
            assert!(
                pages <= count + 2,
                "count {count}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen.len(),
            count,
            "count {count}: expected {count} ids across {pages} pages, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "count {count}: the pages must cover exactly the created pipelines, got {seen:?}"
        );
    }

    // A marker this service never issued is a bad request, not a silent first page.
    let router = emulator();
    for bogus in ["not-a-marker", "dp1:zz", "dp1:", "0"] {
        let (status, body) = call(&router, "ListPipelines", &json!({"marker": bogus})).await;
        assert_eq!(
            status, 400,
            "marker {bogus:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "marker {bogus:?}: got {body}"
        );
    }
}

/// One row of the definition-validation table.
struct ValidationCase {
    label: &'static str,
    pipeline_objects: Value,
    /// `Some(error code)` when the definition is refused outright, `None` when it is
    /// stored.
    refused_with: Option<&'static str>,
    /// Whether a stored definition comes back with `errored: true`.
    errored: bool,
}

/// `count` distinct string-valued fields, for the field-count rows.
fn fields(count: usize) -> Value {
    json!((0..count)
        .map(|index| json!({"key": format!("k{index}"), "stringValue": format!("v{index}")}))
        .collect::<Vec<Value>>())
}

/// The two tiers `PutPipelineDefinition` documents: a FATAL problem saves nothing and is
/// refused, anything else is saved with `errored` set.
#[tokio::test]
async fn put_pipeline_definition_separates_fatal_from_recorded_validation_errors() {
    let too_many: Vec<Value> = (0..=MAX_PIPELINE_OBJECTS)
        .map(|index| {
            json!({"id": format!("o{index}"), "name": format!("o{index}"),
                   "fields": [{"key": "type", "stringValue": "ShellCommandActivity"}]})
        })
        .collect();
    let at_the_limit: Vec<Value> = too_many[..MAX_PIPELINE_OBJECTS].to_vec();

    let cases = vec![
        ValidationCase {
            label: "well formed",
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "scheduleType", "stringValue": "ondemand"}]}
            ]),
            refused_with: None,
            errored: false,
        },
        ValidationCase {
            // The case the operation's own documentation shows as an error response.
            label: "a string field is empty",
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "workerGroup", "stringValue": ""}]}
            ]),
            refused_with: Some("InvalidRequestException"),
            errored: false,
        },
        ValidationCase {
            label: "a field carries neither stringValue nor refValue",
            pipeline_objects: json!([
                {"id": "Default", "name": "Default", "fields": [{"key": "workerGroup"}]}
            ]),
            refused_with: Some("InvalidRequestException"),
            errored: false,
        },
        ValidationCase {
            label: "a field carries both stringValue and refValue",
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "parent", "stringValue": "x", "refValue": "Default"}]}
            ]),
            refused_with: Some("InvalidRequestException"),
            errored: false,
        },
        ValidationCase {
            label: "exactly the maximum number of objects",
            pipeline_objects: json!(at_the_limit),
            refused_with: None,
            errored: false,
        },
        ValidationCase {
            label: "one object past the maximum",
            pipeline_objects: json!(too_many),
            refused_with: Some("InvalidRequestException"),
            errored: false,
        },
        ValidationCase {
            label: "a reference to an object the definition does not define",
            pipeline_objects: json!([
                {"id": "Default", "name": "Default",
                 "fields": [{"key": "parent", "refValue": "Missing"}]}
            ]),
            refused_with: None,
            errored: true,
        },
        ValidationCase {
            label: "exactly the maximum number of fields on one object",
            pipeline_objects: json!([
                {"id": "Default", "name": "Default", "fields": fields(MAX_FIELDS_PER_OBJECT)}
            ]),
            refused_with: None,
            errored: false,
        },
        ValidationCase {
            label: "one field past the maximum on one object",
            pipeline_objects: json!([
                {"id": "Default", "name": "Default", "fields": fields(MAX_FIELDS_PER_OBJECT + 1)}
            ]),
            refused_with: Some("InvalidRequestException"),
            errored: false,
        },
    ];

    for case in cases {
        let label = case.label;
        let router = emulator();
        let pipeline_id = create(&router, "validated", "validated-1").await;

        // A definition already committed, so a refused Put can be shown to change nothing.
        let committed = json!([
            {"id": "Default", "name": "Default",
             "fields": [{"key": "scheduleType", "stringValue": "ondemand"}]}
        ]);
        ok(
            &router,
            "PutPipelineDefinition",
            &json!({"pipelineId": pipeline_id, "pipelineObjects": committed}),
        )
        .await;

        let body = json!({"pipelineId": pipeline_id, "pipelineObjects": case.pipeline_objects});
        let (status, response) = call(&router, "PutPipelineDefinition", &body).await;

        match case.refused_with {
            Some(expected_code) => {
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected HTTP 400, got {status} with {response}"
                );
                assert_eq!(
                    error_code(&response),
                    expected_code,
                    "case {label:?}: got {response}"
                );
                let fetched = ok(
                    &router,
                    "GetPipelineDefinition",
                    &json!({"pipelineId": pipeline_id}),
                )
                .await;
                assert_eq!(
                    fetched["pipelineObjects"], committed,
                    "case {label:?}: a refused Put must leave the stored definition alone, \
                     got {fetched}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected HTTP 200, got {status} with {response}"
                );
                assert_eq!(
                    response["errored"],
                    json!(case.errored),
                    "case {label:?}: wrong errored in {response}"
                );
                assert_eq!(
                    response.get("validationErrors").is_some(),
                    case.errored,
                    "case {label:?}: validationErrors must accompany errored, got {response}"
                );
                let fetched = ok(
                    &router,
                    "GetPipelineDefinition",
                    &json!({"pipelineId": pipeline_id}),
                )
                .await;
                assert_eq!(
                    fetched["pipelineObjects"], case.pipeline_objects,
                    "case {label:?}: a stored definition must read back, got {fetched}"
                );
            }
        }
    }
}

/// `version` selects which definition `GetPipelineDefinition` reports: `latest` (the
/// default) the last one saved, `active` the last one ACTIVATED — and nothing else.
#[tokio::test]
async fn get_pipeline_definition_honours_the_version_selector() {
    let router = emulator();
    let pipeline_id = create(&router, "versioned", "versioned-1").await;
    let first = json!([
        {"id": "Default", "name": "Default",
         "fields": [{"key": "scheduleType", "stringValue": "ondemand"}]}
    ]);
    let second = json!([
        {"id": "Default", "name": "Default",
         "fields": [{"key": "scheduleType", "stringValue": "cron"}]},
        {"id": "A", "name": "A", "fields": [{"key": "parent", "refValue": "Default"}]}
    ]);

    // `@version` counts the definitions committed, so it tracks the commits rather than
    // being a constant: 0 before the first, then 1 and 2.
    for (expected_version, definition) in [("0", None), ("1", Some(&first)), ("2", Some(&second))] {
        if let Some(definition) = definition {
            ok(
                &router,
                "PutPipelineDefinition",
                &json!({"pipelineId": pipeline_id, "pipelineObjects": definition}),
            )
            .await;
        }
        let described = ok(
            &router,
            "DescribePipelines",
            &json!({"pipelineIds": [pipeline_id]}),
        )
        .await;
        let reported = described["pipelineDescriptionList"][0]["fields"]
            .as_array()
            .unwrap_or_else(|| panic!("no fields in {described}"))
            .iter()
            .find(|field| field["key"] == json!("@version"))
            .and_then(|field| field["stringValue"].as_str().map(str::to_string));
        assert_eq!(
            reported.as_deref(),
            Some(expected_version),
            "after {} commit(s): wrong @version in {described}",
            expected_version
        );
    }

    // Absent and `latest` both report the last definition saved — the second one, so this
    // cannot pass by echoing the request or by keeping only the first commit.
    for version in [None, Some("latest")] {
        let mut body = json!({"pipelineId": pipeline_id});
        if let Some(version) = version {
            body["version"] = json!(version);
        }
        let fetched = ok(&router, "GetPipelineDefinition", &body).await;
        assert_eq!(
            fetched["pipelineObjects"], second,
            "version {version:?}: expected the last definition saved, got {fetched}"
        );
    }

    // Nothing has been activated (ActivatePipeline is not implemented), so `active`
    // reports no definition rather than the saved one.
    let active = ok(
        &router,
        "GetPipelineDefinition",
        &json!({"pipelineId": pipeline_id, "version": "active"}),
    )
    .await;
    assert!(
        active.get("pipelineObjects").is_none(),
        "version \"active\": nothing is activated, so no definition is reported, got {active}"
    );

    for version in ["LATEST", "1", "newest", " latest"] {
        let (status, body) = call(
            &router,
            "GetPipelineDefinition",
            &json!({"pipelineId": pipeline_id, "version": version}),
        )
        .await;
        assert_eq!(
            status, 400,
            "version {version:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "version {version:?}: got {body}"
        );
    }
}

/// Every pipeline-scoped operation resolves its `pipelineId` out of the store, and
/// answers with the error its own model declares for an id that names no live pipeline.
#[tokio::test]
async fn pipeline_scoped_operations_report_unknown_and_deleted_ids() {
    let router = emulator();
    let deleted_id = create(&router, "doomed", "doomed-1").await;
    ok(
        &router,
        "DeletePipeline",
        &json!({"pipelineId": deleted_id.clone()}),
    )
    .await;

    // Each row: the operation, the request body for a given id, the code expected for an
    // id that never existed, and the code expected for a deleted id.
    struct ErrorRow {
        operation: &'static str,
        body_for: fn(&str) -> Value,
        unknown_code: &'static str,
        deleted_code: &'static str,
    }

    let rows = vec![
        ErrorRow {
            operation: "DescribePipelines",
            body_for: |id| json!({"pipelineIds": [id]}),
            unknown_code: "PipelineNotFoundException",
            deleted_code: "PipelineDeletedException",
        },
        ErrorRow {
            operation: "GetPipelineDefinition",
            body_for: |id| json!({"pipelineId": id}),
            unknown_code: "PipelineNotFoundException",
            deleted_code: "PipelineDeletedException",
        },
        ErrorRow {
            operation: "PutPipelineDefinition",
            body_for: |id| {
                json!({"pipelineId": id, "pipelineObjects": [
                    {"id": "Default", "name": "Default",
                     "fields": [{"key": "scheduleType", "stringValue": "ondemand"}]}
                ]})
            },
            unknown_code: "PipelineNotFoundException",
            deleted_code: "PipelineDeletedException",
        },
        ErrorRow {
            // DeletePipeline models no PipelineDeletedException, so both are not-found.
            operation: "DeletePipeline",
            body_for: |id| json!({"pipelineId": id}),
            unknown_code: "PipelineNotFoundException",
            deleted_code: "PipelineNotFoundException",
        },
    ];

    for row in rows {
        let operation = row.operation;
        for (id, expected) in [
            (UNKNOWN_PIPELINE_ID, row.unknown_code),
            (deleted_id.as_str(), row.deleted_code),
        ] {
            let (status, body) = call(&router, operation, &(row.body_for)(id)).await;
            assert_eq!(
                status, 400,
                "{operation} on {id}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "{operation} on {id}: got {body}"
            );
        }
    }

    // DescribePipelines is all-or-nothing: one bad id in the list fails the call.
    let live_id = create(&router, "live", "live-1").await;
    let (status, body) = call(
        &router,
        "DescribePipelines",
        &json!({"pipelineIds": [live_id, UNKNOWN_PIPELINE_ID]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a list with one unknown id must fail, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "PipelineNotFoundException",
        "a list with one unknown id: got {body}"
    );
}

/// `CreatePipeline` serves creates up to the account quota and refuses past it, while an
/// idempotent repeat — which creates nothing — still succeeds at the quota.
#[tokio::test]
async fn create_pipeline_refuses_past_the_pipeline_quota() {
    let router = emulator();
    let mut ids = Vec::new();
    for index in 0..MAX_PIPELINES {
        ids.push(create(&router, &format!("p{index}"), &format!("u{index}")).await);
    }

    // Rows either side of the quota: the last create inside it already succeeded above,
    // and every further fresh uniqueId is refused.
    for index in [MAX_PIPELINES, MAX_PIPELINES + 1] {
        let (status, body) = call(
            &router,
            "CreatePipeline",
            &json!({"name": format!("p{index}"), "uniqueId": format!("u{index}")}),
        )
        .await;
        assert_eq!(
            status, 400,
            "create {index} past the quota: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "create {index} past the quota: got {body}"
        );
    }

    // A repeat of an existing pair creates nothing, so it is still served at the quota.
    let repeat = create(&router, "p0", "u0").await;
    assert_eq!(
        repeat, ids[0],
        "an idempotent repeat must still return {}, got {repeat}",
        ids[0]
    );

    // Deleting one frees a slot.
    ok(
        &router,
        "DeletePipeline",
        &json!({"pipelineId": ids[0].clone()}),
    )
    .await;
    let after_delete = create(&router, "fresh", "fresh-1").await;
    assert!(
        !ids.contains(&after_delete),
        "a delete must free a quota slot for a new pipeline, got {after_delete}"
    );
}

/// `DescribePipelines` serves a list up to the documented maximum and refuses a longer
/// one, so the number of descriptions one request can produce is bounded.
#[tokio::test]
async fn describe_pipelines_bounds_the_number_of_identifiers_per_call() {
    let router = emulator();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_PIPELINE_IDS + 5 {
        ids.push(create(&router, &format!("p{index}"), &format!("u{index}")).await);
    }

    // Rows either side of the bound: 1 and exactly the maximum are served, anything
    // longer is refused even though every id names a live pipeline.
    for count in [1, MAX_PIPELINE_IDS] {
        let described = ok(
            &router,
            "DescribePipelines",
            &json!({"pipelineIds": ids[..count]}),
        )
        .await;
        assert_eq!(
            described["pipelineDescriptionList"]
                .as_array()
                .map(Vec::len),
            Some(count),
            "count {count}: expected {count} descriptions, got {described}"
        );
    }
    for count in [MAX_PIPELINE_IDS + 1, MAX_PIPELINE_IDS + 5] {
        let (status, body) = call(
            &router,
            "DescribePipelines",
            &json!({"pipelineIds": ids[..count]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "count {count}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "count {count}: got {body}"
        );
    }
}
