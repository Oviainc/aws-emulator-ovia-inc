// SPDX-License-Identifier: CC0-1.0

//! The preloaded Synthea FHIR dataset a data store comes up HOLDING when
//! `CreateFHIRDatastore` is asked to preload one.
//!
//! `CreateFHIRDatastoreRequest$PreloadDataConfig` is documented as "An optional
//! parameter to preload (import) open source Synthea FHIR data upon creation of the
//! data store", and `PreloadDataType` models exactly one value — `SYNTHEA`, "The type
//! of preloaded data. Only Synthea preloaded data is supported." So the member is an
//! EFFECT, not a label: a data store created with it is not an empty data store. The
//! records are written onto the data store's own record at the creation instant, and
//! its FHIR REST data plane ([`super::fhir_rest`]) serves them from the moment the
//! data store is `ACTIVE`, exactly as it serves a resource a caller created through
//! it — the same `read` interaction, the same stored bytes, the same byte budgets.
//!
//! # What the dataset is, and what it is not
//!
//! Synthea (<https://github.com/synthetichealth/synthea>) is an open source
//! SYNTHETIC patient generator: everything it emits is generated, and no part of its
//! output is shipped with this emulator. So this module GENERATES the preload, as a
//! bounded cohort of synthetic patient records in the shape Synthea's FHIR R4 output
//! takes — one `Patient` per record with the `Encounter` it was seen at, the
//! `Condition` recorded there and the vital-sign `Observation`s taken at it, every
//! child resource REFERENCING the patient and the encounter it belongs to.
//!
//! Neither AWS nor the model documents which records a preload delivers, and a real
//! Synthea run emits a different population for every seed, so the cohort's SIZE and
//! its content are this emulator's and are an example of preloaded data rather than a
//! claim about any particular population. What is faithful — and what a caller can
//! act on — is that a preloaded data store comes up holding readable FHIR R4
//! resources of the types a Synthea record is made of, and an un-preloaded one comes
//! up empty.
//!
//! # Where the content comes from
//!
//! The codes are the published terminologies FHIR R4 vital signs and conditions are
//! written in, cited on each constant: LOINC (<https://loinc.org>) for the
//! observations, SNOMED CT for the conditions and the encounter type, and the HL7
//! terminology code systems for the encounter class, the observation category and the
//! condition status. The names, places and measurements are synthetic, and each row
//! is spelled out in [`COHORT`] rather than drawn from a generator, so the dataset one
//! preloaded data store holds is the dataset the next one holds.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::state::{ComAmazonawsHealthlakeHealthlakeState, FhirResource};

/// The LOINC code system URI, as FHIR R4 names it for observation codes.
const LOINC: &str = "http://loinc.org";

/// The SNOMED CT code system URI, as FHIR R4 names it for clinical findings.
const SNOMED: &str = "http://snomed.info/sct";

/// The UCUM code system URI, which is what a FHIR `Quantity`'s `system` carries for
/// a unit of measure.
const UCUM: &str = "http://unitsofmeasure.org";

/// The HL7 `v3-ActCode` code system, the `Encounter.class` binding.
const ACT_CODE: &str = "http://terminology.hl7.org/CodeSystem/v3-ActCode";

/// The HL7 `v2-0203` identifier-type code system, which is where a medical record
/// number's `MR` type code comes from.
const IDENTIFIER_TYPE: &str = "http://terminology.hl7.org/CodeSystem/v2-0203";

/// The HL7 observation-category code system, the `Observation.category` binding.
const OBSERVATION_CATEGORY: &str = "http://terminology.hl7.org/CodeSystem/observation-category";

/// The HL7 condition clinical-status code system, the `Condition.clinicalStatus`
/// binding.
const CONDITION_CLINICAL: &str = "http://terminology.hl7.org/CodeSystem/condition-clinical";

/// The HL7 condition verification-status code system, the
/// `Condition.verificationStatus` binding.
const CONDITION_VERIFICATION: &str = "http://terminology.hl7.org/CodeSystem/condition-ver-status";

/// The system the synthetic medical record numbers below are issued under: the
/// generator the preloaded data is modelled on, so an identifier of a preloaded
/// record cannot be mistaken for one a real assigning authority issued.
const SYNTHEA_IDENTIFIER_SYSTEM: &str = "https://github.com/synthetichealth/synthea";

