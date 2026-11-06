// SPDX-License-Identifier: CC0-1.0

//! `DeleteMedicalVocabulary`, over the wire.
//!
//! THE RULE: a delete in this service's custom-vocabulary families removes the one
//! record its name and Region identify, so every later reader of that name answers
//! "absent" and the name is free to be created again. `DeleteMedicalVocabulary` is
//! IDEMPOTENT about presence: a name the Region holds no medical vocabulary under is
//! already in the state the request asks for, so it is answered with this operation's
//! empty success (HTTP 200, `{}`) and nothing is stored; the one thing it refuses is a
//! record in a non-terminal state (its build still `PENDING`), and a refused delete
//! changes nothing that is stored.
//!
//! The rows span what the rule generalises over: the form of the vocabulary name
//! (every character class its `@pattern` admits, up to the modelled 200-character
//! ceiling), the URI scheme the table file was named with, whether the record carries
//! tags, how many times the delete is run, which Region holds the record, and which
//! resource FAMILY holds a record of the same name.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator.

use serde_json::{json, Value};

use crate::medical_vocabularies::{
    listed_names, medical_vocabulary_arn, MEDICAL_LANGUAGE, PAST_THE_TRANSITION,
};
use crate::transcription_jobs::Transcribe;
use crate::vocabulary_delete_and_tag_sets::{
    absent_resource_refusal, empty_body, refused, succeeded,
};

/// The word this service's own messages name this family by, as its absent-resource
/// refusal spells it.
const KIND: &str = "medical vocabulary";

/// One `CreateMedicalVocabulary` request this suite deletes the record of.
struct Row {
    case: &'static str,
    name: String,
    file_uri: &'static str,
    tags: Option<Value>,
}

/// The class of record a delete is run against, one row per axis the rule
/// generalises over: the form of the name (`VocabularyName`'s own
/// `@length(1..200)` / `@pattern(^[0-9a-zA-Z._-]+$)`, from
/// `smithy_query(query="operation", operation="DeleteMedicalVocabulary")`), the URI
/// scheme of the table file (`VocabularyFileUri`'s `@pattern(^(s3://|http(s*)://).+$)`)
/// and whether the record carries tags.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest allowed name, an s3 table file",
            name: "a".to_string(),
            file_uri: "s3://DOC-EXAMPLE-BUCKET/vocab.txt",
            tags: None,
        },
        Row {
            case: "every allowed punctuation character, an https table file",
            name: "a.b_c-d.0_9".to_string(),
            file_uri: "https://example-bucket.s3.amazonaws.com/vocab.txt",
            tags: None,
        },
        Row {
            case: "a mixed-case name, an http table file, and tags",
            name: "Cardiology.v2".to_string(),
            file_uri: "http://example-bucket.s3.amazonaws.com/vocab.txt",
            tags: Some(json!([{"Key": "team", "Value": "clinical"}])),
        },
        Row {
            case: "a name at the modelled 200-character ceiling",
            name: "z".repeat(200),
            file_uri: "s3://DOC-EXAMPLE-BUCKET/long-name-vocab.txt",
            tags: None,
        },
    ]
}

impl Row {
    /// The create that establishes this row's record.
    fn create(&self) -> Value {
        let mut request = json!({
            "VocabularyName": self.name,
            "LanguageCode": MEDICAL_LANGUAGE,
            "VocabularyFileUri": self.file_uri,
        });
        if let Some(tags) = &self.tags {
            request["Tags"] = tags.clone();
        }
        request
    }
}

/// Create this row's record and wait until its build has settled, so a delete under
/// test is answered for the record's PRESENCE rather than refused for its state.
async fn create_and_settle(transcribe: &Transcribe, row: &Row, case: &str) {
    succeeded(
        &transcribe
            .call("CreateMedicalVocabulary", row.create())
            .await,
        case,
    );
    // The family's one clock-owed transition (`PENDING` -> `FAILED` here, since no
    // object store is mounted to serve the table file) falls due a fixed span after the
    // create; a `FAILED` record is terminal, so the delete is not refused for its state.
    std::thread::sleep(PAST_THE_TRANSITION);
}

