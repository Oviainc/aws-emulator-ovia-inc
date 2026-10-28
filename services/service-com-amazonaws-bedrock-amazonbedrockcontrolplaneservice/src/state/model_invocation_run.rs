// SPDX-License-Identifier: CC0-1.0

//! The WORK a batch inference job performs: the records it reads out of its input object,
//! the model invocation it performs over each one, and the result records it writes under
//! its output prefix.
//!
//! A batch inference job is not a status record: `ModelInvocationJobStatus` ties each of
//! its statuses to real work over real data — `Validating` checks that "your files are
//! .jsonl files and each individual record is a JSON object in the correct format" and
//! that they "fulfill the requirements for file size and number of records", `InProgress`
//! says "this job has begun. You can start viewing the results in the output S3
//! location", and `Completed` says "view the output files in the output S3 location"
//! (`smithy_query(query="shape",shape="ModelInvocationJobSummary")`, member `status`). So
//! a job this emulator reports as having begun has read every record of the object its
//! `inputDataConfig` names, has invoked the model on each record's `modelInput`, has
//! written one result record per input record under the prefix its `outputDataConfig`
//! names, and reports record counts that account for them.
//!
//! WHERE the records come from and go: this emulator's OWN Amazon S3, over the in-process
//! callback every cross-service call of an emulated service uses ([`s3_client`] — no
//! socket, no outbound request, no host resolution). The caller's `s3Uri` reaches it only
//! as the bucket and key of a `GetObject` and a `PutObject`, never as a host or a URL.
//!
//! WHAT performs the inference: the Amazon Bedrock Runtime emulator's own invocation
//! path, read through that service's state module — the very functions its `InvokeModel`
//! and `Converse` handlers answer from ([`runtime::invoke_turn`],
//! [`runtime::emulated_completion`]) — so a record of a batch job and a synchronous call
//! carrying the same `modelInput` are served by one inference rather than by two that
//! could drift. Nothing is echoed back as if generated: the completion is that service's
//! own, which says in so many words that no language model runs here.
//!
//! WHAT a verdict rests on: a job is failed over its input only when the object store
//! ITSELF said something about it ([`names_nothing`]) — an emulator assembled with no
//! object store to ask establishes nothing, so such a job keeps the lifecycle it has
//! always had and reports no record counts rather than being failed on an unanswered
//! question.

use serde_json::{json, Value};

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state as runtime;

use crate::ops::types::{ModelInvocationJobInputDataConfig, ModelInvocationJobOutputDataConfig};

/// The most records the input object of one job may hold before the job is failed for the
/// number of records it carries.
///
/// The emulator's own ceiling rather than a published quota, for the reason
/// [`crate::state::MAX_MODEL_INVOCATION_JOBS_PER_ACCOUNT`] is one: the whole input is read
/// into this process and the model is invoked once per record, so without a server-side
/// cap a single submission could make one read do unbounded work. It stands where the
/// documented validation criteria put it — "your files fulfill the requirements for file
/// size and number of records" is a check a submitted job passes or fails, so an input
/// past it fails the job rather than being processed in part.
pub const MAX_INPUT_RECORDS: usize = 10_000;

/// The most bytes of input one job reads before it is failed for the size of its file.
///
/// The emulator's own ceiling, for the reason [`MAX_INPUT_RECORDS`] is one and standing
/// where the same documented validation criterion puts it ("your files fulfill the
/// requirements for file size and number of records"): the whole input is read into this
/// process, so the size a caller can make one read allocate is bounded BEFORE the body is
/// collected rather than after. It is of a size with the record ceiling, a few kilobytes of
/// records apiece.
pub const MAX_INPUT_BYTES: i64 = 32 * 1024 * 1024;

/// The most runs one request performs, however many jobs are owed one.
///
/// A listing settles every job of the Region, so without this the work one call does would
/// be the number of jobs times the records of each; with it a call performs at most this
/// many runs and the rest stay owed, so a later call takes them. The number is small for
/// that reason, and larger than one so a listing does not take a dozen calls to settle a
/// handful of jobs.
pub const MAX_RUNS_PER_REQUEST: usize = 4;