/// The SNOMED CT code of the encounter every preloaded record is seen at.
const ENCOUNTER_TYPE_CODE: &str = "162673000";

/// The display of [`ENCOUNTER_TYPE_CODE`].
const ENCOUNTER_TYPE_DISPLAY: &str = "General examination of patient (procedure)";

/// How far apart two preloaded records' encounters sit, so the cohort spans a stretch
/// of time rather than one instant and a reader can tell the records apart by when
/// they were seen. Content of the generated dataset, not a service behaviour.
const ENCOUNTER_SPACING: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a preloaded record's encounter lasted.
const ENCOUNTER_DURATION: Duration = Duration::from_secs(30 * 60);

/// How many logical ids a preloaded resource offers before the preload gives up.
///
/// The mint advances a counter on every call, so each candidate differs and one
/// attempt is enough in practice; the cap is what BOUNDS the walk, because the
/// preload runs while the per-service store lock is held — the same reason
/// [`super::fhir_rest`] bounds its own mint walk.
const MAX_ID_MINT_ATTEMPTS: usize = 8;

/// The FHIR resource types one preloaded record is made of: the patient, the
/// encounter, the condition recorded at it and the four vital-sign observations
/// taken at it.
const RESOURCES_PER_RECORD: usize = 7;

/// One synthetic patient record of the preloaded cohort.
///
/// Every field is content of the generated dataset. The codes are the published
/// terminology this module's constants cite; the names, places and measurements are
/// synthetic and carry a numeric suffix, as Synthea's own output does, so no row can
/// be read as a real person.
struct CohortMember {
    /// `Patient.name[0].family`.
    family: &'static str,
    /// `Patient.name[0].given[0]`.
    given: &'static str,
    /// `Patient.gender`, from the FHIR R4 `AdministrativeGender` value set
    /// (`male | female | other | unknown`).
    gender: &'static str,
    /// `Patient.birthDate`, a FHIR `date`.
    birth_date: &'static str,
    /// `Patient.address[0].city`.
    city: &'static str,
    /// `Patient.address[0].state`.
    state: &'static str,
    /// `Patient.address[0].postalCode`.
    postal_code: &'static str,
    /// The SNOMED CT code of the condition recorded at this record's encounter.
    condition_code: &'static str,
    /// The display of `condition_code`.
    condition_display: &'static str,
    /// Body height in centimetres (LOINC 8302-2).
    height_cm: f64,
    /// Body weight in kilograms (LOINC 29463-7).
    weight_kg: f64,
    /// Heart rate in beats per minute (LOINC 8867-4).
    heart_rate: i64,
    /// Systolic blood pressure in mm Hg (LOINC 8480-6).
    systolic: i64,
    /// Diastolic blood pressure in mm Hg (LOINC 8462-4).
    diastolic: i64,
}

