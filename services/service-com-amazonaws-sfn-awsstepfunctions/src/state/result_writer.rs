// SPDX-License-Identifier: CC0-1.0

//! A Distributed `Map` state's `ResultWriter`: the S3 dataset its Map Run WRITES.
//!
//! "ResultWriter — specifies the Amazon S3 location where Step Functions writes all
//! child workflow execution results": a Distributed Map that declares one does not put
//! its iterations' results out as the state's own result — it writes them to S3 and
//! puts out WHERE it wrote them. The dataset is REALLY WRITTEN: the `Resource` names an
//! S3 operation, and that operation is called on S3's own handler in this same emulator
//! over the seam every other in-process call goes through
//! ([`super::integration::dataset_call`]), so the objects a caller reads back
//! afterwards are the objects S3 itself stored. Nothing is synthesised: a write S3
//! refused fails the state, and no manifest is reported for a write that did not
//! happen.
//!
//! ## What is written
//!
//! Under the `Prefix` the writer's parameters name, in a folder of the MAP RUN's own id
//! (`<Prefix>/<map run id>/`), one object per processing status that has results and
//! then the manifest that names them:
//!
//! * `SUCCEEDED_0.json` — the child workflow executions of the run that succeeded, as
//!   an array of their own records: the ARN, the name, the input and the output of each,
//!   with the dates it ran between and the redrive projection that execution reports;
//! * `FAILED_0.json` — the same for the children that failed, each carrying the `Error`
//!   and `Cause` its iteration failed with instead of an output;
//! * `manifest.json` — "a manifest.json file, which contains the aggregated Map Run
//!   results and references to the result files": the destination bucket, the Map Run's
//!   ARN, and the `ResultFiles` of each status, each entry naming the object's key and
//!   the bytes it holds. A status with no results is reported as an EMPTY list rather
//!   than as an object nothing wrote.
//!
//! A status whose results are not held here is not claimed: `PENDING` names the items a
//! run never started, and this emulator runs every item of a Distributed Map's attempt
//! inside the call that started it, so there is nothing to write under it and its list
//! is empty.
//!
//! ## What the state puts out
//!
//! The Map Run's ARN and where the manifest was written
//! ([`Written::reported`]) — not the array of results, which is in S3 — and the state's
//! own `ResultSelector`, `ResultPath` and `OutputPath` are applied to THAT document,
//! exactly as they are applied to the array a state with no writer produces.
//!
//! ## What a failed write does
//!
//! "States.ResultWriterFailed — occurs when a state fails because it can't write
//! results to the `ResultWriter`" ([`RESULT_WRITER_ERROR`]): a bucket that is not there,
//! a write S3 refused, a result set larger than this emulator writes. It is the MAP
//! STATE's failure and it carries an error name, so the state's own `Retry` and `Catch`
//! answer for it exactly as they answer for a failed task.
//!
//! ## What the write is subject to
//!
//! The execution role is asked BEFORE the request goes anywhere, exactly as it is for a
//! dataset read ([`super::item_reader`]) and for a `Task` state's integration call
//! ([`Tasks::authorize`]).

use serde_json::{Map, Value};

use super::integration::{Call, Failed, Tasks};

/// The ASL predefined error name for a state that could not write its `ResultWriter`
/// ("States.ResultWriterFailed").
pub const RESULT_WRITER_ERROR: &str = "States.ResultWriterFailed";

/// The S3 `Resource` a `ResultWriter` names: the only destination the language gives
/// one ("ResultWriter … Resource: arn:aws:states:::s3:putObject").
pub const PUT_OBJECT_RESOURCE: &str = "arn:aws:states:::s3:putObject";

/// The members a `ResultWriter` may carry: where the results go (`Resource` and the
/// `Parameters`/`Arguments` naming the bucket and the prefix) and how they are written
/// (`WriterConfig`).
const WRITER_MEMBERS: &[&str] = &["Resource", "Parameters", "Arguments", "WriterConfig"];

/// The members a `ResultWriter`'s `Parameters` may carry: the destination bucket and
/// the prefix the run's folder is written under.
const PARAMETER_MEMBERS: &[&str] = &["Bucket", "Prefix"];

