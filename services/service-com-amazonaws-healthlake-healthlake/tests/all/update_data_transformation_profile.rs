// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `UpdateDataTransformationProfile` over the real
//! awsJson1_0 wire, read back through `GetDataTransformationProfile`.
//!
//! The rule under test: an update REPLACES the whole content of a profile's DRAFT
//! (version 0) with the content the call carries — so the DRAFT afterwards holds
//! exactly the new files and none of the old ones — stamps the DRAFT with that
//! call's `ChangeDescription` and the instant it ran, and leaves the profile's
//! metadata and every other profile alone. A profile no record holds, and content
//! this service will not store, are refused BEFORE anything is written, so a refused
//! update leaves the stored DRAFT exactly as it was.
//!
//! The rows differ along what the rule generalises over: how many entries the
//! replacement has relative to the stored DRAFT (fewer, more, the same), how its key
//! set relates to the stored one (disjoint, overlapping, a subset), the size and form
//! of the values, and whether the call carries a `ChangeDescription` at all.
//!
//! Every call below goes through the shared [`Profiles`] harness, i.e.
//! `MultiServiceRouter::dispatch` — the same entry point `core/router` serves — so
//! routing, deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    profile_version_not_found_message,
};

use crate::data_transformation_profile::{
    assert_well_formed_profile_id, mapping, mapping_of, mapping_of_bytes, modelled_message,
    profile_mapping_source, reported_mapping, Profiles,
};
use crate::fhir_datastore_lifecycle::string_at;

/// The `ProfileMapping` of an `UpdateDataTransformationProfile` request body.
fn request_mapping(content: &BTreeMap<String, String>) -> Value {
    let entries: Map<String, Value> = content
        .iter()
        .map(|(path, entry)| (path.clone(), Value::String(entry.clone())))
        .collect();
    Value::Object(entries)
}

/// A `LastUpdatedAt` timestamp as the awsJson wire carries it: epoch seconds.
fn last_updated_at(case: &str, body: &Value) -> f64 {
    body["LastUpdatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: response must carry a LastUpdatedAt: {body}"))
}

/// One update row: the DRAFT a profile starts with, the content the update carries,
/// and the `ChangeDescription` it carries with it (`None` for a call that omits it).
struct Replacement {
    case: &'static str,
    source_format: &'static str,
    initial: BTreeMap<String, String>,
    replacement: BTreeMap<String, String>,
    change_description: Option<String>,
}

/// The replacement table. Every row's DRAFT must end up carrying exactly its
/// `replacement`, whatever its relation to the content that was there.
fn replacement_cases() -> Vec<Replacement> {
    // The modelled `ProfileMappingKey` bound is 1..500 characters and the modelled
    // `ProfileMappingValue` bound is 0..102400, so a 500-character key and an empty
    // value are both at a published edge.
    let long_key = "r".repeat(500);
    vec![
        Replacement {
            case: "a disjoint key set: every stored file is replaced by a different one",
            source_format: "CSV",
            initial: mapping(&[
                ("patients.csv.map", "Patient.id <- patient_id"),
                ("encounters.csv.map", "Encounter.id <- encounter_id"),
            ]),
            replacement: mapping(&[
                ("observations.csv.map", "Observation.code <- loinc"),
                ("conditions.csv.map", "Condition.code <- icd10"),
            ]),
            change_description: Some("mapped observations and conditions instead".to_string()),
        },
        Replacement {
            case: "an overlapping key set: one file rewritten, one dropped, one added",
            source_format: "CCDA",
            initial: mapping(&[
                ("sections/problems.map", "Condition.code <- problem/value"),
                ("sections/vitals.map", "Observation.value[x] <- vital/value"),
            ]),
            replacement: mapping(&[
                ("sections/problems.map", "Condition.code <- problem/code"),
                ("sections/results.map", "Observation <- result"),
            ]),
            change_description: Some("problems now map from code".to_string()),
        },
        Replacement {
            case: "a strict subset: the DRAFT shrinks to one of its files",
            source_format: "CCDA",
            initial: mapping_of(12),
            replacement: mapping(&[("mappings/section-0003.map", "Observation.component[3] <- column_3")]),
            change_description: None,
        },
        Replacement {
            case: "a growing DRAFT: one entry becomes the modelled 500-entry maximum",
            source_format: "CSV",
            initial: mapping(&[("patients.csv.map", "Patient.id <- patient_id")]),
            replacement: mapping_of(500),
            change_description: Some("split the mapping across every section".to_string()),
        },
        Replacement {
            case: "the same count, at the modelled key maximum with an empty value",
            source_format: "CCDA",
            initial: mapping(&[("sections/problems.map", "Condition.code <- problem/value")]),
            replacement: mapping(&[(long_key.as_str(), "")]),
            change_description: Some(String::new()),
        },
        Replacement {
            case: "a large value and a ChangeDescription at its modelled 1000-character maximum",
            source_format: "CSV",
            initial: mapping(&[("patients.csv.map", "Patient.id <- patient_id")]),
            replacement: mapping(&[
                ("patients.csv.map", "v".repeat(100_000).as_str()),
                ("notes.csv.map", "DocumentReference.content <- note"),
            ]),
            change_description: Some("c".repeat(1000)),
        },
    ]
}

