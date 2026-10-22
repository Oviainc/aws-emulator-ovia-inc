// SPDX-License-Identifier: CC0-1.0

//! What a Clean Rooms create decides for the caller, and what the caller decides.
//!
//! The rule under test: a create stores, and every later read reports, an EFFECTIVE
//! value for each optional member whose value the SERVICE decides — a member with a
//! documented default is stored at that default when the request omits it, and a
//! sub-structure every one of whose members is defaulted is materialized rather than
//! left absent — while `Collaboration$memberStatus` is derived from whether the caller
//! actually holds a membership in the collaboration.
//!
//! Each table varies what the rule generalises over (present/absent request member,
//! every value of the member's enum or boolean domain, which sub-configs the request
//! fills in, and who holds the membership), so the recorded case the issue cited is one
//! row among many. Requests go over the wire through the same `MultiServiceRouter` the
//! production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, minimal_collaboration, Emulator, ALL_ANALYTICS_ENGINES, ALL_MEMBER_ABILITIES,
    ALL_LOG_STATUSES,
};

/// The engine a collaboration created without one runs on. The model's `AnalyticsEngine`
/// enum declares SPARK | CLEAN_ROOMS_SQL and `CreateCollaborationInput$analyticsEngine`
/// notes "After July 16, 2025, the CLEAN_ROOMS_SQL parameter will no longer be
/// available", leaving SPARK as the engine the service picks.
const DEFAULT_ANALYTICS_ENGINE: &str = "SPARK";

/// `CreateCollaborationInput$isMetricsEnabled` and `CreateMembershipInput$isMetricsEnabled`:
/// "The default value is `false`."
const DEFAULT_IS_METRICS_ENABLED: bool = false;

/// Every ML cost `MembershipMLPaymentConfig` names, in the model's member order.
const ALL_ML_COSTS: [&str; 3] = ["modelTraining", "modelInference", "syntheticDataGeneration"];

/// The three ML sub-configs of a membership's accepted payment configuration, as
/// `{cost: isResponsible}`.
fn ml_responsibilities(payment: &Value) -> Vec<(&'static str, Option<bool>)> {
    ALL_ML_COSTS
        .iter()
        .map(|cost| {
            (
                *cost,
                payment["machineLearning"][*cost]["isResponsible"].as_bool(),
            )
        })
        .collect()
}

#[tokio::test]
async fn create_collaboration_materializes_the_engine_and_metrics_setting_in_force() {
    let emulator = Emulator::new();

    // Rows span the cross-product of "the caller chose" and "the caller left it to the
    // service" for both members: every `AnalyticsEngine` value plus absent, and both
    // booleans plus absent.
    let engines: [Option<&str>; 3] = [
        None,
        Some(ALL_ANALYTICS_ENGINES[0]),
        Some(ALL_ANALYTICS_ENGINES[1]),
    ];
    let metrics: [Option<bool>; 3] = [None, Some(true), Some(false)];

    for engine in engines {
        for is_metrics_enabled in metrics {
            let label = format!("analyticsEngine={engine:?}, isMetricsEnabled={is_metrics_enabled:?}");
            let mut body = minimal_collaboration("defaults");
            if let Some(engine) = engine {
                body["analyticsEngine"] = json!(engine);
            }
            if let Some(flag) = is_metrics_enabled {
                body["isMetricsEnabled"] = json!(flag);
            }

            let (status, created) = emulator.create_collaboration(body).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected CreateCollaboration 200, got {status} with {created}"
            );
            let collaboration = created["collaboration"].clone();
            let id = collaboration["id"].as_str().unwrap_or_else(|| {
                panic!("case {label:?}: no collaboration.id in {created}")
            });

            let expected_engine = engine.unwrap_or(DEFAULT_ANALYTICS_ENGINE);
            let expected_metrics = is_metrics_enabled.unwrap_or(DEFAULT_IS_METRICS_ENABLED);
            assert_eq!(
                collaboration["analyticsEngine"].as_str(),
                Some(expected_engine),
                "case {label:?}: expected analyticsEngine {expected_engine:?}, got {:?}",
                collaboration.get("analyticsEngine")
            );
            assert_eq!(
                collaboration["isMetricsEnabled"].as_bool(),
                Some(expected_metrics),
                "case {label:?}: expected isMetricsEnabled {expected_metrics}, got {:?}",
                collaboration.get("isMetricsEnabled")
            );

            // A read-back through the implemented reader must report the same effective
            // values: they are STORED, not computed per response.
            let (status, fetched) = emulator.get_collaboration(id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetCollaboration 200, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["collaboration"], collaboration,
                "case {label:?}: GetCollaboration must report what was stored, got {:?}",
                fetched["collaboration"]
            );

            // And the list projection carries the same engine (CollaborationSummary).
            let (status, list) = emulator.call("GET", "/collaborations", None).await;
            assert_eq!(status, 200, "case {label:?}: ListCollaborations: {list}");
            let summary = list["collaborationList"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: no collaborationList in {list}"))
                .iter()
                .find(|row| row["id"].as_str() == Some(id))
                .unwrap_or_else(|| panic!("case {label:?}: {id} is not listed in {list}"))
                .clone();
            assert_eq!(
                summary["analyticsEngine"].as_str(),
                Some(expected_engine),
                "case {label:?}: the summary must carry analyticsEngine {expected_engine:?}, got {:?}",
                summary.get("analyticsEngine")
            );
        }
    }
}

