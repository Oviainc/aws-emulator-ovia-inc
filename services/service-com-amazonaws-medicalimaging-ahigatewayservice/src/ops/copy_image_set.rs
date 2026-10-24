// SPDX-License-Identifier: CC0-1.0

//! @op CopyImageSet
//!
//! Copies one persisted image set in a data store, either into a NEW image set this
//! call mints or as a new VERSION of an existing destination image set, and answers
//! with the properties of both image sets as the copy leaves them.
//!
//! What the operation does, member by member:
//!
//! * `copyImageSetInformation.sourceImageSet.latestVersionId` is an optimistic-
//!   concurrency check: a value that is not the source's current latest version means
//!   the caller's view is stale, which is the modelled `ConflictException` (409). The
//!   same check runs on `destinationImageSet.latestVersionId`.
//! * with NO `destinationImageSet`, the copy mints an image set at version
//!   [`crate::state::FIRST_IMAGE_SET_VERSION_ID`]; with one, it pushes a new version
//!   onto that image set, numbered one past the version the caller named.
//! * `promoteToPrimary` makes the destination image set primary (`isPrimary`), which is
//!   what the member documents: the destination is promoted into the primary DICOM
//!   hierarchy.
//! * `force` is recorded as the destination version's `Overrides.forced`, which is
//!   exactly what the model says that member reports — "if `forced` exists, the
//!   `forced` flag was used when creating the image set" — and is what `GetImageSet`
//!   reads back.
//! * `sourceImageSet.DICOMCopies.copiableAttributes` is documented as "the JSON string
//!   used to specify a subset of SOP Instances to copy", so a value that is not a JSON
//!   document fails the constraint the service sets and is refused with
//!   `ValidationException` (400), worded like every other refusal of this service and
//!   WITHOUT echoing the value, which the model marks `@sensitive`.
//!
//! Both image sets come back `LOCKED` — the source `COPYING_WITH_READ_ONLY_ACCESS`,
//! the destination `COPYING` — and the CLOCK, not a later reader, releases them after
//! [`crate::state::IMAGE_SET_COPY_DELAY`]: the destination then reports `COPIED` and
//! the source the workflow status it carried before the copy, since a copy changes
//! nothing about the source. Nothing here sleeps.
//!
//! A copy also carries the source's PIXEL DATA: the image frames the source holds when
//! the copy is admitted are delivered to the destination by the same clock-owed
//! transition that releases it, so `GetImageFrame` serves them from the destination once
//! the copy has completed and finds nothing there before that.
//!
//! ---
//!
//! # Modeled contract for `CopyImageSet`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Copy an image set.
//!
//! ## Input
//!
//! * `copyImageSetInformation` (CopyImageSetInformation) **required** — Copy image set
//!   information.
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `force` (Boolean) — Providing this parameter will force completion of the CopyImageSet
//!   operation, even if there are inconsistent Patient, Study, and/or Series level metadata
//!   elements between the sourceImageSet and destinationImageSet.
//! * `promoteToPrimary` (Boolean) — Providing this parameter will configure the CopyImageSet
//!   operation to promote the given image set to the primary DICOM hierarchy. If successful,
//!   a new primary image set ID will be returned as the destination image set.
//! * `sourceImageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The source image
//!   set identifier.
//!
//! ## Output
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `destinationImageSetProperties` (CopyDestinationImageSetProperties) **required** — The
//!   properties of the destination image set.
//! * `sourceImageSetProperties` (CopySourceImageSetProperties) **required** — The properties
//!   of the source image set.
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
//! * `ServiceQuotaExceededException` (HTTP 402) — The request caused a service quota to be
//!   exceeded.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{ImageSet, ImageSetState, ImageSetWorkflowStatus, PendingImageSetTransition};

/// The validation path of `copiableAttributes`, as the boundary validator spells it —
/// the member whose JSON this handler checks itself.
const COPIABLE_ATTRIBUTES_PATH: &str =
    "copyImageSetInformation.sourceImageSet.DICOMCopies.copiableAttributes";

