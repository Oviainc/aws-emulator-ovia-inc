// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeIndexPolicies`: the field index policy the service reports
//! for one log group.
//!
//! The rule under test: the policy reported for a log group is the log-group level policy
//! stored on that group when it holds one — rendering that group's ARN, the stored document
//! and its update time, with `source` `LOG_GROUP` and NO `policyName`, because a
//! log-group level policy has none — and otherwise the account-level `FIELD_INDEX_POLICY`
//! whose `selectionCriteria` select that group, rendering that policy's name, document and
//! update time with `source` `ACCOUNT`. A policy put without criteria selects every log
//! group in the account, a scoped policy selects the groups its filters name, and a group
//! that neither holds nor is selected by one reports no policy at all. A log group the
//! account does not hold is a missing resource rather than an empty answer.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span what the rule generalises over (which filter and operator the
//! criteria are written with, how many values the filter lists, where a policy is stored,
//! which identifier form names the group, and whether a group matches); the account-wide
//! policy an issue happens to cite is one row.
//!
//! A log-group level policy is stored the way a client stores one, through
//! `PutIndexPolicy`; what that operation itself is held to is
//! `tests/all/index_policy_puts.rs`. Everything these cases assert is read back over the
//! wire.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, ComAmazonawsCloudwatchlogsLogs20140328State, DataSourceRecord,
};

use crate::account_policies::{field_index_document, put_body};
use crate::harness::{now_ms, Logs, REGION};

/// The operation every case calls.
const OP: &str = "DescribeIndexPolicies";

/// The account-level policy type that indexes fields, from the model's `PolicyType` enum.
/// Source: `smithy_query(query="shape", shape="PolicyType")`.
const FIELD_INDEX_POLICY: &str = "FIELD_INDEX_POLICY";

/// The `IndexPolicy$source` of an account-level policy. Source:
/// `smithy_query(query="shape", shape="IndexSource")` — the enum's two values are `ACCOUNT`
/// and `LOG_GROUP`.
const SOURCE_ACCOUNT: &str = "ACCOUNT";

/// The `IndexPolicy$source` of a policy stored on the one log group it applies to. See
/// [`SOURCE_ACCOUNT`].
const SOURCE_LOG_GROUP: &str = "LOG_GROUP";

/// The log groups every criteria row is judged against: two under distinct `/aws/...`
/// prefixes and one under no `/aws/` prefix at all.
const GROUPS: [&str; 3] = ["/aws/index-a/one", "/aws/other/two", "app/three"];

/// One string member of a reported `IndexPolicy`.
fn member(policy: &Value, key: &str) -> String {
    policy
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an index policy must carry {key}, got {policy}"))
        .to_string()
}

/// The `lastUpdateTime` of a reported `IndexPolicy`.
fn last_update_time(policy: &Value) -> i64 {
    policy
        .get("lastUpdateTime")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("an index policy must carry lastUpdateTime, got {policy}"))
}

/// The policies one request reports, asserting the operation reports them in one page (it
/// mints no `nextToken`: one log group has at most one index policy).
async fn described(logs: &Logs, identifier: &str) -> Vec<Value> {
    let body = json!({"logGroupIdentifiers": [identifier]});
    let page = logs.ok(OP, &body).await;
    assert!(
        page.get("nextToken").is_none(),
        "case {identifier}: {OP} reports one page, got a nextToken in {page}"
    );
    page.get("indexPolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {identifier}: {OP} must return indexPolicies, got {page}"))
        .clone()
}

/// The one policy reported for a log group, or `None` when none applies to it.
async fn described_one(logs: &Logs, identifier: &str) -> Option<Value> {
    let policies = described(logs, identifier).await;
    assert!(
        policies.len() <= 1,
        "case {identifier}: at most one index policy applies to one log group, got {policies:?}"
    );
    policies.into_iter().next()
}

/// Create the log groups `names` names.
async fn create_groups(logs: &Logs, names: &[&str]) {
    for name in names {
        logs.ok("CreateLogGroup", &json!({"logGroupName": name})).await;
    }
}

/// Put one account-level field index policy, scoped by `criteria` when one is given, whose
/// document names `case` as its one field index so a policy mixed up with another cannot
/// pass.
async fn put_account_policy(logs: &Logs, case: &str, criteria: Option<&str>) -> String {
    let document = field_index_document(&[case]);
    logs.ok(
        "PutAccountPolicy",
        &put_body(case, FIELD_INDEX_POLICY, &document, criteria),
    )
    .await;
    document
}

