// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteIndexPolicy`: removing the log-group level field index
//! policy one log group holds.
//!
//! The rule under test: a delete removes the log-group level index policy stored on the log
//! group its identifier names — whichever identifier form names it, whatever the stored
//! document says — and touches nothing else, so afterwards `DescribeIndexPolicies` reports
//! for that group whichever account-level `FIELD_INDEX_POLICY` applies to it and nothing at
//! all when none does, every other group keeps its own policy, and a later put stores a
//! policy again. The policy is the resource the operation deletes, so a log group the
//! account does not hold and a stored group holding no log-group level policy of its own —
//! including one only an account-level policy applies to, which this operation cannot
//! remove — are both refused as a missing resource, and a refused delete removes nothing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass, and what is stored is
//! read back over the wire through `DescribeIndexPolicies`.
//!
//! The tables below span what the rule generalises over (what the stored document says and
//! how many fields it names, which identifier form names the group, whether an
//! account-level policy also applies, and whether the group holds a policy at all).

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::log_group_arn;

use crate::account_policies::field_index_document;
use crate::harness::{Logs, REGION};
// The documents a put stores a policy with, and the read-back every case asserts through,
// shared with `tests/all/index_policy_puts.rs`: the removal cases span the same documents a
// put accepts and report them through the same projection, rather than a second copy of
// either.
use crate::index_policy_puts::{
    described_one, document_with_fields, fields_v2_document, member,
};

/// The operation under test.
const OP: &str = "DeleteIndexPolicy";

/// The refusal every missing resource is answered with. Source:
/// `smithy_query(query="operation", operation="DeleteIndexPolicy")` — the operation's
/// `ResourceNotFoundException`, "The specified resource does not exist."
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The account-level policy type that indexes fields. Source:
/// `smithy_query(query="shape", shape="PolicyType")`.
const FIELD_INDEX_POLICY: &str = "FIELD_INDEX_POLICY";

/// The `IndexPolicy$source` values. Source:
/// `smithy_query(query="shape", shape="IndexSource")` — the enum's two values.
const SOURCE_ACCOUNT: &str = "ACCOUNT";
const SOURCE_LOG_GROUP: &str = "LOG_GROUP";

/// How many field indexes one index policy can name. Source: `PutIndexPolicy` — "As many as
/// 20 fields can be included in the policy."
const MAX_FIELDS: usize = 20;

/// Create one log group.
async fn create_group(logs: &Logs, name: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name})).await;
}

/// Store a log-group level index policy on a group the way a client does.
async fn put_group_policy(logs: &Logs, identifier: &str, document: &str) {
    logs.ok(
        "PutIndexPolicy",
        &json!({"logGroupIdentifier": identifier, "policyDocument": document}),
    )
    .await;
}

/// Delete the log-group level index policy of the group `identifier` names, asserting the
/// response carries no member of its own (the modelled response is empty).
async fn delete_group_policy(logs: &Logs, case: &str, identifier: &str) {
    let response = logs.ok(OP, &json!({"logGroupIdentifier": identifier})).await;
    let empty = match &response {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case}: {OP} answers an empty response, got {response}"
    );
}

/// One row of the removal table: the case, the group it stores a policy on, and the document
/// that policy carries.
struct Row {
    case: &'static str,
    group: &'static str,
    document: String,
}

/// The documents a stored log-group level policy can carry: through `Fields`, through
/// `FieldsV2`, and at the policy's field quota.
fn removal_rows() -> Vec<Row> {
    vec![
        Row {
            case: "one field in Fields",
            group: "/index/remove-one-field",
            document: field_index_document(&["requestId"]),
        },
        Row {
            case: "three fields in Fields",
            group: "/index/remove-three-fields",
            document: field_index_document(&["requestId", "sessionId", "userId"]),
        },
        Row {
            case: "a FieldsV2 facet and field index",
            group: "/index/remove-fields-v2",
            document: fields_v2_document(&[("RequestId", "FIELD_INDEX"), ("APIName", "FACET")]),
        },
        Row {
            case: "twenty fields — the policy's field quota",
            group: "/index/remove-at-the-field-quota",
            document: document_with_fields(MAX_FIELDS),
        },
    ]
}

