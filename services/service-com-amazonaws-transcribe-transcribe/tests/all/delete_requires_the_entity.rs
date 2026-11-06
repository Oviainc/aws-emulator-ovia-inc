// SPDX-License-Identifier: CC0-1.0

//! THE RULE THIS SUITE IS ABOUT: in this service a delete REQUIRES the entity it
//! names. A delete of a name this account holds no resource of that family under is
//! refused with the service's absent-resource client error — the shape whose own model
//! documentation covers it ("Your request didn't pass one or more validation tests.
//! This can occur when the entity you're trying to delete doesn't exist or if it's in
//! a non-terminal state (such as IN PROGRESS)", `BadRequestException`) — the refusal
//! names the KIND of resource rather than echoing the request, and nothing is created
//! or removed. Exactly one delete of such a resource succeeds.
//!
//! `DeleteMedicalVocabulary` is the one family this does NOT hold for: its recorded
//! answer for a name the account holds nothing under is an empty 200, so it is
//! idempotent about presence and its rows live in `crate::medical_vocabulary_delete`
//! rather than here.
//!
//! The table spans the whole delete class rather than one operation: one row per
//! family whose delete this emulator serves and refuses an absent name (custom
//! vocabulary, vocabulary filter, custom language model, transcription job, Call
//! Analytics category, Call Analytics job, Medical Scribe job), crossed with the forms
//! of name the name members' shared
//! `@length(1..200)` / `@pattern(^[0-9a-zA-Z._-]+$)` constraint allows
//! (`smithy_query(query="operation", operation="DeleteVocabulary")`): the shortest
//! allowed name, every allowed punctuation character, a plainly spelled one, and a
//! name at the published 200-character ceiling.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for every clock-owed transition of a resource created here to have
/// fallen due, so a delete under test is answered for the resource's PRESENCE rather
/// than refused for a non-terminal state: the longest chain in this service is a
/// transcription job's `QUEUED -> IN_PROGRESS -> COMPLETED`, two 500 ms spans, with
/// room for a slow machine.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_500);

/// The IAM role a create that reads an Amazon S3 location names, in the form
/// `DataAccessRoleArn`'s `@pattern` pins.
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// The status this service puts on every client error
/// (`crate::scaffolded::wire_status`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error shape this service's absent-resource refusals carry.
const ABSENT_RESOURCE_SHAPE: &str = "BadRequestException";

/// One family whose delete this suite drives: the create that establishes a resource,
/// the reader that proves presence, the delete under test, and the word this service's
/// refusals name the family by.
struct Deletable {
    /// The word the refusal names this family by, as the service's own messages spell
    /// it ("job" for every job family, "category", "language model", …).
    kind: &'static str,
    create: &'static str,
    read: &'static str,
    delete: &'static str,
    /// The member each of those operations names the resource with.
    name_member: &'static str,
    /// This family's create request for a resource named `name`.
    create_body: fn(&str) -> Value,
}

/// Every family whose delete this emulator serves.
fn deletables() -> Vec<Deletable> {
    vec![
        Deletable {
            kind: "vocabulary",
            create: "CreateVocabulary",
            read: "GetVocabulary",
            delete: "DeleteVocabulary",
            name_member: "VocabularyName",
            create_body: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark", "Transcribe"]
                })
            },
        },
        Deletable {
            kind: "vocabulary filter",
            create: "CreateVocabularyFilter",
            read: "GetVocabularyFilter",
            delete: "DeleteVocabularyFilter",
            name_member: "VocabularyFilterName",
            create_body: |name| {
                json!({
                    "VocabularyFilterName": name,
                    "LanguageCode": "en-US",
                    "Words": ["crud", "bogus"]
                })
            },
        },
        Deletable {
            kind: "language model",
            create: "CreateLanguageModel",
            read: "DescribeLanguageModel",
            delete: "DeleteLanguageModel",
            name_member: "ModelName",
            create_body: |name| {
                json!({
                    "ModelName": name,
                    "LanguageCode": "en-US",
                    "BaseModelName": "WideBand",
                    "InputDataConfig": {
                        "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                        "DataAccessRoleArn": ROLE_ARN
                    }
                })
            },
        },
        Deletable {
            kind: "job",
            create: "StartTranscriptionJob",
            read: "GetTranscriptionJob",
            delete: "DeleteTranscriptionJob",
            name_member: "TranscriptionJobName",
            create_body: |name| {
                json!({
                    "TranscriptionJobName": name,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
                })
            },
        },
        Deletable {
            kind: "job",
            create: "StartMedicalTranscriptionJob",
            read: "GetMedicalTranscriptionJob",
            delete: "DeleteMedicalTranscriptionJob",
            name_member: "MedicalTranscriptionJobName",
            create_body: |name| {
                json!({
                    "MedicalTranscriptionJobName": name,
                    // "US English (en-US) is the only valid value for medical
                    // transcription jobs" (model doc for this request's LanguageCode).
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION"
                })
            },
        },
        Deletable {
            kind: "category",
            create: "CreateCallAnalyticsCategory",
            read: "GetCallAnalyticsCategory",
            delete: "DeleteCallAnalyticsCategory",
            name_member: "CategoryName",
            create_body: |name| {
                json!({
                    "CategoryName": name,
                    "Rules": [{
                        "NonTalkTimeFilter": {
                            "Threshold": 10_000,
                            "RelativeTimeRange": {"First": 30},
                            "Negate": false
                        }
                    }]
                })
            },
        },
        Deletable {
            kind: "job",
            create: "StartCallAnalyticsJob",
            read: "GetCallAnalyticsJob",
            delete: "DeleteCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
            create_body: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                })
            },
        },
        Deletable {
            kind: "job",
            create: "StartMedicalScribeJob",
            read: "GetMedicalScribeJob",
            delete: "DeleteMedicalScribeJob",
            name_member: "MedicalScribeJobName",
            create_body: |name| {
                json!({
                    "MedicalScribeJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
                })
            },
        },
    ]
}

