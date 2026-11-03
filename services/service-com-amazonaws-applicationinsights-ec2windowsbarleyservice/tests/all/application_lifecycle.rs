// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the application + component lifecycle, driven over the
//! wire the way a client drives it: `POST /` with
//! `X-Amz-Target: EC2WindowsBarleyService.<Operation>` on the `awsJson1_1`
//! protocol, through the same `MultiServiceRouter` the emulator serves.
//!
//! Every assertion reads the RESPONSE BODY, not just the status, and every table
//! row names itself in the failure message.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated resource is owned by, as the vendored runtime
/// defines it. Never a literal: `ApplicationInfo$AccountId` must agree with it.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router serving this service alone over a fresh engine, with request
/// authentication off so a suite signs nothing.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one operation and return its HTTP status plus its parsed JSON body.
///
/// The `Authorization` credential scope is what routes the request to this
/// service (its signing name is `applicationinsights`), exactly as a signed SDK
/// request does; the `X-Amz-Target` names the operation.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("EC2WindowsBarleyService.{operation}"),
        )
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/applicationinsights/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=not-verified",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The `__type` an awsJson error body names, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Wait out the onboarding span a create call starts, so a reader observes the
/// lifecycle the clock owes rather than the transient one. Derived from the
/// service's own constant, so the suite follows the span if it is ever retuned.
async fn wait_out_onboarding() {
    tokio::time::sleep(
        service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::state::ONBOARDING_SPAN
            + Duration::from_millis(400),
    )
    .await;
}

