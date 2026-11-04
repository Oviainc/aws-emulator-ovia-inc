// SPDX-License-Identifier: CC0-1.0

//! WHERE the Medical Scribe (AWS HealthScribe) operation family is served, end to end
//! over the wire.
//!
//! THE RULE UNDER TEST: an operation family AWS offers in only a SUBSET of a service's
//! Regions is refused in every other Region of that service — ahead of the operation's
//! own input validation and of any lookup of the resource the request names, with one
//! status, one error code and one entirely fixed sentence for every operation of the
//! family, and with nothing read from the store and nothing written to it. In a Region
//! the family IS offered in, every operation of it is served exactly as before, and the
//! families that ARE offered Region-wide behind the same endpoint are untouched.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the REGION the call is signed for — a commercial Region of this service the feature
//!   is not offered in, a three-part Region name, a Region of another partition, and
//!   every Region the feature IS offered in
//!   (`service::medical_scribe_availability::MEDICAL_SCRIBE_REGIONS`);
//! * the OPERATION — all four the family publishes, the three that name a job and the
//!   collection one that names none;
//! * the request FORM — one that would otherwise be served, two that would otherwise be
//!   refused by the operation's own validation, one naming a job another Region holds and
//!   one naming a job no Region ever held.
//!
//! Every call goes through the same `MultiServiceRouter` the production router dispatches
//! on, reusing the transcription-job suite's harness (`crate::transcription_jobs`) rather
//! than standing up a second emulator, so these rows exercise the awsJson1_1 envelope and
//! the boundary constraint validation too.

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::service::medical_scribe_availability::{
    ACCOUNT_NOT_AUTHORIZED, MEDICAL_SCRIBE_REGIONS,
};

use crate::transcription_jobs::Transcribe;

/// Long enough for the clock-owed `IN_PROGRESS -> COMPLETED` of a job started during a row
/// to have fallen due: one span of `crate::state::JOB_TRANSITION_DELAY` (500 ms), with room
/// for a slow machine — the same wait the sibling Medical Scribe suites take.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt` and
/// `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_THE_TRANSITION: std::time::Duration = std::time::Duration::from_millis(900);

/// The modelled client error the family answers a call it may not serve with, and the
/// status this awsJson1_1 service puts on every client error (both pinned by
/// `crate::client_error_wire_status`).
const REFUSAL_CODE: &str = "BadRequestException";
/// See [`REFUSAL_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// Regions this service has an endpoint in that the Medical Scribe family is NOT offered
/// in: a commercial Region of the same partition, a three-part Region name, another
/// commercial Region, and a Region outside the commercial partition — four forms a Region
/// string takes, so no row depends on the shape of one Region name.
const UNSERVED_REGIONS: &[&str] = &["us-west-2", "ap-southeast-3", "eu-central-1", "us-gov-west-1"];

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// A `StartMedicalScribeJob` request for `name` that every check of the operation passes:
/// it names its media, exactly one of the two speaker settings and the companion that
/// setting requires.
fn valid_start(name: &str) -> Value {
    json!({
        "MedicalScribeJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
        "OutputBucketName": "example-scribe-output",
        "DataAccessRoleArn": ROLE_ARN,
        "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
    })
}

