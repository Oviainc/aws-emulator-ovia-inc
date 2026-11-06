// SPDX-License-Identifier: CC0-1.0

//! @op UpdateImageSetMetadata
//!
//! Writes the DICOM metadata of one persisted image set — a new VERSION of it — and
//! answers with the image set as the update leaves it.
//!
//! What the operation does, member by member:
//!
//! * `latestVersionId` is an optimistic-concurrency check, exactly as it is for
//!   `CopyImageSet`: a value that is not the image set's current latest version means the
//!   caller's view is stale, which is the modelled `ConflictException` (409).
//! * `updateImageSetMetadataUpdates` is a union, so a request asks for ONE of two things
//!   ([`MetadataUpdates`]):
//!   * `DICOMUpdates` carries the attributes to write (`updatableAttributes`) and the
//!     attributes to remove (`removableAttributes`), each documented as "the DICOM tags"
//!     — a document written in the shape of the aggregated metadata document
//!     `GetImageSetMetadata` serves, so each attribute in it is identified by the path it
//!     sits at (`Patient` → `DICOM` → keyword, `Study` → `Series` → series UID → …). A
//!     blob that is not a JSON object names no attribute and is refused with
//!     `ValidationException` (400); the value is never echoed, which the model's
//!     `@sensitive` on both members requires. The writes are applied first and the
//!     removals after, so a document naming one attribute in both ends with it removed.
//!   * `revertToVersionId` names "the previous image set version ID to revert the current
//!     image set back to": the attributes that version held
//!     ([`crate::state::ImageSet::attributes_of_version`]) become the image set's again. A
//!     version the image set does not hold is absent (404); the image set's OWN latest
//!     version is not a previous one and is refused with `ValidationException` (400).
//! * an attribute the model NAMES (`DICOMTags`, sixteen members) is written to that
//!   member, so what `SearchImageSets` filters on and what the metadata document reports
//!   never disagree. Only a value that member can legally carry is written: the document
//!   arrives in a blob the boundary validator does not walk, so every constraint the model
//!   declares on the member it is destined for — its kind, its `@length`, its `@pattern`,
//!   its `@range` ([`ModelledAttribute::unmet_requirement`]) — is applied here instead, and
//!   a value that fails one is refused with `ValidationException` (400) rather than
//!   persisted. Every OTHER attribute — DICOM defines thousands — is persisted at the
//!   path it was written at ([`crate::state::ImageSetAttributes::extra_attributes`]) and
//!   reported from there, rather than accepted and dropped. The model names the attributes
//!   of ONE series per image set, the series its `dicomSeriesInstanceUID` identifies
//!   ([`described_series`]), so the attributes of any OTHER series a document names — and
//!   of every series it names while no identifier says which series the image set holds —
//!   are among those others; and an identifier that leaves takes the attributes of the
//!   series it identified to the path of that series with it, so no stored attribute is
//!   left where the document cannot report it ([`settle_modelled_series`]). What the
//!   document says about ITSELF rather than about an image set ([`METADATA_COMPUTED_KEYS`])
//!   names no attribute at all, so an update carrying one stores nothing for it.
//! * `force` is what the model says it is: the flag that admits a change to
//!   `Tag.StudyInstanceUID`, `Tag.SeriesInstanceUID`, `Tag.SOPInstanceUID` and
//!   `Tag.StudyID`, and to a PRIVATE attribute of an individual SOP instance — a DICOM
//!   attribute whose group number is odd, which the document carries under its tag rather
//!   than under a keyword. Without the flag such a change is refused with
//!   `ValidationException` (400); with it, the new version records `Overrides.forced`,
//!   which is what `GetImageSet` reads back.
//! * `includeStudyImageSets` applies the same updates "to all image sets in the same Study
//!   as the requested image set ID": every other image set the data store still HOLDS that
//!   carries the requested image set's `StudyInstanceUID` takes the same document and comes
//!   back `UPDATING_FOR_STUDY_CONSISTENCY`. All of them must be able to take the update,
//!   since a study left half updated is the inconsistent state `ConflictException` reports.
//!   A RETIRED image set is not one of them: a tombstone takes part in no further work, so
//!   it neither takes the update nor blocks it. An
//!   image set holding no `StudyInstanceUID` is in no study, so the flag reaches nothing
//!   else; a REVERT names a version of one image set's own history, which names nothing in
//!   another image set, so the two members cannot be combined.
//!
//! The image set comes back `LOCKED` / `UPDATING` at its NEW version, and the CLOCK, not a
//! later reader, settles it to `ACTIVE` / `UPDATED` after
//! [`crate::state::IMAGE_SET_UPDATE_DELAY`]. Nothing here sleeps. The attributes are
//! written when the update is admitted, so the version the answer names is serving them
//! from that instant — and writing content into an image set is an ACCESS of it, which is
//! what `lastAccessedAt` reports.
//!
//! The DATA STORE must have settled into `ACTIVE` too, and the image set must be `ACTIVE`
//! with nothing else in flight: an update admitted beside another action would leave the
//! two disagreeing about the image set ([`crate::state::IMAGE_SET_NOT_UPDATABLE_MESSAGE`]).
//!
//! What one caller can accumulate here is bounded at both ends: the model caps each
//! document at 30,000 bytes, the parser caps how deeply one may nest (which is what bounds
//! every walk below, none of which recurses past a document it has already parsed), and
//! the attributes one image set may hold outside the modelled ones are capped by
//! [`crate::state::MAX_IMAGE_SET_EXTRA_ATTRIBUTES`] — past which the update is refused
//! with `ServiceQuotaExceededException` (402), like every other bound this crate keeps.
//!
//! ---
//!
//! # Modeled contract for `UpdateImageSetMetadata`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Update image set metadata attributes.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `force` (Boolean) — Setting this flag will force the UpdateImageSetMetadata operation
//!   for the following attributes: Tag.StudyInstanceUID, Tag.SeriesInstanceUID,
//!   Tag.SOPInstanceUID, and Tag.StudyID Adding, removing, or updating private tags for an
//!   individual SOP Instance
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//! * `includeStudyImageSets` (Boolean) — Flag to apply the metadata updates to all image sets
//!   in the same Study as the requested image set ID.
//! * `latestVersionId` (ImageSetExternalVersionId, pattern ^\d+$) **required** — The latest
//!   image set version identifier.
//! * `updateImageSetMetadataUpdates` (MetadataUpdates) **required** — Update image set
//!   metadata updates.
//!
//! ## Output
//!
//! * `createdAt` (Date) — The timestamp when image set metadata was created.
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//! * `imageSetState` (ImageSetState, one of: ACTIVE | DELETED | LOCKED) **required** — The
//!   image set state.
//! * `imageSetWorkflowStatus` (ImageSetWorkflowStatus, one of: COPIED | COPYING |
//!   COPYING_WITH_READ_ONLY_ACCESS | COPY_FAILED | CREATED | DELETED | DELETING | IMPORTED |
//!   IMPORTING | IMPORT_FAILED | UPDATED | UPDATE_FAILED | UPDATING |
//!   UPDATING_FOR_STUDY_CONSISTENCY) — The image set workflow status.
//! * `latestVersionId` (ImageSetExternalVersionId, pattern ^\d+$) **required** — The latest
//!   image set version identifier.
//! * `message` (Message, len 1..2048, pattern ^[\w -:`]+$) — The error message thrown if an
//!   update image set metadata action fails.
//! * `updatedAt` (Date) — The timestamp when image set metadata was updated.
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

