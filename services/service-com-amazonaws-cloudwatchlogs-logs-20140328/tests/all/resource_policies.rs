// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource-policy loop:
//! `PutResourcePolicy` / `DescribeResourcePolicies` / `DeleteResourcePolicy`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rules generalise over (policy name form,
//! document shape, scope, ARN form, page size, quota boundary); any single policy an
//! issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// AWS's documented quota, from `PutResourcePolicy`: "An account can have a maximum of
/// 10 policies without resourceARN and one per LogGroup resourceARN".
const MAX_ACCOUNT_POLICIES: usize = 10;

/// The published bounds of `DescribeResourcePolicies$limit`. Source: the `DescribeLimit`
/// shape, `range 1..50`.
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 50;

/// Account-scoped policy rows, differing in name form, principal and action count.
/// The actions are the ones `PutResourcePolicy` documents as supported
/// (`logs:PutLogEvents`, `logs:CreateLogStream`).
const ACCOUNT_POLICY_ROWS: &[(&str, &str, &[&str])] = &[
    (
        "route53-query-logging",
        "route53.amazonaws.com",
        &["logs:PutLogEvents", "logs:CreateLogStream"],
    ),
    ("es", "es.amazonaws.com", &["logs:PutLogEvents"]),
    (
        "nested/name.with_mixed-chars",
        "delivery.logs.amazonaws.com",
        &["logs:CreateLogStream"],
    ),
];

/// A resource policy document of the form `PutResourcePolicy$policyDocument` documents:
/// a statement allowing a service principal the actions the API supports.
fn policy_document(principal: &str, actions: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "EmulatorTestStatement",
            "Effect": "Allow",
            "Principal": {"Service": [principal]},
            "Action": actions,
            "Resource": "*",
        }]
    })
    .to_string()
}

/// The `policyName` values of a `DescribeResourcePolicies` page, in the order returned.
fn policy_names(page: &Value) -> Vec<String> {
    policies(page)
        .iter()
        .map(|policy| {
            policy
                .get("policyName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every policy carries a policyName, got {policy}"))
                .to_string()
        })
        .collect()
}

/// The `resourcePolicies` list of a `DescribeResourcePolicies` page.
fn policies(page: &Value) -> Vec<Value> {
    page.get("resourcePolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("DescribeResourcePolicies must return resourcePolicies, got {page}")
        })
        .clone()
}

/// A member of a returned policy, as a string.
fn member<'a>(policy: &'a Value, name: &str) -> Option<&'a str> {
    policy.get(name).and_then(Value::as_str)
}

/// Walk every page of one `DescribeResourcePolicies` request at `limit`, asserting each
/// page respects the limit, and answer the policy names in the order returned.
async fn walk(logs: &Logs, filters: &Value, limit: i64) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..256 {
        let mut body = filters.clone();
        body["limit"] = json!(limit);
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let page = logs.ok("DescribeResourcePolicies", &body).await;
        let returned = policies(&page);
        assert!(
            returned.len() <= usize::try_from(limit).expect("a positive limit"),
            "limit {limit} page {page_index}: expected at most {limit} policies, got {}",
            returned.len()
        );
        names.extend(policy_names(&page));
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("limit {limit}: pagination did not terminate, collected {names:?}");
}

/// Create a log group and answer its two ARN forms (`arn`, with the trailing `:*`, and
/// `logGroupArn`, without) as the service itself reports them.
async fn group_with_arns(logs: &Logs, group: &str) -> (String, String) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let page = logs
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupNamePrefix": group, "limit": LIMIT_MAX}),
        )
        .await;
    let found = page
        .get("logGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeLogGroups must return logGroups, got {page}"))
        .iter()
        .find(|entry| entry.get("logGroupName").and_then(Value::as_str) == Some(group))
        .unwrap_or_else(|| panic!("log group {group} must be readable back, got {page}"))
        .clone();
    (
        member(&found, "arn")
            .unwrap_or_else(|| panic!("a log group carries arn, got {found}"))
            .to_string(),
        member(&found, "logGroupArn")
            .unwrap_or_else(|| panic!("a log group carries logGroupArn, got {found}"))
            .to_string(),
    )
}