/// The suffix the output object of an input object carries: a batch inference job writes
/// its results beside the name of the input file it read, under a folder named for the job.
const OUTPUT_SUFFIX: &str = ".out";

/// The content type the output object is stored with: its records are JSON objects, one
/// per line.
const OUTPUT_CONTENT_TYPE: &str = "application/json";

/// The member of an input record that carries what the model is invoked with.
const MODEL_INPUT: &str = "modelInput";

/// The member of an input record that carries the caller's own identifier for it, which
/// the result record is reported under.
const RECORD_ID: &str = "recordId";

/// How many characters a minted `recordId` carries, as the documented input format spells
/// one: an 11-character alphanumeric string.
const RECORD_ID_LENGTH: usize = 11;

/// The Amazon S3 error codes in which S3 reports that what a request named is NOT THERE: a
/// bucket it does not hold (`NoSuchBucket`), a key that bucket does not hold (`NoSuchKey`),
/// a version it holds none of (`NoSuchVersion`) and the code a HEAD-shaped answer carries
/// for either (`NotFound`).
///
/// The code is required TOGETHER WITH the 404 status ([`names_nothing`]) and never the
/// status alone, because a status is not evidence that S3 answered at all: a request that
/// reached no S3 is a 404 too, and that is this emulator failing to ask rather than a
/// verdict about the object. The list is deliberately CLOSED: a code reporting something
/// else about the object establishes nothing, so it fails no job.
const ABSENT_OBJECT_CODES: &[&str] = &["NoSuchBucket", "NoSuchKey", "NoSuchVersion", "NotFound"];

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The ONE client this service reaches object storage with: the read of a job's input
/// records ([`read_input_records`]) and the write of its result records
/// ([`run_batch_inference`]) both go through it, so neither can reach the object store in
/// a way the other would not.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here and this crate names no
/// other service's crate for it. The `endpoint_url` below is therefore an ADDRESS the
/// in-process router reads, never a host any resolver looks up, and `region` is the
/// request's own signing region as [`crate::state::request_region`] narrowed it — so a
/// job's records are read and written where the caller's own Amazon S3 requests go.
///
/// The credentials are the emulator's own: they only sign this operation's in-process
/// calls and are never put in a response or a log.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether S3's answer is S3 ITSELF saying it holds nothing at the address a request
/// named: HTTP 404 AND one of the codes it sends for a missing bucket, key or version
/// ([`ABSENT_OBJECT_CODES`]).
fn names_nothing<E>(
    error: &::aws_sdk_s3::error::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 404)
        && error
            .code()
            .is_some_and(|code| ABSENT_OBJECT_CODES.contains(&code))
}

/// The `s3Uri` a job reads its records from.
#[must_use]
pub fn input_s3_uri(config: &ModelInvocationJobInputDataConfig) -> &str {
    match config {
        ModelInvocationJobInputDataConfig::s3InputDataConfig(config) => config.s3_uri.as_str(),
    }
}

/// The `s3Uri` a job writes its results under.
#[must_use]
pub fn output_s3_uri(config: &ModelInvocationJobOutputDataConfig) -> &str {
    match config {
        ModelInvocationJobOutputDataConfig::s3OutputDataConfig(config) => config.s3_uri.as_str(),
    }
}