/// The whole slice end to end: create, re-create, describe, list with paging,
/// add a component, describe it, then delete and watch the application go.
#[tokio::test]
async fn application_and_component_lifecycle_round_trips() {
    let router = router();

    let (status, created) = call(
        &router,
        "CreateApplication",
        json!({
            "ResourceGroupName": "my-rg",
            "OpsCenterEnabled": true,
            "CWEMonitorEnabled": true,
            "AutoConfigEnabled": true,
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {created:?}");
    let info = &created["ApplicationInfo"];
    assert_eq!(
        info["ResourceGroupName"], "my-rg",
        "CreateApplication must report the group it stored, got {info:?}"
    );
    assert_eq!(
        info["AccountId"], account_id(),
        "CreateApplication must report the emulator's account, got {info:?}"
    );
    assert_eq!(
        info["OpsCenterEnabled"], true,
        "CreateApplication must report the stored OpsCenterEnabled, got {info:?}"
    );
    assert_eq!(
        info["CWEMonitorEnabled"], true,
        "CreateApplication must report the stored CWEMonitorEnabled, got {info:?}"
    );
    assert_eq!(
        info["AutoConfigEnabled"], true,
        "CreateApplication must report the stored AutoConfigEnabled, got {info:?}"
    );
    assert_eq!(
        info["DiscoveryType"], "RESOURCE_GROUP_BASED",
        "a group-named application is onboarded from its resource group, got {info:?}"
    );
    assert_eq!(
        info["LifeCycle"], "CREATING",
        "onboarding is owed by the clock, so a create reports CREATING, got {info:?}"
    );

    // The same group name twice is the resource already being in use.
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 400, "re-create must be refused, got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceInUseException",
        "re-create must name ResourceInUseException, got {body:?}"
    );

    wait_out_onboarding().await;

    // Describe answers from the stored record: every field the create reported,
    // with the lifecycle the clock has since moved on.
    let (status, described) = call(
        &router,
        "DescribeApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 200, "DescribeApplication: got body {described:?}");
    let seen = &described["ApplicationInfo"];
    assert_eq!(
        seen["LifeCycle"], "ACTIVE",
        "the onboarding the create owed is due by now, got {seen:?}"
    );
    for member in [
        "ResourceGroupName",
        "AccountId",
        "OpsCenterEnabled",
        "CWEMonitorEnabled",
        "AutoConfigEnabled",
        "DiscoveryType",
    ] {
        assert_eq!(
            seen[member], info[member],
            "DescribeApplication must report the created {member}: expected {:?}, got {:?}",
            info[member], seen[member]
        );
    }

    // A component round-trips its resource list exactly.
    let resources = json!([
        "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
        "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000001",
    ]);
    let (status, body) = call(
        &router,
        "CreateComponent",
        json!({
            "ResourceGroupName": "my-rg",
            "ComponentName": "web-tier",
            "ResourceList": resources,
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateComponent: got body {body:?}");

    let (status, component) = call(
        &router,
        "DescribeComponent",
        json!({ "ResourceGroupName": "my-rg", "ComponentName": "web-tier" }),
    )
    .await;
    assert_eq!(status, 200, "DescribeComponent: got body {component:?}");
    assert_eq!(
        component["ApplicationComponent"]["ComponentName"], "web-tier",
        "DescribeComponent must report the stored name, got {component:?}"
    );
    assert_eq!(
        component["ApplicationComponent"]["Tier"], "DEFAULT",
        "an unconfigured component sits in the DEFAULT tier, got {component:?}"
    );
    assert_eq!(
        component["ResourceList"], resources,
        "DescribeComponent must round-trip the supplied ResourceList, got {component:?}"
    );

    // An unknown component name in a known group is not found.
    let (status, body) = call(
        &router,
        "DescribeComponent",
        json!({ "ResourceGroupName": "my-rg", "ComponentName": "no-such-tier" }),
    )
    .await;
    assert_eq!(status, 400, "unknown component: got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "unknown component must name ResourceNotFoundException, got {body:?}"
    );

    // Delete takes the application and its components with it.
    let (status, body) = call(
        &router,
        "DeleteApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteApplication: got body {body:?}");

    let (status, body) = call(
        &router,
        "DescribeApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 400, "describe after delete: got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "describe after delete must name ResourceNotFoundException, got {body:?}"
    );

    let (status, listed) = call(&router, "ListApplications", json!({})).await;
    assert_eq!(status, 200, "ListApplications: got body {listed:?}");
    assert_eq!(
        listed["ApplicationInfoList"],
        json!([]),
        "a deleted application must not be listed, got {listed:?}"
    );

    // Deleting what is gone is not found.
    let (status, body) = call(
        &router,
        "DeleteApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 400, "delete of an unknown group: got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "delete of an unknown group must name ResourceNotFoundException, got {body:?}"
    );
}

/// `GroupingType` decides how an application was onboarded, for every value the
/// model's `GroupingType` enum carries plus its absence
/// (`smithy_query(query="shape",shape="GroupingType")` lists `ACCOUNT_BASED` as
/// its only member).
#[tokio::test]
async fn discovery_type_follows_the_grouping_type() {
    let router = router();

    let cases: &[(&str, Option<&str>, &str)] = &[
        ("absent", None, "RESOURCE_GROUP_BASED"),
        ("ACCOUNT_BASED", Some("ACCOUNT_BASED"), "ACCOUNT_BASED"),
    ];

    for (case, grouping_type, expected) in cases {
        let mut request = json!({ "ResourceGroupName": format!("group-{case}") });
        if let Some(grouping_type) = grouping_type {
            request["GroupingType"] = json!(grouping_type);
        }
        let (status, body) = call(&router, "CreateApplication", request).await;
        assert_eq!(status, 200, "case {case:?}: got body {body:?}");
        assert_eq!(
            body["ApplicationInfo"]["DiscoveryType"], *expected,
            "case {case:?}: expected DiscoveryType {expected}, got {:?}",
            body["ApplicationInfo"]["DiscoveryType"]
        );
    }
}

/// The ARN-derived `ResourceType` of a custom component, over the resource lists
/// a custom component may carry: one EC2 instance and several of them. The
/// resources a custom component may NOT group are the subject of
/// `tests/all/admission_checks.rs`.
#[tokio::test]
async fn component_resource_type_is_derived_from_the_grouped_arns() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "types-rg" }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    let cases: &[(&str, &[&str], Option<&str>)] = &[
        (
            "ec2-instance",
            &["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            Some("AWS::EC2::Instance"),
        ),
        (
            "two-instances",
            &[
                "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
                "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000002",
            ],
            Some("AWS::EC2::Instance"),
        ),
    ];

    for (case, resources, expected) in cases {
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "types-rg",
                "ComponentName": case,
                "ResourceList": resources,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: CreateComponent gave {body:?}");

        let (status, body) = call(
            &router,
            "DescribeComponent",
            json!({ "ResourceGroupName": "types-rg", "ComponentName": case }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: DescribeComponent gave {body:?}");
        let reported = body["ApplicationComponent"]
            .get("ResourceType")
            .and_then(Value::as_str);
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected ResourceType {expected:?}, got {reported:?}"
        );
        assert_eq!(
            body["ResourceList"],
            json!(resources),
            "case {case:?}: ResourceList must round-trip, got {:?}",
            body["ResourceList"]
        );
    }
}

/// Paging walks every application exactly once, for every page size from the
/// bottom of the modelled `MaxResults` range upward and for the absent case —
/// the invariant is "the concatenated pages are the whole set, in key order,
/// with no repeats", whatever the page size.
#[tokio::test]
async fn paging_applications_visits_every_application_once() {
    let router = router();

    let names: Vec<String> = (0..7).map(|n| format!("rg-{n:02}")).collect();
    for name in &names {
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": name }),
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got body {body:?}");
    }

    // 1 and 40 are the published bounds of the `MaxResults` range; the rest
    // divide the set evenly and unevenly, and `None` exercises the default.
    let page_sizes: &[Option<i64>] = &[Some(1), Some(2), Some(3), Some(7), Some(40), None];
    for page_size in page_sizes {
        let case = format!("MaxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut calls = 0;
        loop {
            calls += 1;
            assert!(
                calls <= names.len() + 2,
                "case {case:?}: paging did not terminate after {calls} calls, saw {seen:?}"
            );
            let mut request = json!({});
            if let Some(page_size) = page_size {
                request["MaxResults"] = json!(page_size);
            }
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListApplications", request).await;
            assert_eq!(status, 200, "case {case:?}: got body {body:?}");

            let listed = body["ApplicationInfoList"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no ApplicationInfoList in {body:?}"));
            if let Some(limit) = page_size {
                assert!(
                    i64::try_from(listed.len()).unwrap_or(i64::MAX) <= *limit,
                    "case {case:?}: a page returned {} entries, more than MaxResults",
                    listed.len()
                );
            }
            assert!(
                !listed.is_empty(),
                "case {case:?}: a token led to an empty page, body {body:?}"
            );
            for entry in listed {
                let name = entry["ResourceGroupName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no ResourceGroupName in {entry:?}"));
                seen.push(name.to_string());
            }

            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case {case:?}: paging must visit every application once in key order, got {seen:?}"
        );
    }
}

/// Paging a component list follows the same invariant, over the component names
/// of one application.
#[tokio::test]
async fn paging_components_visits_every_component_once() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "paged-rg" }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    let names: Vec<String> = (0..5).map(|n| format!("component-{n}")).collect();
    for name in &names {
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "paged-rg",
                "ComponentName": name,
                "ResourceList": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            }),
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got body {body:?}");
    }

    for page_size in [1_i64, 2, 5, 40] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "ResourceGroupName": "paged-rg", "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListComponents", request).await;
            assert_eq!(status, 200, "case {case:?}: got body {body:?}");
            let listed = body["ApplicationComponentList"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no ApplicationComponentList in {body:?}")
                });
            assert!(
                i64::try_from(listed.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page returned {} entries, more than MaxResults",
                listed.len()
            );
            assert!(
                !listed.is_empty(),
                "case {case:?}: a token led to an empty page, body {body:?}"
            );
            for entry in listed {
                let name = entry["ComponentName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no ComponentName in {entry:?}"));
                seen.push(name.to_string());
            }
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case {case:?}: paging must visit every component once in key order, got {seen:?}"
        );
    }
}

