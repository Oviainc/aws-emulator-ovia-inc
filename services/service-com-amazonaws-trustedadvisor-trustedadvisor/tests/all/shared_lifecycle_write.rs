// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE lifecycle-write rule BOTH `Update…Lifecycle`
//! operations of this service apply — `UpdateRecommendationLifecycle` over an
//! account-level recommendation and `UpdateOrganizationRecommendationLifecycle`
//! over an organization recommendation — driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! The rule, over a TABLE of inputs rather than one call per operation: a
//! lifecycle update of a prioritized recommendation REPLACES that
//! recommendation's `lifecycleStage` and both reason members (so a request that
//! omits `updateReason` / `updateReasonCode` clears the stored one), stamps
//! `lastUpdatedAt` from the clock, and sets `resolvedAt` to that same instant
//! exactly when the new stage is `resolved`; an entry whose `type` is not
//! `priority` is refused with `ConflictException` and nothing is written. Both
//! levels are driven with the same rows and their reads are compared against
//! each other, so the account-level and the organization-level lifecycle cannot
//! answer the same request differently.
//!
//! The stage values, the reason codes and the recommendation types the rows span
//! come from the model's own enums (`UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES`,
//! `UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_REASON_CODE_VALUES` and
//! `RECOMMENDATION_TYPE_VALUES` in `ops::types::shared_types`), and the statuses
//! and error codes from the two operations' bindings:
//! `smithy_query(query="operation",operation="UpdateRecommendationLifecycle,UpdateOrganizationRecommendationLifecycle",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — each is a PUT answering 200 with an empty body and binds
//! `ConflictException` 409.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::ops::types::{
    RECOMMENDATION_TYPE_VALUES, UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_REASON_CODE_VALUES,
    UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES,
};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_recommendation_lifecycle::organization_recommendation_arn;
use crate::recommendation_loop::{
    call, emulator_on_plan, error_code, label, recommendation_arn, timestamp_secs,
    PRIORITIZED_CHECK_IDS,
};

/// The `RecommendationType` the two lifecycle operations are documented to
/// support ("This API only supports prioritized recommendations"); every other
/// value `RecommendationType` declares is an entry they are inapplicable to.
const PRIORITY: &str = "priority";

/// The two surfaces the shared lifecycle write serves: the account-level
/// recommendation and the organization recommendation of the same check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Account,
    Organization,
}

impl Level {
    /// Both levels, so a row of the table below is driven through each of the
    /// two operations.
    const ALL: &'static [Self] = &[Self::Account, Self::Organization];

    /// The identifier the operation at this level takes.
    fn arn(self, check_id: &str) -> String {
        match self {
            Self::Account => recommendation_arn(check_id),
            Self::Organization => organization_recommendation_arn(check_id),
        }
    }

    /// The modelled URI of the read that reports what the update wrote.
    fn read_path(self, check_id: &str) -> String {
        let collection = match self {
            Self::Account => "recommendations",
            Self::Organization => "organization-recommendations",
        };
        format!("/v1/{collection}/{}", label(&self.arn(check_id)))
    }

    /// The modelled URI of the lifecycle update itself.
    fn lifecycle_path(self, check_id: &str) -> String {
        format!("{}/lifecycle", self.read_path(check_id))
    }

    /// The response member the read wraps the recommendation in.
    fn read_member(self) -> &'static str {
        match self {
            Self::Account => "recommendation",
            Self::Organization => "organizationRecommendation",
        }
    }

    /// What this service calls the resource in the refusal it answers for an
    /// entry the operation does not apply to.
    fn resource_noun(self) -> &'static str {
        match self {
            Self::Account => "recommendation",
            Self::Organization => "organization recommendation",
        }
    }
}

/// The recommendation one read at `level` reports, or a panic naming the
/// response that carried none.
async fn read(router: &MultiServiceRouter, case: &str, level: Level, check_id: &str) -> Value {
    let (status, body) = call(router, "GET", &level.read_path(check_id), None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from the {level:?}-level read, got {status} with {body}"
    );
    let got = body[level.read_member()].clone();
    assert!(
        got.is_object(),
        "case {case}: expected a {} in the {level:?}-level read, got {body}",
        level.read_member()
    );
    got
}

