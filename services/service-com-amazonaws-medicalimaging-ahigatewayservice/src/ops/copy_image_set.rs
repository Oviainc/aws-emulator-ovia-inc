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
//! * `promoteToPrimary` is NOT a flag written onto a record the caller named: the member
//!   documents that "a new primary image set ID will be returned as the destination image
//!   set", so the call MINTS the destination and mints it in the primary DICOM hierarchy.
//!   A request that also names `destinationImageSet` asks for two different destinations
//!   at once and cannot hold together, so it is refused with `ValidationException` (400),
//!   naming the member, like every other refusal this handler raises itself.
//! * membership of the primary DICOM hierarchy (`isPrimary`) is otherwise CARRIED by the
//!   copy rather than chosen by the request: a copy of a whole image set mints a
//!   destination in the hierarchy its source is in, while a copy of a SUBSET of the
//!   source's SOP instances (`DICOMCopies.copiableAttributes`) reproduces that hierarchy
//!   only in part and so lands outside the primary one — which is what leaves a caller
//!   something to promote. An image set the copy VERSIONS keeps the membership it already
//!   had: a copy into it changes its content, not which hierarchy it belongs to.
//! * `force` is recorded as the destination version's `Overrides.forced`, which is
//!   exactly what the model says that member reports — "if `forced` exists, the
//!   `forced` flag was used when creating the image set" — and is what `GetImageSet`
//!   reads back. It also does what it is documented FOR: it "will force completion of the
//!   CopyImageSet operation, even if there are inconsistent Patient, Study, and/or Series
//!   level metadata elements between the sourceImageSet and destinationImageSet", so
//!   without it a copy whose content disagrees with the destination's about one of those
//!   elements is not completed at all but refused with the modelled `ConflictException`
//!   (409, [`crate::state::IMAGE_SET_COPY_INCONSISTENT_METADATA_MESSAGE`]). An element only
//!   one of the two holds is no disagreement — the copy supplies it — so an image set that
//!   holds no DICOM content yet takes any copy, and a MINTED destination holds nothing to
//!   disagree with.
//! * `sourceImageSet.DICOMCopies.copiableAttributes` is documented as "the JSON string
//!   used to specify a subset of SOP Instances to copy", and that subset is named in the
//!   shape of the aggregated metadata document — an OBJECT — so a value that does not parse
//!   as JSON, and one that parses as any other JSON value (a scalar, `null`, an array),
//!   names no SOP instance at all: both fail the constraint the service sets and are
//!   refused with `ValidationException` (400) by the one rule every member of this service
//!   documented as carrying such a document is read through
//!   ([`crate::state::metadata_document`]), worded like every other refusal of this service
//!   and WITHOUT echoing the value, which the model marks `@sensitive`. A value that IS one
//!   SELECTS the content the copy carries: the document is written in the shape of the
//!   aggregated metadata document, which places a SOP instance at `Study` → `Series` →
//!   series UID → `Instances` → SOP Instance UID, and only the instances it names there
//!   reach the destination — their pixel data and their attributes, and nothing of the
//!   instances it leaves out. The patient, study and series attributes travel with them,
//!   because the copied instances still belong to that patient, study and series. Nothing
//!   of the selector itself is persisted: what the destination records is the content, and
//!   the model marks the document `@sensitive`.
//!
//! Both image sets come back `LOCKED` — the source `COPYING_WITH_READ_ONLY_ACCESS`,
//! the destination `COPYING` — and the CLOCK, not a later reader, releases them after
//! [`crate::state::IMAGE_SET_COPY_DELAY`]: both then report `COPIED`, the workflow status
//! the model declares for a completed copy, because the copy is what last happened to each
//! of them. What the copy leaves alone is the source's CONTENT. Nothing here sleeps.
//!
//! The DATA STORE the copy runs in must have settled into `ACTIVE` as well: a copy
//! writes into the store, so one still `CREATING`, or already `DELETING`, is refused
//! with the same modelled `ConflictException` ([`crate::state::Datastore::is_active`]).
//!
//! A copy also carries the source's PIXEL DATA: the image frames the source holds when
//! the copy is admitted — those of the selected SOP instances, where the request selected
//! a subset — are delivered to the destination by the same clock-owed transition that
//! releases it, so `GetImageFrame` serves them from the destination once the copy has
//! completed and finds nothing there before that.
//!
//! What that transition delivers is ADDED to the destination rather than put in place of
//! what it held: the content a copy carries is delivered item by item — a frame under the
//! image frame id it is stored by, an attribute at its own path in the metadata document —
//! so the destination ends up holding what was copied into it together with everything of
//! its own the copy describes nothing about ([`crate::state::ImageSet::merge_attributes`]).
//! That is what makes the premise of the consistency check below hold: an element only one
//! of the two sides holds is no disagreement BECAUSE neither side loses it.
//!
//! Because the copy carries that content, a `DELETED` image set named as its source or as
//! its destination is what is ABSENT rather than what is busy: the deletion took the pixel
//! data and the DICOM attributes with it, so the copy answers 404
//! `ResourceNotFoundException` worded by the image-set kind, exactly as the other calls on
//! an image set's content do ([`crate::state::ImageSet::is_deleted`]).
//!
//! A copy that MINTS its destination is bounded by how many image sets one data store
//! will hold ([`crate::state::MAX_IMAGE_SETS_PER_DATASTORE`]), refused past that with the
//! modelled `ServiceQuotaExceededException` (402). The bound counts the image sets the
//! store still HOLDS, never the tombstones beside them
//! ([`crate::state::Datastore::held_image_sets`]): a retired image set has given up its
//! content, so it takes no place and its deletion gives its place back. What a tombstone
//! does keep is its RECORD, which is still reported, so the records are bounded in their
//! own right ([`crate::state::MAX_IMAGE_SET_RECORDS_PER_DATASTORE`]).
//!
//! A copy into an EXISTING destination is bounded by what that one image set accumulates
//! instead: the versions it keeps ([`crate::state::MAX_IMAGE_SET_VERSIONS`]) and the
//! attributes outside the modelled ones the delivery would leave it holding
//! ([`crate::state::MAX_IMAGE_SET_EXTRA_ATTRIBUTES`], the bound every writer of those
//! attributes answers to) — each refused with the same modelled
//! `ServiceQuotaExceededException`, before anything is written.
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

