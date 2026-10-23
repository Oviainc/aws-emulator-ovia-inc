// SPDX-License-Identifier: CC0-1.0

//! The custom-vocabulary family, end to end over the wire: `CreateVocabulary` ->
//! `GetVocabulary` -> `UpdateVocabulary` -> `ListVocabularies` ->
//! `DeleteVocabulary`, the same five for vocabulary filters, and the three tagging
//! operations over every resource type an Amazon Transcribe ARN can name.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time; this suite adds only what is specific to the vocabulary family.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::{dispatch, Reply, Transcribe};

/// Long enough for the `PENDING -> READY` transition of a vocabulary created during
/// the test to have fallen due: one span of `crate::state::VOCABULARY_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so `tokio::time::sleep` is not
/// available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The region every row of this suite is served in, so a reported `DownloadUri` and a
/// tagging ARN are both predictable.
///
/// The rows are unsigned, and `crate::state::DEFAULT_REGION` is the region this
/// emulator serves a request that carries no SigV4 credential scope in; the
/// transcription-job suite covers the signed path, where the scope's region is used
/// instead.
const REGION: &str = "us-east-1";

/// A valid `DataAccessRoleArn`, which the member's `@pattern` pins to an IAM role
/// ARN (`smithy_query(query="operation", operation="CreateVocabulary")`).
const DATA_ACCESS_ROLE: &str = "arn:aws:iam::000000000000:role/TranscribeAccessRole";

/// The class of custom vocabulary a `CreateVocabulary` request can describe, one row
/// per axis the rules generalise over: how many terms it lists, what those terms
/// look like, which of the model's `LanguageCode` values it is in, and whether it
/// carries tags, an encryption configuration and a data-access role.
///
/// The language codes are entries of the model's own `LanguageCode` enum
/// (`smithy_query(query="shape", shape="LanguageCode")`), deliberately including
/// ones no recorded request used.
fn vocabulary_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one term, en-US",
            json!({
                "VocabularyName": "one-term",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        ),
        (
            "several terms with hyphenated and dotted forms, en-GB",
            json!({
                "VocabularyName": "several.terms-v2",
                "LanguageCode": "en-GB",
                "Phrases": ["Los-Angeles", "C.L.I.", "Amazon Transcribe"]
            }),
        ),
        (
            "non-latin terms, ja-JP",
            json!({
                "VocabularyName": "ja_terms",
                "LanguageCode": "ja-JP",
                "Phrases": ["アマゾン", "東京"]
            }),
        ),
        (
            "a term at the published 256-character ceiling, de-DE",
            json!({
                "VocabularyName": "long-term",
                "LanguageCode": "de-DE",
                "Phrases": ["a".repeat(256)]
            }),
        ),
        (
            "tags, a data-access role and an encryption configuration, es-US",
            json!({
                "VocabularyName": "fully-specified",
                "LanguageCode": "es-US",
                "Phrases": ["Bogotá"],
                "Tags": [{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}],
                "DataAccessRoleArn": DATA_ACCESS_ROLE,
                "EncryptionConfiguration": {"KMSKey": "alias/aws/transcribe"}
            }),
        ),
    ]
}

