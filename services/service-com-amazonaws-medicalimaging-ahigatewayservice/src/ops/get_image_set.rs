// SPDX-License-Identifier: CC0-1.0

//! @op GetImageSet
//!
//! Reports one image set's properties from persisted state: the version the caller
//! named through `version`, or the latest version when it named none.
//!
//! A REPORTING operation (`@readonly` in the model), so it writes nothing of its own.
//! It does call [`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`]
//! first, like every handler, which materialises only a transition the CLOCK already
//! owed at that instant — the release of a copy's lock, say. A read never moves a
//! status that was not already due, and never records that the image set was read:
//! `lastAccessedAt` is left absent rather than written by the reader.
//!
//! ---
//!
//! # Modeled contract for `GetImageSet`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Get image set properties.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//! * `versionId` (ImageSetExternalVersionId, pattern ^\d+$) — The image set version
//!   identifier.
//!
//! ## Output
//!
//! * `createdAt` (Date) — The timestamp when image set properties were created.
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `deletedAt` (Date) — The timestamp when the image set properties were deleted.
//! * `imageSetArn` (Arn, pattern
//!   ^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z
//!   …) — The Amazon Resource Name (ARN) assigned to the image set.
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//! * `imageSetState` (ImageSetState, one of: ACTIVE | DELETED | LOCKED) **required** — The
//!   image set state.
//! * `imageSetWorkflowStatus` (ImageSetWorkflowStatus, one of: COPIED | COPYING |
//!   COPYING_WITH_READ_ONLY_ACCESS | COPY_FAILED | CREATED | DELETED | DELETING | IMPORTED |
//!   IMPORTING | IMPORT_FAILED | UPDATED | UPDATE_FAILED | UPDATING |
//!   UPDATING_FOR_STUDY_CONSISTENCY) — The image set workflow status.
//! * `isPrimary` (Boolean) — The flag to determine whether the image set is primary or not.
//! * `lastAccessedAt` (Date) — When the image set was last accessed.
//! * `message` (Message, len 1..2048, pattern ^[\w -:`]+$) — The error message thrown if an
//!   image set action fails.
//! * `overrides` (Overrides) — This object contains the details of any overrides used while
//!   creating a specific image set version. If an image set was copied or updated using the
//!   force flag, this object will contain the forced flag.
//! * `storageTier` (StorageTier, one of: ARCHIVE_INSTANT_ACCESS | FREQUENT_ACCESS) — The
//!   storage tier of the image set.
//! * `updatedAt` (Date) — The timestamp when image set properties were updated.
//! * `versionId` (ImageSetExternalVersionId, pattern ^\d+$) **required** — The image set
//!   version identifier.
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
// `GetImageSetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetImageSetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#GetImageSet")]
#[derive(Debug)]
pub enum GetImageSetErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_image_set::GetImageSetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetImageSetError = crate::scaffolded::Scaffolded<GetImageSetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetImageSetErrorModeled> for GetImageSetError {
    fn from(modeled: GetImageSetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetImageSet` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetImageSet<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetImageSet<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetImageSet<Eng>
{
    type Input = GetImageSetInput;
    type Output = GetImageSetOutput;
    type Error = GetImageSetError;

    const NAME: &'static str = "GetImageSet";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IMAGE_SET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IMAGE_SET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetImageSetErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        let datastore = store.datastores.get(&input.datastore_id).ok_or_else(|| {
            GetImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        let image_set = datastore.image_sets.get(&input.image_set_id).ok_or_else(|| {
            GetImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;

        // `version` names ONE version of the image set; without it the latest is
        // reported. A version this image set does not hold is absent, not invalid.
        let version = match &input.version_id {
            Some(version_id) => image_set.version(version_id).ok_or_else(|| {
                GetImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_VERSION_NOT_FOUND_MESSAGE.to_string(),
                })
            })?,
            None => &image_set.latest_version,
        };

        Ok(GetImageSetOutput {
            datastore_id: input.datastore_id.clone(),
            image_set_id: image_set.image_set_id.clone(),
            version_id: version.wire_version_id(),
            image_set_state: version.image_set_state.as_wire().to_string(),
            image_set_workflow_status: Some(version.workflow_status.as_wire().to_string()),
            // `createdAt` is the image SET's creation, which is why a later version
            // still reports the instant the image set itself appeared; `updatedAt` is
            // the reported VERSION's own.
            created_at: Some(crate::state::to_date_time(image_set.created_at)),
            updated_at: Some(crate::state::to_date_time(version.updated_at)),
            deleted_at: image_set.deleted_at.map(crate::state::to_date_time),
            message: version.message.clone(),
            image_set_arn: Some(image_set.image_set_arn.clone()),
            // "If an image set was copied or updated using the force flag, this object
            // will contain the forced flag" — so it is present only when it was.
            overrides: version.forced.then_some(Overrides { forced: Some(true) }),
            is_primary: Some(image_set.is_primary),
            // Absent, deliberately: recording when an image set was last read would be
            // a reporting operation writing a change no clock owed.
            last_accessed_at: None,
            storage_tier: Some(crate::state::DEFAULT_STORAGE_TIER.to_string()),
        })
    }
}
