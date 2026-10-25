// SPDX-License-Identifier: CC0-1.0

//! @op GetImageSetMetadata
//!
//! Serves the AGGREGATED DICOM metadata document of one image set, built from persisted
//! state and nothing else: the patient, study and series attributes the image set holds
//! ([`crate::state::ImageSet::dicom_tags`]) and the image frames it holds
//! ([`crate::state::ImageSet::image_frames`]), placed in the patient → study → series →
//! instance hierarchy DICOM gives them. An attribute nothing has stored is not in the
//! document at all, so a caller can never read an attribute this emulator invented.
//!
//! The document is JSON, written into the blob as it stands: `contentType` states the
//! format a client reads it as ([`METADATA_CONTENT_TYPE`]), and `contentEncoding` — "the
//! compression format in which image set metadata attributes are returned" — is absent,
//! because nothing compressed it. The response never names a compression the blob is not
//! in, so a client decodes the blob from the response alone.
//!
//! `version` names ONE version of the image set, and a version the image set does not
//! hold is absent (404) exactly as it is for `GetImageSet`. The DICOM attributes this
//! service persists belong to the image SET rather than to one version of it — the
//! model's own `DICOMTags` is per image set — so every version the image set holds
//! aggregates the same content.
//!
//! A DELETED image set holds no content: its pixel data went with the deletion
//! ([`crate::state::ImageSet::advance_to`]), so there is no document to aggregate and
//! the call answers as it does for an image set the data store never held. Its
//! PROPERTIES are still reported, by `GetImageSet`.
//!
//! Serving the metadata IS an access of the content the image set holds — which is what
//! `lastAccessedAt` ("When the image set was last accessed") reports — so the access is
//! recorded on the image set past every refusal, the same way `GetImageFrame` records
//! serving pixel data. Nothing else is written: no status moves, no version is minted,
//! and two reads serve the same bytes.
//!
//! ---
//!
//! # Modeled contract for `GetImageSetMetadata`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Get metadata attributes for an image set.
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
//! * `contentEncoding` (String) — The compression format in which image set metadata
//!   attributes are returned.
//! * `contentType` (String) — The format in which the study metadata is returned to the
//!   customer. Default is text/plain.
//! * `imageSetMetadataBlob` (ImageSetMetadataBlob) **required** — The blob containing the
//!   aggregated metadata information for the image set.
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

use ::std::collections::BTreeMap;

use ::serde_json::{Map, Value};

use crate::state::{ImageFrame, ImageSet};

/// The `contentType` this operation reports: the document in the blob is JSON.
pub const METADATA_CONTENT_TYPE: &str = "application/json";

// `contentEncoding` — "the compression format in which image set metadata attributes are
// returned" — is REPORTED BY ABSENCE here: this handler writes the document into the blob
// uncompressed, so there is no compression format to name, and a client reads the blob as
// the `contentType` below describes it. Nothing in the response may claim a compression
// that was not applied, which is why no constant for one exists.

/// The version of the aggregated-metadata document this handler writes, carried in the
/// document's own `SchemaVersion` member so a reader can tell which shape it is reading.
///
/// A constant of the document FORMAT, not of any request: it is written as a string, the
/// way the published aggregated-metadata document carries it, and it changes only when
/// the shape this handler writes changes — nothing about an image set can move it.
pub const METADATA_SCHEMA_VERSION: &str = "1.1";

/// Add `keyword` to a DICOM attribute object, but only when the attribute is STORED: an
/// absent attribute is absent from the document rather than present and null, so a
/// caller never reads a value nothing recorded.
fn put_text(dicom: &mut Map<String, Value>, keyword: &str, stored: Option<&String>) {
    if let Some(value) = stored {
        dicom.insert(keyword.to_string(), Value::String(value.clone()));
    }
}

/// The patient-level attributes of `tags`, under the DICOM keyword of each.
///
/// The keywords are the DICOM attribute names the stored members are named after
/// (`smithy_query(query="shape",shape="DICOMTags")`: `DICOMPatientId`,
/// `DICOMPatientName`, `DICOMPatientBirthDate`, `DICOMPatientSex`), which is how a DICOM
/// metadata document spells them.
fn patient_attributes(tags: &DICOMTags) -> Value {
    let mut dicom = Map::new();
    put_text(&mut dicom, "PatientID", tags.dicom_patient_id.as_ref());
    put_text(&mut dicom, "PatientName", tags.dicom_patient_name.as_ref());
    put_text(
        &mut dicom,
        "PatientBirthDate",
        tags.dicom_patient_birth_date.as_ref(),
    );
    put_text(&mut dicom, "PatientSex", tags.dicom_patient_sex.as_ref());
    Value::Object(dicom)
}