#[tokio::test]
async fn create_membership_materializes_metrics_and_every_ml_payment_responsibility() {
    // Rows vary the metrics request member (absent / both booleans) against who assigned
    // the ML costs: nobody, the joining member for one cost, the joining member for all
    // three, and the collaboration creator for one cost. A cost nobody was assigned is
    // reported as not-responsible, never omitted.
    struct PaymentCase {
        label: &'static str,
        /// `creatorPaymentConfiguration` of the collaboration, if any.
        creator_payment: Option<Value>,
        /// `paymentConfiguration` of the CreateMembership request, if any.
        requested_payment: Option<Value>,
        /// Expected `isResponsible` per ML cost, in `ALL_ML_COSTS` order.
        expected_ml: [bool; 3],
    }

    let payment_cases = vec![
        PaymentCase {
            label: "nobody assigned any ML cost",
            creator_payment: None,
            requested_payment: None,
            expected_ml: [false, false, false],
        },
        PaymentCase {
            label: "the member accepted a payment config with no ML block",
            creator_payment: None,
            requested_payment: Some(json!({ "queryCompute": { "isResponsible": true } })),
            expected_ml: [false, false, false],
        },
        PaymentCase {
            label: "the member accepted model training only",
            creator_payment: None,
            requested_payment: Some(json!({
                "queryCompute": { "isResponsible": true },
                "machineLearning": { "modelTraining": { "isResponsible": true } },
            })),
            expected_ml: [true, false, false],
        },
        PaymentCase {
            label: "the member accepted every ML cost, with mixed values",
            creator_payment: None,
            requested_payment: Some(json!({
                "queryCompute": { "isResponsible": false },
                "machineLearning": {
                    "modelTraining": { "isResponsible": true },
                    "modelInference": { "isResponsible": false },
                    "syntheticDataGeneration": { "isResponsible": true },
                },
            })),
            expected_ml: [true, false, true],
        },
        PaymentCase {
            label: "the creator assigned model inference to this member",
            creator_payment: Some(json!({
                "queryCompute": { "isResponsible": true },
                "machineLearning": { "modelInference": { "isResponsible": true } },
            })),
            requested_payment: None,
            expected_ml: [false, true, false],
        },
    ];

    let metrics: [Option<bool>; 3] = [None, Some(true), Some(false)];

    for case in payment_cases {
        for is_metrics_enabled in metrics {
            let emulator = Emulator::new();
            let label = format!(
                "{}, isMetricsEnabled={is_metrics_enabled:?}",
                case.label
            );

            let mut collaboration_body = minimal_collaboration("ml-defaults");
            // Every ability, so no ability gates the ML block: only an assignment does.
            collaboration_body["creatorMemberAbilities"] = json!(ALL_MEMBER_ABILITIES);
            if let Some(creator_payment) = case.creator_payment.clone() {
                collaboration_body["creatorPaymentConfiguration"] = creator_payment;
            }
            let (status, created) = emulator.create_collaboration(collaboration_body).await;
            assert_eq!(status, 200, "case {label:?}: CreateCollaboration: {created}");
            let collaboration_id = created["collaboration"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: no collaboration.id in {created}"))
                .to_string();

            let mut body = json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": ALL_LOG_STATUSES[0],
            });
            if let Some(requested_payment) = case.requested_payment.clone() {
                body["paymentConfiguration"] = requested_payment;
            }
            if let Some(flag) = is_metrics_enabled {
                body["isMetricsEnabled"] = json!(flag);
            }
            let (status, response) = emulator.create_membership(body).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected CreateMembership 200, got {status} with {response}"
            );
            let membership = response["membership"].clone();

            let expected_metrics = is_metrics_enabled.unwrap_or(DEFAULT_IS_METRICS_ENABLED);
            assert_eq!(
                membership["isMetricsEnabled"].as_bool(),
                Some(expected_metrics),
                "case {label:?}: expected isMetricsEnabled {expected_metrics}, got {:?}",
                membership.get("isMetricsEnabled")
            );

            let payment = membership["paymentConfiguration"].clone();
            let expected: Vec<(&str, Option<bool>)> = ALL_ML_COSTS
                .iter()
                .zip(case.expected_ml)
                .map(|(cost, flag)| (*cost, Some(flag)))
                .collect();
            assert_eq!(
                ml_responsibilities(&payment),
                expected,
                "case {label:?}: expected machineLearning {expected:?}, got {payment}"
            );
            // jobCompute is the other optional sub-config of the shape: also materialized.
            assert!(
                payment["jobCompute"]["isResponsible"].as_bool().is_some(),
                "case {label:?}: jobCompute must be materialized, got {payment}"
            );

            // Stored, not computed per response: the reader reports the same body.
            let membership_id = membership["id"].as_str().unwrap().to_string();
            let (status, fetched) = emulator.get_membership(&membership_id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetMembership 200, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["membership"], membership,
                "case {label:?}: GetMembership must report what was stored, got {:?}",
                fetched["membership"]
            );
        }
    }
}

