// SPDX-License-Identifier: CC0-1.0

//! The admission checks every Application Insights operation runs before it answers —
//! the refusals a request earns for what it NAMES, rather than for a modelled
//! constraint.
//!
//! Four rules, each asserted over a table that spans what it generalises over:
//!
//! * an operation addressing a monitored application is refused when no application
//!   answers to the resource group it named, with the service's fixed sentence;
//! * a custom component groups standalone EC2 instances, so a resource list naming
//!   anything else is refused and nothing is stored;
//! * an operation addressing a component is refused when the application exists and
//!   holds no component of that name, naming both in its sentence;
//! * `AccountId` selects the resource-group owner a read is served for, and an account
//!   that is not the caller's is refused rather than answered from local state.
//!
//! Every refusal is read off the RESPONSE BODY — its status, its `__type` and its
//! `Message` — because a client branches on all three, and every row names itself in
//! the failure message.
//!
//! No Resource Groups service is mounted here, which is the other half of
//! `CreateApplication`'s group resolution: with nobody to ask, a create is admitted
//! rather than refused on an assumption. The refusal half is
//! `core/router/tests/all/applicationinsights_resource_group_admission.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// An EC2 instance ARN — the one resource a custom component may group.
const INSTANCE_ARN: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";

/// A second one, so a row can group more than one instance.
const OTHER_INSTANCE_ARN: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000002";

/// The account every emulated resource is owned by, as the vendored runtime defines it.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router serving this service alone over a fresh engine, with request authentication
/// off so a suite signs nothing.
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

/// The `Message` an error body carries, whichever casing the envelope used.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// One row's request builder: an operation name and the request to send it, built
/// around the one value the row varies (a resource group, or an account id).
type Operation = (&'static str, fn(&str) -> Value);

/// Assert one refusal whole: its status, its wire error code and its sentence.
fn assert_refused(case: &str, status: u16, body: &Value, expected_code: &str, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
    );
    assert_eq!(
        error_code(body),
        expected_code,
        "case {case:?}: expected {expected_code}, got body {body:?}"
    );
    assert_eq!(
        error_message(body),
        expected,
        "case {case:?}: expected the sentence {expected:?}, got body {body:?}"
    );
}