/// The members a `WriterConfig` may carry.
const WRITER_CONFIG_MEMBERS: &[&str] = &["OutputType", "Transformation"];

/// The `OutputType` this emulator writes: "JSON — Step Functions writes the results as
/// a single JSON array", which is the form the result objects below hold.
const JSON_OUTPUT_TYPE: &str = "JSON";

/// The `Transformation` this emulator writes: `NONE`, the member's own default, which
/// writes "the Map Run's results … along with the manifest".
const NO_TRANSFORMATION: &str = "NONE";

/// The object the manifest of one Map Run is written under, as the language names it:
/// "a manifest.json file".
const MANIFEST_OBJECT: &str = "manifest.json";

/// The third processing status the manifest reports result files for, beside the two
/// terminal execution statuses: the items of a run that were never started.
///
/// It is not an execution status — no record is held under it — so it is spelled here
/// rather than taken from the statuses an execution reports.
const PENDING_RESULTS: &str = "PENDING";

/// The most bytes of result objects ONE Map Run writes.
///
/// Every record in them is a child workflow execution's own input and output, each
/// already inside what a state may carry, and a run has as many as it has items — so
/// the objects are bounded TOGETHER as they are built, and a run past this fails the
/// write rather than holding the whole of it in this process.
pub const MAX_RESULT_BYTES: usize = 4 * 1_024 * 1_024;

/// Where one `Map` state's results are written: the destination its `ResultWriter`
/// names.
#[derive(Debug, Clone)]
pub struct Writer {
    /// "Bucket — the Amazon S3 bucket name in which to write the results."
    pub bucket: String,
    /// "Prefix — (Optional) the Amazon S3 object key prefix in which to write the
    /// results"; empty for a writer that names none, whose folder is at the root of the
    /// bucket.
    pub prefix: String,
}

/// The destination the `writer` member of the state `state` names, with `parameters`
/// the document its own `Parameters` (or JSONata `Arguments`) template rendered to —
/// the writer's members are a definition's text, so nothing here evaluates anything.
///
/// The `Err` is the failure the `Map` state reports: a writer this emulator does not
/// write carries ASL's [`super::asl::RUNTIME_ERROR`] and says what was not written,
/// while one whose own members do not name a destination carries
/// [`RESULT_WRITER_ERROR`].
pub fn read(state: &str, writer: &Value, parameters: &Value) -> Result<Writer, Failed> {
    let Some(members) = writer.as_object() else {
        return Err(failed(
            state,
            format!("'ResultWriter' must be an object, not {writer}"),
        ));
    };
    if let Some(member) = unknown(members, WRITER_MEMBERS) {
        return Err(not_written(state, format!("carries the member '{member}'")));
    }
    let resource = text(writer, "Resource").unwrap_or_default();
    if resource != PUT_OBJECT_RESOURCE {
        return Err(not_written(
            state,
            format!(
                "names the destination '{}'",
                super::asl::echoed(&resource)
            ),
        ));
    }
    if let Some(config) = writer.get("WriterConfig") {
        written_as(state, config)?;
    }
    let Some(named) = parameters.as_object() else {
        return Err(failed(
            state,
            "the 'ResultWriter' names no 'Bucket' to write the results to".to_string(),
        ));
    };
    if let Some(member) = unknown(named, PARAMETER_MEMBERS) {
        return Err(not_written(
            state,
            format!("names the parameter '{member}'"),
        ));
    }
    let Some(bucket) = text(parameters, "Bucket") else {
        return Err(failed(
            state,
            "the 'ResultWriter' names no 'Bucket' to write the results to".to_string(),
        ));
    };
    Ok(Writer {
        bucket,
        prefix: text(parameters, "Prefix").unwrap_or_default(),
    })
}

