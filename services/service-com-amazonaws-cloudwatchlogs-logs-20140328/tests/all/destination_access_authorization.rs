// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHORIZATION of a subscription filter against a CloudWatch
//! Logs logical destination.
//!
//! RULE: a logical destination is authorized by the access policy attached to it and by
//! nothing else — "Through an access policy, a destination controls what is written to it.
//! By default, PutDestination does not set any access policy with the destination, which
//! means a cross-account user cannot call PutSubscriptionFilter against this destination. To
//! enable this, the destination owner must call PutDestinationPolicy after PutDestination"
//! (`PutDestination`). So a `PutSubscriptionFilter` naming a destination this account holds
//! that carries NO access policy is refused with `AccessDeniedException` and stores nothing,
//! and one naming a destination whose attached policy explicitly denies this caller the
//! action on it is refused the same way. The destination's own account reaches it through
//! its identity-based policies as well, so an attached policy that grants only another
//! account does not revoke the owner.
//!
//! The rows differ along what that rule generalises over: whether a policy is attached at
//! all, the EFFECT of its statements, the forms a `Principal` names this caller by, and
//! whether the statement's action and resource are about this request. Destinations that
//! carry no access policy at all — a stream, a delivery stream, a function, and a logical
//! destination belonging to another account, whose policy this account does not hold — are
//! the other half of the rule: nothing here refuses them.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// The operations these suites drive.
const PUT_FILTER: &str = "PutSubscriptionFilter";
const DESCRIBE_FILTERS: &str = "DescribeSubscriptionFilters";
const PUT_DESTINATION: &str = "PutDestination";
const PUT_POLICY: &str = "PutDestinationPolicy";
const CREATE_GROUP: &str = "CreateLogGroup";

/// The wire error code of an authorization refusal, as the AWS "Common Errors" table names
/// it.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The IAM action registering a subscription filter is authorized as.
const SUBSCRIBE_ACTION: &str = "logs:PutSubscriptionFilter";

/// Another account than the emulator's own, so a policy that grants or denies a FOREIGN
/// principal is distinguishable from one about this caller.
const OTHER_ACCOUNT: &str = "111122223333";

/// The account every ARN this emulator mints carries — the account the requests here are
/// signed for.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// An account's root ARN, the spelling IAM writes a whole account by.
fn account_root(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// An ARN in another account, of the form the owning service publishes.
fn other_account_arn(service: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{OTHER_ACCOUNT}:{resource}")
}

/// An ARN in the emulator's OWN account.
fn own_account_arn(service: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{}:{resource}", own_account())
}

/// The role CloudWatch Logs assumes to deliver to a streaming destination.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/CWLtoTargetRole", own_account())
}

