// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Security Hub CSPM core loop: EnableSecurityHub →
//! DescribeHub → BatchImportFindings → GetFindings → BatchUpdateFindings →
//! DisableSecurityHub.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test signs for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
pub(crate) const REGION: &str = "us-east-1";

/// The account every emulated caller presents.
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// A product ARN of the shape Security Hub's own product uses.
pub(crate) const PRODUCT_ARN: &str = "arn:aws:securityhub:us-east-1::product/aws/securityhub";

/// The only schema version the `AwsSecurityFinding` format defines.
pub(crate) const SCHEMA_VERSION: &str = "2018-10-08";

/// One emulator with the Security Hub service mounted, driven over HTTP. Shared with
/// the other suites in this crate's `tests/all/`.
pub(crate) struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request and return `(status, parsed body)`. An empty body parses
    /// as `Value::Null`, which is what an operation with no output members returns.
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
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

    pub(crate) async fn enable(&self, body: Option<Value>) -> (u16, Value) {
        self.call("POST", "/accounts", body.or(Some(json!({})))).await
    }

    async fn describe_hub(&self, hub_arn: Option<&str>) -> (u16, Value) {
        let path = match hub_arn {
            Some(arn) => format!("/accounts?HubArn={arn}"),
            None => "/accounts".to_string(),
        };
        self.call("GET", &path, None).await
    }

    pub(crate) async fn disable(&self) -> (u16, Value) {
        self.call("DELETE", "/accounts", None).await
    }

    pub(crate) async fn import(&self, findings: Vec<Value>) -> (u16, Value) {
        self.call("POST", "/findings/import", Some(json!({ "Findings": findings })))
            .await
    }

    pub(crate) async fn get_findings(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/findings", Some(body)).await
    }

    pub(crate) async fn update_findings(&self, body: Value) -> (u16, Value) {
        self.call("PATCH", "/findings/batchupdate", Some(body)).await
    }

    /// Enable the hub and import `findings`, asserting both succeed.
    pub(crate) async fn with_findings(findings: Vec<Value>) -> Self {
        let harness = Self::new();
        let (status, _) = harness.enable(None).await;
        assert_eq!(status, 200, "EnableSecurityHub must succeed, got {status}");
        let count = findings.len();
        let (status, body) = harness.import(findings).await;
        assert_eq!(status, 200, "BatchImportFindings must succeed, got {status}");
        assert_eq!(
            body["SuccessCount"].as_u64(),
            Some(count as u64),
            "every seeded finding must import: {body}"
        );
        harness
    }
}