/// Whatever the stored policy says, a delete removes it: the group then reports no index
/// policy at all, and a put after the delete stores one again.
#[tokio::test]
async fn a_delete_removes_whatever_policy_the_group_holds() {
    let logs = Logs::new();

    for row in removal_rows() {
        create_group(&logs, row.group).await;
        put_group_policy(&logs, row.group, &row.document).await;
        let before = described_one(&logs, row.group)
            .await
            .unwrap_or_else(|| panic!("case {}: the group holds its own policy", row.case));
        assert_eq!(
            member(&before, "policyDocument"),
            row.document,
            "case {}: expected the stored document, got {before}",
            row.case
        );

        delete_group_policy(&logs, row.case, row.group).await;
        let after = described_one(&logs, row.group).await;
        assert!(
            after.is_none(),
            "case {}: no index policy applies once the group's own is deleted, got {after:?}",
            row.case
        );

        // The group is still there and still takes a policy, so the delete removed the
        // policy rather than the group.
        let again = field_index_document(&["afterDelete"]);
        put_group_policy(&logs, row.group, &again).await;
        let stored = described_one(&logs, row.group)
            .await
            .unwrap_or_else(|| panic!("case {}: a put after the delete stores a policy", row.case));
        assert_eq!(
            member(&stored, "policyDocument"),
            again,
            "case {}: expected the document put after the delete, got {stored}",
            row.case
        );
        assert_eq!(
            member(&stored, "source"),
            SOURCE_LOG_GROUP,
            "case {}: expected source {SOURCE_LOG_GROUP}, got {stored}",
            row.case
        );
    }
}

/// Either identifier form the modelled `LogGroupIdentifier` admits deletes the policy of the
/// one group it names.
#[tokio::test]
async fn either_identifier_form_deletes_the_same_log_groups_policy() {
    const GROUP: &str = "/index/remove-identifier-forms";

    let logs = Logs::new();
    create_group(&logs, GROUP).await;
    // The group's name, and the ARN form WITHOUT the trailing `:*`, which the modelled
    // pattern `^[\w#+=/:,.@-]*$` has no `*` for.
    let (_, arn) = logs.group_arns(GROUP).await;

    for (case, identifier) in [("name", GROUP.to_string()), ("arn", arn)] {
        put_group_policy(&logs, GROUP, &field_index_document(&[case])).await;
        delete_group_policy(&logs, case, &identifier).await;
        let after = described_one(&logs, GROUP).await;
        assert!(
            after.is_none(),
            "case {case}: the policy of the group this identifier names is gone, got {after:?}"
        );
    }
}

/// A delete removes the policy of the ONE group it names: every other group keeps its own.
#[tokio::test]
async fn a_delete_touches_only_the_group_it_names() {
    let rows = removal_rows();

    let logs = Logs::new();
    for row in &rows {
        create_group(&logs, row.group).await;
        put_group_policy(&logs, row.group, &row.document).await;
    }

    for (index, row) in rows.iter().enumerate() {
        delete_group_policy(&logs, row.case, row.group).await;
        for (other, kept) in rows.iter().enumerate() {
            let reported = described_one(&logs, kept.group).await;
            if other <= index {
                assert!(
                    reported.is_none(),
                    "case {}: this group's policy was deleted, got {reported:?}",
                    kept.case
                );
            } else {
                let reported = reported.unwrap_or_else(|| {
                    panic!(
                        "case {}: a group whose policy was not deleted keeps it",
                        kept.case
                    )
                });
                assert_eq!(
                    member(&reported, "policyDocument"),
                    kept.document,
                    "case {}: expected this group's own document, got {reported}",
                    kept.case
                );
            }
        }
    }
}

