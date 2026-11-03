// SPDX-License-Identifier: CC0-1.0

//! The SENTENCES this service refuses with, end to end over the wire: the text that
//! comes back with a refusal, for the classes of refusal whose condition the Smithy
//! model itself cannot carry (`BadRequestException` covers an absent entity, a
//! non-terminal state AND a missing field; `ConflictException` covers a taken name), so
//! the message is the only thing that tells a client which of them it got.
//!
//! THE RULE THESE ROWS ASSERT: a refusal carries the sentence this service publishes
//! for that error CLASS, and that sentence is FIXED — it names the resource KIND and
//! the condition, and interpolates nothing from the request or from the stored record,
//! so the same class of request is refused with byte-identical text whatever it named
//! and whatever state the record it names was in. Four classes, one test each:
//!
//! 1. a create or an update that says nowhere its content comes from (neither of the
//!    two alternatives its family publishes);
//! 2. a start that says nothing about the language of its media;
//! 3. a delete of a resource the service has not finished processing;
//! 4. a create of a name that is already taken — where the sentence depends on WHAT
//!    holds the name: a create this emulator is still serving, or a resource whose own
//!    create has already been answered.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these rows exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler bodies.
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time;
//! only the tests that need to observe a create's own name claim assemble their own
//! emulator, because they have to hold the engine that claim lives in.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::state::{ResourceFamily, DEFAULT_REGION};
use smithy_server_runtime::MultiServiceRouter;

use crate::transcription_jobs::{dispatch, Reply, Transcribe};

/// Long enough for one clock-owed transition of a record created during the test to
/// have fallen due: one span of `crate::state::VOCABULARY_TRANSITION_DELAY` /
/// `crate::state::JOB_TRANSITION_DELAY` (500 ms each), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The status code every client error of this awsJson1_1 service answers on the wire.
const CLIENT_ERROR_STATUS: u16 = 400;

/// How this service refuses a `CreateVocabulary`/`UpdateVocabulary` that names neither
/// `Phrases` nor `VocabularyFileUri`.
const VOCABULARY_SOURCE_REQUIRED: &str =
    "Either Phrases or VocabularyFileUri field should be provided.";

/// How this service refuses a `CreateVocabularyFilter`/`UpdateVocabularyFilter` that
/// names neither `Words` nor `VocabularyFilterFileUri`.
const FILTER_SOURCE_REQUIRED: &str = "You need to provide either the Words field or the \
     VocabularyFilterFileUri field. Provide one of the fields and try your request \
     again.";

/// How this service refuses a start that says nothing about the language of its media.
const LANGUAGE_MISSING: &str = "The language code is missing. Either add a language \
     code or turn on language identification.";

/// How this service refuses a start that asks for more than one determination of that
/// language at once — a different condition, so a different sentence.
const LANGUAGE_OVERSPECIFIED: &str = "Your request must include exactly one of \
     LanguageCode, IdentifyLanguage, or IdentifyMultipleLanguages. Correct your request \
     and try again.";

/// How this service refuses a delete of a job it has not finished working on.
const JOB_NOT_DELETABLE: &str =
    "The job couldn't be deleted. Make sure it exists and isn't pending.";

/// How the vocabulary families word that same condition: they name the two states a
/// build has finished in, which are the terminal values of the `VocabularyState` enum
/// (`smithy_query(query="shape", shape="VocabularyState")` lists `PENDING`, `READY` and
/// `FAILED`). The apostrophe of "you’re" is U+2019, as this service writes it, not the
/// ASCII `'`.
fn vocabulary_not_deletable(kind: &str) -> String {
    format!(
        "The {kind} that you\u{2019}re trying to delete isn't in the READY or FAILED \
         state. Try your request again when the {kind} state is READY or FAILED."
    )
}

/// How this service words a name a create finds held by a resource that is already
/// there — the family's own sentence, for a resource of `kind`.
fn name_taken(kind: &str) -> String {
    format!("The requested {kind} name already exists. Use a different {kind} name.")
}