/// The bucket and key an `S3Uri` that addresses ONE OBJECT names, or `None` for a URI of
/// that scheme addressing no object of it (a bucket alone, a prefix).
///
/// The member's own `@pattern`
/// (`smithy_query(query="shape",shape="ModelInvocationJobS3InputDataConfig")`) admits
/// `s3://bucket`, `s3://bucket/key`, nested keys and a trailing `/`, so an object address
/// is a URI with a non-empty key under the bucket.
#[must_use]
pub fn object_address(uri: &str) -> Option<(String, String)> {
    let (bucket, key) = uri.strip_prefix("s3://")?.split_once('/')?;
    let key = key.trim_end_matches('/');
    (!bucket.is_empty() && !key.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// The bucket and the key PREFIX an `S3Uri` that addresses a location names: the prefix is
/// empty for a URI naming the bucket itself, and carries no leading or trailing separator
/// otherwise, so one join rule builds every key under it.
#[must_use]
pub fn prefix_address(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("s3://")?;
    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    (!bucket.is_empty()).then(|| (bucket.to_string(), prefix.trim_matches('/').to_string()))
}

/// One record of a job's input object: what the model is invoked with, and the caller's own
/// identifier for it where the record carried one.
#[derive(Debug, Clone)]
pub struct InputRecord {
    /// `recordId`, as the input record spelled it. Absent is allowed — the job mints one
    /// for the result record ([`minted_record_id`]).
    pub record_id: Option<String>,
    /// `modelInput`: "the request body for the model", invoked as it stands.
    pub model_input: Value,
}

/// What asking the object store about a job's input established.
#[derive(Debug)]
pub enum InputVerdict {
    /// The store served the input object and every record in it is a JSON object in the
    /// documented form.
    Records(Vec<InputRecord>),
    /// The store ITSELF said the input cannot be processed: it holds nothing at that
    /// address, or what it holds is not a .jsonl file of records in that form. The sentence
    /// is the one the job reports as its `message`.
    Rejected(String),
    /// Nothing is known: no object store answered the question at all.
    Unknown,
}

/// Read the records of the object `uri` names out of this emulator's own Amazon S3 and
/// report what that established.
///
/// This is the `Validating` step, performed over the object the submission pointed at: the
/// criteria the status documents are that the file is readable by the job, that "each
/// individual record is a JSON object in the correct format" and that the file "fulfills
/// the requirements for file size and number of records" ([`MAX_INPUT_RECORDS`]). A record
/// is in that form when it is a JSON object carrying `modelInput`; "validation doesn't
/// check if the `modelInput` value matches the request body for the model", so what the
/// member HOLDS is not judged here — a record the model cannot be invoked with is an error
/// RECORD of the run, not a failure of validation.
pub async fn read_input_records(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
) -> InputVerdict {
    let Some((bucket, key)) = object_address(uri) else {
        // A URI of the S3 scheme that addresses no object of it names no input file for
        // any store to answer about: the job has nothing to read.
        return InputVerdict::Rejected(format!(
            "The input location {uri} does not name a .jsonl object. Point \
             inputDataConfig.s3InputDataConfig.s3Uri at the input file and submit the job \
             again."
        ));
    };
    let answer = s3_client(ctx, region)
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await;
    let object = match answer {
        Ok(object) => {
            // The size of the file, bounded BEFORE its bytes are collected: what the store
            // reports for the object is what reading it would allocate.
            if object
                .content_length()
                .is_some_and(|length| length > MAX_INPUT_BYTES)
            {
                return InputVerdict::Rejected(oversized_message(uri));
            }
            object
        }
        Err(error) => {
            if names_nothing(&error) {
                // S3 says it holds nothing there: the verdict is grounded.
                return InputVerdict::Rejected(format!(
                    "The input data at {uri} could not be read. Make sure that the object \
                     exists and that the service role has access to it, then submit the job \
                     again."
                ));
            }
            // Any other answer is not S3 reporting an absent object, so it establishes
            // nothing about the input — and it is worth a warning, because an emulator with
            // no object store mounted to ask is one where a job reads no records at all.
            // The caller's own values are rendered in their escaped `Debug` form, so a key
            // carrying a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the batch inference input could not be read from this emulator's Amazon S3"
            );
            return InputVerdict::Unknown;
        }
    };
    let bytes = match object.body.collect().await {
        Ok(body) => {
            let bytes = body.into_bytes();
            if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > MAX_INPUT_BYTES {
                return InputVerdict::Rejected(oversized_message(uri));
            }
            bytes
        }
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                detail = %error,
                "the batch inference input could not be read from this emulator's Amazon S3"
            );
            return InputVerdict::Unknown;
        }
    };
    let Ok(text) = ::std::str::from_utf8(&bytes) else {
        return InputVerdict::Rejected(format!(
            "The input data at {uri} is not a .jsonl file: it is not UTF-8 text. Each record \
             must be a JSON object on its own line."
        ));
    };
    parse_input_records(uri, text)
}

