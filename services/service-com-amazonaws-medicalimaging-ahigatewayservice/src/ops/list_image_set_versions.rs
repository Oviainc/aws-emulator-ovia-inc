// SPDX-License-Identifier: CC0-1.0

//! @op ListImageSetVersions
//!
//! Reports every VERSION of one image set from persisted state, oldest version first,
//! one `maxResults`-sized page at a time, with an opaque token that resumes the walk
//! where the previous page stopped. Each version reports exactly what that version's own
//! reader (`GetImageSet` with `version`) reports for it, so the two can never disagree.
//!
//! A REPORTING operation (`@readonly` in the model), so it writes nothing of its own. It
//! does call
//! [`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`]
//! first, like every handler, which materialises only a transition the CLOCK already
//! owed at that instant — the release of a copy's lock, say.
//!
//! An image set's versions are numbered from
//! [`crate::state::FIRST_IMAGE_SET_VERSION_ID`] and never renumbered or reused, so that
//! number is the listing's whole sort key and the position its `nextToken` carries. It
//! is compared as a NUMBER, which is what keeps version 10 after version 9.
//!
//! ---
//!
//! # Modeled contract for `ListImageSetVersions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! List image set versions.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//! * `maxResults` (Integer) — The max results count.
//! * `nextToken` (NextToken, len 1..8192, pattern ^\p{ASCII}{0,8192}$) — The pagination token
//!   used to request the list of image set versions on the next page.
//!
//! ## Output
//!
//! * `imageSetPropertiesList` (ImageSetPropertiesList) **required** — Lists all properties
//!   associated with an image set.
//! * `nextToken` (NextToken, len 1..8192, pattern ^\p{ASCII}{0,8192}$) — The pagination token
//!   used to retrieve the list of image set versions on the next page.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The user does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — Updating or deleting a resource can cause an
//!   inconsistent state.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during processing of
//!   the request.
//! * `ResourceNotFoundException` (HTTP 404) — The request references a resource which does
//!   not exist.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListImageSetVersionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListImageSetVersionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#ListImageSetVersions")]
#[derive(Debug)]
pub enum ListImageSetVersionsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_image_set_versions::ListImageSetVersionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListImageSetVersionsError = crate::scaffolded::Scaffolded<ListImageSetVersionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListImageSetVersionsErrorModeled> for ListImageSetVersionsError {
    fn from(modeled: ListImageSetVersionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListImageSetVersions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListImageSetVersions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListImageSetVersions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListImageSetVersions<Eng>
{
    type Input = ListImageSetVersionsInput;
    type Output = ListImageSetVersionsOutput;
    type Error = ListImageSetVersionsError;

    const NAME: &'static str = "ListImageSetVersions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_IMAGE_SET_VERSIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_IMAGE_SET_VERSIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        // The token is this service's own opaque handle, marked with the image set the
        // position belongs to; only one this service minted against THAT image set can be
        // honoured, and anything else is a client error rather than an empty page. The
        // refusal is the service's FIXED sentence: it does not vary with the token, so it
        // reveals nothing about how a real one is built.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                decode_page_token(&input.datastore_id, &input.image_set_id, token).ok_or_else(
                    || {
                        ListImageSetVersionsErrorModeled::ValidationException(ValidationException {
                            message: crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
                        })
                    },
                )?,
            ),
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
            ListImageSetVersionsErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all.
        let datastore = store.datastore_in(&region, &input.datastore_id).ok_or_else(|| {
            ListImageSetVersionsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        // A collection the service does not hold is the modelled 404, never an empty
        // page: an absent image set has no versions to report.
        let image_set = datastore.image_sets.get(&input.image_set_id).ok_or_else(|| {
            ListImageSetVersionsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;

        // Total order over the whole collection: the external version identifier,
        // ascending — oldest version first. Sorted NUMERICALLY, and here rather than
        // trusting the order the records are held in, so a page boundary is stable.
        let mut ordered: Vec<&crate::state::ImageSetVersion> = image_set.versions().collect();
        ordered.sort_by_key(|version| version.version_id);

        // Resume at the first version numbered at or after the token's position.
        // Comparing the position — not an index into a page already served — is what
        // keeps the walk correct when the collection grew between pages: a version minted
        // after the token can neither displace one never seen nor repeat one served.
        let start = match resume_from {
            None => 0,
            Some(resume_at) => ordered
                .iter()
                .position(|version| version.version_id >= resume_at)
                .unwrap_or(ordered.len()),
        };

        let end = start.saturating_add(page_size).min(ordered.len());
        let image_set_properties_list: Vec<ImageSetProperties> = ordered[start..end]
            .iter()
            .map(|version| ImageSetProperties {
                image_set_id: image_set.image_set_id.clone(),
                version_id: version.wire_version_id(),
                image_set_state: version.image_set_state.as_wire().to_string(),
                image_set_workflow_status: Some(version.workflow_status.as_wire().to_string()),
                // `createdAt` is the image SET's creation, which is why every version
                // reports the instant the image set itself appeared; `updatedAt` is the
                // reported VERSION's own — the same split `GetImageSet` reports.
                created_at: Some(crate::state::to_date_time(image_set.created_at)),
                updated_at: Some(crate::state::to_date_time(version.updated_at)),
                deleted_at: image_set.deleted_at.map(crate::state::to_date_time),
                message: version.message.clone(),
                // "if `forced` exists, the `forced` flag was used when creating the
                // image set" — so it is present only on a version that was created with
                // it.
                overrides: version.forced.then_some(Overrides { forced: Some(true) }),
                is_primary: Some(image_set.is_primary),
            })
            .collect();

        // A token only when a further page exists, so a caller's walk terminates on the
        // page that exhausts the collection.
        let next_token = ordered.get(end).map(|version| {
            encode_page_token(&input.datastore_id, &input.image_set_id, version.version_id)
        });

        Ok(ListImageSetVersionsOutput {
            image_set_properties_list,
            next_token,
        })
    }
}

/// The marker every token this listing mints carries: this operation's own prefix plus
/// the data store and image set the position belongs to.
///
/// A hex string this service never issued, one minted for another collection, and one
/// minted against another image set are all refused rather than honoured as a resume
/// point — the versions of two image sets are two collections, so a position in one is
/// not a position in the other. The codec itself is the service's own
/// ([`crate::state::encode_ordinal_page_token`]), so every listing's token is built one
/// way.
fn page_token_marker(datastore_id: &str, image_set_id: &str) -> String {
    format!("ahi-image-set-versions-v1:{datastore_id}:{image_set_id}:")
}

/// Encode the version identifier the next page starts at as this listing's opaque
/// `nextToken`.
fn encode_page_token(datastore_id: &str, image_set_id: &str, first_of_next_page: u32) -> String {
    crate::state::encode_ordinal_page_token(
        &page_token_marker(datastore_id, image_set_id),
        first_of_next_page,
    )
}

/// Decode a token produced by [`encode_page_token`] against this image set, or `None`
/// when it was not one.
fn decode_page_token(datastore_id: &str, image_set_id: &str, token: &str) -> Option<u32> {
    crate::state::decode_ordinal_page_token(&page_token_marker(datastore_id, image_set_id), token)
}