/// A created vocabulary is stored, reported back as `PENDING`, readable through
/// `GetVocabulary`, and becomes `READY` once the clock says its processing finished.
#[tokio::test]
async fn a_created_vocabulary_is_pending_then_ready_and_reads_back() {
    let transcribe = Transcribe::new();

    for (case, request) in vocabulary_rows() {
        let name = request["VocabularyName"].as_str().expect("a name").to_string();
        let language = request["LanguageCode"].as_str().expect("a code").to_string();

        let created = transcribe.call("CreateVocabulary", request.clone()).await;
        created.ok(case);
        assert_eq!(
            created.member(case, "VocabularyName"),
            name,
            "case {case:?}: the answer must name the created vocabulary, got {:?}",
            created.body
        );
        assert_eq!(
            created.member(case, "LanguageCode"),
            language,
            "case {case:?}: the answer must carry the requested language, got {:?}",
            created.body
        );
        assert_eq!(
            created.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: a new custom vocabulary is PENDING, got {:?}",
            created.body
        );
        assert!(
            created.body.get("LastModifiedTime").is_some(),
            "case {case:?}: the answer must carry LastModifiedTime, got {:?}",
            created.body
        );

        // Read back through the implemented reader: the store, not the request.
        let read = transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await;
        read.ok(case);
        assert_eq!(
            read.member(case, "VocabularyName"),
            name,
            "case {case:?}: GetVocabulary must answer the created vocabulary, got {:?}",
            read.body
        );
        assert_eq!(
            read.member(case, "LanguageCode"),
            language,
            "case {case:?}: GetVocabulary must report the stored language, got {:?}",
            read.body
        );
        assert_eq!(
            read.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: reading a vocabulary must not finish its processing, got {:?}",
            read.body
        );
        let download_uri = read.member(case, "DownloadUri");
        assert!(
            download_uri.starts_with("https://") && download_uri.ends_with(&name),
            "case {case:?}: DownloadUri must name this vocabulary's own location, got \
             {download_uri:?}"
        );
        if let Some(role) = request.get("DataAccessRoleArn").and_then(Value::as_str) {
            assert_eq!(
                read.member(case, "DataAccessRoleArn"),
                role,
                "case {case:?}: the stored data-access role must read back, got {:?}",
                read.body
            );
        }
    }

    // The transition the clock owed every one of them has now fallen due.
    std::thread::sleep(PAST_THE_TRANSITION);
    for (case, request) in vocabulary_rows() {
        let name = request["VocabularyName"].as_str().expect("a name").to_string();
        let read = transcribe
            .call("GetVocabulary", json!({"VocabularyName": name}))
            .await;
        read.ok(case);
        assert_eq!(
            read.member(case, "VocabularyState"),
            "READY",
            "case {case:?}: the vocabulary must be READY once its processing fell due, \
             got {:?}",
            read.body
        );
    }
}

/// Every way a create can fail to say where its terms come from, for both the
/// vocabulary and the vocabulary-filter operations: each is the modelled
/// `BadRequestException`, and nothing is stored.
#[tokio::test]
async fn a_create_without_usable_inline_terms_is_a_bad_request() {
    let transcribe = Transcribe::new();

    // Rows differ along the axis the rule generalises over: which of the two content
    // members the request carries (neither, both, the file one), and the form of the
    // file URI the model's `Uri` pattern allows (`s3://`, `http://`, `https://`).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "vocabulary with neither Phrases nor VocabularyFileUri",
            "CreateVocabulary",
            json!({"VocabularyName": "no-content", "LanguageCode": "en-US"}),
        ),
        (
            "vocabulary with an empty Phrases list",
            "CreateVocabulary",
            json!({"VocabularyName": "empty-phrases", "LanguageCode": "en-US", "Phrases": []}),
        ),
        (
            "vocabulary with both Phrases and VocabularyFileUri",
            "CreateVocabulary",
            json!({
                "VocabularyName": "both-sources",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"],
                "VocabularyFileUri": "s3://example-bucket/vocab.txt"
            }),
        ),
        (
            "vocabulary from an s3 file this emulator cannot read",
            "CreateVocabulary",
            json!({
                "VocabularyName": "from-s3",
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://example-bucket/vocab.txt"
            }),
        ),
        (
            "vocabulary from an https file this emulator cannot read",
            "CreateVocabulary",
            json!({
                "VocabularyName": "from-https",
                "LanguageCode": "en-US",
                "VocabularyFileUri": "https://example.invalid/vocab.txt"
            }),
        ),
        (
            "filter with neither Words nor VocabularyFilterFileUri",
            "CreateVocabularyFilter",
            json!({"VocabularyFilterName": "no-words", "LanguageCode": "en-US"}),
        ),
        (
            "filter with both Words and VocabularyFilterFileUri",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "both-word-sources",
                "LanguageCode": "en-US",
                "Words": ["crud"],
                "VocabularyFilterFileUri": "s3://example-bucket/words.txt"
            }),
        ),
        (
            "filter from an s3 file this emulator cannot read",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "words-from-s3",
                "LanguageCode": "en-US",
                "VocabularyFilterFileUri": "s3://example-bucket/words.txt"
            }),
        ),
        (
            // Past the published 50 KB ceiling on a custom vocabulary's size: 256
            // terms of 256 characters each is 65,536 bytes.
            "vocabulary whose terms are past the published size ceiling",
            "CreateVocabulary",
            json!({
                "VocabularyName": "too-many-terms",
                "LanguageCode": "en-US",
                "Phrases": (0..256).map(|_| "a".repeat(256)).collect::<Vec<String>>()
            }),
        ),
        (
            "filter whose words are past the published size ceiling",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "too-many-words",
                "LanguageCode": "en-US",
                "Words": (0..256).map(|_| "w".repeat(256)).collect::<Vec<String>>()
            }),
        ),
    ];

    for (case, operation, request) in rows {
        let refused = transcribe.call(operation, request.clone()).await;
        refused.refused(case, 400, "BadRequestException");

        // Nothing was stored for a refused create.
        let (reader, key, name) = if operation == "CreateVocabulary" {
            (
                "GetVocabulary",
                "VocabularyName",
                request["VocabularyName"].as_str().expect("a name"),
            )
        } else {
            (
                "GetVocabularyFilter",
                "VocabularyFilterName",
                request["VocabularyFilterName"].as_str().expect("a name"),
            )
        };
        let read = transcribe.call(reader, json!({key: name})).await;
        read.refused(case, 400, "BadRequestException");
    }
}