/// The validation path of the destination image set's identifier.
const DESTINATION_IMAGE_SET_ID_PATH: &str = "copyImageSetInformation.destinationImageSet.imageSetId";

/// What this call does to the destination: mint a new image set, or push a version onto
/// one the data store already holds. Decided while the request is still being checked,
/// applied once every refusal has been ruled out — so no request is half-applied.
enum DestinationPlan {
    /// Mint an image set with this freshly minted identifier.
    Mint(String),
    /// Push a new version onto the image set with this identifier.
    Version(String),
}

/// The source image set's properties, read from the record this call just wrote, so the
/// response can never claim a state the store does not hold.
fn source_properties(image_set: &ImageSet) -> CopySourceImageSetProperties {
    CopySourceImageSetProperties {
        image_set_id: image_set.image_set_id.clone(),
        latest_version_id: image_set.latest_version_id(),
        image_set_state: Some(image_set.latest_version.image_set_state.as_wire().to_string()),
        image_set_workflow_status: Some(
            image_set.latest_version.workflow_status.as_wire().to_string(),
        ),
        created_at: Some(crate::state::to_date_time(image_set.created_at)),
        updated_at: Some(crate::state::to_date_time(image_set.latest_version.updated_at)),
        image_set_arn: Some(image_set.image_set_arn.clone()),
    }
}

