// SPDX-License-Identifier: CC0-1.0

//! `CreateServiceEnvironment` and the reader that proves it persisted, over the wire.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a create persists every member the request carried and `DescribeServiceEnvironments`
//!   reads it back unchanged — the name, the type, the state, the capacity limits and the
//!   tags;
//! * the name is the environment's identity: the ARN is
//!   `arn:aws:batch:<region>:<account>:service-environment/<name>`, minted once and
//!   reported identically by every later read, and re-creating a name is refused with the
//!   modelled `ClientException` (HTTP 400) instead of overwriting;
//! * `state` defaults to `ENABLED` when the request names none, and is echoed when it
//!   does;
//! * the environment is returned in the non-terminal `CREATING` status and the CLOCK — not
//!   a read — moves it to `VALID`; two reads at the same instant answer identically;
//! * `serviceEnvironmentName` is accepted only in the charset and length the member
//!   documents, and refused with a `ClientException` outside it;
//! * `DescribeServiceEnvironments` resolves each entry by name OR by ARN, collapses
//!   duplicates, skips an entry it holds nothing for, and pages with
//!   `maxResults`/`nextToken` so a caller that pages to the end sees every item exactly
//!   once.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, Batch};

/// The only `ServiceEnvironmentType` the model declares. Source: the model's
/// `com.amazonaws.batch#ServiceEnvironmentType` enum
/// (`smithy_query(query="shape", shape="ServiceEnvironmentType")`).
const ENVIRONMENT_TYPE: &str = "SAGEMAKER_TRAINING";

/// Every `ServiceEnvironmentState` the model declares. Source: the model's
/// `com.amazonaws.batch#ServiceEnvironmentState` enum
/// (`smithy_query(query="shape", shape="ServiceEnvironmentState")`).
const STATES: &[&str] = &["ENABLED", "DISABLED"];

/// One create request and what it must read back.
#[derive(Debug, Clone)]
struct Case {
    /// What the row varies: named in every assertion.
    label: &'static str,
    name: String,
    /// `state` as the request carries it, or `None` to leave the member out.
    state: Option<&'static str>,
    /// The `capacityLimits` list, which differs in length and in `capacityUnit`.
    capacity_limits: Value,
    /// `tags`, or `None` to leave the member out.
    tags: Option<Value>,
}

/// The create body for `case`, with every member the row does not carry left out.
fn create_body(case: &Case) -> Value {
    let mut body = json!({
        "serviceEnvironmentName": case.name,
        "serviceEnvironmentType": ENVIRONMENT_TYPE,
        "capacityLimits": case.capacity_limits,
    });
    if let Some(state) = case.state {
        body["state"] = json!(state);
    }
    if let Some(tags) = case.tags.as_ref() {
        body["tags"] = tags.clone();
    }
    body
}

/// The table: rows differ along what the rules generalise over — whether `state` is given
/// and which value, how many capacity limits and in what unit (`NUM_INSTANCES` or a
/// SageMaker instance type, the two forms `CapacityLimit$capacityUnit` documents), and
/// whether tags are carried.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "state absent, one NUM_INSTANCES limit, no tags",
            name: "se-default-state".to_string(),
            state: None,
            capacity_limits: json!([{"maxCapacity": 10, "capacityUnit": "NUM_INSTANCES"}]),
            tags: None,
        },
        Case {
            label: "state ENABLED, one instance-type limit, one tag",
            name: "se_enabled_1".to_string(),
            state: Some("ENABLED"),
            capacity_limits: json!([{"maxCapacity": 5, "capacityUnit": "ml.m5.large"}]),
            tags: Some(json!({"team": "research"})),
        },
        Case {
            label: "state DISABLED, three limits of mixed units, two tags",
            name: "SE-Disabled-2".to_string(),
            state: Some("DISABLED"),
            capacity_limits: json!([
                {"maxCapacity": 1, "capacityUnit": "ml.m5.large"},
                {"maxCapacity": 2, "capacityUnit": "ml.p4d.24xlarge"},
                {"maxCapacity": 50, "capacityUnit": "NUM_INSTANCES"},
            ]),
            tags: Some(json!({"team": "research", "cost-centre": "42"})),
        },
        Case {
            label: "capacity limit carrying only maxCapacity",
            name: "se-unitless".to_string(),
            state: Some("ENABLED"),
            capacity_limits: json!([{"maxCapacity": 7}]),
            tags: None,
        },
        Case {
            label: "longest accepted name",
            name: "a".repeat(state::MAX_SERVICE_ENVIRONMENT_NAME_LEN),
            state: None,
            capacity_limits: json!([{"maxCapacity": 3, "capacityUnit": "NUM_INSTANCES"}]),
            tags: None,
        },
    ]
}

/// The one `ServiceEnvironmentDetail` for `reference` (a name or an ARN).
async fn describe_one(batch: &Batch, reference: &str) -> Value {
    let body = batch
        .ok(
            "/v1/describeserviceenvironments",
            json!({"serviceEnvironments": [reference]}),
        )
        .await;
    let found = body["serviceEnvironments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        found.len(),
        1,
        "case {reference:?}: expected exactly one serviceEnvironment, got {found:?}"
    );
    found[0].clone()
}

