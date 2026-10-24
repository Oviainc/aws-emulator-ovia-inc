// SPDX-License-Identifier: CC0-1.0

//! The custom MEDICAL vocabulary family, end to end over the wire:
//! `CreateMedicalVocabulary` -> `GetMedicalVocabulary` -> `ListMedicalVocabularies`,
//! and the tag set of a medical vocabulary through the three tagging operations.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time.
//!
//! WHAT A MEDICAL VOCABULARY IS HERE: its terms can only come from the table file at
//! `VocabularyFileUri`, which `CreateMedicalVocabulary` models `@required` with no
//! inline alternative, and this emulator reads no object store. So the RECORD is
//! real — name, language, state, timestamps, tags, and every listing and filter over
//! them — while the build it cannot run ends in the service's own modelled `FAILED`
//! state with a `FailureReason`. These tests assert exactly that, and never a
//! `READY` medical vocabulary.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for the `PENDING -> FAILED` transition of a medical vocabulary created
/// during the test to have fallen due: one span of
/// `crate::state::VOCABULARY_TRANSITION_DELAY` (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so `tokio::time::sleep` is not
/// available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The region every row of this suite is served in (the rows are unsigned, and this
/// is the region this emulator serves a request carrying no SigV4 credential scope
/// in), so a tagging ARN is predictable.
const REGION: &str = "us-east-1";

/// The one language Amazon Transcribe Medical supports, as the model's own doc for
/// `CreateMedicalVocabularyRequest$LanguageCode` states ("US English (en-US) is the
/// only language supported with Amazon Transcribe Medical").
const MEDICAL_LANGUAGE: &str = "en-US";

/// The class of `CreateMedicalVocabulary` request, one row per axis the rules
/// generalise over: the form of the vocabulary name, which URI scheme the table file
/// is named with, and whether the request carries tags.
///
/// The schemes are the ones `VocabularyFileUri`'s own `@pattern` admits —
/// `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
/// operation="CreateMedicalVocabulary")`) — so the rows cover each alternative of
/// that catalog, including ones no recorded request used.
fn medical_vocabulary_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "an s3 table file",
            json!({
                "VocabularyName": "clinical-terms",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
            }),
        ),
        (
            "an https table file and a dotted, mixed-case name",
            json!({
                "VocabularyName": "Cardiology.v2",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "https://example-bucket.s3.amazonaws.com/vocab.txt"
            }),
        ),
        (
            "an http table file, an underscored name and tags",
            json!({
                "VocabularyName": "oncology_terms",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "http://example-bucket.s3.amazonaws.com/vocab.txt",
                "Tags": [{"Key": "team", "Value": "clinical"}, {"Key": "env", "Value": "test"}]
            }),
        ),
        (
            "a name at the modelled 200-character ceiling",
            json!({
                "VocabularyName": "z".repeat(200),
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/long-name-vocab.txt"
            }),
        ),
    ]
}

/// The `VocabularyName` member of a request row.
fn name_of(request: &Value) -> String {
    request["VocabularyName"]
        .as_str()
        .expect("every row names a vocabulary")
        .to_string()
}

/// Whether a member is absent from an answer (or present as JSON `null`), which is
/// how an unset modelled member reaches the wire.
///
/// `pub(crate)` because the NON-medical file-backed vocabulary suite
/// (`crate::vocabulary_file_uri`) asserts the same absences, and one helper is better
/// than two spellings of "the member did not reach the wire".
pub(crate) fn is_absent(reply: &Reply, member: &str) -> bool {
    reply.body.get(member).is_none_or(Value::is_null)
}