use ::std::collections::{BTreeSet, HashMap};

use ::serde_json::{Map, Value};

use crate::state::{
    ImageFrame, ImageSet, ImageSetAttributes, ImageSetState, ImageSetWorkflowStatus,
    PendingImageSetTransition, METADATA_INSTANCES_KEY, METADATA_SERIES_KEY, METADATA_STUDY_KEY,
};

/// The validation path of `copiableAttributes`, as the boundary validator spells it —
/// the member whose JSON this handler checks itself.
const COPIABLE_ATTRIBUTES_PATH: &str =
    "copyImageSetInformation.sourceImageSet.DICOMCopies.copiableAttributes";

/// The SOP instances a `copiableAttributes` document NAMES — the subset the copy is
/// restricted to.
///
/// The member is "the JSON string used to specify a subset of SOP Instances to copy", and
/// the document is written in the shape of the aggregated metadata document, which places
/// one SOP instance at `Study` → `Series` → series UID → `Instances` → SOP Instance UID
/// (the hierarchy `crate::ops::get_image_set_metadata` serves and
/// `crate::ops::update_image_set_metadata` writes). So the instances selected are the keys
/// of every `Instances` level the document carries, whichever series locates them: what
/// identifies a SOP instance is its own UID. A document that names none selects none, and
/// the copy carries no instance.
fn selected_instances(document: &Map<String, Value>) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    let series = document
        .get(METADATA_STUDY_KEY)
        .and_then(|study| study.get(METADATA_SERIES_KEY))
        .and_then(Value::as_object);
    for one_series in series.into_iter().flat_map(Map::values) {
        let instances = one_series
            .get(METADATA_INSTANCES_KEY)
            .and_then(Value::as_object);
        selected.extend(instances.into_iter().flat_map(Map::keys).cloned());
    }
    selected
}

/// The image frames of `frames` whose pixel data came from one of the SOP instances in
/// `selected` — the pixel data a subset copy carries.
///
/// A frame records the instance its pixel data came from
/// ([`crate::state::ImageFrame::sop_instance_uid`]), so pixel data stored under no instance
/// is named by no selector and stays behind.
fn frames_of_instances(
    frames: &HashMap<String, ImageFrame>,
    selected: &BTreeSet<String>,
) -> HashMap<String, ImageFrame> {
    frames
        .iter()
        .filter(|(_, frame)| {
            frame
                .sop_instance_uid
                .as_ref()
                .is_some_and(|uid| selected.contains(uid))
        })
        .map(|(image_frame_id, frame)| (image_frame_id.clone(), frame.clone()))
        .collect()
}

