// SPDX-License-Identifier: CC0-1.0

//! @op ListDatastores
//!
//! Enumerates the persisted data stores, newest last, optionally filtered by
//! `datastoreStatus`, one `maxResults`-sized page at a time. A reporting operation
//! (`@readonly`): it writes nothing of its own, and only materialises the status
//! changes the clock already owed.
//!
//! The continuation token is opaque on the wire — hex over a marked encoding of the
//! FULL sort key the next page starts at — so a string this service did not mint is
//! refused, and paging cannot skip or repeat a store even when the one the token named
//! has been deleted in the meantime.
//!
//! ---
//!
//! # Modeled contract for `ListDatastores`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! List data stores.
//!
//! ## Input
//!
//! * `datastoreStatus` (DatastoreStatus, one of: ACTIVE | CREATE_FAILED | CREATING | DELETED
//!   | DELETING, @httpQuery(datastoreStatus)) — The data store status.
//! * `maxResults` (Integer, range 1..50, @httpQuery(maxResults)) — Valid Range: Minimum value
//!   of 1. Maximum value of 50.
//! * `nextToken` (NextToken, len 1..8192, @httpQuery(nextToken)) — The pagination token used
//!   to retrieve the list of data stores on the next page.
//!
//! ## Output
//!
//! * `datastoreSummaries` (DatastoreSummaries) — The list of summaries of data stores.
//! * `nextToken` (NextToken, len 1..8192) — The pagination token used to retrieve the list of
//!   data stores on the next page.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The user does not have sufficient access to perform
//!   this action.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during processing of
//!   the request.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

