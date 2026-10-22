// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of Security Hub CSPM automation rules:
//! CreateAutomationRule → BatchGetAutomationRules / ListAutomationRules →
//! BatchUpdateAutomationRules → BatchDeleteAutomationRules, and the effect an
//! `ENABLED` rule has on `BatchImportFindings` / `BatchUpdateFindings`.
//!
//! RULE under test: a created automation rule is persisted with the members the
//! request carried (and the documented default for each it omitted), is reported by
//! every reader of that rule, and — only while its `RuleStatus` is `ENABLED` — its
//! action updates the fields it names on exactly the findings whose attributes
//! satisfy its `Criteria`, lowest `RuleOrder` first, stopping after a rule that
//! `IsTerminal` matches.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test signs for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// A product ARN of the shape Security Hub's own product uses.
const PRODUCT_ARN: &str = "arn:aws:securityhub:us-east-1::product/aws/securityhub";

/// The only schema version the `AwsSecurityFinding` format defines.
const SCHEMA_VERSION: &str = "2018-10-08";

/// The `AutomationRulesConfig$CreatedBy` the emulator presents: the one IAM
/// principal it attributes every signed request to (`state::caller_arn`), which is
/// also the identity a finding-history event names.
const CREATED_BY: &str = "arn:aws:iam::000000000000:user/emulator";

/// One emulator with the Security Hub service mounted, driven over HTTP.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// A harness whose hub is already subscribed, which every automation-rule
    /// operation requires.
    async fn subscribed() -> Self {
        let harness = Self::new();
        let (status, body) = harness.call("POST", "/accounts", Some(json!({}))).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// Dispatch one request and return `(status, parsed body)`. An empty body parses
    /// as `Value::Null`, which is what an operation with no output members returns.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn create_rule(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/automationrules/create", Some(body)).await
    }

    async fn get_rules(&self, arns: Vec<&str>) -> (u16, Value) {
        self.call(
            "POST",
            "/automationrules/get",
            Some(json!({ "AutomationRulesArns": arns })),
        )
        .await
    }

    async fn update_rules(&self, items: Vec<Value>) -> (u16, Value) {
        self.call(
            "PATCH",
            "/automationrules/update",
            Some(json!({ "UpdateAutomationRulesRequestItems": items })),
        )
        .await
    }

    async fn delete_rules(&self, arns: Vec<&str>) -> (u16, Value) {
        self.call(
            "POST",
            "/automationrules/delete",
            Some(json!({ "AutomationRulesArns": arns })),
        )
        .await
    }

    async fn list_rules(&self, query: &str) -> (u16, Value) {
        let path = if query.is_empty() {
            "/automationrules/list".to_string()
        } else {
            format!("/automationrules/list?{query}")
        };
        self.call("GET", &path, None).await
    }

    async fn import(&self, findings: Vec<Value>) -> (u16, Value) {
        self.call("POST", "/findings/import", Some(json!({ "Findings": findings })))
            .await
    }

    async fn get_findings(&self) -> (u16, Value) {
        self.call("POST", "/findings", Some(json!({}))).await
    }

    async fn update_findings(&self, body: Value) -> (u16, Value) {
        self.call("PATCH", "/findings/batchupdate", Some(body)).await
    }

    /// Create `rule`, asserting it succeeds, and answer the ARN it minted.
    async fn create_rule_ok(&self, case: &str, rule: Value) -> String {
        let (status, body) = self.create_rule(rule).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateAutomationRule expected 200, got {status} with {body}"
        );
        body["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: response carries no RuleArn: {body}"))
            .to_string()
    }

    /// Import `findings`, asserting every one of them was stored.
    async fn import_ok(&self, case: &str, findings: Vec<Value>) {
        let expected = findings.len() as u64;
        let (status, body) = self.import(findings).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchImportFindings expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["SuccessCount"].as_u64(),
            Some(expected),
            "case {case:?}: every seeded finding must import: {body}"
        );
    }
}

/// A finding that satisfies every documented `AwsSecurityFinding` constraint.
fn base_finding(id: &str) -> Value {
    json!({
        "SchemaVersion": SCHEMA_VERSION,
        "Id": id,
        "ProductArn": PRODUCT_ARN,
        "GeneratorId": "generator/alpha",
        "AwsAccountId": ACCOUNT_ID,
        "CreatedAt": "2026-10-21T20:00:00Z",
        "UpdatedAt": "2026-10-21T21:00:00Z",
        "Title": format!("Title of {id}"),
        "Description": format!("Description of {id}"),
        "Resources": [{ "Type": "AwsEc2Instance", "Id": "i-0123456789abcdef0" }],
        "Severity": { "Label": "HIGH" },
        "RecordState": "ACTIVE",
        "Workflow": { "Status": "NEW" },
        "Types": ["Software and Configuration Checks/Vulnerabilities/CVE"],
    })
}

/// [`base_finding`] with `overrides`' members replacing its own.
fn finding_with(id: &str, overrides: Value) -> Value {
    let mut finding = base_finding(id);
    let (Some(target), Some(source)) = (finding.as_object_mut(), overrides.as_object()) else {
        return finding;
    };
    for (member, value) in source {
        target.insert(member.clone(), value.clone());
    }
    finding
}

/// A `FINDING_FIELDS_UPDATE` action carrying `update` — the only
/// `AutomationRulesActionType` the model defines.
fn finding_fields_update(update: Value) -> Value {
    json!([{ "Type": "FINDING_FIELDS_UPDATE", "FindingFieldsUpdate": update }])
}

/// A complete, valid `CreateAutomationRule` request body for `criteria`/`actions`.
fn rule_request(name: &str, criteria: Value, actions: Value, extra: Value) -> Value {
    let mut request = json!({
        "RuleName": name,
        "Description": format!("Rule {name}"),
        "RuleOrder": 1,
        "Criteria": criteria,
        "Actions": actions,
    });
    if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
    request
}

/// The value at `path` inside `value`; a step that is all digits indexes an array,
/// so a path can walk into a list member. `Value::Null` when any step is absent.
fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut cursor = value;
    for step in path {
        cursor = match (cursor.is_array(), step.parse::<usize>()) {
            (true, Ok(index)) => &cursor[index],
            _ => &cursor[*step],
        };
    }
    cursor
}

/// The one finding in a `GetFindings` response whose `Id` is `id`.
fn finding_with_id(body: &Value, id: &str) -> Value {
    body["Findings"]
        .as_array()
        .and_then(|items| items.iter().find(|f| f["Id"].as_str() == Some(id)).cloned())
        .unwrap_or(Value::Null)
}

