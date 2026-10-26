// SPDX-License-Identifier: CC0-1.0

//! THE RULE THIS SUITE IS ABOUT: `DeleteVocabularyFilter` removes the custom
//! vocabulary filter the Region serving the call holds under the EXACT name in the
//! request. A name that Region holds no FILTER under — however it comes to hold none —
//! is refused with this service's absent-resource client error ("Your request didn't
//! pass one or more validation tests. This can occur when the entity you're trying to
//! delete doesn't exist …", `BadRequestException`), and the call removes nothing: the
//! delete is not idempotent and never answers an empty success for a filter that is
//! not there.
//!
//! The table spans the ways a name can be one this Region holds no filter under,
//! which is what the rule generalises over, rather than the spelling of one absent
//! name (`crate::delete_requires_the_entity` spans the name forms the member's
//! `@length(1..200)` / `@pattern(^[0-9a-zA-Z._-]+$)` constraint allows, across every
//! family's delete): never created at all, differing from a held filter only in letter
//! case ("Custom vocabulary filter names are case sensitive",
//! `smithy_query(query="operation", operation="DeleteVocabularyFilter")`), held by a
//! resource of ANOTHER family under that name, and held under that name by another
//! Region only.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator.

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// The status this service puts on every client error
/// (`crate::scaffolded::wire_status`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error shape this service's absent-resource refusals carry.
const ABSENT_RESOURCE_SHAPE: &str = "BadRequestException";

/// The word this service's own messages name this family by
/// (`crate::state::KIND_VOCABULARY_FILTER`).
const FILTER_KIND: &str = "vocabulary filter";

/// How this service words "there is no such custom vocabulary filter"
/// (`crate::state::not_found_message` for this family's kind): the service's own
/// sentence, naming the KIND of resource and no value from the request.
const ABSENT_FILTER_MESSAGE: &str = "The requested vocabulary filter couldn't be found. \
     Check the vocabulary filter name and try your request again.";

/// The Region the calls under test are signed for.
const SERVING_REGION: &str = "us-east-1";

/// A second Region, so a row can hold a filter somewhere OTHER than where the delete
/// is served.
const OTHER_REGION: &str = "us-west-2";

/// A filter the serving Region holds, spelled in mixed case so the case-sensitivity
/// rows have a held name to differ from.
const MIXED_CASE_FILTER: &str = "MixedCaseFilter";

/// A filter the serving Region holds, plainly spelled.
const HELD_FILTER: &str = "held.filter";

/// A name the serving Region holds a custom VOCABULARY under and no filter.
const VOCABULARY_ONLY: &str = "vocabulary-only-name";

/// A name only [`OTHER_REGION`] holds a filter under.
const OTHER_REGION_FILTER: &str = "another-regions-filter";

/// `CreateVocabularyFilter`'s request for a filter named `name`: the required
/// `LanguageCode` plus inline `Words`, so no row depends on an object store.
fn create_request(name: &str) -> Value {
    json!({
        "VocabularyFilterName": name,
        "LanguageCode": "en-US",
        "Words": ["crud", "bogus"]
    })
}

/// Assert `reply` is this service's absent-resource refusal for a custom vocabulary
/// filter: the client-error status, the modelled shape in the body's `__type`, the
/// service's own sentence verbatim, and no member of a success.
///
/// The status, shape and kind-naming half is the custom-vocabulary suite's own
/// assertion of the same rule (`crate::vocabulary_delete_and_tag_sets`), reused rather
/// than spelled a second time so the two cannot drift; what is local is the EXACT
/// sentence that helper leaves open, the no-echo check and the no-success-member check.
fn absent_filter_refusal(reply: &Reply, case: &str, asked_about: &str) {
    crate::vocabulary_delete_and_tag_sets::absent_resource_refusal(reply, case, FILTER_KIND);
    reply.refused(case, CLIENT_ERROR_STATUS, ABSENT_RESOURCE_SHAPE);
    let message = reply.error_message(case);
    assert_eq!(
        message, ABSENT_FILTER_MESSAGE,
        "case {case:?}: expected this service's absent-filter sentence, got {message:?}"
    );
    assert!(
        !message.contains(asked_about),
        "case {case:?}: the refusal must not echo the requested {asked_about:?}, got \
         {message:?}"
    );
    assert!(
        reply.body.get("VocabularyFilterName").is_none(),
        "case {case:?}: a refusal carries no member of a success, got {:?}",
        reply.body
    );
}

