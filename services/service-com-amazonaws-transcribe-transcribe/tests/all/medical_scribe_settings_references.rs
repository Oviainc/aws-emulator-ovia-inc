// SPDX-License-Identifier: CC0-1.0

//! What a Medical Scribe job's settings may NAME, end to end over the wire.
//!
//! THE RULE UNDER TEST: a `StartMedicalScribeJob` may only apply a custom resource this
//! account HOLDS — `Settings.VocabularyName` names a custom vocabulary and
//! `Settings.VocabularyFilterName` a custom vocabulary filter (`smithy_query(query="shape",
//! shape="MedicalScribeSettings")`), so a name no create of that family ever recorded is
//! refused with the modelled `BadRequestException` (HTTP 400) carrying that family's
//! not-found text and stores nothing, while the same start, once the account holds the
//! resource, is served and reads the named reference back through `GetMedicalScribeJob`.
//! The two families are distinct stores, so a resource of one never answers for a name
//! the other's member asks about.
//!
//! The rule is asserted over a table spanning what it generalises over: which settings
//! member carries the reference (each one alone and both at once), the name forms the
//! members' shared `@pattern`/`@length` admit including both published length bounds,
//! the vocabulary filter method applied, and which of the two speaker-partitioning modes
//! the job asks for — a reference check is independent of that mode.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time, and
//! the not-found wording comes from `crate::transcription_job_request_rules`, the one
//! home for the text every family's not-found refusal carries.

use serde_json::{json, Value};

use crate::transcription_job_request_rules::not_found_message;
use crate::transcription_jobs::{Reply, Transcribe};

/// A `DataAccessRoleArn` of the form the member's `@pattern` pins (the operation models
/// it `@required`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// Every value of `VocabularyFilterMethod` (`smithy_query(query="shape",
/// shape="VocabularyFilterMethod")`), so a filter row is not pinned to one of them.
const FILTER_METHODS: &[&str] = &["remove", "mask", "tag"];

/// The word this service's refusals use for a custom vocabulary, and for a custom
/// vocabulary filter.
const KIND_VOCABULARY: &str = "vocabulary";
const KIND_VOCABULARY_FILTER: &str = "vocabulary filter";

/// `CreateVocabulary` for `name`, so this account holds a custom vocabulary of that
/// name.
fn create_vocabulary(name: &str) -> (&'static str, Value) {
    (
        "CreateVocabulary",
        json!({
            "VocabularyName": name,
            "LanguageCode": "en-US",
            "Phrases": ["Aardvark"]
        }),
    )
}

/// `CreateVocabularyFilter` for `name`, so this account holds a custom vocabulary filter
/// of that name.
fn create_vocabulary_filter(name: &str) -> (&'static str, Value) {
    (
        "CreateVocabularyFilter",
        json!({
            "VocabularyFilterName": name,
            "LanguageCode": "en-US",
            "Words": ["crud"]
        }),
    )
}

/// One way a start's settings can name custom resources: the members that carry the
/// names, the families they belong to, and the creates that make the account hold them.
struct Reference {
    /// The member path(s), for the case label.
    case: &'static str,
    /// The families the members name, so a refusal must use one of their words.
    kinds: &'static [&'static str],
    /// The settings members that name `name`; `index` varies what qualifies the
    /// reference (the filter method) across rows.
    members: fn(name: &str, index: usize) -> Value,
    /// Every create needed before a start carrying these members is served.
    creates: fn(name: &str) -> Vec<(&'static str, Value)>,
}

/// Each custom resource a Medical Scribe job's settings may name, plus the row that
/// names both at once — a start carrying two references needs both held.
fn references() -> Vec<Reference> {
    vec![
        Reference {
            case: "Settings.VocabularyName",
            kinds: &[KIND_VOCABULARY],
            members: |name, _| json!({"VocabularyName": name}),
            creates: |name| vec![create_vocabulary(name)],
        },
        Reference {
            case: "Settings.VocabularyFilterName",
            kinds: &[KIND_VOCABULARY_FILTER],
            // "VocabularyFilterMethod must be specified if VocabularyFilterName is set."
            members: |name, index| {
                json!({
                    "VocabularyFilterName": name,
                    "VocabularyFilterMethod": FILTER_METHODS[index % FILTER_METHODS.len()]
                })
            },
            creates: |name| vec![create_vocabulary_filter(name)],
        },
        Reference {
            case: "Settings.VocabularyName and Settings.VocabularyFilterName together",
            kinds: &[KIND_VOCABULARY, KIND_VOCABULARY_FILTER],
            members: |name, index| {
                json!({
                    "VocabularyName": name,
                    "VocabularyFilterName": name,
                    "VocabularyFilterMethod": FILTER_METHODS[index % FILTER_METHODS.len()]
                })
            },
            creates: |name| vec![create_vocabulary(name), create_vocabulary_filter(name)],
        },
    ]
}

/// One of the two speaker-partitioning modes a start must choose exactly one of: the
/// settings members it needs, and the `ChannelDefinitions` that are set "if and only if
/// the ChannelIdentification value of Settings is set to true".
struct Mode {
    case: &'static str,
    settings: fn() -> Value,
    channel_definitions: fn() -> Value,
}

