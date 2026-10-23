// SPDX-License-Identifier: CC0-1.0

//! `language` on the reporting operations, over HTTP.
//!
//! The rule this suite pins is not about one recorded call: **`language` SELECTS
//! which stored copy of a response's human-readable text is reported — the copy
//! for the language the request names, or the default-language copy when it
//! names none — and it is not a filter: which entries a list reports, and in
//! what order, never depends on it.**
//!
//! So the rows below span the WHOLE class the rule covers rather than one
//! language: every value of `RecommendationLanguage`, every seeded catalog
//! entry, and every operation whose output carries localizable text. The 11
//! languages are read from `RECOMMENDATION_LANGUAGE_VALUES` in
//! `ops::types::shared_types`, which the model's `RecommendationLanguage` enum
//! generates (`smithy_query(query="shape",shape="RecommendationLanguage",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`),
//! so the table is EXHAUSTIVE over the enum instead of sampling it.
//!
//! The localizable members come from the model too
//! (`smithy_query(query="shape",shape="CheckSummary,Recommendation,RecommendationSummary,RecommendationResourceSummary")`):
//! `CheckSummary$name`/`$description`, `Recommendation$name`/`$description` and
//! `RecommendationSummary$name` are prose; `RecommendationResourceSummary`
//! carries no prose member at all, which is why `ListRecommendationResources`
//! reports the same body whatever `language` asks for — pinned in the last test.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; that gate is pinned in `tests/all/support_level_gate.rs`.

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::ops::types::shared_types::RECOMMENDATION_LANGUAGE_VALUES;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    service_state, LocalizedText, SupportPlan, TextCatalog, DEFAULT_LANGUAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{call, emulator_on_plan, label, recommendation_arn};

/// The seeded check ids, in the order the store keys them. Their source is the
/// seed in `src/state.rs`.
const CHECK_IDS: &[&str] = &[
    "amazon-ebs-snapshots",
    "iam-access-key-rotation",
    "low-utilization-ec2-instances",
    "rds-idle-db-instances",
];

/// An emulator whose account is entitled to the API.
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// The `name` and `description` reported for each check id by one `ListChecks`
/// read, keyed by check id.
async fn check_text(router: &MultiServiceRouter, query: &str) -> Vec<(String, String, String)> {
    let (status, body) = call(router, "GET", &format!("/v1/checks{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {query:?}: expected 200, got {status} with {body}"
    );
    body["checkSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {query:?}: response has no checkSummaries: {body}"))
        .iter()
        .map(|summary| {
            let text = |member: &str| {
                summary[member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {query:?}: a check summary has no {member}: {summary}")
                    })
                    .to_string()
            };
            (text("id"), text("name"), text("description"))
        })
        .collect()
}

/// The `name` and `description` `GetRecommendation` reports for the
/// recommendation of `check_id` in `language`.
async fn recommendation_text(
    router: &MultiServiceRouter,
    check_id: &str,
    query: &str,
) -> (String, String) {
    let arn = recommendation_arn(check_id);
    let (status, body) = call(
        router,
        "GET",
        &format!("/v1/recommendations/{}{query}", label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {check_id:?} {query:?}: expected 200, got {status} with {body}"
    );
    let recommendation = &body["recommendation"];
    let text = |member: &str| {
        recommendation[member]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {check_id:?} {query:?}: recommendation has no {member}: {body}")
            })
            .to_string()
    };
    (text("name"), text("description"))
}

/// The `name` `ListRecommendations` reports for each recommendation, keyed by
/// recommendation ARN.
async fn recommendation_names(
    router: &MultiServiceRouter,
    query: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(router, "GET", &format!("/v1/recommendations{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {query:?}: expected 200, got {status} with {body}"
    );
    body["recommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {query:?}: response has no recommendationSummaries: {body}"))
        .iter()
        .map(|summary| {
            let text = |member: &str| {
                summary[member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {query:?}: a recommendation summary has no {member}: {summary}")
                    })
                    .to_string()
            };
            (text("arn"), text("name"))
        })
        .collect()
}

