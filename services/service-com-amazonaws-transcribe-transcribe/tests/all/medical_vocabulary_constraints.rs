// SPDX-License-Identifier: CC0-1.0

//! The INPUT CONTRACT of `CreateMedicalVocabulary`, end to end over the wire, and the
//! account limit the family is bounded by.
//!
//! THE RULES THESE ROWS ASSERT:
//!
//! 1. A `CreateMedicalVocabulary` request that violates one of the constraint traits
//!    its input shape carries is refused at the boundary — HTTP 400
//!    `ValidationException`, before the handler runs — and stores nothing, while a
//!    request sitting exactly ON each published bound is accepted and recorded, so
//!    every later operation of the family finds it.
//! 2. The family is bounded: once the account holds the published quota of custom
//!    medical vocabularies, a further create is refused with the modelled
//!    `LimitExceededException` and nothing more is stored. That quota is this
//!    family's own — the NON-medical custom vocabularies are a separate resource with
//!    a separate count.
//!
//! The constraints are the ones the model itself records for this operation
//! (`smithy_query(query="operation", operation="CreateMedicalVocabulary")` and
//! `smithy_query(query="shape", shape="Tag,TagList")`):
//! `VocabularyName` `@required @length(1..200) @pattern(^[0-9a-zA-Z._-]+$)`,
//! `VocabularyFileUri` `@required @length(1..2000) @pattern(^(s3://|http(s*)://).+$)`,
//! `LanguageCode` `@required`, `Tags` `@length(1..200)` of `Tag` whose `Key` is
//! `@length(1..128)` and whose `Value` is `@length(0..256)`.
//!
//! Every modelled client error of this service reaches the wire as HTTP 400: the
//! service maps the whole 4xx range onto one status (`scaffolded::wire_status`, pinned
//! by `crate::client_error_wire_status`), so a `LimitExceededException` whose
//! modelled `httpError` is 429 is asserted here at 400 with its own error code.
//!
//! The lifecycle of an accepted medical vocabulary — `PENDING`, then the modelled
//! `FAILED` the build this emulator cannot run ends in — is
//! `crate::medical_vocabularies`' subject; these rows stop at "it was recorded and
//! reads back". The quota row below fills the family (100 creates through the router,
//! a second or so) because the refusal it asserts only exists at the boundary of a
//! full family; the bound itself is read from the service's own
//! `state::MAX_VOCABULARIES` rather than written out here. The emulator, the call and
//! the answer assertions are the transcription-job suite's harness
//! (`crate::transcription_jobs`), reused rather than stood up a second time.

use serde_json::{json, Value};

use crate::medical_vocabularies::{listed_names, medical_vocabulary_arn, MEDICAL_LANGUAGE};
use crate::transcription_jobs::Transcribe;

/// A table file URI inside every constraint `VocabularyFileUri` carries, used by the
/// rows that vary another member.
const A_VALID_FILE_URI: &str = "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt";

/// The code every boundary refusal carries, whichever constraint it was that the
/// request broke.
const VALIDATION_CODE: &str = "ValidationException";

/// The code this family answers for a medical vocabulary the account does not hold:
/// the shape whose own documentation covers an entity that "doesn't exist".
const NOT_FOUND_CODE: &str = "BadRequestException";

/// A `VocabularyFileUri` of exactly `length` characters, inside the member's
/// `@pattern`.
fn file_uri_of_length(length: usize) -> String {
    let prefix = "s3://DOC-EXAMPLE-BUCKET/";
    assert!(
        length > prefix.len(),
        "a URI of {length} characters cannot carry the prefix {prefix:?}"
    );
    format!("{prefix}{}", "a".repeat(length - prefix.len()))
}

/// `count` distinct tags, the smallest ones the `Tag` shape admits.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
            .collect(),
    )
}