/// The `VocabularyName`s of a `ListMedicalVocabularies` answer, in the order the
/// answer listed them.
fn listed_names(reply: &Reply, case: &str) -> Vec<String> {
    reply.ok(case)["Vocabularies"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Vocabularies, got {:?}", reply.body))
        .iter()
        .map(|entry| {
            entry["VocabularyName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a name in {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// A created medical vocabulary is stored, reported back as `PENDING`, readable
/// through `GetMedicalVocabulary`, and ends `FAILED` with a reason naming the file
/// this emulator does not read once the clock makes that transition due.
#[tokio::test]
async fn a_created_medical_vocabulary_is_pending_then_fails_and_reads_back() {
    let transcribe = Transcribe::new();
    let rows = medical_vocabulary_rows();
    let mut created_at: Vec<(&str, String, String, f64)> = Vec::new();

    for (case, request) in &rows {
        let name = name_of(request);
        let uri = request["VocabularyFileUri"]
            .as_str()
            .expect("every row names a file")
            .to_string();

        let created = transcribe
            .call("CreateMedicalVocabulary", request.clone())
            .await;
        created.ok(case);
        assert_eq!(
            created.member(case, "VocabularyName"),
            name,
            "case {case:?}: the answer must name the created vocabulary, got {:?}",
            created.body
        );
        assert_eq!(
            created.member(case, "LanguageCode"),
            MEDICAL_LANGUAGE,
            "case {case:?}: the answer must carry the stored language, got {:?}",
            created.body
        );
        // A new custom medical vocabulary is still being processed: "If the state is
        // READY, you can use the custom vocabulary in a StartMedicalTranscriptionJob
        // request" (model doc for `CreateMedicalVocabularyResponse$VocabularyState`).
        assert_eq!(
            created.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: a fresh medical vocabulary must be PENDING, got {:?}",
            created.body
        );
        assert!(
            is_absent(&created, "FailureReason"),
            "case {case:?}: a PENDING vocabulary has not failed, got {:?}",
            created.body
        );
        let created_time = created.timestamp(case, "LastModifiedTime");

        // The record reads back through the family's own reader, with the same facts.
        let got = transcribe
            .call("GetMedicalVocabulary", json!({"VocabularyName": name}))
            .await;
        got.ok(case);
        assert_eq!(
            (
                got.member(case, "VocabularyName"),
                got.member(case, "LanguageCode"),
                got.member(case, "VocabularyState"),
                got.timestamp(case, "LastModifiedTime"),
            ),
            (
                name.clone(),
                MEDICAL_LANGUAGE.to_string(),
                "PENDING".to_string(),
                created_time
            ),
            "case {case:?}: the reader must report the record just created, got {:?}",
            got.body
        );
        assert!(
            is_absent(&got, "DownloadUri"),
            "case {case:?}: nothing built this vocabulary, so there is no file to \
             download, got {:?}",
            got.body
        );

        created_at.push((case, name, uri, created_time));
    }

    // The transition the clock owes every one of these records: the build that would
    // have had to READ the table file ends in the modelled failure state.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, name, uri, created_time) in &created_at {
        let got = transcribe
            .call("GetMedicalVocabulary", json!({"VocabularyName": name}))
            .await;
        got.ok(case);
        assert_eq!(
            got.member(case, "VocabularyState"),
            "FAILED",
            "case {case:?}: a medical vocabulary whose file is never read must end \
             FAILED, got {:?}",
            got.body
        );
        let reason = got.member(case, "FailureReason");
        assert!(
            reason.contains(uri),
            "case {case:?}: the failure reason must name the file that was not read \
             ({uri}), got {reason:?}"
        );
        let failed_time = got.timestamp(case, "LastModifiedTime");
        assert!(
            failed_time > *created_time,
            "case {case:?}: the record changed when the build ended, so its \
             LastModifiedTime must move past {created_time}, got {failed_time}"
        );
        assert!(
            is_absent(&got, "DownloadUri"),
            "case {case:?}: a FAILED vocabulary has no file to download, got {:?}",
            got.body
        );
    }
}

/// Only US English is accepted, and a request in any other language stores nothing.
#[tokio::test]
async fn a_medical_vocabulary_must_be_us_english() {
    let transcribe = Transcribe::new();

    // Entries of the model's own `LanguageCode` enum
    // (`smithy_query(query="shape", shape="LanguageCode")`), deliberately including
    // ones no recorded request used: each is a valid `LanguageCode` that Amazon
    // Transcribe MEDICAL does not support.
    for language in ["en-GB", "es-US", "ja-JP", "af-ZA", "de-DE"] {
        let refused = transcribe
            .call(
                "CreateMedicalVocabulary",
                json!({
                    "VocabularyName": format!("vocab-{language}"),
                    "LanguageCode": language,
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
                }),
            )
            .await;
        refused.refused(language, 400, "BadRequestException");
        let message = refused.error_message(language);
        assert!(
            message.contains(language) && message.contains(MEDICAL_LANGUAGE),
            "case {language:?}: the refusal must name the language asked for and the \
             only one supported, got {message:?}"
        );
    }

    // en-US is the one row that is accepted.
    transcribe
        .call(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": "vocab-en-US",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
            }),
        )
        .await
        .ok("en-US, the supported language");

    // A refused create stores nothing: the listing holds the accepted row only.
    let listed = transcribe.call("ListMedicalVocabularies", json!({})).await;
    assert_eq!(
        listed_names(&listed, "after the refusals"),
        vec!["vocab-en-US"],
        "a refused create must store nothing, got {:?}",
        listed.body
    );
}

/// A medical vocabulary name is claimed by exactly one create, and the custom
/// vocabulary name space is a different one: the two families are separate resources.
#[tokio::test]
async fn a_medical_vocabulary_name_is_claimed_once_and_the_two_families_are_separate() {
    let transcribe = Transcribe::new();
    let request = json!({
        "VocabularyName": "shared-name",
        "LanguageCode": MEDICAL_LANGUAGE,
        "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
    });

    transcribe
        .call("CreateMedicalVocabulary", request.clone())
        .await
        .ok_status("the first create of a name", 200);

    let conflict = transcribe
        .call("CreateMedicalVocabulary", request.clone())
        .await;
    conflict.refused("a second create of one name", 400, "ConflictException");
    let message = conflict.error_message("a second create of one name");
    assert!(
        message.contains("The requested medical vocabulary name already exists"),
        "the conflict must name the medical vocabulary kind, got {message:?}"
    );
    assert!(
        !message.contains("shared-name"),
        "the conflict must not echo the requested name, got {message:?}"
    );

    // The same name in the NON-medical family is a different resource, so it is free.
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "shared-name",
                "LanguageCode": MEDICAL_LANGUAGE,
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("the same name as a custom vocabulary");

    // And each listing reports only its own family: one entry each, not two.
    let medical = transcribe.call("ListMedicalVocabularies", json!({})).await;
    assert_eq!(
        listed_names(&medical, "the medical listing"),
        vec!["shared-name"],
        "the medical listing must hold exactly the one medical vocabulary, got {:?}",
        medical.body
    );
    let custom = transcribe.call("ListVocabularies", json!({})).await;
    assert_eq!(
        listed_names(&custom, "the custom listing"),
        vec!["shared-name"],
        "the custom listing must hold exactly the one custom vocabulary, got {:?}",
        custom.body
    );

    // A medical vocabulary this account does not hold is not found, whatever the
    // other family holds.
    transcribe
        .call(
            "GetMedicalVocabulary",
            json!({"VocabularyName": "never-created"}),
        )
        .await
        .refused("a medical vocabulary that was never created", 400, "BadRequestException");
}

/// `ListMedicalVocabularies` reports the stored medical vocabularies newest first,
/// filters them by state and by a case-insensitive name substring, echoes the state
/// filter back, and reports only this family.
#[tokio::test]
async fn listing_medical_vocabularies_is_newest_first_and_honours_every_filter() {
    let transcribe = Transcribe::new();
    // Three names, so a `NameContains` row can select a proper subset of them, and
    // one of them differs from another only in letter case so the documented
    // case-insensitivity is exercised.
    for name in ["alpha-Terms", "beta-terms", "gamma-notes"] {
        transcribe
            .call(
                "CreateMedicalVocabulary",
                json!({
                    "VocabularyName": name,
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt"
                }),
            )
            .await
            .ok(name);
    }
    // A NON-medical vocabulary whose name would match the `NameContains` rows: the
    // medical listing must not report it.
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "delta-terms-custom",
                "LanguageCode": MEDICAL_LANGUAGE,
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("a custom vocabulary that is not in this family");

    // Newest first, which is what the request's own documentation states ("Custom
    // vocabularies are ordered by creation date, with the newest vocabulary first").
    let all = transcribe.call("ListMedicalVocabularies", json!({})).await;
    assert_eq!(
        listed_names(&all, "every medical vocabulary"),
        vec!["gamma-notes", "beta-terms", "alpha-Terms"],
        "the listing must be newest first and hold only this family, got {:?}",
        all.body
    );

    // Rows differ along each filter the request publishes, including the
    // case-insensitive substring, a needle that matches nothing, and a needle that
    // matches only the other family's vocabulary.
    let filter_rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "NameContains matching two names regardless of case",
            json!({"NameContains": "TERMS"}),
            vec!["beta-terms", "alpha-Terms"],
        ),
        (
            "NameContains matching one name",
            json!({"NameContains": "gamma"}),
            vec!["gamma-notes"],
        ),
        (
            "NameContains matching nothing",
            json!({"NameContains": "epsilon"}),
            vec![],
        ),
        (
            "NameContains matching only the other family",
            json!({"NameContains": "delta"}),
            vec![],
        ),
        (
            "StateEquals PENDING, which every fresh medical vocabulary is",
            json!({"StateEquals": "PENDING"}),
            vec!["gamma-notes", "beta-terms", "alpha-Terms"],
        ),
        (
            "StateEquals FAILED, which none of them is yet",
            json!({"StateEquals": "FAILED"}),
            vec![],
        ),
        (
            "StateEquals READY, which this emulator builds none of",
            json!({"StateEquals": "READY"}),
            vec![],
        ),
        (
            "both filters together",
            json!({"StateEquals": "PENDING", "NameContains": "terms"}),
            vec!["beta-terms", "alpha-Terms"],
        ),
    ];

    for (case, request, expected) in filter_rows {
        let listed = transcribe
            .call("ListMedicalVocabularies", request.clone())
            .await;
        assert_eq!(
            listed_names(&listed, case),
            expected,
            "case {case:?}: unexpected listing for {request}, got {:?}",
            listed.body
        );
        if let Some(state) = request.get("StateEquals").and_then(Value::as_str) {
            assert_eq!(
                listed.member(case, "Status"),
                state,
                "case {case:?}: the state filter must be echoed back, got {:?}",
                listed.body
            );
        } else {
            assert!(
                is_absent(&listed, "Status"),
                "case {case:?}: a listing that filtered on no state reports none, got \
                 {:?}",
                listed.body
            );
        }
        for entry in listed.body["Vocabularies"].as_array().expect("a list") {
            assert_eq!(
                entry["LanguageCode"],
                json!(MEDICAL_LANGUAGE),
                "case {case:?}: every entry carries its stored language, got {entry:?}"
            );
            assert!(
                entry["LastModifiedTime"].is_number(),
                "case {case:?}: every entry carries its modification time, got {entry:?}"
            );
        }
    }

    // Once the clock-owed transition has fallen due the state filter selects the
    // other way round: the listing reports what the store holds now.
    std::thread::sleep(PAST_THE_TRANSITION);
    let failed = transcribe
        .call("ListMedicalVocabularies", json!({"StateEquals": "FAILED"}))
        .await;
    assert_eq!(
        listed_names(&failed, "StateEquals FAILED once the build fell due"),
        vec!["gamma-notes", "beta-terms", "alpha-Terms"],
        "every medical vocabulary must be FAILED once its build fell due, got {:?}",
        failed.body
    );
    let pending = transcribe
        .call("ListMedicalVocabularies", json!({"StateEquals": "PENDING"}))
        .await;
    assert_eq!(
        listed_names(&pending, "StateEquals PENDING once the build fell due"),
        Vec::<String>::new(),
        "none is still PENDING once its build fell due, got {:?}",
        pending.body
    );
}