/// A name already in use is refused with the modelled conflict — once sequentially,
/// and once with several racers creating the same name at the same time, where
/// exactly one wins.
#[tokio::test(flavor = "multi_thread")]
async fn a_vocabulary_name_is_claimed_by_exactly_one_create() {
    let transcribe = Transcribe::new();
    let request = json!({
        "VocabularyName": "claimed-name",
        "LanguageCode": "en-US",
        "Phrases": ["Aardvark"]
    });

    transcribe
        .call("CreateVocabulary", request.clone())
        .await
        .ok("the first create of a free name");
    let again = transcribe.call("CreateVocabulary", request.clone()).await;
    again.refused("a second create of the same name", 400, "ConflictException");

    // Rows differ in how many racers contend for one fresh name; the invariant is
    // the same for each: one winner, every other racer the modelled conflict.
    for racers in [2_usize, 8] {
        let name = format!("race-{racers}");
        let mut tasks = Vec::new();
        for _ in 0..racers {
            let router = Arc::clone(&transcribe.router);
            let request = json!({
                "VocabularyName": name.clone(),
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            });
            tasks.push(tokio::spawn(async move {
                let reply = dispatch(&router, "CreateVocabulary", request, None).await;
                (reply.status, reply.error_code().to_string())
            }));
        }
        let mut outcomes = Vec::new();
        for task in tasks {
            outcomes.push(task.await.expect("the racer ran"));
        }
        let winners = outcomes.iter().filter(|(status, _)| *status == 200).count();
        let conflicts = outcomes
            .iter()
            .filter(|(status, code)| *status == 400 && code == "ConflictException")
            .count();
        assert_eq!(
            winners, 1,
            "case racers={racers}: exactly one create of {name:?} may win, got {outcomes:?}"
        );
        assert_eq!(
            conflicts,
            racers - 1,
            "case racers={racers}: every other racer must get the modelled conflict, got \
             {outcomes:?}"
        );
    }
}