/// Every account-scoped put is readable back through `DescribeResourcePolicies`, with
/// the document echoed, the ACCOUNT scope, a live timestamp and no resource members —
/// over the whole class of accepted names, principals and action lists.
#[tokio::test]
async fn account_scoped_policies_round_trip() {
    let logs = Logs::new();
    let before = now_ms();

    // Put the rows in reverse name order, so the order Describe reports cannot be the
    // order they were written in.
    for (name, principal, actions) in ACCOUNT_POLICY_ROWS.iter().rev() {
        let put = logs
            .ok(
                "PutResourcePolicy",
                &json!({"policyName": name, "policyDocument": policy_document(principal, actions)}),
            )
            .await;
        let returned = put
            .get("resourcePolicy")
            .unwrap_or_else(|| panic!("case {name}: PutResourcePolicy must return the policy, got {put}"));
        assert_eq!(
            member(returned, "policyScope"),
            Some("ACCOUNT"),
            "case {name}: a policy put without resourceArn is ACCOUNT scoped, got {returned}"
        );
        assert!(
            put.get("revisionId").is_none(),
            "case {name}: revisionId is only returned for resource-scoped policies, got {put}"
        );
    }

    let page = logs.ok("DescribeResourcePolicies", &json!({})).await;
    let mut expected: Vec<&str> = ACCOUNT_POLICY_ROWS.iter().map(|(name, ..)| *name).collect();
    expected.sort_unstable();
    assert_eq!(
        policy_names(&page),
        expected,
        "case default scope: every account-scoped policy is listed in policy-name order, got {page}"
    );

    for (name, principal, actions) in ACCOUNT_POLICY_ROWS {
        let found = policies(&page)
            .into_iter()
            .find(|policy| member(policy, "policyName") == Some(name))
            .unwrap_or_else(|| panic!("case {name}: the policy must be listed, got {page}"));
        assert_eq!(
            member(&found, "policyDocument"),
            Some(policy_document(principal, actions).as_str()),
            "case {name}: the stored document is returned verbatim, got {found}"
        );
        assert_eq!(
            member(&found, "policyScope"),
            Some("ACCOUNT"),
            "case {name}: expected policyScope ACCOUNT, got {found}"
        );
        assert!(
            found.get("resourceArn").is_none() && found.get("revisionId").is_none(),
            "case {name}: resourceArn and revisionId are only populated for resource-scoped policies, got {found}"
        );
        let updated = found
            .get("lastUpdatedTime")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {name}: a policy carries lastUpdatedTime, got {found}"));
        assert!(
            updated >= before,
            "case {name}: expected lastUpdatedTime at or after the put ({before}), got {updated}"
        );
    }
}

/// A second put under one name is an update of that policy, not a second policy.
#[tokio::test]
async fn account_scoped_put_under_one_name_updates_in_place() {
    let logs = Logs::new();
    let name = "shared-name";
    for principal in ["route53.amazonaws.com", "es.amazonaws.com"] {
        logs.ok(
            "PutResourcePolicy",
            &json!({
                "policyName": name,
                "policyDocument": policy_document(principal, &["logs:PutLogEvents"]),
            }),
        )
        .await;
    }

    let page = logs.ok("DescribeResourcePolicies", &json!({})).await;
    assert_eq!(
        policy_names(&page),
        vec![name],
        "case repeat put: one name holds one policy, got {page}"
    );
    assert_eq!(
        member(&policies(&page)[0], "policyDocument"),
        Some(policy_document("es.amazonaws.com", &["logs:PutLogEvents"]).as_str()),
        "case repeat put: the latest document wins, got {page}"
    );
}