/// `attributes` with every SOP instance outside `selected` left behind — the DICOM
/// attributes a subset copy carries.
///
/// The attributes of the patient, the study and the series are kept: the selected instances
/// still belong to them, so what the destination reports about them describes the content it
/// holds. The two study counts are study-level elements of the study the content belongs to
/// rather than a count of what one image set holds, so they are carried as they stand.
fn attributes_of_instances(
    mut attributes: ImageSetAttributes,
    selected: &BTreeSet<String>,
) -> ImageSetAttributes {
    retain_selected_instances(&mut attributes.extra_attributes, selected);
    attributes
}

/// Keep, in the `Instances` levels of `attributes`, only the SOP instances `selected`
/// names, and drop every level the removal leaves empty — so an instance left out of the
/// copy is gone from the destination's document rather than reported as a level with
/// nothing in it.
fn retain_selected_instances(attributes: &mut Map<String, Value>, selected: &BTreeSet<String>) {
    let Some(study) = attributes
        .get_mut(METADATA_STUDY_KEY)
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let mut series_emptied = false;
    if let Some(series_level) = study
        .get_mut(METADATA_SERIES_KEY)
        .and_then(Value::as_object_mut)
    {
        for one_series in series_level.values_mut() {
            let Some(series_object) = one_series.as_object_mut() else {
                continue;
            };
            let Some(instances) = series_object
                .get_mut(METADATA_INSTANCES_KEY)
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            instances.retain(|sop_instance_uid, _| selected.contains(sop_instance_uid));
            if instances.is_empty() {
                series_object.remove(METADATA_INSTANCES_KEY);
            }
        }
        series_level.retain(|_, one_series| !one_series.as_object().is_some_and(Map::is_empty));
        series_emptied = series_level.is_empty();
    }
    if series_emptied {
        study.remove(METADATA_SERIES_KEY);
    }
    if study.is_empty() {
        attributes.remove(METADATA_STUDY_KEY);
    }
}

/// Whether the two sets of DICOM attributes DISAGREE about a patient, study or series level
/// metadata element: an element both of them hold, holding different values.
///
/// An element only one of them holds is no disagreement — the copy supplies what the
/// destination has not got — so an image set nothing has delivered DICOM content to is
/// consistent with every source. The elements compared are the ones the model names at
/// those three levels (`smithy_query(query="shape",shape="DICOMTags")`), each of which
/// describes where the content sits in the DICOM hierarchy or who it is about.
fn inconsistent_metadata(copied: &DICOMTags, held: &DICOMTags) -> bool {
    text_elements(copied)
        .into_iter()
        .zip(text_elements(held))
        .any(|(copied, held)| disagrees(copied.as_ref(), held.as_ref()))
        || number_elements(copied)
            .into_iter()
            .zip(number_elements(held))
            .any(|(copied, held)| disagrees(copied.as_ref(), held.as_ref()))
}

/// Whether one element's two values disagree: both are there, and they differ.
fn disagrees<T: PartialEq>(copied: Option<&T>, held: Option<&T>) -> bool {
    match (copied, held) {
        (Some(copied), Some(held)) => copied != held,
        _ => false,
    }
}

/// Every patient, study and series level element the model carries as TEXT, in one order,
/// so two image sets' elements are compared pairwise.
fn text_elements(tags: &DICOMTags) -> [&Option<String>; 13] {
    [
        &tags.dicom_patient_id,
        &tags.dicom_patient_name,
        &tags.dicom_patient_birth_date,
        &tags.dicom_patient_sex,
        &tags.dicom_study_instance_uid,
        &tags.dicom_study_id,
        &tags.dicom_study_description,
        &tags.dicom_study_date,
        &tags.dicom_study_time,
        &tags.dicom_accession_number,
        &tags.dicom_series_instance_uid,
        &tags.dicom_series_modality,
        &tags.dicom_series_body_part,
    ]
}

/// The same for the elements the model carries as NUMBERS. The two study counts are
/// modelled as plain numbers rather than optional ones, so zero is what an image set
/// nothing has stored a count for holds: absent, not a count of none.
fn number_elements(tags: &DICOMTags) -> [Option<i32>; 3] {
    [
        (tags.dicom_number_of_study_related_series != 0)
            .then_some(tags.dicom_number_of_study_related_series),
        (tags.dicom_number_of_study_related_instances != 0)
            .then_some(tags.dicom_number_of_study_related_instances),
        tags.dicom_series_number,
    ]
}