/// The study-level attributes of `tags`, under the DICOM keyword of each, by the same
/// rule as [`patient_attributes`].
///
/// The two study counts are stored as numbers rather than as optional text
/// (`dicomNumberOfStudyRelatedSeries`, `dicomNumberOfStudyRelatedInstances`), so they are
/// always reported: an image set nothing has delivered content to holds zero of each.
fn study_attributes(tags: &DICOMTags) -> Value {
    let mut dicom = Map::new();
    put_text(
        &mut dicom,
        "StudyInstanceUID",
        tags.dicom_study_instance_uid.as_ref(),
    );
    put_text(&mut dicom, "StudyID", tags.dicom_study_id.as_ref());
    put_text(
        &mut dicom,
        "StudyDescription",
        tags.dicom_study_description.as_ref(),
    );
    put_text(&mut dicom, "StudyDate", tags.dicom_study_date.as_ref());
    put_text(&mut dicom, "StudyTime", tags.dicom_study_time.as_ref());
    put_text(
        &mut dicom,
        "AccessionNumber",
        tags.dicom_accession_number.as_ref(),
    );
    dicom.insert(
        "NumberOfStudyRelatedSeries".to_string(),
        Value::from(tags.dicom_number_of_study_related_series),
    );
    dicom.insert(
        "NumberOfStudyRelatedInstances".to_string(),
        Value::from(tags.dicom_number_of_study_related_instances),
    );
    Value::Object(dicom)
}

/// The DICOM SOP instances `image_set` holds, keyed by SOP Instance UID, each with the
/// image frames of that instance.
///
/// An image frame carries the instance its pixel data came from
/// ([`crate::state::ImageFrame::sop_instance_uid`]), so the instances are exactly the
/// ones the stored pixel data belongs to: a frame stored without an instance belongs to
/// none and is reported under none. Each frame is reported by the `ImageFrameId`
/// `GetImageFrame` takes, which is what a client reads the pixel data with.
///
/// Ordered: the instances by UID and each instance's frames by identifier, so the one
/// document is served byte for byte however the map holding the frames is laid out.
fn instances(image_set: &ImageSet) -> Value {
    let mut by_instance: BTreeMap<&str, Vec<&ImageFrame>> = BTreeMap::new();
    for frame in image_set.image_frames.values() {
        if let Some(uid) = frame.sop_instance_uid.as_deref() {
            by_instance.entry(uid).or_default().push(frame);
        }
    }
    let mut instances = Map::new();
    for (uid, mut frames) in by_instance {
        frames.sort_by(|left, right| left.image_frame_id.cmp(&right.image_frame_id));
        let image_frames: Vec<Value> = frames
            .iter()
            .map(|frame| ::serde_json::json!({ "ID": frame.image_frame_id }))
            .collect();
        instances.insert(
            uid.to_string(),
            ::serde_json::json!({
                "DICOM": { "SOPInstanceUID": uid },
                "ImageFrames": image_frames,
            }),
        );
    }
    Value::Object(instances)
}

/// The series `image_set` holds, keyed by Series Instance UID.
///
/// This service persists the attributes of ONE series per image set (the model's
/// `DICOMTags` declares one `dicomSeriesInstanceUID`), so an image set that has had DICOM
/// content delivered to it reports that series and an image set nothing has delivered
/// content to reports none — rather than a series with no identifier, which no caller
/// could read an instance out of.
fn series(image_set: &ImageSet) -> Value {
    let tags = &image_set.dicom_tags;
    let mut series = Map::new();
    if let Some(series_instance_uid) = tags.dicom_series_instance_uid.as_ref() {
        let mut dicom = Map::new();
        dicom.insert(
            "SeriesInstanceUID".to_string(),
            Value::String(series_instance_uid.clone()),
        );
        put_text(&mut dicom, "Modality", tags.dicom_series_modality.as_ref());
        put_text(
            &mut dicom,
            "BodyPartExamined",
            tags.dicom_series_body_part.as_ref(),
        );
        if let Some(series_number) = tags.dicom_series_number {
            dicom.insert("SeriesNumber".to_string(), Value::from(series_number));
        }
        series.insert(
            series_instance_uid.clone(),
            ::serde_json::json!({
                "DICOM": Value::Object(dicom),
                "Instances": instances(image_set),
            }),
        );
    }
    Value::Object(series)
}

