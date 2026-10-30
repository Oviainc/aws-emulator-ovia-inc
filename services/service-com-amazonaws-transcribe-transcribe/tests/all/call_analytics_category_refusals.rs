// SPDX-License-Identifier: CC0-1.0

//! What the Call Analytics category family REFUSES with, end to end over the wire.
//!
//! THE RULE UNDER TEST: the error shape and the message text a refusal carries belong to
//! the resource FAMILY the refusal is about, not to one template spanning every family —
//! so this family's two name refusals are its own fixed sentences, carried by the shape
//! this family refuses with, and no part of either sentence comes from the request:
//!
//! 1. every operation of the family that looks a category up by name — the read, the
//!    update and the delete — answers a name this account holds no category under with
//!    `BadRequestException` (the shape whose model doc covers a request that "didn't pass
//!    one or more validation tests", on this protocol's single client status) carrying
//!    [`ABSENT_CATEGORY_MESSAGE`];
//! 2. a create naming a category this account already holds answers the SAME shape —
//!    `BadRequestException`, not the `ConflictException` the operation also models
//!    (`smithy_query(query="operation", operation="CreateCallAnalyticsCategory")`) —
//!    carrying [`NAME_TAKEN_MESSAGE`], and leaves the stored category exactly as the
//!    create that won it left it;
//! 3. the families whose own refusals are worded and shaped differently keep their own
//!    wording and shape, so neither text is a global rename: a custom vocabulary, a
//!    custom vocabulary filter and a transcription job still answer a taken name with
//!    `ConflictException` and the shared "already exists" sentence, and a custom
//!    vocabulary still answers an absent name with the shared "couldn't be found" one.
//!
//! The rows differ along what the rule generalises over: the OPERATION of the family that
//! refuses (three for the lookup, one for the create), the FORM of the `CategoryName`
//! asked about (both published `@length` bounds of 1..200 and every character class its
//! `@pattern` `^[0-9a-zA-Z._-]+$` admits — `smithy_query(query="operation",
//! operation="GetCallAnalyticsCategory")`), the rules, input type and tags the creates
//! carry, and, for rule 3, the resource family.
//!
//! The emulator and the answer assertions come from the transcription-job harness
//! (`crate::transcription_jobs`), so no second emulator is stood up here.

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// The modelled error shape every refusal of this family carries, and the status this
/// service's awsJson1_1 wire protocol puts on every client error
/// (`crate::client_error_wire_status`).
const REFUSAL_CODE: &str = "BadRequestException";
/// See [`REFUSAL_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// How this family words "there is no category of that name", for every operation of it
/// that looks one up.
///
/// Entirely FIXED: the name the caller asked about appears nowhere in it. Both
/// apostrophes are U+2019 (RIGHT SINGLE QUOTATION MARK), not the ASCII `'`.
///
/// `pub(crate)`: the update-validation suite
/// (`crate::call_analytics_category_update_rule_validation`) asserts the same refusal for
/// its own rows, so it reads this rather than keeping a second copy of the sentence.
pub(crate) const ABSENT_CATEGORY_MESSAGE: &str = "The category that you\u{2019}ve \
     specified couldn\u{2019}t be found. Check the category name and try your request \
     again.";

/// How this family words "that category name is already taken". Fixed and apostrophed
/// like [`ABSENT_CATEGORY_MESSAGE`], and the conflicting name is not in it either.
pub(crate) const NAME_TAKEN_MESSAGE: &str = "The category name that you\u{2019}ve \
     requested already exists. Change the category name and try your request again.";

/// A rule a category can be created or updated with, naming `target`.
///
/// One member of the `Rule` union is enough for these rows — what the rows vary is the
/// NAME asked about and the operation asking, not the rule shape, which the time-range
/// suite (`crate::call_analytics_rule_time_ranges`) spans on its own.
fn transcript_rule(target: &str) -> Value {
    json!({
        "TranscriptFilter": {
            "TranscriptFilterType": "EXACT",
            "Targets": [target]
        }
    })
}