/// Why a job whose input file is larger than this emulator reads could not be run: the
/// documented validation criterion it breaks, naming the ceiling it stands at.
fn oversized_message(uri: &str) -> String {
    format!(
        "The input data at {uri} is larger than the {MAX_INPUT_BYTES} bytes a batch inference \
         job reads. Split the input and submit the job again."
    )
}

/// The records a `.jsonl` input file holds, or the sentence the job fails with.
///
/// One record per non-blank line, in the order the file lists them, because that is the
/// documented input format: "each individual record is a JSON object" on its own line. A
/// line that is not such an object, and a record that binds no `modelInput`, is the
/// validation criterion the status documents failing — reported against the line it was
/// read on, so a caller can find it.
fn parse_input_records(uri: &str, text: &str) -> InputVerdict {
    let mut records: Vec<InputRecord> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_number = index + 1;
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
            return InputVerdict::Rejected(format!(
                "Record {line_number} of the input data at {uri} is not a JSON object. Each \
                 individual record of a .jsonl input file must be a JSON object in the \
                 correct format."
            ));
        };
        let Some(model_input) = record.get(MODEL_INPUT).cloned() else {
            return InputVerdict::Rejected(format!(
                "Record {line_number} of the input data at {uri} carries no {MODEL_INPUT} \
                 member. Each individual record of a .jsonl input file must be a JSON object \
                 in the correct format."
            ));
        };
        if records.len() == MAX_INPUT_RECORDS {
            return InputVerdict::Rejected(format!(
                "The input data at {uri} holds more than the {MAX_INPUT_RECORDS} records a \
                 batch inference job accepts. Split the input and submit the job again."
            ));
        }
        records.push(InputRecord {
            record_id: record
                .get(RECORD_ID)
                .and_then(Value::as_str)
                .map(str::to_string),
            model_input,
        });
    }
    if records.is_empty() {
        return InputVerdict::Rejected(format!(
            "The input data at {uri} holds no records. A batch inference job runs over the \
             records of a .jsonl input file."
        ));
    }
    InputVerdict::Records(records)
}

/// The run one job still owes: everything the work step reads, collected while this
/// service's store is locked so the run itself holds no guard.
#[derive(Debug, Clone)]
pub struct DueRun {
    /// The `ModelInvocationJobArn` of the job whose run this is, which is the record the
    /// outcome is written back to.
    pub arn: String,
    /// The Region the job was submitted in, which is where its records are read and
    /// written.
    pub region: String,
    /// `modelId`, as the submission spelled it: the model each record is invoked on.
    pub model_id: String,
    /// `ModelInvocationType`: the invocation endpoint each record's `modelInput` is a
    /// request body for.
    pub invocation_type: &'static str,
    /// `inputDataConfig.s3InputDataConfig.s3Uri`.
    pub input_uri: String,
    /// `outputDataConfig.s3OutputDataConfig.s3Uri`.
    pub output_uri: String,
}

impl DueRun {
    /// The minted job id out of [`Self::arn`]: the trailing segment of a
    /// `ModelInvocationJobArn`, which is the folder a job's output objects live under.
    #[must_use]
    pub fn job_id(&self) -> &str {
        self.arn.rsplit('/').next().unwrap_or(self.arn.as_str())
    }
}

/// What a job's run established.
#[derive(Debug)]
pub enum RunOutcome {
    /// The run read its input, invoked the model over every record and stored the result
    /// records: `success` of them were served and `error` of them were not.
    Ran {
        /// `successRecordCount`: the records the model answered for.
        success: i64,
        /// `errorRecordCount`: the records the model could not be invoked for.
        error: i64,
        /// The sentence a job whose every record failed reports as its `message`.
        message: Option<String>,
    },
    /// The input the store served cannot be processed, so the job fails with this sentence.
    Failed(String),
    /// Nothing could be established or stored, so the run is still owed and a later call
    /// takes it again.
    Deferred,
}