/// The forms of name every row is probed with, one per form the name members' shared
/// `@length(1..200)` / `@pattern(^[0-9a-zA-Z._-]+$)` constraint allows, including both
/// published length bounds.
fn name_forms() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest allowed name", "a".to_string()),
        (
            "every allowed punctuation character",
            "a.b_c-d.0_9".to_string(),
        ),
        ("a plainly spelled name", "never-created-at-all".to_string()),
        ("a name at the 200-character ceiling", "n".repeat(200)),
    ]
}

/// Assert `reply` is this service's absent-resource refusal for a resource of `kind`
/// asked about by `name`: the client-error status, the modelled shape in the body's
/// `__type`, a message naming the KIND of resource, and that `name` is not echoed back.
///
/// The shape and kind-naming half is the custom-vocabulary suite's own assertion of the
/// same rule (`crate::vocabulary_delete_and_tag_sets`), reused rather than spelled a
/// second time so the two cannot drift; what is local is the EXACT status that helper
/// leaves open and the no-echo check, which needs the name this row asked about.
///
/// `pub(crate)`: a per-family delete suite
/// (`crate::language_model_delete_requires_the_model`) asserts the same composition for
/// its own rows, so it calls this rather than keeping a second copy of it.
pub(crate) fn absent_resource_refusal(reply: &Reply, case: &str, kind: &str, name: &str) {
    crate::vocabulary_delete_and_tag_sets::absent_resource_refusal(reply, case, kind);
    reply.ok_status(case, CLIENT_ERROR_STATUS);
    // The name is the caller's: the text is the service's own wording, not a rendering
    // of the request. A name of three characters or fewer is skipped, since the wording
    // contains most short letter runs somewhere.
    if name.len() > 3 {
        let message = reply.error_message(case);
        assert!(
            !message.contains(name),
            "case {case:?}: the refusal must not echo the requested {name:?}, got \
             {message:?}"
        );
    }
}

/// RULE: a delete of a name this account holds no resource of that family under is the
/// absent-resource refusal, for every family and every form of name — and the refused
/// delete creates nothing, so the family's reader still refuses the name afterwards.
#[tokio::test]
async fn a_delete_of_a_resource_this_account_does_not_hold_is_refused() {
    let transcribe = Transcribe::new();

    for family in deletables() {
        for (name_case, name) in name_forms() {
            let case = format!("{} / {name_case}", family.delete);
            let request = json!({family.name_member: name});
            let reply = transcribe.call(family.delete, request.clone()).await;
            absent_resource_refusal(&reply, &case, family.kind, &name);

            // The refused delete MINTED nothing: the family's own reader still does not
            // find the name.
            transcribe
                .call(family.read, request)
                .await
                .refused(&case, CLIENT_ERROR_STATUS, ABSENT_RESOURCE_SHAPE);
        }
    }
}

/// RULE, the repeat axis: exactly ONE delete of a resource succeeds. The create is
/// read back through the family's own reader first, so the row proves the delete acted
/// on a resource that was really there; every delete after the first is the
/// absent-resource refusal.
#[tokio::test]
async fn exactly_one_delete_of_a_stored_resource_succeeds() {
    let transcribe = Transcribe::new();
    let families = deletables();
    let forms = name_forms();

    // Create every row first, so the one wait below covers every family's clock-owed
    // transition instead of one wait per row.
    for family in &families {
        for (name_case, name) in &forms {
            let case = format!("{} / {name_case}", family.create);
            transcribe
                .call(family.create, (family.create_body)(name))
                .await
                .ok_status(&case, 200);
        }
    }

    std::thread::sleep(PAST_EVERY_TRANSITION);

    for family in &families {
        for (name_case, name) in &forms {
            let case = format!("{} / {name_case}", family.delete);
            let request = json!({family.name_member: name});

            // The resource really is there before the delete.
            transcribe
                .call(family.read, request.clone())
                .await
                .ok(&case);

            let deleted = transcribe.call(family.delete, request.clone()).await;
            deleted.ok_status(&case, 200);
            assert!(
                matches!(&deleted.body, Value::Null)
                    || matches!(&deleted.body, Value::Object(members) if members.is_empty()),
                "case {case:?}: a delete answers no members, got {:?}",
                deleted.body
            );

            // It is gone, and every later delete of the same name is the refusal.
            transcribe
                .call(family.read, request.clone())
                .await
                .refused(&case, CLIENT_ERROR_STATUS, ABSENT_RESOURCE_SHAPE);
            for attempt in 2..=3 {
                let again = format!("{case} #{attempt}");
                let reply = transcribe.call(family.delete, request.clone()).await;
                absent_resource_refusal(&reply, &again, family.kind, name);
            }
        }
    }
}