/// A resource-scoped put attaches the policy to the log group its ARN names, whichever
/// of the group's two ARN forms the request used, and is read back under the RESOURCE
/// scope — never under the ACCOUNT one.
#[tokio::test]
async fn resource_scoped_policies_round_trip() {
    let logs = Logs::new();

    // Rows differ in the ARN form the request names the group by, and in name shape.
    let groups = ["alpha-group", "nested/group.name_1", "zz"];
    let mut attached: Vec<(String, String, String)> = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let (wildcard_arn, plain_arn) = group_with_arns(&logs, group).await;
        // Half the rows name the `arn` form (trailing `:*`), half the `logGroupArn` one.
        let requested = if index % 2 == 0 {
            wildcard_arn.clone()
        } else {
            plain_arn.clone()
        };
        let put = logs
            .ok(
                "PutResourcePolicy",
                &json!({
                    "policyName": format!("{group}-policy"),
                    "policyDocument": policy_document("route53.amazonaws.com", &["logs:PutLogEvents"]),
                    "resourceArn": requested,
                }),
            )
            .await;
        let revision = member(&put, "revisionId")
            .unwrap_or_else(|| panic!("case {group}: a resource-scoped put returns revisionId, got {put}"))
            .to_string();
        let returned = put
            .get("resourcePolicy")
            .unwrap_or_else(|| panic!("case {group}: the put returns the policy, got {put}"));
        assert_eq!(
            member(returned, "policyScope"),
            Some("RESOURCE"),
            "case {group}: a policy put with resourceArn is RESOURCE scoped, got {returned}"
        );
        assert_eq!(
            member(returned, "resourceArn"),
            Some(plain_arn.as_str()),
            "case {group}: the attachment is reported as the log group's ARN, got {returned}"
        );
        assert_eq!(
            member(returned, "revisionId"),
            Some(revision.as_str()),
            "case {group}: the policy carries the revision the put returned, got {returned}"
        );
        attached.push((group.to_string(), requested, plain_arn));
    }

    // The ACCOUNT scope — the default — reports none of them.
    let account_page = logs.ok("DescribeResourcePolicies", &json!({})).await;
    assert_eq!(
        policy_names(&account_page),
        Vec::<String>::new(),
        "case default scope: a resource-scoped policy is not an account-scoped one, got {account_page}"
    );

    // The RESOURCE scope reports every one, in log-group order.
    let resource_page = logs
        .ok(
            "DescribeResourcePolicies",
            &json!({"policyScope": "RESOURCE"}),
        )
        .await;
    let mut expected: Vec<String> = groups.iter().map(|group| format!("{group}-policy")).collect();
    expected.sort();
    assert_eq!(
        policy_names(&resource_page),
        expected,
        "case RESOURCE scope: every attached policy is listed, got {resource_page}"
    );

    // And a request naming one resource reports exactly that resource's policy, by
    // either ARN form.
    for (group, requested, plain_arn) in &attached {
        for arn in [requested.as_str(), plain_arn.as_str()] {
            let page = logs
                .ok("DescribeResourcePolicies", &json!({"resourceArn": arn}))
                .await;
            assert_eq!(
                policy_names(&page),
                vec![format!("{group}-policy")],
                "case {arn}: a resourceArn selects the policy attached to that resource, got {page}"
            );
            // An explicit ACCOUNT scope is honoured as written: no account-scoped policy
            // is attached to a resource, so naming one selects none.
            let account_scoped = logs
                .ok(
                    "DescribeResourcePolicies",
                    &json!({"resourceArn": arn, "policyScope": "ACCOUNT"}),
                )
                .await;
            assert_eq!(
                policy_names(&account_scoped),
                Vec::<String>::new(),
                "case {arn} at ACCOUNT scope: an account-scoped policy is attached to no resource, got {account_scoped}"
            );
        }
    }
}

