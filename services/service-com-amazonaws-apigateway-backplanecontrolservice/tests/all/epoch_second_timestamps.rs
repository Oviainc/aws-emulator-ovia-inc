// SPDX-License-Identifier: CC0-1.0

//! The serialized FORM of every timestamp this service reports: a whole-second epoch
//! number.
//!
//! Every `Timestamp` member of this surface — `ApiKey$createdDate`,
//! `ApiKey$lastUpdatedDate` and `RestApi$createdDate`
//! (`smithy_query(query="shape", shape="ApiKey,RestApi",
//! model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/\
//! smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")`: each targets
//! `Timestamp`) — travels on this restJson1 surface as an epoch-seconds number, and AWS
//! reports a WHOLE number of seconds. A clock read kept at its native resolution
//! serializes as a fractional epoch value (`1792678925.0145848`), which a client parsing
//! the member as integer epoch seconds rejects or silently truncates.
//!
//! The rule is asserted over a SET of inputs rather than one call: every operation of
//! this service that reports a timestamp, every resource kind that carries one, at
//! create time and on every later read, with the members found by scanning each response
//! body's own members rather than by naming them one at a time — so a member added later
//! is covered too.
//!
//! [`Gateway`] and friends are this crate's one wire-test harness, reused from
//! `rest_api_lifecycle` rather than standing a second emulator up another way.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, string_member, Gateway};

/// The earliest instant a value reported by a running emulator can name, as a number of
/// SECONDS since the epoch: 2020-09-13T12:26:40Z. It is a sanity floor on the UNIT, not
/// on the instant — the same instant in milliseconds is a thousand times larger, so a
/// value below this floor is a value in the wrong unit (or no epoch at all), and one
/// above it is seconds.
const EPOCH_SECONDS_FLOOR: i64 = 1_600_000_000;

