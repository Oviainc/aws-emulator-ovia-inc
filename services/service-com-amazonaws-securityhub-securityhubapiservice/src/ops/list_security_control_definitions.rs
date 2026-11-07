// SPDX-License-Identifier: CC0-1.0

//! @op ListSecurityControlDefinitions
//! Reports the security controls Security Hub CSPM publishes: the standard-agnostic
//! control definitions of `crate::state`'s security-control catalog — all of them, or
//! the subset the standard a `StandardsArn` names applies — selected by `Providers`
//! and paged with `MaxResults`/`NextToken`.
//!
//! The catalog is the service's own published metadata, identical for every caller, so
//! this operation reads no account state and gates on no hub: it is one of the reads a
//! client makes to discover what it can enable and customize (see
//! [`crate::state::not_subscribed_message`]).
//!
//! ---
//!
//! # Modeled contract for `ListSecurityControlDefinitions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-securityhub-securityhubapiservice/smithy-model/com-amazonaws-securityhub-securityhubapiservice.json")
//! ```
//!
//! Lists all of the security controls that apply to a specified standard.
//!
//! ## Input
//!
//! * `MaxResults` (MaxResults, range 1..100) — An optional parameter that limits the total
//!   results of the API response to the specified number. If this parameter isn't provided in
//!   the request, the results include the first 25 security controls that apply to the
//!   specified standard. The results also include a NextToken parameter that you can use in a
//!   subsequent API call to get the next 25 controls. This repeats until all controls for the
//!   standard are returned.
//! * `NextToken` (NextToken) — Optional pagination parameter.
//! * `Providers` (SecurityControlsProviders) — A list of cloud providers to filter the
//!   security control definitions by. For example, specify Azure to return only controls that
//!   evaluate Azure resources.
//! * `StandardsArn` (NonEmptyString, pattern \S) — The Amazon Resource Name (ARN) of the
//!   standard that you want to view controls for.
//!
//! ## Output
//!
//! * `NextToken` (NextToken) — A pagination parameter that's included in the response only if
//!   it was included in the request.
//! * `SecurityControlDefinitions` (SecurityControlDefinitions) **required** — An array of
//!   controls that apply to the specified standard.
//!
//! ## Errors
//!
//! * `InternalException` (HTTP 500) — Internal server error.
//! * `InvalidAccessException` (HTTP 401) — The account doesn't have permission to perform
//!   this action.
//! * `InvalidInputException` (HTTP 400) — The request was rejected because you supplied an
//!   invalid or out-of-range value for an input parameter.
//! * `LimitExceededException` (HTTP 429) — The request was rejected because it attempted to
//!   create resources beyond the current Amazon Web Services account or throttling limits.
//!   The error code describes the limit exceeded.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListSecurityControlDefinitionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-securityhub-securityhubapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListSecurityControlDefinitionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.securityhub#ListSecurityControlDefinitions")]
#[derive(Debug)]
pub enum ListSecurityControlDefinitionsErrorModeled {
    InternalException(InternalException),
    InvalidAccessException(InvalidAccessException),
    InvalidInputException(InvalidInputException),
    LimitExceededException(LimitExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_security_control_definitions::ListSecurityControlDefinitionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListSecurityControlDefinitionsError = crate::scaffolded::Scaffolded<ListSecurityControlDefinitionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListSecurityControlDefinitionsErrorModeled> for ListSecurityControlDefinitionsError {
    fn from(modeled: ListSecurityControlDefinitionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListSecurityControlDefinitions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListSecurityControlDefinitions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListSecurityControlDefinitions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// `MaxResults`' upper bound (`@range(min: 1, max: 100)` on `MaxResults`).
const MAX_RESULTS_LIMIT: usize = 100;

/// The page size a request that names no `MaxResults` gets: "If this parameter isn't
/// provided in the request, the results include the first 25 security controls that
/// apply to the specified standard" (`smithy_query(query="doc",
/// shape="ListSecurityControlDefinitionsRequest", member="MaxResults")`).
const DEFAULT_PAGE_SIZE: usize = 25;

/// Marker this list's opaque `NextToken` carries, so a token minted for another
/// list — or a value a caller invented — is rejected rather than read as an offset.
const NEXT_TOKEN_PREFIX: &str = "securityhub:security-control-definitions:";

/// A `NextToken` resuming at `offset` in the published control order.
fn encode_next_token(offset: usize) -> String {
    crate::state::encode_list_token(NEXT_TOKEN_PREFIX, offset)
}

/// The offset a `NextToken` resumes at, or `None` when it is not one this operation
/// minted.
fn decode_next_token(token: &str) -> Option<usize> {
    crate::state::decode_list_token(NEXT_TOKEN_PREFIX, token)
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListSecurityControlDefinitions<Eng>
{
    type Input = ListSecurityControlDefinitionsInput;
    type Output = ListSecurityControlDefinitionsOutput;
    type Error = ListSecurityControlDefinitionsError;

    const NAME: &'static str = "ListSecurityControlDefinitions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SECURITY_CONTROL_DEFINITIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_SECURITY_CONTROL_DEFINITIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The published catalog is the service's own metadata, the same list for every
        // caller, so this operation reads no account state at all: the engine stays on
        // `self` as the constructor seam requires, unread.
        let _ = &self.engine;

        let invalid_input = |message: String| {
            ListSecurityControlDefinitionsError::Modeled(
                ListSecurityControlDefinitionsErrorModeled::InvalidInputException(
                    InvalidInputException {
                        message: Some(message),
                        code: Some("InvalidInputException".to_string()),
                    },
                ),
            )
        };

        // The only thing a request contributes to resolving a standard is the Region
        // its ARNs are qualified with.
        let region = crate::state::request_region(ctx);

        // A `Providers` list with no values constrains nothing, so it reads the same as
        // an omitted filter; any value the enum does not define is refused rather than
        // silently matching no control.
        let providers = input.providers.as_deref().unwrap_or_default();
        for provider in providers {
            if !SECURITY_CONTROLS_PROVIDER_VALUES.contains(&provider.as_str()) {
                return Err(invalid_input(format!(
                    "Providers: {provider} is not a SecurityControlsProvider; expected one of {}",
                    SECURITY_CONTROLS_PROVIDER_VALUES.join(", ")
                )));
            }
        }

        // "Lists all of the security controls that apply to a specified standard": a
        // request that names one is answered with that standard's controls, and an ARN
        // no published standard carries in this Region names no standard to list.
        let standard = match input.standards_arn.as_deref() {
            Some(standards_arn) => Some(
                crate::state::standard_by_arn(&region, standards_arn).ok_or_else(|| {
                    invalid_input(crate::state::arn_does_not_exist_message(standards_arn))
                })?,
            ),
            None => None,
        };

        let selected: Vec<SecurityControlDefinition> = crate::state::SECURITY_CONTROL_CATALOG
            .iter()
            .filter(|entry| {
                providers.is_empty()
                    || providers.iter().any(|wanted| wanted == entry.provider)
            })
            .filter(|entry| {
                standard.as_ref().is_none_or(|definition| {
                    crate::state::standard_applies_security_control(definition, entry)
                })
            })
            // The one definition of how a control definition is reported, shared with
            // every other operation that answers with a `SecurityControlDefinition`.
            .map(crate::state::security_control_definition_view)
            .collect();

        // `MaxResults` is modelled `@range(min: 1, max: 100)`, so the boundary
        // validator already refuses a value outside it; clamping here keeps the page
        // bounded even if this handler is reached another way.
        let page_size = input.max_results.map_or(DEFAULT_PAGE_SIZE, |requested| {
            usize::try_from(requested)
                .unwrap_or(DEFAULT_PAGE_SIZE)
                .clamp(1, MAX_RESULTS_LIMIT)
        });
        let offset = match input.next_token.as_deref() {
            Some(token) => decode_next_token(token).ok_or_else(|| {
                invalid_input("NextToken is not a token this operation issued".to_string())
            })?,
            None => 0,
        };
        if offset > selected.len() {
            return Err(invalid_input(
                "NextToken refers to a position past the end of the result set".to_string(),
            ));
        }

        // The catalog's published order is a total order over the controls, which is
        // what makes a `NextToken` page repeatable.
        let page: Vec<SecurityControlDefinition> = selected
            .iter()
            .skip(offset)
            .take(page_size)
            .cloned()
            .collect();
        let next_offset = offset + page.len();
        let next_token = (next_offset < selected.len()).then(|| encode_next_token(next_offset));

        Ok(ListSecurityControlDefinitionsOutput {
            security_control_definitions: page,
            next_token,
        })
    }
}
