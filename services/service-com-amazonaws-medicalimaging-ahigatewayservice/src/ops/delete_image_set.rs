// SPDX-License-Identifier: CC0-1.0

//! @op DeleteImageSet
//!
//! Deletes one image set the named data store holds, and reports the state and workflow
//! status the deleted image set now carries.
//!
//! A delete is ASYNCHRONOUS, like every other change this service starts: the call puts
//! the image set into the `LOCKED` / `DELETING` pair and the CLOCK, not a later reader,
//! lands it on `DELETED` / `DELETED` once [`crate::state::IMAGE_SET_DELETION_DELAY`] has
//! passed — which is when the image set gets its `deletedAt` and loses the pixel data it
//! held. The `LOCKED` and `DELETED` state reaches EVERY version of the image set, because
//! the state is whether the image set can be read and written rather than a property of
//! one snapshot of it, while the workflow status — what last happened — is recorded on
//! the latest version. The record itself stays readable through `GetImageSet` as a
//! tombstone, because `deletedAt` ("the timestamp when the image set properties were
//! deleted") is a member that operation reports.
//!
//! `@idempotent`: a repeat delete reports the same pair as the first without moving the
//! instant the deletion falls due, and a delete of an image set the clock has already
//! retired reports its terminal pair rather than failing.
//!
//! ---
//!
//! # Modeled contract for `DeleteImageSet`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Delete an image set.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//!
//! ## Output
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//! * `imageSetState` (ImageSetState, one of: ACTIVE | DELETED | LOCKED) **required** — The
//!   image set state.
//! * `imageSetWorkflowStatus` (ImageSetWorkflowStatus, one of: COPIED | COPYING |
//!   COPYING_WITH_READ_ONLY_ACCESS | COPY_FAILED | CREATED | DELETED | DELETING | IMPORTED |
//!   IMPORTING | IMPORT_FAILED | UPDATED | UPDATE_FAILED | UPDATING |
//!   UPDATING_FOR_STUDY_CONSISTENCY) **required** — The image set workflow status.
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

use crate::state::{ImageSetState, ImageSetWorkflowStatus, PendingImageSetTransition};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteImageSetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteImageSetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#DeleteImageSet")]
#[derive(Debug)]
pub enum DeleteImageSetErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_image_set::DeleteImageSetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteImageSetError = crate::scaffolded::Scaffolded<DeleteImageSetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteImageSetErrorModeled> for DeleteImageSetError {
    fn from(modeled: DeleteImageSetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteImageSet` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteImageSet<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteImageSet<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteImageSet<Eng>
{
    type Input = DeleteImageSetInput;
    type Output = DeleteImageSetOutput;
    type Error = DeleteImageSetError;

    const NAME: &'static str = "DeleteImageSet";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IMAGE_SET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IMAGE_SET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteImageSetErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all.
        let datastore = store
            .datastore_in_mut(&region, &input.datastore_id)
            .ok_or_else(|| {
                DeleteImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    // Constant, and stated by resource KIND: the requested id is not
                    // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                    message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
                })
            })?;

        // A delete acts on the CONTENTS of the data store, so the store has to have
        // settled into `ACTIVE` first: one still `CREATING`, or already `DELETING`, is
        // refused with the modelled `ConflictException` (HTTP 409) before any image set
        // is looked up — the same precondition `CopyImageSet` applies
        // ([`crate::state::Datastore::is_active`]).
        if !datastore.is_active() {
            return Err(DeleteImageSetErrorModeled::ConflictException(ConflictException {
                message: crate::state::DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
            })
            .into());
        }

        let reported_datastore_id = datastore.datastore_id.clone();
        let image_set = datastore
            .image_sets
            .get_mut(&input.image_set_id)
            .ok_or_else(|| {
                DeleteImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                })
            })?;

        // An image set with OTHER work in flight — a copy, which holds its source
        // read-only and its destination locked until the clock completes it — cannot also
        // be deleted: the delete and that work would disagree about the image set, which
        // is what `ConflictException` reports. A deletion already in flight is NOT that
        // case: `@idempotent` says a repeat delete answers like the first one.
        if image_set.pending.is_some() && !image_set.deletion_in_flight() {
            return Err(DeleteImageSetErrorModeled::ConflictException(ConflictException {
                message: crate::state::IMAGE_SET_NOT_DELETABLE_MESSAGE.to_string(),
            })
            .into());
        }

        // Start the deletion only when nothing has started it yet: an image set the clock
        // has already retired keeps its terminal pair and its `deletedAt`, and one whose
        // deletion is still in flight keeps the instant that deletion falls due — so a
        // repeat call can neither resurrect an image set nor push its deletion further
        // out.
        if !image_set.is_deleted() && !image_set.deletion_in_flight() {
            // The lock reaches every version: a delete is about the whole image set, not
            // about one snapshot of it, so no version is left reporting `ACTIVE` while
            // the image set is on its way out. The workflow status — what last happened —
            // is recorded on the latest version.
            image_set.set_state_on_every_version(ImageSetState::Locked);
            image_set.latest_version.workflow_status = ImageSetWorkflowStatus::Deleting;
            image_set.latest_version.updated_at = now;
            image_set.pending = Some(PendingImageSetTransition {
                due_at: now + crate::state::IMAGE_SET_DELETION_DELAY,
                image_set_state: ImageSetState::Deleted,
                workflow_status: ImageSetWorkflowStatus::Deleted,
                // A delete delivers no pixel data: what the image set held goes with it
                // when the deletion lands.
                delivered_image_frames: None,
                // Nor any DICOM attributes: a delete takes content away rather than
                // bringing any.
                delivered_dicom_tags: None,
                retires_image_set: true,
            });
        }

        // Reported from the record this call just wrote, so the answer can never claim a
        // state the store does not hold.
        Ok(DeleteImageSetOutput {
            datastore_id: reported_datastore_id,
            image_set_id: image_set.image_set_id.clone(),
            image_set_state: image_set.latest_version.image_set_state.as_wire().to_string(),
            image_set_workflow_status: image_set
                .latest_version
                .workflow_status
                .as_wire()
                .to_string(),
        })
    }
}