/// Refusals: a bad request is refused with the modelled error and its documented
/// status, never a 500 and never a silent success. Each row crosses one rule.
#[tokio::test]
async fn bad_requests_are_refused_with_the_modelled_error() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "refusals-rg" }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    // `ResourceGroupName` is 1..256 characters matching `^[a-zA-Z0-9\.\-_]*$`;
    // `MaxResults` is 1..40; `ComponentName` is 1..128 matching
    // `^[\d\w\-_\.+]*$`; `AccountId` is exactly 12 digits. Each row is one
    // operation, one member, one side of one of those rules.
    let cases: &[(&str, &str, Value, u16, &str)] = &[
        (
            "create-application-without-a-group-name",
            "CreateApplication",
            json!({ "OpsCenterEnabled": true }),
            400,
            "ValidationException",
        ),
        (
            "create-application-group-name-too-long",
            "CreateApplication",
            json!({ "ResourceGroupName": "x".repeat(257) }),
            400,
            "ValidationException",
        ),
        (
            "create-application-group-name-bad-characters",
            "CreateApplication",
            json!({ "ResourceGroupName": "not a group name" }),
            400,
            "ValidationException",
        ),
        (
            "describe-application-without-a-group-name",
            "DescribeApplication",
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "describe-application-unknown-group",
            "DescribeApplication",
            json!({ "ResourceGroupName": "never-created" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "describe-application-other-account",
            "DescribeApplication",
            json!({ "ResourceGroupName": "refusals-rg", "AccountId": "111111111111" }),
            400,
            "ValidationException",
        ),
        (
            "describe-application-malformed-account",
            "DescribeApplication",
            json!({ "ResourceGroupName": "refusals-rg", "AccountId": "1" }),
            400,
            "ValidationException",
        ),
        (
            "list-applications-max-results-below-the-range",
            "ListApplications",
            json!({ "MaxResults": 0 }),
            400,
            "ValidationException",
        ),
        (
            "list-applications-max-results-above-the-range",
            "ListApplications",
            json!({ "MaxResults": 41 }),
            400,
            "ValidationException",
        ),
        (
            "list-applications-forged-next-token",
            "ListApplications",
            json!({ "NextToken": "not-a-token-this-service-issued" }),
            400,
            "ValidationException",
        ),
        (
            // Well-formed hex that decodes cleanly but carries no marker: it must
            // be refused, not read as the key "ab" and silently resumed from.
            "list-applications-hex-but-forged-next-token",
            "ListApplications",
            json!({ "NextToken": "6162" }),
            400,
            "ValidationException",
        ),
        (
            "list-components-forged-next-token",
            "ListComponents",
            json!({ "ResourceGroupName": "refusals-rg", "NextToken": "6162" }),
            400,
            "ValidationException",
        ),
        (
            "create-component-unknown-group",
            "CreateComponent",
            json!({
                "ResourceGroupName": "never-created",
                "ComponentName": "web-tier",
                "ResourceList": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "create-component-without-a-resource-list",
            "CreateComponent",
            json!({ "ResourceGroupName": "refusals-rg", "ComponentName": "web-tier" }),
            400,
            "ValidationException",
        ),
        (
            "create-component-name-bad-characters",
            "CreateComponent",
            json!({
                "ResourceGroupName": "refusals-rg",
                "ComponentName": "not a component name",
                "ResourceList": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            }),
            400,
            "ValidationException",
        ),
        (
            "list-components-unknown-group",
            "ListComponents",
            json!({ "ResourceGroupName": "never-created" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "list-components-max-results-above-the-range",
            "ListComponents",
            json!({ "ResourceGroupName": "refusals-rg", "MaxResults": 41 }),
            400,
            "ValidationException",
        ),
        (
            "delete-application-unknown-group",
            "DeleteApplication",
            json!({ "ResourceGroupName": "never-created" }),
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, operation, request, expected_status, expected_code) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got body {body:?}"
        );
    }
}

/// `AccountId` selects the resource-group owner a read is served for: the
/// caller's own account is served, and an account that is not the caller's is
/// refused rather than answered from the local account's state.
#[tokio::test]
async fn another_account_is_not_the_callers_to_read() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({
            "ResourceGroupName": "owned-rg",
            "Tags": [{ "Key": "team", "Value": "platform" }],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    // The caller's own account sees the application.
    let (status, listed) = call(
        &router,
        "ListApplications",
        json!({ "AccountId": account_id() }),
    )
    .await;
    assert_eq!(status, 200, "own account: got body {listed:?}");
    assert_eq!(
        listed["ApplicationInfoList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case own-account: the application must be listed, got {listed:?}"
    );

    // Any other 12-digit account is not the caller's to read.
    for other in ["111111111111", "999999999999"] {
        for operation in ["ListApplications", "ListComponents"] {
            let mut request = json!({ "AccountId": other });
            if operation == "ListComponents" {
                request["ResourceGroupName"] = json!("owned-rg");
            }
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {other:?} on {operation}: expected the request to be refused, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {other:?} on {operation}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                body["Message"],
                json!(format!(
                    "Account {} is not able to access applications that belong to account {other}",
                    account_id()
                )),
                "case {other:?} on {operation}: unexpected message in {body:?}"
            );
        }
    }
}

/// The emulator's own store bounds, asserted AT the boundary: the last create
/// inside each quota succeeds and the first one past it is refused, so the store
/// a caller can grow is bounded in every dimension this slice writes.
#[tokio::test]
async fn store_quotas_are_enforced_at_their_boundary() {
    use service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::state::{
        APPLICATION_QUOTA, COMPONENT_QUOTA, COMPONENT_RESOURCE_QUOTA,
    };

    let arn = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";

    // A single request may not store more resources than the per-component bound.
    {
        let router = router();
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "resource-quota-rg" }),
        )
        .await;
        assert_eq!(status, 200, "CreateApplication: got body {body:?}");

        for (case, count, expected_status) in [
            ("at-the-resource-quota", COMPONENT_RESOURCE_QUOTA, 200),
            ("past-the-resource-quota", COMPONENT_RESOURCE_QUOTA + 1, 400),
        ] {
            let resources: Vec<&str> = std::iter::repeat_n(arn, count).collect();
            let (status, body) = call(
                &router,
                "CreateComponent",
                json!({
                    "ResourceGroupName": "resource-quota-rg",
                    "ComponentName": case,
                    "ResourceList": resources,
                }),
            )
            .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status} for {count} resources, got {status} with body {body:?}"
            );
            if expected_status == 400 {
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body:?}"
                );
            }
        }
    }

    // One application holds at most COMPONENT_QUOTA components.
    {
        let router = router();
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "component-quota-rg" }),
        )
        .await;
        assert_eq!(status, 200, "CreateApplication: got body {body:?}");

        for n in 0..COMPONENT_QUOTA {
            let (status, body) = call(
                &router,
                "CreateComponent",
                json!({
                    "ResourceGroupName": "component-quota-rg",
                    "ComponentName": format!("component-{n:04}"),
                    "ResourceList": [arn],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case component-{n}: a create inside the quota must succeed, got {body:?}"
            );
        }
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "component-quota-rg",
                "ComponentName": "one-too-many",
                "ResourceList": [arn],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case past-the-component-quota: expected a refusal, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case past-the-component-quota: expected ValidationException, got {body:?}"
        );
    }

    // The store holds at most APPLICATION_QUOTA applications, and a delete frees
    // a slot again.
    {
        let router = router();
        for n in 0..APPLICATION_QUOTA {
            let (status, body) = call(
                &router,
                "CreateApplication",
                json!({ "ResourceGroupName": format!("quota-rg-{n:05}") }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case application-{n}: a create inside the quota must succeed, got {body:?}"
            );
        }
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "one-too-many" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case past-the-application-quota: expected a refusal, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case past-the-application-quota: expected ValidationException, got {body:?}"
        );

        let (status, body) = call(
            &router,
            "DeleteApplication",
            json!({ "ResourceGroupName": "quota-rg-00000" }),
        )
        .await;
        assert_eq!(status, 200, "DeleteApplication: got body {body:?}");
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "one-too-many" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case after-a-delete: a freed slot must be usable again, got {body:?}"
        );
    }
}

