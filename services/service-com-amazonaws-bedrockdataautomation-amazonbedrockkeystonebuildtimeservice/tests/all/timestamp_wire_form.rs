// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's `date-time` members, the stage a create settles at, and
//! the projection a blueprint listing serves.
//!
//! THE RULES UNDER TEST, each asserted over a SET of inputs rather than over one request:
//!
//! 1. Every timestamp member of this service's responses targets the one `DateTimestamp`
//!    shape, which carries `@timestampFormat("date-time")`
//!    (`smithy_query(query="shape", shape="DateTimestamp")`), and the protocol is restJson1
//!    — so each one goes on the wire as an RFC 3339 date-time STRING, never as an epoch
//!    number. The cases below span every door that serves such a member and every member
//!    name it serves, so they are about the ENCODING and not about any one instant.
//! 2. `CreateBlueprintRequest$blueprintStage` is optional, so the service picks the stage
//!    when the request does not, and every later read reports the stage the blueprint
//!    holds. The rows span the absent case and both modelled values
//!    (`smithy_query(shape="BlueprintStage")` -> `["DEVELOPMENT", "LIVE"]`).
//! 3. `ListBlueprints` serves a NARROW item: the blueprint's identity, the version the item
//!    names, and its creation instant — not the members a `GetBlueprint` read serves (the
//!    stage, the last-modified instant, or the `@sensitive` `blueprintName`).

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::ops::types::{
    Blueprint, BlueprintSummary, DataAutomationProject, DataAutomationProjectSummary,
};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, string_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`), which is what `can_handle` resolves the route key from.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef"
        .to_string()
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// One answered object, or a panic naming the case: the member every door of this service
/// wraps its resource in is `@required`.
fn member(payload: &Value, name: &str, case: &str) -> Value {
    payload
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {name:?} in {payload:?}"))
}

/// Assert that one member is a modelled `date-time`: a JSON STRING that parses with
/// `Format::DateTime` and carries the UTC designator. A number here — the protocol default
/// for a member schema reporting no format — is exactly the defect these cases pin.
fn assert_date_time(object: &Value, name: &str, case: &str) -> DateTime {
    let raw = object
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: member {name:?} missing from {object:?}"));
    let text = raw.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: member {name:?} must be an RFC 3339 date-time STRING, \
             got {raw} in {object:?}"
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: member {name:?} = {text:?} is not a date-time: {error}")
    });
    assert!(
        text.ends_with('Z') && text.contains('T'),
        "case {case:?}: member {name:?} must be RFC 3339 in UTC, got {text:?}"
    );
    parsed
}

/// Every `Type` the model accepts (`smithy_query(shape="Blueprint")` -> `type` -> `Type`
/// -> `["AUDIO", "DOCUMENT", "IMAGE", "VIDEO"]`) crossed with the stage the request names:
/// absent, and each of the two modelled `BlueprintStage` values. The schemas differ in size
/// and shape so no row shares an input with another.
const STAGE_CASES: &[(&str, &str, Option<&str>, &str)] = &[
    ("audio-stage-absent", "AUDIO", None, r#"{"class":"call"}"#),
    ("document-stage-absent", "DOCUMENT", None, "{}"),
    (
        "document-development",
        "DOCUMENT",
        Some("DEVELOPMENT"),
        r#"{"class":"invoice","fields":[]}"#,
    ),
    ("image-live", "IMAGE", Some("LIVE"), r#"{"class":"receipt"}"#),
    (
        "video-development",
        "VIDEO",
        Some("DEVELOPMENT"),
        r#"{"class":"ad","nested":{"a":{"b":[1,2,3]}}}"#,
    ),
    (
        "video-live",
        "VIDEO",
        Some("LIVE"),
        r#"{"class":"ad","v":2}"#,
    ),
];

/// A blueprint created without `blueprintStage` settles at LIVE, one created with a stage
/// settles at that stage, and the stage every later read reports is the one the record
/// holds — not a stage the emulator substituted.
#[tokio::test]
async fn the_stage_a_create_settles_at_is_live_unless_the_request_names_one() {
    let router = emulator();

    for (case, kind, stage, schema) in STAGE_CASES {
        let mut body = json!({
            "blueprintName": format!("stage-{case}"),
            "type": kind,
            "schema": schema,
        });
        if let Some(stage) = stage {
            body["blueprintStage"] = json!(stage);
        }
        let (status, payload) = call(&router, "PUT", "/blueprints/", &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
        );
        let created = member(&payload, "blueprint", case);
        let expected = stage.unwrap_or("LIVE");
        assert_eq!(
            string_at(&created, "blueprintStage", case),
            expected,
            "case {case:?}: CreateBlueprint reported the wrong blueprintStage: {created:?}"
        );

        // The stage is PERSISTED, not echoed: the read-back reports it, and a read scoped
        // to the other modelled stage finds nothing.
        let arn = string_at(&created, "blueprintArn", case);
        let uri = format!("/blueprints/{}/", encode_label(&arn));
        let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBlueprint 200, got {status} with {payload:?}"
        );
        let fetched = member(&payload, "blueprint", case);
        assert_eq!(
            string_at(&fetched, "blueprintStage", case),
            expected,
            "case {case:?}: GetBlueprint reported another stage than the create: {fetched:?}"
        );

        let other = if expected == "LIVE" {
            "DEVELOPMENT"
        } else {
            "LIVE"
        };
        let (status, payload) =
            call(&router, "POST", &uri, &json!({ "blueprintStage": other })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a read scoped to {other:?} must be 404, got {status} with \
             {payload:?}"
        );
    }
}

/// Every blueprint door serves its two `date-time` members as RFC 3339 strings: the create,
/// the read, the version-scoped read, the update and the version cut. The rows differ in the
/// blueprint's type, stage and schema, so what is asserted is the ENCODING of the member and
/// not one recorded instant.
#[tokio::test]
async fn every_blueprint_door_serves_date_time_strings() {
    let router = emulator();

    for (case, kind, stage, schema) in STAGE_CASES {
        let mut body = json!({
            "blueprintName": format!("wire-{case}"),
            "type": kind,
            "schema": schema,
        });
        if let Some(stage) = stage {
            body["blueprintStage"] = json!(stage);
        }
        let (status, payload) = call(&router, "PUT", "/blueprints/", &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
        );
        let created = member(&payload, "blueprint", case);
        let created_at = assert_date_time(&created, "creationTime", &format!("create {case}"));
        assert_date_time(&created, "lastModifiedTime", &format!("create {case}"));
        let arn = string_at(&created, "blueprintArn", case);
        let uri = format!("/blueprints/{}/", encode_label(&arn));

        let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: expected GetBlueprint 200");
        let fetched = member(&payload, "blueprint", case);
        let read_at = assert_date_time(&fetched, "creationTime", &format!("read {case}"));
        assert_date_time(&fetched, "lastModifiedTime", &format!("read {case}"));
        assert_eq!(
            read_at.to_string(),
            created_at.to_string(),
            "case {case:?}: the read reported another creationTime than the create"
        );

        // The version cut: a 201 carrying the same projection, plus the version-scoped read
        // of the snapshot it took.
        let versions_uri = format!("/blueprints/{}/versions/", encode_label(&arn));
        let (status, payload) = call(&router, "POST", &versions_uri, &json!({})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateBlueprintVersion 201, got {status} with {payload:?}"
        );
        let cut = member(&payload, "blueprint", case);
        assert_date_time(&cut, "creationTime", &format!("version cut {case}"));
        assert_date_time(&cut, "lastModifiedTime", &format!("version cut {case}"));
        let version = string_at(&cut, "blueprintVersion", case);

        let (status, payload) =
            call(&router, "POST", &uri, &json!({ "blueprintVersion": version })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a version-scoped GetBlueprint 200, got {status}"
        );
        let pinned = member(&payload, "blueprint", case);
        assert_date_time(&pinned, "creationTime", &format!("pinned read {case}"));
        assert_date_time(&pinned, "lastModifiedTime", &format!("pinned read {case}"));

        // The update: `creationTime` stays put and `lastModifiedTime` moves, both as strings.
        let (status, payload) = call(
            &router,
            "PUT",
            &uri,
            &json!({ "schema": r#"{"class":"updated"}"# }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBlueprint 200, got {status} with {payload:?}"
        );
        let updated = member(&payload, "blueprint", case);
        let updated_created_at =
            assert_date_time(&updated, "creationTime", &format!("update {case}"));
        let modified_at =
            assert_date_time(&updated, "lastModifiedTime", &format!("update {case}"));
        assert_eq!(
            updated_created_at.to_string(),
            created_at.to_string(),
            "case {case:?}: an update must not move creationTime: {updated:?}"
        );
        assert!(
            modified_at.secs() >= created_at.secs(),
            "case {case:?}: lastModifiedTime {modified_at:?} precedes creationTime \
             {created_at:?}"
        );
    }
}

/// `ListBlueprints` serves the narrow list item, with its one timestamp as a date-time
/// string. The rows differ in how many blueprints the store holds, in whether a version has
/// been cut for the blueprint, and in the page size, because the projection is a property of
/// the item and not of the page it lands in.
#[tokio::test]
async fn a_blueprint_listing_serves_only_the_narrow_item() {
    // Source of the member set: the item a listing serves carries the blueprint's identity,
    // the version the item names and its creation instant. `blueprintStage`,
    // `lastModifiedTime` and the `@sensitive` `blueprintName`
    // (`smithy_query(query="shape", shape="BlueprintSummary")`) belong to the full
    // `Blueprint` a read serves.
    const WIDER: [&str; 3] = ["blueprintStage", "blueprintName", "lastModifiedTime"];

    let cases: &[(&str, usize, bool, Option<i64>)] = &[
        ("one, unversioned, whole page", 1, false, None),
        ("one, versioned, whole page", 1, true, None),
        ("three, unversioned, paged by one", 3, false, Some(1)),
        ("three, versioned, paged by two", 3, true, Some(2)),
        ("five, versioned, page at the count", 5, true, Some(5)),
    ];

    for (case, total, versioned, page_size) in cases {
        let router = emulator();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..*total {
            let (status, payload) = call(
                &router,
                "PUT",
                "/blueprints/",
                &json!({
                    "blueprintName": format!("list-item-bp-{index}"),
                    "type": "DOCUMENT",
                    "schema": format!("{{\"class\":\"c{index}\"}}"),
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
            );
            let arn = string_at(&member(&payload, "blueprint", case), "blueprintArn", case);
            if *versioned {
                let uri = format!("/blueprints/{}/versions/", encode_label(&arn));
                let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected CreateBlueprintVersion 201, got {status} with \
                     {payload:?}"
                );
            }
            created.insert(arn);
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["maxResults"] = json!(size);
            }
            if let Some(ref token) = token {
                body["nextToken"] = json!(token);
            }
            let (status, payload) = call(&router, "POST", "/blueprints/", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListBlueprints 200, got {status} with {payload:?}"
            );
            let page = payload["blueprints"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: blueprints is not a list: {payload:?}"))
                .clone();
            for item in &page {
                let object = item
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case:?}: a list item is not an object: {item}"));
                let members: BTreeSet<&str> = object.keys().map(String::as_str).collect();
                let mut expected: BTreeSet<&str> = ["blueprintArn", "creationTime"].into();
                if *versioned {
                    expected.insert("blueprintVersion");
                }
                assert_eq!(
                    members, expected,
                    "case {case:?}: the list item carries the wrong member set: {item}"
                );
                for wider in WIDER {
                    assert!(
                        object.get(wider).is_none(),
                        "case {case:?}: a list item must not carry {wider:?}: {item}"
                    );
                }
                assert_date_time(item, "creationTime", case);
                seen.insert(string_at(item, "blueprintArn", case));
            }
            token = payload
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, created,
            "case {case:?}: the listing did not report every stored blueprint"
        );
    }
}

/// The project doors serve their `date-time` members the same way, because the rule is the
/// `DateTimestamp` shape's and not one shape's: the create, the read and the list item. The
/// rows differ in the project's stage, its type and the configuration blocks it carries.
#[tokio::test]
async fn every_project_door_serves_date_time_strings() {
    let router = emulator();

    // `DataAutomationProjectStage` -> `["DEVELOPMENT", "LIVE"]` and
    // `DataAutomationProjectType` is server-chosen, so the rows vary the stage the request
    // names (including the absent case) and the output configuration it asks for.
    let cases: &[(&str, Option<&str>, Value)] = &[
        (
            "stage-absent-standard-only",
            None,
            json!({"standardOutputConfiguration": {}}),
        ),
        (
            "development-with-description",
            Some("DEVELOPMENT"),
            json!({
                "standardOutputConfiguration": {},
                "projectDescription": "a project the listing must report",
            }),
        ),
        (
            "live-standard-only",
            Some("LIVE"),
            json!({"standardOutputConfiguration": {}}),
        ),
    ];

    let mut arns: BTreeSet<String> = BTreeSet::new();
    for (case, stage, extra) in cases {
        let mut body = json!({ "projectName": format!("wire-project-{case}") });
        if let Some(stage) = stage {
            body["projectStage"] = json!(stage);
        }
        for (key, value) in extra.as_object().expect("an object of extra members") {
            body[key] = value.clone();
        }

        let (status, payload) = call(&router, "PUT", "/data-automation-projects/", &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateDataAutomationProject 201, got {status} with \
             {payload:?}"
        );
        // `CreateDataAutomationProjectResponse` carries no timestamp of its own — its
        // members are `projectArn`, `projectStage` and `status`
        // (`smithy_query(query="operation", operation="CreateDataAutomationProject")`), so
        // the project's two `date-time` members are reached through the read below.
        let arn = string_at(&payload, "projectArn", case);

        let uri = format!("/data-automation-projects/{}/", encode_label(&arn));
        let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDataAutomationProject 200, got {status} with {payload:?}"
        );
        let fetched = member(&payload, "project", case);
        let created_at = assert_date_time(&fetched, "creationTime", &format!("read {case}"));
        assert_date_time(&fetched, "lastModifiedTime", &format!("read {case}"));

        // The instants are PERSISTED, so a second read reports the same creationTime rather
        // than a fresh clock reading.
        let (status, payload) = call(&router, "POST", &uri, &json!({})).await;
        assert_eq!(status, 200, "case {case:?}: expected a second read 200, got {status}");
        let again = member(&payload, "project", case);
        let read_at = assert_date_time(&again, "creationTime", &format!("re-read {case}"));
        assert_eq!(
            read_at.to_string(),
            created_at.to_string(),
            "case {case:?}: two reads reported different creationTimes"
        );
        arns.insert(arn);
    }

    let (status, payload) = call(&router, "POST", "/data-automation-projects/", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"listing\": expected ListDataAutomationProjects 200, got {status} with {payload:?}"
    );
    let listed = payload["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"listing\": projects is not a list: {payload:?}"));
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for item in listed {
        assert_date_time(item, "creationTime", "listing");
        seen.insert(string_at(item, "projectArn", "listing"));
    }
    assert_eq!(
        seen, arns,
        "case \"listing\": the listing did not report every stored project"
    );
}

/// The member schema the JSON codec reads the wire form off: every member of this service's
/// served shapes that targets `DateTimestamp` reports `date-time`, and a member the model
/// leaves alone reports no format at all.
///
/// This is what fails if any of these schemas goes back to the vendored codegen's output,
/// which drops the trait (see `src/ops/types/date_time_wire.rs`).
#[test]
fn every_modelled_date_time_member_schema_carries_the_date_time_format() {
    // Source: every member of a served shape of this service targeting
    // `com.amazonaws.bedrockdataautomation#DateTimestamp`, from
    // `smithy_query(query="shape", shape="Blueprint,BlueprintSummary,DataAutomationProject,
    // DataAutomationProjectSummary")`.
    let date_time_members: Vec<(&str, &Schema)> = vec![
        ("Blueprint$creationTime", &Blueprint::M_CREATION_TIME),
        (
            "Blueprint$lastModifiedTime",
            &Blueprint::M_LAST_MODIFIED_TIME,
        ),
        (
            "Blueprint$optimizationTime",
            &Blueprint::M_OPTIMIZATION_TIME,
        ),
        (
            "BlueprintSummary$creationTime",
            &BlueprintSummary::M_CREATION_TIME,
        ),
        (
            "BlueprintSummary$lastModifiedTime",
            &BlueprintSummary::M_LAST_MODIFIED_TIME,
        ),
        (
            "DataAutomationProject$creationTime",
            &DataAutomationProject::M_CREATION_TIME,
        ),
        (
            "DataAutomationProject$lastModifiedTime",
            &DataAutomationProject::M_LAST_MODIFIED_TIME,
        ),
        (
            "DataAutomationProjectSummary$creationTime",
            &DataAutomationProjectSummary::M_CREATION_TIME,
        ),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // A member the model annotates with no format keeps the protocol default, so the fix put
    // no format where the model names none.
    for (case, schema) in [
        ("Blueprint$blueprintArn", &Blueprint::M_BLUEPRINT_ARN),
        ("Blueprint$type", &Blueprint::M_TYPE),
        (
            "BlueprintSummary$blueprintVersion",
            &BlueprintSummary::M_BLUEPRINT_VERSION,
        ),
        ("DataAutomationProject$status", &DataAutomationProject::M_STATUS),
        (
            "DataAutomationProjectSummary$projectName",
            &DataAutomationProjectSummary::M_PROJECT_NAME,
        ),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model annotates no format for this member, got {format:?}"
        );
    }
}