/// A resource the service is still processing cannot be deleted; once its processing
/// has fallen due the same delete goes through and the resource is gone.
#[tokio::test]
async fn a_vocabulary_being_processed_cannot_be_deleted() {
    let transcribe = Transcribe::new();
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "pending-delete",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("create the vocabulary to delete");

    let while_pending = transcribe
        .call("DeleteVocabulary", json!({"VocabularyName": "pending-delete"}))
        .await;
    while_pending.refused("delete while PENDING", 400, "BadRequestException");

    // Still there: a refused delete removes nothing.
    transcribe
        .call("GetVocabulary", json!({"VocabularyName": "pending-delete"}))
        .await
        .ok("the vocabulary survived the refused delete");

    std::thread::sleep(PAST_THE_TRANSITION);
    transcribe
        .call("DeleteVocabulary", json!({"VocabularyName": "pending-delete"}))
        .await
        .ok_status("delete once READY", 200);
    transcribe
        .call("GetVocabulary", json!({"VocabularyName": "pending-delete"}))
        .await
        .refused("read a deleted vocabulary", 400, "BadRequestException");
    // A delete is IDEMPOTENT: a name this account holds no vocabulary under answers
    // the same empty success, not a refusal — `crate::vocabulary_delete_and_tag_sets`
    // spans that class, this row keeps the lifecycle honest about it.
    crate::vocabulary_delete_and_tag_sets::succeeded(
        &transcribe
            .call("DeleteVocabulary", json!({"VocabularyName": "never-created"}))
            .await,
        "delete a vocabulary that does not exist",
    );
}

