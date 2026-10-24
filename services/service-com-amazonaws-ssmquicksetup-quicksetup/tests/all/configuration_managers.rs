// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the configuration-manager slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the persisted
//! configuration-manager and configuration records, so whatever a create or an update
//! stored is what a later Get or List reports — including the ids the service assigned
//! — and anything never stored (or since deleted) is the modelled 404.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::ops::types::{
    STATUS_TYPE_VALUES, STATUS_VALUES,
};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    MAX_FILTERS, MAX_FILTER_VALUES, MAX_ITEMS_LIMIT, STATUS_INITIALIZING, STATUS_SUCCEEDED,
    STATUS_TYPE_DEPLOYMENT, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; every ARN must carry exactly this one.
const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A well-formed `ManagerArn` (the modelled pattern ends in a uuid) that no create
/// ever minted, for the not-found cases.
const UNKNOWN_MANAGER_ARN: &str =
    "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/\
     00000000-0000-4000-8000-000000000000";

/// Long enough that the clock-owed deployment transition has come due.
fn past_transition() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes
/// on and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/configurationManager", Some(body)).await
}

async fn get(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await
}

async fn update(router: &MultiServiceRouter, manager_arn: &str, body: &Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/configurationManager/{}", label(manager_arn)),
        Some(body),
    )
    .await
}

async fn delete(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await
}

async fn list_managers(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listConfigurationManagers", Some(body)).await
}

async fn list_configurations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listConfigurations", Some(body)).await
}

/// Create a manager that must succeed, answering its ARN.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, answer) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let arn = answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string();
    assert_manager_arn(case, &arn);
    arn
}

/// The minimum a create needs: one definition whose `Type` satisfies the modelled
/// `^[a-zA-Z0-9_\-.:/]{3,200}$` pattern.
fn definition(type_: &str, parameters: Value) -> Value {
    json!({ "Type": type_, "Parameters": parameters })
}

/// Read back the manager, asserting the read succeeded.
async fn get_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = get(router, manager_arn).await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
}

/// Every `ManagerArn` this service mints matches the modelled pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<8-4-4-4-12 hex>$`
/// — the one `smithy_query(query="operation",operation="GetConfigurationManager")`
/// reports on its `ManagerArn` label — in the signing region and the emulator account.
fn assert_manager_arn(case: &str, arn: &str) {
    let prefix = format!("arn:aws:ssm-quicksetup:{REGION}:{ACCOUNT}:configuration-manager/");
    let uuid = arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case}: expected an ARN under {prefix}, got {arn:?}"));
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.len(),
        5,
        "case {case}: expected a 5-group uuid resource segment, got {uuid:?}"
    );
    for (group, width) in groups.iter().zip([8, 4, 4, 4, 12]) {
        assert!(
            group.len() == width && group.bytes().all(|b| b.is_ascii_hexdigit()),
            "case {case}: expected a {width}-digit hex group, got {group:?} in {uuid:?}"
        );
    }
    // The service mints version-4 uuids: the third group starts with the version
    // nibble and the fourth with the variant nibble.
    assert!(
        groups[2].starts_with('4') && groups[3].starts_with('8'),
        "case {case}: expected a version-4 uuid, got {uuid:?}"
    );
}

/// A configuration-definition id satisfies the modelled `^[a-z0-9-]{1,20}$` pattern
/// that `UpdateConfigurationDefinition` binds on its `{Id}` label.
fn assert_definition_id(case: &str, id: &str) {
    assert!(
        (1..=20).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "case {case}: expected an id matching ^[a-z0-9-]{{1,20}}$, got {id:?}"
    );
}

/// Every status summary reports values from the modelled `StatusType` / `Status`
/// enums (the generated `STATUS_TYPE_VALUES` / `STATUS_VALUES` catalogs) and carries
/// the `@required` `LastUpdatedAt`.
fn assert_status_summaries(case: &str, summaries: &Value) {
    let summaries = summaries
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {summaries:?}"));
    assert!(
        !summaries.is_empty(),
        "case {case}: expected at least one status summary"
    );
    for summary in summaries {
        let status_type = summary
            .get("StatusType")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: StatusType is required, got {summary:?}"));
        assert!(
            STATUS_TYPE_VALUES.contains(&status_type),
            "case {case}: StatusType {status_type:?} is not in the modelled catalog {STATUS_TYPE_VALUES:?}"
        );
        let status = summary
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a Status, got {summary:?}"));
        assert!(
            STATUS_VALUES.contains(&status),
            "case {case}: Status {status:?} is not in the modelled catalog {STATUS_VALUES:?}"
        );
        assert!(
            summary.get("LastUpdatedAt").is_some(),
            "case {case}: LastUpdatedAt is required, got {summary:?}"
        );
    }
}

/// The `Deployment` summary's `Status`, which is what the clock settles.
fn deployment_status(case: &str, summaries: &Value) -> String {
    summaries
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {summaries:?}"))
        .iter()
        .find(|summary| {
            summary.get("StatusType").and_then(Value::as_str) == Some(STATUS_TYPE_DEPLOYMENT)
        })
        .and_then(|summary| summary.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a {STATUS_TYPE_DEPLOYMENT} summary, got {summaries:?}"))
        .to_string()
}

/// A modelled timestamp member as seconds since the epoch, so two stamps can be
/// ordered.
///
/// The members carry `@timestampFormat("date-time")`, but the framework's restJson1
/// serializer writes the JSON protocol's default epoch-seconds number; both renderings
/// are read here so this assertion pins the ORDERING of the stamps, not the wire form
/// the framework chooses for them.
fn epoch_seconds(case: &str, value: &Value, member: &str) -> f64 {
    let stamp = value
        .get(member)
        .unwrap_or_else(|| panic!("case {case}: expected {member}, got {value:?}"));
    match stamp {
        Value::Number(seconds) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: {member} {stamp:?} is not a number")),
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|e| panic!("case {case}: {member} {text:?} is not a date-time: {e}"))
        .as_secs_f64(),
        other => panic!("case {case}: {member} is neither a timestamp number nor a string: {other:?}"),
    }
}