use crate::state::DatastoreStatus;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListDatastoresError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListDatastoresError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#ListDatastores")]
#[derive(Debug)]
pub enum ListDatastoresErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_datastores::ListDatastoresError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListDatastoresError = crate::scaffolded::Scaffolded<ListDatastoresErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListDatastoresErrorModeled> for ListDatastoresError {
    fn from(modeled: ListDatastoresErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListDatastores` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListDatastores<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListDatastores<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListDatastores<Eng>
{
    type Input = ListDatastoresInput;
    type Output = ListDatastoresOutput;
    type Error = ListDatastoresError;

    const NAME: &'static str = "ListDatastores";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_DATASTORES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_DATASTORES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        // The status filter must name a modelled `DatastoreStatus`. The boundary
        // validator already refused anything else, so an unparseable value here can
        // only come from a caller bypassing it.
        let status_filter = match input.datastore_status.as_deref() {
            None => None,
            Some(value) => match DatastoreStatus::from_wire(value) {
                Some(status) => Some(status),
                None => {
                    return Err(ListDatastoresErrorModeled::ValidationException(
                        ValidationException {
                            // Worded through the one refusal formatter this service
                            // owns, so a refusal raised here reads exactly like one
                            // the boundary validator stated.
                            message: crate::state::one_validation_error(
                                &smithy_server_runtime::Violation {
                                    path: "datastoreStatus".to_string(),
                                    kind: smithy_server_runtime::ViolationKind::EnumValue(
                                        DATASTORE_STATUS_VALUES
                                            .iter()
                                            .map(|value| (*value).to_string())
                                            .collect(),
                                    ),
                                    value: Some(value.to_string()),
                                },
                            ),
                        },
                    )
                    .into());
                }
            },
        };

        // The token is this service's own opaque handle; only one this service minted
        // can be honoured, and a forged one is a client error, not an empty page. The
        // refusal is the service's FIXED sentence: it does not vary with the token, so
        // it reveals nothing about how a real one is built.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(decode_page_token(token).ok_or_else(|| {
                ListDatastoresErrorModeled::ValidationException(ValidationException {
                    message: crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
                })
            })?),
        };

        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            // Clamped here, not merely at the boundary: one request must never be able
            // to materialise an unbounded page.
            .map(|requested| {
                requested.clamp(
                    crate::state::MIN_LIST_PAGE_SIZE,
                    crate::state::MAX_LIST_PAGE_SIZE,
                )
            })
            .unwrap_or(crate::state::MAX_LIST_PAGE_SIZE);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListDatastoresErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Total order over the whole collection — creation instant first, id to break a
        // tie — so a page boundary is stable no matter how the underlying map iterates.
        let mut ordered: Vec<&crate::state::Datastore> = store
            .datastores
            .values()
            .filter(|datastore| {
                status_filter.is_none_or(|status| datastore.datastore_status == status)
            })
            .collect();
        ordered.sort_by(|left, right| sort_key(left).cmp(&sort_key(right)));

        // Resume at the first store whose FULL sort key is at or after the token's.
        // Comparing the whole key — not the id alone — is what makes the walk correct
        // when the store the token named was deleted between pages: it can neither land
        // before the previous page (repeating a store) nor past one never seen
        // (skipping it).
        let start = match &resume_from {
            None => 0,
            Some(cursor) => ordered
                .iter()
                .position(|datastore| sort_key(datastore) >= cursor.as_key())
                .unwrap_or(ordered.len()),
        };

        let end = start.saturating_add(page_size).min(ordered.len());
        let datastore_summaries: Vec<DatastoreSummary> = ordered[start..end]
            .iter()
            .map(|datastore| DatastoreSummary {
                datastore_id: datastore.datastore_id.clone(),
                datastore_name: datastore.reported_name(),
                datastore_status: datastore.datastore_status.as_wire().to_string(),
                datastore_arn: Some(datastore.datastore_arn.clone()),
                created_at: Some(crate::state::to_date_time(datastore.created_at)),
                updated_at: Some(crate::state::to_date_time(datastore.updated_at)),
            })
            .collect();

        let next_token = ordered.get(end).map(|datastore| encode_page_token(datastore));

        Ok(ListDatastoresOutput {
            datastore_summaries: Some(datastore_summaries),
            next_token,
        })
    }
}

/// The marker every token this service mints carries, so a hex string the service
/// never issued is refused rather than honoured as a resume point.
const PAGE_TOKEN_MARKER: &str = "ahi-datastores-v1:";

/// The total order the listing walks: creation instant first, id to break a tie — so a
/// page boundary is stable no matter how the underlying map iterates.
fn sort_key(datastore: &crate::state::Datastore) -> (u64, u32, &str) {
    let since_epoch = datastore
        .created_at
        .duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (
        since_epoch.as_secs(),
        since_epoch.subsec_nanos(),
        datastore.datastore_id.as_str(),
    )
}

/// A decoded continuation token: the FULL sort key the next page starts at, so resuming
/// does not depend on the store that minted it still existing.
#[derive(Debug)]
struct PageCursor {
    created_at_secs: u64,
    created_at_nanos: u32,
    datastore_id: String,
}

impl PageCursor {
    fn as_key(&self) -> (u64, u32, &str) {
        (
            self.created_at_secs,
            self.created_at_nanos,
            self.datastore_id.as_str(),
        )
    }
}

/// Encode the sort key the next page starts at as an opaque `nextToken`.
///
/// Hex keeps the token inside `NextToken`'s `^\p{ASCII}{0,8192}$` pattern and makes it
/// plainly not a data store id, so no caller can mistake it for one.
fn encode_page_token(first_of_next_page: &crate::state::Datastore) -> String {
    let (secs, nanos, id) = sort_key(first_of_next_page);
    ::hex::encode(format!("{PAGE_TOKEN_MARKER}{secs}.{nanos}:{id}").as_bytes())
}

/// Decode a token produced by [`encode_page_token`], or `None` when it was not — a
/// well-formed hex string without the marker, or with a payload this service would
/// never mint, is not one.
fn decode_page_token(token: &str) -> Option<PageCursor> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let payload = decoded.strip_prefix(PAGE_TOKEN_MARKER)?;
    // `DatastoreId` is `^[0-9a-z]{32}$`, so neither `:` nor `.` can appear in the id and
    // splitting on the first of each is unambiguous.
    let (instant, datastore_id) = payload.split_once(':')?;
    let (secs, nanos) = instant.split_once('.')?;
    Some(PageCursor {
        created_at_secs: secs.parse().ok()?,
        created_at_nanos: nanos.parse().ok()?,
        datastore_id: datastore_id.to_string(),
    })
}