/// Every call of the family a row makes, as `(case, operation, request)`: the four
/// operations, over request forms the operation's own validation would answer differently
/// — so a refusal that is the same for all of them cannot have come from any per-operation
/// check.
///
/// `held_elsewhere` is the name of a job a Region that DOES serve the family holds, so one
/// read and one delete name a job that exists in this account.
fn calls(held_elsewhere: &str) -> Vec<(String, &'static str, Value)> {
    vec![
        (
            "a read of a job name no Region ever held".to_string(),
            "GetMedicalScribeJob",
            json!({"MedicalScribeJobName": "never-created"}),
        ),
        (
            format!("a read of {held_elsewhere:?}, which another Region holds"),
            "GetMedicalScribeJob",
            json!({"MedicalScribeJobName": held_elsewhere}),
        ),
        (
            "a start every check of the operation passes".to_string(),
            "StartMedicalScribeJob",
            valid_start("gated-start"),
        ),
        (
            "a start whose Media names no media file".to_string(),
            "StartMedicalScribeJob",
            json!({
                "MedicalScribeJobName": "gated-start-without-media",
                "Media": {},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
            }),
        ),
        (
            "a start whose Settings set neither speaker setting".to_string(),
            "StartMedicalScribeJob",
            json!({
                "MedicalScribeJobName": "gated-start-without-settings",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {}
            }),
        ),
        (
            "a listing naming no job at all".to_string(),
            "ListMedicalScribeJobs",
            json!({}),
        ),
        (
            "a listing resumed with a cursor this service never handed out".to_string(),
            "ListMedicalScribeJobs",
            json!({"NextToken": "ListMedicalScribeJobs|us-east-1|1|forged"}),
        ),
        (
            "a delete of a job name no Region ever held".to_string(),
            "DeleteMedicalScribeJob",
            json!({"MedicalScribeJobName": "never-created"}),
        ),
        (
            format!("a delete of {held_elsewhere:?}, which another Region holds"),
            "DeleteMedicalScribeJob",
            json!({"MedicalScribeJobName": held_elsewhere}),
        ),
    ]
}

/// The Region the family is offered in that every row creates its control record in.
fn served_region() -> &'static str {
    MEDICAL_SCRIBE_REGIONS
        .first()
        .expect("the feature is offered in at least one Region")
}

/// RULE: in a Region the family is not offered in, all four operations answer the one
/// fixed refusal — whatever the request named, and whether or not that job exists.
#[tokio::test]
async fn every_operation_of_the_family_is_refused_where_it_is_not_offered() {
    let transcribe = Transcribe::new();
    let held = "held-where-the-family-is-offered";
    let serving = served_region();
    transcribe
        .call_in_region("StartMedicalScribeJob", valid_start(held), Some(serving))
        .await
        .ok(&format!("the control job started in {serving}"));

    for region in UNSERVED_REGIONS {
        for (form, operation, request) in calls(held) {
            let case = format!("{operation} in {region}: {form}");
            let reply = transcribe
                .call_in_region(operation, request, Some(region))
                .await;
            reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
            assert_eq!(
                reply.error_message(&case),
                ACCOUNT_NOT_AUTHORIZED,
                "case {case:?}: expected the family's fixed refusal sentence, got {:?}",
                reply.body
            );
            // The sentence is the whole answer: a refusal about the call the account may
            // not make echoes nothing of what the request named and nothing of the store.
            let mut members: Vec<&str> = reply
                .body
                .as_object()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a JSON object, got {:?}", reply.body)
                })
                .keys()
                .map(String::as_str)
                .collect();
            members.sort_unstable();
            assert_eq!(
                members,
                vec!["Message", "__type"],
                "case {case:?}: the refusal must carry the code and the sentence alone, \
                 got {:?}",
                reply.body
            );
        }
    }
}