/// How this service words a name a create finds claimed by ANOTHER CREATE this
/// emulator has not finished serving: its generic sentence, whichever family the
/// create belongs to.
fn name_contended() -> String {
    name_taken("job")
}

/// Assert `reply` is the modelled `BadRequestException` carrying exactly `expected`,
/// and that nothing of `forbidden` — the values the request and the record carry —
/// leaked into it.
fn refused_with(reply: &Reply, case: &str, expected: &str, forbidden: &[&str]) {
    reply.refused(case, CLIENT_ERROR_STATUS, "BadRequestException");
    let message = reply.error_message(case);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the published refusal {expected:?}, got {message:?}"
    );
    for value in forbidden {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not carry the request or record value \
             {value:?}, got {message:?}"
        );
    }
}

/// RULE 1: a create or an update that names neither of the two alternatives its family
/// publishes for where its content comes from is refused with that family's own fixed
/// sentence.
///
/// The rows span what the rule generalises over: which family the request belongs to
/// (custom vocabulary, custom vocabulary filter), which verb claims the content (the
/// create, the update), the FORM "neither" takes (both members absent, an inline list
/// present but empty), the language the request names, and the length of the name it
/// uses — including both published bounds of the name member (`length {min 1, max
/// 200}`, `pattern ^[0-9a-zA-Z._-]+$`).
#[tokio::test]
async fn a_request_naming_no_content_source_is_refused_with_its_family_sentence() {
    let transcribe = Transcribe::new();

    // The update rows need a record to update, and `UpdateVocabulary` publishes "your
    // custom vocabulary must be in a terminal state (READY or FAILED) before you can
    // update it", so both are created first and the one wait below settles the build.
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "settled-vocabulary",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("the vocabulary the update rows name");
    transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "settled-filter",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await
        .ok("the filter the update rows name");
    std::thread::sleep(PAST_THE_TRANSITION);

    let long_name = "v".repeat(200);
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a create naming neither Phrases nor VocabularyFileUri",
            "CreateVocabulary",
            json!({"VocabularyName": "sourceless", "LanguageCode": "en-US"}),
            VOCABULARY_SOURCE_REQUIRED,
        ),
        (
            "a create whose Phrases list is present but empty, in another language",
            "CreateVocabulary",
            json!({"VocabularyName": "q", "LanguageCode": "de-DE", "Phrases": []}),
            VOCABULARY_SOURCE_REQUIRED,
        ),
        (
            "a create at the 200-character name bound, naming no source",
            "CreateVocabulary",
            json!({"VocabularyName": long_name, "LanguageCode": "ja-JP"}),
            VOCABULARY_SOURCE_REQUIRED,
        ),
        (
            "an update of a settled vocabulary naming no source",
            "UpdateVocabulary",
            json!({"VocabularyName": "settled-vocabulary", "LanguageCode": "en-US"}),
            VOCABULARY_SOURCE_REQUIRED,
        ),
        (
            "an update whose Phrases list is present but empty",
            "UpdateVocabulary",
            json!({
                "VocabularyName": "settled-vocabulary",
                "LanguageCode": "en-GB",
                "Phrases": []
            }),
            VOCABULARY_SOURCE_REQUIRED,
        ),
        (
            "a filter create naming neither Words nor VocabularyFilterFileUri",
            "CreateVocabularyFilter",
            json!({"VocabularyFilterName": "wordless", "LanguageCode": "en-US"}),
            FILTER_SOURCE_REQUIRED,
        ),
        (
            "a filter create at the one-character name bound, in another language",
            "CreateVocabularyFilter",
            json!({"VocabularyFilterName": "w", "LanguageCode": "es-US"}),
            FILTER_SOURCE_REQUIRED,
        ),
        (
            "a filter update naming no source",
            "UpdateVocabularyFilter",
            json!({"VocabularyFilterName": "settled-filter"}),
            FILTER_SOURCE_REQUIRED,
        ),
    ];

    for (case, operation, request, expected) in rows {
        let name = request
            .get("VocabularyName")
            .or_else(|| request.get("VocabularyFilterName"))
            .and_then(Value::as_str)
            .expect("every row names its resource")
            .to_string();
        let refused = transcribe.call(operation, request).await;
        refused_with(&refused, case, expected, &[name.as_str()]);
    }

    // The refused creates stored nothing and the refused updates overwrote nothing: the
    // two settled records are still the ones their own creates stored.
    for (reader, member, name, state) in [
        (
            "GetVocabulary",
            "VocabularyName",
            "settled-vocabulary",
            "READY",
        ),
        (
            "GetVocabularyFilter",
            "VocabularyFilterName",
            "settled-filter",
            "",
        ),
    ] {
        let case = format!("{reader} after the refusals");
        let read = transcribe.call(reader, json!({member: name})).await;
        assert_eq!(
            read.ok(&case)[member].as_str(),
            Some(name),
            "case {case:?}: a refused request must leave the record as it was, got {:?}",
            read.body
        );
        if !state.is_empty() {
            assert_eq!(
                read.member(&case, "VocabularyState"),
                state,
                "case {case:?}: a refused update must not restart the build, got {:?}",
                read.body
            );
        }
    }
    for (reader, member, name) in [
        ("GetVocabulary", "VocabularyName", "sourceless"),
        ("GetVocabularyFilter", "VocabularyFilterName", "wordless"),
    ] {
        let read = transcribe.call(reader, json!({member: name})).await;
        read.refused(
            &format!("{reader} for the name a refused create named"),
            CLIENT_ERROR_STATUS,
            "BadRequestException",
        );
    }
}