/// The lifecycle members a read reports, as
/// `(lifecycleStage, updateReason, updateReasonCode)`.
fn lifecycle_of(recommendation: &Value) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        recommendation["lifecycleStage"].as_str(),
        recommendation["updateReason"].as_str(),
        recommendation["updateReasonCode"].as_str(),
    )
}

#[tokio::test]
async fn both_lifecycle_updates_apply_the_same_write() {
    // Rows differ along the new stage, along whether the request carries each
    // reason member, and along which reason code it names: every
    // `UpdateRecommendationLifecycleStage` value (`resolved` followed by a stage
    // that leaves it again, so the `resolvedAt` rule is driven both ways), every
    // reason code the model declares, and a last row that OMITS both reason
    // members. `updateReason` is modelled `length 10..4096`, so each reason is
    // at least ten characters.
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        (
            "in_progress",
            Some("working the finding now"),
            Some("valid_business_case"),
        ),
        (
            "pending_response",
            Some("waiting on the account owner"),
            Some("other"),
        ),
        (
            "resolved",
            Some("right-sized the instances"),
            Some("other_methods_available"),
        ),
        (
            "dismissed",
            Some("reopened then dismissed"),
            Some("low_priority"),
        ),
        (
            "dismissed",
            Some("not this account's call"),
            Some("non_critical_account"),
        ),
        (
            "dismissed",
            Some("does not apply here"),
            Some("not_applicable"),
        ),
        (
            "dismissed",
            Some("a temporary account"),
            Some("temporary_account"),
        ),
        ("in_progress", Some("a reason with no code"), None),
        ("pending_response", None, None),
    ];

    // The table spans every stage and every reason code the model declares, so a
    // value added later is not silently left untested.
    for stage in UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES {
        assert!(
            rows.iter().any(|(row_stage, _, _)| row_stage == stage),
            "the table must cover the lifecycle stage {stage}"
        );
    }
    for code in UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_REASON_CODE_VALUES {
        assert!(
            rows.iter().any(|(_, _, row_code)| *row_code == Some(*code)),
            "the table must cover the reason code {code}"
        );
    }

    // Every check with a prioritized recommendation, which is also every check
    // with an organization recommendation.
    for check_id in PRIORITIZED_CHECK_IDS {
        let (router, _engine) = emulator_on_plan(SupportPlan::Business);

        let mut seeded_last_updated = Vec::new();
        for level in Level::ALL {
            let got = read(&router, check_id, *level, check_id).await;
            seeded_last_updated.push(timestamp_secs(&got["lastUpdatedAt"]));
        }

        for (stage, reason, reason_code) in rows {
            let mut request = json!({ "lifecycleStage": stage });
            if let Some(reason) = reason {
                request["updateReason"] = json!(reason);
            }
            if let Some(reason_code) = reason_code {
                request["updateReasonCode"] = json!(reason_code);
            }

            // What each level reports after the same request, to be compared
            // against the rule AND against each other.
            let mut reported = Vec::new();
            for (index, level) in Level::ALL.iter().enumerate() {
                let case = format!("{check_id}/{level:?}: {stage}/{reason:?}/{reason_code:?}");
                let (status, body) = call(
                    &router,
                    "PUT",
                    &level.lifecycle_path(check_id),
                    Some(request.clone()),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200 from the lifecycle update, got {status} with {body}"
                );
                assert_eq!(
                    body,
                    Value::Null,
                    "case {case}: expected an empty body, got {body}"
                );

                let got = read(&router, &case, *level, check_id).await;
                assert_eq!(
                    lifecycle_of(&got),
                    (Some(*stage), *reason, *reason_code),
                    "case {case}: expected the requested lifecycle to REPLACE the stored one, \
                     got {got}"
                );

                let last_updated = timestamp_secs(&got["lastUpdatedAt"]);
                assert!(
                    last_updated > seeded_last_updated[index],
                    "case {case}: expected lastUpdatedAt after the seeded {}, got {last_updated}",
                    seeded_last_updated[index]
                );

                if *stage == "resolved" {
                    let resolved_at = timestamp_secs(&got["resolvedAt"]);
                    // A millisecond of tolerance: both come from the one stored
                    // instant, so this only absorbs a change of wire encoding.
                    assert!(
                        (resolved_at - last_updated).abs() < 1e-3,
                        "case {case}: expected resolvedAt to be the update instant \
                         {last_updated}, got {resolved_at}"
                    );
                } else {
                    assert!(
                        got["resolvedAt"].is_null(),
                        "case {case}: expected no resolvedAt outside the resolved stage, got {:?}",
                        got["resolvedAt"]
                    );
                }

                reported.push(got);
            }

            // The two levels answered the same request the same way, member for
            // member: one rule, not two.
            let case = format!("{check_id}: {stage}/{reason:?}/{reason_code:?}");
            let (account, organization) = (&reported[0], &reported[1]);
            assert_eq!(
                lifecycle_of(account),
                lifecycle_of(organization),
                "case {case}: expected both levels to report the same lifecycle members, got \
                 {:?} and {:?}",
                lifecycle_of(account),
                lifecycle_of(organization)
            );
            assert_eq!(
                account["resolvedAt"].is_null(),
                organization["resolvedAt"].is_null(),
                "case {case}: expected both levels to agree on resolvedAt, got {:?} and {:?}",
                account["resolvedAt"],
                organization["resolvedAt"]
            );
        }
    }
}