/// How long to wait for the `CREATING` -> `VALID` move to fall due: the span the clock
/// owes it ([`state::JOB_PHASE_MILLIS`]), with margin for a slow machine.
async fn wait_for_valid() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    let batch = Batch::new();

    for case in cases() {
        let created = batch
            .ok("/v1/createserviceenvironment", create_body(&case))
            .await;
        let expected_arn = arn(&format!("service-environment/{}", case.name));

        assert_eq!(
            created["serviceEnvironmentName"].as_str(),
            Some(case.name.as_str()),
            "case {:?}: expected the created name back, got {created}",
            case.label
        );
        assert_eq!(
            created["serviceEnvironmentArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: expected ARN {expected_arn}, got {created}",
            case.label
        );

        let detail = describe_one(&batch, &case.name).await;
        assert_eq!(
            detail["serviceEnvironmentArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: the read-back ARN must be the minted one, got {detail}",
            case.label
        );
        assert_eq!(
            detail["serviceEnvironmentType"].as_str(),
            Some(ENVIRONMENT_TYPE),
            "case {:?}: expected type {ENVIRONMENT_TYPE}, got {detail}",
            case.label
        );
        // "The default value is ENABLED" (`CreateServiceEnvironment$state`).
        assert_eq!(
            detail["state"].as_str(),
            Some(case.state.unwrap_or("ENABLED")),
            "case {:?}: expected state {:?}, got {detail}",
            case.label,
            case.state.unwrap_or("ENABLED")
        );
        assert_eq!(
            detail["capacityLimits"],
            case.capacity_limits,
            "case {:?}: expected the submitted capacityLimits back, got {detail}",
            case.label
        );
        match case.tags.as_ref() {
            Some(tags) => assert_eq!(
                &detail["tags"],
                tags,
                "case {:?}: expected the submitted tags back, got {detail}",
                case.label
            ),
            None => assert!(
                detail["tags"].is_null(),
                "case {:?}: a create with no tags must report none, got {detail}",
                case.label
            ),
        }
    }
}

#[tokio::test]
async fn recreating_a_name_is_refused_and_does_not_overwrite() {
    let batch = Batch::new();

    for case in cases() {
        batch
            .ok("/v1/createserviceenvironment", create_body(&case))
            .await;

        // A second create of the same NAME, carrying different members: it must be
        // refused, and the stored record must be untouched.
        let clashing = Case {
            state: Some(match case.state {
                Some("DISABLED") => "ENABLED",
                _ => "DISABLED",
            }),
            capacity_limits: json!([{"maxCapacity": 999, "capacityUnit": "NUM_INSTANCES"}]),
            tags: Some(json!({"overwritten": "yes"})),
            ..case.clone()
        };
        let (status, body) = batch
            .call("/v1/createserviceenvironment", create_body(&clashing))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: re-creating a name must answer 400, got {status} {body}",
            case.label
        );

        let detail = describe_one(&batch, &case.name).await;
        assert_eq!(
            detail["capacityLimits"],
            case.capacity_limits,
            "case {:?}: the refused create must not overwrite, got {detail}",
            case.label
        );
        assert_eq!(
            detail["state"].as_str(),
            Some(case.state.unwrap_or("ENABLED")),
            "case {:?}: the refused create must not overwrite state, got {detail}",
            case.label
        );
    }
}