fn modes() -> Vec<Mode> {
    vec![
        Mode {
            case: "speaker partitioning",
            settings: || json!({"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}),
            channel_definitions: || Value::Null,
        },
        Mode {
            case: "channel identification",
            settings: || json!({"ChannelIdentification": true}),
            channel_definitions: || {
                json!([
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 1, "ParticipantRole": "PATIENT"}
                ])
            },
        },
    ]
}

/// A start request named `name`, in the partitioning `mode`, whose `Settings` carry
/// `members` beside the mode's own settings members.
fn start_request(name: &str, mode: &Mode, members: &Value) -> Value {
    let mut settings = (mode.settings)();
    let settings_object = settings.as_object_mut().expect("the mode's settings");
    for (member, value) in members.as_object().expect("the reference's members") {
        settings_object.insert(member.clone(), value.clone());
    }

    let mut request = json!({
        "MedicalScribeJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
        "OutputBucketName": "example-scribe-output",
        "DataAccessRoleArn": ROLE_ARN,
        "Settings": settings
    });
    let definitions = (mode.channel_definitions)();
    if !definitions.is_null() {
        request
            .as_object_mut()
            .expect("a start request is a JSON object")
            .insert("ChannelDefinitions".to_string(), definitions);
    }
    request
}

/// `GetMedicalScribeJob` for `name`.
async fn get(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call("GetMedicalScribeJob", json!({"MedicalScribeJobName": name}))
        .await
}

#[tokio::test]
async fn a_medical_scribe_start_naming_a_custom_resource_this_account_lacks_is_refused() {
    // The name forms the members' shared `@pattern` (`^[0-9a-zA-Z._-]+$`) admits, at
    // both published `@length` bounds (1..200).
    let longest = "z".repeat(200);
    let names: Vec<&str> = vec!["never-created", "a", "clinic.terms_2", "9", &longest];

    for (row, reference) in references().iter().enumerate() {
        for (mode_index, mode) in modes().iter().enumerate() {
            // A fresh emulator per member path and mode, so a resource one row created
            // cannot be what makes the next row's start succeed.
            let transcribe = Transcribe::new();
            let expected: Vec<String> = reference
                .kinds
                .iter()
                .map(|kind| not_found_message(kind))
                .collect();

            for (index, resource) in names.iter().enumerate() {
                let case = format!(
                    "{} naming the unheld {resource:?}, with {}",
                    reference.case, mode.case
                );
                let name = format!("dangling-{row}-{mode_index}-{index}");
                let members = (reference.members)(resource, index);
                let refused = transcribe
                    .call("StartMedicalScribeJob", start_request(&name, mode, &members))
                    .await;
                refused.refused(&case, 400, "BadRequestException");

                // The refusal is a family's own not-found text, which names the KIND of
                // resource — the whole message, so a name the caller asked about is not
                // appended to it either.
                let message = refused.error_message(&case);
                assert!(
                    expected.contains(&message),
                    "case {case:?}: the refusal must be one of {expected:?}, got \
                     {message:?}"
                );

                // A refused start stores nothing, so the name it asked for is unknown.
                get(&transcribe, &name)
                    .await
                    .refused(&case, 400, "BadRequestException");
            }

            for (index, resource) in names.iter().enumerate() {
                let case = format!(
                    "{} naming the held {resource:?}, with {}",
                    reference.case, mode.case
                );
                let name = format!("held-{row}-{mode_index}-{index}");
                let members = (reference.members)(resource, index);
                for (operation, input) in (reference.creates)(resource) {
                    transcribe.call(operation, input).await.ok(&case);
                }

                let request = start_request(&name, mode, &members);
                transcribe
                    .call("StartMedicalScribeJob", request.clone())
                    .await
                    .ok(&case);

                // The served start stored the reference, so a read reports it: the
                // mode's settings members plus the row's, and nothing else.
                let fetched = get(&transcribe, &name).await;
                let settings = fetched.ok(&case)["MedicalScribeJob"]["Settings"].clone();
                assert_eq!(
                    settings, request["Settings"],
                    "case {case:?}: the stored job must report the settings it was \
                     started with, got {settings:?}"
                );
            }
        }
    }
}

/// The custom vocabularies and the custom vocabulary FILTERS are families of their own:
/// a resource of one family never makes its name known to the member that names the
/// other, whichever of the two this account holds.
#[tokio::test]
async fn one_custom_family_does_not_stand_in_for_the_other() {
    // Rows differ in which family the account holds, which member names the other, and
    // the name form the two share.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a custom vocabulary standing in for a vocabulary filter",
            "shared-name",
            json!({"VocabularyFilterName": "shared-name", "VocabularyFilterMethod": "mask"}),
            KIND_VOCABULARY_FILTER,
        ),
        (
            "a custom vocabulary filter standing in for a vocabulary",
            "shared.name_2",
            json!({"VocabularyName": "shared.name_2"}),
            KIND_VOCABULARY,
        ),
    ];

    for (index, (case, resource, members, kind)) in rows.iter().enumerate() {
        let transcribe = Transcribe::new();
        // Hold the OTHER family's resource of this name: the one the row's member does
        // not name.
        let (operation, input) = if *kind == KIND_VOCABULARY_FILTER {
            create_vocabulary(resource)
        } else {
            create_vocabulary_filter(resource)
        };
        transcribe.call(operation, input).await.ok(case);

        let name = format!("cross-family-{index}");
        let refused = transcribe
            .call(
                "StartMedicalScribeJob",
                start_request(&name, &modes()[0], members),
            )
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_found_message(kind),
            "case {case:?}: the refusal must name the {kind:?} family, got {message:?}"
        );

        get(&transcribe, &name)
            .await
            .refused(case, 400, "BadRequestException");
    }
}
