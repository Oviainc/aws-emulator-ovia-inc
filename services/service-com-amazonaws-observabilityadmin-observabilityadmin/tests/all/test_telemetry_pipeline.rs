// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `TestTelemetryPipeline`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handler — through the
//! ONE harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. the operation runs the processor chain the request's `Configuration` declares
//!    over each sample record and answers ONE `PipelineOutput` per record, in request
//!    order: the record as the chain left it, or that record's own `Error` when the
//!    chain could not process it — so one bad record in a batch costs only its own
//!    result;
//! 2. a record is read into the chain in the form its `Type` names (a `JSON` document,
//!    or a `STRING`'s text in the `message` field the processors of this format
//!    default their `source` to) and reported back in the form it then holds, so a
//!    definition declaring no processor is the identity on every record;
//! 3. each processor does what this pipeline format says it does, over the inputs the
//!    rule generalises over: the column names a `csv` row is read with, the field a
//!    `parse_json` document is merged into, the keys a per-field processor names — and
//!    a processor that PARSES a field is consumed by what it produced, so a row of
//!    comma-separated values comes back as its columns and not beside the line they
//!    were read from;
//! 4. a configuration this emulator cannot run — a processor or an option it does not
//!    implement, a document that is not a pipeline definition, one past a bound — is a
//!    modelled 400 `ValidationException` that processes no record, never a success
//!    carrying records nothing transformed;
//! 5. nothing is stored: the call creates no pipeline and leaves the store as it found
//!    it.

