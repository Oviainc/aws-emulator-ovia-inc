// SPDX-License-Identifier: CC0-1.0

//! @op ListTerminologies
//!
//! Pages over every stored custom terminology in name order, reporting each by
//! the same properties `ImportTerminology` reported for it, and hands back an
//! opaque `NextToken` that resumes strictly after the name the page ended on.
//!
//! Shared types live in `super::types`; the stored shapes and everything the
//! five terminology operations report by live in [`crate::state`].
//!
//! ---
//!
//! # Modeled contract for `ListTerminologies`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Provides a list of custom terminologies associated with your account.
//!
//! ## Input
//!
//! * `MaxResults` (MaxResultsInteger, range 1..500) — The maximum number of custom
//!   terminologies returned per list request.
//! * `NextToken` (NextToken, len 0..8192, pattern ^\p{ASCII}{0,8192}$) — If the result of the
//!   request to ListTerminologies was truncated, include the NextToken to fetch the next
//!   group of custom terminologies.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, len 0..8192, pattern ^\p{ASCII}{0,8192}$) — If the response to
//!   the ListTerminologies was truncated, the NextToken fetches the next group of custom
//!   terminologies.
//! * `TerminologyPropertiesList` (TerminologyPropertiesList) — The properties list of the
//!   custom terminologies returned on the list request.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — The value of the parameter is not valid.
//!   Review the value of the parameter you are using to correct it, and then retry your
//!   operation.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{
    decode_page_token, encode_page_token, invalid_parameter_value, page_size, service_state,
    terminology_properties,
};

/// The member a refusal about the pagination cursor names.
const NEXT_TOKEN_MEMBER: &str = "NextToken";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTerminologiesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTerminologiesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#ListTerminologies")]
#[derive(Debug)]
pub enum ListTerminologiesErrorModeled {
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_terminologies::ListTerminologiesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTerminologiesError = crate::scaffolded::Scaffolded<ListTerminologiesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTerminologiesErrorModeled> for ListTerminologiesError {
    fn from(modeled: ListTerminologiesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTerminologies` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTerminologies<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTerminologies<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTerminologies<Eng>
{
    type Input = ListTerminologiesInput;
    type Output = ListTerminologiesOutput;
    type Error = ListTerminologiesError;

    const NAME: &'static str = "ListTerminologies";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TERMINOLOGIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TERMINOLOGIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            ListTerminologiesErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;

        // A cursor this listing never issued is refused rather than read as a
        // name: the token is opaque to the caller, so the only tokens that can
        // resume a page are the ones a previous page handed back.
        let resume_after = match input.next_token.as_deref().filter(|token| !token.is_empty()) {
            None => None,
            Some(token) => match decode_page_token(token) {
                Some(name) => Some(name),
                None => {
                    return Err(
                        ListTerminologiesErrorModeled::InvalidParameterValueException(
                            invalid_parameter_value(
                                NEXT_TOKEN_MEMBER,
                                "it was not issued by a ListTerminologies page",
                            ),
                        )
                        .into(),
                    )
                }
            },
        };

        // The page size the caller asked for bounds the work this request does,
        // whatever the store holds.
        let page_size = page_size(input.max_results);

        let store = state.lock();
        let mut page = Vec::new();
        let mut next_token = None;
        let mut last_included: Option<&str> = None;
        for (name, record) in &store.terminologies {
            if resume_after
                .as_deref()
                .is_some_and(|last| name.as_str() <= last)
            {
                continue;
            }
            if page.len() >= page_size {
                // At least one terminology is left over, so hand back a cursor
                // that resumes strictly after the name this page ENDED on —
                // not after the one that did not fit, which would skip it.
                next_token = last_included.map(encode_page_token);
                break;
            }
            page.push(terminology_properties(record));
            last_included = Some(name.as_str());
        }

        Ok(ListTerminologiesOutput {
            terminology_properties_list: Some(page),
            next_token,
        })
    }
}