/// Run one batch inference job: read its records, invoke the model on each one, and store
/// the result records under its output prefix.
///
/// It is the work the job's own clock already made due — the job has begun, and "InProgress
/// – this job has begun. You can start viewing the results in the output S3 location" — so
/// it is performed once per job ([`crate::state::ModelInvocationJobRecord::due_run`]) and
/// no later call performs it again: what a client fetched out of the output location once
/// stays what it fetched.
///
/// HOW MUCH work one call does is bounded by the job's own input, which validation already
/// bounded at [`MAX_INPUT_RECORDS`] records, and by nothing a later request sends.
pub async fn run_batch_inference<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    due: &DueRun,
) -> RunOutcome {
    let records = match read_input_records(ctx, &due.region, &due.input_uri).await {
        InputVerdict::Records(records) => records,
        InputVerdict::Rejected(message) => return RunOutcome::Failed(message),
        InputVerdict::Unknown => return RunOutcome::Deferred,
    };
    let Some((bucket, prefix)) = prefix_address(&due.output_uri) else {
        return RunOutcome::Failed(format!(
            "The output location {} does not name a location in Amazon S3. Point \
             outputDataConfig.s3OutputDataConfig.s3Uri at the prefix the results are to be \
             written under and submit the job again.",
            due.output_uri
        ));
    };

    // The model each record is invoked on, resolved through the very index the Amazon
    // Bedrock Runtime emulator's own inference resolves one through, so a batch job and a
    // synchronous call agree about which identifiers name an invocable model. A store this
    // engine does not carry at all is nothing established, so the run stays owed.
    let Some(runtime_state) = runtime::service_state(engine) else {
        return RunOutcome::Deferred;
    };
    let model = match runtime_state
        .lock()
        .resolve_model_for_inference(&due.model_id)
    {
        Ok(entry) => entry.clone(),
        Err(runtime::ModelRejection::Invalid(message) | runtime::ModelRejection::NotFound(message)) => {
            return RunOutcome::Failed(message)
        }
    };

    let mut success = 0i64;
    let mut error = 0i64;
    let mut first_error: Option<String> = None;
    let mut lines = String::new();
    for (index, record) in records.iter().enumerate() {
        let record_id = record
            .record_id
            .clone()
            .unwrap_or_else(|| minted_record_id(due.job_id(), index));
        let served = match due.invocation_type {
            crate::state::MODEL_INVOCATION_TYPE_CONVERSE => {
                converse_output(&model, &record.model_input)
            }
            _ => invoke_model_output(&model, &record.model_input),
        };
        let line = match served {
            Ok(model_output) => {
                success += 1;
                json!({
                    RECORD_ID: record_id,
                    MODEL_INPUT: record.model_input,
                    "modelOutput": model_output,
                })
            }
            Err(message) => {
                error += 1;
                if first_error.is_none() {
                    first_error = Some(message.clone());
                }
                json!({
                    RECORD_ID: record_id,
                    MODEL_INPUT: record.model_input,
                    "error": {"errorMessage": message},
                })
            }
        };
        lines.push_str(&line.to_string());
        lines.push('\n');
    }

    // WHERE the results go: a folder named for the job under the prefix the submission
    // named, holding one output file per input file it read, named for that file.
    let key = output_key(&prefix, due.job_id(), &due.input_uri);
    let client = s3_client(ctx, &due.region);
    if let Err(error) = client
        .put_object()
        .bucket(&bucket)
        .key(&key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            lines.into_bytes(),
        ))
        .content_type(OUTPUT_CONTENT_TYPE)
        .send()
        .await
    {
        if names_nothing(&error) {
            // S3 says there is no such bucket to write into: the job cannot store its
            // results where it was told to, which is a failure of the job.
            return RunOutcome::Failed(format!(
                "The results could not be written to {}. Make sure that the location exists \
                 and that the service role has access to it, then submit the job again.",
                due.output_uri
            ));
        }
        // Anything else leaves the results unstored without saying the location is wrong —
        // an emulator with no object store mounted, say — so the run stays owed and a later
        // call takes it again.
        ::tracing::warn!(
            bucket = ?bucket,
            key = ?key,
            detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the batch inference results were not stored in this emulator's Amazon S3"
        );
        return RunOutcome::Deferred;
    }
    ::tracing::info!(
        bucket = ?bucket,
        key = ?key,
        success,
        error,
        "the batch inference results were stored"
    );
    RunOutcome::Ran {
        success,
        error,
        // "If the batch inference job failed, this field contains a message describing why
        // the job failed": a job no record of which could be invoked failed, and what it
        // failed on is what the first of them answered.
        message: (success == 0).then(|| first_error.unwrap_or_default()),
    }
}