/// Whether `config` asks for the only form of results this emulator writes.
///
/// A `WriterConfig` that asks for another `OutputType` or another `Transformation` asks
/// for a different dataset — one array per line, or the results without their manifest
/// — and that dataset is not written here, so the state is not run for it rather than
/// run as though the member said something else.
fn written_as(state: &str, config: &Value) -> Result<(), Failed> {
    let Some(members) = config.as_object() else {
        return Err(failed(
            state,
            format!("'WriterConfig' must be an object, not {config}"),
        ));
    };
    if let Some(member) = unknown(members, WRITER_CONFIG_MEMBERS) {
        return Err(not_written(
            state,
            format!("carries the 'WriterConfig' member '{member}'"),
        ));
    }
    for (member, written) in [
        ("OutputType", JSON_OUTPUT_TYPE),
        ("Transformation", NO_TRANSFORMATION),
    ] {
        let declared = text(config, member).unwrap_or_else(|| written.to_string());
        if declared != written {
            return Err(not_written(
                state,
                format!(
                    "asks for the '{member}' '{}'",
                    super::asl::echoed(&declared)
                ),
            ));
        }
    }
    Ok(())
}

/// One child workflow execution's own record in a result object: what that execution
/// reports about itself, as the run that started it knows it.
///
/// The members are `DescribeExecution`'s own, because a result file holds "all child
/// workflow execution results": the execution's identity, the data it ran over and the
/// redrive projection it reports — a SUCCEEDED child is not redrivable ("Execution is
/// SUCCEEDED and cannot be redriven") and a child that failed is redrivable by its Map
/// Run.
#[derive(Debug, Clone)]
pub struct ItemResult {
    pub status: String,
    pub execution_arn: String,
    pub name: String,
    pub input: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    /// The instants that execution ran between, as the service renders a date.
    pub start_date: String,
    pub stop_date: String,
    /// How many times that child workflow execution itself has been redriven, which
    /// is the count its own record carries: a child is never redriven on its own — its
    /// Map Run redrives it, and a run that redrives one starts it afresh, so the count
    /// an execution of an item reports is the zero it was started with.
    pub redrive_count: i32,
}

impl ItemResult {
    /// This child's record, as a result object holds it.
    fn reported(&self, state_machine_arn: &str) -> Value {
        let included = serde_json::json!({ "Included": true });
        let mut record = Map::new();
        record.insert(
            "ExecutionArn".to_string(),
            Value::String(self.execution_arn.clone()),
        );
        record.insert("Name".to_string(), Value::String(self.name.clone()));
        record.insert(
            "StateMachineArn".to_string(),
            Value::String(state_machine_arn.to_string()),
        );
        record.insert("Status".to_string(), Value::String(self.status.clone()));
        record.insert("Input".to_string(), Value::String(self.input.clone()));
        record.insert("InputDetails".to_string(), included.clone());
        if let Some(output) = &self.output {
            record.insert("Output".to_string(), Value::String(output.clone()));
            record.insert("OutputDetails".to_string(), included);
        }
        for (member, value) in [("Error", &self.error), ("Cause", &self.cause)] {
            if let Some(value) = value {
                record.insert(member.to_string(), Value::String(value.clone()));
            }
        }
        record.insert(
            "StartDate".to_string(),
            Value::String(self.start_date.clone()),
        );
        record.insert(
            "StopDate".to_string(),
            Value::String(self.stop_date.clone()),
        );
        record.insert(
            "RedriveCount".to_string(),
            Value::Number(self.redrive_count.into()),
        );
        let (redrive_status, reason) = match self.status == super::EXECUTION_STATUS_SUCCEEDED {
            true => (
                super::REDRIVE_STATUS_NOT_REDRIVABLE,
                Some(super::NOT_REDRIVABLE_SUCCEEDED),
            ),
            false => (super::REDRIVE_STATUS_REDRIVABLE_BY_MAP_RUN, None),
        };
        record.insert(
            "RedriveStatus".to_string(),
            Value::String(redrive_status.to_string()),
        );
        if let Some(reason) = reason {
            record.insert(
                "RedriveStatusReason".to_string(),
                Value::String(reason.to_string()),
            );
        }
        Value::Object(record)
    }
}

/// What one Map Run's `ResultWriter` wrote: where its manifest is.
#[derive(Debug, Clone)]
pub struct Written {
    bucket: String,
    key: String,
    map_run_arn: String,
}

