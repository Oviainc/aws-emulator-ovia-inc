// SPDX-License-Identifier: CC0-1.0

//! The cross-member rules a job start must satisfy, end to end over the wire.
//!
//! THE RULES UNDER TEST:
//!
//! * CO-REQUIREMENT: where the model documents on a request member that including it
//!   obliges the caller to include a companion member, a start that names only the
//!   member is refused with the modelled `BadRequestException` (HTTP 400) and stores
//!   nothing, while one that names both (or neither) is accepted and reads the members
//!   back unchanged. The pairs are the ones this request's own members document
//!   (`smithy_query(query="operation", operation="StartTranscriptionJob")`,
//!   `smithy_query(query="shape", shape="Settings,ModelSettings,JobExecutionSettings")`):
//!   `Settings.VocabularyFilterName` -> `Settings.VocabularyFilterMethod`,
//!   `JobExecutionSettings` -> both its sub-parameters, `ModelSettings` ->
//!   `ModelSettings.LanguageModelName`, `OutputKey` -> `OutputBucketName`, and
//!   `LanguageOptions` -> the automatic language identification it restricts.
//! * DANGLING REFERENCE: a settings member that NAMES a custom vocabulary, custom
//!   vocabulary filter or custom language model may only name one this account holds,
//!   in that member's OWN family; a start naming anything else is refused with the same
//!   modelled error and stores nothing. The rule holds for every job family's start,
//!   not only the transcription one.
//!
//! Each test asserts its rule over a TABLE spanning the class — every documented pair
//! and each direction of it, every member path that can name a custom resource, every
//! family of custom resource, and the name forms the members' own `@pattern` /
//! `@length` admit — not over one row. The emulator, the call and the answer assertions
//! are the transcription-job suite's harness (`crate::transcription_jobs`), reused
//! rather than stood up a second time.

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// An IAM role ARN inside `DataAccessRoleArn`'s own `@pattern`
/// (`smithy_query(query="shape", shape="JobExecutionSettings")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// The name of a custom vocabulary filter this suite creates where a row needs the
/// filter to EXIST, so the row is decided by the rule under test and not by the
/// dangling-reference rule.
const HELD_FILTER: &str = "held-filter";

/// The name of a custom language model this suite creates, for the same reason.
const HELD_MODEL: &str = "held-model";

/// The smallest start request the service serves, under the name `name`.
fn base(name: &str) -> Value {
    json!({
        "TranscriptionJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
        "LanguageCode": "en-US"
    })
}

/// [`base`] with every member of `members` set on it — and with a member whose value
/// is JSON null REMOVED, which is how a row drops `LanguageCode` in favour of
/// automatic language identification.
fn start_request(name: &str, members: &Value) -> Value {
    let mut request = base(name);
    let object = request
        .as_object_mut()
        .expect("a start request is a JSON object");
    for (member, value) in members
        .as_object()
        .expect("a row's members are a JSON object")
    {
        if value.is_null() {
            object.remove(member);
        } else {
            object.insert(member.clone(), value.clone());
        }
    }
    request
}

/// `GetTranscriptionJob` for `name`.
async fn get(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
        .await
}

/// `CreateVocabulary` for `name`, so this account holds a custom vocabulary of that
/// name — and holds it in the state a start may apply it in, `READY`, which a create
/// reaches only once its build falls due.
async fn create_vocabulary(transcribe: &Transcribe, name: &str) {
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
        )
        .await
        .ok(&format!("CreateVocabulary {name:?}"));
    transcribe.vocabulary_ready(name, None).await;
}

/// `CreateVocabularyFilter` for `name`.
async fn create_vocabulary_filter(transcribe: &Transcribe, name: &str) {
    transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": name,
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await
        .ok(&format!("CreateVocabularyFilter {name:?}"));
}