use ::serde_json::{Map, Value};
use ::smithy_server_runtime::ViolationKind;

use crate::state::{
    ImageSet, ImageSetAttributes, ImageSetState, ImageSetWorkflowStatus,
    PendingImageSetTransition, METADATA_COMPUTED_KEYS,
};

/// The validation path a refusal about the attributes to WRITE names.
const UPDATABLE_ATTRIBUTES_PATH: &str =
    "updateImageSetMetadataUpdates.DICOMUpdates.updatableAttributes";

/// The validation path a refusal about the attributes to REMOVE names.
const REMOVABLE_ATTRIBUTES_PATH: &str =
    "updateImageSetMetadataUpdates.DICOMUpdates.removableAttributes";

/// The validation path a refusal about the pair of them together names.
const DICOM_UPDATES_PATH: &str = "updateImageSetMetadataUpdates.DICOMUpdates";

/// The validation path a refusal about the version to revert to names.
const REVERT_TO_VERSION_PATH: &str = "updateImageSetMetadataUpdates.revertToVersionId";

/// The validation path a refusal about the study-wide flag names.
const INCLUDE_STUDY_IMAGE_SETS_PATH: &str = "includeStudyImageSets";

/// The patient level of the aggregated metadata document.
const PATIENT_KEY: &str = crate::state::METADATA_PATIENT_KEY;

/// The study level of the aggregated metadata document.
const STUDY_KEY: &str = crate::state::METADATA_STUDY_KEY;

/// The map of series inside the study level, keyed by Series Instance UID.
const SERIES_KEY: &str = crate::state::METADATA_SERIES_KEY;

/// The map of SOP instances inside one series, keyed by SOP Instance UID.
const INSTANCES_KEY: &str = crate::state::METADATA_INSTANCES_KEY;

/// The object holding one level's DICOM attributes, keyed by attribute keyword.
const DICOM_KEY: &str = crate::state::METADATA_DICOM_KEY;

/// The attribute keywords the `force` flag exists for, from that member's own
/// documentation: "Tag.StudyInstanceUID, Tag.SeriesInstanceUID, Tag.SOPInstanceUID, and
/// Tag.StudyID". Each IDENTIFIES the place the content occupies in the DICOM hierarchy, so
/// changing one re-homes the image set rather than describing it.
const FORCE_GUARDED_KEYWORDS: [&str; 4] = [
    "StudyInstanceUID",
    "SeriesInstanceUID",
    "SOPInstanceUID",
    "StudyID",
];

/// How many hexadecimal digits the document spells a DICOM tag with: the four of the
/// group number followed by the four of the element number.
const TAG_HEX_DIGITS: usize = 8;

/// How many of those digits are the GROUP number — the leading half of the tag, which is
/// what says whether the attribute is a private one.
const TAG_GROUP_HEX_DIGITS: usize = 4;

/// One attribute an update document names: where it sits in the aggregated metadata
/// document, and what the document holds there.
#[derive(Debug, Clone)]
struct DocumentAttribute {
    /// The names of the levels that lead to the attribute, the attribute's own last.
    path: Vec<String>,
    value: Value,
}

/// What one modelled attribute holds in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttributeKind {
    Text,
    Integer,
}

impl AttributeKind {
    /// How a refusal names this kind.
    fn described(self) -> &'static str {
        match self {
            Self::Text => "a string",
            Self::Integer => "an integer",
        }
    }
}

/// The `@range` MAXIMUM the model declares on both study counts —
/// `DICOMNumberOfStudyRelatedSeries` and `DICOMNumberOfStudyRelatedInstances`
/// (`smithy_query(query="shape",shape="DICOMTags")`), each `@range(min: 0, max: 1000000)`.
const MAX_STUDY_RELATED_COUNT: i64 = 1_000_000;

/// The `@range` `DICOMSeriesNumber` declares
/// (`smithy_query(query="shape",shape="DICOMTags")`:
/// `@range(min: -2147483648, max: 2147483647)`) — the whole of the 32-bit integer the
/// model types every numeric member of the shape at, so it bounds any of them that
/// declares no narrower range of its own.
const INTEGER_MEMBER_RANGE: (i64, i64) = (i32::MIN as i64, i32::MAX as i64);

/// The `@pattern` a modelled attribute's member declares. The model declares ONE
/// expression across the members that have one, so it is named rather than compiled: a
/// variant per expression, with the text to quote in a refusal and the test to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttributePattern {
    /// What a DICOM Unique Identifier is: the pattern both the Study and the Series
    /// Instance UID members declare.
    DicomUid,
}

impl AttributePattern {
    /// The expression itself, copied verbatim from the model
    /// (`smithy_query(query="shape",shape="DICOMTags")`) — the text a refusal quotes.
    fn declared(self) -> &'static str {
        match self {
            Self::DicomUid => "^[0-9.]+$",
        }
    }

    /// Whether `text` satisfies it.
    ///
    /// A character class is a MEMBERSHIP test rather than a pattern language, so the class
    /// and the `+` quantifier between the `^` and the `$` are applied character by
    /// character — the way this service applies the pattern on a tag key
    /// (`crate::state::tag_key_matches_pattern`): the check then costs one pass over the
    /// value with no regex engine on a path that reads request text.
    fn matches(self, text: &str) -> bool {
        match self {
            Self::DicomUid => {
                !text.is_empty() && text.chars().all(|digit| digit.is_ascii_digit() || digit == '.')
            }
        }
    }
}