/// The key the output file of the input object `input_uri` is stored under: the prefix the
/// submission named, then the job's own id, then the input file's name with [`OUTPUT_SUFFIX`].
fn output_key(prefix: &str, job_id: &str, input_uri: &str) -> String {
    let input_name = input_uri
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("input.jsonl");
    if prefix.is_empty() {
        format!("{job_id}/{input_name}{OUTPUT_SUFFIX}")
    } else {
        format!("{prefix}/{job_id}/{input_name}{OUTPUT_SUFFIX}")
    }
}

/// The `recordId` a result record carries for an input record that bound none: "an
/// 11-character alphanumeric string", minted by the service rather than taken from the
/// input.
///
/// Derived from the job's own id and the record's position in its input, so the id a result
/// record is reported under is stable across reads of that output and no two records of one
/// job share one.
fn minted_record_id(job_id: &str, index: usize) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(job_id.as_bytes());
    digest.update(index.to_be_bytes());
    hex::encode(digest.finalize())[..RECORD_ID_LENGTH].to_string()
}

/// The `modelOutput` of one record invoked through the `InvokeModel` endpoint: the response
/// body in the model family's own envelope, as that operation answers with it.
fn invoke_model_output(model: &runtime::ModelEntry, model_input: &Value) -> Result<Value, String> {
    let Some(family) = model.family else {
        return Err(no_inference_message(model));
    };
    let body = serde_json::to_vec(model_input).map_err(|error| error.to_string())?;
    // The refusals the body itself earns, from the operation's own rule: a body that is not
    // a JSON object, or that omits a member the family's documented request schema
    // requires, cannot be invoked — so that record is one that failed to process rather
    // than one answered with an envelope nothing was inferred for.
    runtime::validate_invoke_request(family, None, None, &body, None, None)?;
    Ok(runtime::invoke_turn(model, family, &body).response)
}