/// The hand-written member lists cannot drift from the model: every member schema reports the
/// wire name and the member index the model gives it, in the order the shape declares.
///
/// `member_index` is what the deserializer dispatches on, so a wrong index silently reads one
/// member into another's field.
#[test]
fn every_hand_written_member_list_matches_the_models_order() {
    // Source: `smithy_query(query="shape", shape="Blueprint,BlueprintSummary,
    // DataAutomationProject,DataAutomationProjectSummary")`, whose member order is the order
    // below.
    let shapes: Vec<(&str, &Schema, Vec<&str>)> = vec![
        (
            "Blueprint",
            &Blueprint::SCHEMA,
            vec![
                "blueprintArn",
                "schema",
                "type",
                "creationTime",
                "lastModifiedTime",
                "blueprintName",
                "blueprintVersion",
                "blueprintStage",
                "kmsKeyId",
                "kmsEncryptionContext",
                "optimizationSamples",
                "optimizationTime",
            ],
        ),
        (
            "BlueprintSummary",
            &BlueprintSummary::SCHEMA,
            vec![
                "blueprintArn",
                "blueprintVersion",
                "blueprintStage",
                "blueprintName",
                "creationTime",
                "lastModifiedTime",
            ],
        ),
        (
            "DataAutomationProject",
            &DataAutomationProject::SCHEMA,
            vec![
                "projectArn",
                "creationTime",
                "lastModifiedTime",
                "projectName",
                "projectStage",
                "projectType",
                "projectDescription",
                "standardOutputConfiguration",
                "customOutputConfiguration",
                "overrideConfiguration",
                "dataAutomationLibraryConfiguration",
                "status",
                "kmsKeyId",
                "kmsEncryptionContext",
            ],
        ),
        (
            "DataAutomationProjectSummary",
            &DataAutomationProjectSummary::SCHEMA,
            vec![
                "projectArn",
                "projectStage",
                "projectType",
                "projectName",
                "creationTime",
            ],
        ),
    ];

    for (case, schema, expected) in shapes {
        let members = schema.members();
        let names: Vec<&str> = members
            .iter()
            .map(|member| member.member_name().unwrap_or("<unnamed>"))
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: the member list is not the model's, in the model's order"
        );
        for (index, member) in members.iter().enumerate() {
            assert_eq!(
                member.member_index(),
                Some(index),
                "case {case:?}: member {:?} reports the wrong member_index",
                member.member_name()
            );
        }
    }
}