/// The revision guard: a first put must omit `expectedRevisionId`, every later put must
/// present the revision the policy is currently at, and each accepted put moves it on.
#[tokio::test]
async fn resource_scoped_put_is_guarded_by_the_revision_id() {
    let logs = Logs::new();
    let group = "guarded-group";
    let (_, arn) = group_with_arns(&logs, group).await;
    let document = policy_document("route53.amazonaws.com", &["logs:PutLogEvents"]);

    // "Use null when creating a resource policy for the first time."
    assert_eq!(
        logs.err(
            "PutResourcePolicy",
            &json!({
                "policyName": "p", "policyDocument": document, "resourceArn": arn,
                "expectedRevisionId": "00000000000000000000000000000000",
            }),
        )
        .await,
        "InvalidParameterException",
        "case create with a revision: a first put has no revision to expect"
    );

    let first = logs
        .ok(
            "PutResourcePolicy",
            &json!({"policyName": "p", "policyDocument": document, "resourceArn": arn}),
        )
        .await;
    let first_revision = member(&first, "revisionId")
        .expect("the create returns a revisionId")
        .to_string();

    // "Required when resourceArn is provided to prevent concurrent modifications."
    assert_eq!(
        logs.err(
            "PutResourcePolicy",
            &json!({"policyName": "p", "policyDocument": document, "resourceArn": arn}),
        )
        .await,
        "InvalidParameterException",
        "case update without a revision: an update must present the current revision"
    );
    for stale in ["ffffffffffffffffffffffffffffffff", "not-a-revision"] {
        assert_ne!(
            stale, first_revision,
            "case stale revision {stale}: the test's stale value must not be the live one"
        );
        assert_eq!(
            logs.err(
                "PutResourcePolicy",
                &json!({
                    "policyName": "p", "policyDocument": document, "resourceArn": arn,
                    "expectedRevisionId": stale,
                }),
            )
            .await,
            "OperationAbortedException",
            "case stale revision {stale}: a put at the wrong revision is a conflict"
        );
    }

    let updated_document = policy_document("es.amazonaws.com", &["logs:CreateLogStream"]);
    let second = logs
        .ok(
            "PutResourcePolicy",
            &json!({
                "policyName": "p", "policyDocument": updated_document, "resourceArn": arn,
                "expectedRevisionId": first_revision,
            }),
        )
        .await;
    let second_revision = member(&second, "revisionId")
        .expect("the update returns a revisionId")
        .to_string();
    assert_ne!(
        second_revision, first_revision,
        "case accepted update: an update moves the policy to a new revision"
    );

    let page = logs
        .ok(
            "DescribeResourcePolicies",
            &json!({"resourceArn": arn, "policyScope": "RESOURCE"}),
        )
        .await;
    let found = policies(&page)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the policy must still be attached, got {page}"));
    assert_eq!(
        member(&found, "policyDocument"),
        Some(updated_document.as_str()),
        "case accepted update: the stored document is the updated one, got {found}"
    );
    assert_eq!(
        member(&found, "revisionId"),
        Some(second_revision.as_str()),
        "case accepted update: the reported revision is the one the update returned, got {found}"
    );
    // One policy per LogGroup resourceARN: the update replaced, it did not add.
    assert_eq!(
        policies(&page).len(),
        1,
        "case accepted update: a log group holds one resource policy, got {page}"
    );
}

