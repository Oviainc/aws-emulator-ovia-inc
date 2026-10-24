// SPDX-License-Identifier: CC0-1.0

//! The tag-key namespace Amazon Web Services reserves for itself, over the wire.
//!
//! The rule under test: a tag key beginning with `aws:` (however it is cased) names a tag
//! AWS generates, so no caller-supplied key may carry that prefix — every operation on
//! this surface that takes tag keys refuses such a request with the modelled
//! `ValidationException` (HTTP 400) and changes NOTHING, while a key that merely looks
//! like one (the letters without the colon, the prefix later in the key, a multi-byte
//! character where the prefix would end) is an ordinary key and is stored. The rule is
//! about a KEY, not about one operation or one resource type: `CreateDashboard` and
//! `CreateScheduledReport` take keys at create time, `TagResource` and `UntagResource`
//! take them afterwards, and `ResourceArn` names either a `dashboard` or a
//! `scheduled-report`.
//!
//! The reserved prefix itself comes from the handler-side constant
//! (`state::RESERVED_TAG_KEY_PREFIX`) rather than being re-spelled here, so the rows and
//! the rule cannot drift apart. The modelled `ResourceTagKey` pattern is `^[\S\s]*$`
//! (source: `smithy_query(query="shape",shape="ResourceTagKey")`), which admits every row
//! below — including the whitespace and multi-byte ones — so each one really does reach
//! the handler rather than being stopped by boundary validation.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSBCMDashboardsService.<Op>`, awsJson1_0), so what these tests assert
//! is what an SDK client sees, and the tag sets they compare come from
//! `ListTagsForResource` — the persisted record, not the request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;
use super::untag_resource::{create_dashboard_with_tags, pairs, stored_tags, tags_json};
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::RESERVED_TAG_KEY_PREFIX;