/// The preloaded cohort: the synthetic patient records a preloaded data store comes
/// up holding.
///
/// Spelled out rather than generated, so the dataset one preloaded data store holds
/// is the dataset the next one holds, and bounded so a preload cannot be the thing
/// that fills the store: `COHORT.len() * RESOURCES_PER_RECORD` is far below
/// [`crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE`].
///
/// The condition codes are SNOMED CT: 44054006 "Diabetes mellitus type 2
/// (disorder)", 59621000 "Essential hypertension (disorder)", 195967001 "Asthma
/// (disorder)", 162864005 "Body mass index 30+ - obesity (finding)" and 40055000
/// "Chronic sinusitis (disorder)".
const COHORT: &[CohortMember] = &[
    CohortMember {
        family: "Abbott260",
        given: "Rosalyn413",
        gender: "female",
        birth_date: "1974-12-25",
        city: "Springfield",
        state: "Massachusetts",
        postal_code: "01103",
        condition_code: "44054006",
        condition_display: "Diabetes mellitus type 2 (disorder)",
        height_cm: 163.5,
        weight_kg: 78.2,
        heart_rate: 74,
        systolic: 132,
        diastolic: 84,
    },
    CohortMember {
        family: "Barrows455",
        given: "Dewayne781",
        gender: "male",
        birth_date: "1958-06-03",
        city: "Quincy",
        state: "Massachusetts",
        postal_code: "02169",
        condition_code: "59621000",
        condition_display: "Essential hypertension (disorder)",
        height_cm: 178.1,
        weight_kg: 92.4,
        heart_rate: 68,
        systolic: 148,
        diastolic: 91,
    },
    CohortMember {
        family: "Cassin499",
        given: "Marisol205",
        gender: "female",
        birth_date: "2001-09-17",
        city: "Lowell",
        state: "Massachusetts",
        postal_code: "01852",
        condition_code: "195967001",
        condition_display: "Asthma (disorder)",
        height_cm: 157.9,
        weight_kg: 54.6,
        heart_rate: 82,
        systolic: 114,
        diastolic: 72,
    },
    CohortMember {
        family: "Dickinson704",
        given: "Emory871",
        gender: "male",
        birth_date: "1989-02-11",
        city: "Worcester",
        state: "Massachusetts",
        postal_code: "01604",
        condition_code: "162864005",
        condition_display: "Body mass index 30+ - obesity (finding)",
        height_cm: 171.2,
        weight_kg: 101.7,
        heart_rate: 77,
        systolic: 127,
        diastolic: 80,
    },
    CohortMember {
        family: "Ebert178",
        given: "Tamala639",
        gender: "other",
        birth_date: "1966-11-29",
        city: "Brockton",
        state: "Massachusetts",
        postal_code: "02301",
        condition_code: "40055000",
        condition_display: "Chronic sinusitis (disorder)",
        height_cm: 168.4,
        weight_kg: 66.9,
        heart_rate: 71,
        systolic: 121,
        diastolic: 76,
    },
];

/// How many synthetic patient records a preloaded data store comes up holding.
///
/// `pub` for the same reason [`PRELOADED_RESOURCE_COUNT`] is: a caller can state what
/// a preload delivers without restating the cohort.
pub const PRELOADED_RECORD_COUNT: usize = COHORT.len();

/// How many FHIR resources a preloaded data store comes up holding.
///
/// `pub` so a caller — and this crate's test suite — can state the count a preload
/// delivers without restating the cohort.
pub const PRELOADED_RESOURCE_COUNT: usize = PRELOADED_RECORD_COUNT * RESOURCES_PER_RECORD;

/// A preload must never be the thing that fills a data store: the cohort is spelled
/// out above, so the count it delivers is checked against the store's own per-data
/// store ceiling at COMPILE time rather than refused at run time.
const _: () = assert!(PRELOADED_RESOURCE_COUNT <= crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE);

/// The FHIR resources a data store holds, keyed as
/// [`crate::state::Datastore::fhir_resources`] keys them, beside the bytes they cost
/// it ([`crate::state::Datastore::fhir_resource_bytes`]).
type PreloadedResources = (BTreeMap<(String, String), FhirResource>, usize);