/// The destination image set's properties, read from state for the same reason as
/// [`source_properties`]. The two shapes carry the same members under different names,
/// which is why this is two functions and not one.
fn destination_properties(image_set: &ImageSet) -> CopyDestinationImageSetProperties {
    CopyDestinationImageSetProperties {
        image_set_id: image_set.image_set_id.clone(),
        latest_version_id: image_set.latest_version_id(),
        image_set_state: Some(image_set.latest_version.image_set_state.as_wire().to_string()),
        image_set_workflow_status: Some(
            image_set.latest_version.workflow_status.as_wire().to_string(),
        ),
        created_at: Some(crate::state::to_date_time(image_set.created_at)),
        updated_at: Some(crate::state::to_date_time(image_set.latest_version.updated_at)),
        image_set_arn: Some(image_set.image_set_arn.clone()),
    }
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CopyImageSetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CopyImageSetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#CopyImageSet")]
#[derive(Debug)]
pub enum CopyImageSetErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::copy_image_set::CopyImageSetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CopyImageSetError = crate::scaffolded::Scaffolded<CopyImageSetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CopyImageSetErrorModeled> for CopyImageSetError {
    fn from(modeled: CopyImageSetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CopyImageSet` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CopyImageSet<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CopyImageSet<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CopyImageSet<Eng>
{
    type Input = CopyImageSetInput;
    type Output = CopyImageSetOutput;
    type Error = CopyImageSetError;

    const NAME: &'static str = "CopyImageSet";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::COPY_IMAGE_SET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::COPY_IMAGE_SET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();
        let information = &input.copy_image_set_information;

        // `copiableAttributes` is documented as "the JSON string used to specify a
        // subset of SOP Instances to copy", and the model constrains only its length —
        // so whether it IS a JSON document is checked here, before any state is read.
        // The value is never echoed: the model marks the member `@sensitive`.
        if let Some(copies) = &information.source_image_set.dicom_copies {
            if ::serde_json::from_str::<::serde_json::Value>(&copies.copiable_attributes).is_err() {
                return Err(CopyImageSetErrorModeled::ValidationException(ValidationException {
                    message: crate::state::handler_requirement_error(
                        COPIABLE_ATTRIBUTES_PATH,
                        None,
                        "Member must be a JSON document",
                    ),
                })
                .into());
            }
        }

        // A copy whose destination IS the source has no coherent answer: the one image
        // set would have to be the read-only source and the locked destination of the
        // same copy. Refused as the input-constraint failure it is, naming the member.
        if let Some(destination) = &information.destination_image_set {
            if destination.image_set_id == input.source_image_set_id {
                return Err(CopyImageSetErrorModeled::ValidationException(ValidationException {
                    message: crate::state::handler_requirement_error(
                        DESTINATION_IMAGE_SET_ID_PATH,
                        Some(&destination.image_set_id),
                        "Member must not name the source image set",
                    ),
                })
                .into());
            }
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CopyImageSetErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        let datastore = store.datastores.get(&input.datastore_id).ok_or_else(|| {
            CopyImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        let datastore_arn = datastore.datastore_arn.clone();

        let source = datastore
            .image_sets
            .get(&input.source_image_set_id)
            .ok_or_else(|| {
                CopyImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                })
            })?;
        if source.latest_version_id() != information.source_image_set.latest_version_id {
            return Err(CopyImageSetErrorModeled::ConflictException(ConflictException {
                message: crate::state::IMAGE_SET_VERSION_CONFLICT_MESSAGE.to_string(),
            })
            .into());
        }
        if source.latest_version.image_set_state != ImageSetState::Active {
            return Err(CopyImageSetErrorModeled::ConflictException(ConflictException {
                message: crate::state::IMAGE_SET_NOT_ACTIVE_MESSAGE.to_string(),
            })
            .into());
        }
        // A copy leaves the source's content alone, so the status it reports once the
        // copy has completed is the one it carried going in.
        let source_workflow_status_before_copy = source.latest_version.workflow_status;
        // The pixel data this copy carries to its destination: read from the source as
        // it stands when the copy is admitted, delivered when the copy completes.
        let copied_image_frames = source.image_frames.clone();
        let held_image_sets = datastore.image_sets.len();

        // Every refusal the destination can raise, before anything is written.
        let plan = match &information.destination_image_set {
            Some(destination) => {
                let held = datastore
                    .image_sets
                    .get(&destination.image_set_id)
                    .ok_or_else(|| {
                        CopyImageSetErrorModeled::ResourceNotFoundException(
                            ResourceNotFoundException {
                                message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                            },
                        )
                    })?;
                if held.latest_version_id() != destination.latest_version_id {
                    return Err(CopyImageSetErrorModeled::ConflictException(ConflictException {
                        message: crate::state::IMAGE_SET_VERSION_CONFLICT_MESSAGE.to_string(),
                    })
                    .into());
                }
                if held.latest_version.image_set_state != ImageSetState::Active {
                    return Err(CopyImageSetErrorModeled::ConflictException(ConflictException {
                        message: crate::state::IMAGE_SET_NOT_ACTIVE_MESSAGE.to_string(),
                    })
                    .into());
                }
                // Bound the versions one image set can accumulate: the version this
                // copy supersedes is kept, so the list grows by one per copy
                // (`crate::state::MAX_IMAGE_SET_VERSIONS`).
                if held.versions().count() >= crate::state::MAX_IMAGE_SET_VERSIONS {
                    return Err(CopyImageSetErrorModeled::ServiceQuotaExceededException(
                        ServiceQuotaExceededException {
                            message: format!(
                                "this emulator holds at most {} versions per image set",
                                crate::state::MAX_IMAGE_SET_VERSIONS
                            ),
                        },
                    )
                    .into());
                }
                DestinationPlan::Version(destination.image_set_id.clone())
            }
            None => {
                // The same per-data-store memory bound the import jobs carry
                // (`crate::state::MAX_IMAGE_SETS_PER_DATASTORE`), refused with this
                // operation's modelled quota error.
                if held_image_sets >= crate::state::MAX_IMAGE_SETS_PER_DATASTORE {
                    return Err(CopyImageSetErrorModeled::ServiceQuotaExceededException(
                        ServiceQuotaExceededException {
                            message: format!(
                                "this emulator holds at most {} image sets per data store",
                                crate::state::MAX_IMAGE_SETS_PER_DATASTORE
                            ),
                        },
                    )
                    .into());
                }
                DestinationPlan::Mint(store.mint_id())
            }
        };

        // Past here the request is accepted, so both image sets are written before
        // anything is reported: a 200 naming a destination that was never stored would
        // dress absence as success.
        let forced = input.force.unwrap_or(false);
        let promote_to_primary = input.promote_to_primary.unwrap_or(false);
        let copy_due = PendingImageSetTransition {
            due_at: now + crate::state::IMAGE_SET_COPY_DELAY,
            image_set_state: ImageSetState::Active,
            workflow_status: ImageSetWorkflowStatus::Copied,
            // The destination holds the source's pixel data once the copy has
            // COMPLETED, never before: a frame is readable exactly when the copy that
            // delivers it lands.
            delivered_image_frames: Some(copied_image_frames),
        };

        let datastore = store.datastores.get_mut(&input.datastore_id).ok_or_else(|| {
            CopyImageSetErrorModeled::InternalServerException(InternalServerException {
                message: "the data store could not be reached while copying the image set"
                    .to_string(),
            })
        })?;
        let destination_image_set_id = match plan {
            DestinationPlan::Mint(image_set_id) => {
                let mut image_set = ImageSet::new(
                    image_set_id.clone(),
                    crate::state::image_set_arn(&datastore_arn, &image_set_id),
                    ImageSetState::Locked,
                    ImageSetWorkflowStatus::Copying,
                    promote_to_primary,
                    forced,
                    now,
                );
                image_set.pending = Some(copy_due.clone());
                datastore.image_sets.insert(image_set_id.clone(), image_set);
                image_set_id
            }
            DestinationPlan::Version(image_set_id) => {
                let image_set = datastore.image_sets.get_mut(&image_set_id).ok_or_else(|| {
                    CopyImageSetErrorModeled::InternalServerException(InternalServerException {
                        message:
                            "the destination image set could not be reached while copying into it"
                                .to_string(),
                    })
                })?;
                image_set.push_version(
                    ImageSetState::Locked,
                    ImageSetWorkflowStatus::Copying,
                    forced,
                    now,
                );
                image_set.pending = Some(copy_due.clone());
                // `promoteToPrimary` promotes the DESTINATION into the primary DICOM
                // hierarchy; it never demotes an image set that is already primary.
                if promote_to_primary {
                    image_set.is_primary = true;
                }
                image_set_id
            }
        };

        // The source is readable but not writable while the copy runs, which is what
        // `COPYING_WITH_READ_ONLY_ACCESS` reports. The destination is never the source
        // (refused above), so this reaches a different record.
        let source = datastore
            .image_sets
            .get_mut(&input.source_image_set_id)
            .ok_or_else(|| {
                CopyImageSetErrorModeled::InternalServerException(InternalServerException {
                    message: "the source image set could not be reached while copying it"
                        .to_string(),
                })
            })?;
        source.latest_version.image_set_state = ImageSetState::Locked;
        source.latest_version.workflow_status = ImageSetWorkflowStatus::CopyingWithReadOnlyAccess;
        source.latest_version.updated_at = now;
        source.pending = Some(PendingImageSetTransition {
            workflow_status: source_workflow_status_before_copy,
            // A copy READS its source and writes its destination, so the release of the
            // source's read-only lock delivers no pixel data.
            delivered_image_frames: None,
            ..copy_due
        });

        // Report from the records just written, not from local copies of them.
        let datastore = store.datastores.get(&input.datastore_id).ok_or_else(|| {
            CopyImageSetErrorModeled::InternalServerException(InternalServerException {
                message: "the data store could not be reached while reporting the copy".to_string(),
            })
        })?;
        let source = datastore
            .image_sets
            .get(&input.source_image_set_id)
            .ok_or_else(|| {
                CopyImageSetErrorModeled::InternalServerException(InternalServerException {
                    message: "the source image set could not be reached while reporting the copy"
                        .to_string(),
                })
            })?;
        let destination = datastore
            .image_sets
            .get(&destination_image_set_id)
            .ok_or_else(|| {
                CopyImageSetErrorModeled::InternalServerException(InternalServerException {
                    message:
                        "the destination image set could not be reached while reporting the copy"
                            .to_string(),
                })
            })?;

        Ok(CopyImageSetOutput {
            datastore_id: input.datastore_id.clone(),
            source_image_set_properties: source_properties(source),
            destination_image_set_properties: destination_properties(destination),
        })
    }
}