#[tokio::test]
async fn both_lifecycle_updates_refuse_an_entry_of_another_type() {
    // Rows differ along the stored `type`: every `RecommendationType` the model
    // declares EXCEPT the one the two operations support, each driven through
    // both operations. The type is recorded in process, the way the part of
    // Trusted Advisor that classifies a finding would — no request of this API
    // sets it — because at the organization level the store holds prioritized
    // entries only.
    let other_types: Vec<&str> = RECOMMENDATION_TYPE_VALUES
        .iter()
        .copied()
        .filter(|kind| *kind != PRIORITY)
        .collect();
    assert!(
        !other_types.is_empty(),
        "RecommendationType must declare a value other than {PRIORITY}, got {RECOMMENDATION_TYPE_VALUES:?}"
    );

    let check_id = PRIORITIZED_CHECK_IDS[0];

    for kind in &other_types {
        for level in Level::ALL {
            let case = format!("{level:?}: a {kind} entry");
            let (router, engine) = emulator_on_plan(SupportPlan::Business);
            let arn = level.arn(check_id);

            let before = read(&router, &case, *level, check_id).await;

            {
                let state = service_state(&*engine).expect("this service's state");
                let mut state = state.lock();
                match level {
                    Level::Account => {
                        state
                            .recommendations
                            .get_mut(&arn)
                            .unwrap_or_else(|| panic!("case {case}: no seeded recommendation {arn}"))
                            .r#type = (*kind).to_string();
                    }
                    Level::Organization => {
                        state
                            .organization_recommendations
                            .get_mut(&arn)
                            .unwrap_or_else(|| {
                                panic!("case {case}: no seeded organization recommendation {arn}")
                            })
                            .r#type = (*kind).to_string();
                    }
                }
            }

            let (status, body) = call(
                &router,
                "PUT",
                &level.lifecycle_path(check_id),
                Some(json!({
                    "lifecycleStage": "dismissed",
                    "updateReason": "not a prioritized recommendation",
                    "updateReasonCode": "not_applicable",
                })),
            )
            .await;
            assert_eq!(
                status, 409,
                "case {case}: expected 409, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case}: expected ConflictException, got {body}"
            );
            let expected = format!(
                "the lifecycle of the {kind} {} {arn} cannot be updated: this operation only \
                 supports prioritized recommendations",
                level.resource_noun()
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {body}"
            );

            // And the refusal wrote nothing: every lifecycle member the update
            // would have replaced still reports what it did before.
            let after = read(&router, &case, *level, check_id).await;
            assert_eq!(
                lifecycle_of(&after),
                lifecycle_of(&before),
                "case {case}: expected the refusal to leave the lifecycle members alone, got \
                 {after}"
            );
            assert_eq!(
                after["lastUpdatedAt"], before["lastUpdatedAt"],
                "case {case}: expected lastUpdatedAt to be unchanged by the refusal, got {:?}",
                after["lastUpdatedAt"]
            );
            assert_eq!(
                after["resolvedAt"], before["resolvedAt"],
                "case {case}: expected resolvedAt to be unchanged by the refusal, got {:?}",
                after["resolvedAt"]
            );
        }
    }
}