/// Create an application over `name`, which is admitted because this router has no
/// Resource Groups to resolve the group against.
async fn create_application(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateApplication",
        json!({ "ResourceGroupName": name }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateApplication({name:?}): {body:?}");
}

/// Every operation that addresses a monitored application refuses the SAME sentence when
/// no application answers to the resource group the request named — whether the group was
/// never created, was deleted, or merely resembles one that exists.
#[tokio::test]
async fn an_operation_on_an_unmonitored_group_is_refused() {
    let router = router();
    create_application(&router, "monitored-rg").await;
    create_application(&router, "deleted-rg").await;
    let (status, body) = call(
        &router,
        "DeleteApplication",
        json!({ "ResourceGroupName": "deleted-rg" }),
    )
    .await;
    assert_eq!(status, 200, "setup: DeleteApplication: {body:?}");

    // The groups: one never created, one created and then deleted, one whose name only
    // has a monitored group's name as a prefix, and the one-character and 256-character
    // ends of the modelled `ResourceGroupName` length.
    let groups = [
        "never-created",
        "deleted-rg",
        "monitored-rg-extra",
        "x",
        &"y".repeat(256),
    ];
    // Every implemented operation that resolves a resource group, with the rest of the
    // request each one needs.
    let operations: &[Operation] = &[
        ("DeleteApplication", |group| {
            json!({ "ResourceGroupName": group })
        }),
        ("DescribeApplication", |group| {
            json!({ "ResourceGroupName": group })
        }),
        ("ListComponents", |group| {
            json!({ "ResourceGroupName": group })
        }),
        ("DescribeComponent", |group| {
            json!({ "ResourceGroupName": group, "ComponentName": "web-tier" })
        }),
        ("CreateComponent", |group| {
            json!({
                "ResourceGroupName": group,
                "ComponentName": "web-tier",
                "ResourceList": [INSTANCE_ARN],
            })
        }),
    ];

    for group in groups {
        for (operation, request) in operations {
            let case = format!("{operation} on {group:?}");
            let (status, body) = call(&router, operation, request(group)).await;
            assert_refused(
                &case,
                status,
                &body,
                "ResourceNotFoundException",
                "Application does not exist",
            );
        }
    }

    // The monitored group is still served, so what the refusals turn on is the
    // application's absence and nothing about the requests themselves.
    let (status, body) = call(
        &router,
        "DescribeApplication",
        json!({ "ResourceGroupName": "monitored-rg" }),
    )
    .await;
    assert_eq!(status, 200, "the monitored group must be served: {body:?}");
}

/// A custom component groups standalone EC2 instances: a resource list carrying anything
/// else is refused with the service's fixed sentence, and nothing is stored.
#[tokio::test]
async fn a_custom_component_groups_only_ec2_instances() {
    let router = router();
    create_application(&router, "grouping-rg").await;

    // The refused rows span the resource types the `ApplicationComponent$ResourceType`
    // vocabulary names besides an instance ("EC2 instances, Auto Scaling group, Classic
    // ELB, Application ELB, and SQS Queue",
    // `smithy_query(query="shape",shape="ApplicationComponent")`), a type that
    // vocabulary does not name at all, an EC2 resource that is not an instance, a list
    // that mixes instances with one other resource, and the position of the offending
    // ARN in the list.
    let cases: &[(&str, &[&str], bool)] = &[
        ("one instance", &[INSTANCE_ARN], true),
        (
            "two instances",
            &[INSTANCE_ARN, OTHER_INSTANCE_ARN],
            true,
        ),
        (
            "an auto scaling group",
            &["arn:aws:autoscaling:us-east-1:000000000000:autoScalingGroup:uuid:autoScalingGroupName/my-asg"],
            false,
        ),
        (
            "a classic load balancer",
            &["arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/my-classic-lb"],
            false,
        ),
        (
            "an application load balancer",
            &["arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/my-alb/0123456789abcdef"],
            false,
        ),
        (
            "an SQS queue",
            &["arn:aws:sqs:us-east-1:000000000000:my-queue"],
            false,
        ),
        (
            "a Lambda function, a type the component vocabulary does not name",
            &["arn:aws:lambda:us-east-1:000000000000:function:my-fn"],
            false,
        ),
        (
            // An EC2 resource that is not an instance: the SERVICE is right and the
            // resource kind is not.
            "an EC2 volume",
            &["arn:aws:ec2:us-east-1:000000000000:volume/vol-0123456789abcdef0"],
            false,
        ),
        (
            "an instance first, something else after",
            &[INSTANCE_ARN, "arn:aws:sqs:us-east-1:000000000000:my-queue"],
            false,
        ),
        (
            "something else first, an instance after",
            &["arn:aws:sqs:us-east-1:000000000000:my-queue", INSTANCE_ARN],
            false,
        ),
    ];

    for (index, (case, resources, admitted)) in cases.iter().enumerate() {
        let component = format!("component-{index}");
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "grouping-rg",
                "ComponentName": component,
                "ResourceList": resources,
            }),
        )
        .await;

        let (read_status, read) = call(
            &router,
            "DescribeComponent",
            json!({ "ResourceGroupName": "grouping-rg", "ComponentName": component }),
        )
        .await;

        if *admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: grouping instances must be admitted, got {body:?}"
            );
            assert_eq!(
                read_status, 200,
                "case {case:?}: the component must be stored, got {read:?}"
            );
            assert_eq!(
                read["ResourceList"],
                json!(resources),
                "case {case:?}: the stored resource list must round-trip, got {read:?}"
            );
            assert_eq!(
                read["ApplicationComponent"]["ResourceType"],
                json!("AWS::EC2::Instance"),
                "case {case:?}: a component of instances reports their type, got {read:?}"
            );
        } else {
            assert_refused(
                case,
                status,
                &body,
                "ValidationException",
                "Only EC2 Instance can be grouped.",
            );
            assert_eq!(
                read_status, 400,
                "case {case:?}: a refused create must store nothing, got {read:?}"
            );
        }
    }
}

