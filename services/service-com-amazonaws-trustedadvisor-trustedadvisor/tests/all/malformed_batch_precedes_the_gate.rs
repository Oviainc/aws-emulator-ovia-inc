// SPDX-License-Identifier: CC0-1.0

//! `BatchUpdateRecommendationResourceExclusion` over a MALFORMED request, driven
//! over HTTP through the same `MultiServiceRouter` the emulator serves with, on
//! every support plan the service classifies.
//!
//! The RULE: **an input-validation refusal of this service is answered 400
//! `ValidationException` independently of the calling account's entitlement — it
//! is evaluated ahead of the support-level gate — whether the condition is one
//! the model's constraint traits express or one a handler owns because they
//! cannot.** `src/service/validation.rs` classifies both as the same refusal and
//! words both from the same sentence, so they cannot differ in status either: a
//! request's well-formedness is a property of the request, not of the plan the
//! caller is on.
//!
//! The condition a handler owns here is a repeated recommendation-resource ARN:
//! the modelled `@uniqueItems` on `RecommendationResourceExclusionList` compares
//! whole `RecommendationResourceExclusion` structures, so it cannot state that one
//! `arn` appears twice, while the operation's own doc states the request carries
//! "up to 25 unique recommendation resource ARNs"
//! (`smithy_query(query="operation",operation="BatchUpdateRecommendationResourceExclusion",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`).
//! The modelled half of the same class is the list's `@length(1, 25)` minimum.
//!
//! The rows span what the refusal must not depend on: the support plan (every one
//! the service models, entitled and not), the repeat's position in the list, how
//! many entries the list holds up to the modelled maximum, how many times one ARN
//! occurs, how many distinct ARNs repeat, whether the repeated entries agree on
//! `isExcluded`, and whether the repeated ARN names a stored resource, no resource
//! at all, or a resource of a prioritized recommendation.
//!
//! What this suite does NOT move: the refusal of an entry naming ANOTHER
//! account's resource, which is resource authorization rather than request
//! well-formedness and is answered ahead of both stages
//! (`tests/all/batch_exclusion_foreign_account.rs`). The gate itself, and the
//! modelled refusals that precede it, are pinned in
//! `tests/all/support_level_gate.rs`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{dispatch, emulator_on_plan, error_code, resource_arn};

/// The path this operation binds (`PUT /v1/batch-update-recommendation-resource-exclusion`).
const BATCH_PATH: &str = "/v1/batch-update-recommendation-resource-exclusion";

/// The modelled maximum of `RecommendationResourceExclusionList` (`@length(1, 25)`),
/// which is also the count the operation's doc calls "up to 25 unique
/// recommendation resource ARNs per request".
const MAX_ENTRIES: usize = 25;

/// A seeded check whose recommendation is `standard` — the type whose resources an
/// applied entry writes. Source: the `SEED` table in `src/state.rs`.
const STANDARD_CHECK: &str = "amazon-ebs-snapshots";

/// A seeded check whose recommendation is `priority`, the type this operation
/// refuses per entry. Same source.
const PRIORITY_CHECK: &str = "low-utilization-ec2-instances";

/// The request body for a list of `(arn, isExcluded)` entries.
fn body(entries: &[(String, bool)]) -> Value {
    json!({
        "recommendationResourceExclusions": entries
            .iter()
            .map(|(arn, is_excluded)| json!({"arn": arn, "isExcluded": is_excluded}))
            .collect::<Vec<Value>>(),
    })
}

