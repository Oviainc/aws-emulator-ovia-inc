// SPDX-License-Identifier: CC0-1.0

//! @op PutPipelineDefinition
//!
//! Validates the candidate definition, then COMMITS it to the pipeline in
//! [`crate::state`], replacing any previous one, so `GetPipelineDefinition` reads back
//! exactly what was stored.
//!
//! Validation follows the two tiers the operation's own documentation (reproduced below)
//! describes:
//!
//! * **fatal** — "Changes to the pipeline are saved unless one of the following …
//!   validation errors exists": a string or reference field is empty, an object is
//!   missing a name or identifier, or the pipeline exceeds the maximum allowed objects.
//!   Nothing is saved and the request is refused with `InvalidRequestException`, which is
//!   what the second example in the docs below shows ("Could not save the pipeline
//!   definition due to FATAL errors"). The empty-identifier and empty-`refValue` cases
//!   are already refused by the modelled `length` constraints before this handler runs,
//!   and the FINISHED-pipeline case is unreachable while `ActivatePipeline` is
//!   scaffolded — see [`validate_definition`].
//! * **non-fatal** — anything else keeps the definition: it is stored, `errored` is true
//!   and the offending objects come back in `validationErrors`, which is exactly what the
//!   `errored` member documents ("the pipeline definition is stored but cannot be
//!   activated until you correct the pipeline").
//!
//! Neither `pipelineObjects` nor `fieldList` carries a modelled length constraint, so the
//! size checks run FIRST, outside the state lock, and refuse an oversized definition
//! before any per-object work: see [`MAX_PIPELINE_OBJECTS`], [`MAX_FIELDS_PER_OBJECT`] and
//! [`MAX_REPORTED_PROBLEMS`].
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutPipelineDefinition`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-datapipeline-datapipeline/smithy-model/com-amazonaws-datapipeline-datapipeline.json")
//! ```
//!
//! Adds tasks, schedules, and preconditions to the specified pipeline. You can use
//! PutPipelineDefinition to populate a new pipeline. PutPipelineDefinition also validates
//! the configuration as it adds it to the pipeline. Changes to the pipeline are saved
//! unless one of the following three validation errors exists in the pipeline. An object is
//! missing a name or identifier field. A string or reference field is empty. The number of
//! objects in the pipeline exceeds the maximum allowed objects. The pipeline is in a
//! FINISHED state. Pipeline object definitions are passed to the PutPipelineDefinition
//! action and returned by the GetPipelineDefinition action. Example 1 This example sets an
//! valid pipeline configuration and returns success. POST / HTTP/1.1 Content-Type:
//! application/x-amz-json-1.1 X-Amz-Target: DataPipeline.PutPipelineDefinition
//! Content-Length: 914 Host: datapipeline.us-east-1.amazonaws.com X-Amz-Date: Mon, 12 Nov
//! 2012 17:49:52 GMT Authorization: AuthParams {"pipelineId": "df-0937003356ZJEXAMPLE",
//! "pipelineObjects": [ {"id": "Default", "name": "Default", "fields": [ {"key":
//! "workerGroup", "stringValue": "workerGroup"} ] }, {"id": "Schedule", "name": "Schedule",
//! "fields": [ {"key": "startDateTime", "stringValue": "2012-12-12T00:00:00"}, {"key":
//! "type", "stringValue": "Schedule"}, {"key": "period", "stringValue": "1 hour"}, {"key":
//! "endDateTime", "stringValue": "2012-12-21T18:00:00"} ] }, {"id": "SayHello", "name":
//! "SayHello", "fields": [ {"key": "type", "stringValue": "ShellCommandActivity"}, {"key":
//! "command", "stringValue": "echo hello"}, {"key": "parent", "refValue": "Default"},
//! {"key": "schedule", "refValue": "Schedule"} ] } ] } HTTP/1.1 200 x-amzn-RequestId:
//! f74afc14-0754-11e2-af6f-6bc7a6be60d9 Content-Type: application/x-amz-json-1.1
//! Content-Length: 18 Date: Mon, 12 Nov 2012 17:50:53 GMT {"errored": false} Example 2 This
//! example sets an invalid pipeline configuration (the value for workerGroup is an empty
//! string) and returns an error message. POST / HTTP/1.1 Content-Type:
//! application/x-amz-json-1.1 X-Amz-Target: DataPipeline.PutPipelineDefinition
//! Content-Length: 903 Host: datapipeline.us-east-1.amazonaws.com X-Amz-Date: Mon, 12 Nov
//! 2012 17:49:52 GMT Authorization: AuthParams {"pipelineId": "df-06372391ZG65EXAMPLE",
//! "pipelineObjects": [ {"id": "Default", "name": "Default", "fields": [ {"key":
//! "workerGroup", "stringValue": ""} ] }, {"id": "Schedule", "name": "Schedule", "fields":
//! [ {"key": "startDateTime", "stringValue": "2012-09-25T17:00:00"}, {"key": "type",
//! "stringValue": "Schedule"}, {"key": "period", "stringValue": "1 hour"}, {"key":
//! "endDateTime", "stringValue": "2012-09-25T18:00:00"} ] }, {"id": "SayHello", "name":
//! "SayHello", "fields": [ {"key": "type", "stringValue": "ShellCommandActivity"}, {"key":
//! "command", "stringValue": "echo hello"}, {"key": "parent", "refValue": "Default"},
//! {"key": "schedule", "refValue": "Schedule"} ] } ] } HTTP/1.1 200 x-amzn-RequestId:
//! f74afc14-0754-11e2-af6f-6bc7a6be60d9 Content-Type: …
//!
//! ## Input
//!
//! * `parameterObjects` (ParameterObjectList) — The parameter objects used with the pipeline.
//! * `parameterValues` (ParameterValueList) — The parameter values used with the pipeline.
//! * `pipelineId` (id, len 1..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) **required** — The ID of
//!   the pipeline.
//! * `pipelineObjects` (PipelineObjectList) **required** — The objects that define the
//!   pipeline. These objects overwrite the existing pipeline definition.
//!
//! ## Output
//!
//! * `errored` (boolean, default false) **required** — Indicates whether there were
//!   validation errors, and the pipeline definition is stored but cannot be activated until
//!   you correct the pipeline and call PutPipelineDefinition to commit the corrected
//!   pipeline.
//! * `validationErrors` (ValidationErrors) — The validation errors that are associated with
//!   the objects defined in pipelineObjects.
//! * `validationWarnings` (ValidationWarnings) — The validation warnings that are associated
//!   with the objects defined in pipelineObjects.
//!
//! ## Errors
//!
//! * `InternalServiceError` (HTTP 500) — An internal service error occurred.
//! * `InvalidRequestException` (HTTP 400) — The request was not valid. Verify that your
//!   request was properly formatted, that the signature was generated with the correct
//!   credentials, and that you haven't exceeded any of the service limits for your account.
//! * `PipelineDeletedException` (HTTP 400) — The specified pipeline has been deleted.
//! * `PipelineNotFoundException` (HTTP 400) — The specified pipeline was not found. Verify
//!   that you used the correct user and account identifiers.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutPipelineDefinitionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-datapipeline-datapipeline.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutPipelineDefinitionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.datapipeline#PutPipelineDefinition")]
#[derive(Debug)]
pub enum PutPipelineDefinitionErrorModeled {
    InternalServiceError(InternalServiceError),
    InvalidRequestException(InvalidRequestException),
    PipelineDeletedException(PipelineDeletedException),
    PipelineNotFoundException(PipelineNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_pipeline_definition::PutPipelineDefinitionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutPipelineDefinitionError = crate::scaffolded::Scaffolded<PutPipelineDefinitionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutPipelineDefinitionErrorModeled> for PutPipelineDefinitionError {
    fn from(modeled: PutPipelineDefinitionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutPipelineDefinition` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutPipelineDefinition<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutPipelineDefinition<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutPipelineDefinition<Eng>
{
    type Input = PutPipelineDefinitionInput;
    type Output = PutPipelineDefinitionOutput;
    type Error = PutPipelineDefinitionError;

    const NAME: &'static str = "PutPipelineDefinition";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_PIPELINE_DEFINITION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_PIPELINE_DEFINITION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // Validate the candidate definition BEFORE touching the store. The work is sized
        // by the caller's request, so it must not run while the shared state lock is held.
        let validation = validate_definition(
            &input.pipeline_objects,
            input.parameter_objects.as_deref().unwrap_or_default(),
        );

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(PutPipelineDefinitionErrorModeled::InternalServiceError(
                InternalServiceError {
                    message: Some(
                        "the Data Pipeline store is not reachable from this engine".to_string(),
                    ),
                },
            )
            .into());
        };

        // Resolve the pipeline before reporting a definition problem, so an unknown or
        // deleted id is reported as such. One lock acquisition does the resolve and the
        // commit, so the existence this handler checked is the existence it writes to.
        let mut state = state.lock();
        let pipeline = state
            .pipeline_mut(&input.pipeline_id)
            .map_err(|lookup| lookup_error(lookup, &input.pipeline_id))?;

        if !validation.fatal.is_empty() {
            // A fatal problem means nothing is saved; the previous definition, if any,
            // stays untouched.
            return Err(PutPipelineDefinitionErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    message: Some(format!(
                        "Pipeline definition has errors: Could not save the pipeline \
                         definition due to FATAL errors: {}",
                        validation.fatal.join("; ")
                    )),
                },
            )
            .into());
        }

        pipeline.definition_version += 1;
        pipeline.definition = Some(crate::state::PipelineDefinition {
            pipeline_objects: input.pipeline_objects,
            parameter_objects: input.parameter_objects.unwrap_or_default(),
            parameter_values: input.parameter_values.unwrap_or_default(),
        });

        Ok(PutPipelineDefinitionOutput {
            errored: !validation.errors.is_empty(),
            // The documented success response is just `{"errored": false}`: an empty
            // list is omitted rather than sent as `[]`.
            validation_errors: Some(validation.errors).filter(|errors| !errors.is_empty()),
            validation_warnings: None,
        })
    }
}

/// The maximum number of objects one pipeline definition may carry.
///
/// Source: the AWS Data Pipeline service limits ("Number of objects per pipeline: 100"),
/// which the operation's documentation refers to as "the maximum allowed objects".
pub const MAX_PIPELINE_OBJECTS: usize = 100;

/// The maximum number of fields one pipeline object may carry.
///
/// Source: the AWS Data Pipeline service limits ("Number of fields per object: 50").
pub const MAX_FIELDS_PER_OBJECT: usize = 50;

/// How many problems one response reports.
///
/// Both lists are one entry per offending object or field, i.e. sized by the request, so
/// they are capped: the caller only needs enough to correct the definition, and an
/// uncapped list turns a large bad definition into a large response.
pub const MAX_REPORTED_PROBLEMS: usize = 10;

/// What validating a candidate definition found.
#[derive(Debug, Default)]
pub struct DefinitionValidation {
    /// Problems that stop the definition being saved at all. The operation answers
    /// `InvalidRequestException`; the pipeline keeps whatever definition it had. At most
    /// [`MAX_REPORTED_PROBLEMS`] entries.
    pub fatal: Vec<String>,
    /// Problems that do not stop the definition being saved. The definition is stored,
    /// `errored` is true, and these come back in `validationErrors`. At most
    /// [`MAX_REPORTED_PROBLEMS`] entries.
    pub errors: Vec<ValidationError>,
}

/// Validate a candidate definition, splitting what AWS calls FATAL errors from the
/// validation errors that still allow the definition to be stored.
///
/// Fatal, from the `PutPipelineDefinition` documentation:
///
/// 1. *"An object is missing a name or identifier field"* — `PipelineObject$id` and
///    `PipelineObject$name` are `@required` with `length: min 1`, so an absent or empty
///    one is refused by the boundary validator before this runs; nothing here can
///    observe it.
/// 2. *"A string or reference field is empty"* — `Field$stringValue` has `length: min 0`,
///    so an empty one reaches this handler and is checked here. `Field$refValue` has
///    `length: min 1`, so an empty reference is a boundary refusal.
/// 3. *"The number of objects in the pipeline exceeds the maximum allowed objects"* —
///    checked against [`MAX_PIPELINE_OBJECTS`], and against [`MAX_FIELDS_PER_OBJECT`] for
///    the fields of one object. Neither list is bounded by the model, so both are checked
///    FIRST and answered by returning immediately: every later loop here is O(objects ×
///    fields), so an oversized definition is refused before any of that work runs.
/// 4. *"The pipeline is in a FINISHED state"* — a pipeline only reaches a terminal run
///    state through activation, and `ActivatePipeline` is still scaffolded (HTTP 501), so
///    no pipeline in this emulator can be FINISHED yet.
///
/// Also fatal, from `Field`'s own documentation: *"The value is specified as either a
/// string value (StringValue) or a reference to another object (RefValue) but not as
/// both"* — a field carrying both, or neither, carries no value at all.
///
/// Non-fatal: a `refValue` naming an identifier that the definition does not define —
/// neither a pipeline object nor a parameter object. Such a definition is stored but
/// cannot be activated, which is the case `errored` exists to report.
#[must_use]
pub fn validate_definition(
    pipeline_objects: &[PipelineObject],
    parameter_objects: &[ParameterObject],
) -> DefinitionValidation {
    // The size checks come first and return at once: they are what bounds the work every
    // loop below does over a caller-chosen number of objects and fields.
    if pipeline_objects.len() > MAX_PIPELINE_OBJECTS {
        return DefinitionValidation {
            fatal: vec![format!(
                "the pipeline defines {} objects, more than the maximum of \
                 {MAX_PIPELINE_OBJECTS}",
                pipeline_objects.len()
            )],
            errors: Vec::new(),
        };
    }
    if let Some(oversized) = pipeline_objects
        .iter()
        .find(|object| object.fields.len() > MAX_FIELDS_PER_OBJECT)
    {
        return DefinitionValidation {
            fatal: vec![format!(
                "object {} defines {} fields, more than the maximum of \
                 {MAX_FIELDS_PER_OBJECT}",
                oversized.id,
                oversized.fields.len()
            )],
            errors: Vec::new(),
        };
    }

    let mut validation = DefinitionValidation::default();

    for object in pipeline_objects {
        for field in &object.fields {
            if validation.fatal.len() >= MAX_REPORTED_PROBLEMS {
                break;
            }
            match (field.string_value.as_deref(), field.ref_value.as_deref()) {
                (Some(""), None) => validation.fatal.push(format!(
                    "field {} of object {} has an empty stringValue",
                    field.key, object.id
                )),
                (Some(_), None) | (None, Some(_)) => {}
                (Some(_), Some(_)) => validation.fatal.push(format!(
                    "field {} of object {} sets both stringValue and refValue",
                    field.key, object.id
                )),
                (None, None) => validation.fatal.push(format!(
                    "field {} of object {} sets neither stringValue nor refValue",
                    field.key, object.id
                )),
            }
        }
    }

    let defined: std::collections::HashSet<&str> = pipeline_objects
        .iter()
        .map(|object| object.id.as_str())
        .chain(
            parameter_objects
                .iter()
                .map(|parameter| parameter.id.as_str()),
        )
        .collect();
    for object in pipeline_objects {
        if validation.errors.len() >= MAX_REPORTED_PROBLEMS {
            break;
        }
        let unresolved: Vec<String> = object
            .fields
            .iter()
            .filter_map(|field| {
                let referenced = field.ref_value.as_deref()?;
                (!defined.contains(referenced)).then(|| {
                    format!(
                        "field {} references object {referenced}, which the definition \
                         does not define",
                        field.key
                    )
                })
            })
            .collect();
        if !unresolved.is_empty() {
            validation.errors.push(ValidationError {
                id: Some(object.id.clone()),
                errors: Some(unresolved),
            });
        }
    }

    validation
}

/// The modelled error a failed pipeline lookup becomes for this operation.
fn lookup_error(
    lookup: crate::state::PipelineLookupError,
    pipeline_id: &str,
) -> PutPipelineDefinitionError {
    let message = Some(lookup.message(pipeline_id));
    match lookup {
        crate::state::PipelineLookupError::Unknown => {
            PutPipelineDefinitionErrorModeled::PipelineNotFoundException(
                PipelineNotFoundException { message },
            )
        }
        crate::state::PipelineLookupError::Deleted => {
            PutPipelineDefinitionErrorModeled::PipelineDeletedException(
                PipelineDeletedException { message },
            )
        }
    }
    .into()
}