/// RULE 2: a start whose request says nothing about the language of its media is
/// refused with this service's fixed missing-language sentence, while one that asks for
/// more than one determination at once is a different condition with its own sentence,
/// and a request that says it exactly once is SERVED.
///
/// The rows span what the rule generalises over: which of the three members could have
/// said it, whether an absent member or one explicitly turned off is what leaves it
/// unsaid, how many of them say it at once, and whether the request carries the
/// language options that only identification can restrict.
#[tokio::test]
async fn a_start_that_says_nothing_about_its_language_is_refused_with_one_sentence() {
    let transcribe = Transcribe::new();
    let media = json!({"MediaFileUri": "s3://example-media/audio.wav"});

    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "none of the three members present",
            json!({"TranscriptionJobName": "no-language", "Media": media}),
            LANGUAGE_MISSING,
        ),
        (
            "identification present and explicitly off",
            json!({
                "TranscriptionJobName": "identification-off",
                "Media": media,
                "IdentifyLanguage": false
            }),
            LANGUAGE_MISSING,
        ),
        (
            "multi-language identification present and explicitly off",
            json!({
                "TranscriptionJobName": "multi-identification-off",
                "Media": media,
                "IdentifyMultipleLanguages": false
            }),
            LANGUAGE_MISSING,
        ),
        (
            "both identification members present and off",
            json!({
                "TranscriptionJobName": "both-off",
                "Media": media,
                "IdentifyLanguage": false,
                "IdentifyMultipleLanguages": false
            }),
            LANGUAGE_MISSING,
        ),
        (
            "language options with nothing to restrict",
            json!({
                "TranscriptionJobName": "options-only",
                "Media": media,
                "LanguageOptions": ["en-US", "es-US"]
            }),
            LANGUAGE_MISSING,
        ),
        (
            "a language code AND identification asked for",
            json!({
                "TranscriptionJobName": "code-and-identification",
                "Media": media,
                "LanguageCode": "en-US",
                "IdentifyLanguage": true
            }),
            LANGUAGE_OVERSPECIFIED,
        ),
        (
            "both kinds of identification asked for at once",
            json!({
                "TranscriptionJobName": "both-identifications",
                "Media": media,
                "IdentifyLanguage": true,
                "IdentifyMultipleLanguages": true
            }),
            LANGUAGE_OVERSPECIFIED,
        ),
    ];

    for (case, request, expected) in refusals {
        let name = request["TranscriptionJobName"]
            .as_str()
            .expect("every row names its job")
            .to_string();
        let refused = transcribe.call("StartTranscriptionJob", request).await;
        refused_with(&refused, case, expected, &[name.as_str()]);

        // Nothing was started: the name the refused request named holds no job.
        let read = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        read.refused(case, CLIENT_ERROR_STATUS, "BadRequestException");
    }

    // The same requests with the language said exactly once are served, so the rows
    // above are refused for what they left unsaid and for nothing else.
    let served: Vec<(&str, Value)> = vec![
        (
            "a language code alone",
            json!({
                "TranscriptionJobName": "code-only",
                "Media": media,
                "LanguageCode": "en-US"
            }),
        ),
        (
            "identification alone",
            json!({
                "TranscriptionJobName": "identification-only",
                "Media": media,
                "IdentifyLanguage": true,
                "LanguageOptions": ["en-US", "es-US"]
            }),
        ),
        (
            "multi-language identification alone",
            json!({
                "TranscriptionJobName": "multi-identification-only",
                "Media": media,
                "IdentifyMultipleLanguages": true
            }),
        ),
    ];
    for (case, request) in served {
        transcribe
            .call("StartTranscriptionJob", request)
            .await
            .ok_status(case, 200);
    }
}