/// The `message` a repeated-ARN refusal carries, in the sentence this service
/// builds every input-validation refusal from (`src/service/validation.rs`): the
/// clause count, then one `<subject> at '<member path>' failed to satisfy
/// constraint: <requirement>` clause per offending value, joined with `; `.
fn expected_message(repeated: &[&str]) -> String {
    let clauses: Vec<String> = repeated
        .iter()
        .map(|arn| {
            format!(
                "Value '{arn}' at 'recommendationResourceExclusions' failed to satisfy \
                 constraint: Member must have unique arn values"
            )
        })
        .collect();
    match clauses.as_slice() {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// One row: what it is about, the entries to send, and the ARNs the refusal must
/// name, in the order in which each first repeats.
struct Row {
    name: String,
    entries: Vec<(String, bool)>,
    repeated: Vec<String>,
}

/// Every malformed-batch row whose condition the handler owns.
fn repeated_arn_rows() -> Vec<Row> {
    let first = resource_arn(STANDARD_CHECK, "res-ebs-0001");
    let second = resource_arn(STANDARD_CHECK, "res-ebs-0002");
    let prioritized = resource_arn(PRIORITY_CHECK, "res-ec2-0001");
    let unknown = resource_arn(STANDARD_CHECK, "res-ebs-9999");

    // A batch at the modelled maximum of 25 entries whose FIRST ARN is repeated
    // as the last entry: 24 distinct ARNs, 25 entries.
    let mut at_max: Vec<(String, bool)> = (0..MAX_ENTRIES - 1)
        .map(|index| {
            (
                resource_arn(STANDARD_CHECK, &format!("res-ebs-{index:04}")),
                index % 2 == 0,
            )
        })
        .collect();
    let first_of_max = at_max[0].0.clone();
    at_max.push((first_of_max.clone(), false));
    assert_eq!(
        at_max.len(),
        MAX_ENTRIES,
        "case the maximum-length batch: expected {MAX_ENTRIES} entries, got {}",
        at_max.len()
    );

    vec![
        Row {
            name: "the same ARN twice with conflicting isExcluded".to_string(),
            entries: vec![(first.clone(), true), (first.clone(), false)],
            repeated: vec![first.clone()],
        },
        Row {
            name: "the same ARN twice with the same isExcluded".to_string(),
            entries: vec![(first.clone(), false), (first.clone(), false)],
            repeated: vec![first.clone()],
        },
        Row {
            name: "a repeat separated by another entry".to_string(),
            entries: vec![
                (first.clone(), true),
                (second.clone(), false),
                (first.clone(), false),
            ],
            repeated: vec![first.clone()],
        },
        Row {
            name: "one ARN three times, reported once".to_string(),
            entries: vec![
                (first.clone(), true),
                (first.clone(), false),
                (first.clone(), true),
            ],
            repeated: vec![first.clone()],
        },
        Row {
            name: "two ARNs each repeated, in first-repeat order".to_string(),
            entries: vec![
                (second.clone(), true),
                (first.clone(), true),
                (first.clone(), false),
                (second.clone(), false),
            ],
            repeated: vec![first, second],
        },
        Row {
            name: "a repeated ARN naming no stored resource".to_string(),
            entries: vec![(unknown.clone(), true), (unknown.clone(), false)],
            repeated: vec![unknown],
        },
        Row {
            name: "a repeated ARN of a prioritized recommendation".to_string(),
            entries: vec![(prioritized.clone(), false), (prioritized.clone(), true)],
            repeated: vec![prioritized],
        },
        Row {
            name: format!("a repeat in a batch at the modelled maximum of {MAX_ENTRIES} entries"),
            entries: at_max,
            repeated: vec![first_of_max],
        },
    ]
}

/// What every stored recommendation resource holds that this operation writes:
/// its `exclusionStatus` and the `lastUpdatedAt` a write would stamp, keyed by the
/// resource's own ARN. Read in process, because an unentitled caller's reads are
/// refused by the very gate under test here.
fn stored_exclusions(engine: &engine::InMemoryEngine) -> BTreeMap<String, (String, i64)> {
    let state = service_state(engine).expect("the in-memory engine carries this service's state");
    let state = state.lock();
    state
        .recommendation_resources
        .iter()
        .map(|(arn, resource)| {
            (
                arn.clone(),
                (
                    resource.exclusion_status.clone(),
                    resource.last_updated_at_millis,
                ),
            )
        })
        .collect()
}

/// Send one malformed batch and assert the refusal: the status the model binds
/// `ValidationException` to, the error shape in both places restJson1 carries it,
/// and the sentence naming every repeated ARN once. Returns the body, so the
/// caller can compare one plan's answer with another's.
async fn assert_validation_refusal(router: &MultiServiceRouter, row: &Row, case: &str) -> Value {
    let (status, errortype, response) =
        dispatch(router, "PUT", BATCH_PATH, Some(body(&row.entries))).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400 ahead of the gate, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case}: expected __type ValidationException, got {response}"
    );
    assert!(
        errortype
            .as_deref()
            .is_none_or(|header| header.starts_with("ValidationException")),
        "case {case}: expected the x-amzn-errortype header to resolve ValidationException, got \
         {errortype:?}"
    );
    let named: Vec<&str> = row.repeated.iter().map(String::as_str).collect();
    assert_eq!(
        response["message"].as_str(),
        Some(expected_message(&named).as_str()),
        "case {case}: expected the refusal to name every repeated ARN once, got {response}"
    );
    assert!(
        response["batchUpdateRecommendationResourceExclusionErrors"].is_null(),
        "case {case}: expected the whole request to be refused with no per-entry result, got \
         {response}"
    );
    response
}

