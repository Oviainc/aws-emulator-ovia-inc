// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the custom-insight plane: CreateInsight →
//! GetInsightResults, over findings BatchImportFindings persisted.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter` (the
//! harness is the one `security_hub_core_loop` builds), so a route, a status code or a
//! serialized member that is wrong on the wire fails here.

use serde_json::{json, Value};

use crate::security_hub_core_loop::{base_finding, returned_ids, Harness, ACCOUNT_ID, REGION};

/// `CreateInsight`: `POST /insights`.
async fn create_insight(harness: &Harness, body: Value) -> (u16, Value) {
    harness.call("POST", "/insights", Some(body)).await
}

/// `GetInsightResults`: `GET /insights/results/{InsightArn+}`. The greedy label carries
/// the ARN's own colons and slashes, exactly as an SDK request does.
async fn insight_results(harness: &Harness, insight_arn: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/insights/results/{insight_arn}"), None)
        .await
}

/// Create one insight and return its ARN, asserting the call succeeded.
async fn insight_arn_for(
    harness: &Harness,
    name: &str,
    group_by: &str,
    filters: Value,
) -> String {
    let (status, body) = create_insight(
        harness,
        json!({ "Name": name, "GroupByAttribute": group_by, "Filters": filters }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateInsight({name}, {group_by}): expected 200, got {status} with {body}"
    );
    body["InsightArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateInsight({name}) must answer an InsightArn: {body}"))
        .to_string()
}

/// One finding of the seeded corpus, varied along every attribute the table groups by.
fn finding(
    id: &str,
    severity_label: &str,
    workflow_status: &str,
    types: &[&str],
    resources: &[(&str, &str)],
    compliance_status: Option<&str>,
) -> Value {
    let mut finding = base_finding(id);
    finding["Severity"] = json!({ "Label": severity_label });
    finding["Workflow"] = json!({ "Status": workflow_status });
    finding["Types"] = json!(types);
    finding["Resources"] = Value::Array(
        resources
            .iter()
            .map(|(kind, resource_id)| json!({ "Type": kind, "Id": resource_id }))
            .collect(),
    );
    if let Some(status) = compliance_status {
        finding["Compliance"] = json!({ "Status": status });
    }
    finding
}

/// The corpus every grouping case counts over. It deliberately spans the forms a
/// grouping attribute can take: two findings sharing a value, a finding carrying SEVERAL
/// values for one attribute (`Types`, and two `Resources`), two resources of the same
/// type under one finding, and findings carrying NO value for an attribute (`Compliance`
/// is absent on two of them).
fn seeded_findings() -> Vec<Value> {
    vec![
        finding(
            "f-1",
            "HIGH",
            "NEW",
            &["Software and Configuration Checks/A"],
            &[("AwsEc2Instance", "i-1")],
            Some("PASSED"),
        ),
        finding(
            "f-2",
            "HIGH",
            "NEW",
            &[
                "Software and Configuration Checks/A",
                "Software and Configuration Checks/B",
            ],
            &[("AwsEc2Instance", "i-1")],
            Some("FAILED"),
        ),
        finding(
            "f-3",
            "LOW",
            "NOTIFIED",
            &["Software and Configuration Checks/B"],
            &[("AwsS3Bucket", "b-1"), ("AwsS3Bucket", "b-2")],
            None,
        ),
        finding(
            "f-4",
            "CRITICAL",
            "NEW",
            &["Software and Configuration Checks/C"],
            &[("AwsIamRole", "r-1")],
            None,
        ),
        finding(
            "f-5",
            "MEDIUM",
            "RESOLVED",
            &["Software and Configuration Checks/A"],
            &[("AwsEc2Instance", "i-2")],
            Some("PASSED"),
        ),
    ]
}