/// The request body of an update, with the `ChangeDescription` member present only
/// when the row carries one.
fn update_request(row: &Replacement, profile_id: &str) -> Value {
    let mut request = json!({
        "ProfileId": profile_id,
        "ProfileMapping": request_mapping(&row.replacement),
    });
    if let Some(description) = &row.change_description {
        request["ChangeDescription"] = json!(description);
    }
    request
}

/// Every row's update replaces the DRAFT's whole content, and the modelled reader
/// reports exactly that content back — plus the row's change description and the
/// instant the update ran. The update publishes nothing.
#[tokio::test]
async fn update_replaces_the_whole_draft_and_get_reads_it_back() {
    let healthlake = Profiles::new();

    for row in replacement_cases() {
        let case = row.case;
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": format!("update-{}", row.source_format.to_lowercase()),
                    "ProfileDescription": "the profile metadata an update leaves alone",
                    "SourceFormat": row.source_format,
                    "Source": profile_mapping_source(&row.initial),
                }),
            )
            .await;
        let profile_id = string_at(&created, "ProfileId");
        assert_well_formed_profile_id(case, &profile_id);
        let created_at = last_updated_at(case, &created);

        // The content that is there before the update, so the replacement below is
        // shown to have replaced something.
        let before = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            reported_mapping(case, &before),
            row.initial,
            "case {case:?}: the created DRAFT carries the content the create stored"
        );

        let updated = healthlake
            .ok(
                case,
                "UpdateDataTransformationProfile",
                &update_request(&row, &profile_id),
            )
            .await;
        assert_eq!(
            updated["ProfileId"].as_str(),
            Some(profile_id.as_str()),
            "case {case:?}: the update reports the profile it was asked for, got {updated}"
        );
        assert_eq!(
            updated["SourceFormat"].as_str(),
            Some(row.source_format),
            "case {case:?}: the update reports the profile's stored SourceFormat, got {updated}"
        );
        assert_eq!(
            updated["TargetFormat"], created["TargetFormat"],
            "case {case:?}: the update reports the profile's stored TargetFormat, got {updated}"
        );
        assert_eq!(
            updated["ProfileName"], created["ProfileName"],
            "case {case:?}: an update does not rename the profile, got {updated}"
        );
        let updated_at = last_updated_at(case, &updated);
        assert!(
            updated_at >= created_at,
            "case {case:?}: expected LastUpdatedAt at or after the create's {created_at}, got {updated_at}"
        );

        // The DRAFT the update wrote, read back through the modelled reader: the
        // replacement in full, and nothing of what was there before.
        let draft = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        let stored = reported_mapping(case, &draft);
        assert_eq!(
            stored, row.replacement,
            "case {case:?}: the DRAFT carries exactly the content the update carried"
        );
        for path in row.initial.keys() {
            assert!(
                row.replacement.contains_key(path) || !stored.contains_key(path),
                "case {case:?}: expected the replaced file {path:?} to be gone, got {stored:?}"
            );
        }
        assert_eq!(
            draft["Version"].as_i64(),
            Some(0),
            "case {case:?}: an update writes the DRAFT, version 0, got {draft}"
        );
        assert_eq!(
            draft["ChangeDescription"],
            row.change_description
                .as_deref()
                .map_or(Value::Null, |description| json!(description)),
            "case {case:?}: the DRAFT carries the change description this update gave it, got {draft}"
        );
        assert_eq!(
            last_updated_at(case, &draft),
            updated_at,
            "case {case:?}: the reader reports the instant the update stamped, got {draft}"
        );
        // The metadata the create stored is untouched by a content update.
        assert_eq!(
            draft["ProfileName"], created["ProfileName"],
            "case {case:?}: the stored ProfileName survives an update, got {draft}"
        );
        assert_eq!(
            draft["ProfileDescription"].as_str(),
            Some("the profile metadata an update leaves alone"),
            "case {case:?}: the stored ProfileDescription survives an update, got {draft}"
        );
        assert_eq!(
            draft["SourceFormat"].as_str(),
            Some(row.source_format),
            "case {case:?}: the stored SourceFormat survives an update, got {draft}"
        );

        // An update writes the DRAFT and publishes nothing, so a read naming no
        // version still resolves to that DRAFT — the content this update wrote.
        let unpublished = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id }),
            )
            .await;
        assert_eq!(
            unpublished, draft,
            "case {case:?}: an update publishes no version, so the DRAFT is what a read \
             naming none serves, got {unpublished}"
        );
    }
}