/// Pagination walks every policy exactly once at every page size, and a token is only a
/// position within the enumeration that issued it.
#[tokio::test]
async fn pagination_walks_every_policy_once() {
    let logs = Logs::new();
    let mut expected: Vec<String> = Vec::new();
    for index in 0..MAX_ACCOUNT_POLICIES {
        let name = format!("policy-{index:02}");
        logs.ok(
            "PutResourcePolicy",
            &json!({
                "policyName": name,
                "policyDocument": policy_document("route53.amazonaws.com", &["logs:PutLogEvents"]),
            }),
        )
        .await;
        expected.push(name);
    }
    expected.sort();

    // Page sizes spanning the published range: both bounds, and sizes in between that
    // do and do not divide the policy count.
    for limit in [LIMIT_MIN, 3, 4, 10, LIMIT_MAX] {
        assert_eq!(
            walk(&logs, &json!({}), limit).await,
            expected,
            "case limit {limit}: paging must return every policy exactly once, in order"
        );
    }

    // One past each published bound is refused by the modelled `range 1..50`.
    for limit in [LIMIT_MIN - 1, LIMIT_MAX + 1] {
        let (status, body) = logs
            .call("DescribeResourcePolicies", &json!({"limit": limit}))
            .await;
        assert_eq!(
            status, 400,
            "case limit {limit}: a limit outside 1..50 is refused, got {status} {body}"
        );
    }

    // A token that this operation did not mint, and one minted for a different
    // enumeration, are both refused rather than misread.
    assert_eq!(
        logs.err(
            "DescribeResourcePolicies",
            &json!({"nextToken": "not-a-token"}),
        )
        .await,
        "InvalidParameterException",
        "case forged token: an unreadable nextToken is refused"
    );
    let first = logs
        .ok("DescribeResourcePolicies", &json!({"limit": 1}))
        .await;
    let token = member(&first, "nextToken").expect("a short page carries a nextToken");
    assert_eq!(
        logs.err(
            "DescribeResourcePolicies",
            &json!({"limit": 1, "nextToken": token, "policyScope": "RESOURCE"}),
        )
        .await,
        "InvalidParameterException",
        "case refiltered token: a token belongs to the enumeration that issued it"
    );
}

/// The account-scoped quota: ten policies without a `resourceArn`, and an eleventh is
/// refused — while a put over a name already stored stays an update.
#[tokio::test]
async fn account_scoped_policy_quota_is_enforced() {
    let logs = Logs::new();
    let document = policy_document("route53.amazonaws.com", &["logs:PutLogEvents"]);
    for index in 0..MAX_ACCOUNT_POLICIES {
        logs.ok(
            "PutResourcePolicy",
            &json!({"policyName": format!("policy-{index:02}"), "policyDocument": document}),
        )
        .await;
    }
    assert_eq!(
        logs.err(
            "PutResourcePolicy",
            &json!({"policyName": "one-too-many", "policyDocument": document}),
        )
        .await,
        "LimitExceededException",
        "case the 11th policy: the account quota is {MAX_ACCOUNT_POLICIES}"
    );
    // At the quota, replacing an existing policy is still accepted.
    logs.ok(
        "PutResourcePolicy",
        &json!({
            "policyName": "policy-00",
            "policyDocument": policy_document("es.amazonaws.com", &["logs:PutLogEvents"]),
        }),
    )
    .await;
    assert_eq!(
        policy_names(&logs.ok("DescribeResourcePolicies", &json!({"limit": LIMIT_MAX})).await).len(),
        MAX_ACCOUNT_POLICIES,
        "case update at the quota: an update does not add a policy"
    );

    // Each log group carries its own policy, so the account quota does not bound them.
    for index in 0..MAX_ACCOUNT_POLICIES + 2 {
        let group = format!("group-{index:02}");
        let (_, arn) = group_with_arns(&logs, &group).await;
        logs.ok(
            "PutResourcePolicy",
            &json!({"policyName": group, "policyDocument": document, "resourceArn": arn}),
        )
        .await;
    }
    assert_eq!(
        walk(&logs, &json!({"policyScope": "RESOURCE"}), LIMIT_MAX)
            .await
            .len(),
        MAX_ACCOUNT_POLICIES + 2,
        "case per-resource policies: one per log group, independent of the account quota"
    );
}