/// One attribute the MODEL names: the `DICOMTags` member the document's keyword at that
/// level maps to.
///
/// The members are the model's own
/// (`smithy_query(query="shape",shape="DICOMTags")`), each under the DICOM keyword the
/// aggregated metadata document spells it with — the keyword
/// `crate::ops::get_image_set_metadata` reports it under, so one attribute is written and
/// read under one name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelledAttribute {
    PatientId,
    PatientName,
    PatientBirthDate,
    PatientSex,
    StudyInstanceUid,
    StudyId,
    StudyDescription,
    StudyDate,
    StudyTime,
    AccessionNumber,
    StudyRelatedSeries,
    StudyRelatedInstances,
    SeriesInstanceUid,
    SeriesModality,
    SeriesBodyPart,
    SeriesNumber,
}

impl ModelledAttribute {
    /// What this attribute holds in the document: the two study counts and the series
    /// number are numbers, every other modelled attribute is text.
    fn kind(self) -> AttributeKind {
        match self {
            Self::StudyRelatedSeries | Self::StudyRelatedInstances | Self::SeriesNumber => {
                AttributeKind::Integer
            }
            _ => AttributeKind::Text,
        }
    }

    /// Whether `value` is one this attribute's modelled member can legally carry: of the
    /// kind the member holds AND within every constraint the model declares on it
    /// ([`ModelledAttribute::unmet_requirement`]).
    fn admits(self, value: &Value) -> bool {
        self.unmet_requirement(value).is_none()
    }

    /// The `@length` MAXIMUM the model declares on this attribute's member, or `None` for
    /// one holding a number (`smithy_query(query="shape",shape="DICOMTags")`).
    ///
    /// Each text member's `@length` minimum is 0, which every string satisfies, so the
    /// maximum is the whole of the bound there is to apply.
    fn max_length(self) -> Option<usize> {
        match self {
            Self::PatientId
            | Self::PatientName
            | Self::StudyInstanceUid
            | Self::StudyId
            | Self::StudyDescription
            | Self::AccessionNumber
            | Self::SeriesInstanceUid => Some(512),
            Self::PatientBirthDate | Self::StudyDate => Some(36),
            Self::StudyTime => Some(56),
            Self::PatientSex => Some(16),
            Self::SeriesModality => Some(32),
            Self::SeriesBodyPart => Some(128),
            Self::StudyRelatedSeries | Self::StudyRelatedInstances | Self::SeriesNumber => None,
        }
    }

    /// The `@pattern` the model declares on this attribute's member, or `None` when it
    /// declares none (`smithy_query(query="shape",shape="DICOMTags")`): the two UID members
    /// are the ones that carry one.
    fn pattern(self) -> Option<AttributePattern> {
        match self {
            Self::StudyInstanceUid | Self::SeriesInstanceUid => Some(AttributePattern::DicomUid),
            _ => None,
        }
    }

    /// The `@range` this attribute's member carries, as the inclusive pair of bounds
    /// (`smithy_query(query="shape",shape="DICOMTags")`): the two study counts are bounded
    /// at a million, and every other numeric member carries
    /// [`INTEGER_MEMBER_RANGE`] — the `@range` the series number declares, which is also
    /// the integer width the model types the shape's numeric members at, so a numeric
    /// member declaring no `@range` of its own is still bounded by what it can hold.
    ///
    /// Read only for a member [`ModelledAttribute::kind`] says holds a number.
    fn range(self) -> (i64, i64) {
        match self {
            Self::StudyRelatedSeries | Self::StudyRelatedInstances => (0, MAX_STUDY_RELATED_COUNT),
            _ => INTEGER_MEMBER_RANGE,
        }
    }

    /// The requirement `value` FAILS as this attribute's modelled member, or `None` when
    /// it satisfies every one of them.
    ///
    /// The member is a constrained shape, so a document may only write a value the member
    /// can carry: of the kind it holds, then every constraint the model declares on it.
    fn unmet_requirement(self, value: &Value) -> Option<String> {
        match self.kind() {
            AttributeKind::Integer => self.unmet_number_requirement(value),
            AttributeKind::Text => self.unmet_text_requirement(value),
        }
    }

    /// [`ModelledAttribute::unmet_requirement`] for a member holding a NUMBER: a JSON
    /// integer, inside the member's `@range`.
    ///
    /// The requirement is worded by the framework
    /// ([`crate::state::constraint_requirement`]), so a refusal stated here reads like the
    /// one the boundary states for the same constraint on a member it does walk.
    fn unmet_number_requirement(self, value: &Value) -> Option<String> {
        let Some(number) = value.as_i64() else {
            return Some(self.kind_requirement());
        };
        let (min, max) = self.range();
        let violated = if number < min {
            Some(ViolationKind::MinValue(min.to_string()))
        } else if number > max {
            Some(ViolationKind::MaxValue(max.to_string()))
        } else {
            None
        };
        violated.map(crate::state::constraint_requirement)
    }

    /// [`ModelledAttribute::unmet_requirement`] for a member holding TEXT: a JSON string,
    /// inside the member's `@length` and matching its `@pattern`.
    ///
    /// The two constraints are checked in the order the boundary validator's own leaf walk
    /// runs the traits — the length bound, then the pattern — so a value breaking both is
    /// refused the way the boundary would have refused it.
    fn unmet_text_requirement(self, value: &Value) -> Option<String> {
        let Some(text) = value.as_str() else {
            return Some(self.kind_requirement());
        };
        // `@length` on a string counts CHARACTERS, which is how the boundary validator
        // counts the same member's length.
        let length = text.chars().count();
        let over_long = self.max_length().filter(|max| length > *max);
        let violated = match (over_long, self.pattern()) {
            (Some(max), _) => Some(ViolationKind::MaxLength(max as i64)),
            (None, Some(pattern)) if !pattern.matches(text) => {
                Some(ViolationKind::Pattern(pattern.declared().to_string()))
            }
            _ => None,
        };
        violated.map(crate::state::constraint_requirement)
    }