/// `UpdateVocabulary` replaces the stored terms and puts the vocabulary back into
/// `PENDING`, for several replacement term lists; an update of a name that does not
/// exist is this service's `BadRequestException`.
#[tokio::test]
async fn updating_a_vocabulary_resets_its_state() {
    let transcribe = Transcribe::new();
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "updated",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("create the vocabulary to update");
    std::thread::sleep(PAST_THE_TRANSITION);
    assert_eq!(
        transcribe
            .call("GetVocabulary", json!({"VocabularyName": "updated"}))
            .await
            .member("before the update", "VocabularyState"),
        "READY",
        "the vocabulary must be READY before the update, or the reset proves nothing"
    );

    // Rows differ in the replacement content and in the language the rebuilt
    // vocabulary is in, which `UpdateVocabulary` also requires.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one replacement term, same language",
            json!({
                "VocabularyName": "updated",
                "LanguageCode": "en-US",
                "Phrases": ["Zebra"]
            }),
        ),
        (
            "several replacement terms in another language",
            json!({
                "VocabularyName": "updated",
                "LanguageCode": "fr-FR",
                "Phrases": ["Aardvark", "Zèbre", "Gnou"]
            }),
        ),
    ];

    for (case, request) in rows {
        let language = request["LanguageCode"].as_str().expect("a code").to_string();
        let updated = transcribe.call("UpdateVocabulary", request).await;
        updated.ok(case);
        assert_eq!(
            updated.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: an updated vocabulary is PENDING again, got {:?}",
            updated.body
        );
        let read = transcribe
            .call("GetVocabulary", json!({"VocabularyName": "updated"}))
            .await;
        read.ok(case);
        assert_eq!(
            read.member(case, "VocabularyState"),
            "PENDING",
            "case {case:?}: the stored vocabulary must be PENDING after the update, got \
             {:?}",
            read.body
        );
        assert_eq!(
            read.member(case, "LanguageCode"),
            language,
            "case {case:?}: the update must replace the stored language, got {:?}",
            read.body
        );
        std::thread::sleep(PAST_THE_TRANSITION);
        assert_eq!(
            transcribe
                .call("GetVocabulary", json!({"VocabularyName": "updated"}))
                .await
                .member(case, "VocabularyState"),
            "READY",
            "case {case:?}: the rebuilt vocabulary must become READY again"
        );
    }

    transcribe
        .call(
            "UpdateVocabulary",
            json!({
                "VocabularyName": "never-created",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .refused(
            "update a vocabulary that does not exist",
            400,
            "BadRequestException",
        );
}

/// `ListVocabularies` reports the stored vocabularies newest first, filters them by
/// state and by a case-insensitive name substring, and pages with its own token.
#[tokio::test]
async fn listing_vocabularies_filters_and_pages() {
    let transcribe = Transcribe::new();
    // Three names, so a `NameContains` row can select a proper subset of them, and
    // one of them differs only in letter case so the documented case-insensitivity
    // is exercised.
    for name in ["alpha-Report", "beta-report", "gamma-notes"] {
        transcribe
            .call(
                "CreateVocabulary",
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark"]
                }),
            )
            .await
            .ok(name);
    }

    let names = |reply: &Reply, case: &str| -> Vec<String> {
        reply.ok(case)["Vocabularies"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Vocabularies, got {:?}", reply.body))
            .iter()
            .map(|entry| {
                entry["VocabularyName"]
                    .as_str()
                    .expect("a vocabulary name")
                    .to_string()
            })
            .collect()
    };

    // Newest first, which is what the response's own documentation states
    // ("Vocabularies are ordered by creation date, with the newest vocabulary
    // first").
    let all = transcribe.call("ListVocabularies", json!({})).await;
    assert_eq!(
        names(&all, "every vocabulary"),
        vec!["gamma-notes", "beta-report", "alpha-Report"],
        "the listing must be newest first, got {:?}",
        all.body
    );

    // Rows differ along each filter the request publishes, including the
    // case-insensitive substring and a needle that matches nothing.
    let filter_rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "NameContains matching two names regardless of case",
            json!({"NameContains": "REPORT"}),
            vec!["beta-report", "alpha-Report"],
        ),
        (
            "NameContains matching one name",
            json!({"NameContains": "gamma"}),
            vec!["gamma-notes"],
        ),
        (
            "NameContains matching nothing",
            json!({"NameContains": "delta"}),
            vec![],
        ),
        (
            "StateEquals PENDING, which every fresh vocabulary is",
            json!({"StateEquals": "PENDING"}),
            vec!["gamma-notes", "beta-report", "alpha-Report"],
        ),
        (
            "StateEquals READY, which none of them is yet",
            json!({"StateEquals": "READY"}),
            vec![],
        ),
        (
            "both filters together",
            json!({"StateEquals": "PENDING", "NameContains": "report"}),
            vec!["beta-report", "alpha-Report"],
        ),
    ];

    for (case, request, expected) in filter_rows {
        let listed = transcribe.call("ListVocabularies", request.clone()).await;
        assert_eq!(
            names(&listed, case),
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
        }
    }

    // Once the clock-owed transition has fallen due the state filter selects the
    // other way round: the listing reports what the store holds now.
    std::thread::sleep(PAST_THE_TRANSITION);
    let ready = transcribe
        .call("ListVocabularies", json!({"StateEquals": "READY"}))
        .await;
    assert_eq!(
        names(&ready, "StateEquals READY once processing fell due").len(),
        3,
        "every vocabulary must be READY once its processing fell due, got {:?}",
        ready.body
    );

    // Paging: one row per page size, each walking the whole listing through the
    // service's own tokens.
    for page_size in [1_usize, 2, 3] {
        let case = format!("page size {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = transcribe.call("ListVocabularies", request).await;
            let page_names = names(&page, &case);
            assert!(
                page_names.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} entries, got {:?}",
                page.body
            );
            seen.extend(page_names);
            token = page
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            vec!["gamma-notes", "beta-report", "alpha-Report"],
            "case {case:?}: paging must walk every vocabulary exactly once, got {seen:?}"
        );
    }

    transcribe
        .call("ListVocabularies", json!({"NextToken": "not-a-token"}))
        .await
        .refused("a token this service never handed out", 400, "BadRequestException");
}

