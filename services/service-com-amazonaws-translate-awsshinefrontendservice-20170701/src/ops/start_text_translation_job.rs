// SPDX-License-Identifier: CC0-1.0

//! @op StartTextTranslationJob
//!
//! Submits one asynchronous batch translation job: the documents under the
//! request's input location are translated into each requested target language
//! and written to its output location, and the job record the two reading
//! operations report is persisted.
//!
//! The job's data plane is REAL: the documents are read out of — and their
//! translations written back into — the emulator's own object storage, in
//! process, so the counts, the message and the terminal status
//! `DescribeTextTranslationJob` reports are what this job did. A job whose
//! input or output location cannot be used reaches the modelled `FAILED`
//! status rather than reporting a success nothing backs. The translation of
//! one document is the transform every translate operation of this service
//! shares ([`crate::state`]); the job's lifecycle and its data plane live in
//! [`crate::state::batch_translation`].
//!
//! ---
//!
//! # Modeled contract for `StartTextTranslationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Starts an asynchronous batch translation job. Use batch translation jobs to translate
//! large volumes of text across multiple documents at once. For batch translation, you can
//! input documents with different source languages (specify auto as the source language).
//! You can specify one or more target languages. Batch translation translates each input
//! document into each of the target languages. For more information, see Asynchronous batch
//! processing. Batch translation jobs can be described with the DescribeTextTranslationJob
//! operation, listed with the ListTextTranslationJobs operation, and stopped with the
//! StopTextTranslationJob operation.
//!
//! ## Input
//!
//! * `ClientToken` (ClientTokenString, len 1..64, pattern ^[a-zA-Z0-9-]+$, @idempotencyToken)
//!   **required** — A unique identifier for the request. This token is generated for you when
//!   using the Amazon Translate SDK.
//! * `DataAccessRoleArn` (IamRoleArn, len 20..2048, pattern
//!   ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$) **required** — The Amazon Resource Name (ARN)
//!   of an AWS Identity Access and Management (IAM) role that grants Amazon Translate read
//!   access to your input data. For more information, see Identity and access management .
//! * `InputDataConfig` (InputDataConfig) **required** — Specifies the format and location of
//!   the input documents for the translation job.
//! * `JobName` (JobName, len 1..256, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$) — The name of
//!   the batch translation job to be performed.
//! * `OutputDataConfig` (OutputDataConfig) **required** — Specifies the S3 folder to which
//!   your job output will be saved.
//! * `ParallelDataNames` (ResourceNameList) — The name of a parallel data resource to add to
//!   the translation job. This resource consists of examples that show how you want segments
//!   of text to be translated. If you specify multiple target languages for the job, the
//!   parallel data file must include translations for all the target languages. When you add
//!   parallel data to a translation job, you create an Active Custom Translation job. This
//!   parameter accepts only one parallel data resource. Active Custom Translation jobs are
//!   priced at a higher rate than other jobs that don't use parallel data. For more
//!   information, see Amazon Translate pricing. For a list of available parallel data
//!   resources, use the ListParallelData operation. For more …
//! * `Settings` (TranslationSettings) — Settings to configure your translation output. You
//!   can configure the following options: Brevity: not supported. Formality: sets the
//!   formality level of the output text. Profanity: masks profane words and phrases in your
//!   translation output.
//! * `SourceLanguageCode` (LanguageCodeString, len 2..5) **required** — The language code of
//!   the input language. Specify the language if all input documents share the same language.
//!   If you don't know the language of the source files, or your input documents contains
//!   different source languages, select auto. Amazon Translate auto detects the source
//!   language for each input document. For a list of supported language codes, see Supported
//!   languages.
//! * `TargetLanguageCodes` (TargetLanguageCodeStringList, len 1..) **required** — The target
//!   languages of the translation job. Enter up to 10 language codes. Each input file is
//!   translated into each target language. Each language code is 2 or 5 characters long. For
//!   a list of language codes, see Supported languages.
//! * `TerminologyNames` (ResourceNameList) — The name of a custom terminology resource to add
//!   to the translation job. This resource lists examples source terms and the desired
//!   translation for each term. This parameter accepts only one custom terminology resource.
//!   If you specify multiple target languages for the job, translate uses the designated
//!   terminology for each requested target language that has an entry for the source term in
//!   the terminology file. For a list of available custom terminology resources, use the
//!   ListTerminologies operation. For more information, see Custom terminology.
//!
//! ## Output
//!
//! * `JobId` (JobId, len 1..32, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$) — The identifier
//!   generated for the job. To get the status of a job, use this ID with the
//!   DescribeTextTranslationJob operation.
//! * `JobStatus` (JobStatus, one of: COMPLETED | COMPLETED_WITH_ERROR | FAILED | IN_PROGRESS
//!   | STOPPED | STOP_REQUESTED | SUBMITTED) — The status of the job. Possible values
//!   include: SUBMITTED - The job has been received and is queued for processing. IN_PROGRESS
//!   - Amazon Translate is processing the job. COMPLETED - The job was successfully completed
//!   and the output is available. COMPLETED_WITH_ERROR - The job was completed with errors.
//!   The errors can be analyzed in the job's output. FAILED - The job did not complete. To
//!   get details, use the DescribeTextTranslationJob operation. STOP_REQUESTED - The user who
//!   started the job has requested that it be stopped. STOPPED - The job has been stopped.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — The value of the parameter is not valid.
//!   Review the value of the parameter you are using to correct it, and then retry your
//!   operation.
//! * `InvalidRequestException` (HTTP 400) — The request that you made is not valid. Check
//!   your request to determine why it's not valid and then retry the request.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.
//! * `UnsupportedLanguagePairException` (HTTP 400) — Amazon Translate does not support
//!   translation from the language of the source text into the requested target language. For
//!   more information, see Supported languages.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