#[tokio::test]
async fn every_modelled_language_selects_its_own_copy_of_the_check_text() {
    let router = emulator();

    let default = check_text(&router, &format!("?language={DEFAULT_LANGUAGE}")).await;
    assert_eq!(
        default.len(),
        CHECK_IDS.len(),
        "case {DEFAULT_LANGUAGE:?}: expected {} checks, got {default:?}",
        CHECK_IDS.len()
    );

    // One row per value of `RecommendationLanguage` — the whole enum.
    for language in RECOMMENDATION_LANGUAGE_VALUES {
        let localized = check_text(&router, &format!("?language={language}")).await;
        let ids: Vec<&str> = localized.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(
            ids, *CHECK_IDS,
            "case {language:?}: expected checks {CHECK_IDS:?} in key order, got {ids:?}"
        );

        for ((id, name, description), (_, default_name, default_description)) in
            localized.iter().zip(default.iter())
        {
            assert!(
                !name.is_empty() && !description.is_empty(),
                "case {language:?}/{id:?}: expected non-empty text, got {name:?} / {description:?}"
            );
            if *language == DEFAULT_LANGUAGE {
                assert_eq!(
                    (name, description),
                    (default_name, default_description),
                    "case {language:?}/{id:?}: the default language must report the default copy"
                );
            } else {
                assert_ne!(
                    (name, description),
                    (default_name, default_description),
                    "case {language:?}/{id:?}: expected text of its own, got the \
                     {DEFAULT_LANGUAGE} copy {name:?} / {description:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_request_that_names_no_language_is_answered_in_the_default_language() {
    let router = emulator();

    let unspecified = check_text(&router, "").await;
    let default = check_text(&router, &format!("?language={DEFAULT_LANGUAGE}")).await;
    assert_eq!(
        unspecified, default,
        "case \"no language\": expected the {DEFAULT_LANGUAGE} copy {default:?}, got {unspecified:?}"
    );

    for check_id in CHECK_IDS {
        let unspecified = recommendation_text(&router, check_id, "").await;
        let default = recommendation_text(&router, check_id, &format!("?language={DEFAULT_LANGUAGE}"))
            .await;
        assert_eq!(
            unspecified, default,
            "case {check_id:?}: expected the {DEFAULT_LANGUAGE} copy {default:?}, got {unspecified:?}"
        );
    }
}

#[tokio::test]
async fn the_requested_language_selects_the_same_copy_on_every_operation_that_reports_text() {
    let router = emulator();

    for language in RECOMMENDATION_LANGUAGE_VALUES {
        let query = format!("?language={language}");
        let checks = check_text(&router, &query).await;
        let summaries = recommendation_names(&router, &query).await;

        for (check_id, name, description) in &checks {
            // `GetRecommendation` reports both members; they are the check's.
            let (full_name, full_description) =
                recommendation_text(&router, check_id, &query).await;
            assert_eq!(
                (&full_name, &full_description),
                (name, description),
                "case {language:?}/{check_id:?}: GetRecommendation must report the same copy as \
                 ListChecks, got {full_name:?} / {full_description:?}"
            );

            // `ListRecommendations` reports only `name`.
            let arn = recommendation_arn(check_id);
            let summary_name = summaries
                .iter()
                .find(|(summary_arn, _)| *summary_arn == arn)
                .map(|(_, summary_name)| summary_name.clone())
                .unwrap_or_else(|| {
                    panic!("case {language:?}/{check_id:?}: {arn} is missing from {summaries:?}")
                });
            assert_eq!(
                &summary_name, name,
                "case {language:?}/{check_id:?}: ListRecommendations must report the same copy as \
                 ListChecks, got {summary_name:?}"
            );
        }
    }
}

#[tokio::test]
async fn language_selects_text_and_never_filters_or_reorders_a_list() {
    let router = emulator();

    // Rows differ along the filters `language` is combined with: none, one
    // filter that keeps several entries, one that keeps a single entry, and one
    // that keeps none.
    let filters: &[&str] = &[
        "",
        "&pillar=cost_optimizing",
        "&source=rds",
        "&awsService=lambda",
    ];

    for filter in filters {
        let baseline = check_text(&router, &format!("?maxResults=200{filter}")).await;
        let baseline_ids: Vec<&str> = baseline.iter().map(|(id, _, _)| id.as_str()).collect();
        for language in RECOMMENDATION_LANGUAGE_VALUES {
            let localized =
                check_text(&router, &format!("?maxResults=200{filter}&language={language}")).await;
            let ids: Vec<&str> = localized.iter().map(|(id, _, _)| id.as_str()).collect();
            assert_eq!(
                ids, baseline_ids,
                "case {language:?} with {filter:?}: expected the same checks in the same order \
                 as {baseline_ids:?}, got {ids:?}"
            );
        }
    }

    // And the pagination sequence is the same sequence, so a cursor issued for
    // one language resumes under another: `language` is text selection, not part
    // of the read's identity.
    let (status, first) = call(
        &router,
        "GET",
        &format!("/v1/checks?maxResults=1&language={DEFAULT_LANGUAGE}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a nextToken on a one-entry page: {first}"))
        .to_string();
    for language in RECOMMENDATION_LANGUAGE_VALUES {
        let page = check_text(
            &router,
            &format!("?maxResults=1&language={language}&nextToken={}", label(&token)),
        )
        .await;
        let ids: Vec<&str> = page.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec![CHECK_IDS[1]],
            "case {language:?}: a cursor must resume the same sequence, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn reading_in_one_language_leaves_the_catalog_as_it_was() {
    let router = emulator();

    let before = check_text(&router, &format!("?language={DEFAULT_LANGUAGE}")).await;
    // Read every other language in between: a read reports a copy, it never
    // moves the catalog to that language.
    for language in RECOMMENDATION_LANGUAGE_VALUES {
        let _ = check_text(&router, &format!("?language={language}")).await;
        for check_id in CHECK_IDS {
            let _ = recommendation_text(&router, check_id, &format!("?language={language}")).await;
        }
    }
    let after = check_text(&router, &format!("?language={DEFAULT_LANGUAGE}")).await;
    assert_eq!(
        after, before,
        "expected the {DEFAULT_LANGUAGE} copy {before:?} to be unchanged, got {after:?}"
    );

    // Two reads in the same language agree, too.
    for language in RECOMMENDATION_LANGUAGE_VALUES {
        let first = check_text(&router, &format!("?language={language}")).await;
        let second = check_text(&router, &format!("?language={language}")).await;
        assert_eq!(
            first, second,
            "case {language:?}: repeated reads must agree, got {first:?} then {second:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_holds_a_copy_in_every_modelled_language() {
    let (_router, engine) = emulator_on_plan(SupportPlan::Business);
    let state = service_state(&*engine).expect("this service's state");
    let state = state.lock();

    for (check_id, check) in &state.checks {
        let held: Vec<&str> = check.text.languages().collect();
        for language in RECOMMENDATION_LANGUAGE_VALUES {
            assert!(
                held.contains(language),
                "case {check_id:?}: expected a copy in every modelled language, \
                 {language:?} is missing from {held:?}"
            );
        }
    }

    // The recommendation a check produced reports the check's own catalog.
    for (arn, entry) in &state.recommendations {
        let check_arn = entry
            .check_arn
            .as_deref()
            .unwrap_or_else(|| panic!("case {arn:?}: expected a checkArn"));
        let check = state
            .checks
            .values()
            .find(|check| check.arn == check_arn)
            .unwrap_or_else(|| panic!("case {arn:?}: {check_arn} names no stored check"));
        for language in RECOMMENDATION_LANGUAGE_VALUES {
            let expected = check.text.text(Some(language));
            let actual = entry.text.text(Some(language));
            assert_eq!(
                (&actual.name, &actual.description),
                (&expected.name, &expected.description),
                "case {arn:?}/{language:?}: expected the check's copy, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_language_the_catalog_holds_no_copy_in_falls_back_to_the_default() {
    // The fallback is a property of [`TextCatalog`] itself: the seeded catalog
    // holds all 11 modelled languages, so this exercises an entry that does not.
    let default = LocalizedText {
        name: "a name".to_string(),
        description: "a description".to_string(),
    };
    let catalog = TextCatalog::new(
        default.clone(),
        [
            ("ja", "ja name", "ja description"),
            // An `en` entry among the translations is the default copy, never a
            // second one beside it.
            (DEFAULT_LANGUAGE, "ignored", "ignored"),
        ],
    );

    for language in RECOMMENDATION_LANGUAGE_VALUES {
        let text = catalog.text(Some(language));
        let expected = match *language {
            "ja" => ("ja name", "ja description"),
            _ => (default.name.as_str(), default.description.as_str()),
        };
        assert_eq!(
            (text.name.as_str(), text.description.as_str()),
            expected,
            "case {language:?}: expected {expected:?}, got {text:?}"
        );
    }

    // No language at all, and a language outside the enum (which the request
    // walk refuses with a 400 before a handler runs), both answer the default.
    for language in [None, Some("en_GB")] {
        let text = catalog.text(language);
        assert_eq!(
            (text.name.as_str(), text.description.as_str()),
            (default.name.as_str(), default.description.as_str()),
            "case {language:?}: expected the default copy, got {text:?}"
        );
    }

    let held: Vec<&str> = catalog.languages().collect();
    assert_eq!(
        held,
        vec![DEFAULT_LANGUAGE, "ja"],
        "expected the default language and its one translation, got {held:?}"
    );
}

#[tokio::test]
async fn list_recommendation_resources_reports_the_same_body_in_every_language() {
    let router = emulator();

    // `RecommendationResourceSummary` carries no prose member, so `language`
    // selects nothing in this output: every language must answer identically.
    for check_id in CHECK_IDS {
        let arn = recommendation_arn(check_id);
        let mut baseline: Option<Value> = None;
        for language in RECOMMENDATION_LANGUAGE_VALUES {
            let (status, body) = call(
                &router,
                "GET",
                &format!(
                    "/v1/recommendations/{}/resources?language={language}",
                    label(&arn)
                ),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {check_id:?}/{language:?}: expected 200, got {status} with {body}"
            );
            match &baseline {
                None => baseline = Some(body),
                Some(expected) => assert_eq!(
                    &body, expected,
                    "case {check_id:?}/{language:?}: expected {expected}, got {body}"
                ),
            }
        }
    }
}
