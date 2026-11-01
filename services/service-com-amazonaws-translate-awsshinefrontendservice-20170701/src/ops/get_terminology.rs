// SPDX-License-Identifier: CC0-1.0

//! @op GetTerminology
//!
//! Reports the stored custom terminology of that name — the same properties
//! `ImportTerminology` reported for it — plus the location its input file is
//! presigned from. A name no import stored is a 404, never an empty success.
//!
//! Shared types live in `super::types`; the stored shapes and everything the
//! five terminology operations report by live in [`crate::state`].
//!
//! ---
//!
//! # Modeled contract for `GetTerminology`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Retrieves a custom terminology.
//!
//! ## Input
//!
//! * `Name` (ResourceName, len 1..256, pattern ^([A-Za-z0-9-]_?)+$) **required** — The name
//!   of the custom terminology being retrieved.
//! * `TerminologyDataFormat` (TerminologyDataFormat, one of: CSV | TMX | TSV) — The data
//!   format of the custom terminology being retrieved. If you don't specify this parameter,
//!   Amazon Translate returns a file with the same format as the file that was imported to
//!   create the terminology. If you specify this parameter when you retrieve a
//!   multi-directional terminology resource, you must specify the same format as the input
//!   file that was imported to create it. Otherwise, Amazon Translate throws an error.
//!
//! ## Output
//!
//! * `AuxiliaryDataLocation` (TerminologyDataLocation) — The Amazon S3 location of a file
//!   that provides any errors or warnings that were produced by your input file. This file
//!   was created when Amazon Translate attempted to create a terminology resource. The
//!   location is returned as a presigned URL to that has a 30-minute expiration.
//! * `TerminologyDataLocation` (TerminologyDataLocation) — The Amazon S3 location of the most
//!   recent custom terminology input file that was successfully imported into Amazon
//!   Translate. The location is returned as a presigned URL that has a 30-minute expiration.
//!   Amazon Translate doesn't scan all input files for the risk of CSV injection attacks. CSV
//!   injection occurs when a .csv or .tsv file is altered so that a record contains malicious
//!   code. The record begins with a special character, such as =, +, -, or @. When the file
//!   is opened in a spreadsheet program, the program might interpret the record as a formula
//!   and run the code within it. Before you download an input file from Amazon S3, ensure
//!   that you recognize the file and trust its creator.
//! * `TerminologyProperties` (TerminologyProperties) — The properties of the custom
//!   terminology being retrieved.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — The value of the parameter is not valid.
//!   Review the value of the parameter you are using to correct it, and then retry your
//!   operation.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{
    auxiliary_data_location, invalid_parameter_value, request_region, service_state,
    terminology_data_location, terminology_not_found, terminology_properties,
    TERMINOLOGY_RESOURCE,
};

/// The member a refusal about the requested format names.
const FORMAT_MEMBER: &str = "TerminologyDataFormat";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetTerminologyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetTerminologyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#GetTerminology")]
#[derive(Debug)]
pub enum GetTerminologyErrorModeled {
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_terminology::GetTerminologyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetTerminologyError = crate::scaffolded::Scaffolded<GetTerminologyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetTerminologyErrorModeled> for GetTerminologyError {
    fn from(modeled: GetTerminologyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetTerminology` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetTerminology<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetTerminology<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetTerminology<Eng>
{
    type Input = GetTerminologyInput;
    type Output = GetTerminologyOutput;
    type Error = GetTerminologyError;

    const NAME: &'static str = "GetTerminology";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_TERMINOLOGY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_TERMINOLOGY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = service_state(&*self.engine).ok_or_else(|| {
            GetTerminologyErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;
        let region = request_region(ctx.parts());

        let store = state.lock();
        let record = store.terminologies.get(&input.name).ok_or_else(|| {
            GetTerminologyErrorModeled::ResourceNotFoundException(terminology_not_found(
                TERMINOLOGY_RESOURCE,
                &input.name,
            ))
        })?;

        // "If you don't specify this parameter, Amazon Translate returns a file
        // with the same format as the file that was imported to create the
        // terminology. If you specify this parameter when you retrieve a
        // multi-directional terminology resource, you must specify the same
        // format as the input file that was imported to create it. Otherwise,
        // Amazon Translate throws an error."
        let requested_format = input.terminology_data_format.unwrap_or_default();
        let format = if requested_format.is_empty() {
            record.format.clone()
        } else if record.is_multi_directional() && requested_format != record.format {
            return Err(GetTerminologyErrorModeled::InvalidParameterValueException(
                invalid_parameter_value(
                    FORMAT_MEMBER,
                    &format!(
                        "the multi-directional terminology {} was imported as {} and can only \
                         be retrieved in that format",
                        record.name, record.format
                    ),
                ),
            )
            .into());
        } else {
            requested_format
        };

        Ok(GetTerminologyOutput {
            terminology_properties: Some(terminology_properties(record)),
            terminology_data_location: Some(terminology_data_location(&region, record, &format)),
            auxiliary_data_location: auxiliary_data_location(&region, record),
        })
    }
}