/// Every filter name `region` reports through `ListVocabularyFilters`, in name order
/// so a row asserts WHICH filters are held without depending on the listing's own
/// ordering rule (`crate::list_paging_rule`).
async fn held_filters(transcribe: &Transcribe, case: &str, region: &str) -> Vec<String> {
    let listed = transcribe
        .call_in_region("ListVocabularyFilters", json!({"MaxResults": 100}), Some(region))
        .await;
    let mut names: Vec<String> = listed.ok(case)["VocabularyFilters"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a VocabularyFilters list, got {:?}",
                listed.body
            )
        })
        .iter()
        .map(|entry| {
            entry["VocabularyFilterName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a named filter in the listing, got {entry:?}"
                    )
                })
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// The filters and the same-named vocabulary every row of the refusal table is probed
/// against: two filters in the serving Region, one filter in another Region, and a
/// custom vocabulary whose name no filter is held under.
async fn with_the_fixture() -> Transcribe {
    let transcribe = Transcribe::new();

    for name in [HELD_FILTER, MIXED_CASE_FILTER] {
        transcribe
            .call_in_region(
                "CreateVocabularyFilter",
                create_request(name),
                Some(SERVING_REGION),
            )
            .await
            .ok_status(&format!("create the held filter {name:?}"), 200);
    }
    transcribe
        .call_in_region(
            "CreateVocabularyFilter",
            create_request(OTHER_REGION_FILTER),
            Some(OTHER_REGION),
        )
        .await
        .ok_status("create the other Region's filter", 200);
    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": VOCABULARY_ONLY,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
            Some(SERVING_REGION),
        )
        .await
        .ok_status("create the vocabulary whose name no filter is held under", 200);

    transcribe
}

/// RULE: a `DeleteVocabularyFilter` naming something the serving Region holds no
/// custom vocabulary filter under is refused — for every way the name can be one it
/// holds no filter under — and removes nothing, so both held filters, the other
/// Region's filter and the same-named vocabulary are all still there afterwards.
#[tokio::test]
async fn deleting_a_name_this_region_holds_no_filter_under_is_refused() {
    let transcribe = with_the_fixture().await;

    // The rows differ along what "this Region holds no filter under that name" can
    // mean: nothing was ever created under it; it differs from a held filter only in
    // letter case, in either direction and in a single letter; it names a resource of
    // another family; it is held by another Region only.
    let rows: Vec<(&str, String)> = vec![
        (
            "a name nothing was ever created under",
            "never-created-at-all".to_string(),
        ),
        (
            "a held filter's name, lowercased",
            MIXED_CASE_FILTER.to_lowercase(),
        ),
        (
            "a held filter's name, uppercased",
            MIXED_CASE_FILTER.to_uppercase(),
        ),
        (
            "a held filter's name differing in one letter's case",
            "MixedCaseFilteR".to_string(),
        ),
        (
            "a name held by a custom vocabulary and by no filter",
            VOCABULARY_ONLY.to_string(),
        ),
        (
            "a name only another Region holds a filter under",
            OTHER_REGION_FILTER.to_string(),
        ),
    ];

    for (case, name) in &rows {
        let reply = transcribe
            .call_in_region(
                "DeleteVocabularyFilter",
                json!({"VocabularyFilterName": name}),
                Some(SERVING_REGION),
            )
            .await;
        absent_filter_refusal(&reply, case, name);

        // Nothing was removed and nothing was minted: the serving Region still holds
        // exactly the two filters it was given, each readable, and the refused name is
        // still not one of them.
        let mut expected = vec![HELD_FILTER.to_string(), MIXED_CASE_FILTER.to_string()];
        expected.sort();
        assert_eq!(
            held_filters(&transcribe, case, SERVING_REGION).await,
            expected,
            "case {case:?}: a refused delete must leave the Region's filters as they were"
        );
        for held in [HELD_FILTER, MIXED_CASE_FILTER] {
            transcribe
                .call_in_region(
                    "GetVocabularyFilter",
                    json!({"VocabularyFilterName": held}),
                    Some(SERVING_REGION),
                )
                .await
                .ok(&format!("{case} / {held} is still readable"));
        }
        transcribe
            .call_in_region(
                "GetVocabularyFilter",
                json!({"VocabularyFilterName": OTHER_REGION_FILTER}),
                Some(OTHER_REGION),
            )
            .await
            .ok(&format!("{case} / the other Region's filter is untouched"));
        transcribe
            .call_in_region(
                "GetVocabulary",
                json!({"VocabularyName": VOCABULARY_ONLY}),
                Some(SERVING_REGION),
            )
            .await
            .ok(&format!("{case} / the same-named vocabulary is untouched"));
    }
}

