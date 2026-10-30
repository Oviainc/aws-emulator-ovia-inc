// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's `@timestampFormat("date-time")` members.
//!
//! RULE under test: a member whose modelled target carries `@timestampFormat("date-time")`
//! travels on this service's restJson1 wire as an RFC 3339 / ISO-8601 STRING — never as a
//! bare epoch number — whichever shape, operation or member it is. Every timestamp member
//! of this service targets `com.amazonaws.securityhub#Timestamp`, and the model marks that
//! shape `@timestampFormat("date-time")` (`smithy_query(query="shape", shape="Timestamp")`
//! reports `"timestampFormat": "date-time"`), so the rule covers all of them.
//!
//! The rule is pinned twice, because two different things can drop the format:
//!
//! * on the WIRE, end to end through `MultiServiceRouter`, which is what an SDK client
//!   sees and what a typed client's generated deserializer refuses when the member is a
//!   number;
//! * on the SCHEMA, because the member schema's own `timestamp_format()` is the single
//!   thing the JSON codec reads to decide the rendering — a member schema that reports no
//!   format silently falls back to the protocol default, epoch seconds.

use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::{
    AutomationRulesConfig, AutomationRulesMetadata, ConfigurationPolicyAssociationSummary,
    FindingHistoryRecord, Invitation, Member,
};
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

    /// A harness whose hub is already subscribed, which every operation below requires.
    async fn subscribed() -> Self {
        let harness = Self::new();
        let (status, body) = harness.call("POST", "/accounts", Some(json!({}))).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// Dispatch one request and return `(status, parsed body)`.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261029/{REGION}/securityhub/aws4_request, \
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

    /// Dispatch one request, assert it succeeded, and answer the parsed body.
    async fn ok(&self, case: &str, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, parsed) = self.call(method, path, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {method} {path} expected 200, got {status} with {parsed}"
        );
        parsed
    }
}

/// A complete `CreateAutomationRule` request body: `name` and `order` vary per case, the
/// rest is the one `AutomationRulesActionType` the model defines.
fn rule_request(name: &str, order: i64) -> Value {
    json!({
        "RuleName": name,
        "Description": format!("Rule {name}"),
        "RuleOrder": order,
        "Criteria": { "SeverityLabel": [{ "Value": "HIGH", "Comparison": "EQUALS" }] },
        "Actions": [{
            "Type": "FINDING_FIELDS_UPDATE",
            "FindingFieldsUpdate": { "Workflow": { "Status": "SUPPRESSED" } },
        }],
    })
}

/// A finding that satisfies every documented `AwsSecurityFinding` constraint.
fn finding(id: &str) -> Value {
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
        "Severity": { "Label": "MEDIUM" },
        "RecordState": "ACTIVE",
        "Workflow": { "Status": "NEW" },
        "Types": ["Software and Configuration Checks/Vulnerabilities/CVE"],
    })
}

/// A 12-digit member account id built from `n`, so a case can span batch sizes without a
/// hand-written list.
fn account_id(n: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + n as u64)
}

/// The instant a `date-time` member carries, after asserting the member's WIRE FORM: a
/// JSON string in the canonical RFC 3339 rendering of that instant, and not a number.
///
/// Returns the instant so a caller can also assert an ordering between two members.
fn date_time_of(case: &str, member: &str, value: &Value) -> DateTime {
    assert!(
        !value.is_number(),
        "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
         an RFC 3339 string and never an epoch number, got {value:?}"
    );
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
             a JSON string, got {value:?}"
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must be a readable RFC 3339 instant, got {text:?}: {error}")
    });
    let rendered = parsed.fmt(Format::DateTime).unwrap_or_default();
    assert_eq!(
        rendered, text,
        "case {case:?}: {member} must carry the canonical RFC 3339 rendering of its instant, \
         got {text:?}"
    );
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name an instant after the epoch, got {text:?}"
    );
    parsed
}