/// A duplicate component is the resource already being in use, for several
/// component names; and a second application's components stay its own.
#[tokio::test]
async fn components_are_keyed_per_application() {
    let router = router();
    for group in ["left-rg", "right-rg"] {
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": group }),
        )
        .await;
        assert_eq!(status, 200, "creating {group}: got body {body:?}");
    }

    let arn = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";
    // Component names differing in form: plain, dotted, underscored, plus-signed
    // — every one inside the `^[\d\w\-_\.+]*$` pattern the model declares.
    for name in ["web", "web.tier", "web_tier", "web+tier", "3"] {
        // The same name creates once per application, and is in use the second
        // time within one application.
        for group in ["left-rg", "right-rg"] {
            let (status, body) = call(
                &router,
                "CreateComponent",
                json!({
                    "ResourceGroupName": group,
                    "ComponentName": name,
                    "ResourceList": [arn],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?} in {group}: expected the first create to succeed, got {body:?}"
            );
        }
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "left-rg",
                "ComponentName": name,
                "ResourceList": [arn],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: a duplicate component must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {name:?}: expected ResourceInUseException, got {body:?}"
        );
    }

    // Deleting one application leaves the other's components untouched.
    let (status, body) = call(
        &router,
        "DeleteApplication",
        json!({ "ResourceGroupName": "left-rg" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteApplication: got body {body:?}");

    let (status, body) = call(
        &router,
        "ListComponents",
        json!({ "ResourceGroupName": "right-rg" }),
    )
    .await;
    assert_eq!(status, 200, "ListComponents: got body {body:?}");
    let mut remaining: BTreeMap<String, bool> = BTreeMap::new();
    for entry in body["ApplicationComponentList"]
        .as_array()
        .unwrap_or_else(|| panic!("no ApplicationComponentList in {body:?}"))
    {
        remaining.insert(
            entry["ComponentName"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            true,
        );
    }
    assert_eq!(
        remaining.len(),
        5,
        "the surviving application must keep its five components, got {remaining:?}"
    );

    let (status, body) = call(
        &router,
        "ListComponents",
        json!({ "ResourceGroupName": "left-rg" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the deleted application has no component list, got {body:?}"
    );
}