/// One access policy document carrying `statements`.
fn policy(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One statement of an access policy.
fn statement(effect: &str, principal: Value, action: Value, resource: Value) -> Value {
    json!({
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": resource,
    })
}

/// A log group, created.
async fn create_group(logs: &Logs, group: &str) {
    logs.ok(CREATE_GROUP, &json!({"logGroupName": group})).await;
}

/// A logical destination, created over a Kinesis stream of this account, with NO access
/// policy attached ("By default, PutDestination does not set any access policy with the
/// destination"). Answers the ARN the destination reports for itself.
async fn create_destination(logs: &Logs, name: &str) -> String {
    let created = logs
        .ok(
            PUT_DESTINATION,
            &json!({
                "destinationName": name,
                "targetArn": own_account_arn("kinesis", "stream/log-events"),
                "roleArn": role_arn(),
            }),
        )
        .await;
    created
        .get("destination")
        .and_then(|destination| destination.get("arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{PUT_DESTINATION} reports the destination's arn, got {created}"))
        .to_string()
}

/// One `PutSubscriptionFilter` body; `role` is left out when it is `None`.
fn put_filter_body(group: &str, name: &str, destination: &str, role: Option<String>) -> Value {
    let mut body = json!({
        "logGroupName": group,
        "filterName": name,
        "filterPattern": "",
        "destinationArn": destination,
    });
    if let Some(role) = role {
        body["roleArn"] = json!(role);
    }
    body
}

/// The filter names the service reports for one log group.
async fn filter_names(logs: &Logs, group: &str) -> Vec<String> {
    let body = json!({"logGroupName": group});
    let response = logs.ok(DESCRIBE_FILTERS, &body).await;
    response
        .get("subscriptionFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("{DESCRIBE_FILTERS} {body}: must report subscriptionFilters, got {response}")
        })
        .iter()
        .filter_map(|filter| {
            filter
                .get("filterName")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

/// The sentence AWS refuses an unauthorized call with: the caller's account, the action it
/// asked for and the resource it asked for.
fn expected_message(resource_arn: &str) -> String {
    format!(
        "User with accountId: {} is not authorized to perform: {SUBSCRIBE_ACTION} on \
         resource: {resource_arn}",
        own_account()
    )
}

/// One row of the access-policy table: the document attached to the destination (or none at
/// all), and whether the owner's `PutSubscriptionFilter` against it is authorized.
struct Row {
    case: &'static str,
    /// The `accessPolicy` `PutDestinationPolicy` attaches, or `None` for a destination left
    /// as `PutDestination` created it.
    access_policy: Option<String>,
    /// Whether the put is accepted.
    authorized: bool,
}

/// Every form of attached policy the rule distinguishes, against one destination per row.
fn rows(destination_arn: &str) -> Vec<Row> {
    let own_root = account_root(&own_account());
    let other_root = account_root(OTHER_ACCOUNT);
    vec![
        Row {
            case: "no access policy attached at all",
            access_policy: None,
            authorized: false,
        },
        Row {
            case: "a grant to this account's root",
            access_policy: Some(policy(json!([statement(
                "Allow",
                json!({"AWS": &own_root}),
                json!(SUBSCRIBE_ACTION),
                json!(destination_arn),
            )]))),
            authorized: true,
        },
        Row {
            case: "a grant to this account by bare account id",
            access_policy: Some(policy(json!([statement(
                "Allow",
                json!({"AWS": own_account()}),
                json!(SUBSCRIBE_ACTION),
                json!(destination_arn),
            )]))),
            authorized: true,
        },
        Row {
            case: "a grant naming only another account",
            access_policy: Some(policy(json!([statement(
                "Allow",
                json!({"AWS": &other_root}),
                json!(SUBSCRIBE_ACTION),
                json!(destination_arn),
            )]))),
            authorized: true,
        },
        Row {
            case: "a grant to another account of an unrelated action",
            access_policy: Some(policy(json!([statement(
                "Allow",
                json!({"AWS": &other_root}),
                json!("logs:PutDestination"),
                json!("*"),
            )]))),
            authorized: true,
        },
        Row {
            case: "an explicit deny of this account's root for this action",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!({"AWS": &own_root}),
                json!(SUBSCRIBE_ACTION),
                json!(destination_arn),
            )]))),
            authorized: false,
        },
        Row {
            case: "an explicit deny of this account by bare account id",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!({"AWS": own_account()}),
                json!(SUBSCRIBE_ACTION),
                json!(destination_arn),
            )]))),
            authorized: false,
        },
        Row {
            case: "an explicit deny of this emulator's own principal",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!({"AWS": ::service_common::caller_principal::ambient_user_arn()}),
                json!(SUBSCRIBE_ACTION),
                json!(destination_arn),
            )]))),
            authorized: false,
        },
        Row {
            case: "an explicit deny of every principal, action and resource",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!("*"),
                json!("*"),
                json!("*"),
            )]))),
            authorized: false,
        },
        Row {
            case: "an explicit deny under an action wildcard",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!({"AWS": &own_root}),
                json!("logs:Put*"),
                json!(destination_arn),
            )]))),
            authorized: false,
        },
        Row {
            case: "a grant to this account beside a deny of it",
            access_policy: Some(policy(json!([
                statement(
                    "Allow",
                    json!({"AWS": &own_root}),
                    json!(SUBSCRIBE_ACTION),
                    json!(destination_arn),
                ),
                statement(
                    "Deny",
                    json!({"AWS": &own_root}),
                    json!(SUBSCRIBE_ACTION),
                    json!(destination_arn),
                ),
            ]))),
            authorized: false,
        },
        Row {
            case: "an explicit deny of this account for a different action",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!({"AWS": &own_root}),
                json!("logs:DeleteSubscriptionFilter"),
                json!(destination_arn),
            )]))),
            authorized: true,
        },
        Row {
            case: "an explicit deny of this account on a different resource",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!({"AWS": &own_root}),
                json!(SUBSCRIBE_ACTION),
                json!(own_account_arn("logs", "destination:another-destination")),
            )]))),
            authorized: true,
        },
        Row {
            case: "an explicit deny of another account only",
            access_policy: Some(policy(json!([statement(
                "Deny",
                json!({"AWS": &other_root}),
                json!(SUBSCRIBE_ACTION),
                json!(destination_arn),
            )]))),
            authorized: true,
        },
    ]
}