/// The preloaded Synthea dataset, as the resources a data store holds and the bytes
/// they cost it — the same two values [`crate::state::Datastore::fhir_resources`] and
/// [`crate::state::Datastore::fhir_resource_bytes`] carry for a data store a caller
/// wrote to.
///
/// Every resource is stored exactly as the FHIR data plane stores one a caller
/// created: the content with the server-owned `id` and `meta` written onto it (by the
/// one builder of that, [`super::fhir_rest::stored_content`]), at version 1, stamped
/// with the data store's creation instant, under a logical id the store's own mint
/// assigned. So a `read` of a preloaded resource and a `read` of a created one are
/// the same interaction over the same kind of record.
///
/// `None` when the preload could not be built — an id the mint could not place, or an
/// instant that could not be rendered — so the caller answers a modelled server error
/// rather than bringing a half-preloaded data store up.
pub(crate) fn preloaded_resources(
    store: &mut ComAmazonawsHealthlakeHealthlakeState,
    created_at: SystemTime,
) -> Option<PreloadedResources> {
    let stamp = ::aws_smithy_types::DateTime::from(created_at);
    let last_updated = stamp
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    let mut resources: BTreeMap<(String, String), FhirResource> = BTreeMap::new();
    let mut bytes = 0usize;

    for (index, member) in COHORT.iter().enumerate() {
        // Each record's encounter sits one spacing further back than the last, so
        // the cohort spans a stretch of time. `checked_sub` keeps the function total
        // on a clock close enough to the epoch that the subtraction would underflow.
        let encounter_start = created_at
            .checked_sub(ENCOUNTER_SPACING * (index as u32 + 1))
            .unwrap_or(created_at);
        let encounter_end = encounter_start + ENCOUNTER_DURATION;
        let seen_at = instant(encounter_start)?;
        let seen_until = instant(encounter_end)?;

        let patient_id = mint(store, &resources, "Patient", created_at)?;
        let patient_ref = format!("Patient/{patient_id}");
        let encounter_id = mint(store, &resources, "Encounter", created_at)?;
        let encounter_ref = format!("Encounter/{encounter_id}");

        let mut record = vec![
            ("Patient", patient_id, patient(member, index)),
            (
                "Encounter",
                encounter_id,
                encounter(&patient_ref, &seen_at, &seen_until),
            ),
        ];
        record.push((
            "Condition",
            mint(store, &resources, "Condition", created_at)?,
            condition(member, &patient_ref, &encounter_ref, &seen_at),
        ));
        for observation in observations(member, &patient_ref, &encounter_ref, &seen_at) {
            record.push((
                "Observation",
                mint(store, &resources, "Observation", created_at)?,
                observation,
            ));
        }

        for (resource_type, id, content) in record {
            let content = super::fhir_rest::stored_content(content, &id, 1, &last_updated);
            let serialized = content.to_string();
            let resource = FhirResource {
                resource_type: resource_type.to_string(),
                id: id.clone(),
                version: 1,
                last_updated: stamp,
                content,
                bytes: serialized.len(),
            };
            bytes = bytes.saturating_add(resource.bytes);
            resources.insert((resource_type.to_string(), id), resource);
        }
    }

    Some((resources, bytes))
}

/// A logical id for a preloaded resource of `resource_type`, from the store's OWN
/// mint — the one a FHIR create assigns from — checked against the resources this
/// preload has already placed so no preloaded resource can overwrite another.
///
/// `None` when the capped walk found no unused candidate, which the caller answers as
/// a server error rather than by dropping a record.
fn mint(
    store: &mut ComAmazonawsHealthlakeHealthlakeState,
    placed: &BTreeMap<(String, String), FhirResource>,
    resource_type: &str,
    created_at: SystemTime,
) -> Option<String> {
    for _ in 0..MAX_ID_MINT_ATTEMPTS {
        let candidate = store.next_fhir_resource_id(created_at);
        if !placed.contains_key(&(resource_type.to_string(), candidate.clone())) {
            return Some(candidate);
        }
    }
    None
}