impl Written {
    /// The document the `Map` state puts out for a run whose results were written:
    /// "the Map state's output contains the Map Run ARN and the Amazon S3 location of
    /// the manifest", and not the results themselves, which are in S3.
    pub fn reported(&self) -> Value {
        serde_json::json!({
            "MapRunArn": self.map_run_arn,
            "ResultWriterDetails": {
                "Bucket": self.bucket,
                "Key": self.key,
            },
        })
    }
}

/// Write the results of the Map Run `map_run_arn` to the destination `writer` names,
/// through `tasks` — the same seam a dataset read and a `Task` state's integration call
/// are made over.
///
/// `results` is one record per child workflow execution the run's items ran as, in the
/// order they were started; `state_machine_arn` is the state machine every one of them
/// ran, which is the one the Map Run reports.
///
/// The `Err` is the `Map` state's own failure ([`RESULT_WRITER_ERROR`]).
pub fn write(
    state: &str,
    writer: &Writer,
    map_run_arn: &str,
    state_machine_arn: &str,
    results: &[ItemResult],
    tasks: &dyn Tasks,
) -> Result<Written, Failed> {
    let folder = folder_of(writer, map_run_arn);
    let mut files: Map<String, Value> = Map::new();
    let mut written = 0usize;
    // One object per status, written in the order the statuses are reported, so a
    // reader of the manifest finds the objects it names already stored.
    for status in [
        super::EXECUTION_STATUS_SUCCEEDED,
        super::EXECUTION_STATUS_FAILED,
        PENDING_RESULTS,
    ] {
        // The array this status's object holds, grown RECORD BY RECORD and measured as
        // it grows: a run has as many records as it has items and each one is a child's
        // own input and output, so a run past the ceiling fails the write before the
        // rest of its records are rendered rather than after the whole of them is held.
        let mut body = String::from("[");
        for result in results.iter().filter(|result| result.status == status) {
            if body.len() > 1 {
                body.push(',');
            }
            body.push_str(&result.reported(state_machine_arn).to_string());
            let held = written.saturating_add(body.len());
            if held > MAX_RESULT_BYTES {
                return Err(failed(
                    state,
                    format!(
                        "the results of this Map Run are at least {held} bytes, more than the \
                         {MAX_RESULT_BYTES} this emulator writes"
                    ),
                ));
            }
        }
        // A status with no results is reported as an EMPTY list rather than as an
        // object nothing wrote.
        if body.len() == 1 {
            files.insert(status.to_string(), Value::Array(Vec::new()));
            continue;
        }
        body.push(']');
        written = written.saturating_add(body.len());
        let key = format!("{folder}{status}_0.json");
        put(state, writer, &key, &body, tasks)?;
        files.insert(
            status.to_string(),
            serde_json::json!([{ "Key": key, "Size": body.len() }]),
        );
    }
    // "A manifest.json file, which contains the aggregated Map Run results and
    // references to the result files": written LAST, so a manifest a caller can read
    // names objects that are already there.
    let manifest = serde_json::json!({
        "DestinationBucket": writer.bucket,
        "MapRunArn": map_run_arn,
        "ResultFiles": Value::Object(files),
    })
    .to_string();
    let key = format!("{folder}{MANIFEST_OBJECT}");
    put(state, writer, &key, &manifest, tasks)?;
    Ok(Written {
        bucket: writer.bucket.clone(),
        key,
        map_run_arn: map_run_arn.to_string(),
    })
}

/// The folder one Map Run's objects are written in: the writer's own prefix and the Map
/// Run's id, so two runs of one `Map` state never write over each other's results.
///
/// The id is the last field of the Map Run's ARN
/// (`mapRun:<stateMachineName>/<label>:<id>`, [`super::map_run_arn`]), which is what
/// makes one run's folder unique; a prefix the writer did not name puts the folder at
/// the root of the bucket.
fn folder_of(writer: &Writer, map_run_arn: &str) -> String {
    let id = map_run_arn.rsplit(':').next().unwrap_or_default();
    match writer.prefix.is_empty() {
        true => format!("{id}/"),
        false => format!("{}/{id}/", writer.prefix.trim_end_matches('/')),
    }
}