/// The whole aggregated metadata document of `image_set`, as it stands in `datastore_id`.
///
/// Every value in it is read from state: the two identifiers name the records the document
/// was built from, and the attribute objects carry what has been stored for the image set
/// and nothing besides.
fn metadata_document(datastore_id: &str, image_set: &ImageSet) -> Value {
    ::serde_json::json!({
        "SchemaVersion": METADATA_SCHEMA_VERSION,
        "DatastoreID": datastore_id,
        "ImageSetID": image_set.image_set_id,
        "Patient": { "DICOM": patient_attributes(&image_set.dicom_tags) },
        "Study": {
            "DICOM": study_attributes(&image_set.dicom_tags),
            "Series": series(image_set),
        },
    })
}


// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetImageSetMetadataError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetImageSetMetadataError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#GetImageSetMetadata")]
#[derive(Debug)]
pub enum GetImageSetMetadataErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_image_set_metadata::GetImageSetMetadataError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetImageSetMetadataError = crate::scaffolded::Scaffolded<GetImageSetMetadataErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetImageSetMetadataErrorModeled> for GetImageSetMetadataError {
    fn from(modeled: GetImageSetMetadataErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetImageSetMetadata` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetImageSetMetadata<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetImageSetMetadata<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetImageSetMetadata<Eng>
{
    type Input = GetImageSetMetadataInput;
    type Output = GetImageSetMetadataOutput;
    type Error = GetImageSetMetadataError;

    const NAME: &'static str = "GetImageSetMetadata";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IMAGE_SET_METADATA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IMAGE_SET_METADATA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetImageSetMetadataErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Every identifier below has already satisfied its modelled `@pattern`
        // (`datastoreId` / `imageSetId` `^[0-9a-z]{32}$`, `version` `^\d+$`): the
        // boundary validator refuses a malformed one with 400 `ValidationException`
        // before this handler runs, so what is left to decide here is only whether the
        // service HOLDS what the request names.
        //
        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all.
        let datastore = store.datastore_in(&region, &input.datastore_id).ok_or_else(|| {
            GetImageSetMetadataErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        let image_set = datastore.image_sets.get(&input.image_set_id).ok_or_else(|| {
            GetImageSetMetadataErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        // A DELETED image set holds no content, so what is absent is the image set
        // itself rather than one attribute of it — the same answer `GetImageFrame` gives
        // for the pixel data that went with the deletion.
        if image_set.is_deleted() {
            return Err(GetImageSetMetadataErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                },
            )
            .into());
        }

        // `version` names ONE version of the image set; a version this image set does
        // not hold is absent, not invalid. The content aggregated below belongs to the
        // image SET, so every version it holds reports the same document.
        if let Some(version_id) = &input.version_id {
            if image_set.version(version_id).is_none() {
                return Err(GetImageSetMetadataErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::IMAGE_SET_VERSION_NOT_FOUND_MESSAGE.to_string(),
                    },
                )
                .into());
            }
        }

        let document = metadata_document(&input.datastore_id, image_set);
        let serialized = ::serde_json::to_vec(&document).map_err(|_| {
            GetImageSetMetadataErrorModeled::InternalServerException(InternalServerException {
                message: "the image set metadata document could not be written".to_string(),
            })
        })?;

        let output = GetImageSetMetadataOutput {
            image_set_metadata_blob: serialized,
            content_type: Some(METADATA_CONTENT_TYPE.to_string()),
            // Nothing compressed the document, so there is no compression format to
            // report: the member is absent rather than naming one the blob is not in.
            content_encoding: None,
        };

        // The metadata WAS served, so the content this image set holds was accessed at
        // this instant: that is the event `lastAccessedAt` reports, and it is recorded on
        // the image SET because content belongs to the image set rather than to one
        // version of it. Reached again mutably only now, past every refusal, so a call
        // that served nothing recorded nothing — and reached through the REGION this
        // request was addressed to, so the record written is the one that was read.
        if let Some(image_set) = store
            .datastore_in_mut(&region, &input.datastore_id)
            .and_then(|datastore| datastore.image_sets.get_mut(&input.image_set_id))
        {
            image_set.record_access(now);
        }

        Ok(output)
    }
}