/// RULE, the other half: the EXACT name in the serving Region removes exactly that
/// filter, once. Every neighbour — a filter whose name differs only in case, an
/// unrelated filter, the same name in another Region — survives it, and the second
/// delete of the same name is the absent-resource refusal.
#[tokio::test]
async fn the_exact_name_removes_exactly_that_filter_once() {
    let transcribe = Transcribe::new();

    // Two names differing only in case are two filters: the create of the second is a
    // success, not a name conflict.
    let case = "two filters whose names differ only in case";
    for name in ["alpha.one", "Alpha.One", "beta-two"] {
        transcribe
            .call_in_region(
                "CreateVocabularyFilter",
                create_request(name),
                Some(SERVING_REGION),
            )
            .await
            .ok_status(&format!("{case} / create {name:?}"), 200);
    }
    transcribe
        .call_in_region(
            "CreateVocabularyFilter",
            create_request("alpha.one"),
            Some(OTHER_REGION),
        )
        .await
        .ok_status(&format!("{case} / create the other Region's filter"), 200);

    let deleted = transcribe
        .call_in_region(
            "DeleteVocabularyFilter",
            json!({"VocabularyFilterName": "alpha.one"}),
            Some(SERVING_REGION),
        )
        .await;
    deleted.ok_status(case, 200);
    crate::vocabulary_delete_and_tag_sets::empty_body(&deleted, case);

    // Exactly that one filter is gone from the serving Region.
    transcribe
        .call_in_region(
            "GetVocabularyFilter",
            json!({"VocabularyFilterName": "alpha.one"}),
            Some(SERVING_REGION),
        )
        .await
        .refused(case, CLIENT_ERROR_STATUS, ABSENT_RESOURCE_SHAPE);
    assert_eq!(
        held_filters(&transcribe, case, SERVING_REGION).await,
        vec!["Alpha.One".to_string(), "beta-two".to_string()],
        "case {case:?}: only the exactly named filter may be removed"
    );
    for (region, name) in [
        (SERVING_REGION, "Alpha.One"),
        (SERVING_REGION, "beta-two"),
        (OTHER_REGION, "alpha.one"),
    ] {
        transcribe
            .call_in_region(
                "GetVocabularyFilter",
                json!({"VocabularyFilterName": name}),
                Some(region),
            )
            .await
            .ok(&format!("{case} / {name:?} in {region} survives"));
    }

    // And the delete does not repeat: the name it removed is now one this Region holds
    // no filter under.
    for attempt in 2..=3 {
        let again = format!("{case} / delete #{attempt}");
        let reply = transcribe
            .call_in_region(
                "DeleteVocabularyFilter",
                json!({"VocabularyFilterName": "alpha.one"}),
                Some(SERVING_REGION),
            )
            .await;
        absent_filter_refusal(&reply, &again, "alpha.one");
    }
}