/// Store ONE object of the dataset this writer writes.
fn put(
    state: &str,
    writer: &Writer,
    key: &str,
    body: &str,
    tasks: &dyn Tasks,
) -> Result<(), Failed> {
    let call = call(state)?;
    let mut request = Map::new();
    request.insert("Bucket".to_string(), Value::String(writer.bucket.clone()));
    request.insert("Key".to_string(), Value::String(key.to_string()));
    request.insert(
        super::integration::DATASET_BODY.to_string(),
        Value::String(body.to_string()),
    );
    let request = Value::Object(request);
    if let Some(denied) = tasks.authorize(&call, &request) {
        return Err(denied);
    }
    tasks
        .perform(&call, &request, None)
        .map(|_answered| ())
        .map_err(|refused| {
            failed(
                state,
                format!(
                    "the object '{key}' could not be written: {} ({})",
                    refused.cause, refused.error
                ),
            )
        })
}

/// The S3 call one write is made as.
fn call(state: &str) -> Result<Call, Failed> {
    let operation = super::integration::PUT_OBJECT_OPERATION;
    super::integration::dataset_call(operation).ok_or_else(|| {
        failed(
            state,
            format!("this emulator binds no '{operation}' request to write a dataset with"),
        )
    })
}

/// The first member of `members` that is not one of `allowed` ([`super::unknown_member`]).
fn unknown(members: &Map<String, Value>, allowed: &[&str]) -> Option<String> {
    super::unknown_member(members, allowed)
}

/// The text `document` names `member` with ([`super::declared_text`]).
fn text(document: &Value, member: &str) -> Option<String> {
    super::declared_text(document, member)
}

/// The failure a state whose `ResultWriter` could not be WRITTEN reports: the ASL error
/// name for exactly that, and a cause saying what stopped it.
fn failed(state: &str, why: String) -> Failed {
    Failed {
        error: RESULT_WRITER_ERROR.to_string(),
        cause: format!("the 'ResultWriter' of state '{state}' could not be written: {why}"),
    }
}