    /// The requirement a value of the WRONG KIND for this attribute fails — the one
    /// requirement no modelled constraint states, since the document carries a value's kind
    /// where a member's schema states it.
    fn kind_requirement(self) -> String {
        format!("Member must hold {}", self.kind().described())
    }

    /// What this attribute's modelled member HOLDS, spelled as the aggregated metadata
    /// document spells the value, or `None` when nothing is stored there — the read that
    /// pairs with [`ModelledAttribute::write`].
    ///
    /// The model types the two study counts as plain numbers rather than optional ones, so
    /// each of them always holds one: zero is what an image set nothing has stored a count
    /// for holds, which is what the document reports for it.
    fn stored(self, tags: &DICOMTags) -> Option<Value> {
        let text = |held: &Option<String>| held.clone().map(Value::String);
        match self {
            Self::PatientId => text(&tags.dicom_patient_id),
            Self::PatientName => text(&tags.dicom_patient_name),
            Self::PatientBirthDate => text(&tags.dicom_patient_birth_date),
            Self::PatientSex => text(&tags.dicom_patient_sex),
            Self::StudyInstanceUid => text(&tags.dicom_study_instance_uid),
            Self::StudyId => text(&tags.dicom_study_id),
            Self::StudyDescription => text(&tags.dicom_study_description),
            Self::StudyDate => text(&tags.dicom_study_date),
            Self::StudyTime => text(&tags.dicom_study_time),
            Self::AccessionNumber => text(&tags.dicom_accession_number),
            Self::StudyRelatedSeries => {
                Some(Value::from(tags.dicom_number_of_study_related_series))
            }
            Self::StudyRelatedInstances => {
                Some(Value::from(tags.dicom_number_of_study_related_instances))
            }
            Self::SeriesInstanceUid => text(&tags.dicom_series_instance_uid),
            Self::SeriesModality => text(&tags.dicom_series_modality),
            Self::SeriesBodyPart => text(&tags.dicom_series_body_part),
            Self::SeriesNumber => tags.dicom_series_number.map(Value::from),
        }
    }

    /// Write `value` into this attribute's modelled member, or REMOVE what it held when
    /// `value` is `None`.
    ///
    /// A removed count reads back as zero rather than as absent: the model types the two
    /// study counts as plain numbers, so zero is what an image set nothing has stored a
    /// count for reports.
    fn write(self, tags: &mut DICOMTags, value: Option<&Value>) {
        let text = || value.and_then(Value::as_str).map(str::to_string);
        let number = || {
            value
                .and_then(Value::as_i64)
                .and_then(|number| i32::try_from(number).ok())
        };
        match self {
            Self::PatientId => tags.dicom_patient_id = text(),
            Self::PatientName => tags.dicom_patient_name = text(),
            Self::PatientBirthDate => tags.dicom_patient_birth_date = text(),
            Self::PatientSex => tags.dicom_patient_sex = text(),
            Self::StudyInstanceUid => tags.dicom_study_instance_uid = text(),
            Self::StudyId => tags.dicom_study_id = text(),
            Self::StudyDescription => tags.dicom_study_description = text(),
            Self::StudyDate => tags.dicom_study_date = text(),
            Self::StudyTime => tags.dicom_study_time = text(),
            Self::AccessionNumber => tags.dicom_accession_number = text(),
            Self::StudyRelatedSeries => {
                tags.dicom_number_of_study_related_series = number().unwrap_or_default();
            }
            Self::StudyRelatedInstances => {
                tags.dicom_number_of_study_related_instances = number().unwrap_or_default();
            }
            Self::SeriesInstanceUid => tags.dicom_series_instance_uid = text(),
            Self::SeriesModality => tags.dicom_series_modality = text(),
            Self::SeriesBodyPart => tags.dicom_series_body_part = text(),
            Self::SeriesNumber => tags.dicom_series_number = number(),
        }
    }
}