/// A delete removes exactly the policy it names, is guarded by the revision for a
/// resource-scoped one, and deleting the log group revokes the policy attached to it.
#[tokio::test]
async fn delete_removes_the_policy_it_names() {
    let logs = Logs::new();
    let document = policy_document("route53.amazonaws.com", &["logs:PutLogEvents"]);
    for name in ["keep-me", "delete-me"] {
        logs.ok(
            "PutResourcePolicy",
            &json!({"policyName": name, "policyDocument": document}),
        )
        .await;
    }
    logs.ok(
        "DeleteResourcePolicy",
        &json!({"policyName": "delete-me"}),
    )
    .await;
    assert_eq!(
        policy_names(&logs.ok("DescribeResourcePolicies", &json!({})).await),
        vec!["keep-me"],
        "case account delete: only the named policy is revoked"
    );
    assert_eq!(
        logs.err(
            "DeleteResourcePolicy",
            &json!({"policyName": "delete-me"}),
        )
        .await,
        "ResourceNotFoundException",
        "case delete twice: a policy that is gone cannot be deleted again"
    );

    let group = "attached-group";
    let (_, arn) = group_with_arns(&logs, group).await;
    let put = logs
        .ok(
            "PutResourcePolicy",
            &json!({"policyName": "attached", "policyDocument": document, "resourceArn": arn}),
        )
        .await;
    let revision = member(&put, "revisionId")
        .expect("a resource-scoped put returns a revisionId")
        .to_string();
    assert_eq!(
        logs.err("DeleteResourcePolicy", &json!({"resourceArn": arn}))
            .await,
        "InvalidParameterException",
        "case resource delete without a revision: the revision is required"
    );
    assert_eq!(
        logs.err(
            "DeleteResourcePolicy",
            &json!({"resourceArn": arn, "expectedRevisionId": "ffffffffffffffffffffffffffffffff"}),
        )
        .await,
        "OperationAbortedException",
        "case resource delete at a stale revision: a delete at the wrong revision is a conflict"
    );
    logs.ok(
        "DeleteResourcePolicy",
        &json!({"resourceArn": arn, "expectedRevisionId": revision}),
    )
    .await;
    assert_eq!(
        policy_names(
            &logs
                .ok(
                    "DescribeResourcePolicies",
                    &json!({"policyScope": "RESOURCE"}),
                )
                .await
        ),
        Vec::<String>::new(),
        "case resource delete: the attachment is gone"
    );

    // Deleting the log group takes the policy attached to it with it.
    let second_put = logs
        .ok(
            "PutResourcePolicy",
            &json!({"policyName": "attached", "policyDocument": document, "resourceArn": arn}),
        )
        .await;
    assert!(
        member(&second_put, "revisionId").is_some(),
        "case re-put after delete: the resource takes a policy again, got {second_put}"
    );
    logs.ok("DeleteLogGroup", &json!({"logGroupName": group}))
        .await;
    assert_eq!(
        policy_names(
            &logs
                .ok(
                    "DescribeResourcePolicies",
                    &json!({"policyScope": "RESOURCE"}),
                )
                .await
        ),
        Vec::<String>::new(),
        "case log group deleted: the policy attached to it is revoked with it"
    );
}