/// Store a log-group level index policy on a group the way a client does.
async fn put_group_policy(logs: &Logs, group: &str, document: &str) {
    logs.ok(
        "PutIndexPolicy",
        &json!({"logGroupIdentifier": group, "policyDocument": document}),
    )
    .await;
}

/// Store the data sources `sources` names — `(name, type)` pairs — on a stored group. No
/// operation associates a data source with a log group yet, and `DataSource$name` /
/// `DataSource$type` target plain strings, so any pair does: what the case is about is
/// which groups a data-source-scoped policy selects.
fn seed_data_sources(logs: &Logs, group: &str, sources: &[(&str, &str)]) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let record = store
        .log_groups
        .get_mut(group)
        .unwrap_or_else(|| panic!("case {group}: the log group must be stored"));
    record.data_sources = sources
        .iter()
        .map(|(name, source_type)| DataSourceRecord {
            name: (*name).to_string(),
            source_type: Some((*source_type).to_string()),
            format: None,
        })
        .collect();
}

/// Assert that `identifier` reports the account-level policy `case` with `document`, and
/// that it reports the group's own ARN and an update time inside `[put_at, now]`.
async fn assert_reports_account_policy(
    logs: &Logs,
    case: &str,
    identifier: &str,
    document: &str,
    put_at: i64,
) {
    let (_, log_group_arn) = logs.group_arns(identifier).await;
    let policy = described_one(logs, identifier)
        .await
        .unwrap_or_else(|| panic!("case {case}/{identifier}: an account-wide policy applies"));
    assert_eq!(
        member(&policy, "source"),
        SOURCE_ACCOUNT,
        "case {case}/{identifier}: expected source {SOURCE_ACCOUNT}, got {policy}"
    );
    assert_eq!(
        member(&policy, "policyName"),
        case,
        "case {case}/{identifier}: expected the account policy's name, got {policy}"
    );
    assert_eq!(
        member(&policy, "policyDocument"),
        document,
        "case {case}/{identifier}: expected the stored document, got {policy}"
    );
    assert_eq!(
        member(&policy, "logGroupIdentifier"),
        log_group_arn,
        "case {case}/{identifier}: expected the log group's ARN, got {policy}"
    );
    let stamped = last_update_time(&policy);
    assert!(
        (put_at..=now_ms()).contains(&stamped),
        "case {case}/{identifier}: expected an update time in [{put_at}, now], got {stamped}"
    );
}