/// RULE on the wire: every `date-time` member an implemented operation reports comes back
/// as an RFC 3339 string, whichever shape and operation carries it.
///
/// The table spans what the rule generalises over: the shape the member lives in
/// (`AutomationRulesConfig`, `AutomationRulesMetadata`, `Member`, `FindingHistoryRecord`),
/// the operation that reports it (a batch read, a list, a member read, a history read), the
/// member itself (`CreatedAt`, `UpdatedAt`, `UpdateTime`), how many elements the response
/// carries (1 → 3), and whether the resource has been updated since it was created.
#[tokio::test]
async fn every_date_time_member_travels_as_an_rfc_3339_string() {
    let harness = Harness::subscribed().await;

    // Two rules, so a response carrying more than one element is covered, and so one of
    // them can be updated while the other is not.
    let mut rule_arns: Vec<String> = Vec::new();
    for (name, order) in [("fresh-rule", 1), ("updated-rule", 1000)] {
        let created = harness
            .ok(name, "POST", "/automationrules/create", Some(rule_request(name, order)))
            .await;
        rule_arns.push(
            created["RuleArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name:?}: response carries no RuleArn: {created}"))
                .to_string(),
        );
    }
    let updated_arn = rule_arns[1].clone();
    harness
        .ok(
            "updated-rule",
            "PATCH",
            "/automationrules/update",
            Some(json!({
                "UpdateAutomationRulesRequestItems": [{
                    "RuleArn": updated_arn,
                    "Description": "Rule updated-rule, revised",
                }],
            })),
        )
        .await;

    // Three member accounts, so the Members list spans one and many.
    let member_ids: Vec<String> = (0..3).map(account_id).collect();
    let details: Vec<Value> = member_ids
        .iter()
        .map(|id| json!({ "AccountId": id, "Email": format!("{id}@example.com") }))
        .collect();
    harness
        .ok(
            "members",
            "POST",
            "/members",
            Some(json!({ "AccountDetails": details })),
        )
        .await;

    // One imported finding, whose change event is what GetFindingHistory reports.
    harness
        .ok(
            "finding",
            "POST",
            "/findings/import",
            Some(json!({ "Findings": [finding("finding/timestamps")] })),
        )
        .await;

    // Each row: the case name, the member's dotted path, and the value the wire carried.
    let mut rows: Vec<(String, String, Value)> = Vec::new();

    let batch = harness
        .ok(
            "BatchGetAutomationRules",
            "POST",
            "/automationrules/get",
            Some(json!({ "AutomationRulesArns": rule_arns })),
        )
        .await;
    let reported_rules = batch["Rules"]
        .as_array()
        .unwrap_or_else(|| panic!("BatchGetAutomationRules must answer Rules, got {batch}"));
    assert_eq!(
        reported_rules.len(),
        2,
        "BatchGetAutomationRules must report both rules, got {batch}"
    );
    for rule in reported_rules {
        let name = rule["RuleName"].as_str().unwrap_or("<unnamed>").to_string();
        let case = format!("BatchGetAutomationRules {name}");
        let created = date_time_of(&case, "Rules[].CreatedAt", &rule["CreatedAt"]);
        let updated = date_time_of(&case, "Rules[].UpdatedAt", &rule["UpdatedAt"]);
        assert!(
            updated.as_nanos() >= created.as_nanos(),
            "case {case:?}: UpdatedAt must not precede CreatedAt, got {rule}"
        );
        if name == "fresh-rule" {
            assert_eq!(
                rule["UpdatedAt"], rule["CreatedAt"],
                "case {case:?}: a rule that was never updated reports UpdatedAt == CreatedAt, \
                 got {rule}"
            );
        } else {
            assert!(
                updated.as_nanos() > created.as_nanos(),
                "case {case:?}: an updated rule reports a later UpdatedAt, got {rule}"
            );
        }
        rows.push((case.clone(), "Rules[].CreatedAt".to_string(), rule["CreatedAt"].clone()));
        rows.push((case, "Rules[].UpdatedAt".to_string(), rule["UpdatedAt"].clone()));
    }

    let listed = harness
        .ok("ListAutomationRules", "GET", "/automationrules/list", None)
        .await;
    let metadata = listed["AutomationRulesMetadata"]
        .as_array()
        .unwrap_or_else(|| panic!("ListAutomationRules must answer metadata, got {listed}"));
    assert_eq!(
        metadata.len(),
        2,
        "ListAutomationRules must report both rules, got {listed}"
    );
    for rule in metadata {
        let name = rule["RuleName"].as_str().unwrap_or("<unnamed>");
        let case = format!("ListAutomationRules {name}");
        for member in ["CreatedAt", "UpdatedAt"] {
            rows.push((
                case.clone(),
                format!("AutomationRulesMetadata[].{member}"),
                rule[member].clone(),
            ));
        }
    }

    let members = harness
        .ok(
            "GetMembers",
            "POST",
            "/members/get",
            Some(json!({ "AccountIds": member_ids })),
        )
        .await;
    let reported_members = members["Members"]
        .as_array()
        .unwrap_or_else(|| panic!("GetMembers must answer Members, got {members}"));
    assert_eq!(
        reported_members.len(),
        3,
        "GetMembers must report every association, got {members}"
    );
    for member in reported_members {
        let id = member["AccountId"].as_str().unwrap_or("<unknown>");
        rows.push((
            format!("GetMembers {id}"),
            "Members[].UpdatedAt".to_string(),
            member["UpdatedAt"].clone(),
        ));
        // No invitation has been sent, so `InvitedAt` is absent rather than an instant:
        // the rule is about the FORM of a present member, not about inventing one.
        assert!(
            member["InvitedAt"].is_null(),
            "case \"GetMembers {id}\": an uninvited member carries no InvitedAt, got {member}"
        );
    }

    let history = harness
        .ok(
            "GetFindingHistory",
            "POST",
            "/findingHistory/get",
            Some(json!({
                "FindingIdentifier": { "Id": "finding/timestamps", "ProductArn": PRODUCT_ARN },
            })),
        )
        .await;
    let records = history["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("GetFindingHistory must answer Records, got {history}"));
    assert!(
        !records.is_empty(),
        "GetFindingHistory must report the import event, got {history}"
    );
    for (index, record) in records.iter().enumerate() {
        rows.push((
            format!("GetFindingHistory record {index}"),
            "Records[].UpdateTime".to_string(),
            record["UpdateTime"].clone(),
        ));
    }

    assert!(
        rows.len() >= 12,
        "the table must cover every reported date-time member, got {} rows",
        rows.len()
    );
    for (case, member, value) in &rows {
        date_time_of(case, member, value);
    }
}