/// The validation path of the destination image set's identifier.
const DESTINATION_IMAGE_SET_ID_PATH: &str = "copyImageSetInformation.destinationImageSet.imageSetId";

/// The validation path of the destination image set member itself — the member a promote
/// request must not carry, since a promote mints its own destination.
const DESTINATION_IMAGE_SET_PATH: &str = "copyImageSetInformation.destinationImageSet";

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
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();
        let information = &input.copy_image_set_information;
        let promote_to_primary = input.promote_to_primary.unwrap_or(false);
        let forced = input.force.unwrap_or(false);

        // `copiableAttributes` is documented as "the JSON string used to specify a
        // subset of SOP Instances to copy", named in the shape of the aggregated metadata
        // document, and the model constrains only its length — so whether it IS such a
        // document is checked here, before any state is read, by the rule every member of
        // this service carrying one is read through. A value that parses as some other
        // JSON value names no SOP instance and is refused rather than honoured as a
        // selection. The value is never echoed: the model marks the member `@sensitive`.
        // What the document NAMES is the subset the copy is then restricted to; a request
        // that carries no `DICOMCopies` copies the whole image set.
        let selected = match &information.source_image_set.dicom_copies {
            Some(copies) => {
                match crate::state::metadata_document(copies.copiable_attributes.as_bytes()) {
                    Ok(document) => Some(selected_instances(&document)),
                    Err(requirement) => {
                        return Err(CopyImageSetErrorModeled::ValidationException(
                            ValidationException {
                                message: crate::state::handler_requirement_error(
                                    COPIABLE_ATTRIBUTES_PATH,
                                    None,
                                    requirement,
                                ),
                            },
                        )
                        .into());
                    }
                }
            }
            None => None,
        };

        // A promote answers with "a new primary image set ID ... as the destination image
        // set", so it mints its destination: a request that ALSO names one is asking for
        // two destinations at once and has no coherent answer. Refused as the
        // input-constraint failure it is, naming the member that cannot be there.
        if promote_to_primary && information.destination_image_set.is_some() {
            return Err(CopyImageSetErrorModeled::ValidationException(ValidationException {
                message: crate::state::handler_requirement_error(
                    DESTINATION_IMAGE_SET_PATH,
                    None,
                    "Member must not be present when promoteToPrimary is requested",
                ),
            })
            .into());
        }

        // A copy whose destination IS the source has no coherent answer: the one image
        // set would have to be the read-only source and the locked destination of the
        // same copy. Refused as the input-constraint failure it is, naming the member —
        // and NOT the identifier it held: what the request broke is this member's
        // relation to `sourceImageSetId`, not anything about the value itself, which is
        // a legal `ImageSetId`. So the clause withholds it, exactly as the promote
        // clash above does ([`crate::state::handler_requirement_error`]).
        if let Some(destination) = &information.destination_image_set {
            if destination.image_set_id == input.source_image_set_id {
                return Err(CopyImageSetErrorModeled::ValidationException(ValidationException {
                    message: crate::state::handler_requirement_error(
                        DESTINATION_IMAGE_SET_ID_PATH,
                        None,
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

        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all.
        let datastore = store.datastore_in(&region, &input.datastore_id).ok_or_else(|| {
            CopyImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        let datastore_arn = datastore.datastore_arn.clone();

        // A copy mints or supersedes an image set INSIDE the data store, so the store has
        // to have settled into `ACTIVE` first: one still `CREATING`, or already
        // `DELETING`, is refused with the modelled `ConflictException` (HTTP 409) before
        // any image set is looked up ([`crate::state::Datastore::is_active`]).
        if !datastore.is_active() {
            return Err(CopyImageSetErrorModeled::ConflictException(ConflictException {
                message: crate::state::DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
            })
            .into());
        }

        let source = datastore
            .image_sets
            .get(&input.source_image_set_id)
            .ok_or_else(|| {
                CopyImageSetErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                })
            })?;
        // A DELETED image set is a tombstone: its pixel data and its DICOM attributes went
        // with the deletion, so a call that carries that CONTENT finds the image set itself
        // absent rather than busy — the same answer `GetImageSetMetadata`, `GetImageFrame`
        // and `UpdateImageSetMetadata` give, decided by the one helper that owns the rule
        // ([`crate::state::ImageSet::is_deleted`]). Before the version and state checks, as
        // in `UpdateImageSetMetadata`: a record that is gone is not reported as a stale view
        // of one that is still there.
        if source.is_deleted() {
            return Err(CopyImageSetErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                },
            )
            .into());
        }
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
        // Which DICOM hierarchy the source belongs to, which a whole copy of it carries.
        let source_is_primary = source.is_primary;
        // The pixel data this copy carries to its destination: read from the source as
        // it stands when the copy is admitted, delivered when the copy completes. A
        // request that selected a subset of the source's SOP instances carries the pixel
        // data of THOSE instances and no other.
        let copied_image_frames = match &selected {
            Some(selected) => frames_of_instances(&source.image_frames, selected),
            None => source.image_frames.clone(),
        };
        // The DICOM attributes travel with it: a copy of an image set describes the same
        // patient, study and series — the attributes the model names and the attributes a
        // metadata update wrote alike — so the destination is searchable by them, and
        // reports them, once the copy has landed. A subset copy carries the attributes of
        // the instances it selected, and none of those it left behind.
        let copied_attributes = match &selected {
            Some(selected) => attributes_of_instances(source.attributes(), selected),
            None => source.attributes(),
        };
        // The image sets this data store still HOLDS, which is what the cap on its image
        // sets counts: a retired one has given up its content and takes no place, so a
        // deletion gives its place back ([`crate::state::Datastore::held_image_sets`]).
        let held_image_sets = datastore.held_image_sets().count();
        // Every image set RECORD it holds, the tombstones of finished deletions included:
        // a retired record is still reported, so it still occupies memory, which is what
        // the record cap bounds.
        let image_set_records = datastore.image_sets.len();

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
                // The destination carries the copied content too, so a tombstone is the
                // same absence there: by the rule the source is decided by.
                if held.is_deleted() {
                    return Err(CopyImageSetErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                        },
                    )
                    .into());
                }
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
                // `force` "will force completion of the CopyImageSet operation, even if
                // there are inconsistent Patient, Study, and/or Series level metadata
                // elements between the sourceImageSet and destinationImageSet" — so
                // WITHOUT it a copy whose content disagrees with the destination's about
                // one of those elements is not completed: the destination would end up
                // describing two different studies, which is the inconsistent state the
                // modelled `ConflictException` (HTTP 409) reports.
                if !forced && inconsistent_metadata(&copied_attributes.dicom_tags, &held.dicom_tags)
                {
                    return Err(CopyImageSetErrorModeled::ConflictException(ConflictException {
                        message: crate::state::IMAGE_SET_COPY_INCONSISTENT_METADATA_MESSAGE
                            .to_string(),
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
                // A copy DELIVERS attributes into the destination rather than putting them
                // in place of what it held, so it is a writer of the attributes outside the
                // modelled ones and is bounded by the same cap the metadata update is
                // (`crate::state::MAX_IMAGE_SET_EXTRA_ATTRIBUTES`), with the same wording:
                // every version the destination keeps holds a snapshot of them, so without
                // it a run of copies could grow one image set without limit. Counted over
                // what the destination WOULD hold once this copy had landed, while the
                // request is still being checked, so no admitted copy lands an image set
                // past the bound.
                let mut delivered_into = held.attributes();
                delivered_into.merge(copied_attributes.clone());
                if delivered_into.written_attributes()
                    > crate::state::MAX_IMAGE_SET_EXTRA_ATTRIBUTES
                {
                    return Err(CopyImageSetErrorModeled::ServiceQuotaExceededException(
                        ServiceQuotaExceededException {
                            message: crate::state::extra_attributes_quota_message(),
                        },
                    )
                    .into());
                }
                DestinationPlan::Version(destination.image_set_id.clone())
            }
            None => {
                // The same per-data-store memory bound the import jobs carry
                // (`crate::state::MAX_IMAGE_SETS_PER_DATASTORE`), refused with this
                // operation's modelled quota error. Counted over the image sets the store
                // still holds, so a data store whose image sets have all been deleted can
                // be copied into again.
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
                // A retired image set gives its place back but keeps a record that is
                // still reported, so what this emulator HOLDS is bounded separately
                // (`crate::state::MAX_IMAGE_SET_RECORDS_PER_DATASTORE`): without it a
                // mint/delete loop would grow one data store without limit.
                if image_set_records >= crate::state::MAX_IMAGE_SET_RECORDS_PER_DATASTORE {
                    return Err(CopyImageSetErrorModeled::ServiceQuotaExceededException(
                        ServiceQuotaExceededException {
                            message: format!(
                                "this emulator holds at most {} image set records per data store",
                                crate::state::MAX_IMAGE_SET_RECORDS_PER_DATASTORE
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
        //
        // The hierarchy a MINTED destination comes into existence in. A promote asks for a
        // primary one outright; otherwise the copy carries what it copied — the whole
        // image set keeps its source's membership, a subset of its SOP instances
        // reproduces that hierarchy only in part and lands outside the primary one.
        let minted_is_primary = if promote_to_primary {
            true
        } else if selected.is_some() {
            false
        } else {
            source_is_primary
        };
        let copy_due = PendingImageSetTransition {
            due_at: now + crate::state::IMAGE_SET_COPY_DELAY,
            image_set_state: ImageSetState::Active,
            workflow_status: ImageSetWorkflowStatus::Copied,
            // The destination holds the source's pixel data once the copy has
            // COMPLETED, never before: a frame is readable exactly when the copy that
            // delivers it lands.
            delivered_image_frames: Some(copied_image_frames),
            // The destination describes the source's patient, study and series from the
            // same instant: the attributes a search filters on are delivered with the
            // content they describe. Delivered attribute by attribute, as the frames are
            // delivered frame by frame, so a destination that already held DICOM content
            // keeps every attribute of its own this copy carries nothing for.
            delivered_attributes: Some(copied_attributes),
            // A copy does not retire an image set: both the source and the destination
            // are still there once it has landed.
            retires_image_set: false,
        };

        let datastore = store
            .datastore_in_mut(&region, &input.datastore_id)
            .ok_or_else(|| {
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
                    minted_is_primary,
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
                // An image set the copy versions keeps the hierarchy it is already in: a
                // copy into it changes the content it holds, not which hierarchy holds it
                // — and a promote never reaches here, having been refused above.
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
        // The lock reaches every version: it is the state the image set is in while the
        // copy runs, not the state of one snapshot of it, so no version is left reporting
        // `ACTIVE` for an image set every writer refuses
        // ([`crate::state::ImageSet::set_state_on_every_version`]). The workflow status —
        // what last happened — is recorded on the latest version.
        source.set_state_on_every_version(ImageSetState::Locked);
        source.latest_version.workflow_status = ImageSetWorkflowStatus::CopyingWithReadOnlyAccess;
        source.latest_version.updated_at = now;
        // The copy READ this source's pixel data and DICOM attributes just above, so the
        // source was accessed at the instant the copy was admitted — what the image set's
        // `lastAccessedAt` reports. The DESTINATION is accessed when the content lands in
        // it, which the clock records where it delivers it
        // (`crate::state::ImageSet::advance_to`).
        source.record_access(now);
        source.pending = Some(PendingImageSetTransition {
            // `imageSetWorkflowStatus` reports WHAT LAST HAPPENED to an image set, and
            // `COPIED` is "the image set copy is complete"
            // (`smithy_query(query="shape",shape="ImageSetWorkflowStatus")`). A completed
            // copy happened to BOTH image sets it names, so the source reports it too
            // once its read-only lock is released — the same status the destination lands
            // on, from the same instant. What a copy leaves alone is the source's
            // CONTENT, not the record of what it last took part in.
            //
            // Recorded, not inferred: the behavioural replay of a client that copies an
            // image set and then waits for its SOURCE to reach a terminal copy status
            // reads `COPIED` there
            // (`issues/things-that-now-work/20261029T201912Z-medical-imaging-imported-image-sets-never-materialise-deletedatastore.md`,
            // the `copy-image-set-creates-active-copy-with-matching-metadata` check). An
            // earlier change here restored the status the source carried going in, which
            // that client waits on forever.
            workflow_status: ImageSetWorkflowStatus::Copied,
            // A copy READS its source and writes its destination, so the release of the
            // source's read-only lock delivers no pixel data.
            delivered_image_frames: None,
            // Nor any DICOM attributes: the source already holds its own.
            delivered_attributes: None,
            ..copy_due
        });

        // Report from the records just written, not from local copies of them.
        let datastore = store.datastore_in(&region, &input.datastore_id).ok_or_else(|| {
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
