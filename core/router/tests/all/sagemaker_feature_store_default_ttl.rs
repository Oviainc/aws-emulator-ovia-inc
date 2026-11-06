// SPDX-License-Identifier: CC0-1.0

//! The TTL half of the SageMaker Feature Store hand-off: the CONTROL plane configures a
//! feature group's DEFAULT `TtlDuration`, and the FeatureStore Runtime DATA plane requires
//! an `EventTime` on every update of that group's records.
//!
//! RULE: a record's expiry is `EventTime + TtlDuration`, so an `UpdateRecord` made under a
//! TTL has to be told an event time to derive the record's expiry from — whether the TTL
//! is the request's own `TtlDuration` or the default one the group's `OnlineStoreConfig`
//! configures. An update under a TTL that carries no value for the group's
//! `EventTimeFeatureName` is refused with the modelled `ValidationError` (HTTP 400) and
//! changes nothing; one that carries it is applied.
//!
//! The runtime API carries no feature-group configuration — an `UpdateRecord` names a
//! group, a record and features — so the data plane can only know a group's default TTL
//! because the control plane recorded it in ONE shared directory
//! (`service_common::feature_store_groups`), on the create and on every update that
//! changes it. That is observable only where both halves run on the SAME router and the
//! SAME engine, which is what this suite drives, through the harness the hand-off suite
//! beside it declares.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sagemaker_feature_store_control_plane_handoff::{
    build_router, dispatch, feature, record, Answer,
};

/// The two feature names every group this suite creates fixes.
const IDENTIFIER_FEATURE: &str = "record_id";
const EVENT_TIME_FEATURE: &str = "event_time";

/// The event time the seeded record carries. Well in the future, so a `TtlDuration` the
/// row applies does not put the record's `ExpiresAt` in the past, which would evict it
/// before the row reads it back.
const EVENT_TIME: &str = "2035-01-01T00:00:00Z";

/// How the data plane states an update under a TTL that carries no `EventTime`: fixed
/// text, naming the two ways a TTL comes to be in force and nothing of the request or the
/// feature group.
const EVENT_TIME_REQUIRED: &str =
    "Validation Error: EventTime is required when TtlDuration is specified or when the feature \
     group has a default TTL configured. Include the EventTime feature in the Features list.";

/// `CreateFeatureGroup` on the control plane, with the `OnlineStoreConfig` the row states.
async fn create_group(router: &MultiServiceRouter, name: &str, online_store: Value) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        &[("x-amz-target", "SageMaker.CreateFeatureGroup")],
        Some(json!({
            "FeatureGroupName": name,
            "RecordIdentifierFeatureName": IDENTIFIER_FEATURE,
            "EventTimeFeatureName": EVENT_TIME_FEATURE,
            "FeatureDefinitions": [
                { "FeatureName": IDENTIFIER_FEATURE, "FeatureType": "String" },
                { "FeatureName": EVENT_TIME_FEATURE, "FeatureType": "Fractional" },
                { "FeatureName": "feature_a", "FeatureType": "String" }
            ],
            "OnlineStoreConfig": online_store
        })),
    )
    .await
}

/// `UpdateFeatureGroup` on the control plane, changing the group's online store
/// configuration.
async fn update_group(router: &MultiServiceRouter, name: &str, online_store: Value) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        &[("x-amz-target", "SageMaker.UpdateFeatureGroup")],
        Some(json!({
            "FeatureGroupName": name,
            "OnlineStoreConfig": online_store
        })),
    )
    .await
}

/// `UpdateRecord` on the data plane: the posted features, and the request's own
/// `TtlDuration` when the row states one.
async fn update_record(
    router: &MultiServiceRouter,
    group: &str,
    features: Value,
    ttl_duration: Option<Value>,
) -> Answer {
    let mut body = json!({
        "RecordIdentifierValueAsString": "R1",
        "Features": features
    });
    if let Some(ttl) = ttl_duration {
        body["TtlDuration"] = ttl;
    }
    dispatch(
        router,
        "POST",
        &format!("/FeatureGroup/{group}/Record"),
        &[],
        Some(body),
    )
    .await
}