/// `ListMedicalVocabularies` pages with its own token at every page size, defaults to
/// the documented page of 5, refuses a `MaxResults` outside the modelled range and a
/// token it never handed out.
#[tokio::test]
async fn listing_medical_vocabularies_pages_with_its_own_token() {
    let transcribe = Transcribe::new();
    // Seven, so the documented default page of 5 is a PROPER prefix of the listing.
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let name = format!("vocab-{index}");
        transcribe
            .call(
                "CreateMedicalVocabulary",
                json!({
                    "VocabularyName": name.clone(),
                    "LanguageCode": MEDICAL_LANGUAGE,
                    "VocabularyFileUri": format!("s3://DOC-EXAMPLE-BUCKET/vocab-{index}.txt")
                }),
            )
            .await
            .ok(&name);
        created.push(name);
    }
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();

    // "If you do not specify a value, a default of 5 is used" (model doc for
    // `ListMedicalVocabulariesRequest$MaxResults`).
    let defaulted = transcribe.call("ListMedicalVocabularies", json!({})).await;
    assert_eq!(
        listed_names(&defaulted, "default page size"),
        newest_first[..5].to_vec(),
        "the documented default page is the newest 5, got {:?}",
        defaulted.body
    );
    assert!(
        defaulted.body["NextToken"].is_string(),
        "a truncated page must carry a NextToken, got {:?}",
        defaulted.body
    );

    // Every page size walks the same sequence, with nothing repeated and nothing
    // missed — including each published bound of the modelled `@range(min: 1,
    // max: 100)`.
    for page_size in [1_usize, 2, 3, 7, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = transcribe.call("ListMedicalVocabularies", request).await;
            let page_names = listed_names(&page, &case);
            assert!(
                page_names.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} entries, got {:?}",
                page.body
            );
            seen.extend(page_names);
            token = page.body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= newest_first.len(),
                "case {case:?}: paging must terminate, got {seen:?}"
            );
        }
        assert_eq!(
            seen, newest_first,
            "case {case:?}: paging must walk every medical vocabulary once, newest \
             first, got {seen:?}"
        );
    }

    // One past each published bound of `MaxResults` is refused at the boundary,
    // before the handler runs.
    for page_size in [0, 101] {
        let case = format!("MaxResults={page_size}, past the modelled range");
        transcribe
            .call("ListMedicalVocabularies", json!({"MaxResults": page_size}))
            .await
            .refused(&case, 400, "ValidationException");
    }

    // A token this service never handed out is refused rather than silently
    // restarting the listing from the top.
    transcribe
        .call("ListMedicalVocabularies", json!({"NextToken": "not-a-token"}))
        .await
        .refused("a token this service never handed out", 400, "BadRequestException");
}

