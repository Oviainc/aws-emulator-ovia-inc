// SPDX-License-Identifier: CC0-1.0

//! @op GetImageSetMetadata
//!
//! Serves the AGGREGATED DICOM metadata document of one image set, built from persisted
//! state and nothing else: the patient, study and series attributes the version being read
//! holds ([`crate::state::ImageSet::attributes_of_version`]) and the image frames the image
//! set holds ([`crate::state::ImageSet::image_frames`]), placed in the patient → study →
//! series → instance hierarchy DICOM gives them. An attribute nothing has stored is not in
//! the document at all, so a caller can never read an attribute this emulator invented.
//!
//! The model names sixteen DICOM attributes and DICOM defines thousands, so an image set
//! also holds the attributes written into it that the model does not name
//! ([`crate::state::ImageSet::extra_attributes`]). Those are reported at the very path of
//! this document they were written at, merged INTO the levels built above — an attribute
//! is served from where it was stored, whether or not the model has a member for it. The
//! document the handler builds from state is what it reports: a stored attribute fills a
//! place the document has nothing at, and never displaces a value computed here (the
//! schema declaration, the two identifiers, a modelled attribute, the image frames of a
//! SOP instance) or collapses a level of the hierarchy, so the document can only ever name
//! the records it was built from.
//!
//! The document is JSON, and `contentType` — "the format in which the study metadata is
//! returned to the customer" — reports the format that member documents this operation
//! returns it in ([`METADATA_CONTENT_TYPE`]), not a format chosen here. The blob itself is
//! COMPRESSED, as AWS HealthImaging returns it, and `contentEncoding` — "the compression
//! format in which image set metadata attributes are returned" — names the format it is in
//! ([`METADATA_CONTENT_ENCODING`]). The response never names a compression the blob is not
//! in, nor applies one it does not name, so a client decodes the blob from the response
//! alone.
//!
//! `version` names ONE version of the image set, and the document reports the DICOM
//! attributes THAT version holds: the latest version's are the attributes the image set
//! holds now, and an earlier version's are the snapshot taken when a later version
//! superseded it ([`crate::state::ImageSet::attributes_of_version`]) — the same snapshot a
//! metadata update's revert puts back, so a version can never be served a document it never
//! held. A version the image set does not hold is absent (404) exactly as it is for
//! `GetImageSet`, and a request naming no version reads the latest.
//!
//! The pixel data is NOT held per version — an image frame is addressed by its own
//! identifier alone, which is all `GetImageFrame` takes — so the instance level reports the
//! frames the image set holds, under the SOP instance each one's pixel data came from.
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

use crate::state::{
    ImageFrame, ImageSet, ImageSetAttributes, METADATA_DATASTORE_ID_KEY, METADATA_DICOM_KEY,
    METADATA_IMAGE_SET_ID_KEY, METADATA_INSTANCES_KEY, METADATA_PATIENT_KEY,
    METADATA_SCHEMA_VERSION_KEY, METADATA_SERIES_KEY, METADATA_STUDY_KEY,
};

/// The `contentType` this operation reports for the study metadata it serves: the format
/// the response member's own documentation names for it —
/// `smithy_query(query="doc",shape="GetImageSetMetadataResponse",member="contentType")`,
/// "The format in which the study metadata is returned to the customer. Default is
/// `text/plain`."
///
/// That documentation distinguishes no case, unlike the `GetImageFrame` member's
/// per-transfer-syntax catalog ([`crate::state::image_frame_content_type`]), so the
/// documented default is the format every answer reports — whatever the document it
/// aggregates holds. The blob's own serialisation is unaffected: the document written into
/// it is the JSON one `metadata_document` builds.
pub const METADATA_CONTENT_TYPE: &str = "text/plain";

/// The `contentEncoding` this operation reports — "the compression format in which image
/// set metadata attributes are returned"
/// (`smithy_query(query="doc",shape="GetImageSetMetadataResponse",member="contentEncoding")`).
///
/// AWS HealthImaging returns the aggregated metadata document COMPRESSED, which is why the
/// response declares a compression format at all and why the published way to read the
/// blob is to decompress it (`aws medical-imaging get-image-set-metadata …
/// studymetadata.json.gz`). So the blob this handler writes really is gzip-compressed, and
/// the member names the format it is in: nothing in the response claims a compression that
/// was not applied, and nothing applies one the response does not name.
///
/// `contentEncoding` is the compression the CONTENT is carried under;
/// [`METADATA_CONTENT_TYPE`] is what the content itself is, which the compression does not
/// change.
pub const METADATA_CONTENT_ENCODING: &str = "gzip";

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
                METADATA_DICOM_KEY: { "SOPInstanceUID": uid },
                "ImageFrames": image_frames,
            }),
        );
    }
    Value::Object(instances)
}

/// The series `tags` describe, keyed by Series Instance UID, holding the SOP instances the
/// pixel data of `image_set` belongs to.
///
/// This service persists the attributes of ONE series per version of an image set (the
/// model's `DICOMTags` declares one `dicomSeriesInstanceUID`), so a version that has had
/// DICOM content delivered to it reports that series and one nothing has delivered content
/// to reports none — rather than a series with no identifier, which no caller could read an
/// instance out of.
fn series(tags: &DICOMTags, image_set: &ImageSet) -> Value {
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
                METADATA_DICOM_KEY: Value::Object(dicom),
                METADATA_INSTANCES_KEY: instances(image_set),
            }),
        );
    }
    Value::Object(series)
}