/// A chain of updates each replaces what the one before it wrote, and repeating one
/// update leaves the same content behind: the DRAFT is whatever the LAST update
/// carried, never an accumulation of them.
#[tokio::test]
async fn successive_updates_each_replace_the_one_before() {
    let healthlake = Profiles::new();
    let case = "a chain of replacements";
    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "update-chain",
                "SourceFormat": "CSV",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    let chain = [
        mapping(&[("b.map", "Encounter.id <- enc")]),
        mapping_of(7),
        mapping(&[("b.map", "Encounter.id <- enc_2"), ("c.map", "Observation <- obs")]),
        mapping(&[("a.map", "Patient.id <- id")]),
    ];
    for (step, expected) in chain.iter().enumerate() {
        for repeat in 0..2 {
            // The same update twice: replacement is idempotent in what it stores.
            healthlake
                .ok(
                    case,
                    "UpdateDataTransformationProfile",
                    &json!({
                        "ProfileId": profile_id,
                        "ProfileMapping": request_mapping(expected),
                        "ChangeDescription": format!("step {step}"),
                    }),
                )
                .await;
            let draft = healthlake
                .ok(
                    case,
                    "GetDataTransformationProfile",
                    &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
                )
                .await;
            assert_eq!(
                &reported_mapping(case, &draft),
                expected,
                "case {case:?}: after step {step} repeat {repeat} the DRAFT is what that update carried"
            );
            assert_eq!(
                draft["ChangeDescription"].as_str(),
                Some(format!("step {step}").as_str()),
                "case {case:?}: step {step} repeat {repeat} stamps its own change description, got {draft}"
            );
        }
    }
}

/// An update's `ChangeDescription` describes THAT update, so one that carries none
/// leaves the DRAFT with none rather than the previous update's sentence.
#[tokio::test]
async fn an_update_without_a_change_description_clears_the_previous_one() {
    let healthlake = Profiles::new();
    let case = "an update that carries no ChangeDescription";
    let created = healthlake
        .ok(
            case,
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "update-change-description",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(&[("a.map", "Condition <- problem")])),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    healthlake
        .ok(
            case,
            "UpdateDataTransformationProfile",
            &json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[("b.map", "Observation <- vital")])),
                "ChangeDescription": "mapped vitals",
            }),
        )
        .await;
    let described = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
        )
        .await;
    assert_eq!(
        described["ChangeDescription"].as_str(),
        Some("mapped vitals"),
        "case {case:?}: the first update's description is stored, got {described}"
    );

    healthlake
        .ok(
            case,
            "UpdateDataTransformationProfile",
            &json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[("c.map", "AllergyIntolerance <- allergy")])),
            }),
        )
        .await;
    let undescribed = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
        )
        .await;
    assert!(
        undescribed["ChangeDescription"].is_null(),
        "case {case:?}: an update with no description leaves none behind, got {undescribed}"
    );
    assert_eq!(
        reported_mapping(case, &undescribed),
        mapping(&[("c.map", "AllergyIntolerance <- allergy")]),
        "case {case:?}: the content is still the one that update carried"
    );
}

/// The DRAFT a refused update must leave untouched.
const UNTOUCHED: &[(&str, &str)] = &[
    ("sections/problems.map", "Condition.code <- problem/value"),
    ("sections/vitals.map", "Observation.value[x] <- vital/value"),
];