/// A well-formed `ResourceArn` (pattern
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:(dashboard|scheduled-report)/(\*|[-a-z0-9]+)$`,
/// source: `smithy_query(query="operation",operation="TagResource")`) that no resource in
/// a fresh store is keyed by.
const MISSING_RESOURCE_ARN: &str = "arn:aws:bcm-dashboards::123456789012:dashboard/no-such-board";

/// Keys inside the reserved namespace. Rows differ along the CASING of the prefix, what
/// follows it (nothing, a documented AWS key, whitespace, a multi-byte word) and the
/// key's length, because the rule is about the prefix alone.
///
/// Source for the namespace: AWS's tagging rules reserve the `aws:` prefix for the tags
/// AWS generates and state that a caller can neither create, edit nor delete such a tag;
/// the prefix itself is read from `RESERVED_TAG_KEY_PREFIX`.
fn reserved_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        ("the prefix as the whole key", prefix.to_string()),
        ("an AWS-generated key", format!("{prefix}created-by")),
        (
            "the prefix in upper case",
            format!("{}cost-center", prefix.to_uppercase()),
        ),
        ("the prefix capitalised", "Aws:cloudformation:stack-name".to_string()),
        ("the prefix in mixed case", "aWs:x".to_string()),
        ("a multi-byte remainder", format!("{prefix}propriétaire")),
        (
            "whitespace after the prefix, which the key pattern admits",
            format!("{prefix} created by"),
        ),
        (
            "the longest key the model admits (128 characters)",
            format!("{prefix}{}", "k".repeat(128 - prefix.len())),
        ),
    ]
}

/// Keys OUTSIDE the reserved namespace that resemble one. Rows differ along where the
/// prefix's letters sit and where the character boundaries fall: the last two put a
/// multi-byte character at or across the byte index the prefix ends at, which a
/// byte-slicing check would panic on.
fn unreserved_lookalikes() -> Vec<(&'static str, String)> {
    vec![
        ("the prefix without its colon", "aws".to_string()),
        ("the prefix's letters inside a longer word", "awsome".to_string()),
        ("a key that merely contains the prefix", "team:aws:owner".to_string()),
        ("the prefix after a space", " aws:owner".to_string()),
        ("the prefix letters with another separator", "aws-created-by".to_string()),
        (
            "a multi-byte character where the prefix's colon would be",
            "awsé".to_string(),
        ),
        (
            "a multi-byte character straddling the prefix's last byte",
            "awé:x".to_string(),
        ),
    ]
}

/// The call must be refused with HTTP 400 `ValidationException`, and the refusal must name
/// the member that broke the rule and the offending key — the message a validation refusal
/// built from the request carries.
async fn refused_as_validation(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
    member: &str,
    offending_key: &str,
) {
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status} with {response}"
    );
    assert!(
        error_code(&response).ends_with("ValidationException"),
        "case {case:?}: expected ValidationException from {operation}, got {} with {response}",
        error_code(&response)
    );
    let message = response["message"].as_str().unwrap_or("<no message>");
    assert!(
        message.contains(member) && message.contains(offending_key),
        "case {case:?}: the refusal from {operation} must name {member:?} and the key \
         {offending_key:?}, got {message:?}"
    );
}

/// Create a scheduled report on `dashboard` carrying `tags`, and return its ARN.
async fn create_report_with_tags(
    router: &MultiServiceRouter,
    dashboard: &str,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": name,
                "dashboardArn": dashboard,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
            },
            "resourceTags": tags_json(tags),
        }),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: CreateScheduledReport returned no arn: {created}"))
        .to_string()
}

/// Every ARN `ListScheduledReports` reports.
async fn listed_reports(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok_call(router, "ListScheduledReports", &json!({"maxResults": 100})).await;
    page["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListScheduledReports must answer a list: {page}"))
        .iter()
        .map(|item| item["arn"].as_str().unwrap_or("<none>").to_string())
        .collect()
}

/// Every ARN `ListDashboards` reports.
async fn listed_dashboards(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok_call(router, "ListDashboards", &json!({"maxResults": 100})).await;
    page["dashboards"]
        .as_array()
        .unwrap_or_else(|| panic!("ListDashboards must answer a list: {page}"))
        .iter()
        .map(|item| item["arn"].as_str().unwrap_or("<none>").to_string())
        .collect()
}

/// `TagResource` refuses a reserved key and stores NOTHING — not the reserved tag, and
/// not the ordinary tags of the same request. Checked on both kinds of resource a
/// `ResourceArn` names, with the reserved key alone and mixed in among allowed ones.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_stores_nothing() {
    for (label, key) in reserved_keys() {
        let router = router();
        let dashboard_tags = pairs(&[("env", "prod")]);
        let dashboard =
            create_dashboard_with_tags(&router, "Reserved Key Board", &dashboard_tags).await;
        let report_tags = pairs(&[("team", "finops")]);
        let report =
            create_report_with_tags(&router, &dashboard, "Reserved Key Report", &report_tags).await;

        for (kind, arn, initial) in [
            ("dashboard", &dashboard, &dashboard_tags),
            ("scheduled-report", &report, &report_tags),
        ] {
            // The reserved key alone, and the reserved key between two keys a caller may
            // set: either way the request is refused as a whole.
            let placements = [
                ("alone", vec![(key.clone(), "x".to_string())]),
                (
                    "between allowed keys",
                    vec![
                        ("owner".to_string(), "fin".to_string()),
                        (key.clone(), "x".to_string()),
                        ("tier".to_string(), "gold".to_string()),
                    ],
                ),
            ];
            for (placement, tags) in placements {
                let case = format!("{label} on a {kind}, {placement}");
                refused_as_validation(
                    &router,
                    "TagResource",
                    &json!({"resourceArn": arn, "resourceTags": tags_json(&tags)}),
                    &case,
                    "resourceTags",
                    &key,
                )
                .await;
                let stored = stored_tags(&router, arn, &case).await;
                assert_eq!(
                    &stored, initial,
                    "case {case:?}: a refused tagging must leave the stored tags unchanged, \
                     got {stored:?}"
                );
            }
        }
    }
}

/// A reserved key is refused from the REQUEST, before the ARN is resolved: the 400 wins
/// over the not-found answer a well-formed but unknown ARN draws.
#[tokio::test]
async fn a_reserved_key_is_refused_before_the_arn_is_resolved() {
    let router = router();
    for (label, key) in reserved_keys() {
        let case = format!("{label} on an absent resource");
        refused_as_validation(
            &router,
            "TagResource",
            &json!({
                "resourceArn": MISSING_RESOURCE_ARN,
                "resourceTags": tags_json(&[(key.clone(), "x".to_string())]),
            }),
            &case,
            "resourceTags",
            &key,
        )
        .await;
        refused_as_validation(
            &router,
            "UntagResource",
            &json!({"resourceArn": MISSING_RESOURCE_ARN, "resourceTagKeys": [key.clone()]}),
            &case,
            "resourceTagKeys",
            &key,
        )
        .await;
    }
}

/// `UntagResource` refuses a reserved key and removes NOTHING — a caller can no more
/// delete an AWS-generated tag than create one — while the same keys are removable once
/// the reserved one is out of the request.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_nothing() {
    for (label, key) in reserved_keys() {
        let router = router();
        let initial = pairs(&[("env", "prod"), ("owner", "fin")]);
        let dashboard = create_dashboard_with_tags(&router, "Reserved Untag Board", &initial).await;

        let requests = [
            ("alone", vec![key.clone()]),
            (
                "beside a removable key",
                vec!["env".to_string(), key.clone()],
            ),
            (
                "named twice",
                vec![key.clone(), key.clone(), "owner".to_string()],
            ),
        ];
        for (placement, keys) in requests {
            let case = format!("{label}, {placement}");
            refused_as_validation(
                &router,
                "UntagResource",
                &json!({"resourceArn": dashboard, "resourceTagKeys": keys}),
                &case,
                "resourceTagKeys",
                &key,
            )
            .await;
            let stored = stored_tags(&router, &dashboard, &case).await;
            assert_eq!(
                stored, initial,
                "case {case:?}: a refused removal must leave the stored tags unchanged, \
                 got {stored:?}"
            );
        }

        // The refusal is about the reserved key, not about the request: the same removal
        // without it goes through.
        ok_call(
            &router,
            "UntagResource",
            &json!({"resourceArn": dashboard, "resourceTagKeys": ["env"]}),
        )
        .await;
        let stored = stored_tags(&router, &dashboard, label).await;
        assert_eq!(
            stored,
            pairs(&[("owner", "fin")]),
            "case {label:?}: a removal naming no reserved key must remove exactly that key, \
             got {stored:?}"
        );
    }
}

/// Tag-on-create obeys the same rule: a reserved key in `resourceTags` creates NOTHING,
/// so the dashboard name stays free, no report ARN is minted, and the request's
/// `clientToken` is not consumed.
#[tokio::test]
async fn the_create_paths_refuse_a_reserved_key_and_create_nothing() {
    for (label, key) in reserved_keys() {
        let router = router();
        let source = create_dashboard_with_tags(&router, "Create Source", &[]).await;
        let dashboards_before = listed_dashboards(&router).await;
        let reports_before = listed_reports(&router).await;

        let case = format!("CreateDashboard with {label}");
        refused_as_validation(
            &router,
            "CreateDashboard",
            &json!({
                "name": "Tagged At Create",
                "widgets": [],
                "resourceTags": tags_json(&[
                    ("owner".to_string(), "fin".to_string()),
                    (key.clone(), "x".to_string()),
                ]),
            }),
            &case,
            "resourceTags",
            &key,
        )
        .await;
        let after = listed_dashboards(&router).await;
        assert_eq!(
            after, dashboards_before,
            "case {case:?}: a refused create must store no dashboard, got {after:?}"
        );

        let case = format!("CreateScheduledReport with {label}");
        let token = "idem-token-reserved";
        refused_as_validation(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": "Tagged At Create",
                    "dashboardArn": source,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                },
                "resourceTags": tags_json(&[(key.clone(), "x".to_string())]),
                "clientToken": token,
            }),
            &case,
            "resourceTags",
            &key,
        )
        .await;
        let after = listed_reports(&router).await;
        assert_eq!(
            after, reports_before,
            "case {case:?}: a refused create must store no scheduled report, got {after:?}"
        );

        // Nothing was created, so neither the dashboard name nor the client token was
        // consumed: the same request without the reserved key succeeds.
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({"name": "Tagged At Create", "widgets": [], "resourceTags": []}),
        )
        .await;
        assert!(
            created["arn"].is_string(),
            "case {case:?}: the name a refused create used must still be free, got {created}"
        );
        let created = ok_call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": "Tagged At Create",
                    "dashboardArn": source,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                },
                "clientToken": token,
            }),
        )
        .await;
        assert!(
            created["arn"].is_string(),
            "case {case:?}: the client token a refused create presented must still be free, \
             got {created}"
        );
    }
}

/// A key that only resembles the reserved namespace is an ORDINARY key: it is accepted on
/// every path and round-trips through `ListTagsForResource`, so the refusal is the prefix
/// and nothing wider. The multi-byte rows also pin that the check never slices a key
/// mid-character.
#[tokio::test]
async fn keys_outside_the_reserved_namespace_round_trip() {
    for (label, key) in unreserved_lookalikes() {
        let router = router();

        // Accepted at create time...
        let at_create = vec![(key.clone(), "created".to_string())];
        let dashboard =
            create_dashboard_with_tags(&router, "Lookalike Board", &at_create).await;
        let stored = stored_tags(&router, &dashboard, label).await;
        assert_eq!(
            stored, at_create,
            "case {label:?}: an ordinary key must be stored at create time, got {stored:?}"
        );

        // ... updated by TagResource ...
        ok_call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": dashboard,
                "resourceTags": tags_json(&[(key.clone(), "tagged".to_string())]),
            }),
        )
        .await;
        let stored = stored_tags(&router, &dashboard, label).await;
        assert_eq!(
            stored,
            vec![(key.clone(), "tagged".to_string())],
            "case {label:?}: an ordinary key must take the new value, got {stored:?}"
        );

        // ... and removable by UntagResource.
        ok_call(
            &router,
            "UntagResource",
            &json!({"resourceArn": dashboard, "resourceTagKeys": [key.clone()]}),
        )
        .await;
        let stored = stored_tags(&router, &dashboard, label).await;
        assert!(
            stored.is_empty(),
            "case {label:?}: an ordinary key must be removable, got {stored:?}"
        );
    }
}