/// Merge the attributes the model does not name into the document, each at the path it
/// was written at.
///
/// A DEEP merge over objects: a stored object descends into the document level of the same
/// name, so a level the document already builds gains the attributes stored under it, and a
/// level the document has no member for appears because something stored one.
///
/// What the document already holds from state STANDS: the merge only fills places the
/// document has nothing at. So a stored value never takes the place of a value this handler
/// computed (the schema declaration, the identifiers naming the records the document was
/// built from, a modelled attribute, the image frames of a SOP instance) and never collapses
/// a level of the hierarchy it built, whatever it was stored under.
fn merge_stored_attributes(document: &mut Value, stored: &Map<String, Value>) {
    let Some(level) = document.as_object_mut() else {
        return;
    };
    for (name, value) in stored {
        match (level.get_mut(name), value) {
            (Some(held), Value::Object(nested)) if held.is_object() => {
                merge_stored_attributes(held, nested);
            }
            // The document already reports something computed from state here, so what was
            // stored under the same name does not displace it.
            (Some(_), _) => {}
            (None, _) => {
                level.insert(name.clone(), value.clone());
            }
        }
    }
}

/// `document`, in the gzip format [`METADATA_CONTENT_ENCODING`] names, or `None` when the
/// codec could not write it.
///
/// The compression is DETERMINISTIC: the same document compresses to the same bytes every
/// time, so two reads of one version of an image set serve the blob byte for byte alike.
fn gzip(document: &[u8]) -> Option<Vec<u8>> {
    use ::std::io::Write;

    let mut encoder =
        ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    encoder.write_all(document).ok()?;
    encoder.finish().ok()
}

/// The bytes of the aggregated metadata document a `GetImageSetMetadata` blob carries,
/// decoded from the compression [`METADATA_CONTENT_ENCODING`] names — the inverse of
/// [`gzip`], and `None` when the blob is not in that format.
///
/// Kept beside the compression it undoes, so the blob's encoding is stated in ONE place:
/// a reader of this response — a client of this crate, or a suite asserting what the wire
/// carried — decodes it the way the handler wrote it rather than re-deriving the format
/// from the header.
pub fn decoded_metadata_blob(blob: &[u8]) -> Option<Vec<u8>> {
    use ::std::io::Read;

    let mut document = Vec::new();
    ::flate2::read::GzDecoder::new(blob)
        .read_to_end(&mut document)
        .ok()?;
    Some(document)
}

/// The whole aggregated metadata document `attributes` describe for `image_set`, as it
/// stands in `datastore_id`.
///
/// Every value in it is read from state: the two identifiers name the records the document
/// was built from, and the attribute objects carry what has been stored for the version
/// being read and nothing besides — which is why the attributes are passed IN rather than
/// taken off the image set, so reading one version and reading another serve each one's own
/// content. The structure built here is what the document reports — the attributes stored
/// are merged INTO it ([`merge_stored_attributes`]), so nothing a caller stored can report
/// another identity for it or hide a level of it.
fn metadata_document(
    datastore_id: &str,
    image_set: &ImageSet,
    attributes: &ImageSetAttributes,
) -> Value {
    let tags = &attributes.dicom_tags;
    let mut document = ::serde_json::json!({
        METADATA_SCHEMA_VERSION_KEY: METADATA_SCHEMA_VERSION,
        METADATA_DATASTORE_ID_KEY: datastore_id,
        METADATA_IMAGE_SET_ID_KEY: image_set.image_set_id,
        METADATA_PATIENT_KEY: { METADATA_DICOM_KEY: patient_attributes(tags) },
        METADATA_STUDY_KEY: {
            METADATA_DICOM_KEY: study_attributes(tags),
            METADATA_SERIES_KEY: series(tags, image_set),
        },
    });
    merge_stored_attributes(&mut document, &attributes.extra_attributes);
    document
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

        // `version` names ONE version of the image set, and the attributes aggregated
        // below are the ones THAT version holds: the latest version's are the image set's
        // own, an earlier version's the snapshot taken when it was superseded. A version
        // this image set does not hold holds no attributes to read, so it is absent rather
        // than invalid; naming no version reads the latest.
        let attributes = match &input.version_id {
            Some(version_id) => image_set.attributes_of_version(version_id).ok_or_else(|| {
                GetImageSetMetadataErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::IMAGE_SET_VERSION_NOT_FOUND_MESSAGE.to_string(),
                    },
                )
            })?,
            None => image_set.attributes(),
        };

        let document = metadata_document(&input.datastore_id, image_set, &attributes);
        let serialized = ::serde_json::to_vec(&document).map_err(|_| {
            GetImageSetMetadataErrorModeled::InternalServerException(InternalServerException {
                message: "the image set metadata document could not be written".to_string(),
            })
        })?;
        let compressed = gzip(&serialized).ok_or_else(|| {
            GetImageSetMetadataErrorModeled::InternalServerException(InternalServerException {
                message: "the image set metadata document could not be compressed".to_string(),
            })
        })?;

        let output = GetImageSetMetadataOutput {
            image_set_metadata_blob: compressed,
            content_type: Some(METADATA_CONTENT_TYPE.to_string()),
            // The blob really is in that compression format: it was compressed just above
            // ([`METADATA_CONTENT_ENCODING`]).
            content_encoding: Some(METADATA_CONTENT_ENCODING.to_string()),
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