/// The value `feature_a` holds in the record the data plane serves, and `""` when it
/// serves no record at all.
async fn served_feature_a(router: &MultiServiceRouter, group: &str) -> String {
    let got = dispatch(
        router,
        "GET",
        &format!("/FeatureGroup/{group}?RecordIdentifierValueAsString=R1"),
        &[],
        None,
    )
    .await;
    got.body
        .get("Record")
        .and_then(Value::as_array)
        .and_then(|features| {
            features
                .iter()
                .find(|held| held["FeatureName"] == json!("feature_a"))
        })
        .and_then(|held| held["ValueAsString"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// The message an answer carries, under either spelling: the framework's own refusal uses
/// `message`, a modelled error's `Message` member.
fn message(answer: &Answer) -> String {
    answer
        .body
        .get("message")
        .or_else(|| answer.body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn an_update_under_the_groups_default_ttl_requires_an_event_time() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // The class the rule spans: how the TTL governing the record comes about — the
    // group's own default, configured on the create; the request's `TtlDuration`; both; or
    // neither.
    //
    // (case, the group's name, its OnlineStoreConfig, the request's TtlDuration, expected
    //  status)
    let rows: Vec<(&str, &str, Value, Option<Value>, u16)> = vec![
        (
            "a group with a default TtlDuration in hours",
            "ttl-hours",
            json!({
                "EnableOnlineStore": true,
                "TtlDuration": { "Unit": "Hours", "Value": 1 }
            }),
            None,
            400,
        ),
        (
            "a group with a default TtlDuration in weeks",
            "ttl-weeks",
            json!({
                "EnableOnlineStore": true,
                "TtlDuration": { "Unit": "Weeks", "Value": 4 }
            }),
            None,
            400,
        ),
        (
            "a group with no default TtlDuration, and a TtlDuration on the request",
            "ttl-request",
            json!({ "EnableOnlineStore": true }),
            Some(json!({ "Unit": "Days", "Value": 2 })),
            400,
        ),
        (
            "a group with a default TtlDuration, and a TtlDuration on the request",
            "ttl-both",
            json!({
                "EnableOnlineStore": true,
                "TtlDuration": { "Unit": "Days", "Value": 1 }
            }),
            Some(json!({ "Unit": "Hours", "Value": 12 })),
            400,
        ),
        (
            "a group with no default TtlDuration, and none on the request",
            "ttl-none",
            json!({ "EnableOnlineStore": true }),
            None,
            200,
        ),
    ];

    for (case, group, online_store, ttl_duration, expected_status) in rows {
        let created = create_group(&router, group, online_store).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFeatureGroup 200, got {} with body {}",
            created.status, created.body
        );
        let put = dispatch(
            &router,
            "PUT",
            &format!("/FeatureGroup/{group}"),
            &[],
            Some(json!({ "Record": record("R1", EVENT_TIME, "original") })),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutRecord 200, got {} with body {}",
            put.status, put.body
        );

        // The update that carries no value for the group's `EventTimeFeatureName`.
        let answered = update_record(
            &router,
            group,
            json!([feature("feature_a", "no-event-time")]),
            ttl_duration.clone(),
        )
        .await;
        assert_eq!(
            answered.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for an update with no EventTime, \
             got {} with body {}",
            answered.status, answered.body
        );
        if expected_status == 400 {
            let stated = message(&answered);
            assert_eq!(
                (answered.error_code().as_str(), stated.as_str()),
                ("ValidationError", EVENT_TIME_REQUIRED),
                "case {case:?}: expected ValidationError stating {EVENT_TIME_REQUIRED:?}, got \
                 {:?} with {stated:?}",
                answered.error_code()
            );
            let served = served_feature_a(&router, group).await;
            assert_eq!(
                served, "original",
                "case {case:?}: expected the refused update to change nothing, got \
                 feature_a {served:?}"
            );
        }

        // The control, in every row: the SAME update carrying the group's event-time
        // feature is applied, so the refusal above is the missing EventTime and nothing
        // else.
        let applied = update_record(
            &router,
            group,
            json!([
                feature("feature_a", "with-event-time"),
                feature(EVENT_TIME_FEATURE, EVENT_TIME)
            ]),
            ttl_duration,
        )
        .await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: expected an update carrying the EventTime to be applied, got {} \
             with body {}",
            applied.status, applied.body
        );
        let served = served_feature_a(&router, group).await;
        assert_eq!(
            served, "with-event-time",
            "case {case:?}: expected the applied update to be readable, got feature_a \
             {served:?}"
        );
    }
}

#[tokio::test]
async fn removing_the_groups_default_ttl_lifts_the_event_time_requirement() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // A group created WITH a default TTL: the data plane requires an EventTime.
    create_group(
        &router,
        "ttl-lifted",
        json!({
            "EnableOnlineStore": true,
            "TtlDuration": { "Unit": "Hours", "Value": 6 }
        }),
    )
    .await;
    let put = dispatch(
        &router,
        "PUT",
        "/FeatureGroup/ttl-lifted",
        &[],
        Some(json!({ "Record": record("R1", EVENT_TIME, "original") })),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "case \"the seeding PutRecord\": expected 200, got {} with body {}",
        put.status, put.body
    );
    let refused = update_record(
        &router,
        "ttl-lifted",
        json!([feature("feature_a", "while-ttl-configured")]),
        None,
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "ValidationError"),
        "case \"while the default TTL stands\": expected HTTP 400 ValidationError, got {} {:?} \
         with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );

    // "To remove the default TtlDuration from an existing feature group, use the
    // UpdateFeatureGroup API and set the TtlDuration Unit and Value to null"
    // (`UpdateFeatureGroup`): the group no longer has a default TTL, so the update that
    // was refused is now applied.
    let updated = update_group(&router, "ttl-lifted", json!({ "TtlDuration": {} })).await;
    assert_eq!(
        updated.status, 200,
        "case \"UpdateFeatureGroup removing the default TtlDuration\": expected 200, got {} \
         with body {}",
        updated.status, updated.body
    );
    let applied = update_record(
        &router,
        "ttl-lifted",
        json!([feature("feature_a", "after-ttl-removed")]),
        None,
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "case \"after the default TTL is removed\": expected the update with no EventTime to \
         be applied, got {} with body {}",
        applied.status, applied.body
    );
    let served = served_feature_a(&router, "ttl-lifted").await;
    assert_eq!(
        served, "after-ttl-removed",
        "case \"after the default TTL is removed\": expected the applied update to be \
         readable, got feature_a {served:?}"
    );

    // And configuring one again puts the requirement back, so the directory follows the
    // control plane rather than being written once.
    let reconfigured = update_group(
        &router,
        "ttl-lifted",
        json!({ "TtlDuration": { "Unit": "Days", "Value": 3 } }),
    )
    .await;
    assert_eq!(
        reconfigured.status, 200,
        "case \"UpdateFeatureGroup configuring a default TtlDuration again\": expected 200, \
         got {} with body {}",
        reconfigured.status, reconfigured.body
    );
    let refused_again = update_record(
        &router,
        "ttl-lifted",
        json!([feature("feature_a", "after-ttl-restored")]),
        None,
    )
    .await;
    assert_eq!(
        (refused_again.status, refused_again.error_code().as_str()),
        (400, "ValidationError"),
        "case \"after the default TTL is configured again\": expected HTTP 400 \
         ValidationError, got {} {:?} with body {}",
        refused_again.status,
        refused_again.error_code(),
        refused_again.body
    );
}