/// The access policy attached to a logical destination decides whether a subscription filter
/// may be registered against it: a destination carrying none authorizes nobody, a policy
/// that explicitly denies this caller the action on it refuses, and every other attached
/// policy leaves the owner authorized. A refused put stores NOTHING.
#[tokio::test]
async fn the_destination_access_policy_decides_whether_a_filter_may_be_registered() {
    let logs = Logs::new();
    let group = "authorization-group";
    create_group(&logs, group).await;

    // One destination per row, so a row's attached policy is the only thing deciding it, and
    // each row's policy names that destination's OWN ARN as the resource it is about. The
    // ARN is read off `PutDestination`, which is how a client learns one.
    let row_count = rows("arn:aws:logs:::destination:counted").len();
    for index in 0..row_count {
        let name = format!("authorization-destination-{index}");
        let destination_arn = create_destination(&logs, &name).await;
        let Row {
            case,
            access_policy,
            authorized,
        } = rows(&destination_arn)
            .into_iter()
            .nth(index)
            .unwrap_or_else(|| panic!("row {index} is in the table"));
        if let Some(document) = access_policy.as_deref() {
            logs.ok(
                PUT_POLICY,
                &json!({"destinationName": name, "accessPolicy": document}),
            )
            .await;
        }

        let filter = format!("filter-{index}");
        let body = put_filter_body(group, &filter, &destination_arn, None);
        if authorized {
            logs.ok(PUT_FILTER, &body).await;
            assert!(
                filter_names(&logs, group).await.contains(&filter),
                "case {case:?}: an authorized put attaches the filter, got {:?}",
                filter_names(&logs, group).await
            );
            logs.ok(
                "DeleteSubscriptionFilter",
                &json!({"logGroupName": group, "filterName": filter}),
            )
            .await;
        } else {
            let (code, message) = logs.refusal(PUT_FILTER, &body).await;
            assert_eq!(
                code, ACCESS_DENIED,
                "case {case:?}: an unauthorized put is refused as unauthorized, \
                 got {code} {message}"
            );
            assert_eq!(
                message,
                expected_message(&destination_arn),
                "case {case:?}: expected {:?}, got {message:?}",
                expected_message(&destination_arn)
            );
            let held = filter_names(&logs, group).await;
            assert_eq!(
                held,
                Vec::<String>::new(),
                "case {case:?}: a refused put stores nothing, got {held:?}"
            );
        }
    }
}