/// The whole rule on a settled record: the delete removes what the name identified
/// from every reader, every repeat of it is the same empty success, and the name is
/// free for a new create.
#[tokio::test]
async fn a_delete_removes_the_record_and_frees_its_name() {
    let transcribe = Transcribe::new();

    for row in rows() {
        let case = row.case;
        create_and_settle(&transcribe, &row, case).await;

        // The record is there before the delete.
        let read = transcribe
            .call(
                "GetMedicalVocabulary",
                json!({"VocabularyName": row.name}),
            )
            .await;
        assert_eq!(
            read.ok(case)["VocabularyName"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: the record must be readable before the delete, got {:?}",
            read.body
        );

        // Run the delete three times: the FIRST removes the record and every repeat
        // answers the same empty success, because what it asks for is already true.
        for attempt in 1..=3 {
            let attempt_case = format!("{case} / delete #{attempt}");
            let reply = transcribe
                .call(
                    "DeleteMedicalVocabulary",
                    json!({"VocabularyName": row.name}),
                )
                .await;
            succeeded(&reply, &attempt_case);
            empty_body(&reply, &attempt_case);

            // Every reader of the family answers "absent" afterwards.
            absent_resource_refusal(
                &transcribe
                    .call(
                        "GetMedicalVocabulary",
                        json!({"VocabularyName": row.name}),
                    )
                    .await,
                &attempt_case,
                KIND,
            );
            let listed = transcribe
                .call("ListMedicalVocabularies", json!({"MaxResults": 100}))
                .await;
            let names = listed_names(&listed, &attempt_case);
            assert!(
                !names.contains(&row.name),
                "case {attempt_case:?}: a deleted record must not be listed, got {names:?}"
            );
            // The tag set went with the record: the resource its ARN names is gone.
            absent_resource_refusal(
                &transcribe
                    .call(
                        "ListTagsForResource",
                        json!({"ResourceArn": medical_vocabulary_arn(&row.name)}),
                    )
                    .await,
                &attempt_case,
                KIND,
            );
        }

        // The name is FREE again: the same create succeeds, and the new record is a
        // new one — `PENDING`, carrying none of the deleted record's tags.
        succeeded(
            &transcribe
                .call(
                    "CreateMedicalVocabulary",
                    json!({
                        "VocabularyName": row.name,
                        "LanguageCode": MEDICAL_LANGUAGE,
                        "VocabularyFileUri": row.file_uri,
                    }),
                )
                .await,
            case,
        );
        let recreated = transcribe
            .call(
                "ListTagsForResource",
                json!({"ResourceArn": medical_vocabulary_arn(&row.name)}),
            )
            .await;
        assert_eq!(
            recreated.ok(case)["Tags"],
            json!([]),
            "case {case:?}: a re-created record must carry no tag of the deleted one, \
             got {:?}",
            recreated.body
        );
        std::thread::sleep(PAST_THE_TRANSITION);
        succeeded(
            &transcribe
                .call(
                    "DeleteMedicalVocabulary",
                    json!({"VocabularyName": row.name}),
                )
                .await,
            case,
        );
    }
}

/// The names a delete that finds nothing is probed with: every form the member's own
/// constraint admits, including ones no create in this suite used.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest allowed name", "q".to_string()),
        ("every allowed punctuation character", "x.y_z-0.9".to_string()),
        ("a plainly named record", "never-created-at-all".to_string()),
        ("a name at the 200-character ceiling", "n".repeat(200)),
    ]
}

/// A delete of a record this Region does not hold is an EMPTY SUCCESS — for every form
/// of name — and stores nothing, so the listing is still empty afterwards and the
/// family's reader still answers "absent".
#[tokio::test]
async fn deleting_a_record_this_region_does_not_hold_is_an_empty_success() {
    let transcribe = Transcribe::new();

    for (name_case, name) in absent_names() {
        let reply = transcribe
            .call("DeleteMedicalVocabulary", json!({"VocabularyName": name}))
            .await;
        succeeded(&reply, name_case);
        empty_body(&reply, name_case);

        // The delete must not MINT what it did not find.
        absent_resource_refusal(
            &transcribe
                .call("GetMedicalVocabulary", json!({"VocabularyName": name}))
                .await,
            name_case,
            KIND,
        );
    }

    let listed = transcribe
        .call("ListMedicalVocabularies", json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        listed.ok("the listing after absent deletes")["Vocabularies"],
        json!([]),
        "an absent delete must store nothing, got {:?}",
        listed.body
    );
}