/// The failure a state asking for a dataset this emulator does not write reports: a gap
/// in this emulator rather than the service refusing anything, which is ASL's own
/// runtime error with a cause saying what was not written.
fn not_written(state: &str, what: String) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "the 'ResultWriter' of state '{state}' {what}, which this emulator does not write, \
             so nothing was run for it"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A writer over `parameters`, as [`read`] resolves it.
    fn writer(parameters: Value) -> Result<Writer, Failed> {
        read(
            "Fan",
            &json!({ "Resource": PUT_OBJECT_RESOURCE, "Parameters": parameters.clone() }),
            &parameters,
        )
    }

    #[test]
    fn a_writer_names_the_bucket_and_the_prefix_its_parameters_do() {
        let rows: &[(&str, Value, &str, &str)] = &[
            (
                "a bucket and a prefix",
                json!({ "Bucket": "results", "Prefix": "runs" }),
                "results",
                "runs",
            ),
            (
                "a bucket alone",
                json!({ "Bucket": "results" }),
                "results",
                "",
            ),
            (
                "a prefix with its own trailing separator",
                json!({ "Bucket": "results", "Prefix": "runs/" }),
                "results",
                "runs/",
            ),
        ];
        for (case, parameters, bucket, prefix) in rows {
            let resolved = writer(parameters.clone()).expect("the writer resolves");
            assert_eq!(
                (resolved.bucket.as_str(), resolved.prefix.as_str()),
                (*bucket, *prefix),
                "case {case:?}: expected {bucket:?}/{prefix:?}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn one_run_writes_under_its_own_id() {
        let arn = "arn:aws:states:us-east-1:000000000000:mapRun:machine/Fan:abc-123";
        let rows: &[(&str, &str, &str)] = &[
            ("a prefix", "runs", "runs/abc-123/"),
            ("a prefix with a separator", "runs/", "runs/abc-123/"),
            ("no prefix", "", "abc-123/"),
        ];
        for (case, prefix, expected) in rows {
            let folder = folder_of(
                &Writer {
                    bucket: "results".to_string(),
                    prefix: (*prefix).to_string(),
                },
                arn,
            );
            assert_eq!(
                folder, *expected,
                "case {case:?}: expected {expected:?}, got {folder:?}"
            );
        }
    }

    #[test]
    fn a_writer_naming_no_destination_fails_the_result_writer() {
        let refused = writer(json!({})).expect_err("a writer with no bucket is refused");
        assert_eq!(
            refused.error, RESULT_WRITER_ERROR,
            "a writer naming no bucket fails the result writer, got {refused:?}"
        );
    }

    #[test]
    fn a_writer_this_emulator_does_not_write_says_so() {
        let rows: &[(&str, Value)] = &[
            (
                "another destination",
                json!({ "Resource": "arn:aws:states:::s3:putObject.waitForTaskToken" }),
            ),
            (
                "a member this emulator does not interpret",
                json!({ "Resource": PUT_OBJECT_RESOURCE, "Mode": "INLINE" }),
            ),
            (
                "another output type",
                json!({
                    "Resource": PUT_OBJECT_RESOURCE,
                    "WriterConfig": { "OutputType": "JSONL" },
                }),
            ),
            (
                "another transformation",
                json!({
                    "Resource": PUT_OBJECT_RESOURCE,
                    "WriterConfig": { "Transformation": "COMPACT" },
                }),
            ),
        ];
        for (case, declared) in rows {
            let refused = read("Fan", declared, &json!({ "Bucket": "results" }))
                .expect_err("the writer is not written");
            assert_eq!(
                refused.error,
                super::super::asl::RUNTIME_ERROR,
                "case {case:?}: expected the runtime error, got {refused:?}"
            );
            assert!(
                refused.cause.contains("does not write"),
                "case {case:?}: the cause says what was not written, got {:?}",
                refused.cause
            );
        }
    }

    #[test]
    fn a_default_writer_config_is_the_one_this_emulator_writes() {
        let resolved = read(
            "Fan",
            &json!({
                "Resource": PUT_OBJECT_RESOURCE,
                "WriterConfig": { "OutputType": JSON_OUTPUT_TYPE, "Transformation": NO_TRANSFORMATION },
            }),
            &json!({ "Bucket": "results" }),
        )
        .expect("the declared defaults are what is written");
        assert_eq!(
            resolved.bucket, "results",
            "the writer resolves, got {resolved:?}"
        );
    }

    #[test]
    fn a_result_record_is_the_child_executions_own_report() {
        let succeeded = ItemResult {
            status: super::super::EXECUTION_STATUS_SUCCEEDED.to_string(),
            execution_arn: "arn:aws:states:us-east-1:000000000000:execution:m/Fan:one".to_string(),
            name: "one".to_string(),
            input: "{\"id\":1}".to_string(),
            output: Some("{\"ok\":true}".to_string()),
            error: None,
            cause: None,
            start_date: "2026-01-02T03:04:05.000Z".to_string(),
            stop_date: "2026-01-02T03:04:06.000Z".to_string(),
            redrive_count: 0,
        };
        let reported = succeeded.reported("arn:aws:states:us-east-1:000000000000:stateMachine:m");
        assert_eq!(
            reported.get("Output").and_then(Value::as_str),
            Some("{\"ok\":true}"),
            "a succeeded child reports its output, got {reported:?}"
        );
        assert_eq!(
            reported.get("RedriveStatus").and_then(Value::as_str),
            Some(super::super::REDRIVE_STATUS_NOT_REDRIVABLE),
            "a succeeded child cannot be redriven, got {reported:?}"
        );
        let failed = ItemResult {
            status: super::super::EXECUTION_STATUS_FAILED.to_string(),
            output: None,
            error: Some("States.TaskFailed".to_string()),
            cause: Some("the item failed".to_string()),
            ..succeeded
        };
        let reported = failed.reported("arn:aws:states:us-east-1:000000000000:stateMachine:m");
        assert_eq!(
            (
                reported.get("Error").and_then(Value::as_str),
                reported.get("Output"),
                reported.get("RedriveStatus").and_then(Value::as_str),
            ),
            (
                Some("States.TaskFailed"),
                None,
                Some(super::super::REDRIVE_STATUS_REDRIVABLE_BY_MAP_RUN),
            ),
            "a failed child reports its failure and is redrivable by its Map Run, got \
             {reported:?}"
        );
    }
}
