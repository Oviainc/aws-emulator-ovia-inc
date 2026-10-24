// SPDX-License-Identifier: CC0-1.0

//! Server-side authorization of the ARN-addressed tagging surface, end to end:
//! `TagResource`, `UntagResource` and `ListTagsForResource`.
//!
//! The rule under test: each of these operations is allowed only when, at the instant of
//! the call, the policies in force allow THAT operation's own IAM action on the ARN of the
//! resource the request names — the calling principal's identity-based policies or the
//! addressed app monitor's resource-based policy, with an explicit `Deny` in either
//! refusing whatever the other says. Everything else is refused with
//! `AccessDeniedException` (HTTP 403), and a refusal leaves the resource's tag map exactly
//! as it was and reports none of it.
//!
//! The tables span what the rule generalises over: the operation and its action, where the
//! grant comes from (identity-based or resource-based), the action and resource patterns a
//! statement names, and the effect. Every row goes through the wire path — the
//! `ResourceArn` percent-encoded into one path segment the way an SDK encodes a non-greedy
//! `@httpLabel` — with the roles created through IAM's own API and the session credential
//! minted by STS's own `AssumeRole`, the same path a client takes.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::authorization::{
    identity_policy, monitor_arn, resource_policy, role_arn, statement, Emulator, ACCOUNT_KEY,
};
use crate::support::percent_encoded;

/// The status a successful `TagResource`/`UntagResource` answers with: 204 No Content
/// (`crate::state::TAGGING_NO_CONTENT_STATUS`).
const NO_CONTENT: u16 = 204;

/// The wire error code, and the status, a request IAM does not allow is refused with. The
/// same pair every other operation of this service refuses with — `AccessDeniedException`
/// is an AWS common error, which is why the three tagging operations' modelled error lists
/// do not have to declare it.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The tag map every app monitor below is created with.
fn baseline_tags() -> BTreeMap<String, String> {
    [("Team", "web")]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// One row of the operation table: a tagging operation, the IAM action it is authorized
/// against, a request that would otherwise succeed, and what the resource holds after an
/// ALLOWED call.
struct OperationCase {
    action: &'static str,
    method: &'static str,
    /// Appended to `/tags/{ResourceArn}`; `{key}` is replaced with a baseline tag key.
    query: &'static str,
    body: Option<Value>,
    /// The status the call answers when it is allowed.
    allowed_status: u16,
    /// The resource's tag map after an allowed call.
    after_allowed: &'static [(&'static str, &'static str)],
}

/// The three operations of this frontend, each with its own action.
fn operation_cases() -> Vec<OperationCase> {
    vec![
        OperationCase {
            action: "rum:TagResource",
            method: "POST",
            query: "",
            body: Some(json!({"Tags": {"Owner": "platform"}})),
            allowed_status: NO_CONTENT,
            after_allowed: &[("Owner", "platform"), ("Team", "web")],
        },
        OperationCase {
            action: "rum:UntagResource",
            method: "DELETE",
            query: "?tagKeys={key}",
            body: None,
            allowed_status: NO_CONTENT,
            after_allowed: &[],
        },
        OperationCase {
            action: "rum:ListTagsForResource",
            method: "GET",
            query: "",
            body: None,
            allowed_status: 200,
            after_allowed: &[("Team", "web")],
        },
    ]
}

/// A RUM action of this service that is NOT the row's own, so a grant is shown to be read
/// action by action rather than service by service.
fn another_action(action: &str) -> &'static str {
    if action == "rum:GetAppMonitor" {
        "rum:ListAppMonitors"
    } else {
        "rum:GetAppMonitor"
    }
}

/// `action` as a role-name fragment: IAM role names admit no `:`.
fn slug(action: &str) -> String {
    action.replace(':', "-").to_lowercase()
}

/// The request path for one row, against the resource `resource_arn` names.
fn path_for(row: &OperationCase, resource_arn: &str, tag_key: &str) -> String {
    format!(
        "/tags/{}{}",
        percent_encoded(resource_arn),
        row.query.replace("{key}", &percent_encoded(tag_key))
    )
}

/// The expected tag map of a row, as the stored map is read back.
fn expected_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Create one app monitor carrying [`baseline_tags`], with the account's own credential,
/// and answer its ARN.
async fn create_tagged_monitor(emulator: &Emulator, name: &str) -> String {
    let tags: serde_json::Map<String, Value> = baseline_tags()
        .into_iter()
        .map(|(key, value)| (key, Value::String(value)))
        .collect();
    let reply = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            "/appmonitor",
            Some(json!({
                "Name": name,
                "Domain": "example.com",
                "Tags": Value::Object(tags),
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: creating the app monitor must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    monitor_arn(name)
}

/// The tag map the stored app monitor holds, read with the account's own credential
/// through `GetAppMonitor` — a reader independent of the tagging surface under test, so a
/// refusal that wrote something could not hide it.
async fn stored_tags(emulator: &Emulator, case: &str, name: &str) -> BTreeMap<String, String> {
    let reply = emulator
        .rum(ACCOUNT_KEY, "GET", &format!("/appmonitor/{name}"), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: GetAppMonitor must answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    tag_map(&reply.json(case)["AppMonitor"], case)
}

/// The `Tags` of a reported shape as a map, with an omitted member read as the empty map
/// it stands for.
fn tag_map(reported: &Value, case: &str) -> BTreeMap<String, String> {
    match &reported["Tags"] {
        Value::Null => BTreeMap::new(),
        Value::Object(entries) => entries
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: a tag value must be a string, got {value}")
                        })
                        .to_string(),
                )
            })
            .collect(),
        other => panic!("case {case:?}: expected Tags to be a map, got {other}"),
    }
}