/// The vocabulary-filter family round trips: a filter created from words is readable,
/// its words are replaceable, it is listed by a case-insensitive name substring, and
/// it is deletable.
#[tokio::test]
async fn the_vocabulary_filter_family_round_trips() {
    let transcribe = Transcribe::new();

    // Rows differ in how many words the filter holds, their form, and its language.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one word, en-US",
            json!({
                "VocabularyFilterName": "one-word",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        ),
        (
            "several words with punctuation, en-AU",
            json!({
                "VocabularyFilterName": "several-words",
                "LanguageCode": "en-AU",
                "Words": ["crud", "dash-it", "blimey"]
            }),
        ),
        (
            "non-latin words and tags, ru-RU",
            json!({
                "VocabularyFilterName": "ru.words",
                "LanguageCode": "ru-RU",
                "Words": ["чепуха"],
                "Tags": [{"Key": "team", "Value": "speech"}]
            }),
        ),
    ];

    for (case, request) in &rows {
        let name = request["VocabularyFilterName"]
            .as_str()
            .expect("a name")
            .to_string();
        let language = request["LanguageCode"].as_str().expect("a code").to_string();
        let created = transcribe
            .call("CreateVocabularyFilter", request.clone())
            .await;
        // Every success of this awsJson1_1 service is 200: the model's `@http`
        // `code: 201` is inert on an RPC protocol.
        created.ok_status(case, 200);
        assert_eq!(
            created.member(case, "VocabularyFilterName"),
            name,
            "case {case:?}: the answer must name the created filter, got {:?}",
            created.body
        );

        let read = transcribe
            .call("GetVocabularyFilter", json!({"VocabularyFilterName": name}))
            .await;
        read.ok(case);
        assert_eq!(
            read.member(case, "LanguageCode"),
            language,
            "case {case:?}: the stored language must read back, got {:?}",
            read.body
        );
        let download_uri = read.member(case, "DownloadUri");
        assert!(
            download_uri.starts_with("https://") && download_uri.ends_with(&name),
            "case {case:?}: DownloadUri must name this filter's own location, got \
             {download_uri:?}"
        );

        // Replacing the words keeps the stored language, which this operation does
        // not accept, and moves LastModifiedTime no further back. The timestamp is an
        // awsJson epoch-seconds number, so it is read as one.
        let before = read.timestamp(case, "LastModifiedTime");
        let updated = transcribe
            .call(
                "UpdateVocabularyFilter",
                json!({"VocabularyFilterName": name, "Words": ["replaced", "words"]}),
            )
            .await;
        updated.ok(case);
        assert_eq!(
            updated.member(case, "LanguageCode"),
            language,
            "case {case:?}: an update must keep the stored language, got {:?}",
            updated.body
        );
        let after = updated.timestamp(case, "LastModifiedTime");
        assert!(
            after >= before,
            "case {case:?}: LastModifiedTime must not go backwards ({before} -> {after})"
        );
    }

    let listed = transcribe
        .call("ListVocabularyFilters", json!({"NameContains": "WORDS"}))
        .await;
    let names: Vec<&str> = listed.ok("filters whose name contains WORDS")["VocabularyFilters"]
        .as_array()
        .expect("VocabularyFilters")
        .iter()
        .map(|entry| {
            entry["VocabularyFilterName"]
                .as_str()
                .expect("a filter name")
        })
        .collect();
    assert_eq!(
        names,
        vec!["ru.words", "several-words"],
        "the filter listing must be newest first, match case-insensitively and select \
         only the names that contain the needle, got {:?}",
        listed.body
    );

    for (case, request) in &rows {
        let name = request["VocabularyFilterName"].as_str().expect("a name");
        transcribe
            .call("DeleteVocabularyFilter", json!({"VocabularyFilterName": name}))
            .await
            .ok_status(case, 200);
        transcribe
            .call("GetVocabularyFilter", json!({"VocabularyFilterName": name}))
            .await
            .refused(case, 400, "BadRequestException");
        // The same delete, run again now that the filter is gone, is the same empty
        // success: a delete of something already absent is not an error.
        crate::vocabulary_delete_and_tag_sets::succeeded(
            &transcribe
                .call("DeleteVocabularyFilter", json!({"VocabularyFilterName": name}))
                .await,
            case,
        );
    }
}