/// The forms of `CategoryName` the rows ask about: both published `@length` bounds
/// (1..200) and each character class the member's `@pattern` `^[0-9a-zA-Z._-]+$` admits
/// (`smithy_query(query="operation", operation="GetCallAnalyticsCategory")`).
fn name_forms() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name the length bound admits", "a".to_string()),
        (
            "a dotted, underscored and hyphenated name",
            "sentiment.positive_last-30".to_string(),
        ),
        ("an all-digit name", "1234567890".to_string()),
        ("a mixed-case name", "SentimentPositive".to_string()),
        (
            "a name at the 200-character ceiling",
            "n".repeat(200),
        ),
    ]
}

/// Every operation of this family that looks a category up by name, with the request
/// body each one needs for `name`: the read, the update (whose `Rules` are required) and
/// the delete.
fn lookups(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "GetCallAnalyticsCategory",
            json!({"CategoryName": name}),
        ),
        (
            "UpdateCallAnalyticsCategory",
            json!({"CategoryName": name, "Rules": [transcript_rule("goodbye")]}),
        ),
        (
            "DeleteCallAnalyticsCategory",
            json!({"CategoryName": name}),
        ),
    ]
}

/// Assert `reply` is this family's refusal: the modelled shape on this protocol's client
/// status, carrying exactly `expected` — a sentence with no part of the request in it, so
/// `asked_about` is absent from it and the apostrophe is the one the service writes.
fn family_refusal(reply: &Reply, case: &str, expected: &str, asked_about: &str) {
    reply.refused(case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
    let message = reply.error_message(case);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal {expected:?}, got {message:?}"
    );
    assert!(
        message.contains('\u{2019}') && !message.contains('\''),
        "case {case:?}: the refusal is written with the U+2019 apostrophe, got {message:?}"
    );
    // A name of three characters or fewer is skipped: the wording contains most short
    // letter runs somewhere.
    if asked_about.len() > 3 {
        assert!(
            !message.contains(asked_about),
            "case {case:?}: the refusal must not echo the requested {asked_about:?}, got \
             {message:?}"
        );
    }
}

/// RULE 1: every lookup of this family refuses a name it holds no category under with the
/// family's shape and fixed text, whatever form the name takes.
#[tokio::test]
async fn a_category_this_account_does_not_hold_is_refused_by_every_lookup() {
    let transcribe = Transcribe::new();

    for (name_case, name) in name_forms() {
        for (operation, request) in lookups(&name) {
            let case = format!("{operation} / {name_case}");
            let reply = transcribe.call(operation, request).await;
            family_refusal(&reply, &case, ABSENT_CATEGORY_MESSAGE, &name);
        }
    }
}

/// RULE 1, the deleted axis: a category that WAS held is refused the same way once its
/// delete has dropped it, so the refusal is about the store and not about the name form.
#[tokio::test]
async fn a_deleted_category_is_refused_by_every_lookup() {
    let transcribe = Transcribe::new();

    for (name_case, name) in name_forms() {
        transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": [transcript_rule(&name)]}),
            )
            .await
            .ok(&format!("create / {name_case}"));
        transcribe
            .call(
                "DeleteCallAnalyticsCategory",
                json!({"CategoryName": name}),
            )
            .await
            .ok_status(&format!("delete / {name_case}"), 200);

        for (operation, request) in lookups(&name) {
            let case = format!("{operation} after the delete / {name_case}");
            let reply = transcribe.call(operation, request).await;
            family_refusal(&reply, &case, ABSENT_CATEGORY_MESSAGE, &name);
        }
    }
}