/// The definition ids a Get reports, in order.
fn definition_ids(case: &str, manager: &Value) -> Vec<String> {
    manager
        .get("ConfigurationDefinitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected ConfigurationDefinitions, got {manager:?}"))
        .iter()
        .map(|definition| {
            definition
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a stored definition must carry its service-assigned Id, got {definition:?}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The round trip: a create is readable back exactly as it was stored.
// ---------------------------------------------------------------------------

/// A create followed by a Get reports everything that was stored, whatever the
/// request's shape: one definition or several, with or without a name, a description,
/// tags, a type version or the local-deployment role members. Each row differs along
/// what the rule generalises over — the number of definitions, which optional members
/// are present, and the size of the parameter map.
#[tokio::test]
async fn a_created_manager_is_read_back_exactly_as_it_was_stored() {
    let router = emulator();
    // Every definition id the service assigns must be new, across managers too.
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "one definition, named, described and tagged",
            json!({
                "Name": "demo",
                "Description": "bootstrap",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({
                        "DelegatedAccountId": "111122223333",
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": "us-east-1",
                    }),
                )],
                "Tags": { "env": "dev" },
            }),
        ),
        (
            "three definitions, no name, no description, no tags",
            json!({
                "ConfigurationDefinitions": [
                    definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
                    definition("AWSQuickSetupType-PatchPolicy", json!({ "RateControlErrorThreshold": "10%" })),
                    definition("AWSQuickSetupType-DHMC", json!({ "TargetType": "*" })),
                ],
            }),
        ),
        (
            "a definition carrying every optional member",
            json!({
                "Name": "local.deployment-1",
                "Description": "",
                "ConfigurationDefinitions": [{
                    "Type": "AWSQuickSetupType-ResourceExplorer",
                    "Parameters": { "SelectedAggregatorRegion": "us-west-2" },
                    "TypeVersion": "1.1",
                    "LocalDeploymentExecutionRoleName": "QuickSetupExecRole",
                    "LocalDeploymentAdministrationRoleArn":
                        "arn:aws:iam::000000000000:role/QuickSetupAdminRole",
                }],
                "Tags": { "env": "prod", "owner": "platform" },
            }),
        ),
        (
            "a definition with an empty parameter map",
            json!({
                "Name": "empty-params",
                "ConfigurationDefinitions": [definition("AWSQuickSetupType-SSM", json!({}))],
            }),
        ),
    ];

    for (case, request) in cases {
        let manager_arn = create_ok(&router, case, &request).await;
        let manager = get_ok(&router, case, &manager_arn).await;

        assert_eq!(
            manager.get("ManagerArn").and_then(Value::as_str),
            Some(manager_arn.as_str()),
            "case {case}: the Get must report the ARN it was asked for, got {manager:?}"
        );
        for member in ["Name", "Description"] {
            assert_eq!(
                manager.get(member),
                request.get(member),
                "case {case}: {member} must come back as it was stored, got {manager:?}"
            );
        }
        assert_eq!(
            manager.get("Tags").cloned().unwrap_or_else(|| json!({})),
            request.get("Tags").cloned().unwrap_or_else(|| json!({})),
            "case {case}: Tags must come back as they were stored, got {manager:?}"
        );

        let created = epoch_seconds(case, &manager, "CreatedAt");
        let modified = epoch_seconds(case, &manager, "LastModifiedAt");
        assert!(
            modified >= created,
            "case {case}: LastModifiedAt {modified} must not precede CreatedAt {created}"
        );
        assert_status_summaries(
            case,
            manager
                .get("StatusSummaries")
                .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {manager:?}")),
        );

        let wanted = request
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .expect("the request carries definitions");
        let stored = manager
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected ConfigurationDefinitions, got {manager:?}"));
        assert_eq!(
            stored.len(),
            wanted.len(),
            "case {case}: expected one stored definition per submitted one, got {stored:?}"
        );
        for (index, (want, got)) in wanted.iter().zip(stored).enumerate() {
            for member in [
                "Type",
                "Parameters",
                "TypeVersion",
                "LocalDeploymentExecutionRoleName",
                "LocalDeploymentAdministrationRoleArn",
            ] {
                assert_eq!(
                    got.get(member),
                    want.get(member),
                    "case {case}: definition {index}'s {member} must come back as stored, got {got:?}"
                );
            }
            let id = got
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: definition {index} must carry a service-assigned Id, got {got:?}"));
            assert_definition_id(case, id);
            assert!(
                seen_ids.insert(id.to_string()),
                "case {case}: definition id {id:?} was assigned twice"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListConfigurationManagers
// ---------------------------------------------------------------------------

/// Paging visits every stored manager exactly once, for any store size and any page
/// bound: each page is at most `MaxItems` long, a `NextToken` is emitted only while
/// more remain, and the union of the pages is exactly the set of created ARNs.
#[tokio::test]
async fn paging_the_manager_list_visits_each_manager_exactly_once() {
    // (managers in the store, MaxItems asked for) — the page bound includes both
    // published ends of the modelled `@range(1, 100)`.
    let cases: [(usize, Option<i64>); 6] = [
        (1, Some(1)),
        (2, Some(1)),
        (5, Some(2)),
        (3, None),
        (4, Some(i64::from(MAX_ITEMS_LIMIT))),
        (0, Some(1)),
    ];

    for (manager_count, max_items) in cases {
        let case = format!("{manager_count} managers, MaxItems={max_items:?}");
        let router = emulator();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..manager_count {
            created.insert(
                create_ok(
                    &router,
                    &case,
                    &json!({
                        "Name": format!("manager-{index}"),
                        "ConfigurationDefinitions": [definition(
                            "AWSQuickSetupType-SSMOpsCenter",
                            json!({ "TargetRegions": "us-east-1" }),
                        )],
                    }),
                )
                .await,
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = Map::new();
            if let Some(max_items) = max_items {
                body.insert("MaxItems".to_string(), json!(max_items));
            }
            if let Some(token) = &token {
                body.insert("StartingToken".to_string(), json!(token));
            }
            let (status, answer) = list_managers(&router, &Value::Object(body)).await;
            assert_eq!(
                status, 200,
                "case {case}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
            );
            let page = answer
                .get("ConfigurationManagersList")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: expected a list, got {answer:?}"));
            if let Some(max_items) = max_items {
                assert!(
                    page.len() <= max_items as usize,
                    "case {case}: a page may not exceed MaxItems, got {} entries",
                    page.len()
                );
            }
            for summary in page {
                let arn = summary
                    .get("ManagerArn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: ManagerArn is required, got {summary:?}"));
                assert!(
                    summary.get("ConfigurationDefinitionSummaries").is_some(),
                    "case {case}: expected ConfigurationDefinitionSummaries, got {summary:?}"
                );
                assert_status_summaries(
                    &case,
                    summary
                        .get("StatusSummaries")
                        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {summary:?}")),
                );
                seen.push(arn.to_string());
            }
            pages += 1;
            assert!(
                pages <= manager_count + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
            match answer.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen.len(),
            manager_count,
            "case {case}: expected each manager exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case}: the pages must cover exactly the created managers, got {seen:?}"
        );
    }
}

/// A `MaxItems` outside the modelled `@range(1, 100)` is refused; both published
/// bounds are accepted.
#[tokio::test]
async fn the_page_bound_is_accepted_at_its_published_ends_and_refused_outside_them() {
    let router = emulator();
    create_ok(
        &router,
        "range",
        &json!({
            "Name": "range",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;

    for (max_items, want_status) in [
        (0, 400),
        (1, 200),
        (i64::from(MAX_ITEMS_LIMIT), 200),
        (i64::from(MAX_ITEMS_LIMIT) + 1, 400),
    ] {
        let (status, answer) = list_managers(&router, &json!({ "MaxItems": max_items })).await;
        assert_eq!(
            status, want_status,
            "case MaxItems={max_items}: expected {want_status}, got {status} / {answer:?}"
        );
        if want_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case MaxItems={max_items}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

/// A `StartingToken` this service never issued is refused rather than silently read
/// as "start from the beginning" — including a token the OTHER listing issued, whose
/// key names a record of a different kind.
#[tokio::test]
async fn a_pagination_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    for (case, uri) in [
        ("managers", "/listConfigurationManagers"),
        ("configurations", "/listConfigurations"),
    ] {
        for token in ["not-a-token", "6e6f742d612d746f6b656e"] {
            let (status, answer) =
                call(&router, "POST", uri, Some(&json!({ "StartingToken": token }))).await;
            assert_eq!(
                status, 400,
                "case {case} / {token}: a forged StartingToken must be refused, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case} / {token}: expected ValidationException, got {answer:?}"
            );
        }
    }

    // Two managers, each with two definitions, so both listings hand out a token.
    for name in ["alpha", "beta"] {
        create_ok(
            &router,
            "cross-listing token",
            &json!({
                "Name": name,
                "ConfigurationDefinitions": [
                    definition("AWSQuickSetupType-SSMOpsCenter", json!({ "TargetRegions": "us-east-1" })),
                    definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
                ],
            }),
        )
        .await;
    }

    let (status, answer) = list_managers(&router, &json!({ "MaxItems": 1 })).await;
    assert_eq!(status, 200, "got {answer:?}");
    let manager_token = answer["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a manager page token, got {answer:?}"))
        .to_string();
    let (status, answer) = list_configurations(&router, &json!({ "MaxItems": 1 })).await;
    assert_eq!(status, 200, "got {answer:?}");
    let configuration_token = answer["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a configuration page token, got {answer:?}"))
        .to_string();

    // (case, the listing to hand the token to, the token the other listing issued)
    for (case, uri, token) in [
        (
            "a manager token on the configuration listing",
            "/listConfigurations",
            manager_token.clone(),
        ),
        (
            "a configuration token on the manager listing",
            "/listConfigurationManagers",
            configuration_token.clone(),
        ),
    ] {
        let (status, answer) =
            call(&router, "POST", uri, Some(&json!({ "StartingToken": token }))).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
    }

    // And each listing still accepts its OWN token.
    for (case, uri, token) in [
        (
            "the manager listing's own token",
            "/listConfigurationManagers",
            manager_token,
        ),
        (
            "the configuration listing's own token",
            "/listConfigurations",
            configuration_token,
        ),
    ] {
        let (status, answer) =
            call(&router, "POST", uri, Some(&json!({ "StartingToken": token }))).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} / {answer:?}"
        );
    }
}

/// A `Filters` key that names no field of the listed shape is refused, and a key that
/// does narrows the page to the records whose value is one of the filter's `Values`.
#[tokio::test]
async fn a_filter_narrows_the_listing_and_an_unknown_filter_key_is_refused() {
    let router = emulator();
    let first = create_ok(
        &router,
        "filters",
        &json!({
            "Name": "alpha",
            "Description": "first",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let second = create_ok(
        &router,
        "filters",
        &json!({
            "Name": "beta",
            "Description": "second",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-Scheduler",
                json!({ "TargetTagKey": "Env" }),
            )],
        }),
    )
    .await;

    // (filter, the ARNs the listing must report)
    let cases: Vec<(Value, Vec<String>)> = vec![
        (
            json!([{ "Key": "Name", "Values": ["alpha"] }]),
            vec![first.clone()],
        ),
        (
            json!([{ "Key": "Name", "Values": ["beta", "gamma"] }]),
            vec![second.clone()],
        ),
        (
            json!([{ "Key": "Description", "Values": ["second"] }]),
            vec![second.clone()],
        ),
        (
            json!([{ "Key": "Name", "Values": ["alpha"] }, { "Key": "Description", "Values": ["second"] }]),
            Vec::new(),
        ),
        (json!([{ "Key": "Name", "Values": ["nobody"] }]), Vec::new()),
    ];
    for (filters, want) in cases {
        let (status, answer) = list_managers(&router, &json!({ "Filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {filters:?}: the listing must succeed, got {status} / {answer:?}"
        );
        let got: Vec<String> = answer["ConfigurationManagersList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {filters:?}: expected a list, got {answer:?}"))
            .iter()
            .map(|summary| summary["ManagerArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got,
            want.clone(),
            "case {filters:?}: expected {want:?}, got {got:?}"
        );
    }

    // `ManagerArn` is not a filter key on either listing — a filter value cannot even
    // carry one, since the modelled `FilterValues` pattern refuses `:`.
    for (case, uri) in [
        ("managers", "/listConfigurationManagers"),
        ("configurations", "/listConfigurations"),
    ] {
        let (status, answer) = call(
            &router,
            "POST",
            uri,
            Some(&json!({ "Filters": [{ "Key": "ManagerArn", "Values": ["alpha"] }] })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an unknown filter key must be refused, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
    }

    // The filter list and each filter's values are caller-sized and the model gives
    // them no length, so the service's own bounds are accepted at their ends and
    // refused one past them.
    let at_most = |count: usize| {
        Value::Array(
            (0..count)
                .map(|_| json!({ "Key": "Name", "Values": ["alpha"] }))
                .collect(),
        )
    };
    let many_values = |count: usize| {
        json!([{
            "Key": "Name",
            "Values": Value::Array((0..count).map(|index| json!(format!("alpha-{index}"))).collect()),
        }])
    };
    for (case, filters, want_status) in [
        ("filters at the cap", at_most(MAX_FILTERS), 200),
        ("one filter too many", at_most(MAX_FILTERS + 1), 400),
        ("values at the cap", many_values(MAX_FILTER_VALUES), 200),
        ("one value too many", many_values(MAX_FILTER_VALUES + 1), 400),
    ] {
        let (status, answer) = list_managers(&router, &json!({ "Filters": filters })).await;
        assert_eq!(
            status, want_status,
            "case {case}: expected {want_status}, got {status} / {answer:?}"
        );
        if want_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListConfigurations
// ---------------------------------------------------------------------------

/// Each manager's create produced one configuration record per configuration
/// definition, and `ListConfigurations` reports exactly those — narrowed by the
/// `ManagerArn` and `ConfigurationDefinitionId` the request names.
#[tokio::test]
async fn the_configuration_listing_reports_one_record_per_stored_definition() {
    let router = emulator();
    let first = create_ok(
        &router,
        "configurations",
        &json!({
            "Name": "two-definitions",
            "ConfigurationDefinitions": [
                definition("AWSQuickSetupType-SSMOpsCenter", json!({ "TargetRegions": "us-east-1" })),
                {
                    "Type": "AWSQuickSetupType-Scheduler",
                    "Parameters": { "TargetTagKey": "Env" },
                    "TypeVersion": "2.0",
                },
            ],
        }),
    )
    .await;
    let second = create_ok(
        &router,
        "configurations",
        &json!({
            "Name": "one-definition",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-DHMC",
                json!({ "TargetType": "*" }),
            )],
        }),
    )
    .await;

    let first_ids = definition_ids(
        "configurations",
        &get_ok(&router, "configurations", &first).await,
    );
    let second_ids = definition_ids(
        "configurations",
        &get_ok(&router, "configurations", &second).await,
    );
    assert_eq!(first_ids.len(), 2, "got {first_ids:?}");

    // (case, request, the (ManagerArn, ConfigurationDefinitionId) pairs it must report)
    type ListingCase = (&'static str, Value, Vec<(String, String)>);
    let cases: Vec<ListingCase> = vec![
        (
            "every configuration",
            json!({}),
            vec![
                (first.clone(), first_ids[0].clone()),
                (first.clone(), first_ids[1].clone()),
                (second.clone(), second_ids[0].clone()),
            ],
        ),
        (
            "one manager",
            json!({ "ManagerArn": first }),
            vec![
                (first.clone(), first_ids[0].clone()),
                (first.clone(), first_ids[1].clone()),
            ],
        ),
        (
            "one definition of one manager",
            json!({ "ManagerArn": first, "ConfigurationDefinitionId": first_ids[1] }),
            vec![(first.clone(), first_ids[1].clone())],
        ),
        (
            "a definition id that manager does not own",
            json!({ "ManagerArn": second, "ConfigurationDefinitionId": first_ids[0] }),
            Vec::new(),
        ),
        (
            "a filter on the configuration type",
            json!({ "Filters": [{ "Key": "Type", "Values": ["AWSQuickSetupType-DHMC"] }] }),
            vec![(second.clone(), second_ids[0].clone())],
        ),
        (
            "a filter on the deployment region",
            json!({ "Filters": [{ "Key": "Region", "Values": [REGION] }] }),
            vec![
                (first.clone(), first_ids[0].clone()),
                (first.clone(), first_ids[1].clone()),
                (second.clone(), second_ids[0].clone()),
            ],
        ),
    ];

    for (case, request, want) in cases {
        let (status, answer) = list_configurations(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
        );
        let rows = answer
            .get("ConfigurationsList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a list, got {answer:?}"));
        let mut got: Vec<(String, String)> = rows
            .iter()
            .map(|row| {
                (
                    row["ManagerArn"].as_str().unwrap_or_default().to_string(),
                    row["ConfigurationDefinitionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        got.sort();
        let mut want_sorted = want.clone();
        want_sorted.sort();
        assert_eq!(
            got, want_sorted,
            "case {case}: expected {want_sorted:?}, got {got:?}"
        );

        for row in rows {
            assert_eq!(
                row.get("Account").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {case}: a configuration is deployed in the emulator account, got {row:?}"
            );
            assert_eq!(
                row.get("Region").and_then(Value::as_str),
                Some(REGION),
                "case {case}: a configuration is deployed in the signing region, got {row:?}"
            );
            let id = row
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a configuration Id, got {row:?}"));
            assert_ne!(
                Some(id),
                row.get("ConfigurationDefinitionId").and_then(Value::as_str),
                "case {case}: a configuration's own id is not its definition id, got {row:?}"
            );
            assert!(
                row.get("CreatedAt").is_some(),
                "case {case}: expected CreatedAt, got {row:?}"
            );
            assert_status_summaries(
                case,
                row.get("StatusSummaries")
                    .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {row:?}")),
            );
        }
    }

    // A manager that was never created is absent, not an empty page.
    let (status, answer) =
        list_configurations(&router, &json!({ "ManagerArn": UNKNOWN_MANAGER_ARN })).await;
    assert_eq!(
        status, 404,
        "an unknown ManagerArn must answer 404, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {answer:?}"
    );
}

/// The configuration listing pages the same way the manager listing does.
#[tokio::test]
async fn paging_the_configuration_list_visits_each_configuration_exactly_once() {
    let router = emulator();
    create_ok(
        &router,
        "paging configurations",
        &json!({
            "Name": "five-definitions",
            "ConfigurationDefinitions": (0..5)
                .map(|index| definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({ "TargetRegions": format!("us-east-{}", index + 1) }),
                ))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;

    for max_items in [1, 2, 5] {
        let case = format!("MaxItems={max_items}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = Map::new();
            body.insert("MaxItems".to_string(), json!(max_items));
            if let Some(token) = &token {
                body.insert("StartingToken".to_string(), json!(token));
            }
            let (status, answer) = list_configurations(&router, &Value::Object(body)).await;
            assert_eq!(
                status, 200,
                "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
            );
            let page = answer["ConfigurationsList"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a list, got {answer:?}"))
                .clone();
            assert!(
                page.len() <= max_items,
                "case {case}: a page may not exceed MaxItems, got {} entries",
                page.len()
            );
            for row in &page {
                seen.push(row["Id"].as_str().unwrap_or_default().to_string());
            }
            match answer.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            5,
            "case {case}: expected every configuration exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>().len(),
            5,
            "case {case}: a configuration was served twice, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateConfigurationManager
// ---------------------------------------------------------------------------

/// An update mutates the stored record: the members it carries replace the stored
/// ones, the members it omits are left as they were, and `LastModifiedAt` moves
/// forward — all of which the next Get reports.
#[tokio::test]
async fn an_update_mutates_the_stored_record_and_bumps_the_modified_stamp() {
    // (case, the update body, the Name and Description the Get must then report)
    let cases: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "name only",
            json!({ "Name": "demo-renamed" }),
            Some("demo-renamed"),
            Some("first"),
        ),
        (
            "description only",
            json!({ "Description": "rewritten" }),
            Some("demo"),
            Some("rewritten"),
        ),
        (
            "both members",
            json!({ "Name": "demo-2", "Description": "second" }),
            Some("demo-2"),
            Some("second"),
        ),
        ("neither member", json!({}), Some("demo"), Some("first")),
    ];

    for (case, body, want_name, want_description) in cases {
        let router = emulator();
        let manager_arn = create_ok(
            &router,
            case,
            &json!({
                "Name": "demo",
                "Description": "first",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({ "TargetRegions": "us-east-1" }),
                )],
            }),
        )
        .await;
        let before = get_ok(&router, case, &manager_arn).await;
        let created = epoch_seconds(case, &before, "CreatedAt");
        let modified_before = epoch_seconds(case, &before, "LastModifiedAt");
        let definitions_before = before.get("ConfigurationDefinitions").cloned();

        // So the bumped stamp is observably later than the one the create wrote.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (status, answer) = update(&router, &manager_arn, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateConfigurationManager must succeed, got {status} / {answer:?}"
        );

        let after = get_ok(&router, case, &manager_arn).await;
        assert_eq!(
            after.get("Name").and_then(Value::as_str),
            want_name,
            "case {case}: expected Name {want_name:?}, got {after:?}"
        );
        assert_eq!(
            after.get("Description").and_then(Value::as_str),
            want_description,
            "case {case}: expected Description {want_description:?}, got {after:?}"
        );
        assert_eq!(
            after.get("ConfigurationDefinitions").cloned(),
            definitions_before,
            "case {case}: an update must not disturb the configuration definitions, got {after:?}"
        );
        assert_eq!(
            epoch_seconds(case, &after, "CreatedAt"),
            created,
            "case {case}: an update must not move CreatedAt, got {after:?}"
        );
        let modified_after = epoch_seconds(case, &after, "LastModifiedAt");
        assert!(
            modified_after > modified_before,
            "case {case}: expected LastModifiedAt to move past {modified_before}, got {modified_after}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteConfigurationManager
// ---------------------------------------------------------------------------

/// A delete removes the manager and the configurations it owned: the Get is a 404,
/// neither listing reports it, and a second delete is a 404 too. The manager that was
/// not deleted is untouched.
#[tokio::test]
async fn a_delete_removes_the_manager_and_the_configurations_it_owned() {
    let router = emulator();
    let doomed = create_ok(
        &router,
        "delete",
        &json!({
            "Name": "doomed",
            "ConfigurationDefinitions": [
                definition("AWSQuickSetupType-SSMOpsCenter", json!({ "TargetRegions": "us-east-1" })),
                definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
            ],
        }),
    )
    .await;
    let survivor = create_ok(
        &router,
        "delete",
        &json!({
            "Name": "survivor",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-DHMC",
                json!({ "TargetType": "*" }),
            )],
        }),
    )
    .await;

    let (status, answer) = list_configurations(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        answer["ConfigurationsList"].as_array().map(Vec::len),
        Some(3),
        "three configurations were created, got {answer:?}"
    );

    let (status, answer) = delete(&router, &doomed).await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must succeed, got {status} / {answer:?}"
    );

    let (status, answer) = get(&router, &doomed).await;
    assert_eq!(
        status, 404,
        "a deleted manager must be absent, got {status} / {answer:?}"
    );
    assert_eq!(error_code(&answer), "ResourceNotFoundException", "{answer:?}");

    let (status, answer) = list_managers(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    let listed: Vec<String> = answer["ConfigurationManagersList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list, got {answer:?}"))
        .iter()
        .map(|summary| summary["ManagerArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![survivor.clone()],
        "only the surviving manager may be listed, got {listed:?}"
    );

    let (status, answer) = list_configurations(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    let owners: Vec<String> = answer["ConfigurationsList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list, got {answer:?}"))
        .iter()
        .map(|row| row["ManagerArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        owners,
        vec![survivor.clone()],
        "a deleted manager's configurations go with it, got {owners:?}"
    );

    // Deleting it again names a manager the store no longer holds.
    let (status, answer) = delete(&router, &doomed).await;
    assert_eq!(
        status, 404,
        "a repeated delete must answer 404, got {status} / {answer:?}"
    );
    assert_eq!(error_code(&answer), "ResourceNotFoundException", "{answer:?}");

    // And the survivor is still readable, unchanged.
    let survivor_record = get_ok(&router, "delete", &survivor).await;
    assert_eq!(
        survivor_record.get("Name").and_then(Value::as_str),
        Some("survivor"),
        "got {survivor_record:?}"
    );
}

/// An ARN no create ever minted is a 404 on every operation that names one.
#[tokio::test]
async fn an_unknown_manager_arn_is_a_404_on_every_operation() {
    let router = emulator();
    let uri = format!("/configurationManager/{}", label(UNKNOWN_MANAGER_ARN));
    let cases: [(&str, &str, Option<Value>); 3] = [
        ("GET", uri.as_str(), None),
        ("PUT", uri.as_str(), Some(json!({ "Name": "renamed" }))),
        ("DELETE", uri.as_str(), None),
    ];
    for (method, uri, body) in cases {
        let (status, answer) = call(&router, method, uri, body.as_ref()).await;
        assert_eq!(
            status, 404,
            "case {method}: expected 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {method}: expected ResourceNotFoundException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A create the model or the service cannot accept is refused with a 400
/// `ValidationException`, and nothing is stored for it.
#[tokio::test]
async fn a_create_the_service_cannot_accept_is_refused_and_stores_nothing() {
    let router = emulator();
    let cases: Vec<(&str, Value)> = vec![
        (
            "no ConfigurationDefinitions member at all",
            json!({ "Name": "demo" }),
        ),
        (
            "an empty ConfigurationDefinitions list",
            json!({ "Name": "demo", "ConfigurationDefinitions": [] }),
        ),
        (
            "a Type shorter than the modelled 3 characters",
            json!({ "ConfigurationDefinitions": [definition("ab", json!({ "K": "v" }))] }),
        ),
        (
            "a Type carrying a character the modelled pattern refuses",
            json!({ "ConfigurationDefinitions": [definition("bad type", json!({ "K": "v" }))] }),
        ),
        (
            "a definition with no Parameters member",
            json!({ "ConfigurationDefinitions": [{ "Type": "AWSQuickSetupType-SSM" }] }),
        ),
        (
            "a Name longer than the modelled 120 characters",
            json!({
                "Name": "n".repeat(121),
                "ConfigurationDefinitions": [definition("AWSQuickSetupType-SSM", json!({}))],
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, answer) = create(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
    }

    let (status, answer) = list_managers(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        answer["ConfigurationManagersList"].as_array().map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// The clock-owed deployment transition
// ---------------------------------------------------------------------------

/// `CreateConfigurationManager` starts a deployment, so the manager and its
/// configurations are reported in the transient `INITIALIZING` status first and settle
/// into `SUCCEEDED` once the span the clock owes has passed — with no further call,
/// and the same answer from the Get and from both listings.
#[tokio::test]
async fn the_deployment_status_settles_once_the_clock_owes_it() {
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        "settle",
        &json!({
            "Name": "settling",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;

    let fresh = get_ok(&router, "settle", &manager_arn).await;
    assert_eq!(
        deployment_status("fresh", &fresh["StatusSummaries"]),
        STATUS_INITIALIZING,
        "a caller reading straight after the create must still see the transient status, got {fresh:?}"
    );

    tokio::time::sleep(past_transition()).await;

    let settled = get_ok(&router, "settle", &manager_arn).await;
    assert_eq!(
        deployment_status("settled", &settled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "the deployment must settle without any other call, got {settled:?}"
    );

    let (status, answer) = list_managers(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        deployment_status(
            "listed manager",
            &answer["ConfigurationManagersList"][0]["StatusSummaries"]
        ),
        STATUS_SUCCEEDED,
        "the listing must report the settled status, got {answer:?}"
    );

    let (status, answer) = list_configurations(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        deployment_status(
            "listed configuration",
            &answer["ConfigurationsList"][0]["StatusSummaries"]
        ),
        STATUS_SUCCEEDED,
        "the configuration listing must report the settled status, got {answer:?}"
    );

    // A read reports the settled status; it does not move it anywhere else.
    let again = get_ok(&router, "settle", &manager_arn).await;
    assert_eq!(
        again["StatusSummaries"], settled["StatusSummaries"],
        "a second read must report the same summaries, got {again:?}"
    );
}

/// The operations this slice leaves for their own issues still answer the scaffold's
/// 501 — never an empty 200, which a client cannot tell from a real implementation.
#[tokio::test]
async fn the_operations_this_slice_does_not_cover_still_answer_501() {
    let router = emulator();
    // `GET /serviceSettings` and `GET /listQuickSetupTypes` are no longer here: both
    // are implemented, and `tests/all/service_settings_and_types.rs` covers them.
    // `GET /getConfiguration/{ConfigurationId}` is no longer here either — it is
    // implemented, and the `GetConfiguration` section below covers it.
    let cases: [(&str, &str, Option<Value>); 1] = [
        (
            "GET",
            "/tags/arn%3Aaws%3Assm-quicksetup%3Aus-west-2%3A000000000000%3Aconfiguration-manager%2F00000000-0000-4000-8000-000000000000",
            None,
        ),
    ];
    for (method, uri, body) in cases {
        let (status, answer) = call(&router, method, uri, body.as_ref()).await;
        assert_eq!(
            status, 501,
            "case {method} {uri}: a stub must answer 501, got {status} / {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetConfiguration
// ---------------------------------------------------------------------------

/// `GET /getConfiguration/{ConfigurationId}`, the id carried as one `@httpLabel`.
async fn get_configuration(router: &MultiServiceRouter, configuration_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(configuration_id)),
        None,
    )
    .await
}

/// Every `ConfigurationSummary` the listing reports for one manager, in listing order.
async fn configurations_of(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> Vec<Value> {
    let (status, answer) = list_configurations(router, &json!({ "ManagerArn": manager_arn })).await;
    assert_eq!(
        status, 200,
        "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .clone()
}

/// The rule: `GetConfiguration` reports the stored configuration its `ConfigurationId`
/// names — the record the create minted for one configuration definition — so every
/// member comes back as it was stored, and as the configuration listing reports it.
///
/// Each row differs along what the rule generalises over: how many definitions the
/// create carried, how large each parameter map is (including an empty one), whether a
/// `TypeVersion` was given, and which Quick Setup type was deployed.
#[tokio::test]
async fn a_stored_configuration_is_read_back_by_its_service_generated_id() {
    let router = emulator();
    // Every configuration id the service hands out must be new, across managers too.
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "one definition, one parameter",
            json!({
                "Name": "single",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({ "TargetRegions": "us-east-1" }),
                )],
            }),
        ),
        (
            "three definitions, differing types, one with a TypeVersion",
            json!({
                "ConfigurationDefinitions": [
                    definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
                    {
                        "Type": "AWSQuickSetupType-PatchPolicy",
                        "Parameters": {
                            "RateControlErrorThreshold": "10%",
                            "TargetRegions": "us-east-1",
                            "TargetType": "*",
                        },
                        "TypeVersion": "2.0",
                    },
                    definition("AWSQuickSetupType-DHMC", json!({ "TargetType": "*" })),
                ],
            }),
        ),
        (
            "a definition with an empty parameter map",
            json!({
                "Name": "empty-params",
                "ConfigurationDefinitions": [definition("AWSQuickSetupType-SSM", json!({}))],
            }),
        ),
        (
            "a definition carrying every optional member",
            json!({
                "Name": "fully-specified",
                "ConfigurationDefinitions": [{
                    "Type": "AWSQuickSetupType-ResourceExplorer",
                    "Parameters": { "SelectedAggregatorRegion": "us-west-2" },
                    "TypeVersion": "1.1",
                    "LocalDeploymentExecutionRoleName": "QuickSetupExecRole",
                    "LocalDeploymentAdministrationRoleArn":
                        "arn:aws:iam::000000000000:role/QuickSetupAdminRole",
                }],
                "Tags": { "env": "prod" },
            }),
        ),
    ];

    for (case, request) in cases {
        let manager_arn = create_ok(&router, case, &request).await;
        let submitted = request
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .expect("the request carries definitions");
        // The definition ids the service assigned, in the order the create submitted
        // them, so each configuration can be matched to the definition it deploys.
        let ids = definition_ids(case, &get_ok(&router, case, &manager_arn).await);
        let listed = configurations_of(&router, case, &manager_arn).await;
        assert_eq!(
            listed.len(),
            submitted.len(),
            "case {case}: expected one configuration per submitted definition, got {listed:?}"
        );

        for summary in &listed {
            let id = summary
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected an Id, got {summary:?}"));
            let definition_id = summary
                .get("ConfigurationDefinitionId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a ConfigurationDefinitionId, got {summary:?}")
                });
            let index = ids
                .iter()
                .position(|assigned| assigned == definition_id)
                .unwrap_or_else(|| {
                    panic!("case {case}: {definition_id:?} is not one of this manager's definition ids {ids:?}")
                });
            let want = &submitted[index];

            let (status, got) = get_configuration(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case}: GetConfiguration({id}) must succeed, got {status} / {got:?}"
            );
            assert_eq!(
                got.get("Id").and_then(Value::as_str),
                Some(id),
                "case {case}: the Get must report the id it was asked for, got {got:?}"
            );
            assert_eq!(
                got.get("ManagerArn").and_then(Value::as_str),
                Some(manager_arn.as_str()),
                "case {case}: the configuration must name the manager that owns it, got {got:?}"
            );
            assert_eq!(
                got.get("ConfigurationDefinitionId").and_then(Value::as_str),
                Some(definition_id),
                "case {case}: the Get must name the definition it deploys, got {got:?}"
            );
            for member in ["Type", "TypeVersion"] {
                assert_eq!(
                    got.get(member),
                    want.get(member),
                    "case {case}: {member} must come back as it was stored, got {got:?}"
                );
            }
            // The create supplied the parameters; `GetConfiguration$Parameters` and the
            // listing's `ConfigurationSummary$FirstClassParameters` are both that map.
            let stored_parameters = got.get("Parameters").cloned().unwrap_or_else(|| json!({}));
            assert_eq!(
                stored_parameters,
                want.get("Parameters").cloned().unwrap_or_else(|| json!({})),
                "case {case}: Parameters must come back as they were stored, got {got:?}"
            );
            assert_eq!(
                Some(&stored_parameters),
                summary.get("FirstClassParameters"),
                "case {case}: the Get and the listing must report one stored parameter map, got {got:?}"
            );
            assert_eq!(
                got.get("Account").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {case}: a configuration is deployed in the emulator account, got {got:?}"
            );
            assert_eq!(
                got.get("Region").and_then(Value::as_str),
                Some(REGION),
                "case {case}: a configuration is deployed in the signing region, got {got:?}"
            );
            let created = epoch_seconds(case, &got, "CreatedAt");
            let modified = epoch_seconds(case, &got, "LastModifiedAt");
            assert!(
                modified >= created,
                "case {case}: LastModifiedAt {modified} must not precede CreatedAt {created}"
            );
            assert_eq!(
                epoch_seconds(case, summary, "CreatedAt"),
                created,
                "case {case}: the Get and the listing must report one CreatedAt, got {got:?}"
            );
            assert_status_summaries(
                case,
                got.get("StatusSummaries")
                    .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {got:?}")),
            );
            assert!(
                seen_ids.insert(id.to_string()),
                "case {case}: configuration id {id:?} was handed out twice"
            );
        }
    }
}

/// A configuration the store does not hold is the modelled 404, whatever the form of
/// the id asked for: one reclaimed with the manager that owned it, and well-formed ids
/// of every size the `ConfigurationId` pattern `^[a-zA-Z0-9-_/:]{1,100}$` admits (from
/// `smithy_query(query="operation",operation="GetConfiguration")`) that no create minted.
#[tokio::test]
async fn a_configuration_the_store_does_not_hold_is_the_modelled_404() {
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        "404",
        &json!({
            "Name": "to-be-deleted",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let listed = configurations_of(&router, "404", &manager_arn).await;
    let reclaimed = listed[0]["Id"]
        .as_str()
        .expect("a stored configuration carries its id")
        .to_string();

    // While its manager is stored, the configuration is readable.
    let (status, answer) = get_configuration(&router, &reclaimed).await;
    assert_eq!(
        status, 200,
        "a stored configuration must be readable, got {status} / {answer:?}"
    );

    let (status, answer) = delete(&router, &manager_arn).await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must succeed, got {status} / {answer:?}"
    );

    let cases: Vec<(&str, String)> = vec![
        ("reclaimed with its manager", reclaimed),
        (
            "a uuid no create minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("the shortest id the pattern admits", "a".to_string()),
        ("the longest id the pattern admits", "c".repeat(100)),
        (
            "an id carrying every character class the pattern admits",
            "Ab9-_/:".to_string(),
        ),
    ];

    for (case, configuration_id) in cases {
        let (status, answer) = get_configuration(&router, &configuration_id).await;
        assert_eq!(
            status, 404,
            "case {case}: an unstored configuration must answer 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // One past the pattern's published maximum is a bad request, not a 404: the
    // modelled constraint is refused before the handler runs.
    let (status, answer) = get_configuration(&router, &"c".repeat(101)).await;
    assert_eq!(
        status, 400,
        "an id past the pattern's 100-character maximum must be refused, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "expected ValidationException, got {answer:?}"
    );
}

/// Reading a configuration reports the deployment status the clock owes it — the
/// transient one straight after the create, the settled one once the span has passed —
/// and reports it without moving anything: a second read answers exactly the same.
#[tokio::test]
async fn reading_a_configuration_reports_the_status_the_clock_owes_and_moves_nothing() {
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        "clock",
        &json!({
            "Name": "settling-configuration",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let listed = configurations_of(&router, "clock", &manager_arn).await;
    let id = listed[0]["Id"]
        .as_str()
        .expect("a stored configuration carries its id")
        .to_string();

    let (status, fresh) = get_configuration(&router, &id).await;
    assert_eq!(status, 200, "got {status} / {fresh:?}");
    assert_eq!(
        deployment_status("fresh", &fresh["StatusSummaries"]),
        STATUS_INITIALIZING,
        "a caller reading straight after the create must still see the transient status, got {fresh:?}"
    );

    tokio::time::sleep(past_transition()).await;

    let (status, settled) = get_configuration(&router, &id).await;
    assert_eq!(status, 200, "got {status} / {settled:?}");
    assert_eq!(
        deployment_status("settled", &settled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "the deployment must settle without any other call, got {settled:?}"
    );

    let (status, again) = get_configuration(&router, &id).await;
    assert_eq!(status, 200, "got {status} / {again:?}");
    assert_eq!(
        again, settled,
        "a second read must report the same record, got {again:?}"
    );
}
