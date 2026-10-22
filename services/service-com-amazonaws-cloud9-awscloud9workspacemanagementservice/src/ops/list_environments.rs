// SPDX-License-Identifier: CC0-1.0

//! @op ListEnvironments
//!
//! Reports the id of every environment in the store, in the one stable order
//! the `BTreeMap` key order gives, a page at a time. The `nextToken` names the
//! first key the next page starts at — hex-encoded behind a marker, so a token
//! this service did not issue is refused — and is absent on the last page.
//! Purely reporting: it writes nothing, and reads at most one page's worth of
//! keys.
//!
//! ---
//!
//! # Modeled contract for `ListEnvironments`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloud9-awscloud9workspacemanagementservice/smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json")
//! ```
//!
//! Gets a list of Cloud9 development environment identifiers. Cloud9 is no longer available
//! to new customers. Existing customers of Cloud9 can continue to use the service as
//! normal. Learn more" Cloud9 is no longer available to new customers. Existing customers
//! of Cloud9 can continue to use the service as normal. Learn more"
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 0..25) — The maximum number of environments to get
//!   identifiers for.
//! * `nextToken` (String) — During a previous call, if there are more than 25 items in the
//!   list, only the first 25 items are returned, along with a unique string called a next
//!   token. To get the next batch of items in the list, call this operation again, adding the
//!   next token to the call. To get all of the items in the list, keep calling this operation
//!   with each subsequent next token that is returned, until no more next tokens are
//!   returned.
//!
//! ## Output
//!
//! * `environmentIds` (EnvironmentIdList) — The list of environment identifiers.
//! * `nextToken` (String) — If there are more than 25 items in the list, only the first 25
//!   items are returned, along with a unique string called a next token. To get the next
//!   batch of items in the list, call this operation again, adding the next token to the
//!   call.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The target request is invalid.
//! * `ConflictException` (HTTP 400) — A conflict occurred.
//! * `ForbiddenException` (HTTP 400) — An access permissions issue occurred.
//! * `InternalServerErrorException` (HTTP 500) — An internal server error occurred.
//! * `LimitExceededException` (HTTP 400) — A service limit was exceeded.
//! * `NotFoundException` (HTTP 400) — The target resource cannot be found.
//! * `TooManyRequestsException` (HTTP 400) — Too many service requests were made over the
//!   given time period.

#[allow(unused_imports)]
use super::types::*;

use crate::state::MAX_ENVIRONMENTS_PER_PAGE;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListEnvironmentsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListEnvironmentsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloud9#ListEnvironments")]
#[derive(Debug)]
pub enum ListEnvironmentsErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_environments::ListEnvironmentsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListEnvironmentsError = crate::scaffolded::Scaffolded<ListEnvironmentsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListEnvironmentsErrorModeled> for ListEnvironmentsError {
    fn from(modeled: ListEnvironmentsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `BadRequestException` — "The target request is invalid." `code` keeps the
/// model's declared default and `className` is left unset: the model gives
/// neither a value nor a rule for them.
fn bad_request(message: String) -> ListEnvironmentsError {
    ListEnvironmentsErrorModeled::BadRequestException(BadRequestException {
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// The marker every `nextToken` this service issues carries, inside the
/// encoding. A hex string that does not decode to this prefix followed by an id
/// of the modeled `EnvironmentId` shape was not issued here, so it is refused
/// rather than silently resuming at some arbitrary key.
const NEXT_TOKEN_PREFIX: &str = "cloud9:environment:";

/// Wrap the key a page resumes from into an opaque `nextToken`.
///
/// Hex so the token is opaque: a caller cannot read an environment id out of it
/// and must hand it back unchanged, which is what `@paginated` means by an
/// opaque token.
fn encode_next_token(resume_from: &str) -> String {
    ::hex::encode(format!("{NEXT_TOKEN_PREFIX}{resume_from}").as_bytes())
}

/// Unwrap a `nextToken` back into the key the page resumes from, or `None` when
/// it is not a token this service issued.
fn decode_next_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let key = decoded.strip_prefix(NEXT_TOKEN_PREFIX)?;
    // A resume key is an `EnvironmentId`: `^[a-zA-Z0-9]{8,32}$`. The id itself
    // need not still exist — the environment it named may have been deleted
    // between pages, and resuming from a removed key is correct — but anything
    // that is not an id of that shape was never a token of ours.
    let well_formed =
        (8..=32).contains(&key.len()) && key.chars().all(|c| c.is_ascii_alphanumeric());
    well_formed.then(|| key.to_string())
}

/// The `ListEnvironments` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListEnvironments<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListEnvironments<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListEnvironments<Eng>
{
    type Input = ListEnvironmentsInput;
    type Output = ListEnvironmentsOutput;
    type Error = ListEnvironmentsError;

    const NAME: &'static str = "ListEnvironments";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ENVIRONMENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ENVIRONMENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListEnvironmentsErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the Cloud9 environment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        // `maxResults` is modeled `@range(min: 0, max: 25)`, so the boundary has
        // already refused anything outside it. An absent or zero page size means
        // "no caller-chosen limit", which is that member's own modeled maximum.
        let page_size = match input.max_results {
            Some(requested) if requested > 0 => requested as usize,
            _ => MAX_ENVIRONMENTS_PER_PAGE,
        };

        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(decode_next_token(token).ok_or_else(|| {
                bad_request("nextToken is not a token this service issued".to_string())
            })?),
        };

        let store = state.lock();
        // One stable order, from the `BTreeMap`'s keys, so a token names exactly
        // where the previous page ended whatever has been created since. At most
        // `page_size + 1` keys are read: the page itself plus the one key that
        // says whether another page follows — so the work done under the state
        // lock is bounded by the modeled page size, not by the store's size.
        let mut remaining = match &resume_from {
            Some(start) => store
                .environments
                .range::<String, _>(start..)
                .take(page_size + 1)
                .map(|(id, _)| id.clone())
                .collect::<Vec<String>>(),
            None => store
                .environments
                .keys()
                .take(page_size + 1)
                .cloned()
                .collect::<Vec<String>>(),
        };
        drop(store);

        let next_token = if remaining.len() > page_size {
            let resume_at = remaining[page_size].clone();
            remaining.truncate(page_size);
            Some(encode_next_token(&resume_at))
        } else {
            None
        };

        Ok(ListEnvironmentsOutput {
            next_token,
            environment_ids: Some(remaining),
        })
    }
}