/// Tags round trip on every resource type an Amazon Transcribe ARN this emulator
/// stores can name, and an ARN that names no stored resource is this service's
/// `BadRequestException`.
#[tokio::test]
async fn tags_round_trip_on_each_taggable_resource() {
    let transcribe = Transcribe::new();

    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "tagged-vocab",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("create the vocabulary to tag");
    transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "tagged-filter",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await
        .ok_status("create the filter to tag", 200);
    transcribe
        .call(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "tagged-job",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                "LanguageCode": "en-US"
            }),
        )
        .await
        .ok("start the job to tag");

    let arn = |resource_type: &str, name: &str| {
        format!(
            "arn:aws:transcribe:{REGION}:{account}:{resource_type}/{name}",
            account = engine::emulator_account_id()
        )
    };

    // One row per resource type the three tagging operations address, so nothing
    // here passes for one recorded resource kind.
    let rows: Vec<(&str, String)> = vec![
        ("vocabulary", arn("vocabulary", "tagged-vocab")),
        ("vocabulary filter", arn("vocabulary-filter", "tagged-filter")),
        ("transcription job", arn("transcription-job", "tagged-job")),
    ];

    for (case, resource_arn) in &rows {
        // A resource that carries no tags reports an EMPTY tag set: `Tags` is the
        // answer to "which tags does this resource have", so it is present whether or
        // not the set holds anything.
        let empty = transcribe
            .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
            .await;
        empty.ok(case);
        assert_eq!(
            empty.body.get("Tags"),
            Some(&json!([])),
            "case {case:?}: an untagged resource must report an empty Tags list, got {:?}",
            empty.body
        );

        transcribe
            .call(
                "TagResource",
                json!({
                    "ResourceArn": resource_arn,
                    "Tags": [{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "dev"}]
                }),
            )
            .await
            .ok(case);
        // A second tag of a key already present replaces that key's value.
        transcribe
            .call(
                "TagResource",
                json!({
                    "ResourceArn": resource_arn,
                    "Tags": [{"Key": "env", "Value": "prod"}]
                }),
            )
            .await
            .ok(case);

        let listed = transcribe
            .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
            .await;
        listed.ok(case);
        assert_eq!(
            listed.member(case, "ResourceArn"),
            *resource_arn,
            "case {case:?}: the answer must name the resource asked about, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.body["Tags"],
            json!([{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "prod"}]),
            "case {case:?}: the tag set must hold one value per key, got {:?}",
            listed.body
        );

        transcribe
            .call(
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["env", "not-a-tag-key"]}),
            )
            .await
            .ok_status(case, 200);
        let after = transcribe
            .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
            .await;
        after.ok(case);
        assert_eq!(
            after.body["Tags"],
            json!([{"Key": "team", "Value": "speech"}]),
            "case {case:?}: untagging must remove exactly the named keys, got {:?}",
            after.body
        );
    }

    // Rows differ in WHY the ARN names nothing: a name that was never created, and a
    // resource type this emulator stores no resources of.
    let unknown: Vec<(&str, String)> = vec![
        ("a vocabulary that does not exist", arn("vocabulary", "never-created")),
        ("a job that does not exist", arn("transcription-job", "never-started")),
        ("a resource type this emulator holds none of", arn("language-model", "any-model")),
    ];
    for (case, resource_arn) in unknown {
        transcribe
            .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
            .await
            .refused(case, 400, "BadRequestException");
        transcribe
            .call(
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": [{"Key": "team", "Value": "speech"}]}),
            )
            .await
            .refused(case, 400, "BadRequestException");
        transcribe
            .call(
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["team"]}),
            )
            .await
            .refused(case, 400, "BadRequestException");
    }
}
