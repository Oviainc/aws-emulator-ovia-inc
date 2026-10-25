// SPDX-License-Identifier: CC0-1.0

//! `ListLanguageModels`, end to end over the wire: `CreateLanguageModel` ->
//! `ListLanguageModels` -> `DescribeLanguageModel` -> `DeleteLanguageModel`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), and the create helper, the request table and
//! the transition wait are the language-model suite's (`crate::language_models`),
//! reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: `ListLanguageModels` reports the custom language models this
//! account holds — and only those — each projected onto the same `LanguageModel` shape
//! `DescribeLanguageModel` reports, newest first ("Language models are ordered by
//! creation date, with the newest model first"), narrowed by `StatusEquals` and by a
//! case-insensitive `NameContains` substring search, in pages of `MaxResults` (default
//! 5) walked by a `NextToken` this service handed out and refusing any other cursor —
//! and it changes nothing a later caller sees. Each test asserts that over a table or a
//! set of inputs spanning what the rule generalises over (model count, page size, name
//! case and form, status, and the members a projection carries), not over one row.

use serde_json::{json, Value};

use crate::language_models::{
    create, language_model_request, language_model_rows, PAST_THE_TRANSITION,
};
use crate::transcription_jobs::{Reply, Transcribe};

/// `ListLanguageModels` with `request` as its input.
async fn list(transcribe: &Transcribe, request: Value) -> Reply {
    transcribe.call("ListLanguageModels", request).await
}

/// The `Models` of a listing answer.
fn models_of(body: &Value) -> &Vec<Value> {
    body["Models"].as_array().expect("Models must be a list")
}

/// The model names of a listing's answer, in the order the page reported them.
fn names_of(body: &Value) -> Vec<String> {
    models_of(body)
        .iter()
        .map(|model| {
            model["ModelName"]
                .as_str()
                .expect("every listed model names itself")
                .to_string()
        })
        .collect()
}

/// The `ModelName` of a create request row.
fn name_of(request: &Value) -> String {
    request["ModelName"]
        .as_str()
        .expect("every row names a model")
        .to_string()
}

/// Create every row of [`language_model_rows`], in table order, and answer their names.
async fn create_the_table(transcribe: &Transcribe) -> Vec<String> {
    let mut names = Vec::new();
    for (case, request) in language_model_rows() {
        create(transcribe, case, &request).await;
        names.push(name_of(&request));
    }
    names
}

/// Each listed model is the SAME projection `DescribeLanguageModel` reports for that
/// record: a listing is a report of the store, not a second rendering of it.
#[tokio::test]
async fn every_listed_model_is_the_projection_the_describe_reports() {
    let transcribe = Transcribe::new();
    let names = create_the_table(&transcribe).await;

    // One wait for the whole table, not one per row, so every record is past its
    // clock-owed transition and the two reads cannot straddle it.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in language_model_rows() {
        let name = name_of(&request);
        let described = transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name.clone()}))
            .await;
        let expected = described.ok(case)["LanguageModel"].clone();

        let listed = list(&transcribe, json!({"MaxResults": 100})).await;
        let body = listed.ok(case).clone();
        let model = models_of(&body)
            .iter()
            .find(|model| model["ModelName"] == json!(name.clone()))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the model must appear in the listing, got {body:?}")
            });
        assert_eq!(
            model, expected,
            "case {case:?}: the listing must report the record the describe reports, \
             got {model:?}"
        );
        // Every member the create named reaches the listed projection, so an answer
        // sourced from another record cannot pass.
        assert_eq!(
            (
                model["LanguageCode"].clone(),
                model["BaseModelName"].clone(),
                model["InputDataConfig"].clone(),
            ),
            (
                request["LanguageCode"].clone(),
                request["BaseModelName"].clone(),
                request["InputDataConfig"].clone(),
            ),
            "case {case:?}: unexpected projection, got {model:?}"
        );
    }

    // Listing is a REPORT: a second listing answers exactly what the first did, and
    // every record is still readable afterwards.
    let first = list(&transcribe, json!({"MaxResults": 100})).await;
    let second = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        first.ok("first listing"),
        second.ok("second listing"),
        "a listing must not change what a later listing sees, got {:?} then {:?}",
        first.body,
        second.body
    );
    for name in &names {
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name.clone()}))
            .await
            .ok(name);
    }
}