/// `CreateLanguageModel` for `name`, so this account holds a custom language model of
/// that name — and holds it in the status a start may apply it in, `COMPLETED`, which a
/// create reaches only once its training falls due.
async fn create_language_model(transcribe: &Transcribe, name: &str) {
    transcribe
        .call(
            "CreateLanguageModel",
            json!({
                "ModelName": name,
                "LanguageCode": "en-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        )
        .await
        .ok(&format!("CreateLanguageModel {name:?}"));
    transcribe.language_model_completed(name, None).await;
}

/// One row of [`a_start_naming_one_side_of_a_co_requirement_is_refused`]: the members
/// it adds to the base request, whether the service may serve it, and — for a refused
/// row the co-requirement rule alone decides — the companion member its refusal must
/// name.
struct Pair {
    case: &'static str,
    members: Value,
    accepted: bool,
    names_companion: Option<&'static str>,
}

/// Every documented co-requirement of this request, in both the half-specified form
/// (refused) and the whole form (accepted), plus the row that names neither side.
///
/// The methods are the whole `VocabularyFilterMethod` enum (`mask | remove | tag` —
/// `smithy_query(query="shape", shape="VocabularyFilterMethod")`), and the output keys
/// are the forms `OutputKey` documents (a file name, a prefix, a nested prefix) at both
/// published `@length` bounds (1..1024).
fn pairs() -> Vec<Pair> {
    let mut rows = vec![
        Pair {
            case: "a vocabulary filter name with no filter method",
            members: json!({"Settings": {"VocabularyFilterName": HELD_FILTER}}),
            accepted: false,
            names_companion: Some("Settings.VocabularyFilterMethod"),
        },
        Pair {
            case: "a vocabulary filter name with no method, beside a whole speaker pair",
            members: json!({"Settings": {
                "ShowSpeakerLabels": true,
                "MaxSpeakerLabels": 2,
                "VocabularyFilterName": HELD_FILTER
            }}),
            accepted: false,
            names_companion: Some("Settings.VocabularyFilterMethod"),
        },
        Pair {
            case: "queueing asked for with no role to read the media with",
            members: json!({"JobExecutionSettings": {"AllowDeferredExecution": true}}),
            accepted: false,
            names_companion: Some("JobExecutionSettings.DataAccessRoleArn"),
        },
        Pair {
            case: "queueing declined, still with no role",
            members: json!({"JobExecutionSettings": {"AllowDeferredExecution": false}}),
            accepted: false,
            names_companion: Some("JobExecutionSettings.DataAccessRoleArn"),
        },
        Pair {
            case: "a role with no queueing decision",
            members: json!({"JobExecutionSettings": {"DataAccessRoleArn": ROLE_ARN}}),
            accepted: false,
            names_companion: Some("JobExecutionSettings.AllowDeferredExecution"),
        },
        Pair {
            case: "JobExecutionSettings carrying neither sub-parameter",
            members: json!({"JobExecutionSettings": {}}),
            accepted: false,
            names_companion: Some("JobExecutionSettings.AllowDeferredExecution"),
        },
        Pair {
            case: "ModelSettings naming no custom language model",
            members: json!({"ModelSettings": {}}),
            accepted: false,
            names_companion: Some("ModelSettings.LanguageModelName"),
        },
        Pair {
            case: "an output key naming a file, with no output bucket",
            members: json!({"OutputKey": "my-transcript.json"}),
            accepted: false,
            names_companion: Some("OutputBucketName"),
        },
        Pair {
            case: "an output key naming a nested prefix, with no output bucket",
            members: json!({"OutputKey": "test-files/transcripts/"}),
            accepted: false,
            names_companion: Some("OutputBucketName"),
        },
        Pair {
            case: "an output key one character long, the published floor, with no output bucket",
            members: json!({"OutputKey": "k"}),
            accepted: false,
            names_companion: Some("OutputBucketName"),
        },
        Pair {
            case: "language options beside a language code, so nothing identifies a language",
            members: json!({"LanguageOptions": ["en-US", "fr-FR"]}),
            accepted: false,
            names_companion: Some("IdentifyLanguage or IdentifyMultipleLanguages"),
        },
        Pair {
            case: "language options with single-language identification switched off",
            members: json!({
                "LanguageCode": Value::Null,
                "IdentifyLanguage": false,
                "LanguageOptions": ["en-US", "fr-FR"]
            }),
            accepted: false,
            // This row names no language specifier at all either, so the refusal it
            // gets is whichever of the two rules the handler reaches first.
            names_companion: None,
        },
        Pair {
            case: "neither side of any pair",
            members: json!({}),
            accepted: true,
            names_companion: None,
        },
        Pair {
            case: "both JobExecutionSettings sub-parameters, queueing asked for",
            members: json!({"JobExecutionSettings": {
                "AllowDeferredExecution": true,
                "DataAccessRoleArn": ROLE_ARN
            }}),
            accepted: true,
            names_companion: None,
        },
        Pair {
            case: "both JobExecutionSettings sub-parameters, queueing declined",
            members: json!({"JobExecutionSettings": {
                "AllowDeferredExecution": false,
                "DataAccessRoleArn": ROLE_ARN
            }}),
            accepted: true,
            names_companion: None,
        },
        Pair {
            case: "ModelSettings naming a custom language model this account holds",
            members: json!({"ModelSettings": {"LanguageModelName": HELD_MODEL}}),
            accepted: true,
            names_companion: None,
        },
        Pair {
            case: "an output key with the output bucket it belongs to",
            members: json!({
                "OutputBucketName": "example-output",
                "OutputKey": "test-files/my-transcript.json"
            }),
            accepted: true,
            names_companion: None,
        },
        Pair {
            case: "an output key at the published 1024-character ceiling, with its bucket",
            members: json!({
                "OutputBucketName": "example-output",
                "OutputKey": "k".repeat(1024)
            }),
            accepted: true,
            names_companion: None,
        },
        Pair {
            case: "language options with single-language identification",
            members: json!({
                "LanguageCode": Value::Null,
                "IdentifyLanguage": true,
                "LanguageOptions": ["en-US", "fr-FR"]
            }),
            accepted: true,
            names_companion: None,
        },
        Pair {
            case: "language options with multi-language identification",
            members: json!({
                "LanguageCode": Value::Null,
                "IdentifyMultipleLanguages": true,
                "LanguageOptions": ["en-US", "es-US", "fr-FR"]
            }),
            accepted: true,
            names_companion: None,
        },
    ];
    // One accepted row per documented filter method: the method is what the filter
    // name is co-required with, so each value of the enum completes the pair.
    for method in ["mask", "remove", "tag"] {
        rows.push(Pair {
            case: match method {
                "mask" => "a vocabulary filter name with the mask method",
                "remove" => "a vocabulary filter name with the remove method",
                _ => "a vocabulary filter name with the tag method",
            },
            members: json!({"Settings": {
                "VocabularyFilterName": HELD_FILTER,
                "VocabularyFilterMethod": method
            }}),
            accepted: true,
            names_companion: None,
        });
    }
    rows
}

/// The members of a start request that the `TranscriptionJob` shape also carries, so a
/// stored job must read them back unchanged (`smithy_query(query="shape",
/// shape="TranscriptionJob")`). `OutputBucketName` and `OutputKey` are NOT among them:
/// the output location they choose is reported through `Transcript.TranscriptFileUri`,
/// which the transcription-job suite asserts.
const ECHOED_MEMBERS: &[&str] = &[
    "Settings",
    "ModelSettings",
    "JobExecutionSettings",
    "IdentifyLanguage",
    "IdentifyMultipleLanguages",
    "LanguageOptions",
    "LanguageIdSettings",
];

/// Assert the stored `job` reports every member of `members` the `TranscriptionJob`
/// shape carries.
fn reports_members(case: &str, job: &Value, members: &Value) {
    for (member, value) in members
        .as_object()
        .expect("a row's members are a JSON object")
    {
        if value.is_null() || !ECHOED_MEMBERS.contains(&member.as_str()) {
            continue;
        }
        assert_eq!(
            job.get(member),
            Some(value),
            "case {case:?}: the stored job must report the {member} it was started \
             with, got {:?}",
            job.get(member)
        );
    }
}

#[tokio::test]
async fn a_start_naming_one_side_of_a_co_requirement_is_refused() {
    let transcribe = Transcribe::new();
    // The custom resources the rows NAME, so every row is decided by the
    // co-requirement rule and not by the dangling-reference rule.
    create_vocabulary_filter(&transcribe, HELD_FILTER).await;
    create_language_model(&transcribe, HELD_MODEL).await;

    let mut accepted_names: Vec<String> = Vec::new();
    for (index, row) in pairs().iter().enumerate() {
        let case = row.case;
        let name = format!("pair-{index}");
        let reply = transcribe
            .call("StartTranscriptionJob", start_request(&name, &row.members))
            .await;

        if row.accepted {
            let job = reply.ok(case)["TranscriptionJob"].clone();
            assert_eq!(
                job["TranscriptionJobName"],
                json!(name),
                "case {case:?}: the answer must name the job it started, got {:?}",
                job["TranscriptionJobName"]
            );
            // The members the request named read back unchanged through the reader.
            let fetched = get(&transcribe, &name).await;
            let stored = fetched.ok(case)["TranscriptionJob"].clone();
            reports_members(case, &stored, &row.members);
            accepted_names.push(name);
            continue;
        }

        reply.refused(case, 400, "BadRequestException");
        if let Some(companion) = row.names_companion {
            let message = reply.error_message(case);
            assert!(
                message.contains(companion),
                "case {case:?}: the refusal must name the missing {companion}, got \
                 {message:?}"
            );
        }
        // A refused start stores nothing, so the name it asked for is unknown.
        get(&transcribe, &name)
            .await
            .refused(case, 400, "BadRequestException");
    }

    // Only the accepted rows reached the store.
    let listed = transcribe
        .call("ListTranscriptionJobs", json!({"MaxResults": 100}))
        .await;
    let mut listed_names: Vec<String> = listed.ok("list every stored job")
        ["TranscriptionJobSummaries"]
        .as_array()
        .expect("TranscriptionJobSummaries must be a list")
        .iter()
        .map(|summary| {
            summary["TranscriptionJobName"]
                .as_str()
                .expect("every summary names its job")
                .to_string()
        })
        .collect();
    listed_names.sort();
    accepted_names.sort();
    assert_eq!(
        listed_names, accepted_names,
        "exactly the accepted starts may be stored, got {listed_names:?}"
    );
}

/// Where a `StartTranscriptionJob` request can NAME a custom resource: the members
/// that apply the resource `name`, the family that name must belong to, and how this
/// account comes to hold it.
struct Reference {
    case: &'static str,
    /// The word this service's not-found text names the family by.
    kind: &'static str,
    /// The request members that apply the resource called `name`.
    members: fn(&str) -> Value,
    /// Makes this account hold the resource `name` in the family the member reads.
    create: fn(&str) -> (&'static str, Value),
}

/// Every member path of this request that names a custom resource, one row each, over
/// all three families those names belong to.
fn references() -> Vec<Reference> {
    vec![
        Reference {
            case: "Settings.VocabularyName",
            kind: "vocabulary",
            members: |name| json!({"Settings": {"VocabularyName": name}}),
            create: |name| {
                (
                    "CreateVocabulary",
                    json!({
                        "VocabularyName": name,
                        "LanguageCode": "en-US",
                        "Phrases": ["Aardvark"]
                    }),
                )
            },
        },
        Reference {
            case: "Settings.VocabularyFilterName",
            kind: "vocabulary filter",
            members: |name| {
                json!({"Settings": {
                    "VocabularyFilterName": name,
                    "VocabularyFilterMethod": "remove"
                }})
            },
            create: |name| {
                (
                    "CreateVocabularyFilter",
                    json!({
                        "VocabularyFilterName": name,
                        "LanguageCode": "en-US",
                        "Words": ["crud"]
                    }),
                )
            },
        },
        Reference {
            case: "ModelSettings.LanguageModelName",
            kind: "language model",
            members: |name| json!({"ModelSettings": {"LanguageModelName": name}}),
            create: |name| {
                (
                    "CreateLanguageModel",
                    json!({
                        "ModelName": name,
                        "LanguageCode": "en-US",
                        "BaseModelName": "NarrowBand",
                        "InputDataConfig": {
                            "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                            "DataAccessRoleArn": ROLE_ARN
                        }
                    }),
                )
            },
        },
        Reference {
            case: "LanguageIdSettings[en-US].VocabularyName",
            kind: "vocabulary",
            members: |name| {
                json!({
                    "LanguageCode": Value::Null,
                    "IdentifyLanguage": true,
                    "LanguageIdSettings": {"en-US": {"VocabularyName": name}}
                })
            },
            create: |name| {
                (
                    "CreateVocabulary",
                    json!({
                        "VocabularyName": name,
                        "LanguageCode": "en-US",
                        "Phrases": ["Aardvark"]
                    }),
                )
            },
        },
        Reference {
            case: "LanguageIdSettings[es-US].VocabularyFilterName",
            kind: "vocabulary filter",
            // The per-language member documents the same co-requirement with
            // `VocabularyFilterMethod` the top-level one does, and the map models no
            // method of its own, so the row carries it in `Settings`: what is under
            // test is the family lookup, not that rule.
            members: |name| {
                json!({
                    "LanguageCode": Value::Null,
                    "IdentifyMultipleLanguages": true,
                    "Settings": {"VocabularyFilterMethod": "mask"},
                    "LanguageIdSettings": {"es-US": {"VocabularyFilterName": name}}
                })
            },
            create: |name| {
                (
                    "CreateVocabularyFilter",
                    json!({
                        "VocabularyFilterName": name,
                        "LanguageCode": "es-US",
                        "Words": ["crud"]
                    }),
                )
            },
        },
        Reference {
            case: "LanguageIdSettings[en-US].LanguageModelName",
            kind: "language model",
            members: |name| {
                json!({
                    "LanguageCode": Value::Null,
                    "IdentifyLanguage": true,
                    "LanguageIdSettings": {"en-US": {"LanguageModelName": name}}
                })
            },
            create: |name| {
                (
                    "CreateLanguageModel",
                    json!({
                        "ModelName": name,
                        "LanguageCode": "en-US",
                        "BaseModelName": "WideBand",
                        "InputDataConfig": {
                            "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                            "DataAccessRoleArn": ROLE_ARN
                        }
                    }),
                )
            },
        },
    ]
}

/// How this service words "there is no such resource", for a resource of `kind` — most
/// families' not-found refusal carries the sentence below ("The requested job couldn't
/// be found. Check the job name and try your request again." in the recorded
/// responses), naming the KIND and never the name the caller asked about.
///
/// The wording belongs to the FAMILY: the Call Analytics category family words the same
/// condition its own way, which its refusal suite
/// (`crate::call_analytics_category_refusals`) pins, so that family's sentence is read
/// from there rather than composed here. The custom MEDICAL vocabulary family shares
/// the plain vocabulary family's sentence — the recorded refusal for an absent medical
/// vocabulary reads "The requested vocabulary couldn't be found. Check the vocabulary
/// name and try your request again.", with no "medical" in either clause.
///
/// `pub(crate)` so the Call Analytics start suite
/// (`crate::call_analytics_job_start_limits`) asserts the same wording from one home
/// rather than restating it.
pub(crate) fn not_found_message(kind: &str) -> String {
    if kind == "category" {
        return crate::call_analytics_category_refusals::ABSENT_CATEGORY_MESSAGE.to_string();
    }
    let kind = if kind == "medical vocabulary" {
        "vocabulary"
    } else {
        kind
    };
    format!("The requested {kind} couldn't be found. Check the {kind} name and try your request again.")
}

#[tokio::test]
async fn a_start_naming_a_custom_resource_this_account_lacks_is_refused() {
    // The name forms the members' own `@pattern` (`^[0-9a-zA-Z._-]+$`) admits, at both
    // published `@length` bounds (1..200).
    let longest = "z".repeat(200);
    let names: Vec<&str> = vec!["never-created", "a", "terms.v2_9", &longest];

    for (row, reference) in references().iter().enumerate() {
        // A fresh emulator per member path, so a resource one row created cannot be
        // what makes the next row's start succeed.
        let transcribe = Transcribe::new();
        for (index, resource) in names.iter().enumerate() {
            let case = format!("{} naming the unheld {resource:?}", reference.case);
            let name = format!("dangling-{row}-{index}");
            let refused = transcribe
                .call(
                    "StartTranscriptionJob",
                    start_request(&name, &(reference.members)(resource)),
                )
                .await;
            refused.refused(&case, 400, "BadRequestException");
            let message = refused.error_message(&case);
            assert_eq!(
                message,
                not_found_message(reference.kind),
                "case {case:?}: unexpected refusal message, got {message:?}"
            );
            // A refused start stores nothing, so the name it asked for is unknown.
            get(&transcribe, &name)
                .await
                .refused(&case, 400, "BadRequestException");

            // The same request, once this account holds the resource, is served and
            // reads the settings back.
            let held = format!("held-{row}-{index}");
            let (operation, input) = (reference.create)(resource);
            transcribe
                .call(operation, input)
                .await
                .ok(&format!("{operation} {resource:?}"));
            // A held custom vocabulary is applicable only once its build has finished:
            // READY is what makes one usable. A custom language model is the same rule
            // on its own member: COMPLETED is what makes one usable.
            if operation == "CreateVocabulary" {
                transcribe.vocabulary_ready(resource, None).await;
            }
            if operation == "CreateLanguageModel" {
                transcribe.language_model_completed(resource, None).await;
            }
            let members = (reference.members)(resource);
            transcribe
                .call("StartTranscriptionJob", start_request(&held, &members))
                .await
                .ok(&case);
            let stored = get(&transcribe, &held).await;
            let job = stored.ok(&case)["TranscriptionJob"].clone();
            reports_members(&case, &job, &members);
        }
    }
}

#[tokio::test]
async fn a_custom_resource_of_another_family_does_not_stand_in() {
    let transcribe = Transcribe::new();

    // One name, held in exactly one family at a time: a member that reads a different
    // family must still refuse it.
    let name = "shared-name";
    create_vocabulary(&transcribe, name).await;

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a custom vocabulary where a vocabulary FILTER is named",
            json!({"Settings": {
                "VocabularyFilterName": name,
                "VocabularyFilterMethod": "mask"
            }}),
            "vocabulary filter",
        ),
        (
            "a custom vocabulary where a language MODEL is named",
            json!({"ModelSettings": {"LanguageModelName": name}}),
            "language model",
        ),
    ];

    for (index, (case, members, kind)) in rows.iter().enumerate() {
        let job = format!("wrong-family-{index}");
        let refused = transcribe
            .call("StartTranscriptionJob", start_request(&job, members))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_found_message(kind),
            "case {case:?}: unexpected refusal message, got {message:?}"
        );
        get(&transcribe, &job)
            .await
            .refused(case, 400, "BadRequestException");
    }

    // Held in the right family too, the same name is accepted by each member.
    create_vocabulary_filter(&transcribe, name).await;
    create_language_model(&transcribe, name).await;
    for (index, (case, members, _)) in rows.iter().enumerate() {
        let job = format!("right-family-{index}");
        transcribe
            .call("StartTranscriptionJob", start_request(&job, members))
            .await
            .ok(case);
    }
}

/// The media locations the rows of
/// [`every_job_family_refuses_a_start_naming_a_vocabulary_it_lacks`] name, staged in the
/// object store that suite's emulator mounts: a start whose media the store does not
/// hold is refused for the media, which is another suite's subject
/// (`crate::media_object_resolution`).
const STAGED_MEDIA: &[&str] = &[
    "s3://example-media/audio.flac",
    "s3://example-media/dictation.wav",
    "s3://example-media/call.flac",
    "s3://example-media/consult.flac",
];

/// One row of [`every_job_family_refuses_a_start_naming_a_vocabulary_it_lacks`]: a
/// start operation of this service, the request that applies the custom vocabulary
/// `name`, and the family that name must be held in.
struct JobFamily {
    case: &'static str,
    start: &'static str,
    request: fn(&str, &str) -> Value,
    /// The create that makes this account hold the vocabulary in the family this
    /// operation's settings read.
    create: &'static str,
    kind: &'static str,
}

#[tokio::test]
async fn every_job_family_refuses_a_start_naming_a_vocabulary_it_lacks() {
    let families: Vec<JobFamily> = vec![
        JobFamily {
            case: "transcription job",
            start: "StartTranscriptionJob",
            request: |job, vocabulary| {
                json!({
                    "TranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                    "Settings": {"VocabularyName": vocabulary}
                })
            },
            create: "CreateVocabulary",
            kind: "vocabulary",
        },
        JobFamily {
            case: "medical transcription job",
            start: "StartMedicalTranscriptionJob",
            request: |job, vocabulary| {
                json!({
                    "MedicalTranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION",
                    "Settings": {"VocabularyName": vocabulary}
                })
            },
            create: "CreateMedicalVocabulary",
            kind: "medical vocabulary",
        },
        JobFamily {
            case: "Call Analytics job",
            start: "StartCallAnalyticsJob",
            request: |job, vocabulary| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                    "Settings": {"VocabularyName": vocabulary}
                })
            },
            create: "CreateVocabulary",
            kind: "vocabulary",
        },
        JobFamily {
            case: "Medical Scribe job",
            start: "StartMedicalScribeJob",
            request: |job, vocabulary| {
                json!({
                    "MedicalScribeJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {
                        "ShowSpeakerLabels": true,
                        "MaxSpeakerLabels": 2,
                        "VocabularyName": vocabulary
                    }
                })
            },
            create: "CreateVocabulary",
            kind: "vocabulary",
        },
    ];

    for (index, family) in families.iter().enumerate() {
        // A fresh emulator per family, so one family's vocabulary cannot be what makes
        // the next family's start succeed. With the object store mounted beside the
        // service — a custom MEDICAL vocabulary's terms come only from a table file it
        // serves, so only there can one of that family be built — the media each row
        // names is staged first, or the start would be refused for its media instead.
        let transcribe = Transcribe::with_object_store();
        transcribe.given_media_objects(STAGED_MEDIA).await;
        let case = family.case;
        let vocabulary = format!("family-terms-{index}");

        let refused = transcribe
            .call(family.start, (family.request)("dangling-job", &vocabulary))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_found_message(family.kind),
            "case {case:?}: unexpected refusal message, got {message:?}"
        );

        // The vocabulary the operation's own family reads, then the same start. A held
        // custom vocabulary of EITHER family is applicable only once its build has
        // finished: READY is what makes one usable, so each create is waited out.
        if family.create == "CreateVocabulary" {
            transcribe
                .call(
                    family.create,
                    json!({
                        "VocabularyName": vocabulary,
                        "LanguageCode": "en-US",
                        "Phrases": ["Aardvark"]
                    }),
                )
                .await
                .ok(&format!("{} {vocabulary:?}", family.create));
            transcribe.vocabulary_ready(&vocabulary, None).await;
        } else {
            transcribe.medical_vocabulary_ready(&vocabulary).await;
        }
        transcribe
            .call(family.start, (family.request)("held-job", &vocabulary))
            .await
            .ok(case);
    }
}