use crate::state::batch_translation::{
    job_id, parse_s3_uri, run_batch, BatchOutcome, BatchPlan, JobOutcome, TranslationJob,
    CONTENT_TYPE_PLAIN, JOB_PHASE, MAX_PARALLEL_DATA_PER_JOB, MAX_STORED_JOBS,
    MAX_TARGET_LANGUAGES,
};
use crate::state::{
    invalid_parameter_value, is_supported_language, request_region, resource_not_found,
    service_state, terminology_not_found, unsupported_language_pair, AUTO_LANGUAGE_CODE,
    MAX_TERMINOLOGIES_PER_TRANSLATION, TERMINOLOGY_RESOURCE,
};

/// The member a refusal names when the job's target languages are what is
/// wrong with the request.
const TARGET_LANGUAGE_CODES_MEMBER: &str = "TargetLanguageCodes";
/// The member that declares the format of the input documents.
const CONTENT_TYPE_MEMBER: &str = "InputDataConfig.ContentType";
/// The member that names where the input documents are.
const INPUT_S3_URI_MEMBER: &str = "InputDataConfig.S3Uri";
/// The member that names where the translations are written.
const OUTPUT_S3_URI_MEMBER: &str = "OutputDataConfig.S3Uri";
/// The member that names the custom terminologies to apply.
const TERMINOLOGY_NAMES_MEMBER: &str = "TerminologyNames";
/// The member that names the parallel data resources to apply.
const PARALLEL_DATA_NAMES_MEMBER: &str = "ParallelDataNames";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartTextTranslationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartTextTranslationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#StartTextTranslationJob")]
#[derive(Debug)]
pub enum StartTextTranslationJobErrorModeled {
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
    UnsupportedLanguagePairException(UnsupportedLanguagePairException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_text_translation_job::StartTextTranslationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartTextTranslationJobError = crate::scaffolded::Scaffolded<StartTextTranslationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartTextTranslationJobErrorModeled> for StartTextTranslationJobError {
    fn from(modeled: StartTextTranslationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartTextTranslationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartTextTranslationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartTextTranslationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartTextTranslationJob<Eng>
{
    type Input = StartTextTranslationJobInput;
    type Output = StartTextTranslationJobOutput;
    type Error = StartTextTranslationJobError;

    const NAME: &'static str = "StartTextTranslationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_TEXT_TRANSLATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_TEXT_TRANSLATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = service_state(&*self.engine).ok_or_else(|| {
            internal_server_error("the batch translation job store is unavailable")
        })?;
        let region = request_region(ctx.parts());
        let now = SystemTime::now();

        // 1. What the REQUEST alone settles. Every one of these is decided
        //    before a single document is read, so a submission that cannot be
        //    carried out costs no data-plane work.
        let target_language_codes = input.target_language_codes.clone();
        if target_language_codes.len() > MAX_TARGET_LANGUAGES {
            return Err(invalid_parameter(
                TARGET_LANGUAGE_CODES_MEMBER,
                &format!(
                    "a batch translation job translates into at most \
                     {MAX_TARGET_LANGUAGES} target languages, and this request names {}",
                    target_language_codes.len()
                ),
            ));
        }
        let source_language_code = input.source_language_code.clone();
        // Amazon Translate translates between the languages it supports, and
        // never from a language into itself. `auto` is not one of them: it asks
        // the service to determine each document's source language.
        if source_language_code != AUTO_LANGUAGE_CODE
            && !is_supported_language(&source_language_code)
        {
            let target = target_language_codes.first().cloned().unwrap_or_default();
            return Err(unsupported_pair(&source_language_code, &target));
        }
        for target in &target_language_codes {
            if !is_supported_language(target) || *target == source_language_code {
                return Err(unsupported_pair(&source_language_code, target));
            }
        }

        // The documents are read as the format the request declares them to be
        // in, so a format this emulator's batch data plane does not read is
        // refused here rather than reported as a job that translated nothing.
        if input.input_data_config.content_type != CONTENT_TYPE_PLAIN {
            return Err(invalid_parameter(
                CONTENT_TYPE_MEMBER,
                &format!(
                    "this emulator's batch translation reads {CONTENT_TYPE_PLAIN} input \
                     documents, and this request declares {}",
                    input.input_data_config.content_type
                ),
            ));
        }
        let Some(input_location) = parse_s3_uri(&input.input_data_config.s3_uri) else {
            return Err(invalid_parameter(
                INPUT_S3_URI_MEMBER,
                "it names no Amazon S3 bucket",
            ));
        };
        let Some(output_location) = parse_s3_uri(&input.output_data_config.s3_uri) else {
            return Err(invalid_parameter(
                OUTPUT_S3_URI_MEMBER,
                "it names no Amazon S3 bucket",
            ));
        };

        let terminology_names = input.terminology_names.clone().unwrap_or_default();
        // "Only one terminology can be applied per StartTextTranslationJob
        // request at this time."
        if terminology_names.len() > MAX_TERMINOLOGIES_PER_TRANSLATION {
            return Err(invalid_parameter(
                TERMINOLOGY_NAMES_MEMBER,
                &format!(
                    "a batch translation job applies at most \
                     {MAX_TERMINOLOGIES_PER_TRANSLATION} custom terminology resource"
                ),
            ));
        }
        let parallel_data_names = input.parallel_data_names.clone().unwrap_or_default();
        // "This parameter accepts only one parallel data resource."
        if parallel_data_names.len() > MAX_PARALLEL_DATA_PER_JOB {
            return Err(invalid_parameter(
                PARALLEL_DATA_NAMES_MEMBER,
                &format!(
                    "a batch translation job applies at most {MAX_PARALLEL_DATA_PER_JOB} \
                     parallel data resource"
                ),
            ));
        }

        let job_id = job_id(&region, &input.client_token);

        // 2. What the STORE settles, read under ONE lock: the submission this
        //    token already started, the account's room for another job, and
        //    the terminology records the job translates with.
        let terminologies = {
            let store = state.lock();
            if let Some(started) = store.job_for_client_token(&input.client_token) {
                return Ok(StartTextTranslationJobOutput {
                    job_id: Some(started.job_id.clone()),
                    job_status: Some(started.status(now).to_string()),
                });
            }
            if store.jobs.len() >= MAX_STORED_JOBS {
                return Err(too_many_jobs());
            }
            // A parallel data resource the account does not hold names nothing
            // this job could apply. Nothing creates one in this store yet, so
            // every name a request gives is one no resource answers to.
            if !parallel_data_names.is_empty() {
                return Err(StartTextTranslationJobErrorModeled::ResourceNotFoundException(
                    resource_not_found(),
                )
                .into());
            }
            let mut records = Vec::with_capacity(terminology_names.len());
            for name in &terminology_names {
                let Some(record) = store.terminologies.get(name) else {
                    return Err(
                        StartTextTranslationJobErrorModeled::ResourceNotFoundException(
                            terminology_not_found(TERMINOLOGY_RESOURCE, name),
                        )
                        .into(),
                    );
                };
                // Copied out of the store, so the documents below are
                // translated with no lock held.
                records.push(record.clone());
            }
            records
        };

        // 3. The job's own work: every document under the input location is
        //    translated into each target language and written to the output
        //    location, awaited with NO store lock held.
        let plan = BatchPlan {
            job_id: job_id.clone(),
            input: input_location,
            output: output_location,
            source_language_code: source_language_code.clone(),
            target_language_codes: target_language_codes.clone(),
            terminologies,
            settings: input.settings.clone(),
        };
        // The Amazon S3 client is the workspace's ONE in-process construction
        // of it (`service_common::s3_client`), so this service configures that
        // hop no differently from any other: the transport is the emulator's
        // own callback, and nothing outside this process is reachable from it.
        let client = ::service_common::s3_client::s3_client(ctx.callback(), &region);
        let (outcome, carried_out) = match run_batch(&client, &plan).await {
            Ok(outcome) => (outcome, true),
            // The job could not be carried out at all: it ends in the modelled
            // FAILED status carrying the reason, and nothing is reported as
            // translated.
            Err(reason) => (
                BatchOutcome {
                    message: Some(reason),
                    ..BatchOutcome::default()
                },
                false,
            ),
        };
        let job = TranslationJob {
            job_id: job_id.clone(),
            job_name: input.job_name,
            client_token: input.client_token,
            source_language_code,
            target_language_codes,
            terminology_names,
            parallel_data_names,
            input_data_config: input.input_data_config,
            output_data_config: input.output_data_config,
            data_access_role_arn: input.data_access_role_arn,
            settings: input.settings,
            submitted_at: now,
            // The job is reported as submitted first and reaches its terminal
            // status a phase later, so a caller that polls sees the lifecycle
            // the model documents.
            in_progress_at: now + JOB_PHASE,
            finished_at: now + JOB_PHASE + JOB_PHASE,
            outcome: if !carried_out {
                JobOutcome::Failed
            } else if outcome.documents_with_errors > 0 {
                JobOutcome::CompletedWithError
            } else {
                JobOutcome::Completed
            },
            message: outcome.message,
            input_documents: outcome.input_documents,
            translated_documents: outcome.translated_documents,
            documents_with_errors: outcome.documents_with_errors,
            stop_requested_at: None,
        };

        let mut store = state.lock();
        // The status this submission ANSWERS with is read off the clock as it is
        // answered, not as the request arrived, so the answer and a read taken
        // straight after it agree even when the data-plane work above took a
        // phase or more.
        let answered = SystemTime::now();
        // Everything the submission was judged against is re-checked under the
        // lock that stores the record: the data-plane work above was awaited
        // without the lock, so another submission of the same token, or of the
        // last job this account has room for, may have landed in between.
        if let Some(started) = store.job_for_client_token(&job.client_token) {
            return Ok(StartTextTranslationJobOutput {
                job_id: Some(started.job_id.clone()),
                job_status: Some(started.status(answered).to_string()),
            });
        }
        if store.jobs.len() >= MAX_STORED_JOBS {
            return Err(too_many_jobs());
        }
        let job_status = job.status(answered).to_string();
        store.jobs.insert(job_id.clone(), job);

        Ok(StartTextTranslationJobOutput {
            job_id: Some(job_id),
            job_status: Some(job_status),
        })
    }
}

/// The modelled `InternalServerException` a request that cannot reach this
/// service's store is answered with.
fn internal_server_error(reason: &str) -> StartTextTranslationJobError {
    StartTextTranslationJobErrorModeled::InternalServerException(InternalServerException {
        message: Some(reason.to_string()),
    })
    .into()
}

/// The modelled `InvalidParameterValueException` for a member of this request
/// the service cannot act on, worded the way this service words that class.
fn invalid_parameter(parameter: &str, reason: &str) -> StartTextTranslationJobError {
    StartTextTranslationJobErrorModeled::InvalidParameterValueException(
        invalid_parameter_value(parameter, reason),
    )
    .into()
}

/// The modelled `UnsupportedLanguagePairException` for a pair this service does
/// not translate, worded the way every translate operation words that class.
fn unsupported_pair(source: &str, target: &str) -> StartTextTranslationJobError {
    StartTextTranslationJobErrorModeled::UnsupportedLanguagePairException(
        unsupported_language_pair(source, target),
    )
    .into()
}

/// The refusal a submission gets once this account's store holds every batch
/// translation job it can: the job record is never deleted, so the bound is
/// what keeps a caller looping on submission from growing the store without
/// limit.
fn too_many_jobs() -> StartTextTranslationJobError {
    StartTextTranslationJobErrorModeled::TooManyRequestsException(TooManyRequestsException {
        message: Some(format!(
            "You have made too many requests within a short period of time. Wait for a short \
             time and then try your request again. This account holds \
             {MAX_STORED_JOBS} batch translation jobs."
        )),
    })
    .into()
}