/// A finding that satisfies every documented `AwsSecurityFinding` constraint.
pub(crate) fn base_finding(id: &str) -> Value {
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

/// The ISO-8601 instant `seconds_ago` before now, in the `date-time` encoding a
/// `AwsSecurityFinding` timestamp carries (whole seconds, UTC).
///
/// A test whose expectation depends on a window measured back from NOW — a `DateRange`
/// filter, say — has to place its fixture relative to now too: a hard-coded instant
/// stops being inside the window once the wall clock passes it.
pub(crate) fn iso8601_before_now(seconds_ago: u64) -> String {
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the clock must be after the Unix epoch")
        .as_secs();
    let seconds = i64::try_from(now.saturating_sub(seconds_ago)).expect("instant fits in an i64");
    aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a date-time rendering of an epoch-second instant")
}

/// The ids in `Findings`, in the order the response listed them.
pub(crate) fn returned_ids(body: &Value) -> Vec<String> {
    body["Findings"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|f| f["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The one finding in `Findings` with `id`.
pub(crate) fn finding_with_id(body: &Value, id: &str) -> Value {
    body["Findings"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|f| f["Id"].as_str() == Some(id))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// One row of the import-rejection table: the case name, the mutation that breaks a
/// documented constraint, and the member the error message must name.
type RejectionCase = (&'static str, fn(&mut Value), &'static str);

/// One row of the update table: the case name, the update members to send, and a
/// check that returns what is wrong with the stored finding, or `None`.
type UpdateCase = (&'static str, Value, fn(&Value) -> Option<String>);

// ---------------------------------------------------------------------------
// EnableSecurityHub / DescribeHub / DisableSecurityHub
// ---------------------------------------------------------------------------

/// RULE: `DescribeHub` reports the hub record `EnableSecurityHub` persisted — the
/// documented defaults for members the request omitted, and the submitted value for
/// every member it carried — identically on every call.
///
/// The table spans the request forms `EnableSecurityHub` accepts: no members at all,
/// each enum value of `ControlFindingGenerator`, `EnableDefaultStandards` both ways,
/// and a tag map.
#[tokio::test]
async fn describe_hub_reports_what_enable_security_hub_persisted() {
    // (case, request body, expected ControlFindingGenerator)
    let cases: [(&str, Option<Value>, &str); 5] = [
        // Documented default: "the default value of this field is SECURITY_CONTROL if
        // you enabled Security Hub CSPM on or after February 23, 2023".
        ("no members", None, "SECURITY_CONTROL"),
        (
            "explicit STANDARD_CONTROL",
            Some(json!({ "ControlFindingGenerator": "STANDARD_CONTROL" })),
            "STANDARD_CONTROL",
        ),
        (
            "explicit SECURITY_CONTROL",
            Some(json!({ "ControlFindingGenerator": "SECURITY_CONTROL" })),
            "SECURITY_CONTROL",
        ),
        (
            "default standards off",
            Some(json!({ "EnableDefaultStandards": false })),
            "SECURITY_CONTROL",
        ),
        (
            "tagged, default standards on",
            Some(json!({
                "EnableDefaultStandards": true,
                "ControlFindingGenerator": "STANDARD_CONTROL",
                "Tags": { "Team": "sec", "Env": "prod" },
            })),
            "STANDARD_CONTROL",
        ),
    ];

    let expected_arn = format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:hub/default");

    for (case, body, expected_generator) in cases {
        let harness = Harness::new();

        let (status, _) = harness.enable(body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");

        let (status, first) = harness.describe_hub(None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeHub expected 200, got {status}"
        );
        assert_eq!(
            first["HubArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected HubArn {expected_arn}, got {:?}",
            first["HubArn"]
        );
        assert_eq!(
            first["ControlFindingGenerator"].as_str(),
            Some(expected_generator),
            "case {case:?}: expected ControlFindingGenerator {expected_generator}, got {:?}",
            first["ControlFindingGenerator"]
        );
        assert_eq!(
            first["AutoEnableControls"].as_bool(),
            Some(true),
            "case {case:?}: expected AutoEnableControls true, got {:?}",
            first["AutoEnableControls"]
        );
        let subscribed_at = first["SubscribedAt"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !subscribed_at.is_empty(),
            "case {case:?}: expected a SubscribedAt timestamp, got {:?}",
            first["SubscribedAt"]
        );

        // Reading twice must not change what is reported.
        let (_, second) = harness.describe_hub(None).await;
        assert_eq!(
            second, first,
            "case {case:?}: DescribeHub must be stable across calls, got {second:?} after {first:?}"
        );

        // Selecting the hub by its own ARN reports the same record.
        let (status, by_arn) = harness.describe_hub(Some(&expected_arn)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeHub by ARN expected 200, got {status}"
        );
        assert_eq!(
            by_arn, first,
            "case {case:?}: DescribeHub by ARN must report the same hub, got {by_arn:?}"
        );
    }
}

/// RULE: a resource that already exists cannot be created a second time —
/// `EnableSecurityHub` on a subscribed account/Region is `ResourceConflictException`
/// (409), and the first subscription is untouched.
#[tokio::test]
async fn enabling_an_already_subscribed_account_conflicts() {
    let harness = Harness::new();
    let (status, _) = harness.enable(None).await;
    assert_eq!(status, 200, "first enable: expected 200, got {status}");
    let (_, before) = harness.describe_hub(None).await;

    for attempt in 1..=2 {
        let (status, body) = harness
            .enable(Some(json!({ "ControlFindingGenerator": "STANDARD_CONTROL" })))
            .await;
        assert_eq!(
            status, 409,
            "attempt {attempt}: expected 409, got {status} with {body}"
        );
    }

    let (_, after) = harness.describe_hub(None).await;
    assert_eq!(
        after, before,
        "a refused enable must not change the hub, got {after:?} after {before:?}"
    );
}

/// RULE: operations on an account that is not subscribed are refused with
/// `InvalidAccessException` (401), which is how Security Hub reports "this account
/// has no subscription"; `DisableSecurityHub` has no hub to delete and answers
/// `ResourceNotFoundException` (404).
///
/// The table spans the operations in this slice that read or write hub-scoped state.
#[tokio::test]
async fn an_unsubscribed_account_is_refused() {
    // (case, method, path, body, expected status)
    let probes: [(&str, &str, &str, Option<Value>, u16); 4] = [
        ("DescribeHub", "GET", "/accounts", None, 401),
        (
            "BatchImportFindings",
            "POST",
            "/findings/import",
            Some(json!({ "Findings": [base_finding("finding-1")] })),
            401,
        ),
        ("GetFindings", "POST", "/findings", Some(json!({})), 401),
        (
            "BatchUpdateFindings",
            "PATCH",
            "/findings/batchupdate",
            Some(json!({
                "FindingIdentifiers": [{ "Id": "finding-1", "ProductArn": PRODUCT_ARN }],
                "Workflow": { "Status": "RESOLVED" },
            })),
            401,
        ),
    ];

    // Before any subscription exists ...
    let harness = Harness::new();
    for (case, method, path, body, expected) in probes.clone() {
        let (status, payload) = harness.call(method, path, body).await;
        assert_eq!(
            status, expected,
            "case {case:?} before enable: expected {expected}, got {status} with {payload}"
        );
    }
    let (status, payload) = harness.disable().await;
    assert_eq!(
        status, 404,
        "DisableSecurityHub before enable: expected 404, got {status} with {payload}"
    );

    // ... and again after the subscription is removed.
    let (status, _) = harness.enable(None).await;
    assert_eq!(status, 200, "enable: expected 200, got {status}");
    let (status, _) = harness.import(vec![base_finding("finding-1")]).await;
    assert_eq!(status, 200, "import: expected 200, got {status}");
    let (status, payload) = harness.disable().await;
    assert_eq!(
        status, 200,
        "DisableSecurityHub: expected 200, got {status} with {payload}"
    );

    for (case, method, path, body, expected) in probes {
        let (status, payload) = harness.call(method, path, body).await;
        assert_eq!(
            status, expected,
            "case {case:?} after disable: expected {expected}, got {status} with {payload}"
        );
    }
    let (status, payload) = harness.disable().await;
    assert_eq!(
        status, 404,
        "second DisableSecurityHub: expected 404, got {status} with {payload}"
    );

    // Re-enabling starts from an empty hub: disable really removed the findings.
    let (status, _) = harness.enable(None).await;
    assert_eq!(status, 200, "re-enable: expected 200, got {status}");
    let (status, body) = harness.get_findings(json!({})).await;
    assert_eq!(status, 200, "GetFindings after re-enable: got {status}");
    assert_eq!(
        returned_ids(&body),
        Vec::<String>::new(),
        "a re-enabled hub must hold no findings, got {body}"
    );
}

/// RULE: `DescribeHub`'s `HubArn` selects which hub to report, and an account/Region
/// owns exactly one — so any other ARN names a resource that does not exist (404).
#[tokio::test]
async fn describe_hub_rejects_an_arn_that_names_no_hub() {
    let harness = Harness::new();
    let (status, _) = harness.enable(None).await;
    assert_eq!(status, 200, "enable: expected 200, got {status}");

    let cases = [
        "arn:aws:securityhub:eu-west-1:000000000000:hub/default",
        "arn:aws:securityhub:us-east-1:000000000000:hub/other",
        "not-an-arn",
    ];
    for case in cases {
        let (status, body) = harness.describe_hub(Some(case)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// BatchImportFindings / GetFindings
// ---------------------------------------------------------------------------

/// RULE: `GetFindings` returns the findings `BatchImportFindings` stored, with the
/// members that were submitted, and a re-import of the same
/// `(ProductArn, Id)` updates that one record instead of adding another.
///
/// The table spans findings that differ in the members a client varies: severity,
/// record state, workflow status, type count, resource count, and the optional
/// members a provider may or may not send.
#[tokio::test]
async fn imported_findings_come_back_from_get_findings() {
    let mut low = base_finding("finding-low");
    low["Severity"] = json!({ "Label": "LOW" });
    low["RecordState"] = json!("ARCHIVED");
    low["Types"] = json!([]);

    let mut rich = base_finding("finding-rich");
    rich["Severity"] = json!({ "Label": "CRITICAL", "Product": 9.5, "Original": "SEV0" });
    rich["Confidence"] = json!(42);
    rich["Criticality"] = json!(99);
    rich["UserDefinedFields"] = json!({ "owner": "team-sec" });
    rich["SourceUrl"] = json!("https://example.invalid/findings/rich");
    rich["Resources"] = json!([
        { "Type": "AwsEc2Instance", "Id": "i-0123456789abcdef0" },
        { "Type": "AwsS3Bucket", "Id": "arn:aws:s3:::bucket-one" },
    ]);
    rich["Types"] = json!([
        "Software and Configuration Checks/Vulnerabilities/CVE",
        "Effects/Data Exposure",
    ]);

    let mut minimal = base_finding("finding-minimal");
    minimal.as_object_mut().expect("object").remove("Severity");
    minimal.as_object_mut().expect("object").remove("Workflow");
    minimal.as_object_mut().expect("object").remove("Types");
    minimal.as_object_mut().expect("object").remove("RecordState");
    // A conforming finding populates `Severity` or `FindingProviderFields`; this one
    // takes the second branch of that documented `anyOf`.
    minimal["FindingProviderFields"] = json!({ "Severity": { "Label": "LOW" } });

    let seeded = vec![base_finding("finding-base"), low, rich, minimal];
    let harness = Harness::with_findings(seeded.clone()).await;

    let (status, body) = harness.get_findings(json!({})).await;
    assert_eq!(status, 200, "GetFindings: expected 200, got {status}");
    let mut ids = returned_ids(&body);
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "finding-base".to_string(),
            "finding-low".to_string(),
            "finding-minimal".to_string(),
            "finding-rich".to_string()
        ],
        "an absent Filters must return every stored finding, got {body}"
    );

    for submitted in &seeded {
        let id = submitted["Id"].as_str().expect("an Id");
        let stored = finding_with_id(&body, id);
        for (member, expected) in submitted.as_object().expect("object") {
            // `Severity` is completed on import (a submitted `Label` gains the
            // documented `Normalized`), so it has its own test; every other
            // submitted member comes back byte-for-byte.
            if member == "Severity" {
                assert_eq!(
                    stored["Severity"]["Label"], expected["Label"],
                    "case {id:?}: Severity.Label expected {:?}, got {:?}",
                    expected["Label"], stored["Severity"]["Label"]
                );
                continue;
            }
            assert_eq!(
                &stored[member], expected,
                "case {id:?}: member {member} expected {expected}, got {:?}",
                stored[member]
            );
        }
        assert_eq!(
            stored["Region"].as_str(),
            Some(REGION),
            "case {id:?}: expected the service-populated Region {REGION}, got {:?}",
            stored["Region"]
        );
        assert!(
            stored["ProcessedAt"].is_string(),
            "case {id:?}: expected a service-populated ProcessedAt, got {:?}",
            stored["ProcessedAt"]
        );
    }

    // Re-importing one identifier updates that record; the collection does not grow.
    let mut reimported = base_finding("finding-base");
    reimported["Title"] = json!("Retitled by a re-import");
    reimported["Severity"] = json!({ "Label": "MEDIUM" });
    let (status, import_body) = harness.import(vec![reimported]).await;
    assert_eq!(status, 200, "re-import: expected 200, got {status}");
    assert_eq!(
        (
            import_body["SuccessCount"].as_u64(),
            import_body["FailedCount"].as_u64()
        ),
        (Some(1), Some(0)),
        "re-import counts: got {import_body}"
    );

    let (_, body) = harness.get_findings(json!({})).await;
    assert_eq!(
        returned_ids(&body).len(),
        4,
        "a re-import must update, not duplicate: got {body}"
    );
    let updated = finding_with_id(&body, "finding-base");
    assert_eq!(
        updated["Title"].as_str(),
        Some("Retitled by a re-import"),
        "the re-imported Title must replace the stored one, got {:?}",
        updated["Title"]
    );
}

/// RULE: a finding that violates one of the length / count / format constraints the
/// `AwsSecurityFinding` shape documents is reported in `FailedFindings` with the
/// member and constraint it broke, and the rest of the batch is still stored —
/// `SuccessCount` and `FailedCount` count what actually happened.
#[tokio::test]
async fn import_rejects_findings_that_break_a_documented_constraint() {
    // (case, mutation, the member the error message must name)
    let cases: [RejectionCase; 10] = [
        (
            "wrong schema version",
            |f| f["SchemaVersion"] = json!("2018-10-07"),
            "SchemaVersion",
        ),
        (
            "Id one past the 512 maximum",
            |f| f["Id"] = json!("x".repeat(513)),
            "Id",
        ),
        (
            "Title one past the 256 maximum",
            |f| f["Title"] = json!("t".repeat(257)),
            "Title",
        ),
        (
            "Description one past the 1024 maximum",
            |f| f["Description"] = json!("d".repeat(1025)),
            "Description",
        ),
        (
            "AwsAccountId shorter than 12",
            |f| f["AwsAccountId"] = json!("12345678901"),
            "AwsAccountId",
        ),
        (
            "Resources below the 1 minimum",
            |f| f["Resources"] = json!([]),
            "Resources",
        ),
        (
            "Types one past the 50 maximum",
            |f| f["Types"] = json!(vec!["Effects/Data Exposure"; 51]),
            "Types",
        ),
        (
            "RelatedFindings one past the 10 maximum",
            |f| {
                f["RelatedFindings"] = json!(vec![
                    json!({ "ProductArn": PRODUCT_ARN, "Id": "related" });
                    11
                ]);
            },
            "RelatedFindings",
        ),
        (
            "RelatedFindings below the 1 minimum",
            |f| f["RelatedFindings"] = json!([]),
            "RelatedFindings",
        ),
        (
            "UpdatedAt is not a timestamp",
            |f| f["UpdatedAt"] = json!("yesterday"),
            "UpdatedAt",
        ),
    ];

    for (index, (case, mutate, member)) in cases.into_iter().enumerate() {
        let harness = Harness::new();
        let (status, _) = harness.enable(None).await;
        assert_eq!(status, 200, "case {case:?}: enable got {status}");

        let bad_id = format!("bad-{index}");
        let mut bad = base_finding(&bad_id);
        mutate(&mut bad);
        let good = base_finding("good");

        let (status, body) = harness.import(vec![bad.clone(), good]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");
        assert_eq!(
            (body["SuccessCount"].as_u64(), body["FailedCount"].as_u64()),
            (Some(1), Some(1)),
            "case {case:?}: expected 1 success and 1 failure, got {body}"
        );
        let failed = body["FailedFindings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected one FailedFindings entry, got {body}"
        );
        assert_eq!(
            failed[0]["ErrorCode"].as_str(),
            Some("InvalidInput"),
            "case {case:?}: expected ErrorCode InvalidInput, got {:?}",
            failed[0]["ErrorCode"]
        );
        assert_eq!(
            failed[0]["Id"].as_str(),
            bad["Id"].as_str(),
            "case {case:?}: the failure must name the submitted Id, got {:?}",
            failed[0]["Id"]
        );
        let message = failed[0]["ErrorMessage"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: expected ErrorMessage to name {member}, got {message:?}"
        );

        // Only the valid finding was stored.
        let (_, found) = harness.get_findings(json!({})).await;
        assert_eq!(
            returned_ids(&found),
            vec!["good".to_string()],
            "case {case:?}: only the valid finding must be stored, got {found}"
        );
    }
}

/// RULE: `Severity` fills in whichever of `Label` / `Normalized` the submitter left
/// out, from the other, per the two mappings the `Severity` and `SeverityUpdate`
/// shapes document.
///
/// The table covers every published `SeverityLabel`, both documented directions, and
/// one value inside each `Normalized` band including both of its bounds.
#[tokio::test]
async fn severity_label_and_normalized_complete_each_other() {
    // (case, submitted Severity, expected Label, expected Normalized)
    let cases: [(&str, Value, &str, i64); 13] = [
        ("label INFORMATIONAL", json!({ "Label": "INFORMATIONAL" }), "INFORMATIONAL", 0),
        ("label LOW", json!({ "Label": "LOW" }), "LOW", 1),
        ("label MEDIUM", json!({ "Label": "MEDIUM" }), "MEDIUM", 40),
        ("label HIGH", json!({ "Label": "HIGH" }), "HIGH", 70),
        ("label CRITICAL", json!({ "Label": "CRITICAL" }), "CRITICAL", 90),
        ("normalized 0", json!({ "Normalized": 0 }), "INFORMATIONAL", 0),
        ("normalized 1 (LOW lower bound)", json!({ "Normalized": 1 }), "LOW", 1),
        ("normalized 39 (LOW upper bound)", json!({ "Normalized": 39 }), "LOW", 39),
        ("normalized 40 (MEDIUM lower bound)", json!({ "Normalized": 40 }), "MEDIUM", 40),
        ("normalized 69 (MEDIUM upper bound)", json!({ "Normalized": 69 }), "MEDIUM", 69),
        ("normalized 70 (HIGH lower bound)", json!({ "Normalized": 70 }), "HIGH", 70),
        ("normalized 89 (HIGH upper bound)", json!({ "Normalized": 89 }), "HIGH", 89),
        ("normalized 100 (CRITICAL upper bound)", json!({ "Normalized": 100 }), "CRITICAL", 100),
    ];

    let findings: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(index, (_, severity, _, _))| {
            let mut finding = base_finding(&format!("severity-{index}"));
            finding["Severity"] = severity.clone();
            finding
        })
        .collect();
    let harness = Harness::with_findings(findings).await;
    let (_, body) = harness.get_findings(json!({})).await;

    for (index, (case, _, expected_label, expected_normalized)) in cases.iter().enumerate() {
        let stored = finding_with_id(&body, &format!("severity-{index}"));
        assert_eq!(
            stored["Severity"]["Label"].as_str(),
            Some(*expected_label),
            "case {case:?}: expected Label {expected_label}, got {:?}",
            stored["Severity"]["Label"]
        );
        assert_eq!(
            stored["Severity"]["Normalized"].as_i64(),
            Some(*expected_normalized),
            "case {case:?}: expected Normalized {expected_normalized}, got {:?}",
            stored["Severity"]["Normalized"]
        );
    }
}

/// RULE (documented on `StringFilter$Comparison`): `EQUALS`, `PREFIX` and `CONTAINS`
/// select findings that HAVE the value and are joined by `OR` on one field;
/// `NOT_EQUALS`, `PREFIX_NOT_EQUALS` and `NOT_CONTAINS` select findings that do NOT
/// and are joined by `AND`.
///
/// The table spans every comparison GetFindings supports, several fields (one of them
/// list-valued), and the OR/AND joins — not only the `Id EQUALS` case the report
/// cited.
#[tokio::test]
async fn string_filters_select_findings_by_the_documented_comparisons() {
    let mut alpha = base_finding("alpha-1");
    alpha["Title"] = json!("CloudFront distribution is public");
    alpha["Severity"] = json!({ "Label": "HIGH" });
    alpha["Types"] = json!(["Effects/Data Exposure"]);

    let mut alpha_two = base_finding("alpha-2");
    alpha_two["Title"] = json!("CloudWatch alarm is missing");
    alpha_two["Severity"] = json!({ "Label": "LOW" });
    alpha_two["RecordState"] = json!("ARCHIVED");
    alpha_two["Types"] = json!(["Software and Configuration Checks/Industry and Regulatory Standards"]);

    let mut beta = base_finding("beta-1");
    beta["Title"] = json!("S3 bucket allows public read");
    beta["Severity"] = json!({ "Label": "CRITICAL" });
    beta["Workflow"] = json!({ "Status": "SUPPRESSED" });
    beta["Types"] = json!(["Effects/Data Exposure", "TTPs/Discovery"]);

    let harness = Harness::with_findings(vec![alpha, alpha_two, beta]).await;

    // (case, Filters, expected ids, sorted)
    let cases: [(&str, Value, Vec<&str>); 10] = [
        (
            "Id EQUALS",
            json!({ "Id": [{ "Value": "alpha-1", "Comparison": "EQUALS" }] }),
            vec!["alpha-1"],
        ),
        (
            "Id PREFIX",
            json!({ "Id": [{ "Value": "alpha", "Comparison": "PREFIX" }] }),
            vec!["alpha-1", "alpha-2"],
        ),
        (
            "Title CONTAINS",
            json!({ "Title": [{ "Value": "Cloud", "Comparison": "CONTAINS" }] }),
            vec!["alpha-1", "alpha-2"],
        ),
        (
            "SeverityLabel EQUALS joined by OR",
            json!({ "SeverityLabel": [
                { "Value": "HIGH", "Comparison": "EQUALS" },
                { "Value": "CRITICAL", "Comparison": "EQUALS" },
            ] }),
            vec!["alpha-1", "beta-1"],
        ),
        (
            "Id NOT_EQUALS joined by AND",
            json!({ "Id": [
                { "Value": "alpha-1", "Comparison": "NOT_EQUALS" },
                { "Value": "alpha-2", "Comparison": "NOT_EQUALS" },
            ] }),
            vec!["beta-1"],
        ),
        (
            "Id PREFIX_NOT_EQUALS",
            json!({ "Id": [{ "Value": "alpha", "Comparison": "PREFIX_NOT_EQUALS" }] }),
            vec!["beta-1"],
        ),
        (
            "Title NOT_CONTAINS",
            json!({ "Title": [{ "Value": "Cloud", "Comparison": "NOT_CONTAINS" }] }),
            vec!["beta-1"],
        ),
        (
            "PREFIX combined with NOT_EQUALS",
            json!({ "Id": [
                { "Value": "alpha", "Comparison": "PREFIX" },
                { "Value": "alpha-2", "Comparison": "NOT_EQUALS" },
            ] }),
            vec!["alpha-1"],
        ),
        (
            "Type is list-valued",
            json!({ "Type": [{ "Value": "Effects/Data Exposure", "Comparison": "EQUALS" }] }),
            vec!["alpha-1", "beta-1"],
        ),
        (
            "WorkflowStatus and RecordState together",
            json!({
                "WorkflowStatus": [{ "Value": "NEW", "Comparison": "EQUALS" }],
                "RecordState": [{ "Value": "ACTIVE", "Comparison": "EQUALS" }],
            }),
            vec!["alpha-1"],
        ),
    ];

    for (case, filters, expected) in cases {
        let (status, body) = harness.get_findings(json!({ "Filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let mut ids = returned_ids(&body);
        ids.sort();
        let mut expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        expected.sort();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

/// RULE: a `DateFilter` selects findings whose timestamp attribute falls inside the
/// window it names — between `Start` and `End` inclusive, or `WITHIN` / `OLDER_THAN`
/// a `DateRange` measured back from now ("If you don't specify a value, the default
/// is `WITHIN`" — `DateRange$Comparison`).
#[tokio::test]
async fn date_filters_select_findings_inside_the_window_they_name() {
    // Two findings a known distance apart, so an absolute window and a relative one
    // can each pick out exactly one of them.
    let mut old = base_finding("date-old");
    old["CreatedAt"] = json!("2020-01-01T00:00:00Z");
    old["UpdatedAt"] = json!("2020-01-02T00:00:00Z");

    // The recent finding's instants are relative to NOW, because the `DateRange` row
    // below is: "OLDER_THAN one day" is measured back from the moment the request runs,
    // so a hard-coded "recent" timestamp stops being recent once the wall clock passes
    // it and the row starts selecting both findings.
    let mut recent = base_finding("date-recent");
    recent["CreatedAt"] = json!(iso8601_before_now(2 * 60 * 60));
    recent["UpdatedAt"] = json!(iso8601_before_now(60 * 60));

    let harness = Harness::with_findings(vec![old, recent]).await;

    // (case, Filters, expected ids)
    let cases: [(&str, Value, Vec<&str>); 5] = [
        (
            "Start and End bracket the old finding",
            json!({ "CreatedAt": [{ "Start": "2019-01-01T00:00:00Z", "End": "2021-01-01T00:00:00Z" }] }),
            vec!["date-old"],
        ),
        (
            "Start alone excludes the old finding",
            json!({ "CreatedAt": [{ "Start": "2025-01-01T00:00:00Z" }] }),
            vec!["date-recent"],
        ),
        (
            "End alone excludes the recent finding",
            json!({ "CreatedAt": [{ "End": "2021-01-01T00:00:00Z" }] }),
            vec!["date-old"],
        ),
        (
            "DateRange OLDER_THAN one day keeps only the old finding",
            json!({ "UpdatedAt": [{ "DateRange": { "Value": 1, "Unit": "DAYS", "Comparison": "OLDER_THAN" } }] }),
            vec!["date-old"],
        ),
        (
            "two windows on one field are joined by OR",
            json!({ "CreatedAt": [
                { "End": "2021-01-01T00:00:00Z" },
                { "Start": "2025-01-01T00:00:00Z" },
            ] }),
            vec!["date-old", "date-recent"],
        ),
    ];

    for (case, filters, expected) in cases {
        let (status, body) = harness.get_findings(json!({ "Filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let mut ids = returned_ids(&body);
        ids.sort();
        let mut expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        expected.sort();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

/// RULE: the filter combinations `StringFilter$Comparison` calls out as errors are
/// refused with `InvalidInputException` (400), and `CONTAINS_WORD` — which "works
/// only in the GetFindingsV2, GetFindingStatisticsV2, GetResourcesV2, and
/// GetResourcesStatisticsV2 APIs" — is refused here too.
///
/// Whether a request is valid cannot depend on what is stored, so every row runs
/// against a hub that holds a finding AND against one that holds none.
#[tokio::test]
async fn illegal_filter_combinations_are_refused() {
    let populated = Harness::with_findings(vec![base_finding("finding-1")]).await;
    let empty = Harness::new();
    let (status, _) = empty.enable(None).await;
    assert_eq!(status, 200, "enable on the empty hub: got {status}");

    let cases: [(&str, Value); 13] = [
        (
            "CONTAINS with NOT_CONTAINS on one field",
            json!({ "Title": [
                { "Value": "a", "Comparison": "CONTAINS" },
                { "Value": "b", "Comparison": "NOT_CONTAINS" },
            ] }),
        ),
        (
            "CONTAINS with EQUALS on one field",
            json!({ "Title": [
                { "Value": "a", "Comparison": "CONTAINS" },
                { "Value": "b", "Comparison": "EQUALS" },
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
            "DateFilter.Start is not a timestamp",
            json!({ "UpdatedAt": [{ "Start": "yesterday", "End": "2026-10-22T00:00:00Z" }] }),
        ),
        (
            "DateRange.Unit is not a modelled unit",
            json!({ "UpdatedAt": [{ "DateRange": { "Value": 7, "Unit": "WEEKS" } }] }),
        ),
        // A filter that names no predicate must be refused, not ignored: a filtered
        // request may never answer an unfiltered page.
        (
            "StringFilter without a Value",
            json!({ "Id": [{ "Comparison": "EQUALS" }] }),
        ),
        (
            "NumberFilter without a bound",
            json!({ "Confidence": [{}] }),
        ),
        ("BooleanFilter without a Value", json!({ "Sample": [{}] })),
        (
            "DateFilter without Start, End or DateRange",
            json!({ "CreatedAt": [{}] }),
        ),
        // A comparison GetFindings does not evaluate, and a DateRange that names no
        // window, are refused rather than dropped.
        (
            "an unevaluated StringFilterComparison",
            json!({ "Id": [{ "Value": "finding-1", "Comparison": "STARTS_WITH" }] }),
        ),
        (
            "DateRange without a Value",
            json!({ "UpdatedAt": [{ "DateRange": { "Unit": "DAYS" } }] }),
        ),
        (
            "DateRange with a negative Value",
            json!({ "UpdatedAt": [{ "DateRange": { "Value": -30, "Unit": "DAYS" } }] }),
        ),
    ];

    for (case, filters) in cases {
        for (hub, label) in [(&populated, "populated hub"), (&empty, "empty hub")] {
            let (status, body) = hub.get_findings(json!({ "Filters": filters.clone() })).await;
            assert_eq!(
                status, 400,
                "case {case:?} on the {label}: expected 400, got {status} with {body}"
            );
        }
    }
}

/// RULE: a filter member this slice does not evaluate is answered with 501, never
/// with findings nothing filtered.
#[tokio::test]
async fn an_unevaluated_filter_member_answers_501() {
    let harness = Harness::with_findings(vec![base_finding("finding-1")]).await;

    let cases: [(&str, Value); 3] = [
        (
            "ResourceId",
            json!({ "ResourceId": [{ "Value": "i-0", "Comparison": "EQUALS" }] }),
        ),
        (
            "UserDefinedFields",
            json!({ "UserDefinedFields": [{ "Key": "owner", "Value": "x", "Comparison": "EQUALS" }] }),
        ),
        ("Keyword", json!({ "Keyword": [{ "Value": "public" }] })),
    ];

    for (case, filters) in cases {
        let (status, body) = harness.get_findings(json!({ "Filters": filters })).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501, got {status} with {body}"
        );
    }
}

/// RULE: a page carries at most `MaxResults` findings and a `NextToken` that resumes
/// exactly where it stopped; the last page carries no `NextToken`, and no finding is
/// repeated or skipped across the pages.
///
/// The table spans page sizes that divide the collection, that do not, that equal it
/// and that exceed it.
#[tokio::test]
async fn paging_truncates_at_max_results_and_resumes_from_next_token() {
    let total = 7;
    let findings: Vec<Value> = (0..total)
        .map(|index| {
            let mut finding = base_finding(&format!("page-{index}"));
            // Distinct UpdatedAt values, so the default newest-first order is total.
            finding["UpdatedAt"] = json!(format!("2026-10-21T21:0{index}:00Z"));
            finding
        })
        .collect();
    let harness = Harness::with_findings(findings).await;

    for page_size in [1usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let (status, body) = harness.get_findings(request).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected 200, got {status} with {body}"
            );
            let page = returned_ids(&body);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page must hold at most {page_size}, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= total + 1,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: no finding may repeat across pages, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            total,
            "page size {page_size}: every finding must appear exactly once, got {seen:?}"
        );
        // The default order is newest UpdatedAt first.
        let expected: Vec<String> = (0..total)
            .rev()
            .map(|index| format!("page-{index}"))
            .collect();
        assert_eq!(
            seen, expected,
            "page size {page_size}: expected newest-first order {expected:?}, got {seen:?}"
        );
    }

    // A token this operation did not issue is a client error, not an empty page.
    for bogus in ["not-a-token", "6e6f7065"] {
        let (status, body) = harness
            .get_findings(json!({ "NextToken": bogus }))
            .await;
        assert_eq!(
            status, 400,
            "token {bogus:?}: expected 400, got {status} with {body}"
        );
    }
}

/// RULE: `SortCriteria` orders the page by the named finding attribute in the named
/// direction.
#[tokio::test]
async fn sort_criteria_orders_the_page() {
    let mut first = base_finding("sort-a");
    first["Severity"] = json!({ "Label": "LOW" });
    first["Confidence"] = json!(10);
    first["UpdatedAt"] = json!("2026-10-21T21:00:00Z");

    let mut second = base_finding("sort-b");
    second["Severity"] = json!({ "Label": "CRITICAL" });
    second["Confidence"] = json!(90);
    second["UpdatedAt"] = json!("2026-10-21T22:00:00Z");

    let mut third = base_finding("sort-c");
    third["Severity"] = json!({ "Label": "MEDIUM" });
    third["Confidence"] = json!(50);
    third["UpdatedAt"] = json!("2026-10-21T20:00:00Z");

    let harness = Harness::with_findings(vec![first, second, third]).await;

    // (case, SortCriteria, expected order)
    let cases: [(&str, Value, Vec<&str>); 5] = [
        (
            "Id asc",
            json!([{ "Field": "Id", "SortOrder": "asc" }]),
            vec!["sort-a", "sort-b", "sort-c"],
        ),
        (
            "Id desc",
            json!([{ "Field": "Id", "SortOrder": "desc" }]),
            vec!["sort-c", "sort-b", "sort-a"],
        ),
        (
            "UpdatedAt asc",
            json!([{ "Field": "UpdatedAt", "SortOrder": "asc" }]),
            vec!["sort-c", "sort-a", "sort-b"],
        ),
        (
            "Confidence desc",
            json!([{ "Field": "Confidence", "SortOrder": "desc" }]),
            vec!["sort-b", "sort-c", "sort-a"],
        ),
        (
            "SeverityNormalized asc",
            json!([{ "Field": "SeverityNormalized", "SortOrder": "asc" }]),
            vec!["sort-a", "sort-c", "sort-b"],
        ),
    ];

    for (case, criteria, expected) in cases {
        let (status, body) = harness
            .get_findings(json!({ "SortCriteria": criteria }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        assert_eq!(
            returned_ids(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // A criterion that names no field is a bad request; one that names a field this
    // slice cannot order by is "not written yet".
    let (status, body) = harness
        .get_findings(json!({ "SortCriteria": [{ "SortOrder": "asc" }] }))
        .await;
    assert_eq!(
        status, 400,
        "a SortCriterion without a Field: expected 400, got {status} with {body}"
    );
    let (status, body) = harness
        .get_findings(json!({ "SortCriteria": [{ "Field": "ResourceId", "SortOrder": "asc" }] }))
        .await;
    assert_eq!(
        status, 501,
        "an unimplemented sort field: expected 501, got {status} with {body}"
    );
}

/// RULE: every attribute `GetFindings` advertises as sortable actually orders a page
/// by that attribute's value, ascending or descending as asked.
///
/// The two seeded findings are built so that the FIRST carries the lower value for
/// every sortable attribute a client can set, so `asc` must be `[a, b]` and `desc`
/// `[b, a]` for each one. Two attributes are service-populated and therefore equal on
/// both findings (`Region`, and `ProcessedAt`, which is one instant per import
/// batch); for those the rule is only that the order does not depend on the
/// direction. The sweep also pins the pairing between the advertised attribute names
/// and the accessors that read them: a name the accessor does not know would answer
/// 501 here.
#[tokio::test]
async fn every_sortable_attribute_orders_a_page_by_that_attribute() {
    use service_com_amazonaws_securityhub_securityhubapiservice::ops::get_findings::{
        DATE_ATTRIBUTES, NUMBER_ATTRIBUTES, STRING_ATTRIBUTES,
    };

    /// Attributes Security Hub CSPM populates itself, so both findings carry the
    /// same value and neither direction can separate them.
    const SERVICE_POPULATED: [&str; 2] = ["Region", "ProcessedAt"];

    // `a` holds the lower value for every attribute a client can set; `b` the higher.
    // `Severity.Label` and `Severity.Normalized` are both given explicitly, so the
    // label's alphabetical order and the normalized value's numeric order agree.
    let low = json!({
        "SchemaVersion": SCHEMA_VERSION,
        "Id": "sortable-a",
        "ProductArn": "arn:aws:securityhub:us-east-1::product/aws/aaa",
        "ProductName": "product-a",
        "CompanyName": "company-a",
        "GeneratorId": "generator-a",
        "AwsAccountId": "000000000000",
        "AwsAccountName": "account-a",
        "Types": ["Effects/Aaa"],
        "FirstObservedAt": "2026-10-01T00:00:00Z",
        "LastObservedAt": "2026-10-02T00:00:00Z",
        "CreatedAt": "2026-10-03T00:00:00Z",
        "UpdatedAt": "2026-10-04T00:00:00Z",
        "Severity": { "Label": "CRITICAL", "Normalized": 1, "Product": 1.0 },
        "Confidence": 1,
        "Criticality": 1,
        "Title": "Title aaa",
        "Description": "Description aaa",
        "Remediation": { "Recommendation": { "Text": "Recommendation aaa" } },
        "SourceUrl": "https://example.invalid/aaa",
        "Resources": [{ "Type": "AwsEc2Instance", "Id": "i-0aaa" }],
        "Compliance": { "Status": "FAILED", "SecurityControlId": "control-a" },
        "VerificationState": "BENIGN_POSITIVE",
        "WorkflowState": "ASSIGNED",
        "Workflow": { "Status": "NEW" },
        "RecordState": "ACTIVE",
        "Note": { "Text": "note aaa", "UpdatedBy": "user-a", "UpdatedAt": "2026-10-05T00:00:00Z" },
    });
    let high = json!({
        "SchemaVersion": SCHEMA_VERSION,
        "Id": "sortable-b",
        "ProductArn": "arn:aws:securityhub:us-east-1::product/aws/bbb",
        "ProductName": "product-b",
        "CompanyName": "company-b",
        "GeneratorId": "generator-b",
        "AwsAccountId": "000000000001",
        "AwsAccountName": "account-b",
        "Types": ["Effects/Bbb"],
        "FirstObservedAt": "2026-10-11T00:00:00Z",
        "LastObservedAt": "2026-10-12T00:00:00Z",
        "CreatedAt": "2026-10-13T00:00:00Z",
        "UpdatedAt": "2026-10-14T00:00:00Z",
        "Severity": { "Label": "HIGH", "Normalized": 2, "Product": 2.0 },
        "Confidence": 2,
        "Criticality": 2,
        "Title": "Title bbb",
        "Description": "Description bbb",
        "Remediation": { "Recommendation": { "Text": "Recommendation bbb" } },
        "SourceUrl": "https://example.invalid/bbb",
        "Resources": [{ "Type": "AwsEc2Instance", "Id": "i-0bbb" }],
        "Compliance": { "Status": "PASSED", "SecurityControlId": "control-b" },
        "VerificationState": "TRUE_POSITIVE",
        "WorkflowState": "NEW",
        "Workflow": { "Status": "NOTIFIED" },
        "RecordState": "ARCHIVED",
        "Note": { "Text": "note bbb", "UpdatedBy": "user-b", "UpdatedAt": "2026-10-15T00:00:00Z" },
    });

    let harness = Harness::with_findings(vec![low, high]).await;

    for field in STRING_ATTRIBUTES
        .iter()
        .chain(NUMBER_ATTRIBUTES.iter())
        .chain(DATE_ATTRIBUTES.iter())
    {
        let mut pages = Vec::new();
        for order in ["asc", "desc"] {
            let (status, body) = harness
                .get_findings(json!({ "SortCriteria": [{ "Field": field, "SortOrder": order }] }))
                .await;
            assert_eq!(
                status, 200,
                "field {field:?} {order}: expected 200, got {status} with {body}"
            );
            pages.push(returned_ids(&body));
        }
        let (ascending, descending) = (&pages[0], &pages[1]);

        if SERVICE_POPULATED.contains(field) {
            assert_eq!(
                ascending, descending,
                "field {field:?}: both findings carry the same service-populated value, so the order must not depend on the direction, got asc {ascending:?} and desc {descending:?}"
            );
            continue;
        }
        assert_eq!(
            ascending,
            &vec!["sortable-a".to_string(), "sortable-b".to_string()],
            "field {field:?} asc: expected the lower value first, got {ascending:?}"
        );
        assert_eq!(
            descending,
            &vec!["sortable-b".to_string(), "sortable-a".to_string()],
            "field {field:?} desc: expected the higher value first, got {descending:?}"
        );
    }
}

/// RULE: the published breadth and depth limits on `Filters` are enforced — "You can
/// filter by up to 10 finding attributes. For each attribute, you can provide up to
/// 20 filter values."
#[tokio::test]
async fn filters_beyond_the_published_limits_are_refused() {
    use service_com_amazonaws_securityhub_securityhubapiservice::ops::get_findings::STRING_ATTRIBUTES;

    let harness = Harness::with_findings(vec![base_finding("limits-1")]).await;

    // (case, number of attributes, values per attribute, expected status)
    let cases: [(&str, usize, usize, u16); 6] = [
        ("1 attribute, 1 value", 1, 1, 200),
        ("10 attributes (the maximum)", 10, 1, 200),
        ("11 attributes (one past it)", 11, 1, 400),
        ("20 values on one attribute (the maximum)", 1, 20, 200),
        ("21 values on one attribute (one past it)", 1, 21, 400),
        ("both limits exceeded", 11, 21, 400),
    ];

    for (case, attributes, values, expected) in cases {
        let mut filters = serde_json::Map::new();
        for attribute in STRING_ATTRIBUTES.iter().take(attributes) {
            let list: Vec<Value> = (0..values)
                .map(|index| json!({ "Value": format!("v{index}"), "Comparison": "PREFIX" }))
                .collect();
            filters.insert((*attribute).to_string(), Value::Array(list));
        }
        let (status, body) = harness
            .get_findings(json!({ "Filters": Value::Object(filters) }))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// BatchUpdateFindings
// ---------------------------------------------------------------------------

/// RULE: `BatchUpdateFindings` writes the members it carries onto the stored finding,
/// leaves the members it does not carry alone, and a later `GetFindings` reports the
/// new values.
///
/// The table spans each updatable member group.
#[tokio::test]
async fn batch_update_findings_mutates_the_stored_finding() {
    // (case, update members, assertions on the stored finding)
    let cases: [UpdateCase; 7] = [
        (
            "Workflow.Status",
            json!({ "Workflow": { "Status": "SUPPRESSED" } }),
            |f| {
                (f["Workflow"]["Status"].as_str() != Some("SUPPRESSED"))
                    .then(|| format!("Workflow.Status is {:?}", f["Workflow"]["Status"]))
            },
        ),
        (
            "Note",
            json!({ "Note": { "Text": "triaged", "UpdatedBy": "analyst" } }),
            |f| {
                if f["Note"]["Text"].as_str() != Some("triaged") {
                    return Some(format!("Note.Text is {:?}", f["Note"]["Text"]));
                }
                if f["Note"]["UpdatedBy"].as_str() != Some("analyst") {
                    return Some(format!("Note.UpdatedBy is {:?}", f["Note"]["UpdatedBy"]));
                }
                (!f["Note"]["UpdatedAt"].is_string())
                    .then(|| format!("Note.UpdatedAt is {:?}", f["Note"]["UpdatedAt"]))
            },
        ),
        (
            "Severity.Label completes Normalized",
            json!({ "Severity": { "Label": "INFORMATIONAL" } }),
            |f| {
                if f["Severity"]["Label"].as_str() != Some("INFORMATIONAL") {
                    return Some(format!("Severity.Label is {:?}", f["Severity"]["Label"]));
                }
                (f["Severity"]["Normalized"].as_i64() != Some(0))
                    .then(|| format!("Severity.Normalized is {:?}", f["Severity"]["Normalized"]))
            },
        ),
        (
            "Confidence and Criticality",
            json!({ "Confidence": 55, "Criticality": 77 }),
            |f| {
                if f["Confidence"].as_i64() != Some(55) {
                    return Some(format!("Confidence is {:?}", f["Confidence"]));
                }
                (f["Criticality"].as_i64() != Some(77))
                    .then(|| format!("Criticality is {:?}", f["Criticality"]))
            },
        ),
        (
            "VerificationState",
            json!({ "VerificationState": "TRUE_POSITIVE" }),
            |f| {
                (f["VerificationState"].as_str() != Some("TRUE_POSITIVE"))
                    .then(|| format!("VerificationState is {:?}", f["VerificationState"]))
            },
        ),
        (
            "Types",
            json!({ "Types": ["TTPs/Discovery"] }),
            |f| {
                (f["Types"] != json!(["TTPs/Discovery"]))
                    .then(|| format!("Types is {:?}", f["Types"]))
            },
        ),
        (
            "UserDefinedFields",
            json!({ "UserDefinedFields": { "ticket": "SEC-9" } }),
            |f| {
                (f["UserDefinedFields"]["ticket"].as_str() != Some("SEC-9"))
                    .then(|| format!("UserDefinedFields is {:?}", f["UserDefinedFields"]))
            },
        ),
    ];

    for (case, update, check) in cases {
        let harness = Harness::with_findings(vec![base_finding("target")]).await;
        let (_, before) = harness.get_findings(json!({})).await;
        let before = finding_with_id(&before, "target");

        let mut request = update.clone();
        request["FindingIdentifiers"] = json!([{ "Id": "target", "ProductArn": PRODUCT_ARN }]);
        let (status, body) = harness.update_findings(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["ProcessedFindings"],
            json!([{ "Id": "target", "ProductArn": PRODUCT_ARN }]),
            "case {case:?}: expected the identifier in ProcessedFindings, got {body}"
        );
        assert_eq!(
            body["UnprocessedFindings"],
            json!([]),
            "case {case:?}: expected no UnprocessedFindings, got {body}"
        );

        let (_, after) = harness.get_findings(json!({})).await;
        let after = finding_with_id(&after, "target");
        if let Some(problem) = check(&after) {
            panic!("case {case:?}: the update did not persist: {problem}; stored {after}");
        }
        // Members the update did not carry are untouched.
        for member in ["Id", "ProductArn", "Title", "Description", "CreatedAt"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: member {member} must be unchanged, got {:?} after {:?}",
                after[member], before[member]
            );
        }
    }
}

/// RULE: an identifier `BatchUpdateFindings` cannot apply is reported in
/// `UnprocessedFindings` with an `ErrorCode` and is NOT reported as processed; the
/// identifiers it can apply are still applied.
#[tokio::test]
async fn batch_update_findings_reports_identifiers_it_cannot_apply() {
    let harness = Harness::with_findings(vec![base_finding("present")]).await;

    let (status, body) = harness
        .update_findings(json!({
            "FindingIdentifiers": [
                { "Id": "present", "ProductArn": PRODUCT_ARN },
                { "Id": "absent", "ProductArn": PRODUCT_ARN },
                { "Id": "present", "ProductArn": "arn:aws:securityhub:us-east-1::product/other/tool" },
                { "Id": "present", "ProductArn": PRODUCT_ARN },
            ],
            "Workflow": { "Status": "RESOLVED" },
        }))
        .await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    assert_eq!(
        body["ProcessedFindings"],
        json!([{ "Id": "present", "ProductArn": PRODUCT_ARN }]),
        "only the stored identifier may be processed, got {body}"
    );

    let unprocessed = body["UnprocessedFindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    // (case, the identifier's Id, the expected ErrorCode)
    let expected: [(&str, &str, &str); 3] = [
        ("unknown Id", "absent", "FindingNotFound"),
        ("unknown ProductArn", "present", "FindingNotFound"),
        ("repeated identifier", "present", "DuplicatedFindingIdentifier"),
    ];
    assert_eq!(
        unprocessed.len(),
        expected.len(),
        "expected {} UnprocessedFindings, got {body}",
        expected.len()
    );
    for (index, (case, id, error_code)) in expected.into_iter().enumerate() {
        let entry = &unprocessed[index];
        assert_eq!(
            entry["FindingIdentifier"]["Id"].as_str(),
            Some(id),
            "case {case:?}: expected identifier {id}, got {:?}",
            entry["FindingIdentifier"]
        );
        assert_eq!(
            entry["ErrorCode"].as_str(),
            Some(error_code),
            "case {case:?}: expected ErrorCode {error_code}, got {:?}",
            entry["ErrorCode"]
        );
        assert!(
            entry["ErrorMessage"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: expected an ErrorMessage, got {:?}",
            entry["ErrorMessage"]
        );
    }

    // The one identifier that could be applied was applied exactly once.
    let (_, found) = harness.get_findings(json!({})).await;
    let stored = finding_with_id(&found, "present");
    assert_eq!(
        stored["Workflow"]["Status"].as_str(),
        Some("RESOLVED"),
        "the applicable identifier must still be updated, got {found}"
    );
}