#[tokio::test]
async fn member_status_is_invited_until_the_caller_holds_a_membership() {
    // Rows differ in the caller's relation to the collaboration and in how many other
    // members it declares; the caller is the creator in each (the emulator presents one
    // account), so this pins the creator's own status, which is the one a client polls
    // to learn whether it still owes a CreateMembership.
    let cases: Vec<(&str, Value)> = vec![
        ("no other members", minimal_collaboration("status-alone")),
        (
            "one other member",
            {
                let mut body = minimal_collaboration("status-one");
                body["members"] = json!([{
                    "accountId": "111111111111",
                    "memberAbilities": [ALL_MEMBER_ABILITIES[0]],
                    "displayName": "partner-a",
                }]);
                body
            },
        ),
        (
            "two other members, every ability for the creator",
            {
                let mut body = minimal_collaboration("status-two");
                body["creatorMemberAbilities"] = json!(ALL_MEMBER_ABILITIES);
                body["members"] = json!([
                    {
                        "accountId": "111111111111",
                        "memberAbilities": [ALL_MEMBER_ABILITIES[1]],
                        "displayName": "partner-a",
                    },
                    {
                        "accountId": "222222222222",
                        "memberAbilities": [ALL_MEMBER_ABILITIES[2]],
                        "displayName": "partner-b",
                    },
                ]);
                body
            },
        ),
    ];

    for (label, body) in cases {
        let emulator = Emulator::new();
        let (status, created) = emulator.create_collaboration(body).await;
        assert_eq!(status, 200, "case {label:?}: CreateCollaboration: {created}");
        let collaboration = created["collaboration"].clone();
        let id = collaboration["id"].as_str().unwrap().to_string();
        assert_eq!(
            collaboration["creatorAccountId"].as_str(),
            Some(account_id()),
            "case {label:?}: the caller must be the creator, got {:?}",
            collaboration["creatorAccountId"]
        );

        // 1. Created, not yet joined: INVITED, on the create response, the reader and
        //    the list projection alike.
        for (source, status_value) in [
            ("CreateCollaboration", collaboration["memberStatus"].clone()),
            ("GetCollaboration", {
                let (_, fetched) = emulator.get_collaboration(&id).await;
                fetched["collaboration"]["memberStatus"].clone()
            }),
            ("ListCollaborations", {
                let (_, list) = emulator.call("GET", "/collaborations", None).await;
                list["collaborationList"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["id"].as_str() == Some(id.as_str()))
                    .unwrap()["memberStatus"]
                    .clone()
            }),
        ] {
            assert_eq!(
                status_value.as_str(),
                Some("INVITED"),
                "case {label:?}: {source} must report INVITED before the caller joins, got {status_value:?}"
            );
        }

        // 2. Joined: ACTIVE.
        let (status, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": id,
                "queryLogStatus": ALL_LOG_STATUSES[0],
            }))
            .await;
        assert_eq!(status, 200, "case {label:?}: CreateMembership: {joined}");
        let membership_id = joined["membership"]["id"].as_str().unwrap().to_string();
        let (_, fetched) = emulator.get_collaboration(&id).await;
        assert_eq!(
            fetched["collaboration"]["memberStatus"].as_str(),
            Some("ACTIVE"),
            "case {label:?}: the holder of a membership is ACTIVE, got {:?}",
            fetched["collaboration"]["memberStatus"]
        );

        // 3. Left: the membership is gone, so the status falls back to INVITED — the
        //    status is derived from state, not latched by the create.
        let (status, body) = emulator
            .call("DELETE", &format!("/memberships/{membership_id}"), None)
            .await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected DeleteMembership 204, got {status} with {body}"
        );
        let (_, fetched) = emulator.get_collaboration(&id).await;
        assert_eq!(
            fetched["collaboration"]["memberStatus"].as_str(),
            Some("INVITED"),
            "case {label:?}: with no membership the caller is INVITED again, got {:?}",
            fetched["collaboration"]["memberStatus"]
        );
    }
}
