// SPDX-License-Identifier: CC0-1.0

//! @op ListGuardrails
//!
//! Enumerates the stored guardrails out of [`crate::state`] as `GuardrailSummary`
//! items. Per the model's own documentation, an absent `guardrailIdentifier` lists the
//! DRAFT of every guardrail, while an identifier lists EVERY version of that one
//! guardrail (its DRAFT, then its numbered versions ascending).
//!
//! Pagination is a cursor over the total order the listing walks — `(guardrailId,
//! version rank)` — so a page resumes exactly where the previous one ended whatever was
//! created or deleted in between. `@paginated` names `nextToken`/`guardrails`.
//!
//! This operation is `@readonly`: it writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListGuardrails`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Lists details about all the guardrails in an account. To list the DRAFT version of all
//! your guardrails, don't specify the guardrailIdentifier field. To list all versions of a
//! guardrail, specify the ARN of the guardrail in the guardrailIdentifier field. You can
//! set the maximum number of results to return in a response in the maxResults field. If
//! there are more results than the number you set, the response returns a nextToken that
//! you can send in another ListGuardrails request to see the next batch of results.
//!
//! ## Input
//!
//! * `guardrailIdentifier` (GuardrailIdentifier, len ..2048, pattern
//!   ^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$)
//!   — The unique identifier of the guardrail. This can be an ID or the ARN.
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in
//!   the response.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If there are more results
//!   than were returned in the response, the response returns a nextToken that you can send
//!   in another ListGuardrails request to see the next batch of results.
//!
//! ## Output
//!
//! * `guardrails` (GuardrailSummaries, len ..1000) **required** — A list of objects, each of
//!   which contains details about a guardrail.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If there are more results
//!   than were returned in the response, the response returns a nextToken that you can send
//!   in another ListGuardrails request to see the next batch of results.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListGuardrailsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListGuardrailsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListGuardrails")]
#[derive(Debug)]
pub enum ListGuardrailsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_guardrails::ListGuardrailsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListGuardrailsError = crate::scaffolded::Scaffolded<ListGuardrailsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListGuardrailsErrorModeled> for ListGuardrailsError {
    fn from(modeled: ListGuardrailsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListGuardrails` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListGuardrails<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListGuardrails<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The largest page `ListGuardrails` answers with, and the page size an absent
/// `maxResults` takes: the `guardrails` list is modelled `len ..1000` and
/// `maxResults` `range 1..1000`, so 1000 is the whole span either admits.
const MAX_PAGE_SIZE: usize = 1000;

/// The listing's cursor: a guardrail id, then the revision's rank within that
/// guardrail (the DRAFT first as rank 0, then each numbered version).
///
/// Rendered so BYTE order over cursors IS that total order — `/` (0x2F) sorts below
/// every character `GuardrailId` (`^[a-z0-9]+$`) can carry, and the rank is
/// fixed-width — which is what lets a page resume at "the first row at or after this
/// cursor" even when the row the previous page ended on has since been deleted.
fn cursor_of(id: &str, version: &str) -> String {
    let rank = crate::state::version_number(version).unwrap_or(0);
    format!("{id}/{rank:08}")
}

/// Whether `token` is a cursor THIS listing issued, so a token from somewhere else —
/// including from the other shape of this listing — is refused rather than silently
/// read as an empty or wrong page.
fn cursor_belongs(token: &str, filtered_id: Option<&str>) -> bool {
    let Some((id, rank)) = token.rsplit_once('/') else {
        return false;
    };
    if id.is_empty() || rank.len() != 8 || !rank.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match filtered_id {
        // A filtered listing walks one guardrail's revisions, so its cursor names that
        // guardrail.
        Some(filtered_id) => id == filtered_id,
        // An unfiltered listing walks only DRAFTs, so its cursor is always at rank 0.
        None => rank.bytes().all(|b| b == b'0'),
    }
}

/// One listed revision as the `GuardrailSummary` the response carries.
fn summary(
    guardrail: &crate::state::Guardrail,
    revision: &crate::state::GuardrailRevision,
) -> GuardrailSummary {
    GuardrailSummary {
        id: guardrail.id.clone(),
        arn: guardrail.arn.clone(),
        status: revision.status.to_string(),
        name: guardrail.name.clone(),
        description: revision.description.clone(),
        version: revision.version.clone(),
        created_at: crate::state::date_time(revision.created_at),
        updated_at: crate::state::date_time(revision.updated_at),
        cross_region_details: revision.cross_region_details.clone(),
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListGuardrails<Eng>
{
    type Input = ListGuardrailsInput;
    type Output = ListGuardrailsOutput;
    type Error = ListGuardrailsError;

    const NAME: &'static str = "ListGuardrails";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_GUARDRAILS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_GUARDRAILS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListGuardrailsErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("guardrail state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let store = state.lock();

        // In cursor order: with an identifier, every revision of that one guardrail;
        // without one, the DRAFT of every guardrail.
        let mut rows: Vec<(String, GuardrailSummary)> = Vec::new();
        match input.guardrail_identifier.as_deref() {
            Some(identifier) => {
                let Some(guardrail) = store.guardrail(identifier) else {
                    return Err(ListGuardrailsErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(format!(
                                "no guardrail {identifier} exists in this account"
                            )),
                        },
                    )
                    .into());
                };
                for revision in guardrail.revisions() {
                    rows.push((
                        cursor_of(&guardrail.id, &revision.version),
                        summary(guardrail, revision),
                    ));
                }
            }
            None => {
                let mut ids: Vec<&str> = store.guardrails.keys().map(String::as_str).collect();
                ids.sort_unstable();
                for id in ids {
                    let Some(guardrail) = store.guardrails.get(id) else {
                        continue;
                    };
                    rows.push((
                        cursor_of(&guardrail.id, &guardrail.draft.version),
                        summary(guardrail, &guardrail.draft),
                    ));
                }
            }
        }

        if let Some(token) = input.next_token.as_deref() {
            let filtered_id = input
                .guardrail_identifier
                .as_deref()
                .map(crate::state::guardrail_id_of);
            if !cursor_belongs(token, filtered_id) {
                return Err(ListGuardrailsErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            "nextToken is not a pagination token this service issued".to_string(),
                        ),
                    },
                )
                .into());
            }
            rows.retain(|(cursor, _)| cursor.as_str() >= token);
        }

        let page_size = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        // The row the next page starts on IS the next page's cursor, so the pages
        // partition the listing with neither a gap nor a repeat.
        let next_token = rows.get(page_size).map(|(cursor, _)| cursor.clone());
        rows.truncate(page_size);

        Ok(ListGuardrailsOutput {
            guardrails: rows.into_iter().map(|(_, summary)| summary).collect(),
            next_token,
        })
    }
}