/// One row per constraint the input shape carries that a request can break: each
/// `@required` member omitted, each published length bound overstepped by one in each
/// direction, and each kind of value the two `@pattern`s exclude.
///
/// The third column is what `GetMedicalVocabulary` must answer afterwards when asked
/// for the name the row named — this family's own not-found refusal where the name is
/// itself a modelled `VocabularyName`, and the boundary refusal where the name is what
/// the row broke — or `""` for the row that names no vocabulary at all.
fn refused_rows() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "VocabularyName omitted, though the model marks it required",
            json!({
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            "",
        ),
        (
            "LanguageCode omitted, though the model marks it required",
            json!({
                "VocabularyName": "no-language",
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            NOT_FOUND_CODE,
        ),
        (
            "VocabularyFileUri omitted, though the model marks it required",
            json!({
                "VocabularyName": "no-file",
                "LanguageCode": MEDICAL_LANGUAGE
            }),
            NOT_FOUND_CODE,
        ),
        (
            "VocabularyName below the modelled length floor of 1",
            json!({
                "VocabularyName": "",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            VALIDATION_CODE,
        ),
        (
            "VocabularyName one past the modelled 200-character ceiling",
            json!({
                "VocabularyName": "z".repeat(201),
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            VALIDATION_CODE,
        ),
        (
            "VocabularyName outside the modelled pattern: a space",
            json!({
                "VocabularyName": "clinical terms",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            VALIDATION_CODE,
        ),
        (
            "VocabularyName outside the modelled pattern: a punctuation character it \
             does not admit",
            json!({
                "VocabularyName": "clinical:terms",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            VALIDATION_CODE,
        ),
        (
            "VocabularyFileUri below the modelled length floor of 1",
            json!({
                "VocabularyName": "empty-file-uri",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": ""
            }),
            NOT_FOUND_CODE,
        ),
        (
            "VocabularyFileUri one past the modelled 2000-character ceiling",
            json!({
                "VocabularyName": "long-file-uri",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": file_uri_of_length(2001)
            }),
            NOT_FOUND_CODE,
        ),
        (
            "VocabularyFileUri outside the modelled pattern: a scheme it does not admit",
            json!({
                "VocabularyName": "ftp-file-uri",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "ftp://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
            }),
            NOT_FOUND_CODE,
        ),
        (
            "VocabularyFileUri outside the modelled pattern: a scheme with no path \
             after it",
            json!({
                "VocabularyName": "bare-scheme",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "s3://"
            }),
            NOT_FOUND_CODE,
        ),
        (
            "Tags below the modelled list floor of 1",
            json!({
                "VocabularyName": "no-tags",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI,
                "Tags": []
            }),
            NOT_FOUND_CODE,
        ),
        (
            "Tags one past the modelled list ceiling of 200",
            json!({
                "VocabularyName": "too-many-tags",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI,
                "Tags": tags(201)
            }),
            NOT_FOUND_CODE,
        ),
        (
            "a tag key one past the modelled 128-character ceiling",
            json!({
                "VocabularyName": "long-tag-key",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI,
                "Tags": [{"Key": "k".repeat(129), "Value": "v"}]
            }),
            NOT_FOUND_CODE,
        ),
        (
            "a tag key below the modelled length floor of 1",
            json!({
                "VocabularyName": "empty-tag-key",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI,
                "Tags": [{"Key": "", "Value": "v"}]
            }),
            NOT_FOUND_CODE,
        ),
        (
            "a tag value one past the modelled 256-character ceiling",
            json!({
                "VocabularyName": "long-tag-value",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI,
                "Tags": [{"Key": "k", "Value": "v".repeat(257)}]
            }),
            NOT_FOUND_CODE,
        ),
    ]
}

/// One row per published bound a request may sit exactly ON: the shortest and the
/// longest name, the longest file URI, the smallest tag set with the smallest tag the
/// `Tag` shape admits (an EMPTY value, whose floor is 0), and the largest tag set with
/// the longest key and value.
fn accepted_boundary_rows() -> Vec<(&'static str, Value, usize)> {
    // The list ceiling, with the 200th tag carrying a key and a value at their own
    // ceilings (128 and 256 characters).
    let mut largest_tag_set = tags(199);
    largest_tag_set
        .as_array_mut()
        .expect("a tag list")
        .push(json!({"Key": "k".repeat(128), "Value": "v".repeat(256)}));

    vec![
        (
            "a name at the modelled length floor of 1",
            json!({
                "VocabularyName": "a",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            0,
        ),
        (
            "a name at the modelled 200-character ceiling",
            json!({
                "VocabularyName": "z".repeat(200),
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
            0,
        ),
        (
            "a file URI at the modelled 2000-character ceiling",
            json!({
                "VocabularyName": "uri-at-the-ceiling",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": file_uri_of_length(2000)
            }),
            0,
        ),
        (
            "one tag, the list floor, with a tag value at its floor of 0",
            json!({
                "VocabularyName": "one_tag",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI,
                "Tags": [{"Key": "k", "Value": ""}]
            }),
            1,
        ),
        (
            "200 tags, the list ceiling, one of them with a key and a value at theirs",
            json!({
                "VocabularyName": "many.tags",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI,
                "Tags": largest_tag_set
            }),
            200,
        ),
    ]
}

/// RULE 1: a request breaking a modelled constraint is refused at the boundary and
/// stores nothing, while a request on each published bound is accepted, recorded and
/// readable back.
#[tokio::test]
async fn the_modelled_constraints_on_a_medical_vocabulary_create_are_enforced_at_the_boundary() {
    let transcribe = Transcribe::new();

    for (case, request, reader_code) in refused_rows() {
        let refused = transcribe
            .call("CreateMedicalVocabulary", request.clone())
            .await;
        refused.refused(case, 400, VALIDATION_CODE);
        // A refused request never reached the handler, so the name it asked for is
        // not a resource: the family's own reader does not find it. Which refusal the
        // reader answers is the row's own third column.
        if !reader_code.is_empty() {
            let name = request["VocabularyName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: this row names a vocabulary"));
            transcribe
                .call("GetMedicalVocabulary", json!({"VocabularyName": name}))
                .await
                .refused(case, 400, reader_code);
        }
    }

    let mut expected: Vec<String> = Vec::new();
    for (case, request, tag_count) in accepted_boundary_rows() {
        let name = request["VocabularyName"]
            .as_str()
            .expect("every row names a vocabulary")
            .to_string();
        let created = transcribe
            .call("CreateMedicalVocabulary", request.clone())
            .await;
        created.ok(case);
        assert_eq!(
            (
                created.member(case, "VocabularyName"),
                created.member(case, "LanguageCode"),
                created.member(case, "VocabularyState"),
            ),
            (name.clone(), MEDICAL_LANGUAGE.to_string(), "PENDING".to_string()),
            "case {case:?}: a request on a published bound must be accepted and \
             answered with the record it stored, got {:?}",
            created.body
        );

        // Recorded, so the family's own reader finds it with the same facts.
        let got = transcribe
            .call("GetMedicalVocabulary", json!({"VocabularyName": &name}))
            .await;
        got.ok(case);
        assert_eq!(
            got.member(case, "VocabularyName"),
            name,
            "case {case:?}: the reader must report the record just created, got {:?}",
            got.body
        );

        // The tag set the create named is on the record, whatever its size.
        let listed_tags = transcribe
            .call(
                "ListTagsForResource",
                json!({"ResourceArn": medical_vocabulary_arn(&name)}),
            )
            .await;
        listed_tags.ok(case);
        let actual_tags = listed_tags.body["Tags"]
            .as_array()
            .map_or(0, |set| set.len());
        assert_eq!(
            actual_tags, tag_count,
            "case {case:?}: the create's own {tag_count} tags must be stored on the \
             record, got {actual_tags}"
        );

        expected.push(name);
    }

    // Exactly the accepted rows are in the store: every refusal above stored nothing.
    let listed = transcribe
        .call("ListMedicalVocabularies", json!({"MaxResults": 100}))
        .await;
    let mut names = listed_names(&listed, "every stored medical vocabulary");
    names.sort();
    expected.sort();
    assert_eq!(
        names, expected,
        "the store must hold exactly the accepted rows, got {:?}",
        listed.body
    );
}

/// RULE 2: the family is bounded by its own account quota, and the NON-medical custom
/// vocabularies are counted separately.
#[tokio::test]
async fn a_medical_vocabulary_create_stops_at_this_familys_own_account_quota() {
    let transcribe = Transcribe::new();
    // Amazon Transcribe publishes a default quota of 100 custom medical vocabularies
    // per account per Region, which this emulator applies per family; the figure is
    // read from the service's own constant rather than restated here.
    let quota = service_com_amazonaws_transcribe_transcribe::state::MAX_VOCABULARIES;
    // The whole family is read back in ONE page below, which `MaxResults`' modelled
    // `@range(min: 1, max: 100)` caps at 100 entries.
    assert!(
        quota <= 100,
        "this test reads the family back in one page, so the quota must fit the \
         modelled MaxResults ceiling of 100, got {quota}"
    );

    for index in 0..quota {
        let name = format!("quota-vocab-{index:03}");
        transcribe
            .call(
                "CreateMedicalVocabulary",
                json!({
                    "VocabularyName": &name,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": A_VALID_FILE_URI
                }),
            )
            .await
            .ok_status(&name, 200);
    }

    let over = "quota-vocab-over";
    let refused = transcribe
        .call(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": over,
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": A_VALID_FILE_URI
            }),
        )
        .await;
    // The modelled `LimitExceededException`, which this service puts on the wire as
    // 400 like every other client error (see `crate::client_error_wire_status`).
    refused.refused("one create past the quota", 400, "LimitExceededException");
    // The status and the error code are the requirements; the wording is not, so the
    // only thing asserted about the message is that it does not hand the caller's own
    // resource name back.
    let message = refused.error_message("one create past the quota");
    assert!(
        !message.contains(over),
        "the refusal must not echo the requested name, got {message:?}"
    );

    // Nothing was stored for the refused create, and the quota's worth is still there.
    transcribe
        .call("GetMedicalVocabulary", json!({"VocabularyName": over}))
        .await
        .refused(
            "the vocabulary the refused create asked for",
            400,
            "BadRequestException",
        );
    let listed = transcribe
        .call("ListMedicalVocabularies", json!({"MaxResults": quota}))
        .await;
    assert_eq!(
        listed_names(&listed, "the full quota").len(),
        quota,
        "the store must still hold exactly {quota} medical vocabularies, got {:?}",
        listed.body
    );

    // The quota is this family's own: a NON-medical custom vocabulary is a different
    // resource and is still accepted.
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "quota-vocab-custom",
                "LanguageCode": MEDICAL_LANGUAGE,
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("a custom vocabulary while the medical family is full");
}
