// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a `Map` state's `ItemReader` and `ItemBatcher`, driven over
//! both services' own wire protocols through the production `MultiServiceRouter` with
//! Step Functions and S3 mounted on ONE engine — the shape `core/router/src/lib.rs`
//! mounts them in — so the dataset the reader reads is really the one S3's own
//! handlers stored.
//!
//! The rule under test: a `Map` state that declares an `ItemReader` iterates the S3
//! dataset that reader's `Resource` and `Parameters` name — the objects a bucket
//! prefix lists, or the CSV, JSON or JSON Lines document one object holds — bounded by
//! the reader's `MaxItems`; a read that could not be made fails the state with
//! `States.ItemReaderFailed` before a single iteration runs; and an `ItemBatcher` then
//! groups whatever was read into the batches its iterations are handed, each carrying
//! its `Items` and the state's `BatchInput`.
//!
//! Each table spans what the rule generalises over: the dataset's FORM (a listing, and
//! each `InputType` of one object), its SIZE (none, one, several, more than the bound),
//! the CSV reading (where the headers come from, which delimiter), the bound on how
//! many items are delivered, and the bound on how many go in one batch. The recorded
//! cases the issue was filed about — a Distributed Map over a bucket prefix, and a CSV
//! item reader delivering header-keyed string columns — are each ONE row.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN};

/// Step Functions' wire facts, from its model's own `service_info`: the service shape
/// the `X-Amz-Target` prefix is, its `sigv4Name`, and the `awsJson` version its
/// protocol declares.
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";
const SFN_SIGNING_NAME: &str = "states";
const SFN_JSON_VERSION: &str = "1.0";

/// S3's signing name, which is what routes a request to it
/// (`smithy_query(query="service_info")` on its model: `sigv4Name` and
/// `endpointPrefix` are both `s3`).
const S3_SIGNING_NAME: &str = "s3";

