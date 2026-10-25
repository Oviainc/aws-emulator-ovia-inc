// SPDX-License-Identifier: CC0-1.0

//! `GetOrganizationRecommendation` as a REPORTING read, over HTTP through the
//! same `MultiServiceRouter` the emulator serves with.
//!
//! The two rules pinned here, each over a TABLE of inputs rather than one call,
//! are the ones the single-read handler owns and that no other suite states:
//!
//! * **this operation models no `language` member, so the prose it reports is
//!   the stored default-language copy** — the same copy the account-level read
//!   of the member recommendation reports when it names no language, and not any
//!   other language's copy. The modelled input is
//!   `organizationRecommendationIdentifier` alone
//!   (`smithy_query(query="operation",operation="GetOrganizationRecommendation,GetRecommendation",
//!   model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`:
//!   `GetRecommendation` binds `language`, `GetOrganizationRecommendation` does
//!   not), and the 11 languages come from `RECOMMENDATION_LANGUAGE_VALUES`,
//!   which the model's `RecommendationLanguage` enum generates — so the table is
//!   exhaustive over that enum instead of sampling it;
//! * **a read reports the store and changes nothing a later caller sees** —
//!   repeating the read any number of times, and interleaving it with the reads
//!   of the other stored organization recommendations, reports a body identical
//!   to the first one and leaves `ListOrganizationRecommendations` and the member
//!   accounts' `GetRecommendation` exactly as they were.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::ops::types::shared_types::RECOMMENDATION_LANGUAGE_VALUES;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{SupportPlan, DEFAULT_LANGUAGE};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::read;
use crate::recommendation_loop::{
    call, emulator_on_plan, label, recommendation_arn, CHECK_IDS,
};

/// An emulator whose account is entitled to the API.
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// A string member of a response object, or a panic naming the case.
fn text(case: &str, object: &Value, member: &str) -> String {
    object[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a {member}, got {object}"))
        .to_string()
}

/// The `name` and `description` the account-level `GetRecommendation` reports
/// for the member recommendation of `check_id` under `query`.
async fn member_text(
    router: &MultiServiceRouter,
    case: &str,
    check_id: &str,
    query: &str,
) -> (String, String) {
    let (status, body) = call(
        router,
        "GET",
        &format!(
            "/v1/recommendations/{}{query}",
            label(&recommendation_arn(check_id))
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from GetRecommendation, got {status} with {body}"
    );
    let recommendation = &body["recommendation"];
    (
        text(case, recommendation, "name"),
        text(case, recommendation, "description"),
    )
}

/// The whole `ListOrganizationRecommendations` page, as one comparable value.
async fn organization_list(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        "/v1/organization-recommendations?maxResults=200",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from ListOrganizationRecommendations, got {status} with {body}"
    );
    body
}

#[tokio::test]
async fn the_organization_read_reports_the_default_language_copy() {
    let router = emulator();

    for check_id in CHECK_IDS {
        let case = *check_id;
        let organization = read(&router, case, check_id).await;
        let reported = (
            text(case, &organization, "name"),
            text(case, &organization, "description"),
        );

        // The copy the member recommendation reports when the request names no
        // language at all: the store's default-language copy.
        let default = member_text(&router, case, check_id, "").await;
        assert_eq!(
            reported, default,
            "case {case}: expected the default-language copy {default:?}, got {reported:?}"
        );

        // One row per value of `RecommendationLanguage` — the whole enum,
        // including entries and languages this operation's own issue did not
        // cite. The organization read must agree with the DEFAULT language's
        // copy and with no other, which is what makes the first assertion a
        // statement about the language rather than about a catalog that happens
        // to hold one copy.
        for language in RECOMMENDATION_LANGUAGE_VALUES {
            let localized = member_text(
                &router,
                &format!("{case}/{language}"),
                check_id,
                &format!("?language={language}"),
            )
            .await;
            if *language == DEFAULT_LANGUAGE {
                assert_eq!(
                    reported, localized,
                    "case {case}/{language}: expected the organization read to report the \
                     {DEFAULT_LANGUAGE} copy {localized:?}, got {reported:?}"
                );
            } else if localized != default {
                // Only where the catalog holds a copy of its OWN in this
                // language: an entry holding none reports the default copy
                // under any language, which is the copy the organization read
                // reports too, so there is nothing to tell apart.
                assert_ne!(
                    reported, localized,
                    "case {case}/{language}: expected the organization read NOT to report the \
                     {language} copy, got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn repeating_the_organization_read_reports_the_same_body_and_moves_nothing() {
    let router = emulator();

    // What every other read of the store reports before the reads below.
    let list_before = organization_list(&router, "before").await;
    let mut members_before = Vec::new();
    for check_id in CHECK_IDS {
        members_before.push(member_text(&router, "before", check_id, "").await);
    }

    // Rows differ along HOW MANY times the same identifier is read, so a
    // handler that moved a counter, a lifecycle stage or a timestamp on a read
    // would diverge at some row rather than be hidden by a single call.
    for reads in [1usize, 2, 5] {
        for check_id in CHECK_IDS {
            let case = format!("{check_id} read {reads} times");
            let first = read(&router, &case, check_id).await;
            for round in 1..reads {
                // Interleave the other stored organization recommendations, so
                // the rule covers a read that follows a read of a sibling entry
                // and not only a repeat of itself.
                for other in CHECK_IDS {
                    let _ = read(&router, &case, other).await;
                }
                let again = read(&router, &case, check_id).await;
                assert_eq!(
                    again, first,
                    "case {case} (round {round}): expected the same body {first}, got {again}"
                );
            }
        }
    }

    let list_after = organization_list(&router, "after").await;
    assert_eq!(
        list_after, list_before,
        "expected ListOrganizationRecommendations to be unchanged by the reads, got {list_after}"
    );
    for (check_id, before) in CHECK_IDS.iter().zip(&members_before) {
        let after = member_text(&router, "after", check_id, "").await;
        assert_eq!(
            &after, before,
            "case {check_id}: expected the member recommendation to be unchanged by the \
             organization reads, got {after:?}"
        );
    }
}