#[tokio::test]
async fn a_malformed_batch_is_refused_the_same_way_on_every_support_plan() {
    // The answer each row got on the first plan, which every other plan's answer
    // must equal: the refusal is the request's, so it cannot differ by plan.
    let mut answers: BTreeMap<String, Value> = BTreeMap::new();

    for plan in SupportPlan::ALL {
        let (router, engine) = emulator_on_plan(*plan);
        let before = stored_exclusions(&engine);

        for row in repeated_arn_rows() {
            let case = format!("{} on {plan:?}", row.name);
            let response = assert_validation_refusal(&router, &row, &case).await;

            match answers.get(&row.name) {
                None => {
                    answers.insert(row.name.clone(), response);
                }
                Some(expected) => assert_eq!(
                    &response, expected,
                    "case {case}: expected the same answer every plan gets, got {response} where \
                     another plan answered {expected}"
                ),
            }

            // Nothing was applied: not the repeated entries, and not the
            // well-formed entries beside them.
            let after = stored_exclusions(&engine);
            assert_eq!(
                after, before,
                "case {case}: expected the refused batch to write nothing, got {after:?} where \
                 the store held {before:?}"
            );
        }
    }

    assert_eq!(
        answers.len(),
        repeated_arn_rows().len(),
        "expected one recorded answer per row, got {}",
        answers.len()
    );
}

/// The two halves of the class agree, and the gate still answers what it owns: on
/// an account whose plan excludes the API, a batch below the modelled list minimum
/// and a batch repeating an ARN are both 400, while a WELL-FORMED batch is the
/// gate's 403.
#[tokio::test]
async fn both_halves_of_the_class_precede_the_gate_and_a_well_formed_batch_does_not() {
    let unentitled: Vec<SupportPlan> = SupportPlan::ALL
        .iter()
        .copied()
        .filter(|plan| !plan.includes_trusted_advisor_api())
        .collect();
    assert!(
        !unentitled.is_empty(),
        "the service must classify at least one plan as not covering the API, or the rows below \
         would assert nothing"
    );

    let applicable = resource_arn(STANDARD_CHECK, "res-ebs-0001");
    for plan in unentitled {
        let (router, _engine) = emulator_on_plan(plan);

        // The modelled half: an empty list, below `@length(1, 25)`.
        let (status, errortype, response) = dispatch(
            &router,
            "PUT",
            BATCH_PATH,
            Some(json!({ "recommendationResourceExclusions": [] })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case an empty batch on {plan:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            errortype.as_deref(),
            Some("ValidationException"),
            "case an empty batch on {plan:?}: expected x-amzn-errortype ValidationException, got \
             {errortype:?} with {response}"
        );

        // The handler-owned half: one ARN twice.
        let repeated = Row {
            name: "the same ARN twice".to_string(),
            entries: vec![(applicable.clone(), true), (applicable.clone(), false)],
            repeated: vec![applicable.clone()],
        };
        assert_validation_refusal(&router, &repeated, &format!("one ARN twice on {plan:?}")).await;

        // And a well-formed batch from the same caller is the gate's own refusal.
        let (status, errortype, response) = dispatch(
            &router,
            "PUT",
            BATCH_PATH,
            Some(body(&[(applicable.clone(), true)])),
        )
        .await;
        assert_eq!(
            status, 403,
            "case a well-formed batch on {plan:?}: expected HTTP 403, got {status} with {response}"
        );
        assert_eq!(
            errortype.as_deref(),
            Some("AccessDeniedException"),
            "case a well-formed batch on {plan:?}: expected x-amzn-errortype \
             AccessDeniedException, got {errortype:?} with {response}"
        );
    }
}