#[tokio::test]
async fn status_starts_creating_and_only_the_clock_moves_it() {
    let batch = Batch::new();

    // One row per state, so the clock-owed move is asserted for both — a DISABLED
    // environment is created just as a non-terminal one.
    for state_value in STATES {
        let name = format!("se-status-{}", state_value.to_lowercase());
        batch
            .ok(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": name,
                    "serviceEnvironmentType": ENVIRONMENT_TYPE,
                    "state": state_value,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;

        // The span is never zero, so a caller reading straight after the create still
        // observes the transient status.
        let first = describe_one(&batch, &name).await;
        assert_eq!(
            first["status"].as_str(),
            Some("CREATING"),
            "case {state_value:?}: a fresh environment must report CREATING, got {first}"
        );
        // A READ does not move it: the second read at the same instant answers the same.
        let second = describe_one(&batch, &name).await;
        assert_eq!(
            second["status"].as_str(),
            first["status"].as_str(),
            "case {state_value:?}: two reads at the same instant must agree, got {second}"
        );

        wait_for_valid().await;

        let settled = describe_one(&batch, &name).await;
        assert_eq!(
            settled["status"].as_str(),
            Some("VALID"),
            "case {state_value:?}: the clock must move CREATING -> VALID, got {settled}"
        );
        assert_eq!(
            settled["state"].as_str(),
            Some(*state_value),
            "case {state_value:?}: the clock must not touch state, got {settled}"
        );
    }
}

#[tokio::test]
async fn a_name_outside_the_documented_charset_is_refused() {
    let batch = Batch::new();

    // `serviceEnvironmentName`'s doc: "It can be up to 128 characters long and can
    // contain letters, numbers, hyphens (-), and underscores (_)." The accepted rows
    // cover each published form including the length bound; the refused rows are one
    // past it and the characters outside the charset.
    let accepted: &[(&str, String)] = &[
        ("letters", "env".to_string()),
        ("digits", "env2024".to_string()),
        ("hyphen", "my-env".to_string()),
        ("underscore", "my_env".to_string()),
        ("mixed case", "MyEnv-01_x".to_string()),
        ("single character", "e".to_string()),
        (
            "at the length bound",
            "b".repeat(state::MAX_SERVICE_ENVIRONMENT_NAME_LEN),
        ),
    ];
    let refused: &[(&str, String)] = &[
        ("empty", String::new()),
        (
            "one past the length bound",
            "c".repeat(state::MAX_SERVICE_ENVIRONMENT_NAME_LEN + 1),
        ),
        ("space", "my env".to_string()),
        ("dot", "my.env".to_string()),
        ("slash", "my/env".to_string()),
        ("colon", "my:env".to_string()),
        ("non-ASCII letter", "env-é".to_string()),
    ];

    for (label, name) in accepted {
        let body = batch
            .ok(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": name,
                    "serviceEnvironmentType": ENVIRONMENT_TYPE,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;
        assert_eq!(
            body["serviceEnvironmentName"].as_str(),
            Some(name.as_str()),
            "case {label:?}: a documented name must be accepted, got {body}"
        );
    }

    for (label, name) in refused {
        let (status, body) = batch
            .call(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": name,
                    "serviceEnvironmentType": ENVIRONMENT_TYPE,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: a name outside the documented charset must answer 400, got {status} {body}"
        );
        // Nothing was stored for it.
        let found = batch
            .ok(
                "/v1/describeserviceenvironments",
                json!({"serviceEnvironments": [name]}),
            )
            .await;
        assert_eq!(
            found["serviceEnvironments"],
            json!([]),
            "case {label:?}: a refused create must persist nothing, got {found}"
        );
    }
}

#[tokio::test]
async fn describe_resolves_by_name_or_arn_and_pages_over_everything() {
    let batch = Batch::new();

    let names: Vec<String> = (0..7).map(|index| format!("se-page-{index}")).collect();
    for name in &names {
        batch
            .ok(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": name,
                    "serviceEnvironmentType": ENVIRONMENT_TYPE,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;
    }

    // Every "name or ARN" entry resolves both ways, and an entry the service holds
    // nothing for is simply not reported.
    let arn_of = |name: &str| arn(&format!("service-environment/{name}"));
    let reference_rows: &[(&str, Vec<String>, usize)] = &[
        ("by name", vec![names[0].clone()], 1),
        ("by ARN", vec![arn_of(&names[0])], 1),
        (
            "name and ARN of the SAME environment collapse",
            vec![names[1].clone(), arn_of(&names[1])],
            1,
        ),
        (
            "mixed names and ARNs",
            vec![names[2].clone(), arn_of(&names[3]), names[4].clone()],
            3,
        ),
        ("unknown name is skipped", vec!["se-absent".to_string()], 0),
        (
            "unknown ARN beside a known name",
            vec![arn_of("se-absent"), names[5].clone()],
            1,
        ),
    ];
    for (label, references, expected) in reference_rows {
        let body = batch
            .ok(
                "/v1/describeserviceenvironments",
                json!({"serviceEnvironments": references}),
            )
            .await;
        let found = body["serviceEnvironments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            found.len(),
            *expected,
            "case {label:?}: expected {expected} environment(s), got {found:?}"
        );
    }

    // Paging over EVERY environment sees each exactly once, at each page size.
    for page_size in [1_usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..(names.len() + 2) {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let body = batch.ok("/v1/describeserviceenvironments", request).await;
            let page = body["serviceEnvironments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must hold at most {page_size}, got {}",
                page.len()
            );
            for detail in page {
                seen.push(
                    detail["serviceEnvironmentName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case page size {page_size}: paging must terminate, still holding {token:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "case page size {page_size}: expected {} distinct environments, got {seen:?}",
            names.len()
        );
        assert_eq!(
            seen.len(),
            names.len(),
            "case page size {page_size}: every environment exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_store_is_bounded_and_a_create_past_the_cap_is_refused() {
    let batch = Batch::new();

    for index in 0..state::limits::MAX_SERVICE_ENVIRONMENTS {
        let (status, body) = batch
            .call(
                "/v1/createserviceenvironment",
                json!({
                    "serviceEnvironmentName": format!("se-cap-{index}"),
                    "serviceEnvironmentType": ENVIRONMENT_TYPE,
                    "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {index}: a create inside the cap must succeed, got {status} {body}"
        );
    }

    let (status, body) = batch
        .call(
            "/v1/createserviceenvironment",
            json!({
                "serviceEnvironmentName": "se-cap-over",
                "serviceEnvironmentType": ENVIRONMENT_TYPE,
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "NUM_INSTANCES"}],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case past the cap: expected 400, got {status} {body}"
    );
}