/// Step Functions and S3 over ONE engine, so the objects this suite writes through
/// S3's API are the ones a `Map` state's `ItemReader` reads.
struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// Call one Step Functions operation, requiring a 200.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SFN_SIGNING_NAME}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{SFN_SIGNING_NAME}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{SFN_JSON_VERSION}"),
            )
            .header("x-amz-target", format!("{SFN_TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Put one S3 request on the wire, PATH-STYLE against the Region's own endpoint —
    /// the way a client with no bucket in its host addresses S3 — and report its
    /// status and body.
    async fn s3(&self, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let host = format!("{S3_SIGNING_NAME}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{S3_SIGNING_NAME}/\
             aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("authorization", authorization)
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }

    /// A bucket in this Region to hold a dataset.
    async fn bucket(&self, name: &str) {
        let configuration = format!(
            "<CreateBucketConfiguration><LocationConstraint>{REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        );
        let (status, body) = self
            .s3("PUT", &format!("/{name}"), configuration.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "the bucket {name:?} must be created first, got {status} with body {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    /// One object of a dataset, stored through S3's own `PutObject`.
    async fn object(&self, bucket: &str, key: &str, body: &str) {
        let (status, answered) = self
            .s3("PUT", &format!("/{bucket}/{key}"), body.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "the object {key:?} must be stored first, got {status} with body {:?}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// Create a state machine, start one execution of it and report what the execution
    /// ended as once the clock has made the outcome due.
    async fn run(&self, name: &str, definition: &Value, input: &Value) -> Value {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": input.to_string(),
                }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        self.sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await
    }

    /// Every history event of one execution, in order.
    async fn history(&self, execution_arn: &str) -> Vec<Value> {
        let answered = self
            .sfn(
                "GetExecutionHistory",
                &json!({ "executionArn": execution_arn, "maxResults": 1000 }),
            )
            .await;
        answered
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("expected an events array in {answered:?}"))
    }
}

/// A `Map` state whose items come from `reader`, running `processor` over each one.
fn mapping(reader: Value, extra: Value) -> Value {
    let mut state = json!({
        "Type": "Map",
        "ItemReader": reader,
        "ItemProcessor": {
            "ProcessorConfig": { "Mode": "DISTRIBUTED" },
            "StartAt": "Keep",
            "States": { "Keep": { "Type": "Pass", "End": true } },
        },
        "End": true,
    });
    let members = state.as_object_mut().expect("the state is an object");
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        members.insert(member.clone(), value.clone());
    }
    json!({ "StartAt": "Fan", "States": { "Fan": state } })
}

/// An `ItemReader` over the objects of `bucket` under `prefix`.
fn listing(bucket: &str, prefix: Option<&str>) -> Value {
    let mut parameters = json!({ "Bucket": bucket });
    if let Some(prefix) = prefix {
        parameters
            .as_object_mut()
            .expect("the parameters are an object")
            .insert("Prefix".to_string(), json!(prefix));
    }
    json!({
        "Resource": "arn:aws:states:::s3:listObjectsV2",
        "Parameters": parameters,
    })
}

/// An `ItemReader` over the one object `key` names, read as `config` says.
fn one_object(bucket: &str, key: &str, config: Value) -> Value {
    json!({
        "Resource": "arn:aws:states:::s3:getObject",
        "ReaderConfig": config,
        "Parameters": { "Bucket": bucket, "Key": key },
    })
}

/// The `output` a succeeded execution reports, parsed.
fn output(described: &Value, case: &str) -> Value {
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described:?}"
    );
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"))
}

/// The CSV dataset every CSV row below reads, and the items its columns name.
const RATINGS_CSV: &str = "name,rating\nalpha,5\nbeta,3\ngamma,1\n";

/// The same three rows with no header row of their own, tab-separated.
const RATINGS_TSV: &str = "alpha\t5\nbeta\t3\ngamma\t1\n";

/// The items both of those datasets hold, as the reader delivers them: every column a
/// STRING, keyed by its header.
fn ratings_items() -> Value {
    json!([
        { "name": "alpha", "rating": "5" },
        { "name": "beta", "rating": "3" },
        { "name": "gamma", "rating": "1" },
    ])
}

// ---------------------------------------------------------------------------
// The dataset is read, and its items are the ones S3 holds
// ---------------------------------------------------------------------------

/// A `Map` state's `ItemReader` iterates the dataset it names, whichever form that
/// dataset takes.
///
/// The rows differ along what the rule generalises over: the dataset's FORM (an object
/// listing, and each `InputType` of a single object), its SIZE (one item, several,
/// none at all), where a CSV's member names come from (`FIRST_ROW` and `GIVEN`), and
/// which delimiter separates its columns.
#[tokio::test]
async fn an_item_reader_iterates_the_dataset_it_names() {
    let chain = Chain::new();
    chain.bucket("datasets").await;
    chain.object("datasets", "csv/ratings.csv", RATINGS_CSV).await;
    chain.object("datasets", "csv/ratings.tsv", RATINGS_TSV).await;
    chain
        .object("datasets", "json/orders.json", "[{\"id\":1},{\"id\":2}]")
        .await;
    chain
        .object(
            "datasets",
            "json/orders.jsonl",
            "{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n",
        )
        .await;
    chain.object("datasets", "json/empty.json", "[]").await;
    chain.object("datasets", "objects/one.txt", "first").await;
    chain.object("datasets", "objects/two.txt", "second!").await;

    // Every row's processor keeps the item it was handed, so the execution's output IS
    // the array of items the reader delivered.
    let rows: &[(&str, Value, Value)] = &[
        (
            // The recorded case: a Distributed Map over the objects of one prefix.
            "the objects of one prefix",
            listing("datasets", Some("objects/")),
            json!([
                { "Key": "objects/one.txt", "Size": 5, "StorageClass": "STANDARD" },
                { "Key": "objects/two.txt", "Size": 7, "StorageClass": "STANDARD" },
            ]),
        ),
        (
            "a prefix holding one object",
            listing("datasets", Some("csv/ratings.tsv")),
            json!([{ "Key": "csv/ratings.tsv", "Size": RATINGS_TSV.len(), "StorageClass": "STANDARD" }]),
        ),
        (
            "a prefix holding none",
            listing("datasets", Some("nothing/")),
            json!([]),
        ),
        (
            // The recorded case: a CSV item reader delivering header-keyed string
            // columns.
            "a CSV object whose first row is its header",
            one_object("datasets", "csv/ratings.csv", json!({ "InputType": "CSV" })),
            ratings_items(),
        ),
        (
            "a CSV object whose headers the definition gives, tab separated",
            one_object(
                "datasets",
                "csv/ratings.tsv",
                json!({
                    "InputType": "CSV",
                    "CSVHeaderLocation": "GIVEN",
                    "CSVHeaders": ["name", "rating"],
                    "CSVDelimiter": "TAB",
                }),
            ),
            ratings_items(),
        ),
        (
            "a JSON object holding an array",
            one_object("datasets", "json/orders.json", json!({ "InputType": "JSON" })),
            json!([{ "id": 1 }, { "id": 2 }]),
        ),
        (
            "a JSON object holding an empty array",
            one_object("datasets", "json/empty.json", json!({ "InputType": "JSON" })),
            json!([]),
        ),
        (
            "a JSON Lines object, one item per line",
            one_object(
                "datasets",
                "json/orders.jsonl",
                json!({ "InputType": "JSONL" }),
            ),
            json!([{ "id": 1 }, { "id": 2 }, { "id": 3 }]),
        ),
    ];

    for (index, (case, reader, expected)) in rows.iter().enumerate() {
        let described = chain
            .run(
                &format!("reader-{index}"),
                &mapping(reader.clone(), json!({})),
                &json!({}),
            )
            .await;
        let produced = output(&described, case);
        // A listing's items carry the object's own `Etag` and `LastModified` beside the
        // members asserted, and those are the stored object's facts rather than this
        // table's: what is compared is every member the table names.
        let produced = narrowed(&produced, expected);
        assert_eq!(
            produced, *expected,
            "case {case:?}: expected the items {expected}, got {produced}"
        );
    }
}

/// `produced` with each item cut back to the members the matching item of `expected`
/// names, so a table row asserts the members it states and says nothing about the ones
/// it does not.
fn narrowed(produced: &Value, expected: &Value) -> Value {
    let (Some(produced), Some(expected)) = (produced.as_array(), expected.as_array()) else {
        return produced.clone();
    };
    Value::Array(
        produced
            .iter()
            .zip(expected)
            .map(|(item, wanted)| match (item.as_object(), wanted.as_object()) {
                (Some(item), Some(wanted)) => Value::Object(
                    wanted
                        .keys()
                        .filter_map(|member| {
                            Some((member.clone(), item.get(member)?.clone()))
                        })
                        .collect(),
                ),
                _ => item.clone(),
            })
            .collect(),
    )
}

/// Every object of a listing's items is the stored object's own metadata: the `Etag`
/// and the `LastModified` S3 answered for it, beside its key and size.
#[tokio::test]
async fn a_listings_items_carry_the_stored_objects_own_metadata() {
    let chain = Chain::new();
    chain.bucket("metadata").await;
    chain.object("metadata", "only.txt", "payload").await;

    let described = chain
        .run(
            "metadata-items",
            &mapping(listing("metadata", None), json!({})),
            &json!({}),
        )
        .await;
    let produced = output(&described, "one stored object");
    let items = produced.as_array().expect("the output is an array");
    assert_eq!(items.len(), 1, "one stored object is one item, got {produced}");
    let item = &items[0];
    for member in ["Etag", "Key", "LastModified", "Size", "StorageClass"] {
        assert!(
            item.get(member).is_some(),
            "the item carries the object's {member}, got {item}"
        );
    }
    assert_eq!(
        item.get("Size").and_then(Value::as_i64),
        Some(7),
        "Size is the stored byte length, got {item}"
    );
    assert!(
        item.get("LastModified")
            .and_then(Value::as_i64)
            .is_some_and(|seconds| seconds > 0),
        "LastModified is the instant S3 answered, in whole epoch seconds, got {item}"
    );
    assert!(
        item.get("Etag")
            .and_then(Value::as_str)
            .is_some_and(|etag| etag.starts_with('"')),
        "Etag is the entity tag S3 answered, got {item}"
    );
}

// ---------------------------------------------------------------------------
// MaxItems
// ---------------------------------------------------------------------------

/// "MaxItems — limits the number of data items passed", and `MaxItemsPath` reads that
/// figure from the state's own input instead: the reader delivers at most that many,
/// whichever dataset it read, and every item when neither member is declared.
///
/// The rows differ along the bound (absent, one, below the dataset's size, exactly it,
/// past it), along WHERE the bound is written (the literal member and the `*Path` form
/// reading the execution input) and along the dataset the bound is applied to (an object
/// listing, whose items the reader pages, and a CSV object, whose items are its rows).
#[tokio::test]
async fn max_items_bounds_how_many_items_the_reader_delivers() {
    let chain = Chain::new();
    chain.bucket("bounded").await;
    chain.object("bounded", "rows/ratings.csv", RATINGS_CSV).await;
    for key in ["list/a.txt", "list/b.txt", "list/c.txt"] {
        chain.object("bounded", key, "x").await;
    }

    // Both datasets hold THREE items, so one table of bounds covers each.
    let bounds: &[(&str, Option<u64>, usize)] = &[
        ("no bound", None, 3),
        ("a bound of one", Some(1), 1),
        ("a bound below the item count", Some(2), 2),
        ("a bound equal to the item count", Some(3), 3),
        ("a bound past the item count", Some(9), 3),
    ];
    for (index, (case, bound, expected)) in bounds.iter().enumerate() {
        for (dataset, reader) in [
            ("a listing", listing("bounded", Some("list/"))),
            (
                "a CSV object",
                one_object("bounded", "rows/ratings.csv", json!({ "InputType": "CSV" })),
            ),
        ] {
            // The same bound written both ways: as the literal member, and as the path
            // that reads it out of the execution input.
            for (written, member) in [("MaxItems", false), ("MaxItemsPath", true)] {
                let described = chain
                    .run(
                        &format!("bound-{index}-{}-{written}", dataset.len()),
                        &mapping(bounded(reader.clone(), *bound, member), json!({})),
                        &json!({ "limit": bound }),
                    )
                    .await;
                let produced = output(&described, case);
                let delivered = produced.as_array().map_or(0, Vec::len);
                assert_eq!(
                    delivered, *expected,
                    "case {case:?} over {dataset}, written as {written}: expected {expected} \
                     items, got {produced}"
                );
            }
        }
    }
}

/// `reader` with its `MaxItems` declared in its `ReaderConfig` — as the literal member,
/// or as the `MaxItemsPath` that reads the figure from the state's input — when the row
/// names a bound; unchanged when it names none, which is both members absent.
fn bounded(reader: Value, bound: Option<u64>, by_path: bool) -> Value {
    if bound.is_none() {
        return reader;
    }
    let mut reader = reader;
    let members = reader.as_object_mut().expect("the reader is an object");
    let mut config = members
        .get("ReaderConfig")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let config_members = config
        .as_object_mut()
        .expect("the reader config is an object");
    match by_path {
        true => config_members.insert("MaxItemsPath".to_string(), json!("$.limit")),
        false => config_members.insert("MaxItems".to_string(), json!(bound)),
    };
    members.insert("ReaderConfig".to_string(), config);
    reader
}

// ---------------------------------------------------------------------------
// A read that could not be made
// ---------------------------------------------------------------------------

/// A dataset that could not be read fails the `Map` state with
/// `States.ItemReaderFailed`, and the state starts NOTHING: no iteration is recorded
/// in the execution's history.
///
/// The rows differ along what made the read impossible: an object nobody stored, a
/// bucket nobody created, and a document whose bytes are not what the declared
/// `InputType` describes.
#[tokio::test]
async fn a_dataset_that_cannot_be_read_fails_the_state_before_any_iteration() {
    let chain = Chain::new();
    chain.bucket("partial").await;
    chain
        .object("partial", "object.json", "{\"id\":1}")
        .await;
    chain.object("partial", "rows.csv", "name\nalpha\n").await;

    let rows: &[(&str, Value)] = &[
        (
            "an object nobody stored",
            one_object("partial", "missing.json", json!({ "InputType": "JSON" })),
        ),
        (
            "a bucket nobody created",
            one_object("absent", "any.json", json!({ "InputType": "JSON" })),
        ),
        (
            "a listing of a bucket nobody created",
            listing("absent", None),
        ),
        (
            "a JSON dataset that is not an array",
            one_object("partial", "object.json", json!({ "InputType": "JSON" })),
        ),
        (
            "a JSON Lines dataset whose lines are not JSON",
            one_object("partial", "rows.csv", json!({ "InputType": "JSONL" })),
        ),
    ];

    for (index, (case, reader)) in rows.iter().enumerate() {
        let name = format!("unreadable-{index}");
        let created = chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": mapping(reader.clone(), json!({})).to_string(),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = chain
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let described = chain
            .sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some("States.ItemReaderFailed"),
            "case {case:?}: expected States.ItemReaderFailed, got {described:?}"
        );
        assert!(
            described
                .get("cause")
                .and_then(Value::as_str)
                .is_some_and(|cause| cause.contains("'ItemReader' of state 'Fan'")),
            "case {case:?}: the cause names the state's own reader, got {described:?}"
        );
        let history = chain.history(&execution).await;
        let iterations: Vec<&Value> = history
            .iter()
            .filter(|event| {
                event
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|event_type| event_type.starts_with("MapIteration"))
            })
            .collect();
        assert!(
            iterations.is_empty(),
            "case {case:?}: a state whose dataset could not be read starts no iteration, \
             got {iterations:?}"
        );
    }
}

