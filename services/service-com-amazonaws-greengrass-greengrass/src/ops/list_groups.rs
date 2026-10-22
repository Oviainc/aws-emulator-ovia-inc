// SPDX-License-Identifier: CC0-1.0

//! @op ListGroups
//!
//! Enumerates every persisted group as a `GroupInformation` entry, oldest first,
//! honouring `MaxResults` as the page size (clamped to a server ceiling, with a
//! smaller server default when the caller names none) and returning an opaque
//! `NextToken` that resumes exactly where the page ended (absent on the last page). A
//! reporting operation: it writes nothing, and a deleted group is simply not there to
//! list.
//!
//! ## Not region-scoped in this slice
//!
//! This service keeps ONE store, so every group it holds is listed whatever region the
//! request was signed for, each entry carrying the region-scoped ARN minted when the
//! group was created. Real Greengrass is regional and would list only the groups of the
//! region addressed; isolating the store per region belongs with the region-scoping of
//! `GetGroup`, `CreateGroupVersion`, `CreateDeployment` and `DeleteGroup` and is left to
//! its own change rather than half-applied here.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `ListGroups`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Retrieves a list of groups.
//!
//! ## Input
//!
//! * `MaxResults` (__string) — The maximum number of results to be returned per request.
//! * `NextToken` (__string) — The token for the next set of results, or ''null'' if there are
//!   no additional results.
//!
//! ## Output
//!
//! * `Groups` (__listOfGroupInformation) — Information about a group.
//! * `NextToken` (__string) — The token for the next set of results, or ''null'' if there are
//!   no additional results.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListGroupsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The model declares NO error for `ListGroups`, so the only widening here is the
// service's own modelled server error (HTTP 500) for the unreachable-store path. A
// malformed `MaxResults`/`NextToken` is refused through the wrapper's
// `ValidationException` (HTTP 400) rather than by inventing a client error this
// operation does not model.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#ListGroups",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum ListGroupsErrorModeled {
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_groups::ListGroupsError`). The wrapper is kept because it is
/// the only place this operation can refuse a bad request: its own model declares
/// no client-input error.
pub type ListGroupsError = crate::scaffolded::Scaffolded<ListGroupsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListGroupsErrorModeled> for ListGroupsError {
    fn from(modeled: ListGroupsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListGroups` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListGroups<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListGroups<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The page size `ListGroups` applies when the caller names none. A server-side
/// default is what bounds the response: without one a zero-byte request with no
/// `MaxResults` would render the whole store in a single body. Deliberately below
/// [`MAX_PAGE_SIZE`], so defaulting and clamping are distinguishable.
const DEFAULT_PAGE_SIZE: usize = 50;
/// The ceiling a caller-supplied `MaxResults` is clamped to: a caller may ask for
/// less than [`DEFAULT_PAGE_SIZE`] or more, but never for more than this.
const MAX_PAGE_SIZE: usize = 100;

/// Render a page cursor as the opaque `NextToken` a client hands back. The value a
/// client sees is meaningless to it; only this module reads it.
fn encode_cursor(seq: u64) -> String {
    format!("{seq:016x}")
}

/// Read a cursor a previous page handed out. `None` when the token is not one
/// [`encode_cursor`] could have minted: the check is exactly its output alphabet —
/// sixteen lowercase hex digits — so a sign-prefixed or upper-cased string this
/// service never issued is refused rather than honoured as a sequence.
fn decode_cursor(token: &str) -> Option<u64> {
    let minted_here = token.len() == 16
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    minted_here
        .then(|| u64::from_str_radix(token, 16).ok())
        .flatten()
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListGroups<Eng>
{
    type Input = ListGroupsInput;
    type Output = ListGroupsOutput;
    type Error = ListGroupsError;

    const NAME: &'static str = "ListGroups";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_GROUPS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_GROUPS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListGroupsErrorModeled::InternalServerErrorException(crate::state::store_unreachable())
        })?;

        // `MaxResults` is modelled as a STRING query parameter, so the page size is
        // not validated at the boundary: a value that is not a positive integer is
        // refused here instead of being silently ignored. An absent `MaxResults`
        // takes the server's own default page size, and a caller-supplied one is
        // clamped to the server maximum, so no single request renders the store.
        let limit = match input.max_results.as_deref().map(str::trim) {
            None | Some("") => DEFAULT_PAGE_SIZE,
            Some(raw) => match raw.parse::<usize>() {
                Ok(parsed) if parsed > 0 => parsed.min(MAX_PAGE_SIZE),
                _ => {
                    return Err(crate::scaffolded::Scaffolded::validation(format!(
                        "MaxResults must be a positive integer, got '{raw}'"
                    )))
                }
            },
        };

        let cursor = match input.next_token.as_deref().filter(|t| !t.is_empty()) {
            None => 0,
            Some(token) => match decode_cursor(token) {
                Some(seq) => seq,
                None => {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        "NextToken is not a token this service issued".to_string(),
                    ))
                }
            },
        };

        let store = state.lock();
        // One group past the page is read as well: its sequence is what the next
        // token resumes at, and its presence is what says there is a next page.
        let mut window: Vec<&crate::state::Group> = store
            .groups_in_order()
            .into_iter()
            .skip_while(|group| group.seq < cursor)
            .take(limit.saturating_add(1))
            .collect();
        let next_token = (window.len() > limit)
            .then(|| window.pop().map(|group| encode_cursor(group.seq)))
            .flatten();

        Ok(ListGroupsOutput {
            groups: Some(window.iter().map(|group| group.information()).collect()),
            next_token,
        })
    }
}