/// `(GroupByAttributeValue, Count)` in the order the response listed them.
fn result_pairs(body: &Value) -> Vec<(String, i64)> {
    body["InsightResults"]["ResultValues"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    (
                        value["GroupByAttributeValue"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        value["Count"].as_i64().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `(value, count)` rows as the table writes them, for comparison with [`result_pairs`].
fn expected_pairs(rows: &[(&str, i64)]) -> Vec<(String, i64)> {
    rows.iter()
        .map(|(value, count)| ((*value).to_string(), *count))
        .collect()
}

// ---------------------------------------------------------------------------
// CreateInsight → GetInsightResults
// ---------------------------------------------------------------------------

/// RULE: an insight's results count the hub's matching findings per DISTINCT value the
/// finding carries for the insight's grouping attribute — a finding contributes once to
/// each value it carries, and nothing to any group when it carries none.
///
/// The table spans the kinds of grouping attribute the model names: a single-valued
/// top-level member (`SeverityLabel` is nested, `Id`/`RecordState`/`AwsAccountId` are
/// not), a list-valued one (`Type`), a nested one (`WorkflowStatus`,
/// `ComplianceStatus` — absent on two findings), and the attributes of the finding's
/// `Resources` (`ResourceType`, where one finding's two buckets are ONE bucket group,
/// and `ResourceId`).
#[tokio::test]
async fn insight_results_count_findings_per_distinct_group_value() {
    // (GroupByAttribute, expected (value, count) rows in response order)
    let cases: [(&str, &[(&str, i64)]); 8] = [
        (
            "SeverityLabel",
            &[("HIGH", 2), ("CRITICAL", 1), ("LOW", 1), ("MEDIUM", 1)],
        ),
        (
            "WorkflowStatus",
            &[("NEW", 3), ("NOTIFIED", 1), ("RESOLVED", 1)],
        ),
        (
            "Type",
            &[
                ("Software and Configuration Checks/A", 3),
                ("Software and Configuration Checks/B", 2),
                ("Software and Configuration Checks/C", 1),
            ],
        ),
        (
            "ResourceType",
            &[("AwsEc2Instance", 3), ("AwsIamRole", 1), ("AwsS3Bucket", 1)],
        ),
        (
            "ResourceId",
            &[("i-1", 2), ("b-1", 1), ("b-2", 1), ("i-2", 1), ("r-1", 1)],
        ),
        ("AwsAccountId", &[(ACCOUNT_ID, 5)]),
        ("ComplianceStatus", &[("PASSED", 2), ("FAILED", 1)]),
        ("RecordState", &[("ACTIVE", 5)]),
    ];

    let harness = Harness::with_findings(seeded_findings()).await;

    for (group_by, expected) in cases {
        let arn = insight_arn_for(&harness, &format!("by {group_by}"), group_by, json!({})).await;

        let (status, body) = insight_results(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {group_by:?}: GetInsightResults expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["InsightResults"]["InsightArn"].as_str(),
            Some(arn.as_str()),
            "case {group_by:?}: expected InsightArn {arn}, got {:?}",
            body["InsightResults"]["InsightArn"]
        );
        assert_eq!(
            body["InsightResults"]["GroupByAttribute"].as_str(),
            Some(group_by),
            "case {group_by:?}: expected GroupByAttribute {group_by}, got {:?}",
            body["InsightResults"]["GroupByAttribute"]
        );
        let actual = result_pairs(&body);
        assert_eq!(
            actual,
            expected_pairs(expected),
            "case {group_by:?}: expected {:?}, got {actual:?}",
            expected_pairs(expected)
        );
    }
}

/// RULE: an insight's stored `Filters` select findings by exactly the predicate the same
/// filters select by in `GetFindings`, so grouping by `Id` — the finding's own identity,
/// one group of one finding each — reproduces the identifiers that call returns.
///
/// `GetFindings` is the independent oracle here: the expected set is never written down,
/// it is read back from the implemented filter path. The table's rows differ along the
/// filter kinds `AwsSecurityFindingFilters` carries — none, a string EQUALS, a string
/// PREFIX, a CONTAINS over a list-valued attribute, a negated string, a number bound, a
/// date bound, and two attributes at once (joined by AND).
#[tokio::test]
async fn insight_filters_select_what_get_findings_selects() {
    // (case, Filters)
    let cases: [(&str, Value); 8] = [
        ("no filters", json!({})),
        (
            "severity EQUALS",
            json!({ "SeverityLabel": [{ "Value": "HIGH", "Comparison": "EQUALS" }] }),
        ),
        (
            "id PREFIX",
            json!({ "Id": [{ "Value": "f-", "Comparison": "PREFIX" }] }),
        ),
        (
            "type CONTAINS",
            json!({ "Type": [{ "Value": "Checks/B", "Comparison": "CONTAINS" }] }),
        ),
        (
            "workflow NOT_EQUALS",
            json!({ "WorkflowStatus": [{ "Value": "NEW", "Comparison": "NOT_EQUALS" }] }),
        ),
        (
            "severity normalized range",
            json!({ "SeverityNormalized": [{ "Gte": 70.0 }] }),
        ),
        (
            "updated after",
            json!({ "UpdatedAt": [{ "Start": "2026-10-21T20:30:00Z" }] }),
        ),
        (
            "two attributes",
            json!({
                "SeverityLabel": [{ "Value": "HIGH", "Comparison": "EQUALS" }],
                "WorkflowStatus": [{ "Value": "NEW", "Comparison": "EQUALS" }],
            }),
        ),
    ];

    let harness = Harness::with_findings(seeded_findings()).await;

    for (case, filters) in cases {
        let (status, findings) = harness.get_findings(json!({ "Filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetFindings expected 200, got {status} with {findings}"
        );
        // The ids `GetFindings` answered for the same filters, read back from the
        // implemented filter path rather than written down here.
        let mut expected: Vec<String> = returned_ids(&findings);
        expected.sort();
        // Every row is written to select at least one of the seeded findings, so an
        // empty oracle means the row stopped exercising its filter kind rather than
        // that the insight agreed with `GetFindings` about nothing.
        assert!(
            !expected.is_empty(),
            "case {case:?}: the row must select at least one finding, got {findings}"
        );

        let arn = insight_arn_for(&harness, case, "Id", filters.clone()).await;
        let (status, body) = insight_results(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetInsightResults expected 200, got {status} with {body}"
        );
        let pairs = result_pairs(&body);
        let mut grouped: Vec<String> = pairs.iter().map(|(value, _)| value.clone()).collect();
        grouped.sort();
        assert_eq!(
            grouped, expected,
            "case {case:?}: expected the insight to group the ids {expected:?}, got {grouped:?}"
        );
        assert!(
            pairs.iter().all(|(_, count)| *count == 1),
            "case {case:?}: grouping by Id must count each finding once, got {pairs:?}"
        );
    }
}

/// RULE: a created insight is a resource with a server-minted ARN — one ARN per create,
/// of the service's own `insight/<account>/custom/<id>` form — and reading its results
/// changes nothing, so repeated reads answer identically.
#[tokio::test]
async fn created_insights_get_distinct_arns_and_reads_do_not_mutate() {
    let harness = Harness::with_findings(seeded_findings()).await;
    let prefix = format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:insight/{ACCOUNT_ID}/custom/");

    let mut arns: Vec<String> = Vec::new();
    for name in ["first", "second", "third"] {
        let arn = insight_arn_for(&harness, name, "SeverityLabel", json!({})).await;
        assert!(
            arn.starts_with(&prefix),
            "case {name:?}: expected an ARN starting {prefix}, got {arn}"
        );
        let id = &arn[prefix.len()..];
        // The identifier is 128 bits in the 8-4-4-4-12 hex grouping Security Hub's own
        // insight ARNs carry: five all-hex groups of exactly those lengths.
        let groups: Vec<&str> = id.split('-').collect();
        let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
        assert_eq!(
            lengths,
            vec![8, 4, 4, 4, 12],
            "case {name:?}: expected 8-4-4-4-12 hex groups after the prefix, got {id:?}"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.chars().all(|c| c.is_ascii_hexdigit())),
            "case {name:?}: every group must be hex, got {id:?}"
        );
        arns.push(arn);
    }
    for (position, arn) in arns.iter().enumerate() {
        assert!(
            !arns[..position].contains(arn),
            "case {position}: every create must mint its own ARN, got a repeat of {arn}"
        );
    }

    let (status, first) = insight_results(&harness, &arns[0]).await;
    assert_eq!(status, 200, "first read: expected 200, got {status}");
    let (status, second) = insight_results(&harness, &arns[0]).await;
    assert_eq!(status, 200, "second read: expected 200, got {status}");
    assert_eq!(
        first, second,
        "case \"repeated read\": GetInsightResults must not change what a later caller sees, got {second}"
    );

    // Every insight is still addressable after the others were created and read.
    for (position, arn) in arns.iter().enumerate() {
        let (status, body) = insight_results(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {position}: expected 200 for {arn}, got {status} with {body}"
        );
    }
}

/// RULE: the insight plane refuses what the model says it refuses — an unsubscribed
/// account (401), a `GroupByAttribute` that is not a finding attribute or a `Filters`
/// outside the documented limits (400), a name already in use (409), and an ARN no
/// insight has (404).
#[tokio::test]
async fn the_insight_plane_refuses_what_the_model_refuses() {
    // Before EnableSecurityHub there is no hub to create an insight in.
    let fresh = Harness::new();
    let (status, body) = create_insight(
        &fresh,
        json!({ "Name": "no hub", "GroupByAttribute": "SeverityLabel", "Filters": {} }),
    )
    .await;
    assert_eq!(
        status, 401,
        "case \"CreateInsight before enable\": expected 401, got {status} with {body}"
    );
    let (status, body) = insight_results(
        &fresh,
        &format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:insight/{ACCOUNT_ID}/custom/absent"),
    )
    .await;
    assert_eq!(
        status, 401,
        "case \"GetInsightResults before enable\": expected 401, got {status} with {body}"
    );

    let harness = Harness::with_findings(seeded_findings()).await;

    // (case, request body, expected status)
    let mut cases: Vec<(&str, Value, u16)> = vec![
        (
            "unknown GroupByAttribute",
            json!({ "Name": "bad group", "GroupByAttribute": "NotAFindingAttribute", "Filters": {} }),
            400,
        ),
        (
            "GroupByAttribute is an output-only member name",
            json!({ "Name": "bad group 2", "GroupByAttribute": "InsightArn", "Filters": {} }),
            400,
        ),
        (
            "CONTAINS with NOT_CONTAINS on one field",
            json!({
                "Name": "bad filters",
                "GroupByAttribute": "SeverityLabel",
                "Filters": { "Title": [
                    { "Value": "a", "Comparison": "CONTAINS" },
                    { "Value": "b", "Comparison": "NOT_CONTAINS" },
                ] },
            }),
            400,
        ),
        (
            "a valid insight",
            json!({ "Name": "unique", "GroupByAttribute": "SeverityLabel", "Filters": {} }),
            200,
        ),
        (
            "the same name again",
            json!({ "Name": "unique", "GroupByAttribute": "WorkflowStatus", "Filters": {} }),
            409,
        ),
        (
            "a different name",
            json!({ "Name": "unique 2", "GroupByAttribute": "SeverityLabel", "Filters": {} }),
            200,
        ),
    ];
    // "For each attribute, you can provide up to 20 filter values" (`Insight$Filters`):
    // the 21st value is outside the documented limit.
    let twenty_one: Vec<Value> = (0..21)
        .map(|index| json!({ "Value": format!("title-{index}"), "Comparison": "EQUALS" }))
        .collect();
    cases.push((
        "21 filter values on one attribute",
        json!({
            "Name": "too many values",
            "GroupByAttribute": "SeverityLabel",
            "Filters": { "Title": twenty_one },
        }),
        400,
    ));

    for (case, body, expected) in cases {
        let (status, payload) = create_insight(&harness, body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {payload}"
        );
    }

    let (status, payload) = insight_results(
        &harness,
        &format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:insight/{ACCOUNT_ID}/custom/absent"),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"GetInsightResults for an unknown ARN\": expected 404, got {status} with {payload}"
    );
}

/// RULE: an insight this slice cannot evaluate answers 501 naming what it cannot
/// evaluate, rather than counting findings it did not filter or group — a grouping
/// attribute whose value no accessor reads, and a filter member nothing applies.
#[tokio::test]
async fn an_insight_this_slice_cannot_evaluate_answers_501() {
    let harness = Harness::with_findings(seeded_findings()).await;

    // (case, GroupByAttribute, Filters, the text the refusal must name)
    let cases: [(&str, &str, Value, &str); 3] = [
        (
            "grouped by a process attribute",
            "ProcessName",
            json!({}),
            "ProcessName",
        ),
        (
            "grouped by a malware attribute",
            "MalwareName",
            json!({}),
            "MalwareName",
        ),
        (
            "filtered by a keyword",
            "SeverityLabel",
            json!({ "Keyword": [{ "Value": "anything" }] }),
            "Keyword",
        ),
    ];

    for (case, group_by, filters, named) in cases {
        let arn = insight_arn_for(&harness, case, group_by, filters).await;
        let (status, body) = insight_results(&harness, &arn).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501, got {status} with {body}"
        );
        assert!(
            body.to_string().contains(named),
            "case {case:?}: expected the refusal to name {named}, got {body}"
        );
    }
}

/// RULE: a hub holds at most the published quota of custom insights — the create that
/// would exceed it is the modelled `LimitExceededException` (429) — and the quota counts
/// the insights of THAT hub, so insights deleted with the hub stop counting.
#[tokio::test]
async fn the_custom_insight_quota_bounds_what_one_caller_accumulates() {
    // "Insights — 100 custom insights per account per Region" (AWS Security Hub quotas),
    // the same constant `create_insight.rs` cites.
    const QUOTA: usize = 100;

    let harness = Harness::new();
    let (status, body) = harness.enable(None).await;
    assert_eq!(status, 200, "EnableSecurityHub: expected 200, got {status} with {body}");

    for index in 0..QUOTA {
        let (status, body) = create_insight(
            &harness,
            json!({
                "Name": format!("insight-{index}"),
                "GroupByAttribute": "SeverityLabel",
                "Filters": {},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {index}: a create within the quota expected 200, got {status} with {body}"
        );
    }

    let over_quota = json!({
        "Name": format!("insight-{QUOTA}"),
        "GroupByAttribute": "SeverityLabel",
        "Filters": {},
    });
    for attempt in 1..=2 {
        let (status, body) = create_insight(&harness, over_quota.clone()).await;
        assert_eq!(
            status, 429,
            "attempt {attempt}: the create past the quota expected 429, got {status} with {body}"
        );
    }

    // The quota is the hub's, so releasing the hub releases it.
    let (status, body) = harness.disable().await;
    assert_eq!(status, 200, "DisableSecurityHub: expected 200, got {status} with {body}");
    let (status, body) = harness.enable(None).await;
    assert_eq!(status, 200, "re-enable: expected 200, got {status} with {body}");
    let (status, body) = create_insight(&harness, over_quota).await;
    assert_eq!(
        status, 200,
        "after disable: the quota must count only this hub's insights, got {status} with {body}"
    );
}

/// RULE: a custom insight is a resource of the hub it was created in, so disabling
/// Security Hub CSPM deletes it with the findings it queried, and a later subscription
/// starts with no insights.
#[tokio::test]
async fn disabling_security_hub_deletes_the_hubs_insights() {
    let harness = Harness::with_findings(seeded_findings()).await;
    let arn = insight_arn_for(&harness, "by severity", "SeverityLabel", json!({})).await;

    let (status, body) = insight_results(&harness, &arn).await;
    assert_eq!(status, 200, "before disable: expected 200, got {status} with {body}");

    let (status, body) = harness.disable().await;
    assert_eq!(status, 200, "DisableSecurityHub: expected 200, got {status} with {body}");
    let (status, body) = harness.enable(None).await;
    assert_eq!(status, 200, "re-enable: expected 200, got {status} with {body}");

    let (status, body) = insight_results(&harness, &arn).await;
    assert_eq!(
        status, 404,
        "after disable: expected 404 for the deleted insight {arn}, got {status} with {body}"
    );

    // And the name is free again, which is what proves the record itself is gone.
    let (status, body) = create_insight(
        &harness,
        json!({ "Name": "by severity", "GroupByAttribute": "SeverityLabel", "Filters": {} }),
    )
    .await;
    assert_eq!(
        status, 200,
        "after disable: the name must be reusable, got {status} with {body}"
    );
    let reused = body["InsightArn"].as_str().unwrap_or_default();
    assert_ne!(
        reused, arn,
        "after disable: the new insight must get its own ARN, got {reused}"
    );
}