/// Deleting a group's own policy where an account-level `FIELD_INDEX_POLICY` also applies
/// leaves that account policy in place — it becomes what the group reports, and it cannot
/// itself be deleted through this operation.
#[tokio::test]
async fn an_account_level_policy_outlives_the_groups_own_and_is_not_deletable_here() {
    const HOLDER: &str = "/index/remove-with-account-policy";
    const WITHOUT_OWN: &str = "/index/remove-only-account-policy";
    const ACCOUNT_POLICY: &str = "applies-to-all";

    let logs = Logs::new();
    create_group(&logs, HOLDER).await;
    create_group(&logs, WITHOUT_OWN).await;
    let account_document = field_index_document(&["accountWide"]);
    logs.ok(
        "PutAccountPolicy",
        &json!({
            "policyName": ACCOUNT_POLICY,
            "policyType": FIELD_INDEX_POLICY,
            "policyDocument": account_document,
        }),
    )
    .await;

    let own_document = field_index_document(&["groupLevel"]);
    put_group_policy(&logs, HOLDER, &own_document).await;
    delete_group_policy(&logs, "with an account policy", HOLDER).await;

    let reported = described_one(&logs, HOLDER)
        .await
        .unwrap_or_else(|| panic!("case with an account policy: the account-wide policy applies"));
    assert_eq!(
        member(&reported, "source"),
        SOURCE_ACCOUNT,
        "case with an account policy: expected source {SOURCE_ACCOUNT}, got {reported}"
    );
    assert_eq!(
        member(&reported, "policyDocument"),
        account_document,
        "case with an account policy: expected the account policy's document, got {reported}"
    );
    assert_eq!(
        member(&reported, "policyName"),
        ACCOUNT_POLICY,
        "case with an account policy: expected the account policy's name, got {reported}"
    );

    // A group only the account-level policy applies to holds no policy of its own, so this
    // operation has none to delete and the account policy it reports survives the refusal.
    let code = logs
        .err(OP, &json!({"logGroupIdentifier": WITHOUT_OWN}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case only an account policy: an account-level policy is not one this operation deletes, got {code}"
    );
    let reported = described_one(&logs, WITHOUT_OWN)
        .await
        .unwrap_or_else(|| panic!("case only an account policy: the account-wide policy applies"));
    assert_eq!(
        member(&reported, "source"),
        SOURCE_ACCOUNT,
        "case only an account policy: a refused delete removes nothing, got {reported}"
    );
}

/// A delete the store cannot answer is a missing resource: a log group the account does not
/// hold, under either identifier form; a stored group that has never held a policy; and a
/// second delete of a policy already deleted.
#[tokio::test]
async fn a_delete_with_nothing_to_remove_is_refused() {
    const ABSENT: &str = "/index/remove-absent";
    const NEVER_HELD: &str = "/index/remove-never-held";
    const DELETED: &str = "/index/remove-twice";

    let logs = Logs::new();

    for (case, identifier) in [
        ("absent group by name", ABSENT.to_string()),
        (
            "absent group by arn",
            log_group_arn(REGION, ABSENT, false),
        ),
    ] {
        let code = logs
            .err(OP, &json!({"logGroupIdentifier": identifier}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case}: a log group the account does not hold is a missing resource, got {code}"
        );
    }

    create_group(&logs, NEVER_HELD).await;
    let code = logs
        .err(OP, &json!({"logGroupIdentifier": NEVER_HELD}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case never held: a group holding no index policy has none to delete, got {code}"
    );

    create_group(&logs, DELETED).await;
    put_group_policy(&logs, DELETED, &field_index_document(&["requestId"])).await;
    delete_group_policy(&logs, "first delete", DELETED).await;
    let code = logs.err(OP, &json!({"logGroupIdentifier": DELETED})).await;
    assert_eq!(
        code, NOT_FOUND,
        "case deleted twice: the policy is gone after the first delete, got {code}"
    );
}