/// The listing is every stored model, newest first, over every page size and both
/// filters.
#[tokio::test]
async fn the_listing_is_every_stored_model_newest_first_paged_and_filtered() {
    let transcribe = Transcribe::new();

    // Eleven models, more than the documented default page of 5, in two name families
    // of different case so `NameContains` has something to narrow and something to
    // match case-insensitively. The languages cycle through entries of the model's own
    // `CLMLanguageCode` enum and the base models through the whole `BaseModelName`
    // enum (`smithy_query(query="shape", shape="CLMLanguageCode,BaseModelName")`).
    let languages = ["en-US", "hi-IN", "es-US", "en-GB", "en-AU", "de-DE", "ja-JP"];
    let mut created: Vec<String> = Vec::new();
    for index in 0..11_usize {
        let name = if index % 2 == 0 {
            format!("alpha-{index}")
        } else {
            format!("BETA-{index}")
        };
        let base_model = if index % 3 == 0 {
            "NarrowBand"
        } else {
            "WideBand"
        };
        let request = language_model_request(&name, languages[index % languages.len()], base_model);
        create(&transcribe, &name, &request).await;
        created.push(name);
    }

    // Newest first: the reverse of the creation order.
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();
    let all = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        names_of(all.ok("unpaged listing")),
        newest_first,
        "the listing must be newest first, got {:?}",
        all.body["Models"]
    );

    // Every page size pages through exactly the same sequence, with no model repeated
    // and none missed. 1 and 100 are the published bounds of `MaxResults`
    // (`@range(min: 1, max: 100)`).
    for page_size in [1_u64, 2, 3, 7, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = list(&transcribe, request).await;
            let body = page.ok(&case);
            let page_names = names_of(body);
            assert!(
                page_names.len() as u64 <= page_size,
                "case {case:?}: a page must hold at most {page_size} models, got \
                 {page_names:?}"
            );
            seen.extend(page_names);
            token = body["NextToken"].as_str().map(str::to_string);
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
            "case {case:?}: paging must walk every model once, newest first, got {seen:?}"
        );
    }

    // An unfiltered listing with no MaxResults returns the documented default page of
    // 5, with a cursor to the rest.
    let defaulted = list(&transcribe, json!({})).await;
    let body = defaulted.ok("default page size");
    assert_eq!(
        names_of(body).len(),
        5,
        "the documented default page size is 5, got {:?}",
        body["Models"]
    );
    assert!(
        body["NextToken"].is_string(),
        "a truncated page must carry a NextToken, got {:?}",
        body["NextToken"]
    );

    // `NameContains` is a case-insensitive substring search.
    let beta: Vec<String> = created
        .iter()
        .rev()
        .filter(|name| name.starts_with("BETA"))
        .cloned()
        .collect();
    let contains_cases: &[(&str, Vec<String>)] = &[
        ("beta", beta.clone()),
        ("BETA", beta.clone()),
        ("Beta", beta),
        ("alpha-4", vec!["alpha-4".to_string()]),
        ("-", newest_first.clone()),
        ("zeta", Vec::new()),
    ];
    for (needle, expected) in contains_cases {
        let filtered = list(
            &transcribe,
            json!({"NameContains": needle, "MaxResults": 100}),
        )
        .await;
        assert_eq!(
            &names_of(filtered.ok(needle)),
            expected,
            "case {needle:?}: unexpected NameContains result, got {:?}",
            filtered.body["Models"]
        );
    }

    // The `StatusEquals` filter selects only models in that status, and the three
    // modelled statuses together still cover every model. Waiting past the transition
    // first keeps the partition from being read mid-transition, and the assertion is
    // on the UNION of the names rather than on a sum, so it cannot depend on where the
    // clock is between two of the calls.
    std::thread::sleep(PAST_THE_TRANSITION);
    let mut covered: Vec<String> = Vec::new();
    // Every value of `ModelStatus`
    // (`smithy_query(query="shape", shape="ModelStatus")`).
    for status in ["IN_PROGRESS", "COMPLETED", "FAILED"] {
        let filtered = list(
            &transcribe,
            json!({"StatusEquals": status, "MaxResults": 100}),
        )
        .await;
        let body = filtered.ok(status);
        for model in models_of(body) {
            assert_eq!(
                model["ModelStatus"],
                json!(status),
                "case {status:?}: the filter must return only models in that status, \
                 got {model:?}"
            );
        }
        covered.extend(names_of(body));
    }
    covered.sort();
    covered.dedup();
    let mut expected_names = newest_first.clone();
    expected_names.sort();
    assert_eq!(
        covered, expected_names,
        "the three statuses together must cover every model, got {covered:?}"
    );

    // Nothing this emulator records trains, so no model is ever `COMPLETED`: every
    // record ends in the service's own modelled `FAILED` state once its transition
    // falls due.
    let completed = list(
        &transcribe,
        json!({"StatusEquals": "COMPLETED", "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        completed.ok("StatusEquals=COMPLETED")["Models"],
        json!([]),
        "nothing trains a model here, so none is COMPLETED, got {:?}",
        completed.body["Models"]
    );

    // The two filters compose, and so do a filter and a page.
    let composed = list(
        &transcribe,
        json!({"NameContains": "ALPHA", "StatusEquals": "FAILED", "MaxResults": 2}),
    )
    .await;
    let body = composed.ok("NameContains and StatusEquals together, paged");
    let page = names_of(body);
    assert!(
        page.len() == 2 && page.iter().all(|name| name.starts_with("alpha")),
        "a composed filtered page must hold two alpha models, got {page:?}"
    );
    assert!(
        body["NextToken"].is_string(),
        "a truncated filtered page must carry a NextToken, got {:?}",
        body["NextToken"]
    );
}

/// A cursor this service did not hand out is refused; one it did walks on from where
/// the page stopped.
#[tokio::test]
async fn a_next_token_this_service_did_not_hand_out_is_refused() {
    let transcribe = Transcribe::new();

    // Neither an unparseable cursor nor a well-formed one naming a sequence that
    // cannot exist (creation sequences start at 1) is accepted: silently listing from
    // the top would hide a caller's paging bug.
    for token in ["not-a-cursor", "0|support-calls", "|", "-1|support-calls"] {
        list(&transcribe, json!({"NextToken": token}))
            .await
            .refused(token, 400, "BadRequestException");
    }

    // A token this service DID hand out walks on from where the page stopped.
    let names = create_the_table(&transcribe).await;
    let first = list(&transcribe, json!({"MaxResults": 3})).await;
    let token = first.ok("first page")["NextToken"]
        .as_str()
        .expect("a truncated page carries a NextToken")
        .to_string();
    let second = list(&transcribe, json!({"NextToken": token, "MaxResults": 3})).await;
    // Newest first, so the second page holds the three models created just before the
    // first page's three.
    let expected: Vec<String> = names.iter().rev().skip(3).take(3).cloned().collect();
    assert_eq!(
        names_of(second.ok("second page")),
        expected,
        "the second page must resume strictly after the first, got {:?}",
        second.body["Models"]
    );
}

/// The listing reports only this family, only what is stored, and refuses a
/// `MaxResults` outside the modelled range.
#[tokio::test]
async fn the_listing_reports_only_the_language_model_family_and_only_what_is_stored() {
    let transcribe = Transcribe::new();

    // An empty store lists an empty page and no cursor, rather than refusing.
    let empty = list(&transcribe, json!({"MaxResults": 100})).await;
    let body = empty.ok("empty listing");
    assert_eq!(
        body["Models"],
        json!([]),
        "an account that created no custom language model lists nothing, got {:?}",
        body["Models"]
    );
    assert!(
        body["NextToken"].is_null(),
        "a listing that is not truncated carries no NextToken, got {:?}",
        body["NextToken"]
    );

    // Resources of OTHER families under the same names are not custom language
    // models: each family is keyed on its own store.
    let models = create_the_table(&transcribe).await;
    for name in &models {
        transcribe
            .call(
                "CreateVocabulary",
                json!({
                    "VocabularyName": name.clone(),
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark"]
                }),
            )
            .await
            .ok(name);
        transcribe
            .call(
                "StartTranscriptionJob",
                json!({
                    "TranscriptionJobName": name.clone(),
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/not-a-model.wav"}
                }),
            )
            .await
            .ok(name);
    }

    let listed = list(&transcribe, json!({"MaxResults": 100})).await;
    let mut listed_names = names_of(listed.ok("listing beside the other families"));
    listed_names.sort();
    let mut expected = models.clone();
    expected.sort();
    assert_eq!(
        listed_names, expected,
        "the listing must report exactly the custom language models, got {:?}",
        listed.body["Models"]
    );

    // A refused create stores nothing, so it reaches no listing: a name this account
    // already holds a model under is a conflict.
    let duplicate = language_model_request(&models[0], "en-US", "WideBand");
    transcribe
        .call("CreateLanguageModel", duplicate)
        .await
        // Every client error of this service answers HTTP 400 and names its shape in
        // the body (the class is spanned by `crate::client_error_wire_status`).
        .refused("a duplicate model name", 400, "ConflictException");
    let after = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        names_of(after.ok("listing after a refused create")).len(),
        models.len(),
        "a refused create must reach no listing, got {:?}",
        after.body["Models"]
    );

    // A deleted model leaves the listing: the listing is the store, not a log of what
    // was ever created. A delete needs a terminal status, so the clock runs out first.
    std::thread::sleep(PAST_THE_TRANSITION);
    let deleted = models[0].clone();
    transcribe
        .call(
            "DeleteLanguageModel",
            json!({"ModelName": deleted.clone()}),
        )
        .await
        .ok(&deleted);
    let remaining = list(&transcribe, json!({"MaxResults": 100})).await;
    let names = names_of(remaining.ok("listing after a delete"));
    assert!(
        !names.contains(&deleted) && names.len() == models.len() - 1,
        "case {deleted:?}: a deleted model must leave the listing, got {names:?}"
    );

    // A `MaxResults` past each published bound, and a `NameContains` outside its own
    // modelled constraints, are refused by the boundary validator before the handler
    // runs: `MaxResults` is `@range(min: 1, max: 100)` and `NameContains` is
    // `@length 1..200` with `@pattern ^[0-9a-zA-Z._-]+$`
    // (`smithy_query(query="operation", operation="ListLanguageModels")`).
    for max_results in [0, 101] {
        let case = format!("MaxResults={max_results}");
        list(&transcribe, json!({"MaxResults": max_results}))
            .await
            .refused(&case, 400, "ValidationException");
    }
    let one_past_the_ceiling = "z".repeat(201);
    for (case, needle) in [
        ("the empty NameContains, one below the modelled floor", ""),
        (
            "a NameContains one past the modelled length ceiling",
            one_past_the_ceiling.as_str(),
        ),
        ("a NameContains the pattern excludes", "has spaces"),
    ] {
        list(&transcribe, json!({"NameContains": needle}))
            .await
            .refused(case, 400, "ValidationException");
    }
    // Each published bound of that length constraint is served, not refused: one
    // character and 200 characters are both inside it. One row of the table is named
    // with 200 `z`s, so both bounds are substrings of exactly that one model's name.
    let long_name = "z".repeat(200);
    for (case, needle) in [
        ("the modelled NameContains floor", "z".to_string()),
        ("the modelled NameContains ceiling", long_name.clone()),
    ] {
        let served = list(&transcribe, json!({"NameContains": needle})).await;
        assert_eq!(
            names_of(served.ok(case)),
            vec![long_name.clone()],
            "case {case:?}: a search inside the constraint is served, got {:?}",
            served.body["Models"]
        );
    }
}