/// An instant as a FHIR `dateTime`, in the same rendering
/// [`super::fhir_rest::stored_content`] writes `meta.lastUpdated` with.
fn instant(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// One synthetic `Patient`.
///
/// The medical record number is issued under [`SYNTHEA_IDENTIFIER_SYSTEM`] and
/// numbered by the row's position in the cohort, so it names the generated record
/// rather than echoing anything a caller sent.
fn patient(member: &CohortMember, index: usize) -> Value {
    json!({
        "resourceType": "Patient",
        "identifier": [{
            "type": {
                "coding": [{
                    "system": IDENTIFIER_TYPE,
                    "code": "MR",
                    "display": "Medical Record Number",
                }],
                "text": "Medical Record Number",
            },
            "system": SYNTHEA_IDENTIFIER_SYSTEM,
            "value": format!("SYNTHEA-{:04}", index + 1),
        }],
        "active": true,
        "name": [{
            "use": "official",
            "family": member.family,
            "given": [member.given],
        }],
        "gender": member.gender,
        "birthDate": member.birth_date,
        "address": [{
            "city": member.city,
            "state": member.state,
            "postalCode": member.postal_code,
            "country": "US",
        }],
    })
}

/// The `Encounter` one synthetic record was seen at: a finished ambulatory visit of
/// the patient `patient_ref` names.
fn encounter(patient_ref: &str, start: &str, end: &str) -> Value {
    json!({
        "resourceType": "Encounter",
        "status": "finished",
        "class": {
            "system": ACT_CODE,
            "code": "AMB",
            "display": "ambulatory",
        },
        "type": [{
            "coding": [{
                "system": SNOMED,
                "code": ENCOUNTER_TYPE_CODE,
                "display": ENCOUNTER_TYPE_DISPLAY,
            }],
            "text": ENCOUNTER_TYPE_DISPLAY,
        }],
        "subject": { "reference": patient_ref },
        "period": { "start": start, "end": end },
    })
}

/// The `Condition` recorded at one synthetic record's encounter: active, confirmed,
/// and referencing both the patient and the encounter it was recorded at.
fn condition(
    member: &CohortMember,
    patient_ref: &str,
    encounter_ref: &str,
    recorded_at: &str,
) -> Value {
    json!({
        "resourceType": "Condition",
        "clinicalStatus": {
            "coding": [{ "system": CONDITION_CLINICAL, "code": "active" }],
        },
        "verificationStatus": {
            "coding": [{ "system": CONDITION_VERIFICATION, "code": "confirmed" }],
        },
        "code": {
            "coding": [{
                "system": SNOMED,
                "code": member.condition_code,
                "display": member.condition_display,
            }],
            "text": member.condition_display,
        },
        "subject": { "reference": patient_ref },
        "encounter": { "reference": encounter_ref },
        "onsetDateTime": recorded_at,
        "recordedDate": recorded_at,
    })
}

/// The vital-sign `Observation`s taken at one synthetic record's encounter: body
/// height (LOINC 8302-2), body weight (LOINC 29463-7), heart rate (LOINC 8867-4) and
/// the blood-pressure panel (LOINC 85354-9) with its systolic (8480-6) and diastolic
/// (8462-4) components.
fn observations(
    member: &CohortMember,
    patient_ref: &str,
    encounter_ref: &str,
    effective: &str,
) -> Vec<Value> {
    let mut panel = observation(
        "85354-9",
        "Blood pressure panel with all children optional",
        patient_ref,
        encounter_ref,
        effective,
    );
    panel["component"] = json!([
        {
            "code": {
                "coding": [{
                    "system": LOINC,
                    "code": "8480-6",
                    "display": "Systolic blood pressure",
                }],
            },
            "valueQuantity": quantity(member.systolic as f64, "mm[Hg]"),
        },
        {
            "code": {
                "coding": [{
                    "system": LOINC,
                    "code": "8462-4",
                    "display": "Diastolic blood pressure",
                }],
            },
            "valueQuantity": quantity(member.diastolic as f64, "mm[Hg]"),
        },
    ]);

    let mut height = observation(
        "8302-2",
        "Body Height",
        patient_ref,
        encounter_ref,
        effective,
    );
    height["valueQuantity"] = quantity(member.height_cm, "cm");

    let mut weight = observation(
        "29463-7",
        "Body Weight",
        patient_ref,
        encounter_ref,
        effective,
    );
    weight["valueQuantity"] = quantity(member.weight_kg, "kg");

    let mut heart_rate = observation(
        "8867-4",
        "Heart rate",
        patient_ref,
        encounter_ref,
        effective,
    );
    heart_rate["valueQuantity"] = quantity(member.heart_rate as f64, "/min");

    vec![height, weight, heart_rate, panel]
}

/// A final vital-sign `Observation` of `code`, without its value: the caller writes
/// either a `valueQuantity` or the panel's `component` list onto it.
fn observation(
    code: &str,
    display: &str,
    patient_ref: &str,
    encounter_ref: &str,
    effective: &str,
) -> Value {
    json!({
        "resourceType": "Observation",
        "status": "final",
        "category": [{
            "coding": [{
                "system": OBSERVATION_CATEGORY,
                "code": "vital-signs",
                "display": "Vital Signs",
            }],
        }],
        "code": {
            "coding": [{ "system": LOINC, "code": code, "display": display }],
            "text": display,
        },
        "subject": { "reference": patient_ref },
        "encounter": { "reference": encounter_ref },
        "effectiveDateTime": effective,
        "issued": effective,
    })
}

/// A FHIR `Quantity` in the UCUM unit `unit`, which a vital sign's value is stated
/// in.
fn quantity(value: f64, unit: &str) -> Value {
    json!({
        "value": value,
        "unit": unit,
        "system": UCUM,
        "code": unit,
    })
}