/// An operation addressing a component of an application that EXISTS is refused, naming
/// the component and the group it was asked for, when the application holds no component
/// of that name.
#[tokio::test]
async fn an_unknown_component_of_a_known_application_is_refused() {
    let router = router();
    // Two applications, so the refusal names the group FROM THE REQUEST rather than any
    // one of them.
    for group in ["left-rg", "right-rg"] {
        create_application(&router, group).await;
    }
    let (status, body) = call(
        &router,
        "CreateComponent",
        json!({
            "ResourceGroupName": "left-rg",
            "ComponentName": "web-tier",
            "ResourceList": [INSTANCE_ARN],
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateComponent: {body:?}");

    // The rows: a name nothing was ever created under, the name of a component of the
    // OTHER application, a name that only has an existing component's name as a prefix,
    // and the one-character and 128-character ends of the modelled `CustomComponentName`
    // length (1..128).
    let cases: &[(&str, &str)] = &[
        ("left-rg", "no-such-tier"),
        ("right-rg", "web-tier"),
        ("left-rg", "web-tier-extra"),
        ("left-rg", "c"),
        ("right-rg", "d"),
    ];

    for (group, component) in cases {
        let case = format!("DescribeComponent({group:?}, {component:?})");
        let (status, body) = call(
            &router,
            "DescribeComponent",
            json!({ "ResourceGroupName": group, "ComponentName": component }),
        )
        .await;
        assert_refused(
            &case,
            status,
            &body,
            "ResourceNotFoundException",
            &format!("Custom component {component} does not exist for {group}"),
        );
    }

    let longest = "e".repeat(128);
    let (status, body) = call(
        &router,
        "DescribeComponent",
        json!({ "ResourceGroupName": "left-rg", "ComponentName": longest }),
    )
    .await;
    assert_refused(
        "the longest component name the model admits",
        status,
        &body,
        "ResourceNotFoundException",
        &format!("Custom component {longest} does not exist for left-rg"),
    );

    // The component that WAS created is still served, so what the refusals turn on is
    // the component's absence.
    let (status, body) = call(
        &router,
        "DescribeComponent",
        json!({ "ResourceGroupName": "left-rg", "ComponentName": "web-tier" }),
    )
    .await;
    assert_eq!(status, 200, "the created component must be served: {body:?}");
}

/// A `NextToken` this service did not issue is refused by the payload-wrapping schema
/// the token declares, named with the operation that was called.
///
/// The schema id in each row is worked out by hand from the token's own text, not read
/// from the implementation: a pagination token is base64, so its first three characters
/// carry the two bytes the schema id is read from, big-endian and signed.
#[tokio::test]
async fn a_pagination_token_this_service_did_not_issue_is_refused() {
    let router = router();
    create_application(&router, "paged-rg").await;

    // "not" is base64 for the bytes 0x9E 0x8B (characters 39, 40, 45 of the alphabet),
    // and 0x9E8B read as a signed 16-bit big-endian integer is -24949. The remaining
    // characters are past the group the schema id comes from, so they do not change it.
    let not_a_token = "not-a-token-this-service-issued";
    // "616" is base64 for the bytes 0xEB 0x5E (characters 58, 53, 58), which read the
    // same way is -5282. Well-formed hex, so it decodes cleanly as one of this
    // service's own tokens would — and is still refused, because it carries none of the
    // payload this service writes.
    let forged_hex = "6162";

    let cases: &[(&str, &str, i32)] = &[
        ("ListApplications", not_a_token, -24949),
        ("ListApplications", forged_hex, -5282),
        ("ListComponents", not_a_token, -24949),
        ("ListComponents", forged_hex, -5282),
    ];

    for (operation, token, schema) in cases {
        let case = format!("{operation} with {token:?}");
        let mut request = json!({ "NextToken": token });
        if *operation == "ListComponents" {
            request["ResourceGroupName"] = json!("paged-rg");
        }
        let (status, body) = call(&router, operation, request).await;
        assert_refused(
            &case,
            status,
            &body,
            "ValidationException",
            &format!(
                "Pagination token exception in operation '{operation}': invalid payload \
                 wrapping schema {schema}"
            ),
        );
        // The layer that reads the token words its body with the lowercase `message`
        // member, which is what a client reads this refusal out of.
        assert!(
            body.get("message").and_then(Value::as_str).is_some(),
            "case {case:?}: expected a lowercase `message` member, got {body:?}"
        );
    }

    // A token this service DID issue still continues the listing it came from, so what
    // the refusals turn on is the payload the token carries.
    create_application(&router, "paged-rg-2").await;
    let (status, first) = call(&router, "ListApplications", json!({ "MaxResults": 1 })).await;
    assert_eq!(status, 200, "the first page must be served: {first:?}");
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first page must carry a token, got {first:?}"))
        .to_string();
    let (status, second) = call(&router, "ListApplications", json!({ "NextToken": token })).await;
    assert_eq!(status, 200, "the issued token must be accepted: {second:?}");
}

/// `AccountId` selects the resource-group owner a read is served for: the caller's own
/// account is served, and every other account is refused — on each operation that models
/// the member.
#[tokio::test]
async fn an_account_that_is_not_the_callers_is_refused() {
    let router = router();
    create_application(&router, "owned-rg").await;
    let (status, body) = call(
        &router,
        "CreateComponent",
        json!({
            "ResourceGroupName": "owned-rg",
            "ComponentName": "web-tier",
            "ResourceList": [INSTANCE_ARN],
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateComponent: {body:?}");

    // Each implemented operation that models `AccountId`, with the rest of the request
    // it needs.
    let operations: &[Operation] = &[
        ("ListApplications", |account| {
            json!({ "AccountId": account })
        }),
        ("DescribeApplication", |account| {
            json!({ "ResourceGroupName": "owned-rg", "AccountId": account })
        }),
        ("ListComponents", |account| {
            json!({ "ResourceGroupName": "owned-rg", "AccountId": account })
        }),
        ("DescribeComponent", |account| {
            json!({
                "ResourceGroupName": "owned-rg",
                "ComponentName": "web-tier",
                "AccountId": account,
            })
        }),
    ];

    // `AccountId` is exactly 12 digits (`^\d{12}$`), so every row below is a value the
    // boundary validator admits: the lowest, the highest, and one in between.
    let foreign = ["000000000001", "999999999999", "123456789012"];

    for (operation, request) in operations {
        // The caller's own account is served, whether it is named or left out.
        for account in [Some(account_id()), None] {
            let case = format!("{operation} for {account:?}");
            let mut body = request(account_id());
            if account.is_none() {
                body.as_object_mut()
                    .expect("the request is a JSON object")
                    .remove("AccountId");
            }
            let (status, answered) = call(&router, operation, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the caller's own account must be served, got {answered:?}"
            );
        }

        for account in foreign {
            let case = format!("{operation} for {account}");
            let (status, body) = call(&router, operation, request(account)).await;
            assert_refused(
                &case,
                status,
                &body,
                "ValidationException",
                &format!(
                    "Account {} is not able to access applications that belong to account \
                     {account}",
                    account_id()
                ),
            );
        }
    }
}