/// A medical vocabulary carries the tags its create named, and the three tagging
/// operations address it through its own ARN resource type.
#[tokio::test]
async fn tags_round_trip_on_a_medical_vocabulary() {
    let transcribe = Transcribe::new();
    transcribe
        .call(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": "tagged-medical-vocab",
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt",
                "Tags": [{"Key": "team", "Value": "clinical"}]
            }),
        )
        .await
        .ok("create the medical vocabulary to tag");

    // The resource type is `medical-vocabulary`, the one the Amazon Transcribe
    // "Tagging resources" guide lists for this family.
    let resource_arn = format!(
        "arn:aws:transcribe:{REGION}:{account}:medical-vocabulary/tagged-medical-vocab",
        account = engine::emulator_account_id()
    );

    let listed = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    listed.ok("the tags the create named");
    assert_eq!(
        listed.body["Tags"],
        json!([{"Key": "team", "Value": "clinical"}]),
        "the create's own tags must be stored on the record, got {:?}",
        listed.body
    );

    transcribe
        .call(
            "TagResource",
            json!({
                "ResourceArn": resource_arn,
                "Tags": [{"Key": "env", "Value": "test"}]
            }),
        )
        .await
        .ok("tag the medical vocabulary");
    let after_tag = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    after_tag.ok("after tagging");
    assert_eq!(
        after_tag.body["Tags"],
        json!([{"Key": "team", "Value": "clinical"}, {"Key": "env", "Value": "test"}]),
        "tagging must merge into the stored set, got {:?}",
        after_tag.body
    );

    transcribe
        .call(
            "UntagResource",
            json!({"ResourceArn": resource_arn, "TagKeys": ["team"]}),
        )
        .await
        .ok_status("untag the medical vocabulary", 200);
    let after_untag = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    after_untag.ok("after untagging");
    assert_eq!(
        after_untag.body["Tags"],
        json!([{"Key": "env", "Value": "test"}]),
        "untagging must remove exactly the named keys, got {:?}",
        after_untag.body
    );

    // A medical vocabulary ARN naming a record this account does not hold is this
    // service's `BadRequestException`, not an empty tag set.
    transcribe
        .call(
            "ListTagsForResource",
            json!({
                "ResourceArn": format!(
                    "arn:aws:transcribe:{REGION}:{account}:medical-vocabulary/never-created",
                    account = engine::emulator_account_id()
                )
            }),
        )
        .await
        .refused("a medical vocabulary that does not exist", 400, "BadRequestException");
}