/// The refusal is about the DESTINATION's authorization, so attaching a policy turns the
/// identical put into an accepted one, and the filter it then creates reads back through
/// `DescribeSubscriptionFilters` with the destination it names.
#[tokio::test]
async fn attaching_an_access_policy_lets_the_same_put_through() {
    let logs = Logs::new();
    let group = "activation-group";
    create_group(&logs, group).await;
    let name = "activated-destination";
    let destination_arn = create_destination(&logs, name).await;
    let filter = "owner-filter";
    let body = put_filter_body(group, filter, &destination_arn, None);

    let (code, message) = logs.refusal(PUT_FILTER, &body).await;
    assert_eq!(
        code, ACCESS_DENIED,
        "case before the policy: the put is refused, got {code} {message}"
    );
    assert_eq!(
        message,
        expected_message(&destination_arn),
        "case before the policy: expected {:?}, got {message:?}",
        expected_message(&destination_arn)
    );

    // A policy naming ONLY a foreign account: the owner is authorized by its own identity,
    // so this is enough to make the destination usable by it.
    let document = policy(json!([statement(
        "Allow",
        json!({"AWS": account_root(OTHER_ACCOUNT)}),
        json!(SUBSCRIBE_ACTION),
        json!(&destination_arn),
    )]));
    logs.ok(
        PUT_POLICY,
        &json!({"destinationName": name, "accessPolicy": document}),
    )
    .await;

    logs.ok(PUT_FILTER, &body).await;
    let reported = logs
        .ok(DESCRIBE_FILTERS, &json!({"logGroupName": group}))
        .await;
    let found = reported
        .get("subscriptionFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case after the policy: expected filters, got {reported}"))
        .iter()
        .find(|held| held.get("filterName") == Some(&json!(filter)))
        .cloned()
        .unwrap_or_else(|| panic!("case after the policy: the filter is reported, got {reported}"));
    assert_eq!(
        found.get("destinationArn").and_then(Value::as_str),
        Some(destination_arn.as_str()),
        "case after the policy: the filter reports the destination it was pointed at, got {found}"
    );
}

/// The rule is about destinations whose access policy THIS account holds. A destination that
/// carries no access policy of its own — a Kinesis stream, a Firehose delivery stream, a
/// Lambda function — and a logical destination this account does not hold are not refused
/// here: nothing was read that could refuse them.
#[tokio::test]
async fn a_destination_this_account_holds_no_policy_for_is_not_refused() {
    let logs = Logs::new();
    let group = "unpoliced-group";
    create_group(&logs, group).await;
    let role = role_arn();

    // (case, destinationArn, roleArn) — a stream of either streaming vendor needs the role
    // CloudWatch Logs assumes; the other two kinds carry their own authorization.
    let rows: Vec<(&str, String, Option<String>)> = vec![
        (
            "a Kinesis stream of this account",
            own_account_arn("kinesis", "stream/log-events"),
            Some(role.clone()),
        ),
        (
            "a Firehose delivery stream of this account",
            own_account_arn("firehose", "deliverystream/log-events"),
            Some(role.clone()),
        ),
        (
            "a Lambda function of this account",
            own_account_arn("lambda", "function:log-consumer"),
            None,
        ),
        (
            "a logical destination of another account",
            other_account_arn("logs", "destination:cross-account-destination"),
            None,
        ),
        (
            "a logical destination this account never created",
            own_account_arn("logs", "destination:never-created"),
            None,
        ),
    ];

    for (index, (case, destination, role)) in rows.iter().enumerate() {
        let filter = format!("unpoliced-{index}");
        logs.ok(
            PUT_FILTER,
            &put_filter_body(group, &filter, destination, role.clone()),
        )
        .await;
        assert!(
            filter_names(&logs, group).await.contains(&filter),
            "case {case:?}: the put is accepted and the filter attached, got {:?}",
            filter_names(&logs, group).await
        );
        logs.ok(
            "DeleteSubscriptionFilter",
            &json!({"logGroupName": group, "filterName": filter}),
        )
        .await;
    }
}