/// A `Map` state whose dataset could not be read is CAUGHT like any other failed
/// state: `States.ItemReaderFailed` carries an error name, so a `Catch` for it runs.
#[tokio::test]
async fn a_failed_read_is_caught_by_a_catcher_that_names_it() {
    let chain = Chain::new();
    chain.bucket("caught").await;

    let state = mapping(
        one_object("caught", "missing.json", json!({ "InputType": "JSON" })),
        json!({
            "Catch": [{ "ErrorEquals": ["States.ItemReaderFailed"], "Next": "Report" }],
            "End": true,
        }),
    );
    let mut definition = state;
    definition
        .get_mut("States")
        .and_then(Value::as_object_mut)
        .expect("the states are an object")
        .insert(
            "Report".to_string(),
            json!({ "Type": "Pass", "Result": "caught", "End": true }),
        );

    let described = chain.run("caught-read", &definition, &json!({})).await;
    assert_eq!(
        output(&described, "a catcher naming the reader's error"),
        json!("caught"),
        "a catcher that names States.ItemReaderFailed catches it, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// ItemBatcher
// ---------------------------------------------------------------------------

/// "ItemBatcher — specifies how to batch the items": each iteration is handed one
/// BATCH, carrying the items of that batch and the state's own `BatchInput`.
///
/// The rows differ along the bound (one item per batch, several, more than the dataset
/// holds, none at all), and along whether the state declares a `BatchInput`.
#[tokio::test]
async fn an_item_batcher_groups_the_items_into_the_batches_it_declares() {
    let chain = Chain::new();
    chain.bucket("batched").await;
    chain
        .object(
            "batched",
            "five.jsonl",
            "{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n{\"id\":4}\n{\"id\":5}\n",
        )
        .await;
    let reader = one_object("batched", "five.jsonl", json!({ "InputType": "JSONL" }));

    let rows: &[(&str, Value, Value)] = &[
        (
            "one item per batch",
            json!({ "MaxItemsPerBatch": 1 }),
            json!([
                { "Items": [{ "id": 1 }] },
                { "Items": [{ "id": 2 }] },
                { "Items": [{ "id": 3 }] },
                { "Items": [{ "id": 4 }] },
                { "Items": [{ "id": 5 }] },
            ]),
        ),
        (
            "two per batch leaves a short last batch",
            json!({ "MaxItemsPerBatch": 2 }),
            json!([
                { "Items": [{ "id": 1 }, { "id": 2 }] },
                { "Items": [{ "id": 3 }, { "id": 4 }] },
                { "Items": [{ "id": 5 }] },
            ]),
        ),
        (
            "a bound past the item count is one batch",
            json!({ "MaxItemsPerBatch": 50 }),
            json!([{ "Items": [{ "id": 1 }, { "id": 2 }, { "id": 3 }, { "id": 4 }, { "id": 5 }] }]),
        ),
        (
            "no bound at all is one batch",
            json!({}),
            json!([{ "Items": [{ "id": 1 }, { "id": 2 }, { "id": 3 }, { "id": 4 }, { "id": 5 }] }]),
        ),
        (
            "a BatchInput is carried by every batch",
            json!({ "MaxItemsPerBatch": 3, "BatchInput": { "run": "r-1" } }),
            json!([
                { "BatchInput": { "run": "r-1" }, "Items": [{ "id": 1 }, { "id": 2 }, { "id": 3 }] },
                { "BatchInput": { "run": "r-1" }, "Items": [{ "id": 4 }, { "id": 5 }] },
            ]),
        ),
    ];

    for (index, (case, batcher, expected)) in rows.iter().enumerate() {
        let described = chain
            .run(
                &format!("batcher-{index}"),
                &mapping(reader.clone(), json!({ "ItemBatcher": batcher })),
                &json!({}),
            )
            .await;
        let produced = output(&described, case);
        assert_eq!(
            produced, *expected,
            "case {case:?}: expected the batches {expected}, got {produced}"
        );
    }
}

/// A state declaring BOTH an `ItemSelector` and an `ItemBatcher` applies the selector
/// to each item FIRST, and batches what it produced.
#[tokio::test]
async fn an_item_selector_is_applied_to_each_item_before_it_is_batched() {
    let chain = Chain::new();
    chain.bucket("selected").await;
    chain
        .object(
            "selected",
            "three.jsonl",
            "{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n",
        )
        .await;

    let described = chain
        .run(
            "selected-batches",
            &mapping(
                one_object("selected", "three.jsonl", json!({ "InputType": "JSONL" })),
                json!({
                    "ItemSelector": {
                        "at.$": "$$.Map.Item.Index",
                        "id.$": "$$.Map.Item.Value.id",
                    },
                    "ItemBatcher": { "MaxItemsPerBatch": 2 },
                }),
            ),
            &json!({}),
        )
        .await;
    assert_eq!(
        output(&described, "a selector applied before batching"),
        json!([
            { "Items": [{ "at": 0, "id": 1 }, { "at": 1, "id": 2 }] },
            { "Items": [{ "at": 2, "id": 3 }] },
        ]),
        "the batches hold the selector's projection of each item, got {described:?}"
    );
}

/// Each child workflow execution of a BATCHED Distributed Map reports the number of
/// items its own batch held — "the total number of items processed in a child workflow
/// execution".
#[tokio::test]
async fn a_batched_childs_item_count_is_the_size_of_its_batch() {
    let chain = Chain::new();
    chain.bucket("counted").await;
    chain
        .object(
            "counted",
            "five.jsonl",
            "{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n{\"id\":4}\n{\"id\":5}\n",
        )
        .await;

    let created = chain
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": "counted-children",
                "definition": mapping(
                    one_object("counted", "five.jsonl", json!({ "InputType": "JSONL" })),
                    json!({ "ItemBatcher": { "MaxItemsPerBatch": 2 } }),
                )
                .to_string(),
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let machine = string(&created, "stateMachineArn");
    let started = chain
        .sfn(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
        )
        .await;
    let execution = string(&started, "executionArn");
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    let runs = chain
        .sfn("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let map_runs = runs
        .get("mapRuns")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a mapRuns array in {runs:?}"));
    assert_eq!(
        map_runs.len(),
        1,
        "a Distributed Map state starts one Map Run, got {runs:?}"
    );
    let map_run_arn = string(&map_runs[0], "mapRunArn");
    let listed = chain
        .sfn("ListExecutions", &json!({ "mapRunArn": map_run_arn }))
        .await;
    let children = listed
        .get("executions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an executions array in {listed:?}"));
    let counts: Vec<i64> = children
        .iter()
        .filter_map(|child| child.get("itemCount").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        counts.len(),
        children.len(),
        "every child of a Map Run reports an itemCount, got {listed:?}"
    );
    let mut sorted = counts.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        vec![1, 2, 2],
        "five items batched two at a time are three children of 2, 2 and 1 items, got \
         {counts:?}"
    );
}