/// The modelled attribute the document path `path` names, or `None` when the model names
/// no attribute there — an attribute keyword it does not declare, a level it does not
/// describe (one SOP instance's attributes), or the attributes of a series the modelled
/// members do not describe.
///
/// `series_instance_uid` is the series the modelled members describe ([`described_series`]):
/// the model declares the attributes of ONE series per image set, identified by that one
/// `dicomSeriesInstanceUID`, so a path naming any OTHER series — and every series path at
/// all while the members describe none — names content the modelled members cannot hold and
/// is kept as written.
fn modelled_attribute(path: &[String], series_instance_uid: Option<&str>) -> Option<ModelledAttribute> {
    let keyword = path.last()?.as_str();
    let level: Vec<&str> = path.iter().map(String::as_str).collect();
    match level.as_slice() {
        [PATIENT_KEY, DICOM_KEY, _] => match keyword {
            "PatientID" => Some(ModelledAttribute::PatientId),
            "PatientName" => Some(ModelledAttribute::PatientName),
            "PatientBirthDate" => Some(ModelledAttribute::PatientBirthDate),
            "PatientSex" => Some(ModelledAttribute::PatientSex),
            _ => None,
        },
        [STUDY_KEY, DICOM_KEY, _] => match keyword {
            "StudyInstanceUID" => Some(ModelledAttribute::StudyInstanceUid),
            "StudyID" => Some(ModelledAttribute::StudyId),
            "StudyDescription" => Some(ModelledAttribute::StudyDescription),
            "StudyDate" => Some(ModelledAttribute::StudyDate),
            "StudyTime" => Some(ModelledAttribute::StudyTime),
            "AccessionNumber" => Some(ModelledAttribute::AccessionNumber),
            "NumberOfStudyRelatedSeries" => Some(ModelledAttribute::StudyRelatedSeries),
            "NumberOfStudyRelatedInstances" => Some(ModelledAttribute::StudyRelatedInstances),
            _ => None,
        },
        [STUDY_KEY, SERIES_KEY, series, DICOM_KEY, _]
            if series_instance_uid.is_some_and(|described| described == *series) =>
        {
            match keyword {
                SERIES_INSTANCE_UID_KEYWORD => Some(ModelledAttribute::SeriesInstanceUid),
                "Modality" => Some(ModelledAttribute::SeriesModality),
                "BodyPartExamined" => Some(ModelledAttribute::SeriesBodyPart),
                "SeriesNumber" => Some(ModelledAttribute::SeriesNumber),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The attributes OF one series the model names, under the keyword the aggregated metadata
/// document spells each with.
///
/// The three that DESCRIBE the series the modelled `dicomSeriesInstanceUID` identifies
/// (`smithy_query(query="shape",shape="DICOMTags")`), rather than identifying it: they are
/// the members that move with the series they belong to.
const SERIES_ATTRIBUTES: [(&str, ModelledAttribute); 3] = [
    ("Modality", ModelledAttribute::SeriesModality),
    ("BodyPartExamined", ModelledAttribute::SeriesBodyPart),
    ("SeriesNumber", ModelledAttribute::SeriesNumber),
];

/// The keyword the document spells the identifier of a series with — the attribute that
/// says WHICH series a series level of the document is.
const SERIES_INSTANCE_UID_KEYWORD: &str = "SeriesInstanceUID";

/// The path the aggregated metadata document holds one attribute of the series
/// `series_instance_uid` at: the series levels are keyed BY the Series Instance UID.
fn series_path(series_instance_uid: &str, keyword: &str) -> Vec<String> {
    [STUDY_KEY, SERIES_KEY, series_instance_uid, DICOM_KEY, keyword]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// What `attributes` holds at `path`, or `None` when it holds nothing there.
fn value_at<'a>(attributes: &'a Map<String, Value>, path: &[String]) -> Option<&'a Value> {
    let (name, rest) = path.split_first()?;
    let held = attributes.get(name)?;
    if rest.is_empty() {
        return Some(held);
    }
    value_at(held.as_object()?, rest)
}

/// The series the modelled members describe while `document` is applied: the series the
/// image set HOLDS, or — for an image set holding none — the series `document` itself
/// identifies by writing that series' own `SeriesInstanceUID`.
///
/// The model declares the attributes of ONE series per image set, identified by that
/// `dicomSeriesInstanceUID`, so a document naming SEVERAL series identifies none of them,
/// and neither does one whose identifier disagrees with the series level it sits under:
/// what the modelled members cannot describe is kept as written instead. A document of
/// attributes to REMOVE identifies nothing either — it takes attributes away rather than
/// saying which series the image set holds.
fn described_series(
    attributes: &ImageSetAttributes,
    document: &Map<String, Value>,
    removing: bool,
) -> Option<String> {
    if let Some(held) = attributes.dicom_tags.dicom_series_instance_uid.clone() {
        return Some(held);
    }
    if removing {
        return None;
    }
    let mut identified: Option<String> = None;
    for DocumentAttribute { path, value } in attributes_of(document) {
        let level: Vec<&str> = path.iter().map(String::as_str).collect();
        let [STUDY_KEY, SERIES_KEY, series, DICOM_KEY, SERIES_INSTANCE_UID_KEYWORD] =
            level.as_slice()
        else {
            continue;
        };
        if value.as_str() != Some(*series)
            || identified
                .as_deref()
                .is_some_and(|already| already != *series)
        {
            return None;
        }
        identified = Some((*series).to_string());
    }
    identified
}

/// Keep the modelled members and the attributes kept as written describing ONE series
/// each, once a document has been applied — `described` being the series the modelled
/// members described while it was ([`described_series`]).
///
/// Those members describe the series their `dicomSeriesInstanceUID` identifies, so: once
/// the image set holds that identifier, the attributes of that series belong in the
/// modelled members, and one of them kept as written moves in; once it holds none, the
/// members describe no series, so what they still hold is kept at the path of the series
/// they described, which is where the metadata document reports it. Either way one
/// attribute is held in ONE place, and a search and the metadata document report the same
/// value for it.
fn settle_modelled_series(attributes: &mut ImageSetAttributes, described: Option<&str>) {
    if let Some(held) = attributes.dicom_tags.dicom_series_instance_uid.clone() {
        for (keyword, modelled) in SERIES_ATTRIBUTES {
            let at = series_path(&held, keyword);
            let Some(value) = value_at(&attributes.extra_attributes, &at).cloned() else {
                continue;
            };
            // A value the modelled member cannot legally carry — of another kind than the
            // model types the member as, or outside the constraints it declares — stays
            // where it was written: the member carries what the model says it carries, and
            // nothing is dropped for it.
            if !modelled.admits(&value) {
                continue;
            }
            modelled.write(&mut attributes.dicom_tags, Some(&value));
            remove_at(&mut attributes.extra_attributes, &at);
        }
        return;
    }
    let Some(described) = described else {
        return;
    };
    for (keyword, modelled) in SERIES_ATTRIBUTES {
        let Some(value) = modelled.stored(&attributes.dicom_tags) else {
            continue;
        };
        modelled.write(&mut attributes.dicom_tags, None);
        write_at(
            &mut attributes.extra_attributes,
            &series_path(described, keyword),
            value,
        );
    }
}

/// Whether `key` names a PRIVATE DICOM attribute: the document spells such an attribute
/// with its tag — the four hexadecimal digits of the group number followed by the four of
/// the element number — and DICOM reserves every ODD group number for private attributes.
fn is_private_tag(key: &str) -> bool {
    key.len() == TAG_HEX_DIGITS
        && key.chars().all(|digit| digit.is_ascii_hexdigit())
        && key
            .get(..TAG_GROUP_HEX_DIGITS)
            .and_then(|group| u16::from_str_radix(group, 16).ok())
            .is_some_and(|group| group % 2 == 1)
}

/// Whether changing the attribute at `path` needs the `force` flag: one of the four
/// keywords the member names, or a private attribute of an individual SOP instance —
/// which is the second case its documentation lists.
fn needs_force(path: &[String]) -> bool {
    let Some(keyword) = path.last() else {
        return false;
    };
    FORCE_GUARDED_KEYWORDS.contains(&keyword.as_str())
        || (path.iter().any(|level| level == INSTANCES_KEY) && is_private_tag(keyword))
}

/// Every attribute `document` names, in document order.
///
/// A level of the document is an object; anything else is an attribute's VALUE, so an
/// array (the image frames of one SOP instance) is one attribute rather than a level. What
/// the document says about ITSELF — its schema declaration and the identifiers naming the
/// records it describes ([`METADATA_COMPUTED_KEYS`]) — names no attribute and is skipped.
fn attributes_of(document: &Map<String, Value>) -> Vec<DocumentAttribute> {
    let mut found = Vec::new();
    let mut path = Vec::new();
    collect_attributes(document, &mut path, &mut found);
    found
}

/// The recursive half of [`attributes_of`], carrying the path walked so far.
fn collect_attributes(
    level: &Map<String, Value>,
    path: &mut Vec<String>,
    found: &mut Vec<DocumentAttribute>,
) {
    for (name, value) in level {
        if path.is_empty() && METADATA_COMPUTED_KEYS.contains(&name.as_str()) {
            continue;
        }
        path.push(name.clone());
        match value {
            Value::Object(nested) => collect_attributes(nested, path, found),
            _ => found.push(DocumentAttribute {
                path: path.clone(),
                value: value.clone(),
            }),
        }
        path.pop();
    }
}

/// Write `value` into `attributes` at `path`, creating every level the path names.
fn write_at(attributes: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((name, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        attributes.insert(name.clone(), value);
        return;
    }
    let level = attributes
        .entry(name.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !level.is_object() {
        // What the path names is a LEVEL, so an attribute previously written here is not
        // one: the levels the path names are what the document needs to carry it.
        *level = Value::Object(Map::new());
    }
    if let Some(nested) = level.as_object_mut() {
        write_at(nested, rest, value);
    }
}

/// Remove whatever `attributes` holds at `path`, and any level the removal leaves empty —
/// so a removed attribute is gone from the document rather than leaving an empty level
/// behind. Removing what is not there changes nothing.
fn remove_at(attributes: &mut Map<String, Value>, path: &[String]) {
    let Some((name, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        attributes.remove(name);
        return;
    }
    let Some(level) = attributes.get_mut(name).and_then(Value::as_object_mut) else {
        return;
    };
    remove_at(level, rest);
    if level.is_empty() {
        attributes.remove(name);
    }
}

/// What one request asks of every image set it reaches.
#[derive(Debug, Clone)]
enum UpdatePlan {
    /// The attributes to write and the attributes to remove, each the aggregated metadata
    /// document the caller supplied.
    Attributes {
        updatable: Option<Map<String, Value>>,
        removable: Option<Map<String, Value>>,
    },
    /// The version whose attributes the image set goes back to.
    Revert(String),
}

/// This operation's `ValidationException` (HTTP 400) for a requirement this handler checks
/// itself, worded through the one envelope every refusal of this service is worded
/// through. The rejected value is never echoed: what a refusal here is about is a document
/// of DICOM attributes, which the model marks `@sensitive`. A requirement may name the
/// ATTRIBUTE that broke the rule — one of the keywords the model or the `force` flag names,
/// or a DICOM tag, never free text a caller composed — exactly as every constraint refusal
/// of this service names the member that broke one.
fn refuse(path: &str, requirement: &str) -> UpdateImageSetMetadataError {
    UpdateImageSetMetadataErrorModeled::ValidationException(ValidationException {
        message: crate::state::handler_requirement_error(path, None, requirement),
    })
    .into()
}

/// The aggregated-metadata document one update blob carries, or `None` when the blob is
/// absent.
///
/// Both members are documented as "the DICOM tags" to update or remove, and a DICOM
/// metadata document is the object `GetImageSetMetadata` serves — so a blob that is not a
/// JSON object names no attribute at all and fails the constraint the service sets. The
/// rule itself is [`crate::state::metadata_document`], shared with every other member this
/// service documents as carrying such a document.
fn parse_document(
    blob: Option<&[u8]>,
    path: &str,
) -> Result<Option<Map<String, Value>>, UpdateImageSetMetadataError> {
    let Some(bytes) = blob else {
        return Ok(None);
    };
    crate::state::metadata_document(bytes)
        .map(Some)
        .map_err(|requirement| refuse(path, requirement))
}

/// Apply one update document to `attributes`: `removing` says whether the document names
/// the attributes to take away rather than the attributes to write.
///
/// An attribute the model names is written to its modelled member — and cleared out of the
/// attributes kept as written, so the one attribute is held in one place — and every other
/// attribute is written at the very path the document named it at.
fn apply_document(
    attributes: &mut ImageSetAttributes,
    document: &Map<String, Value>,
    removing: bool,
    path: &str,
) -> Result<(), UpdateImageSetMetadataError> {
    // The series the modelled members describe while THIS document is applied, decided
    // before any of it is: the image set holds the attributes of one series, so which
    // series that is cannot depend on the order the document's attributes are walked in.
    let described = described_series(attributes, document, removing);
    for DocumentAttribute { path: at, value } in attributes_of(document) {
        match modelled_attribute(&at, described.as_deref()) {
            Some(modelled) if removing => {
                modelled.write(&mut attributes.dicom_tags, None);
                remove_at(&mut attributes.extra_attributes, &at);
            }
            Some(modelled) => {
                if let Some(requirement) = modelled.unmet_requirement(&value) {
                    let keyword = at.last().cloned().unwrap_or_default();
                    return Err(refuse(path, &format!("{requirement} for {keyword}")));
                }
                modelled.write(&mut attributes.dicom_tags, Some(&value));
                remove_at(&mut attributes.extra_attributes, &at);
            }
            None if removing => remove_at(&mut attributes.extra_attributes, &at),
            None => write_at(&mut attributes.extra_attributes, &at, value),
        }
    }
    settle_modelled_series(attributes, described.as_deref());
    Ok(())
}

/// The attributes `image_set` holds once the documents of `plan` have been applied to it.
///
/// Only the attribute documents reach every image set an update touches; a revert is
/// resolved against the named image set alone, in the handler.
fn updated_attributes(
    image_set: &ImageSet,
    updatable: Option<&Map<String, Value>>,
    removable: Option<&Map<String, Value>>,
) -> Result<ImageSetAttributes, UpdateImageSetMetadataError> {
    let mut attributes = image_set.attributes();
    if let Some(document) = updatable {
        apply_document(
            &mut attributes,
            document,
            false,
            UPDATABLE_ATTRIBUTES_PATH,
        )?;
    }
    if let Some(document) = removable {
        apply_document(&mut attributes, document, true, REMOVABLE_ATTRIBUTES_PATH)?;
    }
    // What a caller may accumulate in one image set is bounded: each update persists the
    // attributes it named, and every version the image set keeps holds a snapshot of them
    // (`crate::state::MAX_IMAGE_SET_EXTRA_ATTRIBUTES`). Counted AFTER the removals, so an
    // update that replaces attributes rather than adding any is never refused for it —
    // through the same counter every writer of these attributes is bounded by.
    if attributes.written_attributes() > crate::state::MAX_IMAGE_SET_EXTRA_ATTRIBUTES {
        return Err(
            UpdateImageSetMetadataErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: crate::state::extra_attributes_quota_message(),
                },
            )
            .into(),
        );
    }
    Ok(attributes)
}

/// Whether `image_set` can take a metadata update right now: `ACTIVE`, with no action the
/// clock still owes it.
fn is_updatable(image_set: &ImageSet) -> bool {
    image_set.latest_version.image_set_state == ImageSetState::Active
        && image_set.pending.is_none()
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateImageSetMetadataError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateImageSetMetadataError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#UpdateImageSetMetadata")]
#[derive(Debug)]
pub enum UpdateImageSetMetadataErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_image_set_metadata::UpdateImageSetMetadataError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateImageSetMetadataError = crate::scaffolded::Scaffolded<UpdateImageSetMetadataErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateImageSetMetadataErrorModeled> for UpdateImageSetMetadataError {
    fn from(modeled: UpdateImageSetMetadataErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateImageSetMetadata` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateImageSetMetadata<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateImageSetMetadata<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateImageSetMetadata<Eng>
{
    type Input = UpdateImageSetMetadataInput;
    type Output = UpdateImageSetMetadataOutput;
    type Error = UpdateImageSetMetadataError;

    const NAME: &'static str = "UpdateImageSetMetadata";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_IMAGE_SET_METADATA_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_IMAGE_SET_METADATA_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();
        let forced = input.force.unwrap_or(false);
        let include_study_image_sets = input.include_study_image_sets.unwrap_or(false);

        // What the request ASKS, decided before any state is read: a document that names
        // no attribute, or a change the `force` flag exists for without that flag, is
        // refused whatever the service holds.
        let plan = match &input.update_image_set_metadata_updates {
            MetadataUpdates::DICOMUpdates(updates) => {
                let updatable = parse_document(
                    updates.updatable_attributes.as_deref(),
                    UPDATABLE_ATTRIBUTES_PATH,
                )?;
                let removable = parse_document(
                    updates.removable_attributes.as_deref(),
                    REMOVABLE_ATTRIBUTES_PATH,
                )?;
                if updatable.is_none() && removable.is_none() {
                    return Err(refuse(
                        DICOM_UPDATES_PATH,
                        "Member must carry updatableAttributes or removableAttributes",
                    ));
                }
                if !forced {
                    for (document, path) in [
                        (updatable.as_ref(), UPDATABLE_ATTRIBUTES_PATH),
                        (removable.as_ref(), REMOVABLE_ATTRIBUTES_PATH),
                    ] {
                        let Some(document) = document else {
                            continue;
                        };
                        for attribute in attributes_of(document) {
                            if needs_force(&attribute.path) {
                                let keyword = attribute.path.last().cloned().unwrap_or_default();
                                return Err(refuse(
                                    path,
                                    &format!(
                                        "Member must not change {keyword} unless the force \
                                         flag is set"
                                    ),
                                ));
                            }
                        }
                    }
                }
                UpdatePlan::Attributes {
                    updatable,
                    removable,
                }
            }
            MetadataUpdates::revertToVersionId(version_id) => {
                // A version identifier names a version of ONE image set's own history, so
                // it names nothing in the other image sets of the study: the two members
                // cannot describe one request.
                if include_study_image_sets {
                    return Err(refuse(
                        INCLUDE_STUDY_IMAGE_SETS_PATH,
                        "Member must not be set when reverting to a previous image set version",
                    ));
                }
                UpdatePlan::Revert(version_id.clone())
            }
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateImageSetMetadataErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        // Every identifier below has already satisfied its modelled `@pattern`
        // (`datastoreId` / `imageSetId` `^[0-9a-z]{32}$`, `latestVersionId` `^\d+$`), so
        // what is left to decide is whether the service HOLDS what the request names.
        //
        // Reached in the region this request was addressed to: a data store exists in ONE
        // region, so one held in another is not found here at all.
        let datastore = store
            .datastore_in(&region, &input.datastore_id)
            .ok_or_else(|| {
                UpdateImageSetMetadataErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        // Constant, and stated by resource KIND: the requested id is not
                        // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                        message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
                    },
                )
            })?;

        // An update writes into the data store, so the store has to have settled into
        // `ACTIVE` first: one still `CREATING`, or already `DELETING`, is refused with the
        // modelled `ConflictException` (HTTP 409) before any image set is looked up — the
        // same precondition `CopyImageSet` and `DeleteImageSet` apply.
        if !datastore.is_active() {
            return Err(UpdateImageSetMetadataErrorModeled::ConflictException(
                ConflictException {
                    message: crate::state::DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
                },
            )
            .into());
        }

        let image_set = datastore
            .image_sets
            .get(&input.image_set_id)
            .ok_or_else(|| {
                UpdateImageSetMetadataErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                    },
                )
            })?;

        // A deleted image set holds no metadata: its content went with the deletion, so
        // what is absent is the image set itself — the same answer `GetImageSetMetadata`
        // gives for the document it can no longer aggregate.
        if image_set.is_deleted() {
            return Err(UpdateImageSetMetadataErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                },
            )
            .into());
        }

        // The optimistic-concurrency check, before the state check: a caller whose view of
        // the image set is stale is told THAT, whatever the image set is doing now.
        if image_set.latest_version_id() != input.latest_version_id {
            return Err(UpdateImageSetMetadataErrorModeled::ConflictException(
                ConflictException {
                    message: crate::state::IMAGE_SET_VERSION_CONFLICT_MESSAGE.to_string(),
                },
            )
            .into());
        }
        if !is_updatable(image_set) {
            return Err(UpdateImageSetMetadataErrorModeled::ConflictException(
                ConflictException {
                    message: crate::state::IMAGE_SET_NOT_UPDATABLE_MESSAGE.to_string(),
                },
            )
            .into());
        }
        if image_set.versions().count() >= crate::state::MAX_IMAGE_SET_VERSIONS {
            return Err(
                UpdateImageSetMetadataErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "this emulator holds at most {} versions per image set",
                            crate::state::MAX_IMAGE_SET_VERSIONS
                        ),
                    },
                )
                .into(),
            );
        }

        // What each image set this request reaches will hold, computed before anything is
        // written: a 200 that had refused half of a study would leave the study in exactly
        // the state the refusal exists to prevent.
        let mut planned: Vec<(String, ImageSetAttributes, ImageSetWorkflowStatus)> = Vec::new();
        match &plan {
            UpdatePlan::Revert(version_id) => {
                let attributes = image_set
                    .attributes_of_version(version_id)
                    .ok_or_else(|| {
                        UpdateImageSetMetadataErrorModeled::ResourceNotFoundException(
                            ResourceNotFoundException {
                                message: crate::state::IMAGE_SET_VERSION_NOT_FOUND_MESSAGE
                                    .to_string(),
                            },
                        )
                    })?;
                // "the PREVIOUS image set version ID to revert the current image set back
                // to": the image set's own latest version is not a previous one.
                if *version_id == image_set.latest_version_id() {
                    return Err(refuse(
                        REVERT_TO_VERSION_PATH,
                        "Member must name a version earlier than the image set's latest version",
                    ));
                }
                planned.push((
                    image_set.image_set_id.clone(),
                    attributes,
                    ImageSetWorkflowStatus::Updating,
                ));
            }
            UpdatePlan::Attributes {
                updatable,
                removable,
            } => {
                planned.push((
                    image_set.image_set_id.clone(),
                    updated_attributes(image_set, updatable.as_ref(), removable.as_ref())?,
                    ImageSetWorkflowStatus::Updating,
                ));

                // The study the requested image set is in, as it stands going in. An image
                // set holding no `StudyInstanceUID` is in no study, so the flag reaches
                // nothing besides the image set the request named. The study is made up of
                // the image sets the data store still HOLDS: a retired one takes part in no
                // further work, so it neither takes this update nor blocks it
                // ([`crate::state::Datastore::held_image_sets`]).
                let study_instance_uid = image_set.dicom_tags.dicom_study_instance_uid.clone();
                if include_study_image_sets {
                    if let Some(study_instance_uid) = study_instance_uid {
                        let mut study: Vec<&ImageSet> = datastore
                            .held_image_sets()
                            .filter(|other| {
                                other.image_set_id != input.image_set_id
                                    && other.dicom_tags.dicom_study_instance_uid.as_deref()
                                        == Some(study_instance_uid.as_str())
                            })
                            .collect();
                        // One stable order, so a refusal about a study does not depend on
                        // how the map holding its image sets happens to iterate.
                        study.sort_by(|left, right| left.image_set_id.cmp(&right.image_set_id));
                        for other in study {
                            if !is_updatable(other) {
                                return Err(UpdateImageSetMetadataErrorModeled::ConflictException(
                                    ConflictException {
                                        message: crate::state::IMAGE_SET_NOT_UPDATABLE_MESSAGE
                                            .to_string(),
                                    },
                                )
                                .into());
                            }
                            if other.versions().count() >= crate::state::MAX_IMAGE_SET_VERSIONS {
                                return Err(
                                    UpdateImageSetMetadataErrorModeled::ServiceQuotaExceededException(
                                        ServiceQuotaExceededException {
                                            message: format!(
                                                "this emulator holds at most {} versions per image set",
                                                crate::state::MAX_IMAGE_SET_VERSIONS
                                            ),
                                        },
                                    )
                                    .into(),
                                );
                            }
                            planned.push((
                                other.image_set_id.clone(),
                                updated_attributes(other, updatable.as_ref(), removable.as_ref())?,
                                // The other image sets of the study are being brought into
                                // line with it, which is the status the model has for
                                // exactly that.
                                ImageSetWorkflowStatus::UpdatingForStudyConsistency,
                            ));
                        }
                    }
                }
            }
        }

        // Past here the request is accepted. Each image set takes a NEW version carrying
        // the attributes just computed, is locked while the update is in flight, and is
        // settled by the clock after `IMAGE_SET_UPDATE_DELAY` — nothing sleeps.
        let datastore = store
            .datastore_in_mut(&region, &input.datastore_id)
            .ok_or_else(|| {
                UpdateImageSetMetadataErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the data store could not be reached while updating the image \
                                  set metadata"
                            .to_string(),
                    },
                )
            })?;
        for (image_set_id, attributes, workflow_status) in planned {
            let Some(image_set) = datastore.image_sets.get_mut(&image_set_id) else {
                continue;
            };
            image_set.push_version(ImageSetState::Locked, workflow_status, forced, now);
            image_set.set_attributes(attributes);
            // Content was WRITTEN into the image set, which is an access of the content it
            // holds — the event `lastAccessedAt` reports.
            image_set.record_access(now);
            image_set.pending = Some(PendingImageSetTransition {
                due_at: now + crate::state::IMAGE_SET_UPDATE_DELAY,
                image_set_state: ImageSetState::Active,
                workflow_status: ImageSetWorkflowStatus::Updated,
                // An update writes metadata: the pixel data the image set holds is
                // untouched, so the settling transition delivers none.
                delivered_image_frames: None,
                // Nor any attributes: the update already wrote them, so the version this
                // call answers with is serving them from the instant it was admitted.
                delivered_attributes: None,
                retires_image_set: false,
            });
        }

        // Reported from the record just written, so the answer can never claim a version
        // or a state the store does not hold.
        let image_set = datastore
            .image_sets
            .get(&input.image_set_id)
            .ok_or_else(|| {
                UpdateImageSetMetadataErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the image set could not be reached while reporting the update"
                            .to_string(),
                    },
                )
            })?;

        Ok(UpdateImageSetMetadataOutput {
            datastore_id: input.datastore_id.clone(),
            image_set_id: image_set.image_set_id.clone(),
            latest_version_id: image_set.latest_version_id(),
            image_set_state: image_set.latest_version.image_set_state.as_wire().to_string(),
            image_set_workflow_status: Some(
                image_set
                    .latest_version
                    .workflow_status
                    .as_wire()
                    .to_string(),
            ),
            // `createdAt` is the image SET's creation, as `GetImageSet` reports it;
            // `updatedAt` is the new version's own instant.
            created_at: Some(crate::state::to_date_time(image_set.created_at)),
            updated_at: Some(crate::state::to_date_time(image_set.latest_version.updated_at)),
            // "The error message thrown if an update image set metadata action fails" —
            // carried by the version it was recorded on, and absent while nothing failed.
            message: image_set.latest_version.message.clone(),
        })
    }
}