use serde_json::{json, Map, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::ops::test_telemetry_pipeline::{
    MAX_COMPILED_PATTERNS, MAX_PROCESSORS, MAX_PROCESSOR_ENTRIES, MAX_RECORD_DATA_BYTES,
    MAX_TEST_RECORDS, MAX_TOTAL_RECORD_BYTES, MAX_WORK_BYTES,
};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::ComAmazonawsObservabilityadminObservabilityadminState;

use crate::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// The region every request below signs for.
const HOME_REGION: &str = "us-west-2";

/// One `TestTelemetryPipeline` call.
async fn test_pipeline(emulator: &Emulator, body: &Value) -> (u16, Value) {
    call(emulator, HOME_REGION, "/TestTelemetryPipeline", body).await
}

/// A request body carrying `configuration` and `records`.
fn request(configuration: &str, records: Value) -> Value {
    json!({
        "Configuration": { "Body": configuration },
        "Records": records,
    })
}

/// One `STRING` sample record, the form a raw log line arrives in.
fn text_record(data: &str) -> Value {
    json!({ "Data": data, "Type": "STRING" })
}

/// One `JSON` sample record.
fn json_record(data: &str) -> Value {
    json!({ "Data": data, "Type": "JSON" })
}

/// The `Results` list of a successful call.
fn results(response: &Value) -> Vec<Value> {
    response
        .get("Results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Record` of one result, as `(Data, Type)`.
fn record_of(result: &Value) -> Option<(String, String)> {
    let record = result.get("Record")?;
    Some((
        record.get("Data")?.as_str()?.to_string(),
        record.get("Type")?.as_str()?.to_string(),
    ))
}

/// The `Error.Message` of one result.
fn error_of(result: &Value) -> Option<String> {
    Some(
        result
            .get("Error")?
            .get("Message")?
            .as_str()?
            .to_string(),
    )
}

/// The fields of one result's record, which a processed record reports as a JSON
/// document.
fn fields_of(case: &str, result: &Value) -> Map<String, Value> {
    let (data, record_type) = record_of(result)
        .unwrap_or_else(|| panic!("case {case:?}: expected a processed record, got {result:?}"));
    assert_eq!(
        record_type, "JSON",
        "case {case:?}: a processed record is reported as JSON, got {record_type} ({data})"
    );
    match serde_json::from_str::<Value>(&data) {
        Ok(Value::Object(fields)) => fields,
        other => panic!("case {case:?}: expected a JSON object, got {other:?} from {data}"),
    }
}

/// A pipeline definition whose only processor is `processor`, written at the top level.
fn definition(processor: &str) -> String {
    format!(
        "source:\n  \
           http: {{}}\n\
         processor:\n\
         {processor}\
         sink:\n  \
           - cloudwatch_logs:\n      \
               log_group: /aws/telemetry/example\n"
    )
}

// =============================================================================
// The csv processor
// =============================================================================

/// The `csv` processor reads the source field as one CSV row and puts each column in
/// the record under its column name, in place of the text it read them from: the names
/// the configuration lists, the header the `column_names_source_key` field carries when
/// one is named, and `column_<N>` for a column past the last name — which is also how
/// every column is named when the configuration lists none.
///
/// The rows differ along what that rule generalises over: how many names are
/// configured against how many columns the row has, which delimiter and quote
/// character separate them, whether the names come from the record itself, and which
/// field the row is read out of.
#[tokio::test]
async fn the_csv_processor_names_every_column_it_reads() {
    let emulator = emulator();

    for (case, processor, record, expected) in [
        (
            "a name per column",
            "  - csv:\n      column_names: [\"version\", \"account_id\", \"action\"]\n",
            text_record("2,000000000000,ACCEPT"),
            json!({ "version": "2", "account_id": "000000000000", "action": "ACCEPT" }),
        ),
        (
            "no names configured, so every column is named by its position",
            "  - csv: {}\n",
            text_record("2,000000000000,ACCEPT"),
            json!({ "column_1": "2", "column_2": "000000000000", "column_3": "ACCEPT" }),
        ),
        (
            "fewer names than columns, so the columns past the last name are positional",
            "  - csv:\n      column_names: [\"version\"]\n",
            text_record("2,000000000000,ACCEPT"),
            json!({ "version": "2", "column_2": "000000000000", "column_3": "ACCEPT" }),
        ),
        (
            "more names than columns, so a name with no column names nothing",
            "  - csv:\n      column_names: [\"version\", \"account_id\", \"action\"]\n",
            text_record("2,000000000000"),
            json!({ "version": "2", "account_id": "000000000000" }),
        ),
        (
            "a space delimiter, as a flow log is written",
            "  - csv:\n      delimiter: \" \"\n      column_names: [\"srcaddr\", \"dstaddr\"]\n",
            text_record("10.0.0.1 10.0.0.2"),
            json!({ "srcaddr": "10.0.0.1", "dstaddr": "10.0.0.2" }),
        ),
        (
            "a quoted field carrying the delimiter is one column",
            "  - csv:\n      column_names: [\"path\", \"status\"]\n",
            text_record("\"/a,b\",200"),
            json!({ "path": "/a,b", "status": "200" }),
        ),
        (
            "a configured quote character",
            "  - csv:\n      quote_character: \"'\"\n      column_names: [\"path\", \"status\"]\n",
            text_record("'/a,b',200"),
            json!({ "path": "/a,b", "status": "200" }),
        ),
        (
            "the column names come from a field of the record itself",
            "  - csv:\n      column_names_source_key: \"header\"\n",
            json!({
                "Data": "{\"message\":\"2,ACCEPT\",\"header\":\"version,action\"}",
                "Type": "JSON"
            }),
            json!({ "version": "2", "action": "ACCEPT" }),
        ),
        (
            "a source field other than the default, leaving the record's other fields",
            "  - csv:\n      source: \"raw\"\n      column_names: [\"first\", \"second\"]\n",
            json_record("{\"raw\":\"a,b\",\"kept\":1}"),
            json!({ "first": "a", "second": "b", "kept": 1 }),
        ),
    ] {
        let (status, response) =
            test_pipeline(&emulator, &request(&definition(processor), json!([record]))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let outputs = results(&response);
        assert_eq!(
            outputs.len(),
            1,
            "case {case:?}: expected one result, got {outputs:?}"
        );
        let Some(result) = outputs.first() else {
            panic!("case {case:?}: expected one result, got {outputs:?}");
        };
        let fields = fields_of(case, result);
        // The columns ARE the record: the text they were read from is consumed by
        // them, and a header the names came from is not left behind either.
        assert_eq!(
            Value::Object(fields.clone()),
            expected,
            "case {case:?}: expected {expected:?}, got {fields:?}"
        );
    }
}

/// A row whose source field is not in the record at all is THAT record's error: the
/// `csv` processor exists to give the record its structure, so a record it never read
/// is not a processed record.
#[tokio::test]
async fn a_csv_row_the_processor_cannot_read_is_that_records_error() {
    let emulator = emulator();
    let configuration = definition("  - csv:\n      source: \"absent\"\n");

    let (status, response) = test_pipeline(
        &emulator,
        &request(&configuration, json!([text_record("a,b")])),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let outputs = results(&response);
    let Some(result) = outputs.first() else {
        panic!("expected one result, got {outputs:?}");
    };
    assert!(
        record_of(result).is_none(),
        "a record the chain could not process carries no Record, got {result:?}"
    );
    let message = error_of(result).unwrap_or_default();
    assert!(
        message.contains("absent"),
        "the error names the field the processor could not read, got {message:?}"
    );
}

// =============================================================================
// One result per record, independently
// =============================================================================

/// One `PipelineOutput` per input record, in request order, each carrying its OWN
/// outcome: a record the chain could not process is an `Error` in its own position and
/// costs no other record its result.
///
/// The rows differ along the batch size and along which positions hold a record the
/// chain cannot read, so the independence is asserted over a set rather than on one
/// recorded batch.
#[tokio::test]
async fn every_record_of_a_batch_gets_its_own_result() {
    let emulator = emulator();
    // `parse_json` reads the record's text as a JSON document, so a record carrying
    // text that is not one is a record this chain cannot process.
    let configuration = definition("  - parse_json: {}\n");

    for (case, records, failing) in [
        ("one good record", vec!["{\"a\":1}"], vec![]),
        ("one bad record", vec!["not json"], vec![0usize]),
        (
            "a bad record between two good ones",
            vec!["{\"a\":1}", "not json", "{\"b\":2}"],
            vec![1usize],
        ),
        (
            "bad records at both ends",
            vec!["[", "{\"a\":1}", "}"],
            vec![0usize, 2usize],
        ),
        (
            "every record bad",
            vec!["x", "y", "z", "w"],
            vec![0usize, 1usize, 2usize, 3usize],
        ),
        (
            "five good records",
            vec![
                "{\"a\":1}",
                "{\"a\":2}",
                "{\"a\":3}",
                "{\"a\":4}",
                "{\"a\":5}",
            ],
            vec![],
        ),
    ] {
        let body = request(
            &configuration,
            Value::Array(records.iter().map(|data| text_record(data)).collect()),
        );
        let (status, response) = test_pipeline(&emulator, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let outputs = results(&response);
        assert_eq!(
            outputs.len(),
            records.len(),
            "case {case:?}: expected one result per record, got {outputs:?}"
        );
        for (index, result) in outputs.iter().enumerate() {
            if failing.contains(&index) {
                assert!(
                    record_of(result).is_none() && error_of(result).is_some(),
                    "case {case:?}: record {index} cannot be processed, so its result is an \
                     error, got {result:?}"
                );
                continue;
            }
            let fields = fields_of(case, result);
            let Some(expected) = records.get(index) else {
                panic!("case {case:?}: no record at {index}");
            };
            // `parse_json` is consumed by the document it parsed, so the processed
            // record is exactly that document.
            let expected_fields: Value = serde_json::from_str(expected)
                .unwrap_or_else(|why| panic!("case {case:?}: test input is JSON: {why}"));
            assert_eq!(
                Value::Object(fields.clone()),
                expected_fields,
                "case {case:?}: record {index} is the document the chain parsed, got {fields:?}"
            );
            assert!(
                error_of(result).is_none(),
                "case {case:?}: record {index} was processed, so it carries no error, got \
                 {result:?}"
            );
        }
    }
}

/// A definition declaring no processor is the identity on every record: each one comes
/// back in the form it went in, whatever that form is. The rows span both record forms
/// and both ways a definition can declare its sections.
#[tokio::test]
async fn a_definition_with_no_processor_reports_every_record_unchanged() {
    let emulator = emulator();

    let configurations = [
        (
            "top-level sections",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
        ),
        (
            "a named section",
            "version: \"2\"\nlog-pipeline:\n  source:\n    http: {}\n  sink:\n    \
             - cloudwatch_logs: {}\n"
                .to_string(),
        ),
        (
            "a section declaring an empty processor sequence",
            "source:\n  http: {}\nprocessor: []\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
        ),
    ];
    let records = [
        ("a plain log line", text_record("GET /index.html 200")),
        ("an empty line", text_record("")),
        ("a line that looks like JSON", text_record("{\"a\": 1}")),
        ("a JSON document", json_record("{\"a\":1,\"b\":[1,2]}")),
        (
            "a JSON document carrying a message field",
            json_record("{\"message\":\"hello\"}"),
        ),
        ("a record with no Type", json!({ "Data": "no type here" })),
    ];

    for (configuration_case, configuration) in &configurations {
        for (record_case, record) in &records {
            let case = format!("{configuration_case} / {record_case}");
            let (status, response) =
                test_pipeline(&emulator, &request(configuration, json!([record]))).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
            let outputs = results(&response);
            let Some(result) = outputs.first() else {
                panic!("case {case:?}: expected one result, got {outputs:?}");
            };
            let (data, record_type) = record_of(result).unwrap_or_else(|| {
                panic!("case {case:?}: expected a processed record, got {result:?}")
            });
            let expected_data = record
                .get("Data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let expected_type = record
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or("STRING");
            assert_eq!(
                data, expected_data,
                "case {case:?}: an empty chain changes no record, got {result:?}"
            );
            assert_eq!(
                record_type, expected_type,
                "case {case:?}: a record keeps the form it arrived in, got {result:?}"
            );
        }
    }
}

// =============================================================================
// The rest of the processor set
// =============================================================================

/// Each processor this emulator runs does what this pipeline format says it does. The
/// rows differ along the processor, the keys it names and the values it reads, and
/// every one of them is asserted on the record the chain produced rather than on the
/// status alone.
#[tokio::test]
async fn each_processor_transforms_the_record_it_names() {
    let emulator = emulator();

    for (case, processor, record, expected) in [
        (
            "add_entries adds a field",
            "  - add_entries:\n      entries:\n        - key: stage\n          value: test\n",
            text_record("hello"),
            json!({ "message": "hello", "stage": "test" }),
        ),
        (
            "add_entries resolves a plain scalar by YAML's core schema",
            "  - add_entries:\n      entries:\n        - key: retries\n          value: 3\n        \
             - key: enabled\n          value: true\n",
            text_record("hello"),
            json!({ "message": "hello", "retries": 3, "enabled": true }),
        ),
        (
            "add_entries keeps an existing field unless told to overwrite",
            "  - add_entries:\n      entries:\n        - key: message\n          value: replaced\n",
            json_record("{\"message\":\"hello\"}"),
            json!({ "message": "hello" }),
        ),
        (
            "add_entries overwrites when told to",
            "  - add_entries:\n      entries:\n        - key: message\n          value: replaced\n          \
             overwrite_if_key_exists: true\n",
            json_record("{\"message\":\"hello\"}"),
            json!({ "message": "replaced" }),
        ),
        (
            "delete_entries removes the keys it names",
            "  - delete_entries:\n      with_keys: [\"drop\", \"absent\"]\n",
            json_record("{\"keep\":1,\"drop\":2}"),
            json!({ "keep": 1 }),
        ),
        (
            "rename_keys moves a value to another key",
            "  - rename_keys:\n      entries:\n        - from_key: old\n          to_key: new\n",
            json_record("{\"old\":\"value\"}"),
            json!({ "new": "value" }),
        ),
        (
            "rename_keys leaves an occupied target alone",
            "  - rename_keys:\n      entries:\n        - from_key: old\n          to_key: new\n",
            json_record("{\"old\":\"value\",\"new\":\"taken\"}"),
            json!({ "old": "value", "new": "taken" }),
        ),
        (
            "renaming a key onto itself moves nothing",
            "  - rename_keys:\n      entries:\n        - from_key: message\n          to_key: message\n          \
             overwrite_if_to_key_exists: true\n",
            json_record("{\"message\":\"kept\"}"),
            json!({ "message": "kept" }),
        ),
        (
            "copy_values keeps the source",
            "  - copy_values:\n      entries:\n        - from_key: message\n          to_key: copy\n",
            text_record("hello"),
            json!({ "message": "hello", "copy": "hello" }),
        ),
        (
            "lowercase_string lowercases the keys it names",
            "  - lowercase_string:\n      with_keys: [\"message\"]\n",
            json_record("{\"message\":\"MiXeD\"}"),
            json!({ "message": "mixed" }),
        ),
        (
            "uppercase_string uppercases the keys it names",
            "  - uppercase_string:\n      with_keys: [\"message\"]\n",
            json_record("{\"message\":\"MiXeD\"}"),
            json!({ "message": "MIXED" }),
        ),
        (
            "trim_string trims the keys it names",
            "  - trim_string:\n      with_keys: [\"message\", \"absent\"]\n",
            json_record("{\"message\":\"  padded  \"}"),
            json!({ "message": "padded" }),
        ),
        (
            "substitute_string replaces every match of its pattern",
            "  - substitute_string:\n      entries:\n        - source: message\n          from: \"[0-9]+\"\n          \
             to: \"N\"\n",
            json_record("{\"message\":\"a1b22c333\"}"),
            json!({ "message": "aNbNcN" }),
        ),
        (
            "substitute_string expands a group reference in its replacement",
            "  - substitute_string:\n      entries:\n        - source: message\n          \
             from: \"([a-z]+)=([0-9]+)\"\n          to: \"$2:$1\"\n",
            json_record("{\"message\":\"code=42\"}"),
            json!({ "message": "42:code" }),
        ),
        (
            "split_string splits a field into an array",
            "  - split_string:\n      entries:\n        - source: message\n          delimiter: \";\"\n",
            text_record("a;b;c"),
            json!({ "message": ["a", "b", "c"] }),
        ),
        (
            "split_string defaults its delimiter to a comma",
            "  - split_string:\n      entries:\n        - source: message\n",
            text_record("a,b"),
            json!({ "message": ["a", "b"] }),
        ),
        (
            "parse_json merges the parsed document into the record",
            "  - parse_json: {}\n",
            text_record("{\"a\":1,\"b\":\"two\"}"),
            json!({ "a": 1, "b": "two" }),
        ),
        (
            "parse_json puts the document under its destination",
            "  - parse_json:\n      destination: parsed\n",
            text_record("{\"a\":1}"),
            json!({ "parsed": { "a": 1 } }),
        ),
        (
            "parse_json takes the part its pointer addresses",
            "  - parse_json:\n      pointer: \"/detail/code\"\n      destination: code\n",
            text_record("{\"detail\":{\"code\":42}}"),
            json!({ "code": 42 }),
        ),
        (
            "two processors run in the order the sequence lists them",
            "  - parse_json: {}\n  - uppercase_string:\n      with_keys: [\"level\"]\n",
            text_record("{\"level\":\"warn\"}"),
            json!({ "level": "WARN" }),
        ),
    ] {
        let (status, response) =
            test_pipeline(&emulator, &request(&definition(processor), json!([record]))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let outputs = results(&response);
        let Some(result) = outputs.first() else {
            panic!("case {case:?}: expected one result, got {outputs:?}");
        };
        let fields = fields_of(case, result);
        assert_eq!(
            Value::Object(fields.clone()),
            expected,
            "case {case:?}: expected {expected:?}, got {fields:?}"
        );
    }
}

/// A definition that chains one section into the next through a `pipeline` sink runs
/// every section's processors, in the order the chain links them.
#[tokio::test]
async fn a_chained_definition_runs_every_section_it_links() {
    let emulator = emulator();
    let configuration = "version: \"2\"\n\
         entry-pipeline:\n  \
           source:\n    \
             http: {}\n  \
           processor:\n    \
             - add_entries:\n        \
                 entries:\n          \
                   - key: stage\n            \
                     value: entry\n  \
           sink:\n    \
             - pipeline:\n        \
                 name: \"second-pipeline\"\n\
         second-pipeline:\n  \
           source:\n    \
             pipeline:\n      \
               name: \"entry-pipeline\"\n  \
           processor:\n    \
             - uppercase_string:\n        \
                 with_keys: [\"message\"]\n  \
           sink:\n    \
             - cloudwatch_logs: {}\n";

    let (status, response) = test_pipeline(
        &emulator,
        &request(configuration, json!([text_record("quiet")])),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let outputs = results(&response);
    let Some(result) = outputs.first() else {
        panic!("expected one result, got {outputs:?}");
    };
    let fields = fields_of("a chained definition", result);
    assert_eq!(
        Value::Object(fields.clone()),
        json!({ "message": "QUIET", "stage": "entry" }),
        "both sections' processors ran, got {fields:?}"
    );
}

// =============================================================================
// Refusals
// =============================================================================

/// A configuration this emulator cannot run is a modelled 400 `ValidationException`
/// that processes no record: a processor or an option it does not implement would
/// otherwise be answered as a record nothing transformed.
///
/// The rows span the ways a definition can be one this emulator does not run,
/// malformed, truncated or past a bound.
#[tokio::test]
async fn a_configuration_this_emulator_cannot_run_is_refused() {
    let emulator = emulator();
    let many_processors = "  - trim_string:\n      with_keys: [\"message\"]\n"
        .repeat(MAX_PROCESSORS.saturating_add(1));

    for (case, configuration) in [
        (
            "a processor this emulator does not run",
            definition("  - grok:\n      match: \"%{COMMONAPACHELOG}\"\n"),
        ),
        (
            "an option the processor does not take",
            definition("  - csv:\n      delete_source: true\n"),
        ),
        (
            "an entry naming two processors at once",
            definition("  - csv: {}\n    trim_string: {}\n"),
        ),
        (
            "a processor entry that is not a mapping",
            definition("  - csv\n"),
        ),
        (
            "a processor section that is not a sequence",
            "source:\n  http: {}\nprocessor:\n  csv: {}\nsink:\n  - cloudwatch_logs: {}\n"
                .to_string(),
        ),
        (
            "an entries sequence that is missing",
            definition("  - rename_keys: {}\n"),
        ),
        (
            "an entry missing a required option",
            definition("  - rename_keys:\n      entries:\n        - from_key: old\n"),
        ),
        (
            "a delimiter that is not one character",
            definition("  - csv:\n      delimiter: \"::\"\n"),
        ),
        (
            "a delimiter that is also the quote character",
            definition("  - csv:\n      delimiter: \"'\"\n      quote_character: \"'\"\n"),
        ),
        (
            "a delimiter that would end the row it separates",
            definition("  - csv:\n      delimiter: \"\\n\"\n"),
        ),
        (
            "a pattern that is not a pattern",
            definition(
                "  - substitute_string:\n      entries:\n        - source: message\n          \
                 from: \"[0-9\"\n          to: \"N\"\n",
            ),
        ),
        (
            "a document that is not a mapping",
            "just some text\n".to_string(),
        ),
        (
            "a document that declares no pipeline section",
            "version: \"2\"\ntags:\n  owner: team\n".to_string(),
        ),
        (
            "a truncated document",
            "source:\n  http: {\nsink:\n  - [\n".to_string(),
        ),
        (
            "a document carrying an alias, which expands itself",
            "source: &anchor\n  http: {}\nsink:\n  - *anchor\n".to_string(),
        ),
        (
            "a document nesting past what this emulator reads",
            format!("source:\n  http: {}\n", "[".repeat(64)),
        ),
        (
            "a document indented past what this emulator reads",
            format!("source:\n{}http: {{}}\n", " ".repeat(240)),
        ),
        (
            "more processors than this emulator runs over one record",
            definition(&many_processors),
        ),
        (
            "more entries in one processor than this emulator runs",
            definition(&format!(
                "  - delete_entries:\n      with_keys: [\"a\"]\n  - copy_values:\n      entries:\n{}",
                "        - from_key: message\n          to_key: copy\n"
                    .repeat(MAX_PROCESSOR_ENTRIES.saturating_add(1))
            )),
        ),
        (
            "a replacement longer than this emulator substitutes",
            definition(&format!(
                "  - substitute_string:\n      entries:\n        - source: message\n          \
                 from: \"a\"\n          to: \"{}\"\n",
                "b".repeat(400)
            )),
        ),
        (
            "more substitution patterns than this emulator compiles in one call",
            definition(
                &"  - substitute_string:\n      entries:\n        - source: message\n          from: \"q\"\n          to: \"r\"\n"
                    .repeat(MAX_COMPILED_PATTERNS.saturating_add(1)),
            ),
        ),
        (
            "a pipeline sink naming a section the document does not declare",
            "version: \"2\"\nentry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        \
             name: \"absent\"\n"
                .to_string(),
        ),
        (
            "sections handing their records round a cycle",
            "version: \"2\"\nfirst:\n  source:\n    pipeline:\n      name: \"second\"\n  sink:\n    \
             - pipeline:\n        name: \"second\"\nsecond:\n  source:\n    pipeline:\n      \
             name: \"first\"\n  sink:\n    - pipeline:\n        name: \"first\"\n"
                .to_string(),
        ),
        (
            "a section handing its records to two pipelines at once",
            "version: \"2\"\nentry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        \
             name: \"left\"\n    - pipeline:\n        name: \"right\"\nleft:\n  source:\n    \
             pipeline:\n      name: \"entry\"\n  sink:\n    - cloudwatch_logs: {}\nright:\n  \
             source:\n    pipeline:\n      name: \"entry\"\n  sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
        ),
        (
            "a section no sink hands its records to",
            "version: \"2\"\nentry:\n  source:\n    http: {}\n  sink:\n    - cloudwatch_logs: {}\n\
             orphan:\n  source:\n    http: {}\n  processor:\n    - trim_string:\n        \
             with_keys: [\"message\"]\n  sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
        ),
    ] {
        let (status, response) = test_pipeline(
            &emulator,
            &request(&configuration, json!([text_record("a,b")])),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        assert!(
            results(&response).is_empty(),
            "case {case:?}: a refused configuration processes no record, got {response:?}"
        );
    }
}

/// A member that targets a modelled enum carries one of that enum's own values, and
/// anything else is a 400 — while every value the enum DOES carry is served.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused() {
    let emulator = emulator();
    let configuration = definition("  - uppercase_string:\n      with_keys: [\"message\"]\n");

    for (case, body) in [
        (
            "a signal type outside SignalType",
            request(&configuration, json!([text_record("a")])),
        ),
        (
            "a record format outside RecordFormat",
            request(&configuration, json!([{ "Data": "a", "Type": "YAML" }])),
        ),
    ] {
        let mut body = body;
        if case.contains("signal type") {
            body["SignalType"] = json!("TRACE");
        }
        let (status, response) = test_pipeline(&emulator, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // Both modelled signal types are served, and the chain runs the same way for
    // either: a signal type names which telemetry a record is, not how a processor
    // reads a field.
    for signal_type in ["LOG", "METRIC"] {
        let mut body = request(&configuration, json!([text_record("quiet")]));
        body["SignalType"] = json!(signal_type);
        let (status, response) = test_pipeline(&emulator, &body).await;
        assert_eq!(
            status, 200,
            "case {signal_type:?}: expected 200, got {status} {response:?}"
        );
        let outputs = results(&response);
        let Some(result) = outputs.first() else {
            panic!("case {signal_type:?}: expected one result, got {outputs:?}");
        };
        assert_eq!(
            record_of(result).map(|(data, _)| data),
            Some("QUIET".to_string()),
            "case {signal_type:?}: the chain runs the same way, got {result:?}"
        );
    }
}

/// A request past a bound on the work it would drive is refused before any record is
/// processed.
#[tokio::test]
async fn a_request_past_a_bound_is_refused_before_any_record_is_processed() {
    let emulator = emulator();
    let configuration = definition("  - trim_string:\n      with_keys: [\"message\"]\n");

    for (case, records) in [
        (
            "more records than this emulator processes in one call",
            Value::Array(
                (0..=MAX_TEST_RECORDS)
                    .map(|index| text_record(&format!("line {index}")))
                    .collect(),
            ),
        ),
        (
            "a record carrying more data than this emulator processes",
            json!([text_record(
                &"x".repeat(MAX_RECORD_DATA_BYTES.saturating_add(1))
            )]),
        ),
    ] {
        let (status, response) = test_pipeline(&emulator, &request(&configuration, records)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {}",
            error_code(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        assert!(
            results(&response).is_empty(),
            "case {case:?}: a refused request processes no record"
        );
    }

    // The record count the bound admits IS served, so the bound refuses only what is
    // past it.
    let records = Value::Array(
        (0..MAX_TEST_RECORDS)
            .map(|index| text_record(&format!(" line {index} ")))
            .collect(),
    );
    let (status, response) = test_pipeline(&emulator, &request(&configuration, records)).await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    assert_eq!(
        results(&response).len(),
        MAX_TEST_RECORDS,
        "a call at the bound answers one result per record"
    );
}

/// A record the chain would grow past the size one record is processed into is THAT
/// record's error, refused before the oversized value is stored — and it costs no
/// other record in the batch its result.
///
/// The rows differ along which processor would do the growing: a chain of copies, a
/// split into many parts, and a substitution whose replacement is longer than what it
/// matches.
#[tokio::test]
async fn a_record_the_chain_would_grow_too_large_is_that_records_error() {
    let emulator = emulator();
    let big = "x".repeat(MAX_RECORD_DATA_BYTES);
    let delimiters = ",".repeat(MAX_RECORD_DATA_BYTES);

    for (case, processor, data) in [
        (
            "copies of a large field",
            "  - copy_values:\n      entries:\n        - from_key: message\n          to_key: c1\n        \
             - from_key: c1\n          to_key: c2\n        - from_key: c2\n          to_key: c3\n        \
             - from_key: c3\n          to_key: c4\n        - from_key: c4\n          to_key: c5\n",
            big.clone(),
        ),
        (
            "a split into one part per delimiter",
            "  - split_string:\n      entries:\n        - source: message\n",
            delimiters.clone(),
        ),
        (
            "a substitution whose replacement is longer than every match",
            &format!(
                "  - substitute_string:\n      entries:\n        - source: message\n          from: \"x\"\n          \
                 to: \"{}\"\n",
                "y".repeat(200)
            ),
            big.clone(),
        ),
        (
            "a substitution whose replacement repeats the whole match",
            "  - substitute_string:\n      entries:\n        - source: message\n          from: \"x+\"\n          \
             to: \"$0$0$0$0$0$0$0$0\"\n",
            big.clone(),
        ),
    ] {
        let body = request(
            &definition(processor),
            json!([text_record(&data), text_record("small")]),
        );
        let (status, response) = test_pipeline(&emulator, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a record past the bound is a per-record error, not a refusal of the \
             call, got {status}"
        );
        let outputs = results(&response);
        assert_eq!(
            outputs.len(),
            2,
            "case {case:?}: expected one result per record, got {}",
            outputs.len()
        );
        let Some(oversized) = outputs.first() else {
            panic!("case {case:?}: expected a first result");
        };
        assert!(
            record_of(oversized).is_none() && error_of(oversized).is_some(),
            "case {case:?}: the oversized record carries its own error, got {oversized:?}"
        );
        // The small record beside it is processed as if the other had never been sent.
        let Some(small) = outputs.get(1) else {
            panic!("case {case:?}: expected a second result");
        };
        assert!(
            error_of(small).is_none(),
            "case {case:?}: the second record keeps its own result, got {small:?}"
        );
    }
}

/// The call's work budget bounds the PRODUCT of its records, its chain's steps and the
/// bytes each step reads: a batch whose chain would read past it processes the records
/// the budget covers and answers each record past it with its own error, rather than
/// scanning for as long as the configuration asks.
#[tokio::test]
async fn a_batch_past_the_calls_work_budget_stops_at_the_budget() {
    let emulator = emulator();
    // One `substitute_string` step per chain slot, each reading the whole field: the
    // records below carry MAX_TOTAL_RECORD_BYTES between them, so the chain reads far
    // more than MAX_WORK_BYTES over the batch.
    let chain = "  - substitute_string:\n      entries:\n        - source: message\n          from: \"q\"\n          to: \"r\"\n"
        .repeat(MAX_PROCESSORS);
    let record_bytes = MAX_RECORD_DATA_BYTES;
    let records = MAX_TOTAL_RECORD_BYTES / record_bytes;
    let body = request(
        &definition(&chain),
        Value::Array(
            (0..records)
                .map(|_| text_record(&"x".repeat(record_bytes)))
                .collect(),
        ),
    );
    assert!(
        records.saturating_mul(MAX_PROCESSORS).saturating_mul(record_bytes) > MAX_WORK_BYTES,
        "the batch must ask for more work than the budget: {records} records x \
         {MAX_PROCESSORS} steps x {record_bytes} bytes against {MAX_WORK_BYTES}"
    );

    let (status, response) = test_pipeline(&emulator, &body).await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let outputs = results(&response);
    assert_eq!(
        outputs.len(),
        records,
        "expected one result per record, got {}",
        outputs.len()
    );
    let Some(first) = outputs.first() else {
        panic!("expected a first result");
    };
    assert!(
        record_of(first).is_some(),
        "the records the budget covers are processed, got {first:?}"
    );
    let Some(last) = outputs.last() else {
        panic!("expected a last result");
    };
    assert!(
        error_of(last).is_some() && record_of(last).is_none(),
        "a record past the call's work budget carries its own error, got {last:?}"
    );
}

// =============================================================================
// Nothing is stored
// =============================================================================
/// The call tests a configuration; it does not deploy one. Nothing it is handed
/// becomes a stored pipeline, so a `GetTelemetryPipeline` for the section the document
/// named is still a 404 and the store still holds nothing.
#[tokio::test]
async fn testing_a_configuration_stores_no_pipeline() {
    let emulator = emulator();
    let configuration = "version: \"2\"\nlog-pipeline:\n  source:\n    http: {}\n  processor:\n    \
         - csv:\n        column_names: [\"a\", \"b\"]\n  sink:\n    - cloudwatch_logs: {}\n";

    let (status, response) = test_pipeline(
        &emulator,
        &request(configuration, json!([text_record("1,2")])),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");

    let (status, read) = call(
        &emulator,
        HOME_REGION,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": "log-pipeline" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a tested configuration is not a stored pipeline, got {status} {read:?}"
    );

    let stored = {
        let state = emulator
            .engine
            .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
        let store = state.lock();
        store.pipeline_count(HOME_REGION)
    };
    assert_eq!(
        stored, 0,
        "the store holds no pipeline after a test call, got {stored}"
    );

    // The ARN a stored pipeline would carry is never minted either, so nothing in the
    // account references the tested document.
    assert!(
        !response.to_string().contains(ACCOUNT),
        "a test answers records, not resource identity, got {response:?}"
    );
}