/// What a put refuses: a missing or non-JSON `policyDocument`, a missing `policyName`,
/// an ARN that is not a log group's, and an ARN naming a log group that does not exist.
#[tokio::test]
async fn put_refuses_requests_it_cannot_store() {
    let logs = Logs::new();
    let document = policy_document("route53.amazonaws.com", &["logs:PutLogEvents"]);

    // "This is formatted as a JSON string" — a policy document is a JSON object.
    for bad in [
        "not json at all",
        "[]",
        "\"a string\"",
        "42",
        "{\"Statement\": ",
    ] {
        assert_eq!(
            logs.err(
                "PutResourcePolicy",
                &json!({"policyName": "p", "policyDocument": bad}),
            )
            .await,
            "InvalidParameterException",
            "case document {bad:?}: a policy document must be a JSON object"
        );
    }
    // And the documented minimum a document can be.
    logs.ok(
        "PutResourcePolicy",
        &json!({"policyName": "p", "policyDocument": "{}"}),
    )
    .await;

    assert_eq!(
        logs.err("PutResourcePolicy", &json!({"policyName": "p"}))
            .await,
        "InvalidParameterException",
        "case no document: policyDocument is required"
    );
    for missing_name in [json!({"policyDocument": document}), json!({"policyName": "", "policyDocument": document})] {
        assert_eq!(
            logs.err("PutResourcePolicy", &missing_name).await,
            "InvalidParameterException",
            "case {missing_name}: policyName is required"
        );
    }

    // A log group ARN is the only resource a policy attaches to: another service's ARN,
    // another CloudWatch Logs resource type, a log STREAM inside a group (which names a
    // group but is not one), and a bare name are all refused.
    for bad_arn in [
        "arn:aws:s3:::a-bucket",
        "arn:aws:logs:eu-west-2:000000000000:destination:a-destination",
        "arn:aws:logs:eu-west-2:000000000000:log-group:never-created:log-stream:a-stream",
        "not-an-arn",
    ] {
        assert_eq!(
            logs.err(
                "PutResourcePolicy",
                &json!({"policyName": "p", "policyDocument": document, "resourceArn": bad_arn}),
            )
            .await,
            "InvalidParameterException",
            "case resourceArn {bad_arn}: only a log group ARN can carry a resource policy"
        );
        // One member means one thing: the delete and the listing hold the same rule.
        assert_eq!(
            logs.err(
                "DeleteResourcePolicy",
                &json!({"resourceArn": bad_arn, "expectedRevisionId": "00000000000000000000000000000000"}),
            )
            .await,
            "InvalidParameterException",
            "case resourceArn {bad_arn}: a delete names a log group ARN too"
        );
        assert_eq!(
            logs.err("DescribeResourcePolicies", &json!({"resourceArn": bad_arn}))
                .await,
            "InvalidParameterException",
            "case resourceArn {bad_arn}: a listing filters on a log group ARN too"
        );
    }
    assert_eq!(
        logs.err(
            "PutResourcePolicy",
            &json!({
                "policyName": "p", "policyDocument": document,
                "resourceArn": "arn:aws:logs:eu-west-2:000000000000:log-group:never-created",
            }),
        )
        .await,
        "ResourceNotFoundException",
        "case unknown log group: a policy cannot be attached to a group that does not exist"
    );

    // A `resourceArn` that is not an ARN at all is refused with the service's
    // request-derived validation-error message, naming the member and the value it could
    // not parse — the same rule every ARN member of this service is held to. A
    // well-formed ARN of the wrong kind reports the kind instead (the rows above).
    for not_an_arn in ["not-an-arn", "logs.amazonaws.com", "arn:aws:logs"] {
        let (code, message) = logs
            .refusal(
                "PutResourcePolicy",
                &json!({"policyName": "p", "policyDocument": document, "resourceArn": not_an_arn}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case resourceArn {not_an_arn}: a value that is not an ARN is refused, got {code} {message}"
        );
        assert_eq!(
            message,
            format!("1 validation error detected: Unable to parse resourceArn {not_an_arn}"),
            "case resourceArn {not_an_arn}: the refusal names the member and the value, got {message}"
        );
    }

    // `policyScope` only takes the two values the model's `PolicyScope` enum declares,
    // so the boundary validator refuses any other before a handler sees it — with this
    // operation's own refusal error, `InvalidParameterException`.
    assert_eq!(
        logs.err(
            "DescribeResourcePolicies",
            &json!({"policyScope": "EVERYTHING"}),
        )
        .await,
        "InvalidParameterException",
        "case policyScope EVERYTHING: the scope is ACCOUNT or RESOURCE"
    );
}