/// The `modelOutput` of one record invoked through the `Converse` endpoint: the turn that
/// operation answers with, in the `ConverseResponse` envelope the model declares (`output`,
/// `stopReason`, `usage`, `metrics`).
///
/// The turn is composed from the same inference the Converse operation serves —
/// [`runtime::emulated_completion`], bounded by the model's own output ceiling and halted
/// at the first configured stop sequence — so a record of a batch job and a `Converse` call
/// carrying the same body are answered with the same text and the same token accounting.
fn converse_output(model: &runtime::ModelEntry, model_input: &Value) -> Result<Value, String> {
    if model.family.is_none() {
        return Err(no_inference_message(model));
    }
    // Not every base model is served by the Converse API, and the operation refuses one
    // that is not — so a record addressed to such a model is one that failed to process.
    if !model.supports_converse {
        return Err(format!(
            "The model {} does not support the Converse API.",
            model.model_id
        ));
    }
    let messages = model_input
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| {
            "The modelInput of a record invoked through the Converse endpoint must carry the \
             messages of the conversation."
                .to_string()
        })?;
    let inference_config = model_input.get("inferenceConfig");
    // "The maximum number of tokens to allow in the generated response", capped by the
    // model's own ceiling, exactly as the Converse operation reads it.
    let max_output_tokens = inference_config
        .and_then(|config| config.get("maxTokens"))
        .and_then(Value::as_u64)
        .and_then(|max| u32::try_from(max).ok())
        .unwrap_or(model.max_output_tokens)
        .min(model.max_output_tokens);
    let stop_sequences: Vec<String> = inference_config
        .and_then(|config| config.get("stopSequences"))
        .and_then(Value::as_array)
        .map(|sequences| {
            sequences
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let body = serde_json::to_vec(model_input).map_err(|error| error.to_string())?;
    let started = ::std::time::Instant::now();
    let input_tokens = runtime::count_text_tokens(&runtime::prompt_text_of_body(&body));
    let completion = runtime::emulated_completion(
        &model.model_id,
        u32::try_from(messages.len()).unwrap_or(u32::MAX),
        input_tokens,
        max_output_tokens,
        &stop_sequences,
    );
    let output_tokens = runtime::count_text_tokens(&completion.text);
    Ok(json!({
        "output": {
            "message": {
                "role": "assistant",
                "content": [{"text": completion.text}],
            },
        },
        // The modelled `StopReason` the turn ended on: the generation cap cut it short, or
        // it finished on its own.
        "stopReason": if completion.truncated { "max_tokens" } else { "end_turn" },
        "usage": {
            "inputTokens": input_tokens,
            "outputTokens": output_tokens,
            "totalTokens": input_tokens.saturating_add(output_tokens),
        },
        "metrics": {"latencyMs": runtime::latency_ms(started.elapsed())},
    }))
}

/// Why a record addressed to a model this emulator runs no inference for could not be
/// processed: the sentence the inference surface itself answers for such a model, so a
/// batch record and a synchronous call report the same reason.
fn no_inference_message(model: &runtime::ModelEntry) -> String {
    format!(
        "No model runs in this Amazon Bedrock Runtime emulator, so it serves no {} inference \
         for {}.",
        model.modality.label(),
        model.model_id
    )
}

#[cfg(test)]
mod tests {
    //! The parts of the work step that need no object store: how an `S3Uri` is read, where
    //! an output object goes, and which input files validation accepts.

    use super::*;

    /// One row of the address table: the URI, the object address it names and the location
    /// address it names.
    type AddressRow = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str)>,
        Option<(&'static str, &'static str)>,
    );

    /// RULE: an `S3Uri` is read as the bucket and key it names — an object address needs a
    /// non-empty key under the bucket, while a location may be the bucket alone — over the
    /// forms the member's own `@pattern` admits.
    #[test]
    fn an_s3_uri_is_read_as_the_bucket_and_key_it_names() {
        // (case, uri, object address, prefix address)
        let rows: [AddressRow; 7] = [
            (
                "a key at the bucket root",
                "s3://example-batch/input.jsonl",
                Some(("example-batch", "input.jsonl")),
                Some(("example-batch", "input.jsonl")),
            ),
            (
                "a nested key",
                "s3://example-batch/jobs/2026/input.jsonl",
                Some(("example-batch", "jobs/2026/input.jsonl")),
                Some(("example-batch", "jobs/2026/input.jsonl")),
            ),
            (
                "a prefix with a trailing separator",
                "s3://example-batch/output/",
                Some(("example-batch", "output")),
                Some(("example-batch", "output")),
            ),
            (
                "the bucket alone",
                "s3://example-batch",
                None,
                Some(("example-batch", "")),
            ),
            (
                "the bucket with a trailing separator",
                "s3://example-batch/",
                None,
                Some(("example-batch", "")),
            ),
            ("no bucket at all", "s3://", None, None),
            (
                "another scheme",
                "https://example-batch.s3.amazonaws.com/input.jsonl",
                None,
                None,
            ),
        ];

        for (case, uri, object, prefix) in rows {
            let actual = object_address(uri);
            let expected =
                object.map(|(bucket, key)| (bucket.to_string(), key.to_string()));
            assert_eq!(
                actual, expected,
                "case {case:?}: object address of {uri:?} expected {expected:?}, got {actual:?}"
            );
            let actual = prefix_address(uri);
            let expected =
                prefix.map(|(bucket, key)| (bucket.to_string(), key.to_string()));
            assert_eq!(
                actual, expected,
                "case {case:?}: prefix address of {uri:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the output of one input file is stored under the prefix the submission named,
    /// in a folder named for the job, under the input file's own name plus `.out`.
    #[test]
    fn the_output_object_is_named_for_the_job_and_its_input_file() {
        // (case, prefix, input uri, expected key)
        let rows: [(&str, &str, &str, &str); 4] = [
            (
                "a prefix",
                "output",
                "s3://example-batch/input.jsonl",
                "output/abcdefghijkl/input.jsonl.out",
            ),
            (
                "a nested prefix",
                "output/runs",
                "s3://example-batch/in/records.jsonl",
                "output/runs/abcdefghijkl/records.jsonl.out",
            ),
            (
                "the bucket root",
                "",
                "s3://example-batch/input.jsonl",
                "abcdefghijkl/input.jsonl.out",
            ),
            (
                "an input key that is one segment deep with a trailing separator",
                "out",
                "s3://example-batch/input.jsonl/",
                "out/abcdefghijkl/input.jsonl.out",
            ),
        ];

        for (case, prefix, input_uri, expected) in rows {
            let actual = output_key(prefix, "abcdefghijkl", input_uri);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a `.jsonl` input file is accepted when every non-blank line is a JSON object
    /// carrying `modelInput`, and the job is failed over the first line that is not.
    #[test]
    fn validation_accepts_the_documented_record_form_and_fails_every_other() {
        let records = match parse_input_records(
            "s3://example-batch/input.jsonl",
            "{\"recordId\":\"RECORD00001\",\"modelInput\":{\"prompt\":\"one\"}}\n\n\
             {\"modelInput\":{\"prompt\":\"two\"}}\n",
        ) {
            InputVerdict::Records(records) => records,
            other => panic!("two well-formed records must be accepted, got {other:?}"),
        };
        assert_eq!(
            records.len(),
            2,
            "a blank line is no record: expected 2 records, got {}",
            records.len()
        );
        assert_eq!(
            records[0].record_id.as_deref(),
            Some("RECORD00001"),
            "the record's own recordId is kept, got {:?}",
            records[0].record_id
        );
        assert!(
            records[1].record_id.is_none(),
            "a record that bound no recordId carries none, got {:?}",
            records[1].record_id
        );

        // (case, the file's text)
        let rejected: [(&str, &str); 4] = [
            ("a line that is not JSON", "not json at all\n"),
            (
                "a line that is a JSON value rather than an object",
                "[{\"modelInput\":{}}]\n",
            ),
            (
                "a record that binds no modelInput",
                "{\"recordId\":\"RECORD00001\"}\n",
            ),
            ("a file with no records", "\n   \n"),
        ];
        for (case, text) in rejected {
            let verdict = parse_input_records("s3://example-batch/input.jsonl", text);
            assert!(
                matches!(verdict, InputVerdict::Rejected(_)),
                "case {case:?}: expected the job to be failed over its input, got {verdict:?}"
            );
        }
    }

    /// RULE: a minted `recordId` is the 11-character alphanumeric string the documented
    /// input format describes, one per record of a job.
    #[test]
    fn a_minted_record_id_is_eleven_alphanumeric_characters() {
        let mut seen = ::std::collections::BTreeSet::new();
        for index in 0..32usize {
            let id = minted_record_id("abcdefghijkl", index);
            assert_eq!(
                id.len(),
                RECORD_ID_LENGTH,
                "record {index}: expected {RECORD_ID_LENGTH} characters, got {id:?}"
            );
            assert!(
                id.chars().all(char::is_alphanumeric),
                "record {index}: expected an alphanumeric id, got {id:?}"
            );
            assert!(
                seen.insert(id.clone()),
                "record {index}: {id:?} was already minted for another record"
            );
        }
    }
}