/// Every tagging operation evaluates ITS OWN action: a principal allowed exactly that
/// action may perform the operation, a principal allowed a different RUM action is refused
/// with `AccessDeniedException` (403), and the refusal leaves the tag map untouched.
#[tokio::test]
async fn each_tagging_operation_is_authorized_against_its_own_action() {
    for row in operation_cases() {
        let emulator = Emulator::new();
        let monitor = format!("per-action-{}", slug(row.action));
        let resource_arn = create_tagged_monitor(&emulator, &monitor).await;
        // The monitor carries a resource policy that grants nothing (it names a principal
        // nobody is), so the decisions below rest on the identity-based policies alone.
        emulator
            .put_resource_policy(
                &monitor,
                &resource_policy("Allow", &role_arn("nobody"), "rum:PutRumEvents", None),
            )
            .await;

        let allowed_key = emulator
            .assumed_role_key(
                &format!("allowed-{}", slug(row.action)),
                Some(&identity_policy(&statement("Allow", row.action, &resource_arn))),
            )
            .await;
        let other = another_action(row.action);
        let refused_key = emulator
            .assumed_role_key(
                &format!("refused-{}", slug(row.action)),
                Some(&identity_policy(&statement("Allow", other, "*"))),
            )
            .await;

        let key = baseline_tags()
            .keys()
            .next()
            .cloned()
            .expect("the baseline tag map has a key");
        let path = path_for(&row, &resource_arn, &key);

        let case = format!("{} refused for a principal allowed {other}", row.action);
        let reply = emulator
            .rum(&refused_key, row.method, &path, row.body.clone())
            .await;
        assert_eq!(
            reply.status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(&case),
            ACCESS_DENIED_CODE,
            "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let after_refusal = stored_tags(&emulator, &case, &monitor).await;
        assert_eq!(
            after_refusal,
            baseline_tags(),
            "case {case:?}: a refused call must leave the tag map untouched, got \
             {after_refusal:?}"
        );

        let case = format!("{} allowed for a principal allowed exactly it", row.action);
        let reply = emulator
            .rum(&allowed_key, row.method, &path, row.body.clone())
            .await;
        assert_eq!(
            reply.status,
            row.allowed_status,
            "case {case:?}: expected HTTP {}, got {} {:?}",
            row.allowed_status,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        // `ListTagsForResource` reports the map itself; the two writes are read back
        // through `GetAppMonitor`.
        if row.action == "rum:ListTagsForResource" {
            let reported = tag_map(&reply.json(&case), &case);
            assert_eq!(
                reported,
                expected_map(row.after_allowed),
                "case {case:?}: expected the reported tags to be \
                 {:?}, got {reported:?}",
                expected_map(row.after_allowed)
            );
        }
        let after_allowed = stored_tags(&emulator, &case, &monitor).await;
        assert_eq!(
            after_allowed,
            expected_map(row.after_allowed),
            "case {case:?}: expected the stored tags to be {:?}, got {after_allowed:?}",
            expected_map(row.after_allowed)
        );
    }
}

/// Where a row's identity-based grant comes from.
#[derive(Debug, Clone, Copy)]
enum Identity {
    /// The role holds no policy at all.
    None,
    /// `Allow` the operation's own action on the addressed monitor's ARN.
    AllowAction,
    /// `Allow` every action of this service on every resource.
    AllowServiceWildcard,
    /// `Allow` the operation's own action, but on a DIFFERENT monitor's ARN.
    AllowOnAnotherResource,
    /// `Allow` the action and, in the same document, explicitly `Deny` the service.
    AllowAndExplicitDeny,
}

/// What the addressed monitor's resource-based policy says about the calling role.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// The monitor carries no resource policy.
    None,
    /// It allows the calling role the operation's own action.
    AllowAction,
    /// It explicitly denies the calling role the operation's own action.
    ExplicitDeny,
}

/// The grant a tagging call needs is read from the policies in force, from either side,
/// and an explicit `Deny` in either refuses whatever the other says.
///
/// The rows differ along where the grant comes from, the action and resource patterns the
/// statement names, and the effect; every row is run for all three operations of the
/// frontend, because the rule is the same for each of them.
#[tokio::test]
async fn a_tagging_call_is_allowed_only_where_the_policies_in_force_allow_its_action() {
    // (case, the identity-based side, the resource-based side, allowed)
    let grants: [(&str, Identity, Resource, bool); 7] = [
        ("no policy in force at all", Identity::None, Resource::None, false),
        (
            "identity allows the action on the monitor's ARN",
            Identity::AllowAction,
            Resource::None,
            true,
        ),
        (
            "identity allows every action of the service on every resource",
            Identity::AllowServiceWildcard,
            Resource::None,
            true,
        ),
        (
            "identity allows the action on another monitor's ARN",
            Identity::AllowOnAnotherResource,
            Resource::None,
            false,
        ),
        (
            "identity allows the action and explicitly denies the service",
            Identity::AllowAndExplicitDeny,
            Resource::None,
            false,
        ),
        (
            "the monitor's resource policy allows the role, identity grants nothing",
            Identity::None,
            Resource::AllowAction,
            true,
        ),
        (
            "the resource policy explicitly denies while identity allows",
            Identity::AllowAction,
            Resource::ExplicitDeny,
            false,
        ),
    ];

    for (op_index, row) in operation_cases().iter().enumerate() {
        for (grant_index, (label, identity, resource, allowed)) in grants.iter().enumerate() {
            let case = format!("{} with {label}", row.action);
            let emulator = Emulator::new();
            let monitor = format!("grants-{op_index}-{grant_index}");
            let resource_arn = create_tagged_monitor(&emulator, &monitor).await;
            let role = format!("grant-{op_index}-{grant_index}");
            let caller = role_arn(&role);

            let document = match identity {
                Identity::None => None,
                Identity::AllowAction => Some(identity_policy(&statement(
                    "Allow",
                    row.action,
                    &resource_arn,
                ))),
                Identity::AllowServiceWildcard => {
                    Some(identity_policy(&statement("Allow", "rum:*", "*")))
                }
                Identity::AllowOnAnotherResource => Some(identity_policy(&statement(
                    "Allow",
                    row.action,
                    &monitor_arn("some-other-monitor"),
                ))),
                Identity::AllowAndExplicitDeny => Some(identity_policy(&format!(
                    "{},{}",
                    statement("Allow", row.action, &resource_arn),
                    statement("Deny", "rum:*", "*")
                ))),
            };
            let key = emulator
                .assumed_role_key(&role, document.as_deref())
                .await;
            match resource {
                Resource::None => {}
                Resource::AllowAction => {
                    emulator
                        .put_resource_policy(
                            &monitor,
                            &resource_policy("Allow", &caller, row.action, None),
                        )
                        .await
                }
                Resource::ExplicitDeny => {
                    emulator
                        .put_resource_policy(
                            &monitor,
                            &resource_policy("Deny", &caller, row.action, None),
                        )
                        .await
                }
            }

            let tag_key = baseline_tags()
                .keys()
                .next()
                .cloned()
                .expect("the baseline tag map has a key");
            let path = path_for(row, &resource_arn, &tag_key);
            let reply = emulator.rum(&key, row.method, &path, row.body.clone()).await;

            let expected_status = if *allowed {
                row.allowed_status
            } else {
                ACCESS_DENIED_STATUS
            };
            assert_eq!(
                reply.status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let expected_tags = if *allowed {
                expected_map(row.after_allowed)
            } else {
                assert_eq!(
                    reply.error_code(&case),
                    ACCESS_DENIED_CODE,
                    "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
                    String::from_utf8_lossy(&reply.body)
                );
                baseline_tags()
            };
            let after = stored_tags(&emulator, &case, &monitor).await;
            assert_eq!(
                after, expected_tags,
                "case {case:?}: expected the stored tags to be {expected_tags:?}, got {after:?}"
            );
        }
    }
}

/// A refused `ListTagsForResource` reports nothing of the resource: the 403 body carries
/// the refusal's own `message` and neither the `ResourceArn` nor any tag of the monitor
/// the request named.
#[tokio::test]
async fn a_refused_tag_read_reports_nothing_of_the_resource() {
    let emulator = Emulator::new();
    let monitor = "secret-tags";
    let resource_arn = create_tagged_monitor(&emulator, monitor).await;
    let key = emulator
        .assumed_role_key(
            "reader-of-nothing",
            Some(&identity_policy(&statement(
                "Allow",
                "rum:GetAppMonitor",
                "*",
            ))),
        )
        .await;

    let case = "ListTagsForResource refused for a principal allowed only rum:GetAppMonitor";
    let reply = emulator
        .rum(
            &key,
            "GET",
            &format!("/tags/{}", percent_encoded(&resource_arn)),
            None,
        )
        .await;
    assert_eq!(
        reply.status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    assert_eq!(
        body["Tags"],
        Value::Null,
        "case {case:?}: a refusal must report no tags, got {body}"
    );
    assert_eq!(
        body["ResourceArn"],
        Value::Null,
        "case {case:?}: a refusal must report no resource, got {body}"
    );
    let message = reply.message(case);
    assert!(
        message.contains("rum:ListTagsForResource"),
        "case {case:?}: the refusal must name the action it was made about, got \
         {message:?}"
    );
}