/// Every update this service refuses, and what each is refused with. `None` for the
/// expected code is a row refused by the modelled constraints at the boundary, where
/// only the status is this slice's to assert.
fn refusal_cases(profile_id: &str) -> Vec<(&'static str, Value, u16, Option<&'static str>)> {
    let past_entry_maximum = mapping_of(501);
    vec![
        (
            "a well-formed ProfileId no profile has",
            json!({
                "ProfileId": "ffffffffffffffffffffffffffffffff",
                "ProfileMapping": request_mapping(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
            wire_http_status(404),
            Some("ResourceNotFoundException"),
        ),
        (
            "a ProfileId that is not 32 hexadecimal characters",
            json!({
                "ProfileId": "not-a-profile-id",
                "ProfileMapping": request_mapping(&mapping(&[("a.map", "Patient.id <- id")])),
            }),
            400,
            None,
        ),
        (
            "an empty ProfileMapping, below the modelled 1-entry minimum",
            json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&BTreeMap::new()),
            }),
            400,
            None,
        ),
        (
            "a ProfileMapping one entry past the modelled 500-entry maximum",
            json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&past_entry_maximum),
            }),
            400,
            None,
        ),
        (
            "a mapping key one character past the modelled 500-character maximum",
            json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[("k".repeat(501).as_str(), "x")])),
            }),
            400,
            None,
        ),
        (
            "a mapping value one character past the modelled 102400-character maximum",
            json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[(
                    "sections/huge.map",
                    "v".repeat(102_401).as_str(),
                )])),
            }),
            400,
            None,
        ),
        (
            // Every entry below is inside its own modelled bound; together they are
            // past the retained-byte ceiling this emulator holds for one mapping
            // (`state::MAX_PROFILE_MAPPING_BYTES`, 1 MiB), which is this handler's
            // own refusal rather than a modelled one.
            "a mapping whose entries together pass the retained-byte ceiling",
            json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping_of_bytes(1 << 20)),
            }),
            400,
            Some("ValidationException"),
        ),
        (
            "a ChangeDescription one character past its modelled 1000-character maximum",
            json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[("a.map", "Patient.id <- id")])),
                "ChangeDescription": "c".repeat(1001),
            }),
            400,
            None,
        ),
    ]
}

/// Every refused update is refused with the modelled status — and writes nothing, so
/// the stored DRAFT is still the content that was there before it.
#[tokio::test]
async fn refused_updates_leave_the_draft_untouched() {
    let healthlake = Profiles::new();
    let created = healthlake
        .ok(
            "refusals",
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "update-refusals",
                "SourceFormat": "CCDA",
                "Source": profile_mapping_source(&mapping(UNTOUCHED)),
            }),
        )
        .await;
    let profile_id = string_at(&created, "ProfileId");

    for (case, request, expected_status, expected_code) in refusal_cases(&profile_id) {
        let (status, code, payload) = healthlake
            .refused_body(case, "UpdateDataTransformationProfile", &request)
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}: {payload}"
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                code.as_str(),
                expected_code,
                "case {case:?}: expected {expected_code}, got {payload}"
            );
        }
        if expected_code == Some("ResourceNotFoundException") {
            // An update edits the DRAFT, so the version it resolved is 0 and the
            // refusal names the profile beside it.
            let named = string_at(&request, "ProfileId");
            assert_eq!(
                modelled_message(&payload),
                Some(profile_version_not_found_message(&named, 0).as_str()),
                "case {case:?}: an unknown profile is named beside the DRAFT the update \
                 resolved, got {payload}"
            );
        }

        let draft = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            reported_mapping(case, &draft),
            mapping(UNTOUCHED),
            "case {case:?}: a refused update writes nothing"
        );
        assert!(
            draft["ChangeDescription"].is_null(),
            "case {case:?}: a refused update stamps no change description, got {draft}"
        );
    }

    // An update still works after every refusal above.
    let case = "an update after the refusals";
    healthlake
        .ok(
            case,
            "UpdateDataTransformationProfile",
            &json!({
                "ProfileId": profile_id,
                "ProfileMapping": request_mapping(&mapping(&[("after.map", "Patient.id <- id")])),
            }),
        )
        .await;
    let draft = healthlake
        .ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
        )
        .await;
    assert_eq!(
        reported_mapping(case, &draft),
        mapping(&[("after.map", "Patient.id <- id")]),
        "case {case:?}: the update after the refusals is the one that stored content"
    );
}

/// An update writes the ONE profile it names: every other stored profile reads back
/// exactly what it held.
#[tokio::test]
async fn an_update_writes_only_the_profile_it_names() {
    let healthlake = Profiles::new();
    let case = "two profiles, one updated";
    let mut stored: Vec<(String, BTreeMap<String, String>)> = Vec::new();

    for index in 0..3u32 {
        let content = mapping(&[(
            "patients.csv.map",
            format!("Patient.id <- patient_id_{index}").as_str(),
        )]);
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": format!("neighbour-{index}"),
                    "SourceFormat": "CSV",
                    "Source": profile_mapping_source(&content),
                }),
            )
            .await;
        stored.push((string_at(&created, "ProfileId"), content));
    }

    let (target_id, _) = stored[1].clone();
    let replacement = mapping(&[("only.map", "Observation <- result")]);
    healthlake
        .ok(
            case,
            "UpdateDataTransformationProfile",
            &json!({
                "ProfileId": target_id,
                "ProfileMapping": request_mapping(&replacement),
            }),
        )
        .await;
    stored[1].1 = replacement;

    for (profile_id, expected) in &stored {
        let draft = healthlake
            .ok(
                case,
                "GetDataTransformationProfile",
                &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
            )
            .await;
        assert_eq!(
            &reported_mapping(case, &draft),
            expected,
            "case {case:?}: profile {profile_id} holds its own content after the update"
        );
    }
}