/// The `RuleArn`s in a `ListAutomationRules` response, in the order it listed them.
fn listed_arns(body: &Value) -> Vec<String> {
    body["AutomationRulesMetadata"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|r| r["RuleArn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: `CreateAutomationRule` persists the rule and answers its ARN, and every
/// member comes back from `BatchGetAutomationRules` — the ones the request carried
/// as sent, and the ones it omitted at their documented defaults (`RuleStatus`
/// `ENABLED`, `IsTerminal` false).
///
/// The rows span what the rule shape generalises over: the status, the order across
/// both published bounds, terminality, and criteria of every filter kind.
#[tokio::test]
async fn created_rules_round_trip_through_batch_get_automation_rules() {
    let harness = Harness::subscribed().await;

    // (case, extra request members, expected RuleStatus, expected IsTerminal)
    let cases: [(&str, Value, &str, bool); 6] = [
        (
            "RuleStatus and IsTerminal omitted",
            json!({}),
            "ENABLED",
            false,
        ),
        (
            "explicitly ENABLED, RuleOrder at the lower bound",
            json!({ "RuleStatus": "ENABLED", "RuleOrder": 1 }),
            "ENABLED",
            false,
        ),
        (
            "explicitly DISABLED",
            json!({ "RuleStatus": "DISABLED" }),
            "DISABLED",
            false,
        ),
        (
            "terminal rule",
            json!({ "IsTerminal": true, "RuleOrder": 500 }),
            "ENABLED",
            true,
        ),
        (
            "RuleOrder at the upper bound",
            json!({ "RuleOrder": 1000, "IsTerminal": false }),
            "ENABLED",
            false,
        ),
        (
            "tagged rule",
            json!({ "Tags": { "team": "secops" } }),
            "ENABLED",
            false,
        ),
    ];

    // One criteria per row, spanning the filter kinds a rule can carry.
    let criteria_set: [Value; 6] = [
        json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
        json!({ "Title": [{ "Value": "Title of", "Comparison": "PREFIX" }] }),
        // `NumberFilter`'s bounds are modelled `double`, so the bound comes back as
        // one: the round-trip compares the value, not the JSON number's spelling.
        json!({ "Confidence": [{ "Gte": 80.0 }] }),
        json!({ "CreatedAt": [{ "Start": "2026-01-01T00:00:00Z", "End": "2027-01-01T00:00:00Z" }] }),
        json!({ "Type": [{ "Value": "Software and Configuration Checks", "Comparison": "PREFIX" }] }),
        json!({ "RecordState": [{ "Value": "ARCHIVED", "Comparison": "NOT_EQUALS" }] }),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (index, (case, extra, expected_status, expected_terminal)) in cases.into_iter().enumerate() {
        let criteria = criteria_set[index].clone();
        let actions = finding_fields_update(json!({ "Workflow": { "Status": "SUPPRESSED" } }));
        let request = rule_request(
            &format!("rule-{index}"),
            criteria.clone(),
            actions.clone(),
            extra.clone(),
        );
        let expected_order = request["RuleOrder"].clone();
        let rule_arn = harness.create_rule_ok(case, request).await;

        let prefix = format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:automation-rule/");
        assert!(
            rule_arn.starts_with(&prefix),
            "case {case:?}: expected a rule ARN under {prefix}, got {rule_arn:?}"
        );
        let rule_id = rule_arn.trim_start_matches(&prefix);
        assert_eq!(
            rule_id.len(),
            36,
            "case {case:?}: expected a uuid-shaped rule id, got {rule_id:?}"
        );
        assert!(
            !minted.contains(&rule_arn),
            "case {case:?}: every rule must get its own ARN, {rule_arn:?} repeats"
        );
        minted.push(rule_arn.clone());

        let (status, body) = harness.get_rules(vec![&rule_arn]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAutomationRules expected 200, got {status} with {body}"
        );
        let rule = body["Rules"][0].clone();
        assert_eq!(
            rule["RuleArn"].as_str(),
            Some(rule_arn.as_str()),
            "case {case:?}: expected RuleArn {rule_arn:?}, got {rule:?}"
        );
        assert_eq!(
            rule["RuleStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: expected RuleStatus {expected_status:?}, got {rule:?}"
        );
        assert_eq!(
            rule["IsTerminal"].as_bool(),
            Some(expected_terminal),
            "case {case:?}: expected IsTerminal {expected_terminal}, got {rule:?}"
        );
        assert_eq!(
            rule["RuleOrder"], expected_order,
            "case {case:?}: expected RuleOrder {expected_order}, got {rule:?}"
        );
        assert_eq!(
            rule["RuleName"].as_str(),
            Some(format!("rule-{index}").as_str()),
            "case {case:?}: expected the RuleName it was created with, got {rule:?}"
        );
        assert_eq!(
            rule["Description"].as_str(),
            Some(format!("Rule rule-{index}").as_str()),
            "case {case:?}: expected the Description it was created with, got {rule:?}"
        );
        assert_eq!(
            rule["Criteria"], criteria,
            "case {case:?}: expected the Criteria it was created with, got {rule:?}"
        );
        assert_eq!(
            rule["Actions"], actions,
            "case {case:?}: expected the Actions it was created with, got {rule:?}"
        );
        assert_eq!(
            rule["CreatedBy"].as_str(),
            Some(CREATED_BY),
            "case {case:?}: expected CreatedBy {CREATED_BY:?}, got {rule:?}"
        );
        // A rule nothing has updated yet reports the instant it was created for both
        // timestamps, and both must be present.
        assert!(
            rule["CreatedAt"].is_number() || rule["CreatedAt"].is_string(),
            "case {case:?}: expected a CreatedAt timestamp, got {rule:?}"
        );
        assert_eq!(
            rule["UpdatedAt"], rule["CreatedAt"],
            "case {case:?}: a rule that was never updated reports UpdatedAt == CreatedAt, got {rule:?}"
        );
        assert_eq!(
            body["UnprocessedAutomationRules"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a known ARN must not be unprocessed: {body}"
        );
    }

    // Every rule created above is still there, and one call reads them all back.
    let arns: Vec<&str> = minted.iter().map(String::as_str).collect();
    let (status, body) = harness.get_rules(arns).await;
    assert_eq!(status, 200, "reading every rule back: got {status} {body}");
    assert_eq!(
        body["Rules"].as_array().map(Vec::len),
        Some(minted.len()),
        "expected {} rules, got {body}",
        minted.len()
    );
}

/// RULE: `RuleOrder` is modelled `@range(min: 1, max: 1000)`, so each published
/// bound is accepted and the value one past it is refused with 400.
#[tokio::test]
async fn rule_order_is_accepted_on_its_bounds_and_refused_outside_them() {
    let harness = Harness::subscribed().await;

    // (case, RuleOrder, expected status)
    let cases: [(&str, i64, u16); 4] = [
        ("the lower bound", 1, 200),
        ("the upper bound", 1000, 200),
        ("one below the lower bound", 0, 400),
        ("one above the upper bound", 1001, 400),
    ];

    for (case, rule_order, expected) in cases {
        let request = rule_request(
            &format!("order-{rule_order}"),
            json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
            finding_fields_update(json!({ "Confidence": 1 })),
            json!({ "RuleOrder": rule_order }),
        );
        let (status, body) = harness.create_rule(request).await;
        assert_eq!(
            status, expected,
            "case {case:?} (RuleOrder {rule_order}): expected {expected}, got {status} with {body}"
        );
    }
}

/// One row of [`an_enabled_rule_acts_on_exactly_the_findings_its_criteria_select`]:
/// one criteria kind paired with the finding field the rule's action updates, and
/// two findings that differ only in the attribute the criteria names.
struct ActingCase {
    case: &'static str,
    criteria: Value,
    update: Value,
    matching: Value,
    other: Value,
    /// The member path read back out of `GetFindings`.
    path: &'static [&'static str],
    /// What the matching finding must carry at `path` once the rule has acted.
    expected: Value,
    /// What the non-matching finding must still carry at `path`.
    untouched: Value,
}

/// RULE: an `ENABLED` rule's action updates the fields it names on exactly the
/// findings whose attributes satisfy its `Criteria`, and leaves every other finding
/// untouched.
///
/// Each row is one criteria kind paired with one updated finding field; the matching
/// and the non-matching finding differ only in the attribute the criteria names.
#[tokio::test]
async fn an_enabled_rule_acts_on_exactly_the_findings_its_criteria_select() {
    let cases: [ActingCase; 8] = [
        ActingCase {
            case: "Id EQUALS suppresses the workflow",
            criteria: json!({ "Id": [{ "Value": "match-id", "Comparison": "EQUALS" }] }),
            update: json!({ "Workflow": { "Status": "SUPPRESSED" } }),
            matching: finding_with("match-id", json!({})),
            other: finding_with("other-id", json!({})),
            path: &["Workflow", "Status"],
            expected: json!("SUPPRESSED"),
            untouched: json!("NEW"),
        },
        ActingCase {
            case: "SeverityLabel EQUALS lowers the severity",
            criteria: json!({ "SeverityLabel": [{ "Value": "HIGH", "Comparison": "EQUALS" }] }),
            update: json!({ "Severity": { "Label": "LOW" } }),
            matching: finding_with("match-severity", json!({ "Severity": { "Label": "HIGH" } })),
            other: finding_with("other-severity", json!({ "Severity": { "Label": "MEDIUM" } })),
            path: &["Severity", "Label"],
            expected: json!("LOW"),
            untouched: json!("MEDIUM"),
        },
        ActingCase {
            case: "Title PREFIX adds a note",
            criteria: json!({ "Title": [{ "Value": "Flagged", "Comparison": "PREFIX" }] }),
            update: json!({ "Note": { "Text": "triaged by rule", "UpdatedBy": "automation" } }),
            matching: finding_with("match-title", json!({ "Title": "Flagged by the scanner" })),
            other: finding_with("other-title", json!({ "Title": "Ordinary finding" })),
            path: &["Note", "Text"],
            expected: json!("triaged by rule"),
            untouched: Value::Null,
        },
        ActingCase {
            case: "Confidence Gte raises the criticality",
            criteria: json!({ "Confidence": [{ "Gte": 80 }] }),
            update: json!({ "Criticality": 90 }),
            matching: finding_with("match-confidence", json!({ "Confidence": 95 })),
            other: finding_with("other-confidence", json!({ "Confidence": 10 })),
            path: &["Criticality"],
            expected: json!(90),
            untouched: Value::Null,
        },
        ActingCase {
            case: "Type PREFIX sets user-defined fields",
            criteria: json!({ "Type": [{ "Value": "Effects/Data Exposure", "Comparison": "PREFIX" }] }),
            update: json!({ "UserDefinedFields": { "owner": "secops" } }),
            matching: finding_with(
                "match-type",
                json!({ "Types": ["Effects/Data Exposure/Public Bucket"] }),
            ),
            other: finding_with("other-type", json!({ "Types": ["Effects/Denial of Service"] })),
            path: &["UserDefinedFields", "owner"],
            expected: json!("secops"),
            untouched: Value::Null,
        },
        ActingCase {
            case: "CreatedAt window sets the verification state",
            criteria: json!({ "CreatedAt": [{ "Start": "2026-10-01T00:00:00Z", "End": "2026-10-31T00:00:00Z" }] }),
            update: json!({ "VerificationState": "TRUE_POSITIVE" }),
            matching: finding_with("match-created", json!({ "CreatedAt": "2026-10-15T00:00:00Z" })),
            other: finding_with("other-created", json!({ "CreatedAt": "2025-01-15T00:00:00Z" })),
            path: &["VerificationState"],
            expected: json!("TRUE_POSITIVE"),
            untouched: Value::Null,
        },
        ActingCase {
            case: "WorkflowStatus EQUALS replaces the types",
            criteria: json!({ "WorkflowStatus": [{ "Value": "NEW", "Comparison": "EQUALS" }] }),
            update: json!({ "Types": ["Unusual Behaviors/Application"] }),
            matching: finding_with("match-workflow", json!({ "Workflow": { "Status": "NEW" } })),
            other: finding_with(
                "other-workflow",
                json!({ "Workflow": { "Status": "NOTIFIED" } }),
            ),
            path: &["Types", "0"],
            expected: json!("Unusual Behaviors/Application"),
            untouched: json!("Software and Configuration Checks/Vulnerabilities/CVE"),
        },
        ActingCase {
            case: "RecordState NOT_EQUALS sets the confidence",
            criteria: json!({ "RecordState": [{ "Value": "ARCHIVED", "Comparison": "NOT_EQUALS" }] }),
            update: json!({ "Confidence": 55 }),
            matching: finding_with("match-record", json!({ "RecordState": "ACTIVE" })),
            other: finding_with("other-record", json!({ "RecordState": "ARCHIVED" })),
            path: &["Confidence"],
            expected: json!(55),
            untouched: Value::Null,
        },
    ];

    for row in cases {
        let ActingCase {
            case,
            criteria,
            update,
            matching,
            other,
            path,
            expected,
            untouched,
        } = row;
        let harness = Harness::subscribed().await;
        let match_id = matching["Id"].as_str().unwrap_or_default().to_string();
        let other_id = other["Id"].as_str().unwrap_or_default().to_string();
        harness
            .create_rule_ok(
                case,
                rule_request(
                    "acting-rule",
                    criteria,
                    finding_fields_update(update),
                    json!({}),
                ),
            )
            .await;
        harness.import_ok(case, vec![matching, other]).await;

        let (status, body) = harness.get_findings().await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetFindings expected 200, got {status} with {body}"
        );
        let matched = finding_with_id(&body, &match_id);
        let unmatched = finding_with_id(&body, &other_id);
        let actual = at(&matched, path);
        assert_eq!(
            *actual, expected,
            "case {case:?}: the matching finding expected {expected} at {path:?}, got {actual:?}"
        );
        let actual = at(&unmatched, path);
        assert_eq!(
            *actual, untouched,
            "case {case:?}: the non-matching finding expected {untouched} at {path:?}, got {actual:?}"
        );
    }
}

/// RULE: a rule acts only while its `RuleStatus` is `ENABLED`, and
/// `BatchUpdateAutomationRules` is what changes that status — so toggling it decides
/// whether findings written afterwards are acted on.
#[tokio::test]
async fn only_an_enabled_rule_acts_and_batch_update_toggles_that() {
    let harness = Harness::subscribed().await;
    let rule_arn = harness
        .create_rule_ok(
            "toggled rule",
            rule_request(
                "toggled",
                json!({ "RecordState": [{ "Value": "ACTIVE", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Workflow": { "Status": "SUPPRESSED" } })),
                json!({ "RuleStatus": "DISABLED" }),
            ),
        )
        .await;

    // (case, status the rule is in, the finding imported under it, expected workflow)
    let cases: [(&str, &str, &str, &str); 4] = [
        ("created DISABLED", "DISABLED", "finding-disabled-1", "NEW"),
        ("enabled", "ENABLED", "finding-enabled", "SUPPRESSED"),
        ("disabled again", "DISABLED", "finding-disabled-2", "NEW"),
        ("re-enabled", "ENABLED", "finding-enabled-2", "SUPPRESSED"),
    ];

    for (case, rule_status, finding_id, expected) in cases {
        let (status, body) = harness
            .update_rules(vec![
                json!({ "RuleArn": rule_arn, "RuleStatus": rule_status }),
            ])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchUpdateAutomationRules expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["ProcessedAutomationRules"][0].as_str(),
            Some(rule_arn.as_str()),
            "case {case:?}: the updated rule must be reported as processed: {body}"
        );
        let (status, body) = harness.get_rules(vec![&rule_arn]).await;
        assert_eq!(status, 200, "case {case:?}: BatchGet got {status} {body}");
        assert_eq!(
            body["Rules"][0]["RuleStatus"].as_str(),
            Some(rule_status),
            "case {case:?}: expected the toggled RuleStatus {rule_status:?}, got {body}"
        );

        harness
            .import_ok(case, vec![base_finding(finding_id)])
            .await;
        let (_, findings) = harness.get_findings().await;
        let finding = finding_with_id(&findings, finding_id);
        let actual = at(&finding, &["Workflow", "Status"]);
        assert_eq!(
            *actual,
            json!(expected),
            "case {case:?}: finding {finding_id:?} expected Workflow.Status {expected:?}, got {actual:?}"
        );
    }

    // Enabling a rule does not reach back into findings written while it was
    // disabled: a rule acts on a write, never on a read.
    let (_, findings) = harness.get_findings().await;
    for finding_id in ["finding-disabled-1", "finding-disabled-2"] {
        let finding = finding_with_id(&findings, finding_id);
        let actual = at(&finding, &["Workflow", "Status"]);
        assert_eq!(
            *actual,
            json!("NEW"),
            "case {finding_id:?}: a finding imported while the rule was disabled stays as imported, got {actual:?}"
        );
    }
}

/// RULE: a rule acts on a finding `BatchUpdateFindings` writes, exactly as it would
/// on the same values arriving through an import — Security Hub applies rules "to
/// findings and finding updates".
#[tokio::test]
async fn a_rule_acts_on_an_update_that_brings_a_finding_inside_its_criteria() {
    let harness = Harness::subscribed().await;
    harness
        .create_rule_ok(
            "notified rule",
            rule_request(
                "on-notified",
                json!({ "WorkflowStatus": [{ "Value": "NOTIFIED", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Criticality": 70 })),
                json!({}),
            ),
        )
        .await;
    harness
        .import_ok("seed", vec![base_finding("finding-updated")])
        .await;

    // Imported as NEW, so the rule does not match yet.
    let (_, findings) = harness.get_findings().await;
    let finding = finding_with_id(&findings, "finding-updated");
    assert_eq!(
        finding["Criticality"],
        Value::Null,
        "a finding outside the criteria must not be acted on: {finding}"
    );

    let (status, body) = harness
        .update_findings(json!({
            "FindingIdentifiers": [{ "Id": "finding-updated", "ProductArn": PRODUCT_ARN }],
            "Workflow": { "Status": "NOTIFIED" },
        }))
        .await;
    assert_eq!(
        status, 200,
        "BatchUpdateFindings expected 200, got {status} with {body}"
    );

    let (_, findings) = harness.get_findings().await;
    let finding = finding_with_id(&findings, "finding-updated");
    assert_eq!(
        finding["Criticality"],
        json!(70),
        "an update that brings the finding inside the criteria must be acted on: {finding}"
    );
}

/// One row of [`rules_apply_in_rule_order_and_a_terminal_rule_stops_the_walk`]: the
/// three rules to create as `(RuleOrder, IsTerminal, the Criticality it writes)`,
/// and the value the finding must end up with.
type OrderingCase = (&'static str, [(i64, bool, i64); 3], i64);

/// RULE: rules are applied in ascending `RuleOrder`, so the last rule to write a
/// field wins; a matching rule that `IsTerminal` ends the walk, so no later rule
/// writes that field at all.
#[tokio::test]
async fn rules_apply_in_rule_order_and_a_terminal_rule_stops_the_walk() {
    let cases: [OrderingCase; 4] = [
        (
            "ascending order, the last rule wins",
            [(10, false, 10), (20, false, 20), (30, false, 30)],
            30,
        ),
        (
            "rules declared out of order are still applied in RuleOrder",
            [(30, false, 30), (10, false, 10), (20, false, 20)],
            30,
        ),
        (
            "a terminal first rule stops the walk",
            [(10, true, 10), (20, false, 20), (30, false, 30)],
            10,
        ),
        (
            "a terminal middle rule stops the walk after it",
            [(10, false, 10), (20, true, 20), (30, false, 30)],
            20,
        ),
    ];

    for (case, rules, expected) in cases {
        let harness = Harness::subscribed().await;
        for (rule_order, is_terminal, criticality) in rules {
            harness
                .create_rule_ok(
                    case,
                    rule_request(
                        &format!("rule-{rule_order}"),
                        json!({ "Id": [{ "Value": "ordered", "Comparison": "EQUALS" }] }),
                        finding_fields_update(json!({ "Criticality": criticality })),
                        json!({ "RuleOrder": rule_order, "IsTerminal": is_terminal }),
                    ),
                )
                .await;
        }
        harness.import_ok(case, vec![base_finding("ordered")]).await;

        let (_, findings) = harness.get_findings().await;
        let finding = finding_with_id(&findings, "ordered");
        assert_eq!(
            finding["Criticality"],
            json!(expected),
            "case {case:?}: expected Criticality {expected}, got {finding}"
        );
    }
}

/// RULE: two rules that share one `RuleOrder` are applied in the order they were
/// created, so the outcome of a tie is deterministic rather than map-order.
#[tokio::test]
async fn rules_sharing_one_rule_order_apply_in_creation_order() {
    for (case, writes) in [
        ("first created writes 11, second writes 22", [11, 22]),
        ("first created writes 22, second writes 11", [22, 11]),
    ] {
        let harness = Harness::subscribed().await;
        for (index, criticality) in writes.into_iter().enumerate() {
            harness
                .create_rule_ok(
                    case,
                    rule_request(
                        &format!("tie-{index}"),
                        json!({ "Id": [{ "Value": "tied", "Comparison": "EQUALS" }] }),
                        finding_fields_update(json!({ "Criticality": criticality })),
                        json!({ "RuleOrder": 7 }),
                    ),
                )
                .await;
        }
        harness.import_ok(case, vec![base_finding("tied")]).await;

        let expected = writes[1];
        let (_, findings) = harness.get_findings().await;
        let finding = finding_with_id(&findings, "tied");
        assert_eq!(
            finding["Criticality"],
            json!(expected),
            "case {case:?}: the rule created last must win the tie with {expected}, got {finding}"
        );
    }
}

/// RULE: `ListAutomationRules` reports every rule of the calling account — `ENABLED`
/// and `DISABLED` alike — in `RuleOrder` order, paged by `MaxResults` and resumed
/// from the `NextToken` it issued.
#[tokio::test]
async fn list_automation_rules_reports_every_rule_in_rule_order_and_pages() {
    let harness = Harness::subscribed().await;

    // Created out of order, and with both statuses, so neither creation order nor
    // the status can be what the listing is keyed on.
    let mut expected: Vec<String> = Vec::new();
    for (rule_order, rule_status) in [
        (50, "ENABLED"),
        (10, "DISABLED"),
        (40, "ENABLED"),
        (20, "DISABLED"),
        (30, "ENABLED"),
    ] {
        let arn = harness
            .create_rule_ok(
                "listed rule",
                rule_request(
                    &format!("listed-{rule_order}"),
                    json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                    finding_fields_update(json!({ "Confidence": 1 })),
                    json!({ "RuleOrder": rule_order, "RuleStatus": rule_status }),
                ),
            )
            .await;
        expected.push(format!("{rule_order}:{arn}"));
    }
    expected.sort();
    let expected: Vec<String> = expected
        .into_iter()
        .map(|entry| entry.split_once(':').map(|(_, arn)| arn.to_string()).unwrap_or_default())
        .collect();

    let (status, body) = harness.list_rules("").await;
    assert_eq!(status, 200, "ListAutomationRules got {status} with {body}");
    assert_eq!(
        listed_arns(&body),
        expected,
        "one page: expected every rule in RuleOrder order, got {body}"
    );
    assert_eq!(
        body["NextToken"],
        Value::Null,
        "a complete page carries no NextToken: {body}"
    );
    for (index, rule) in body["AutomationRulesMetadata"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            rule["CreatedBy"].as_str(),
            Some(CREATED_BY),
            "rule {index}: expected CreatedBy {CREATED_BY:?}, got {rule}"
        );
        assert!(
            rule["RuleName"].is_string() && rule["Description"].is_string(),
            "rule {index}: metadata must carry RuleName and Description, got {rule}"
        );
    }

    // (case, MaxResults) — every page size walks the same total order.
    for (case, page_size) in [("one at a time", 1usize), ("two at a time", 2), ("all five", 5)] {
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("MaxResults={page_size}");
        loop {
            let (status, body) = harness.list_rules(&query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListAutomationRules got {status} with {body}"
            );
            let page = listed_arns(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page may hold at most {page_size} rules, got {}",
                page.len()
            );
            seen.extend(page);
            match body["NextToken"].as_str() {
                Some(token) => query = format!("MaxResults={page_size}&NextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must walk every rule once, in RuleOrder order"
        );
    }

    let (status, body) = harness.list_rules("NextToken=not-a-token").await;
    assert_eq!(
        status, 400,
        "a NextToken this operation never issued is refused: got {status} with {body}"
    );
}

/// RULE: an ARN that names no rule of the calling account is reported in
/// `UnprocessedAutomationRules` with the 404 of the `ResourceNotFoundException` the
/// same condition raises — it never refuses the whole batch, and never reports the
/// rule as processed.
#[tokio::test]
async fn an_arn_that_names_no_rule_is_reported_unprocessed() {
    let harness = Harness::subscribed().await;
    let known = harness
        .create_rule_ok(
            "known rule",
            rule_request(
                "known",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ),
        )
        .await;
    let unknown = format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:automation-rule/00000000-0000-4000-8000-000000000000");

    // (case, the call, the member that names what the call did process)
    let (status, body) = harness.get_rules(vec![&known, &unknown]).await;
    assert_eq!(status, 200, "BatchGet got {status} with {body}");
    assert_eq!(
        body["Rules"].as_array().map(Vec::len),
        Some(1),
        "BatchGet must return only the known rule: {body}"
    );
    assert_eq!(
        body["UnprocessedAutomationRules"][0]["RuleArn"].as_str(),
        Some(unknown.as_str()),
        "BatchGet must report the unknown ARN as unprocessed: {body}"
    );
    assert_eq!(
        body["UnprocessedAutomationRules"][0]["ErrorCode"],
        json!(404),
        "BatchGet must report 404 for an ARN that names no rule: {body}"
    );

    let (status, body) = harness
        .update_rules(vec![
            json!({ "RuleArn": unknown, "RuleStatus": "DISABLED" }),
        ])
        .await;
    assert_eq!(status, 200, "BatchUpdate got {status} with {body}");
    assert_eq!(
        body["ProcessedAutomationRules"],
        Value::Null,
        "BatchUpdate processed nothing, so it reports no ARNs: {body}"
    );
    assert_eq!(
        body["UnprocessedAutomationRules"][0]["ErrorCode"],
        json!(404),
        "BatchUpdate must report 404 for an ARN that names no rule: {body}"
    );

    let (status, body) = harness.delete_rules(vec![&unknown]).await;
    assert_eq!(status, 200, "BatchDelete got {status} with {body}");
    assert_eq!(
        body["UnprocessedAutomationRules"][0]["ErrorCode"],
        json!(404),
        "BatchDelete must report 404 for an ARN that names no rule: {body}"
    );
}

/// RULE: `BatchDeleteAutomationRules` removes the rule, so it is gone from every
/// reader and acts on no finding written afterwards.
#[tokio::test]
async fn a_deleted_rule_is_gone_and_acts_on_nothing() {
    let harness = Harness::subscribed().await;
    let rule_arn = harness
        .create_rule_ok(
            "deleted rule",
            rule_request(
                "deleted",
                json!({ "RecordState": [{ "Value": "ACTIVE", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Workflow": { "Status": "SUPPRESSED" } })),
                json!({}),
            ),
        )
        .await;

    harness
        .import_ok("before the delete", vec![base_finding("before")])
        .await;
    let (status, body) = harness.delete_rules(vec![&rule_arn]).await;
    assert_eq!(status, 200, "BatchDelete got {status} with {body}");
    assert_eq!(
        body["ProcessedAutomationRules"][0].as_str(),
        Some(rule_arn.as_str()),
        "the deleted rule must be reported as processed: {body}"
    );
    harness
        .import_ok("after the delete", vec![base_finding("after")])
        .await;

    let (_, body) = harness.get_rules(vec![&rule_arn]).await;
    assert_eq!(
        body["Rules"].as_array().map(Vec::len),
        Some(0),
        "a deleted rule is no longer readable: {body}"
    );
    let (_, body) = harness.list_rules("").await;
    assert!(
        listed_arns(&body).is_empty(),
        "a deleted rule is no longer listed: {body}"
    );

    let (_, findings) = harness.get_findings().await;
    for (finding_id, expected) in [("before", "SUPPRESSED"), ("after", "NEW")] {
        let finding = finding_with_id(&findings, finding_id);
        let actual = at(&finding, &["Workflow", "Status"]);
        assert_eq!(
            *actual,
            json!(expected),
            "finding {finding_id:?}: expected Workflow.Status {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a `BatchUpdateAutomationRules` item replaces every member it carries and
/// leaves every member it omits as it was — and the updated rule is what later
/// imports are evaluated against.
#[tokio::test]
async fn batch_update_replaces_only_the_members_the_item_carries() {
    let harness = Harness::subscribed().await;
    let rule_arn = harness
        .create_rule_ok(
            "updated rule",
            rule_request(
                "before-update",
                json!({ "Id": [{ "Value": "never-imported", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 5 })),
                json!({ "RuleOrder": 100, "IsTerminal": false }),
            ),
        )
        .await;

    // (case, the item's members beyond RuleArn, the member read back, expected)
    let cases: [(&str, Value, &str, Value); 5] = [
        (
            "RuleName only",
            json!({ "RuleName": "after-update" }),
            "RuleName",
            json!("after-update"),
        ),
        (
            "Description only",
            json!({ "Description": "a new description" }),
            "Description",
            json!("a new description"),
        ),
        (
            "RuleOrder only",
            json!({ "RuleOrder": 300 }),
            "RuleOrder",
            json!(300),
        ),
        (
            "IsTerminal only",
            json!({ "IsTerminal": true }),
            "IsTerminal",
            json!(true),
        ),
        (
            "Criteria and Actions",
            json!({
                "Criteria": { "Title": [{ "Value": "Title of", "Comparison": "PREFIX" }] },
                "Actions": [{
                    "Type": "FINDING_FIELDS_UPDATE",
                    "FindingFieldsUpdate": { "Confidence": 42 },
                }],
            }),
            "Criteria",
            json!({ "Title": [{ "Value": "Title of", "Comparison": "PREFIX" }] }),
        ),
    ];

    for (case, members, member, expected) in cases {
        let mut item = json!({ "RuleArn": rule_arn });
        if let (Some(target), Some(source)) = (item.as_object_mut(), members.as_object()) {
            for (name, value) in source {
                target.insert(name.clone(), value.clone());
            }
        }
        let (status, body) = harness.update_rules(vec![item]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchUpdateAutomationRules expected 200, got {status} with {body}"
        );

        let (_, body) = harness.get_rules(vec![&rule_arn]).await;
        let rule = body["Rules"][0].clone();
        assert_eq!(
            rule[member], expected,
            "case {case:?}: expected {member} {expected}, got {rule}"
        );
        // Members no item ever carried keep the value the create gave them.
        assert_eq!(
            rule["RuleStatus"],
            json!("ENABLED"),
            "case {case:?}: RuleStatus was never updated, got {rule}"
        );
    }

    // The rule now matches on Title and writes Confidence 42: the update decides what
    // a later import is acted on by.
    harness
        .import_ok("after the updates", vec![base_finding("finally-imported")])
        .await;
    let (_, findings) = harness.get_findings().await;
    let finding = finding_with_id(&findings, "finally-imported");
    assert_eq!(
        finding["Confidence"],
        json!(42),
        "the updated criteria and action are what acted on the import: {finding}"
    );
}

/// RULE: a `Criteria` member this emulator does not evaluate is answered with 501,
/// never by storing a rule that would act on findings nothing selected.
#[tokio::test]
async fn a_criteria_member_this_emulator_does_not_evaluate_answers_501() {
    let harness = Harness::subscribed().await;
    let rule_arn = harness
        .create_rule_ok(
            "seed",
            rule_request(
                "seed",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ),
        )
        .await;

    let cases: [(&str, Value); 4] = [
        (
            "ResourceId",
            json!({ "ResourceId": [{ "Value": "i-0", "Comparison": "EQUALS" }] }),
        ),
        (
            "ResourceTags",
            json!({ "ResourceTags": [{ "Key": "env", "Value": "prod", "Comparison": "EQUALS" }] }),
        ),
        (
            "UserDefinedFields",
            json!({ "UserDefinedFields": [{ "Key": "owner", "Value": "x", "Comparison": "EQUALS" }] }),
        ),
        (
            "RelatedFindingsId",
            json!({ "RelatedFindingsId": [{ "Value": "r-1", "Comparison": "EQUALS" }] }),
        ),
    ];

    for (case, criteria) in cases {
        let (status, body) = harness
            .create_rule(rule_request(
                "unevaluated",
                criteria.clone(),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ))
            .await;
        assert_eq!(
            status, 501,
            "case {case:?} on create: expected 501, got {status} with {body}"
        );

        let (status, body) = harness
            .update_rules(vec![json!({ "RuleArn": rule_arn, "Criteria": criteria })])
            .await;
        assert_eq!(
            status, 501,
            "case {case:?} on update: expected 501, got {status} with {body}"
        );
    }

    // The refused update left the rule exactly as it was.
    let (_, body) = harness.get_rules(vec![&rule_arn]).await;
    assert_eq!(
        body["Rules"][0]["Criteria"],
        json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
        "a refused update must not have changed the stored rule: {body}"
    );
}

/// RULE: a `Criteria` the filter shapes' own rules reject is refused with 400, on
/// create and on update alike — a stored rule may not carry a criterion that cannot
/// be evaluated.
#[tokio::test]
async fn illegal_criteria_are_refused() {
    let harness = Harness::subscribed().await;
    let rule_arn = harness
        .create_rule_ok(
            "seed",
            rule_request(
                "seed",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ),
        )
        .await;

    let cases: [(&str, Value); 8] = [
        (
            "CONTAINS with NOT_CONTAINS on one field",
            json!({ "Title": [
                { "Value": "a", "Comparison": "CONTAINS" },
                { "Value": "b", "Comparison": "NOT_CONTAINS" },
            ] }),
        ),
        (
            "EQUALS with PREFIX_NOT_EQUALS on one field",
            json!({ "Id": [
                { "Value": "finding-1", "Comparison": "EQUALS" },
                { "Value": "finding", "Comparison": "PREFIX_NOT_EQUALS" },
            ] }),
        ),
        (
            "CONTAINS_WORD is a V2-only comparison",
            json!({ "Title": [{ "Value": "Title", "Comparison": "CONTAINS_WORD" }] }),
        ),
        (
            "a comparison nothing evaluates",
            json!({ "Id": [{ "Value": "finding-1", "Comparison": "STARTS_WITH" }] }),
        ),
        (
            "StringFilter without a Value",
            json!({ "Id": [{ "Comparison": "EQUALS" }] }),
        ),
        ("NumberFilter without a bound", json!({ "Confidence": [{}] })),
        (
            "DateFilter.Start is not a timestamp",
            json!({ "UpdatedAt": [{ "Start": "yesterday" }] }),
        ),
        (
            "DateRange with a negative Value",
            json!({ "CreatedAt": [{ "DateRange": { "Value": -30, "Unit": "DAYS" } }] }),
        ),
    ];

    for (case, criteria) in cases {
        let (status, body) = harness
            .create_rule(rule_request(
                "illegal",
                criteria.clone(),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} on create: expected 400, got {status} with {body}"
        );

        let (status, body) = harness
            .update_rules(vec![json!({ "RuleArn": rule_arn, "Criteria": criteria })])
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} on update: expected 400, got {status} with {body}"
        );
    }
}

/// RULE: an account can hold at most the published quota of automation rules per
/// Region (100), so the create past it is refused with `LimitExceededException`
/// (429) and stores nothing.
#[tokio::test]
async fn a_create_past_the_rule_quota_is_refused() {
    let harness = Harness::subscribed().await;

    for index in 0..100 {
        let (status, body) = harness
            .create_rule(rule_request(
                &format!("quota-{index}"),
                json!({ "Id": [{ "Value": format!("finding-{index}"), "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ))
            .await;
        assert_eq!(
            status, 200,
            "rule {index} of the quota: expected 200, got {status} with {body}"
        );
    }

    let (status, body) = harness
        .create_rule(rule_request(
            "quota-101",
            json!({ "Id": [{ "Value": "finding-101", "Comparison": "EQUALS" }] }),
            finding_fields_update(json!({ "Confidence": 1 })),
            json!({}),
        ))
        .await;
    assert_eq!(
        status, 429,
        "the create past the quota: expected 429, got {status} with {body}"
    );

    // Refused, so nothing was stored for it: the hub still holds exactly the quota.
    let mut listed = 0usize;
    let mut query = "MaxResults=100".to_string();
    loop {
        let (status, body) = harness.list_rules(&query).await;
        assert_eq!(status, 200, "ListAutomationRules got {status} with {body}");
        listed += listed_arns(&body).len();
        match body["NextToken"].as_str() {
            Some(token) => query = format!("MaxResults=100&NextToken={token}"),
            None => break,
        }
    }
    assert_eq!(
        listed, 100,
        "a refused create must store nothing: expected 100 rules, got {listed}"
    );
}

/// RULE: a stored rule is bounded: the published breadth and depth limits on a
/// finding filter hold for `Criteria` just as they do for `GetFindings$Filters`, and
/// `RuleName` / `Description` are capped at the lengths this crate caps a finding's
/// `Title` / `Description` at — on create and on update alike, so one request can
/// neither store an unbounded record nor an unbounded amount of per-import work.
#[tokio::test]
async fn a_stored_rule_is_bounded_in_breadth_depth_and_length() {
    let harness = Harness::subscribed().await;
    let rule_arn = harness
        .create_rule_ok(
            "seed",
            rule_request(
                "bounded-seed",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ),
        )
        .await;

    // The member names the criteria shape carries, used to build a criteria that
    // filters on more attributes than the published breadth limit allows.
    const STRING_MEMBERS: [&str; 11] = [
        "Id",
        "ProductArn",
        "AwsAccountId",
        "GeneratorId",
        "Title",
        "Description",
        "SourceUrl",
        "ProductName",
        "CompanyName",
        "SeverityLabel",
        "RecordState",
    ];
    let criteria_with_members = |count: usize| {
        let mut criteria = serde_json::Map::new();
        for member in STRING_MEMBERS.iter().take(count) {
            criteria.insert(
                (*member).to_string(),
                json!([{ "Value": "x", "Comparison": "EQUALS" }]),
            );
        }
        Value::Object(criteria)
    };
    let criteria_with_filters = |count: usize| {
        let filters: Vec<Value> = (0..count)
            .map(|index| json!({ "Value": format!("value-{index}"), "Comparison": "EQUALS" }))
            .collect();
        json!({ "Id": filters })
    };

    // (case, criteria, expected status) — each published bound, and one past it.
    let criteria_cases: [(&str, Value, u16); 4] = [
        ("10 filtered attributes, the bound", criteria_with_members(10), 200),
        ("11 filtered attributes, one past it", criteria_with_members(11), 400),
        ("20 filter values on one attribute, the bound", criteria_with_filters(20), 200),
        ("21 filter values on one attribute, one past it", criteria_with_filters(21), 400),
    ];

    for (case, criteria, expected) in criteria_cases {
        let (status, body) = harness
            .create_rule(rule_request(
                "bounded",
                criteria.clone(),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            ))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?} on create: expected {expected}, got {status} with {body}"
        );

        let (status, body) = harness
            .update_rules(vec![json!({ "RuleArn": rule_arn, "Criteria": criteria })])
            .await;
        assert_eq!(
            status, expected,
            "case {case:?} on update: expected {expected}, got {status} with {body}"
        );
    }

    // (case, member, length, expected status) — each cap, and one byte past it.
    let text_cases: [(&str, &str, usize, u16); 4] = [
        ("RuleName at the cap", "RuleName", 256, 200),
        ("RuleName one past the cap", "RuleName", 257, 400),
        ("Description at the cap", "Description", 1024, 200),
        ("Description one past the cap", "Description", 1025, 400),
    ];

    for (case, member, length, expected) in text_cases {
        let text = "n".repeat(length);
        let (status, body) = harness
            .create_rule(rule_request(
                "bounded-text",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({ member: text.clone() }),
            ))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?} on create: expected {expected}, got {status} with {body}"
        );

        let (status, body) = harness
            .update_rules(vec![json!({ "RuleArn": rule_arn, member: text })])
            .await;
        assert_eq!(
            status, expected,
            "case {case:?} on update: expected {expected}, got {status} with {body}"
        );
    }
}

/// RULE: a rule's `Tags` are part of the stored resource, so the tagging operations
/// resolve a rule ARN exactly as they resolve the hub's: the tags a create carried
/// are listed back, `TagResource` adds to them, and an ARN that names no rule of the
/// caller is a 404.
#[tokio::test]
async fn a_rules_tags_are_readable_and_writable_through_the_tagging_operations() {
    let harness = Harness::subscribed().await;
    let rule_arn = harness
        .create_rule_ok(
            "tagged rule",
            rule_request(
                "tagged",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({ "Tags": { "team": "secops" } }),
            ),
        )
        .await;

    let (status, body) = harness
        .call("GET", &format!("/tags/{}", path_escape(&rule_arn)), None)
        .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource on a rule ARN: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["Tags"]["team"].as_str(),
        Some("secops"),
        "the tags the create carried must be listed back: {body}"
    );

    let (status, body) = harness
        .call(
            "POST",
            &format!("/tags/{}", path_escape(&rule_arn)),
            Some(json!({ "Tags": { "owner": "alice" } })),
        )
        .await;
    assert_eq!(
        status, 200,
        "TagResource on a rule ARN: expected 200, got {status} with {body}"
    );
    let (_, body) = harness
        .call("GET", &format!("/tags/{}", path_escape(&rule_arn)), None)
        .await;
    for (key, value) in [("team", "secops"), ("owner", "alice")] {
        assert_eq!(
            body["Tags"][key].as_str(),
            Some(value),
            "tag {key:?}: expected {value:?} after TagResource, got {body}"
        );
    }

    let unknown = format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:automation-rule/00000000-0000-4000-8000-000000000000");
    let (status, body) = harness
        .call("GET", &format!("/tags/{}", path_escape(&unknown)), None)
        .await;
    assert_eq!(
        status, 404,
        "an ARN that names no rule: expected 404, got {status} with {body}"
    );
}

/// Escape an ARN for the single, non-greedy `{ResourceArn}` path segment: only `/`
/// (and a literal `%`) would otherwise end the segment, and the server
/// percent-decodes the label back to the ARN.
fn path_escape(arn: &str) -> String {
    arn.replace('%', "%25").replace('/', "%2F")
}

/// RULE: an action is refused unless it is a `FINDING_FIELDS_UPDATE` carrying the
/// update it is meant to apply — an action type nothing can perform answers 501, and
/// a `FINDING_FIELDS_UPDATE` with nothing to update is a 400.
#[tokio::test]
async fn an_action_this_emulator_cannot_perform_is_refused() {
    let harness = Harness::subscribed().await;

    // (case, Actions, expected status)
    //
    // `AutomationRulesActionType` is a modelled enum, so the boundary validator
    // refuses a value outside it before this operation's handler runs: that refusal
    // is a 400, not the 501 an action kind nothing can perform would answer.
    let cases: [(&str, Value, u16); 3] = [
        (
            "an action type the model does not define",
            json!([{ "Type": "SEND_TO_TICKETING", "FindingFieldsUpdate": { "Confidence": 1 } }]),
            400,
        ),
        (
            "a FINDING_FIELDS_UPDATE with no update",
            json!([{ "Type": "FINDING_FIELDS_UPDATE" }]),
            400,
        ),
        (
            "an action with no Type",
            json!([{ "FindingFieldsUpdate": { "Confidence": 1 } }]),
            400,
        ),
    ];

    for (case, actions, expected) in cases {
        let (status, body) = harness
            .create_rule(rule_request(
                "refused-action",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                actions,
                json!({}),
            ))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {body}"
        );
    }
}

/// RULE: every automation-rule operation is for a subscribed hub, so an account that
/// has not called `EnableSecurityHub` is refused with `InvalidAccessException` (401)
/// rather than being handed an empty rule set.
#[tokio::test]
async fn automation_rule_operations_need_a_subscribed_hub() {
    let harness = Harness::new();
    let arn = format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:automation-rule/00000000-0000-4000-8000-000000000000");

    // (case, method, path, body)
    let cases: [(&str, &str, &str, Option<Value>); 5] = [
        (
            "CreateAutomationRule",
            "POST",
            "/automationrules/create",
            Some(rule_request(
                "unsubscribed",
                json!({ "Id": [{ "Value": "finding-1", "Comparison": "EQUALS" }] }),
                finding_fields_update(json!({ "Confidence": 1 })),
                json!({}),
            )),
        ),
        ("ListAutomationRules", "GET", "/automationrules/list", None),
        (
            "BatchGetAutomationRules",
            "POST",
            "/automationrules/get",
            Some(json!({ "AutomationRulesArns": [arn.clone()] })),
        ),
        (
            "BatchUpdateAutomationRules",
            "PATCH",
            "/automationrules/update",
            Some(json!({ "UpdateAutomationRulesRequestItems": [
                { "RuleArn": arn.clone(), "RuleStatus": "DISABLED" },
            ] })),
        ),
        (
            "BatchDeleteAutomationRules",
            "POST",
            "/automationrules/delete",
            Some(json!({ "AutomationRulesArns": [arn.clone()] })),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, response) = harness.call(method, path, body).await;
        assert_eq!(
            status, 401,
            "case {case:?}: expected 401 on an unsubscribed account, got {status} with {response}"
        );
    }
}
