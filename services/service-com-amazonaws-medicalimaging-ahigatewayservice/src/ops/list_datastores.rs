// SPDX-License-Identifier: CC0-1.0

//! @op ListDatastores
//!
//! Enumerates the persisted data stores, newest last, optionally filtered by
//! `datastoreStatus`, one `maxResults`-sized page at a time. A reporting operation
//! (`@readonly`): it writes nothing of its own, and only materialises the status
//! changes the clock already owed.
//!
//! Only the data stores of the region the request was addressed to
//! ([`crate::state::request_region`]) are enumerated — a data store is a regional
//! resource — so a listing in a region nothing was created in answers an empty
//! `datastoreSummaries`.
//!
//! The continuation token is opaque on the wire — hex over a marked encoding of the
//! FULL sort key the next page starts at — so a string this service did not mint is
//! refused, and paging cannot skip or repeat a store even when the one the token named
//! has been deleted in the meantime. The marker names the REGION and the
//! `datastoreStatus` filter the position belongs to, because those two are what select
//! the records this listing walks: a token minted while paging one region's data stores,
//! or the stores one status selects, is not a position in another region's walk or in the
//! walk another status (or no status at all) selects, and is refused.
//!
//! A data store whose deletion has landed is enumerated in `DELETED` for as long as its
//! record survives ([`crate::state::DATASTORE_DELETED_RETENTION`]), and leaves the
//! listing when the clock reaps it — so every value of the status enum, that one
//! included, can both appear in a summary and select rows as the filter.
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
        let region = crate::state::request_region(ctx);
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
                                    sensitive: false,
                                },
                            ),
                        },
                    )
                    .into());
                }
            },
        };

        // The token is this service's own opaque handle, marked with the region AND the
        // status filter the position belongs to; only one this service minted for THAT
        // collection can be honoured, and anything else is a client error, not an empty
        // page. The refusal is the service's FIXED sentence: it does not vary with the
        // token, so it reveals nothing about how a real one is built.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(decode_page_token(&region, status_filter, token).ok_or_else(|| {
                ListDatastoresErrorModeled::ValidationException(ValidationException {
                    message: crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
                })
            })?),
        };

        // The page-size rule is the service's one ([`crate::state::list_page_size`]), so
        // every listing bounds a page the same way.
        let page_size = crate::state::list_page_size(input.max_results);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListDatastoresErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Total order over the whole collection — creation instant first, id to break a
        // tie — so a page boundary is stable no matter how the underlying map iterates.
        // Only the data stores of the region this request was addressed to are
        // enumerated: a data store exists in ONE region, so another region's stores are
        // not part of this collection at all. The collection includes a finished
        // deletion's record for as long as it survives
        // (`crate::state::DATASTORE_DELETED_RETENTION`), so `DELETED` is a status this
        // listing both reports and filters by, like every other value of the enum.
        let mut ordered: Vec<&crate::state::Datastore> = store
            .reported_datastores_in(&region)
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

        let next_token = ordered
            .get(end)
            .map(|datastore| encode_page_token(&region, status_filter, datastore));

        Ok(ListDatastoresOutput {
            datastore_summaries: Some(datastore_summaries),
            next_token,
        })
    }
}

/// The marker every token this listing mints carries: this operation's own prefix, the
/// region the position belongs to, and the `datastoreStatus` the walk it is a position in
/// was narrowed by ([`crate::state::page_token_filter_segment`]).
///
/// A hex string this service never issued, one minted for another collection, one minted
/// while paging another region's data stores and one minted under another status filter
/// are all refused rather than honoured as a resume point — a data store is a regional
/// resource, so the data stores of two regions are two collections, and the filter decides
/// which of a region's stores are in the walk at all, so a position in the stores one
/// status selects is not a position in the stores another selects nor in the unfiltered
/// whole. The codec itself is the service's one ([`crate::state::encode_page_token`]), so
/// every listing's token is built one way.
fn page_token_marker(region: &str, status_filter: Option<DatastoreStatus>) -> String {
    format!(
        "ahi-datastores-v1:{region}:{}:",
        crate::state::page_token_filter_segment(status_filter.map(DatastoreStatus::as_wire))
    )
}

/// The total order the listing walks: creation instant first, id to break a tie — so a
/// page boundary is stable no matter how the underlying map iterates.
fn sort_key(datastore: &crate::state::Datastore) -> (u64, u32, &str) {
    crate::state::page_sort_key(datastore.created_at, &datastore.datastore_id)
}

/// Encode the sort key the next page starts at as this listing's opaque `nextToken`.
fn encode_page_token(
    region: &str,
    status_filter: Option<DatastoreStatus>,
    first_of_next_page: &crate::state::Datastore,
) -> String {
    crate::state::encode_page_token(
        &page_token_marker(region, status_filter),
        first_of_next_page.created_at,
        &first_of_next_page.datastore_id,
    )
}

/// Decode a token produced by [`encode_page_token`] against `region` and `status_filter`,
/// or `None` when it was not one — a well-formed hex string without this listing's marker,
/// one carrying another region's, one minted under another status filter, or one with a
/// payload this service would never mint, is not one.
fn decode_page_token(
    region: &str,
    status_filter: Option<DatastoreStatus>,
    token: &str,
) -> Option<crate::state::PageCursor> {
    crate::state::decode_page_token(&page_token_marker(region, status_filter), token)
}