/// Every TOP-LEVEL member of `body` whose name marks it a timestamp of this surface
/// (`createdDate`, `lastUpdatedDate`, `certificateUploadDate`, `expirationDate`, …), with
/// its value.
///
/// Found by scanning the body's own members rather than by naming them, so a timestamp
/// member this suite does not know about is still checked. A nested body (a listing's
/// items) is checked by calling this per item.
fn date_members(body: &Value) -> Vec<(String, Value)> {
    body.as_object()
        .map(|members| {
            members
                .iter()
                .filter(|(name, _)| name.ends_with("Date"))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert every timestamp member of `body` is a WHOLE-second epoch number, and that
/// `expected` are all present.
fn assert_epoch_seconds(body: &Value, expected: &[&str], case: &str) {
    let found = date_members(body);
    for member in expected {
        assert!(
            found.iter().any(|(name, _)| name == member),
            "case {case}: expected the timestamp member {member}, got {body:?}"
        );
    }
    for (member, value) in &found {
        // An integer-valued JSON number: `1792678925`, never `1792678925.0145848` and
        // never `1792678925.0` — both of which parse as a float here and neither of
        // which is what AWS writes.
        let seconds = value.as_i64().unwrap_or_else(|| {
            panic!(
                "case {case}: {member} must be a whole-second epoch number, got {value:?}"
            )
        });
        assert!(
            seconds >= EPOCH_SECONDS_FLOOR,
            "case {case}: {member} must be a number of SECONDS since the epoch, \
             got {seconds} (below {EPOCH_SECONDS_FLOOR})"
        );
    }
}

#[tokio::test]
async fn every_timestamp_an_api_key_reports_is_a_whole_second_epoch_number() {
    // Rows differ along what the rule generalises over: which members the request
    // carries, so nothing about the request shape can change the timestamp's form.
    let cases: Vec<(&str, Value)> = vec![
        ("every member absent", json!({})),
        ("named and enabled", json!({ "name": "clock-1", "enabled": true })),
        (
            "with tags and a customer",
            json!({ "name": "clock-2", "customerId": "c-1", "tags": { "env": "test" } }),
        ),
        (
            "with a caller-supplied value",
            json!({ "name": "clock-3", "value": "k".repeat(20) }),
        ),
    ];

    let gateway = Gateway::new();
    for (case, request) in &cases {
        let created = gateway.call("POST", "/apikeys", Some(request.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            created.status, created.body
        );
        assert_epoch_seconds(
            &created.body,
            &["createdDate", "lastUpdatedDate"],
            &format!("CreateApiKey({case})"),
        );

        // The stored instant is what every later read reports: the same whole-second
        // value, on each read, whether or not the value is asked for.
        let id = string_member(&created.body, "id", case);
        for (round, include_value) in [(0, false), (1, true), (2, false)] {
            let uri = if include_value {
                format!("/apikeys/{id}?includeValue=true")
            } else {
                format!("/apikeys/{id}")
            };
            let read = gateway.call("GET", &uri, None).await;
            assert_eq!(
                read.status, 200,
                "case {case} round {round}: expected 200, got {} with {:?}",
                read.status, read.body
            );
            let read_case = format!("GetApiKey({case}, round {round})");
            assert_epoch_seconds(&read.body, &["createdDate", "lastUpdatedDate"], &read_case);
            for member in ["createdDate", "lastUpdatedDate"] {
                assert_eq!(
                    read.body[member], created.body[member],
                    "case {read_case}: {member} must report the stored instant, \
                     got {:?} after {:?}",
                    read.body[member], created.body[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn every_timestamp_a_rest_api_reports_is_a_whole_second_epoch_number() {
    // Rows differ in the request's shape, as above.
    let cases: Vec<(&str, Value)> = vec![
        ("name only", json!({ "name": "clock-api-1" })),
        (
            "described and versioned",
            json!({ "name": "clock-api-2", "description": "d", "version": "v1" }),
        ),
        (
            "with an endpoint configuration",
            json!({ "name": "clock-api-3", "endpointConfiguration": { "types": ["REGIONAL"] } }),
        ),
    ];

    let gateway = Gateway::new();
    let mut ids: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let created = gateway.call("POST", "/restapis", Some(request.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            created.status, created.body
        );
        assert_epoch_seconds(&created.body, &["createdDate"], &format!("CreateRestApi({case})"));

        let id = string_member(&created.body, "id", case);
        let read = gateway.call("GET", &format!("/restapis/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetRestApi, got {} with {:?}",
            read.status, read.body
        );
        assert_epoch_seconds(&read.body, &["createdDate"], &format!("GetRestApi({case})"));
        assert_eq!(
            read.body["createdDate"], created.body["createdDate"],
            "case GetRestApi({case}): createdDate must report the stored instant, \
             got {:?} after {:?}",
            read.body["createdDate"], created.body["createdDate"]
        );
        ids.push(id);
    }

    // And in a LISTING, where the same records are rendered by a different operation.
    let listed = gateway.call("GET", "/restapis", None).await;
    assert_eq!(
        listed.status, 200,
        "case GetRestApis: expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    let items = listed.body["item"]
        .as_array()
        .unwrap_or_else(|| panic!("case GetRestApis: expected an item array, got {:?}", listed.body))
        .clone();
    assert_eq!(
        items.len(),
        ids.len(),
        "case GetRestApis: expected every created api, got {items:?}"
    );
    for (index, item) in items.iter().enumerate() {
        assert_epoch_seconds(item, &["createdDate"], &format!("GetRestApis item {index}"));
    }
}

#[tokio::test]
async fn every_timestamp_a_deployment_or_stage_reports_is_a_whole_second_epoch_number() {
    // The same rule at the other two resource kinds this service stores an instant for:
    // `Deployment$createdDate`, and `Stage$createdDate` / `$lastUpdatedDate`. Rows differ
    // in the stage the deployment is deployed to and in whether the request describes it,
    // so nothing about the request shape can change the form of the instant.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("deployed to prod", "prod", json!({ "stageName": "prod" })),
        (
            "deployed to beta with a description",
            "beta",
            json!({ "stageName": "beta", "description": "the beta stage" }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "clock-deployments").await;
    for (case, stage_name, request) in &cases {
        let created = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/deployments"),
                Some(request.clone()),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
            created.status, created.body
        );
        assert_epoch_seconds(
            &created.body,
            &["createdDate"],
            &format!("CreateDeployment({case})"),
        );

        let deployment = string_member(&created.body, "id", case);
        let read = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/deployments/{deployment}"),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetDeployment, got {} with {:?}",
            read.status, read.body
        );
        assert_epoch_seconds(
            &read.body,
            &["createdDate"],
            &format!("GetDeployment({case})"),
        );
        assert_eq!(
            read.body["createdDate"], created.body["createdDate"],
            "case GetDeployment({case}): createdDate must report the stored instant, \
             got {:?} after {:?}",
            read.body["createdDate"], created.body["createdDate"]
        );

        // The stage the same call created carries two instants, both read back here.
        let stage = gateway
            .call("GET", &format!("/restapis/{api}/stages/{stage_name}"), None)
            .await;
        assert_eq!(
            stage.status, 200,
            "case {case}: expected 200 from GetStage, got {} with {:?}",
            stage.status, stage.body
        );
        assert_epoch_seconds(
            &stage.body,
            &["createdDate", "lastUpdatedDate"],
            &format!("GetStage({case})"),
        );
    }
}
