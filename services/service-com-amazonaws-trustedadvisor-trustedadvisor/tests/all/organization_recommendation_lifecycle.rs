// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ORGANIZATION recommendation loop —
//! `UpdateOrganizationRecommendationLifecycle` and the
//! `GetOrganizationRecommendation` it is read back through — driven over HTTP
//! through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded
//! call:
//!
//! * **a lifecycle update of a prioritized organization recommendation replaces
//!   that recommendation's `lifecycleStage`, `updateReason` and
//!   `updateReasonCode`, stamps `lastUpdatedAt` from the clock, and sets
//!   `resolvedAt` exactly when the new stage is `resolved`** — asserted for
//!   every `UpdateRecommendationLifecycleStage` value, every reason code the
//!   model declares, and a request that omits the reason members;
//! * the write is confined to the organization recommendation the identifier
//!   names: no other organization recommendation and no account-level
//!   recommendation moves;
//! * **an organization recommendation whose `type` is not `priority` conflicts
//!   (409) and nothing is written**, because the operation's own doc says it
//!   "only supports prioritized recommendations";
//! * a well-formed identifier that resolves to nothing is 404 and one outside
//!   the modelled pattern is 400, for both the update and the read;
//! * `resourcesAggregates` is COUNTED on every read from the resources of the
//!   member accounts' recommendations the organization recommendation
//!   aggregates — never stored — so an exclusion made through
//!   `BatchUpdateRecommendationResourceExclusion` shows up in it.
//!
//! The statuses and error codes come from the model:
//! `smithy_query(query="operation",operation="UpdateOrganizationRecommendationLifecycle,GetOrganizationRecommendation",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — PUT `/v1/organization-recommendations/{id}/lifecycle` answers 200 with an
//! empty body, GET `/v1/organization-recommendations/{id}` answers 200 with an
//! `OrganizationRecommendation`, and both bind `ConflictException` 409,
//! `ResourceNotFoundException` 404 and `ValidationException` 400.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; the gate ahead of every handler is pinned in
//! `tests/all/support_level_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::ops::types::{
    RECOMMENDATION_STATUS_VALUES, RECOMMENDATION_TYPE_VALUES,
    UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_REASON_CODE_VALUES,
    UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES,
};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{
    call, check_arn, emulator_on_plan, error_code, field_of_each, label, recommendation_arn,
    resource_arn, timestamp_secs,
};

/// One seeded check and the organization recommendation its findings aggregate
/// into. Written out by hand; its source is the `SEED` table in `src/state.rs`,
/// and the rows are in organization-recommendation ARN order (check id order),
/// which is the store's key order.
struct Seeded {
    check_id: &'static str,
    /// `RecommendationType` — only `priority` recommendations can have their
    /// lifecycle moved.
    kind: &'static str,
    status: &'static str,
    /// The stage the seed left, before any caller moved it.
    lifecycle_stage: Option<&'static str>,
    pillars: &'static [&'static str],
}

const SEEDED: &[Seeded] = &[
    Seeded {
        check_id: "amazon-ebs-snapshots",
        kind: "standard",
        status: "error",
        lifecycle_stage: None,
        pillars: &["fault_tolerance"],
    },
    Seeded {
        check_id: "iam-access-key-rotation",
        kind: "standard",
        status: "ok",
        lifecycle_stage: None,
        pillars: &["security"],
    },
    Seeded {
        check_id: "low-utilization-ec2-instances",
        kind: "priority",
        status: "warning",
        lifecycle_stage: Some("pending_response"),
        pillars: &["cost_optimizing"],
    },
    Seeded {
        check_id: "rds-idle-db-instances",
        kind: "priority",
        status: "warning",
        lifecycle_stage: Some("in_progress"),
        pillars: &["cost_optimizing", "performance"],
    },
];

/// A mounted emulator whose account's support plan includes the Trusted Advisor
/// API, so each call reaches the operation's own behaviour.
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// `arn:aws:trustedadvisor:::organization-recommendation/<id>` — the ARN
/// pattern `OrganizationRecommendationArn` declares (no region and no account
/// segment). Each seeded check aggregates into
/// `<check id>-organization-recommendation`.
pub(crate) fn organization_recommendation_arn(check_id: &str) -> String {
    format!(
        "arn:aws:trustedadvisor:::organization-recommendation/{check_id}-organization-recommendation"
    )
}

/// The modelled URI of `GetOrganizationRecommendation` for one seeded check.
pub(crate) fn get_path(check_id: &str) -> String {
    format!(
        "/v1/organization-recommendations/{}",
        label(&organization_recommendation_arn(check_id))
    )
}