/// A record whose build has NOT finished is in a non-terminal state, and a delete of
/// it is refused with the documented `BadRequestException` naming the record and the
/// state it is in — leaving the record in place, so the delete made once the build
/// settled succeeds.
#[tokio::test]
async fn deleting_a_record_whose_build_is_still_pending_is_refused() {
    let transcribe = Transcribe::new();

    for row in rows() {
        let case = row.case;
        succeeded(
            &transcribe
                .call("CreateMedicalVocabulary", row.create())
                .await,
            case,
        );

        // Straight after the create the build is still owed, so the record is
        // `PENDING` and the delete is refused with this family's own fixed sentence for
        // that condition, which names neither the vocabulary nor the state it is in.
        let reply = transcribe
            .call(
                "DeleteMedicalVocabulary",
                json!({"VocabularyName": row.name}),
            )
            .await;
        reply.refused(case, 400, "BadRequestException");
        let message = reply.error_message(case);
        assert_eq!(
            message,
            "The medical vocabulary that you\u{2019}re trying to delete isn't in the \
             READY or FAILED state. Try your request again when the medical vocabulary \
             state is READY or FAILED.",
            "case {case:?}: expected this family's non-terminal delete refusal, got \
             {message:?}"
        );

        // Refused, so nothing was removed: the record is still readable and still
        // listed.
        let read = transcribe
            .call(
                "GetMedicalVocabulary",
                json!({"VocabularyName": row.name}),
            )
            .await;
        assert_eq!(
            read.ok(case)["VocabularyName"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: a refused delete must leave the record readable, got {:?}",
            read.body
        );

        // Once the build has settled, the state that blocked the delete is gone and
        // the same call succeeds.
        std::thread::sleep(PAST_THE_TRANSITION);
        let deleted = transcribe
            .call(
                "DeleteMedicalVocabulary",
                json!({"VocabularyName": row.name}),
            )
            .await;
        succeeded(&deleted, case);
        empty_body(&deleted, case);
        absent_resource_refusal(
            &transcribe
                .call(
                    "GetMedicalVocabulary",
                    json!({"VocabularyName": row.name}),
                )
                .await,
            case,
            KIND,
        );
    }
}

/// The delete answers for ONE record: the one its name identifies in the Region the
/// request was signed for, and in its own resource family. A medical vocabulary of the
/// same name in another Region, and a NON-medical custom vocabulary of that name, are
/// untouched.
#[tokio::test]
async fn a_delete_reaches_only_its_own_region_and_family() {
    let transcribe = Transcribe::new();
    let shared = "shared.name";
    let case = "one name in two regions and two families";
    let other_region = Some("us-west-2");
    let home_region = Some("us-east-1");

    for region in [home_region, other_region] {
        succeeded(
            &transcribe
                .call_in_region(
                    "CreateMedicalVocabulary",
                    json!({
                        "VocabularyName": shared,
                        "LanguageCode": MEDICAL_LANGUAGE,
                        "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/vocab.txt",
                    }),
                    region,
                )
                .await,
            case,
        );
    }
    // A custom vocabulary of the SAME name, in the family next door.
    succeeded(
        &transcribe
            .call_in_region(
                "CreateVocabulary",
                json!({
                    "VocabularyName": shared,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "Phrases": ["Aardvark", "Transcribe"],
                }),
                home_region,
            )
            .await,
        case,
    );
    std::thread::sleep(PAST_THE_TRANSITION);

    succeeded(
        &transcribe
            .call_in_region(
                "DeleteMedicalVocabulary",
                json!({"VocabularyName": shared}),
                home_region,
            )
            .await,
        case,
    );

    // Gone in the Region that deleted it...
    absent_resource_refusal(
        &transcribe
            .call_in_region(
                "GetMedicalVocabulary",
                json!({"VocabularyName": shared}),
                home_region,
            )
            .await,
        case,
        KIND,
    );
    // ...and still there in the other Region, and in the other family.
    let elsewhere = transcribe
        .call_in_region(
            "GetMedicalVocabulary",
            json!({"VocabularyName": shared}),
            other_region,
        )
        .await;
    assert_eq!(
        elsewhere.ok(case)["VocabularyName"].as_str(),
        Some(shared),
        "case {case:?}: another Region's record must survive the delete, got {:?}",
        elsewhere.body
    );
    let sibling = transcribe
        .call_in_region("GetVocabulary", json!({"VocabularyName": shared}), home_region)
        .await;
    assert_eq!(
        sibling.ok(case)["VocabularyName"].as_str(),
        Some(shared),
        "case {case:?}: the other family's record must survive the delete, got {:?}",
        sibling.body
    );

    // Each surviving record is deletable through its own call, and nothing else is a
    // way to remove it: the non-medical delete does not answer for the medical family,
    // and a repeat of the medical delete that already ran is its empty success.
    let repeated = transcribe
        .call_in_region(
            "DeleteMedicalVocabulary",
            json!({"VocabularyName": shared}),
            home_region,
        )
        .await;
    succeeded(&repeated, case);
    empty_body(&repeated, case);
    succeeded(
        &transcribe
            .call_in_region(
                "DeleteVocabulary",
                json!({"VocabularyName": shared}),
                home_region,
            )
            .await,
        case,
    );
    refused(
        &transcribe
            .call_in_region("GetVocabulary", json!({"VocabularyName": shared}), home_region)
            .await,
        case,
    );
    succeeded(
        &transcribe
            .call_in_region(
                "DeleteMedicalVocabulary",
                json!({"VocabularyName": shared}),
                other_region,
            )
            .await,
        case,
    );
    refused(
        &transcribe
            .call_in_region(
                "GetMedicalVocabulary",
                json!({"VocabularyName": shared}),
                other_region,
            )
            .await,
        case,
    );
}