/// Every member of one shape, as the model declares it: the wire name, and whether the
/// model gives that member `@timestampFormat("date-time")`.
type ShapeMembers = (&'static str, &'static [(&'static str, bool)]);

/// RULE on the schema: every member the model marks `@timestampFormat("date-time")` reports
/// that format on its own member schema, and a member the model gives no `@timestampFormat`
/// reports none — for every shape of this service that carries a timestamp member, whether
/// or not an implemented operation serializes it yet.
///
/// The member lists come from `smithy_query(query="shape", shape=<Shape>)`, in declaration
/// order; they are also the order each struct declares its fields in. The rows include two
/// shapes the issue never cited (`Invitation`, `ConfigurationPolicyAssociationSummary`), so
/// the rule is pinned beyond the reported operations.
#[test]
fn every_date_time_member_schema_carries_the_modelled_format() {
    let shapes: [(ShapeMembers, &[&aws_smithy_schema::Schema]); 6] = [
        (
            (
                "AutomationRulesConfig",
                &[
                    ("RuleArn", false),
                    ("RuleStatus", false),
                    ("RuleOrder", false),
                    ("RuleName", false),
                    ("Description", false),
                    ("IsTerminal", false),
                    ("Criteria", false),
                    ("Actions", false),
                    ("CreatedAt", true),
                    ("UpdatedAt", true),
                    ("CreatedBy", false),
                ],
            ),
            AutomationRulesConfig::SCHEMA.members(),
        ),
        (
            (
                "AutomationRulesMetadata",
                &[
                    ("RuleArn", false),
                    ("RuleStatus", false),
                    ("RuleOrder", false),
                    ("RuleName", false),
                    ("Description", false),
                    ("IsTerminal", false),
                    ("CreatedAt", true),
                    ("UpdatedAt", true),
                    ("CreatedBy", false),
                ],
            ),
            AutomationRulesMetadata::SCHEMA.members(),
        ),
        (
            (
                "Member",
                &[
                    ("AccountId", false),
                    ("Email", false),
                    ("MasterId", false),
                    ("AdministratorId", false),
                    ("MemberStatus", false),
                    ("InvitedAt", true),
                    ("UpdatedAt", true),
                ],
            ),
            Member::SCHEMA.members(),
        ),
        (
            (
                "Invitation",
                &[
                    ("AccountId", false),
                    ("InvitationId", false),
                    ("InvitedAt", true),
                    ("MemberStatus", false),
                ],
            ),
            Invitation::SCHEMA.members(),
        ),
        (
            (
                "ConfigurationPolicyAssociationSummary",
                &[
                    ("ConfigurationPolicyId", false),
                    ("TargetId", false),
                    ("TargetType", false),
                    ("AssociationType", false),
                    ("UpdatedAt", true),
                    ("AssociationStatus", false),
                    ("AssociationStatusMessage", false),
                ],
            ),
            ConfigurationPolicyAssociationSummary::SCHEMA.members(),
        ),
        (
            (
                "FindingHistoryRecord",
                &[
                    ("FindingIdentifier", false),
                    ("UpdateTime", true),
                    ("FindingCreated", false),
                    ("UpdateSource", false),
                    ("Updates", false),
                    ("NextToken", false),
                ],
            ),
            FindingHistoryRecord::SCHEMA.members(),
        ),
    ];

    for ((shape, modelled), members) in shapes {
        let names: Vec<&str> = members
            .iter()
            .map(|member| member.member_name().unwrap_or("<unnamed>"))
            .collect();
        let expected: Vec<&str> = modelled.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names, expected,
            "case {shape:?}: the schema must carry exactly the model's members, in order"
        );

        for (position, member) in members.iter().enumerate() {
            let name = member.member_name().unwrap_or("<unnamed>");
            let case = format!("{shape}${name}");
            assert_eq!(
                member.member_index(),
                Some(position),
                "case {case:?}: member_index must equal the member's position, got {:?}",
                member.member_index()
            );
            let format = member.timestamp_format().map(|trait_| trait_.format());
            if modelled[position].1 {
                assert_eq!(
                    format,
                    Some(TimestampFormat::DateTime),
                    "case {case:?}: the model gives this member @timestampFormat(\"date-time\"), \
                     so its schema must carry that format, got {format:?}"
                );
            } else {
                assert_eq!(
                    format, None,
                    "case {case:?}: the model gives this member no @timestampFormat, \
                     got {format:?}"
                );
            }
        }
    }
}