/// RULE 2: a create of a name this account already holds is this family's taken-name
/// refusal — the same shape its lookups refuse with — and the category the first create
/// stored is untouched by it.
#[tokio::test]
async fn a_create_of_a_held_category_name_is_refused() {
    let transcribe = Transcribe::new();

    // The rows differ in the create that CLAIMS the name (its rule count, its input type
    // and whether it carries tags) and in every member the second create may differ in,
    // so no row can pass by the two requests being equal.
    let rows: Vec<(&str, String, Value, Value)> = vec![
        (
            "a claim carrying only the required members",
            "claimed.default".to_string(),
            json!({
                "CategoryName": "claimed.default",
                "Rules": [transcript_rule("hello")]
            }),
            json!({
                "CategoryName": "claimed.default",
                "Rules": [transcript_rule("goodbye"), transcript_rule("thanks")],
                "InputType": "REAL_TIME",
                "Tags": [{"Key": "second", "Value": "create"}]
            }),
        ),
        (
            "a claim typed REAL_TIME and tagged",
            "claimed-real_time".to_string(),
            json!({
                "CategoryName": "claimed-real_time",
                "Rules": [transcript_rule("hello"), transcript_rule("hold")],
                "InputType": "REAL_TIME",
                "Tags": [{"Key": "first", "Value": "create"}]
            }),
            json!({
                "CategoryName": "claimed-real_time",
                "Rules": [transcript_rule("goodbye")],
                "InputType": "POST_CALL"
            }),
        ),
        (
            "a claim of the shortest name the length bound admits",
            "c".to_string(),
            json!({
                "CategoryName": "c",
                "Rules": [{"NonTalkTimeFilter": {"Threshold": 10_000}}],
                "InputType": "POST_CALL"
            }),
            json!({
                "CategoryName": "c",
                "Rules": [transcript_rule("goodbye")]
            }),
        ),
        (
            "a claim of a name at the 200-character ceiling",
            "n".repeat(200),
            json!({
                "CategoryName": "n".repeat(200),
                "Rules": [transcript_rule("hello")]
            }),
            json!({
                "CategoryName": "n".repeat(200),
                "Rules": [transcript_rule("goodbye")],
                "InputType": "REAL_TIME"
            }),
        ),
    ];

    for (case, name, first, second) in rows {
        let created = transcribe
            .call("CreateCallAnalyticsCategory", first)
            .await;
        let stored = created.ok(case).get("CategoryProperties").cloned();

        let refused = transcribe
            .call("CreateCallAnalyticsCategory", second)
            .await;
        family_refusal(&refused, case, NAME_TAKEN_MESSAGE, &name);

        // The refused create overwrote nothing: the read reports the category the
        // winning create stored, rules, input type and timestamps alike.
        let read = transcribe
            .call(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": name}),
            )
            .await;
        assert_eq!(
            read.ok(case).get("CategoryProperties").cloned(),
            stored,
            "case {case:?}: the refused create must leave the stored category as it was, \
             got {:?}",
            read.body
        );
    }
}

/// RULE 3: the other families keep their own refusals, so neither sentence above is a
/// rename applied to the whole service.
///
/// The rows are the families whose create or start claims a name and whose recorded
/// refusal is the shared one: each still answers `ConflictException` with the shared
/// "already exists" sentence, and a custom vocabulary's reader still answers the shared
/// "couldn't be found" one.
#[tokio::test]
async fn the_other_families_keep_their_own_refusals() {
    let transcribe = Transcribe::new();

    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a second CreateVocabulary of one name",
            "CreateVocabulary",
            json!({
                "VocabularyName": "claimed",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            "The requested vocabulary name already exists. Use a different vocabulary \
             name.",
        ),
        (
            "a second CreateVocabularyFilter of one name",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "claimed",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
            "The requested vocabulary filter name already exists. Use a different \
             vocabulary filter name.",
        ),
        (
            "a second StartTranscriptionJob of one name",
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "claimed",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
            }),
            "The requested job name already exists. Use a different job name.",
        ),
    ];

    for (case, operation, request, expected) in rows {
        transcribe
            .call(operation, request.clone())
            .await
            .ok_status(case, 200);

        let conflict = transcribe.call(operation, request).await;
        conflict.refused(case, CLIENT_ERROR_STATUS, "ConflictException");
        let message = conflict.error_message(case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
    }

    let absent = transcribe
        .call("GetVocabulary", json!({"VocabularyName": "never-created"}))
        .await;
    absent.refused(
        "GetVocabulary for a vocabulary that was never created",
        CLIENT_ERROR_STATUS,
        REFUSAL_CODE,
    );
    let message = absent.error_message("GetVocabulary for an absent vocabulary");
    assert_eq!(
        message,
        "The requested vocabulary couldn't be found. Check the vocabulary name and try \
         your request again.",
        "case \"GetVocabulary for an absent vocabulary\": the shared not-found sentence \
         must survive, got {message:?}"
    );
}