/// RULE, the store side: a refused call reads nothing and writes nothing — the Region that
/// serves the family still holds exactly the one job it started, and every name a refused
/// start carried is held nowhere.
#[tokio::test]
async fn a_refused_call_leaves_the_store_alone() {
    let transcribe = Transcribe::new();
    let held = "held-while-others-are-refused";
    let serving = served_region();
    transcribe
        .call_in_region("StartMedicalScribeJob", valid_start(held), Some(serving))
        .await
        .ok(&format!("the control job started in {serving}"));

    for region in UNSERVED_REGIONS {
        for (form, operation, request) in calls(held) {
            let case = format!("{operation} in {region}: {form}");
            transcribe
                .call_in_region(operation, request, Some(region))
                .await
                .refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
        }
    }

    let case = format!("the jobs {serving} holds after every refused call");
    let listed = transcribe
        .call_in_region("ListMedicalScribeJobs", json!({}), Some(serving))
        .await;
    let names: Vec<String> = listed.ok(&case)["MedicalScribeJobSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a page, got {:?}", listed.body))
        .iter()
        .map(|summary| {
            summary["MedicalScribeJobName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        vec![held.to_string()],
        "case {case:?}: the serving Region must still hold exactly its own job, got \
         {names:?}"
    );

    // The name every refused start carried is held nowhere: the read in the Region that
    // serves the family answers its OWN not-found refusal, whose sentence is a different
    // one — which is what tells the two conditions apart.
    for name in ["gated-start", "gated-start-without-media", "gated-start-without-settings"] {
        let case = format!("{name:?} read in {serving} after the refused start");
        let reply = transcribe
            .call_in_region(
                "GetMedicalScribeJob",
                json!({"MedicalScribeJobName": name}),
                Some(serving),
            )
            .await;
        reply.refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
        assert_ne!(
            reply.error_message(&case),
            ACCOUNT_NOT_AUTHORIZED,
            "case {case:?}: a Region that serves the family refuses an absent job with \
             its own sentence, got {:?}",
            reply.body
        );
    }
}

/// RULE, the other side: every Region the family IS offered in serves all four operations
/// — the gate decides where the family is served, never what a served call answers.
#[tokio::test]
async fn the_family_is_served_in_every_region_it_is_offered_in() {
    for region in MEDICAL_SCRIBE_REGIONS {
        let transcribe = Transcribe::new();
        let name = "served-round-trip";
        let case = format!("the family in {region}");

        let started = transcribe
            .call_in_region("StartMedicalScribeJob", valid_start(name), Some(region))
            .await;
        assert_eq!(
            started.ok(&case)["MedicalScribeJob"]["MedicalScribeJobName"],
            json!(name),
            "case {case:?}: the start must answer the record it stored, got {:?}",
            started.body
        );

        let fetched = transcribe
            .call_in_region(
                "GetMedicalScribeJob",
                json!({"MedicalScribeJobName": name}),
                Some(region),
            )
            .await;
        assert_eq!(
            fetched.ok(&case)["MedicalScribeJob"]["MedicalScribeJobName"],
            json!(name),
            "case {case:?}: the read must report the stored job, got {:?}",
            fetched.body
        );

        let listed = transcribe
            .call_in_region("ListMedicalScribeJobs", json!({}), Some(region))
            .await;
        assert_eq!(
            listed.ok(&case)["MedicalScribeJobSummaries"][0]["MedicalScribeJobName"],
            json!(name),
            "case {case:?}: the listing must report the stored job, got {:?}",
            listed.body
        );

        // A job is deletable once the service has finished with it.
        std::thread::sleep(PAST_THE_TRANSITION);
        transcribe
            .call_in_region(
                "DeleteMedicalScribeJob",
                json!({"MedicalScribeJobName": name}),
                Some(region),
            )
            .await
            .ok_status(&case, 200);
    }
}

/// RULE, the scope of the gate: it names one family. The families this service offers
/// Region-wide are served in the very Regions the Medical Scribe family is refused in.
#[tokio::test]
async fn the_other_families_behind_the_same_endpoint_are_still_served() {
    for region in UNSERVED_REGIONS {
        let transcribe = Transcribe::new();
        for (operation, request) in [
            (
                "StartTranscriptionJob",
                json!({
                    "TranscriptionJobName": "sibling-job",
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
                }),
            ),
            (
                "StartMedicalTranscriptionJob",
                json!({
                    "MedicalTranscriptionJobName": "sibling-medical-job",
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION"
                }),
            ),
            (
                "StartCallAnalyticsJob",
                json!({
                    "CallAnalyticsJobName": "sibling-analytics-job",
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                }),
            ),
            (
                "CreateVocabulary",
                json!({
                    "VocabularyName": "sibling-vocabulary",
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark"]
                }),
            ),
            ("ListVocabularies", json!({})),
            ("ListTranscriptionJobs", json!({})),
        ] {
            let case = format!("{operation} in {region}");
            transcribe
                .call_in_region(operation, request, Some(region))
                .await
                .ok(&case);
        }
    }
}