/// One family's delete, as RULE 3 drives it: what to create, how to read it back, how
/// to delete it, and the sentence that family refuses the delete of an unfinished
/// resource with.
struct DeleteRow {
    case: &'static str,
    create: &'static str,
    create_request: Value,
    read: &'static str,
    delete: &'static str,
    /// The request members the read and the delete address the record by.
    key: Value,
    /// The name the record is stored under, which the refusal must NOT carry.
    name: &'static str,
    /// The state or status the record is in while the delete is refused, which the
    /// refusal must not carry either.
    state: &'static str,
    expected: String,
}

/// RULE 3: a delete of a resource the service has not finished processing is refused
/// with that family's own fixed sentence — no name, no state — and removes nothing;
/// once the processing the clock owed has fallen due, the same delete goes through.
///
/// The rows span what the rule generalises over: the FAMILY the resource belongs to
/// (both vocabulary families, three job families), hence both of the two sentences and
/// both of the processing members they are about (`VocabularyState: PENDING`,
/// `TranscriptionJobStatus: IN_PROGRESS`), and the kind word each family names itself
/// by.
#[tokio::test]
async fn a_delete_of_an_unfinished_resource_is_refused_with_its_family_sentence() {
    let transcribe = Transcribe::new();
    let rows = vec![
        DeleteRow {
            case: "a custom vocabulary whose build is still owed",
            create: "CreateVocabulary",
            create_request: json!({
                "VocabularyName": "unfinished-vocabulary",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            read: "GetVocabulary",
            delete: "DeleteVocabulary",
            key: json!({"VocabularyName": "unfinished-vocabulary"}),
            name: "unfinished-vocabulary",
            state: "PENDING",
            expected: vocabulary_not_deletable("vocabulary"),
        },
        DeleteRow {
            case: "a custom medical vocabulary whose build is still owed",
            create: "CreateMedicalVocabulary",
            create_request: json!({
                "VocabularyName": "unfinished-medical-vocabulary",
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://example-vocabulary-tables/terms.txt"
            }),
            read: "GetMedicalVocabulary",
            delete: "DeleteMedicalVocabulary",
            key: json!({"VocabularyName": "unfinished-medical-vocabulary"}),
            name: "unfinished-medical-vocabulary",
            state: "PENDING",
            expected: vocabulary_not_deletable("medical vocabulary"),
        },
        DeleteRow {
            case: "a transcription job the service is still working on",
            create: "StartTranscriptionJob",
            create_request: json!({
                "TranscriptionJobName": "unfinished-job",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
            }),
            read: "GetTranscriptionJob",
            delete: "DeleteTranscriptionJob",
            key: json!({"TranscriptionJobName": "unfinished-job"}),
            name: "unfinished-job",
            state: "IN_PROGRESS",
            expected: JOB_NOT_DELETABLE.to_string(),
        },
        DeleteRow {
            case: "a medical transcription job the service is still working on",
            create: "StartMedicalTranscriptionJob",
            create_request: json!({
                "MedicalTranscriptionJobName": "unfinished-medical-job",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
            read: "GetMedicalTranscriptionJob",
            delete: "DeleteMedicalTranscriptionJob",
            key: json!({"MedicalTranscriptionJobName": "unfinished-medical-job"}),
            name: "unfinished-medical-job",
            state: "IN_PROGRESS",
            expected: JOB_NOT_DELETABLE.to_string(),
        },
        DeleteRow {
            case: "a Call Analytics job the service is still working on",
            create: "StartCallAnalyticsJob",
            create_request: json!({
                "CallAnalyticsJobName": "unfinished-analytics-job",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"}
            }),
            read: "GetCallAnalyticsJob",
            delete: "DeleteCallAnalyticsJob",
            key: json!({"CallAnalyticsJobName": "unfinished-analytics-job"}),
            name: "unfinished-analytics-job",
            state: "IN_PROGRESS",
            expected: JOB_NOT_DELETABLE.to_string(),
        },
    ];

    for row in &rows {
        transcribe
            .call(row.create, row.create_request.clone())
            .await
            .ok(row.case);
    }

    for row in &rows {
        let refused = transcribe.call(row.delete, row.key.clone()).await;
        refused_with(&refused, row.case, &row.expected, &[row.name, row.state]);

        // Refused, so nothing was removed: the record is still readable.
        transcribe
            .call(row.read, row.key.clone())
            .await
            .ok(row.case);
    }

    // Once the processing the clock owed each record has fallen due, the very same
    // delete is served — so each refusal above was about the state of its record.
    std::thread::sleep(PAST_THE_TRANSITION);
    for row in &rows {
        transcribe
            .call(row.delete, row.key.clone())
            .await
            .ok_status(row.case, 200);
    }
}

/// One emulator whose engine the test keeps, so it can observe the name claim a create
/// writes into this service's own store.
///
/// The service is mounted on a single shared engine behind the router, exactly as
/// `crate::transcription_jobs::Transcribe` mounts it; the only difference is that the
/// engine is handed back too.
fn emulator_and_engine() -> (Arc<MultiServiceRouter>, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_transcribe_transcribe::EmulatedService::new(Arc::clone(&engine)),
    );
    (Arc::new(router), engine)
}

/// RULE 4: a create of a taken name is refused with the sentence for WHAT holds that
/// name — this service's generic sentence while the claim belongs to a create it is
/// still serving, and the family's own sentence once that create has been answered.
///
/// The rows span the two families whose create claims a name and then still has work to
/// do before it answers, each driven through both states of the claim: open (the create
/// holding it has not finished) and closed (it has). The claim is opened directly in
/// the store here, which is exactly what a create that has not reached its answer yet
/// leaves behind; the racing test below reaches the same state by contending for it.
#[tokio::test]
async fn a_create_of_a_taken_name_is_refused_for_what_holds_the_name() {
    let (router, engine) = emulator_and_engine();
    let state = service_com_amazonaws_transcribe_transcribe::state::service_state(&*engine)
        .expect("the service's own state");

    let rows: Vec<(&str, &str, Value, ResourceFamily, &str, String)> = vec![
        (
            "a second CreateVocabulary of one name",
            "CreateVocabulary",
            json!({
                "VocabularyName": "claimed-vocabulary",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            ResourceFamily::Vocabulary,
            "claimed-vocabulary",
            name_taken("vocabulary"),
        ),
        (
            "a second CreateVocabularyFilter of one name",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "claimed-filter",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
            ResourceFamily::VocabularyFilter,
            "claimed-filter",
            name_taken("vocabulary filter"),
        ),
    ];

    for (case, operation, request, family, name, taken) in rows {
        dispatch(&router, operation, request.clone(), None)
            .await
            .ok_status(case, 200);

        // The create that claimed the name has been answered, so the name is held by
        // the resource it recorded: the family's own sentence.
        let settled = dispatch(&router, operation, request.clone(), None).await;
        settled.refused(case, CLIENT_ERROR_STATUS, "ConflictException");
        let message = settled.error_message(case);
        assert_eq!(
            message, taken,
            "case {case:?}: a name held by a recorded resource must be refused with \
             {taken:?}, got {message:?}"
        );

        // With a claim open on the same name — what a create that has not finished
        // being served leaves in the store — the same request is refused with this
        // service's generic sentence instead.
        state.lock().claim_name(family, DEFAULT_REGION, name);
        let contended = dispatch(&router, operation, request.clone(), None).await;
        contended.refused(case, CLIENT_ERROR_STATUS, "ConflictException");
        let message = contended.error_message(case);
        assert_eq!(
            message,
            name_contended(),
            "case {case:?}: a name claimed by a create still being served must be \
             refused with {:?}, got {message:?}",
            name_contended()
        );

        // And once that create finishes, the family's own sentence is back.
        state.lock().release_name_claim(family, DEFAULT_REGION, name);
        let settled_again = dispatch(&router, operation, request, None).await;
        settled_again.refused(case, CLIENT_ERROR_STATUS, "ConflictException");
        let message = settled_again.error_message(case);
        assert_eq!(
            message, taken,
            "case {case:?}: once the create is answered the family's own sentence is \
             back, got {message:?}"
        );
    }
}

/// RULE 4, the racing axis: N creates of one fresh name at the same time leave exactly
/// one winner, and every loser is refused with one of the two published sentences —
/// never emulator prose, and never the name it asked for.
///
/// The rows differ in how many racers contend for one name. WHICH of the two sentences
/// a loser gets follows from whether the winning create had already been answered when
/// that loser looked, which
/// `a_create_of_a_taken_name_is_refused_for_what_holds_the_name` pins exactly for each
/// state of the claim; what is decidable whatever the scheduling is asserted here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_creates_of_one_name_are_refused_with_a_published_sentence() {
    let (router, _engine) = emulator_and_engine();

    for racers in [2_usize, 8] {
        let name = format!("race-{racers}");
        let case = format!("{racers} racers for one name");
        let mut tasks = Vec::new();
        for _ in 0..racers {
            let router = Arc::clone(&router);
            let request = json!({
                "VocabularyName": name.clone(),
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            });
            tasks.push(tokio::spawn(async move {
                let reply = dispatch(&router, "CreateVocabulary", request, None).await;
                let message = if reply.status == 200 {
                    String::new()
                } else {
                    reply.error_message("a racing create")
                };
                (reply.status, reply.error_code().to_string(), message)
            }));
        }

        let mut outcomes = Vec::new();
        for task in tasks {
            outcomes.push(task.await.expect("the racer ran"));
        }
        let winners = outcomes
            .iter()
            .filter(|(status, _, _)| *status == 200)
            .count();
        assert_eq!(
            winners, 1,
            "case {case:?}: exactly one create of {name:?} may win, got {outcomes:?}"
        );

        let published = [name_contended(), name_taken("vocabulary")];
        for (status, code, message) in outcomes.iter().filter(|(status, _, _)| *status != 200) {
            assert_eq!(
                (*status, code.as_str()),
                (CLIENT_ERROR_STATUS, "ConflictException"),
                "case {case:?}: every loser gets the modelled conflict, got {outcomes:?}"
            );
            assert!(
                published.contains(message),
                "case {case:?}: a loser's refusal must be one of {published:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(&name),
                "case {case:?}: a refusal must not carry the name asked for, got \
                 {message:?}"
            );
        }
    }
}