/// An account-level policy is reported for exactly the log groups its `selectionCriteria`
/// select: every group when it has none, and the groups its filter names when it has one —
/// one prefix, several prefixes, a prefix that is a whole name, an excluded prefix, and a
/// prefix no group starts with.
#[tokio::test]
async fn an_account_policy_is_reported_for_exactly_the_groups_its_criteria_select() {
    // Each row: the case, the criteria the policy is put with, and which of [`GROUPS`] it
    // then applies to.
    let rows: Vec<(&str, Option<&str>, [bool; 3])> = vec![
        ("applies-to-all", None, [true, true, true]),
        (
            "prefix-in-one-value",
            Some(r#"LogGroupNamePrefix IN ["/aws/index-a"]"#),
            [true, false, false],
        ),
        (
            "prefix-in-two-values",
            Some(r#"LogGroupNamePrefix IN ["/aws/index-a", "app"]"#),
            [true, false, true],
        ),
        (
            "prefix-in-a-whole-name",
            Some(r#"LogGroupNamePrefix IN ["/aws/other/two"]"#),
            [false, true, false],
        ),
        (
            "prefix-not-in",
            Some(r#"LogGroupNamePrefix NOT IN ["/aws/index-a"]"#),
            [false, true, true],
        ),
        (
            "prefix-no-group-starts-with",
            Some(r#"LogGroupNamePrefix IN ["/nowhere"]"#),
            [false, false, false],
        ),
    ];

    for (case, criteria, applies) in rows {
        // One account holds one policy per row, so what a group reports is attributable to
        // that row's criteria alone.
        let logs = Logs::new();
        create_groups(&logs, &GROUPS).await;
        let put_at = now_ms();
        let document = put_account_policy(&logs, case, criteria).await;

        for (group, applies) in GROUPS.iter().zip(applies) {
            if applies {
                assert_reports_account_policy(&logs, case, group, &document, put_at).await;
            } else {
                let reported = described_one(&logs, group).await;
                assert!(
                    reported.is_none(),
                    "case {case}/{group}: the criteria do not select this group, got {reported:?}"
                );
            }
        }
    }
}

/// One row of the data-source table: a log group, the `(name, type)` data sources stored
/// on it, and whether a policy scoped to one name-and-type pair selects that group.
struct DataSourceRow {
    group: &'static str,
    sources: Vec<(&'static str, &'static str)>,
    selected: bool,
}

/// A data-source-scoped policy is reported for exactly the log groups carrying a data source
/// of the name AND the type its criteria name together.
#[tokio::test]
async fn a_data_source_scoped_policy_is_reported_for_the_groups_with_that_data_source() {
    const NAME: &str = "orders";
    const TYPE: &str = "ServiceLog";

    let rows = vec![
        DataSourceRow {
            group: "ds/both-match",
            sources: vec![(NAME, TYPE)],
            selected: true,
        },
        DataSourceRow {
            group: "ds/another-type",
            sources: vec![(NAME, "AccessLog")],
            selected: false,
        },
        DataSourceRow {
            group: "ds/another-name",
            sources: vec![("payments", TYPE)],
            selected: false,
        },
        // The name and the type must be carried by ONE data source: "DataSourceName and
        // DataSourceType together" name one source, not two.
        DataSourceRow {
            group: "ds/split-across-sources",
            sources: vec![(NAME, "AccessLog"), ("payments", TYPE)],
            selected: false,
        },
        DataSourceRow {
            group: "ds/one-of-several-matches",
            sources: vec![("payments", "AccessLog"), (NAME, TYPE)],
            selected: true,
        },
        DataSourceRow {
            group: "ds/no-data-sources",
            sources: Vec::new(),
            selected: false,
        },
    ];

    let logs = Logs::new();
    let names: Vec<&str> = rows.iter().map(|row| row.group).collect();
    create_groups(&logs, &names).await;
    for row in &rows {
        seed_data_sources(&logs, row.group, &row.sources);
    }

    let case = "data-source-scoped";
    let put_at = now_ms();
    let criteria = format!(r#"DataSourceName IN ["{NAME}"] AND DataSourceType IN ["{TYPE}"]"#);
    let document = put_account_policy(&logs, case, Some(&criteria)).await;

    for row in &rows {
        if row.selected {
            assert_reports_account_policy(&logs, case, row.group, &document, put_at).await;
        } else {
            let reported = described_one(&logs, row.group).await;
            assert!(
                reported.is_none(),
                "case {case}/{}: the criteria do not select this group, got {reported:?}",
                row.group
            );
        }
    }
}

/// A log group holding its own index policy reports that policy — under every identifier
/// form that names the group — rather than the account-wide one, and reports it without a
/// `policyName`; a group holding none still reports the account-wide policy.
#[tokio::test]
async fn a_log_group_level_policy_is_reported_ahead_of_an_account_wide_one() {
    const HOLDER: &str = "/aws/index-a/one";
    const OTHER: &str = "app/three";

    let logs = Logs::new();
    create_groups(&logs, &[HOLDER, OTHER]).await;
    let put_at = now_ms();
    let account_document = put_account_policy(&logs, "applies-to-all", None).await;

    let own_document = field_index_document(&["requestId", "sessionId"]);
    let own_put_at = now_ms();
    put_group_policy(&logs, HOLDER, &own_document).await;

    // The two identifier forms the modelled `LogGroupIdentifier` pattern
    // (`^[\w#+=/:,.@-]*$`) admits: the group's name, and the ARN form WITHOUT the trailing
    // `:*`, which that pattern has no `*` for.
    let (_, arn) = logs.group_arns(HOLDER).await;
    for (case, identifier) in [("name", HOLDER.to_string()), ("arn", arn.clone())] {
        let policy = described_one(&logs, &identifier)
            .await
            .unwrap_or_else(|| panic!("case {case}: the group's own policy is reported"));
        assert_eq!(
            member(&policy, "source"),
            SOURCE_LOG_GROUP,
            "case {case}: expected source {SOURCE_LOG_GROUP}, got {policy}"
        );
        assert!(
            policy.get("policyName").is_none(),
            "case {case}: a log-group level policy has no name, got {policy}"
        );
        assert_eq!(
            member(&policy, "policyDocument"),
            own_document,
            "case {case}: expected the group's own document, got {policy}"
        );
        assert_eq!(
            member(&policy, "logGroupIdentifier"),
            arn,
            "case {case}: expected the log group's ARN, got {policy}"
        );
        let stamped = last_update_time(&policy);
        assert!(
            (own_put_at..=now_ms()).contains(&stamped),
            "case {case}: expected an update time in [{own_put_at}, now], got {stamped}"
        );
    }

    // The other group holds none of its own, so the account-wide policy is what applies.
    assert_reports_account_policy(&logs, "applies-to-all", OTHER, &account_document, put_at).await;
}

/// Where an account-wide policy and a scoped one both select a group, the scoped one is the
/// policy reported for it; a group only the account-wide policy selects reports that one.
#[tokio::test]
async fn a_scoped_account_policy_is_reported_ahead_of_the_account_wide_one() {
    const SCOPED_GROUP: &str = "/aws/index-a/one";
    const UNSCOPED_GROUP: &str = "app/three";

    let logs = Logs::new();
    create_groups(&logs, &[SCOPED_GROUP, UNSCOPED_GROUP]).await;
    let put_at = now_ms();
    let account_document = put_account_policy(&logs, "applies-to-all", None).await;
    let scoped_document = put_account_policy(
        &logs,
        "scoped-to-a-prefix",
        Some(r#"LogGroupNamePrefix IN ["/aws/index-a"]"#),
    )
    .await;

    assert_reports_account_policy(
        &logs,
        "scoped-to-a-prefix",
        SCOPED_GROUP,
        &scoped_document,
        put_at,
    )
    .await;
    assert_reports_account_policy(
        &logs,
        "applies-to-all",
        UNSCOPED_GROUP,
        &account_document,
        put_at,
    )
    .await;
}

/// A policy deleted from the account stops being reported for the groups it selected, and a
/// group's own policy outlives it.
#[tokio::test]
async fn a_deleted_account_policy_is_no_longer_reported() {
    const GROUP: &str = "/aws/index-a/one";

    let logs = Logs::new();
    create_groups(&logs, &[GROUP]).await;
    let put_at = now_ms();
    let document = put_account_policy(&logs, "applies-to-all", None).await;
    assert_reports_account_policy(&logs, "applies-to-all", GROUP, &document, put_at).await;

    logs.ok(
        "DeleteAccountPolicy",
        &json!({"policyName": "applies-to-all", "policyType": FIELD_INDEX_POLICY}),
    )
    .await;
    let reported = described_one(&logs, GROUP).await;
    assert!(
        reported.is_none(),
        "case deleted: no policy applies once the account policy is gone, got {reported:?}"
    );
}

/// A log group the account does not hold is a missing resource, under every identifier form,
/// and a `nextToken` this operation never minted is refused as an invalid parameter.
#[tokio::test]
async fn a_request_the_store_cannot_answer_is_refused() {
    const GROUP: &str = "/aws/index-a/one";
    const ABSENT: &str = "/aws/index-a/absent";

    let logs = Logs::new();
    create_groups(&logs, &[GROUP]).await;
    put_account_policy(&logs, "applies-to-all", None).await;

    for (case, identifier) in [
        ("name", ABSENT.to_string()),
        ("arn", log_group_arn(REGION, ABSENT, false)),
    ] {
        let code = logs
            .err(OP, &json!({"logGroupIdentifiers": [identifier]}))
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: a log group the account does not hold is a missing resource, got {code}"
        );
    }

    for (case, token) in [
        ("not-a-token", "not-a-token"),
        ("hex-token", "deadbeef"),
    ] {
        let (code, message) = logs
            .refusal(
                OP,
                &json!({"logGroupIdentifiers": [GROUP], "nextToken": token}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: a token this operation never minted is an invalid parameter, got {code}"
        );
        assert!(
            message.contains(token),
            "case {case}: the refusal names the token it was given, got {message}"
        );
    }
}