/// The modelled URI of `UpdateOrganizationRecommendationLifecycle` for one
/// seeded check.
fn lifecycle_path(check_id: &str) -> String {
    format!("{}/lifecycle", get_path(check_id))
}

/// The `organizationRecommendation` one read reports, or a panic naming the
/// response that carried none.
pub(crate) async fn read(router: &MultiServiceRouter, case: &str, check_id: &str) -> Value {
    let (status, body) = call(router, "GET", &get_path(check_id), None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from GetOrganizationRecommendation, got {status} with {body}"
    );
    let got = body["organizationRecommendation"].clone();
    assert!(
        got.is_object(),
        "case {case}: expected an organizationRecommendation, got {body}"
    );
    got
}

/// The lifecycle members a read reports, as `(stage, reason, reasonCode)`.
fn lifecycle_of(recommendation: &Value) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        recommendation["lifecycleStage"].as_str(),
        recommendation["updateReason"].as_str(),
        recommendation["updateReasonCode"].as_str(),
    )
}

#[tokio::test]
async fn every_seeded_organization_recommendation_reports_members_computed_from_the_store() {
    let router = emulator();

    for row in SEEDED {
        let case = row.check_id;
        let got = read(&router, case, row.check_id).await;

        assert_eq!(
            got["arn"].as_str(),
            Some(organization_recommendation_arn(row.check_id).as_str()),
            "case {case}: expected arn {:?}, got {:?}",
            organization_recommendation_arn(row.check_id),
            got["arn"]
        );
        assert_eq!(
            got["id"].as_str(),
            Some(format!("{}-organization-recommendation", row.check_id).as_str()),
            "case {case}: expected the organization recommendation id, got {:?}",
            got["id"]
        );
        assert_eq!(
            got["checkArn"].as_str(),
            Some(check_arn(row.check_id).as_str()),
            "case {case}: expected checkArn {:?}, got {:?}",
            check_arn(row.check_id),
            got["checkArn"]
        );
        assert_eq!(
            got["type"].as_str(),
            Some(row.kind),
            "case {case}: expected type {:?}, got {:?}",
            row.kind,
            got["type"]
        );
        assert_eq!(
            got["status"].as_str(),
            Some(row.status),
            "case {case}: expected status {:?}, got {:?}",
            row.status,
            got["status"]
        );
        assert_eq!(
            lifecycle_of(&got),
            (row.lifecycle_stage, None, None),
            "case {case}: expected the seeded lifecycle {:?} and no update wording, got {got}",
            row.lifecycle_stage
        );
        let pillars = got["pillars"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected pillars, got {got}"))
            .iter()
            .map(|pillar| pillar.as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>();
        assert_eq!(
            pillars, row.pillars,
            "case {case}: expected pillars {:?}, got {pillars:?}",
            row.pillars
        );
        for member in ["name", "description", "source"] {
            assert!(
                got[member].as_str().is_some_and(|text| !text.is_empty()),
                "case {case}: expected a non-empty {member}, got {:?}",
                got[member]
            );
        }
        assert!(
            got["resolvedAt"].is_null(),
            "case {case}: expected no resolvedAt before any update, got {:?}",
            got["resolvedAt"]
        );
        assert!(
            timestamp_secs(&got["createdAt"]) > 0.0
                && timestamp_secs(&got["lastUpdatedAt"]) >= timestamp_secs(&got["createdAt"]),
            "case {case}: expected createdAt <= lastUpdatedAt, got {got}"
        );

        // The classification the store holds is one the model declares.
        assert!(
            RECOMMENDATION_TYPE_VALUES.contains(&row.kind),
            "case {case}: {:?} is not a RecommendationType",
            row.kind
        );
        assert!(
            RECOMMENDATION_STATUS_VALUES.contains(&row.status),
            "case {case}: {:?} is not a RecommendationStatus",
            row.status
        );

        // `resourcesAggregates` is the aggregation of the member accounts'
        // recommendations: the emulator presents one account, so it equals that
        // account's recommendation for the same check.
        let (status, account) = call(
            &router,
            "GET",
            &format!("/v1/recommendations/{}", label(&recommendation_arn(row.check_id))),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetRecommendation, got {status} with {account}"
        );
        assert_eq!(
            got["resourcesAggregates"],
            account["recommendation"]["resourcesAggregates"],
            "case {case}: expected the member recommendation's aggregates {:?}, got {:?}",
            account["recommendation"]["resourcesAggregates"],
            got["resourcesAggregates"]
        );
        // `pillarSpecificAggregates` aggregates the same members, so with one
        // member account it is that recommendation's cost aggregate — present
        // exactly for the cost-optimizing rows.
        assert_eq!(
            got["pillarSpecificAggregates"],
            account["recommendation"]["pillarSpecificAggregates"],
            "case {case}: expected the member recommendation's pillar aggregates {:?}, got {:?}",
            account["recommendation"]["pillarSpecificAggregates"],
            got["pillarSpecificAggregates"]
        );
        assert_eq!(
            got["pillarSpecificAggregates"]["costOptimizing"].is_object(),
            row.pillars.contains(&"cost_optimizing"),
            "case {case}: expected a costOptimizing aggregate only for a cost-optimizing \
             recommendation, got {:?}",
            got["pillarSpecificAggregates"]
        );
    }
}

/// `resourcesAggregates` is counted from the member recommendations' resources
/// on every read, so a change made to one of those resources is visible without
/// a second write: the rows exclude a resource, exclude another, then include
/// the first again.
#[tokio::test]
async fn organization_resources_aggregates_are_counted_on_every_read() {
    let router = emulator();
    let check_id = "amazon-ebs-snapshots";
    let first = resource_arn(check_id, "res-ebs-0001");
    let second = resource_arn(check_id, "res-ebs-0002");
    let resources_path = format!(
        "/v1/recommendations/{}/resources",
        label(&recommendation_arn(check_id))
    );

    let rows: &[(&str, &str, bool)] = &[
        ("exclude the first resource", &first, true),
        ("exclude the second resource too", &second, true),
        ("include the first resource again", &first, false),
    ];

    for (case, arn, is_excluded) in rows {
        let (status, body) = call(
            &router,
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion",
            Some(json!({
                "recommendationResourceExclusions": [{ "arn": arn, "isExcluded": is_excluded }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the exclusion batch, got {status} with {body}"
        );

        // What the resources themselves now report, which is what the
        // organization recommendation's `excludedCount` must count.
        let (_, listed) = call(&router, "GET", &resources_path, None).await;
        let excluded = field_of_each(&listed, "recommendationResourceSummaries", "exclusionStatus")
            .into_iter()
            .filter(|status| status == "excluded")
            .count();

        let got = read(&router, case, check_id).await;
        assert_eq!(
            got["resourcesAggregates"]["excludedCount"].as_u64(),
            Some(excluded as u64),
            "case {case}: expected excludedCount {excluded}, got {:?}",
            got["resourcesAggregates"]["excludedCount"]
        );
    }
}

/// The aggregates are a SUM over every member recommendation an organization
/// recommendation covers, not a pass-through of one: the seed records one member
/// account (the emulator presents exactly one), so this records a second member
/// the way something outside the API would and compares the organization's
/// figures against both members' own reads.
#[tokio::test]
async fn aggregates_sum_over_every_member_recommendation() {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    let target = "low-utilization-ec2-instances";
    let members = [
        recommendation_arn(target),
        recommendation_arn("rds-idle-db-instances"),
    ];

    {
        let state = service_state(&*engine).expect("this service's state");
        let mut state = state.lock();
        let arn = organization_recommendation_arn(target);
        state
            .organization_recommendations
            .get_mut(&arn)
            .expect("the seeded organization recommendation")
            .member_recommendation_arns = members.to_vec();
    }

    // What each member reports on its own, summed.
    let mut expected_counts = [0u64; 4];
    let mut expected_savings = 0.0f64;
    for member in &members {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/v1/recommendations/{}", label(member)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member}: expected 200 from GetRecommendation, got {status} with {body}"
        );
        let aggregates = &body["recommendation"]["resourcesAggregates"];
        for (index, counter) in ["okCount", "warningCount", "errorCount", "excludedCount"]
            .iter()
            .enumerate()
        {
            expected_counts[index] += aggregates[counter].as_u64().unwrap_or_else(|| {
                panic!("case {member}: expected a {counter}, got {aggregates}")
            });
        }
        expected_savings += body["recommendation"]["pillarSpecificAggregates"]["costOptimizing"]
            ["estimatedMonthlySavings"]
            .as_f64()
            .unwrap_or(0.0);
    }

    let got = read(&router, "two member recommendations", target).await;
    for (index, counter) in ["okCount", "warningCount", "errorCount", "excludedCount"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            got["resourcesAggregates"][counter].as_u64(),
            Some(expected_counts[index]),
            "case two member recommendations: expected {counter} {}, got {:?}",
            expected_counts[index],
            got["resourcesAggregates"][counter]
        );
    }
    let savings = got["pillarSpecificAggregates"]["costOptimizing"]["estimatedMonthlySavings"]
        .as_f64()
        .unwrap_or_else(|| panic!("case two member recommendations: expected savings, got {got}"));
    assert!(
        (savings - expected_savings).abs() < 1e-6,
        "case two member recommendations: expected estimatedMonthlySavings {expected_savings}, \
         got {savings}"
    );
    assert!(
        expected_savings > 0.0,
        "case two member recommendations: the members must report some savings for this to be a \
         sum, got {expected_savings}"
    );
}

#[tokio::test]
async fn a_lifecycle_update_persists_and_the_next_read_reports_it() {
    // Every `UpdateRecommendationLifecycleStage` value, paired with reason codes
    // from `UpdateRecommendationLifecycleStageReasonCode`, ending on a stage
    // that leaves `resolved` again so the `resolvedAt` rule is tested both ways,
    // and then a row that OMITS both reason members: the operation is an
    // `@idempotent` PUT, so the request REPLACES the lifecycle and the previous
    // update's wording does not stay attached to the new stage. `updateReason`
    // is modelled `length 10..4096`, so each reason is at least ten characters.
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        (
            "in_progress",
            Some("working the finding now"),
            Some("valid_business_case"),
        ),
        (
            "pending_response",
            Some("waiting on the organization's account owner"),
            Some("other"),
        ),
        (
            "resolved",
            Some("right-sized every member account's instances"),
            Some("other_methods_available"),
        ),
        (
            "dismissed",
            Some("reopened then dismissed"),
            Some("low_priority"),
        ),
        ("dismissed", Some("not this account's call"), Some("non_critical_account")),
        ("dismissed", Some("does not apply here"), Some("not_applicable")),
        ("dismissed", Some("a temporary account"), Some("temporary_account")),
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

    // Every seeded organization recommendation whose `type` is `priority`.
    for target in SEEDED.iter().filter(|row| row.kind == "priority") {
        let router = emulator();
        let seeded_last_updated =
            timestamp_secs(&read(&router, target.check_id, target.check_id).await["lastUpdatedAt"]);

        for (stage, reason, reason_code) in rows {
            let case = format!("{}: {stage}/{reason:?}", target.check_id);
            let mut request = json!({ "lifecycleStage": stage });
            if let Some(reason) = reason {
                request["updateReason"] = json!(reason);
            }
            if let Some(reason_code) = reason_code {
                request["updateReasonCode"] = json!(reason_code);
            }

            // Twice: the operation is `@idempotent`, so the second call is the
            // same request and must leave the same lifecycle behind.
            for attempt in 1..=2 {
                let (status, body) = call(
                    &router,
                    "PUT",
                    &lifecycle_path(target.check_id),
                    Some(request.clone()),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case} attempt {attempt}: expected 200, got {status} with {body}"
                );
                assert_eq!(
                    body,
                    Value::Null,
                    "case {case} attempt {attempt}: expected an empty body, got {body}"
                );

                let got = read(&router, &case, target.check_id).await;
                assert_eq!(
                    lifecycle_of(&got),
                    (Some(*stage), *reason, *reason_code),
                    "case {case} attempt {attempt}: expected the requested lifecycle, got {got}"
                );

                let last_updated = timestamp_secs(&got["lastUpdatedAt"]);
                assert!(
                    last_updated > seeded_last_updated,
                    "case {case} attempt {attempt}: expected lastUpdatedAt after the seeded \
                     {seeded_last_updated}, got {last_updated}"
                );
                if *stage == "resolved" {
                    let resolved_at = timestamp_secs(&got["resolvedAt"]);
                    // A millisecond of tolerance: both come from the same stored
                    // instant, so this only absorbs a change of wire encoding.
                    assert!(
                        (resolved_at - last_updated).abs() < 1e-3,
                        "case {case} attempt {attempt}: expected resolvedAt to be the update \
                         time {last_updated}, got {resolved_at}"
                    );
                } else {
                    assert!(
                        got["resolvedAt"].is_null(),
                        "case {case} attempt {attempt}: expected no resolvedAt, got {:?}",
                        got["resolvedAt"]
                    );
                }
            }

            // The write is confined to the organization recommendation the
            // identifier named: every other one still reports its seeded stage,
            // and so does the account-level recommendation of the same check,
            // which is a separate resource with its own lifecycle operation.
            for other in SEEDED.iter().filter(|row| row.check_id != target.check_id) {
                let got = read(&router, &format!("{case} / {}", other.check_id), other.check_id)
                    .await;
                assert_eq!(
                    lifecycle_of(&got),
                    (other.lifecycle_stage, None, None),
                    "case {case}: the update must not move {}, got {got}",
                    other.check_id
                );
            }
            let (_, account) = call(
                &router,
                "GET",
                &format!(
                    "/v1/recommendations/{}",
                    label(&recommendation_arn(target.check_id))
                ),
                None,
            )
            .await;
            assert_eq!(
                account["recommendation"]["lifecycleStage"].as_str(),
                target.lifecycle_stage,
                "case {case}: the update must not move the account-level recommendation, got {}",
                account["recommendation"]
            );
        }
    }
}

#[tokio::test]
async fn a_lifecycle_update_of_a_non_priority_organization_recommendation_conflicts() {
    let router = emulator();

    // Every seeded organization recommendation whose `type` is not `priority`:
    // the operation's own doc says it "only supports prioritized
    // recommendations".
    for row in SEEDED.iter().filter(|row| row.kind != "priority") {
        let case = row.check_id;
        // Rows differ along the requested stage, so the refusal is about the
        // recommendation's type and not about one stage.
        for stage in UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES {
            let (status, body) = call(
                &router,
                "PUT",
                &lifecycle_path(row.check_id),
                Some(json!({
                    "lifecycleStage": stage,
                    "updateReason": "not applicable to this organization",
                    "updateReasonCode": "not_applicable",
                })),
            )
            .await;
            assert_eq!(
                status, 409,
                "case {case}/{stage}: expected 409, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case}/{stage}: expected ConflictException, got {body}"
            );

            // And nothing was written: the lifecycle is still the seeded one.
            let got = read(&router, &format!("{case}/{stage}"), row.check_id).await;
            assert_eq!(
                lifecycle_of(&got),
                (row.lifecycle_stage, None, None),
                "case {case}/{stage}: expected the seeded lifecycle, got {got}"
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_that_resolves_to_nothing_is_404_and_a_malformed_one_is_400() {
    let router = emulator();
    let body = json!({
        "lifecycleStage": "resolved",
        "updateReason": "fixed across the organization",
        "updateReasonCode": "other",
    });

    // Well-formed `OrganizationRecommendationIdentifier`s (len 20..200, pattern
    // `^arn:[\w-]+:trustedadvisor:::organization-recommendation\/[\w-]+$`) that
    // name nothing in the store: the rows differ along what makes them unknown.
    let unknown = [
        (
            "an all-zeros organization recommendation id",
            "arn:aws:trustedadvisor:::organization-recommendation/00000000-0000-0000-0000-000000000000"
                .to_string(),
        ),
        (
            "an id no seed produced",
            organization_recommendation_arn("not-a-seeded-check"),
        ),
        (
            "a seeded id in another partition",
            organization_recommendation_arn("low-utilization-ec2-instances")
                .replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "the ACCOUNT-level id of a seeded recommendation, in the organization form",
            "arn:aws:trustedadvisor:::organization-recommendation/low-utilization-ec2-instances-recommendation"
                .to_string(),
        ),
    ];

    // Identifiers outside the modelled length or pattern: a 400 before any
    // lookup. The rows differ along the constraint broken.
    let malformed = [
        ("shorter than the modelled minimum", "arn:aws:ta".to_string()),
        (
            "not an ARN at all",
            "not-an-arn-but-long-enough-to-pass-the-length-bound".to_string(),
        ),
        (
            "the account-level recommendation ARN, whose form this label does not accept",
            recommendation_arn("low-utilization-ec2-instances"),
        ),
        (
            "an organization recommendation ARN carrying an account segment",
            format!(
                "arn:aws:trustedadvisor::{}:organization-recommendation/some-recommendation",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];

    let operations: &[(&str, &str, &str)] = &[
        (
            "GetOrganizationRecommendation",
            "GET",
            "/v1/organization-recommendations/{}",
        ),
        (
            "UpdateOrganizationRecommendationLifecycle",
            "PUT",
            "/v1/organization-recommendations/{}/lifecycle",
        ),
    ];

    for (operation, method, template) in operations {
        let payload = (*method == "PUT").then(|| body.clone());
        for (what, arn) in &unknown {
            let case = format!("{operation} with {what}");
            let path = template.replace("{}", &label(arn));
            let (status, response) = call(&router, method, &path, payload.clone()).await;
            assert_eq!(
                status, 404,
                "case {case}: expected 404, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {response}"
            );
        }

        for (what, arn) in &malformed {
            let case = format!("{operation} with {what}");
            let path = template.replace("{}", &label(arn));
            let (status, response) = call(&router, method, &path, payload.clone()).await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case}: expected ValidationException, got {response}"
            );
        }
    }
}
