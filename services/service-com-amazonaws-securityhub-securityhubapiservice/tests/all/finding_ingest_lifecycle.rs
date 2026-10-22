// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what Security Hub CSPM itself owns on the finding
//! ingest path: the lifecycle members it sets on every finding it stores, the Amazon
//! Web Services Security Finding Format rule that a finding must populate `Severity`
//! or `FindingProviderFields`, the members a provider re-import may not update, and the
//! one workflow-status reset the service does make on a re-import.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! member that is wrong on the wire fails here. The emulator harness and the finding
//! builder are the ones `security_hub_core_loop` already defines.

use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::{
    COMPLIANCE_STATUS_VALUES, RECORD_STATE_VALUES, WORKFLOW_STATUS_VALUES,
};

use crate::security_hub_core_loop::{base_finding, finding_with_id, returned_ids, Harness};

/// One row of the lifecycle-defaults table: the case name, how the provider's payload
/// differs, and the `RecordState` / `Workflow.Status` the stored finding must carry.
type DefaultsCase = (String, Value, String, String);

/// One row of the format table: the case name, the provider's payload, and whether the
/// Amazon Web Services Security Finding Format accepts it.
type FormatCase = (&'static str, Value, bool);

/// One row of the re-import table: the case name, the members the customer sets through
/// `BatchUpdateFindings`, the members the provider's re-import carries, and a check that
/// returns what is wrong with the stored finding, or `None`.
type ReimportCase = (&'static str, Value, Value, fn(&Value) -> Option<String>);

/// One row of the reactivation table: the case name, the members the first import
/// carries, the status the customer then sets, the members the re-import carries, and
/// the `Workflow.Status` the stored finding must end up with.
type ReactivationCase = (&'static str, Value, &'static str, Value, &'static str);

/// Apply a payload of members to `document`: a member whose value is `null` is
/// REMOVED (that is how a row says "the submitter sent no such member"), every other
/// member is inserted as given.
fn apply_members(mut document: Value, payload: &Value) -> Value {
    if let (Some(target), Some(source)) = (document.as_object_mut(), payload.as_object()) {
        for (member, value) in source {
            if value.is_null() {
                target.remove(member);
            } else {
                target.insert(member.clone(), value.clone());
            }
        }
    }
    document
}

/// `GetFindings` filtered down to the one finding with `id`.
async fn read_back(harness: &Harness, id: &str) -> Value {
    let (status, body) = harness
        .get_findings(json!({
            "Filters": { "Id": [{ "Value": id, "Comparison": "EQUALS" }] }
        }))
        .await;
    assert_eq!(status, 200, "case {id:?}: GetFindings got {status}: {body}");
    finding_with_id(&body, id)
}

// ---------------------------------------------------------------------------
// Cause 1: the lifecycle members the service sets on ingest
// ---------------------------------------------------------------------------

/// RULE: a finding Security Hub CSPM has ingested always carries the lifecycle members
/// the service owns — `RecordState` and `Workflow.Status` — so one whose provider
/// submitted neither reads back with their initial values (`ACTIVE`, `NEW`), and one
/// that submitted either keeps the submitted value.
///
/// The table spans both directions of the defaulting over every published value of
/// each enum, plus the forms a payload can take: both members absent, only one absent,
/// a `Workflow` object with no `Status`, and a finding that conforms through
/// `FindingProviderFields` instead of `Severity`.
#[tokio::test]
async fn ingested_findings_carry_the_lifecycle_members_the_service_sets() {
    let mut cases: Vec<DefaultsCase> = vec![
        (
            "neither RecordState nor Workflow submitted".to_string(),
            json!({ "RecordState": null, "Workflow": null }),
            "ACTIVE".to_string(),
            "NEW".to_string(),
        ),
        (
            "a Workflow object that carries no Status".to_string(),
            json!({ "RecordState": null, "Workflow": {} }),
            "ACTIVE".to_string(),
            "NEW".to_string(),
        ),
        (
            "a finding that conforms through FindingProviderFields".to_string(),
            json!({
                "RecordState": null,
                "Workflow": null,
                "Severity": null,
                "FindingProviderFields": { "Severity": { "Label": "LOW" } },
            }),
            "ACTIVE".to_string(),
            "NEW".to_string(),
        ),
    ];
    // Each published RecordState submitted, with Workflow left to the service. The
    // catalog is the crate's generated wire-value list for the enum (see
    // `src/ops/types/shared_types.rs`), so this table spans the enum as the model
    // defines it rather than a hand-copied snapshot of it.
    for state in RECORD_STATE_VALUES {
        cases.push((
            format!("RecordState {state} submitted, Workflow omitted"),
            json!({ "RecordState": state, "Workflow": null }),
            state.to_string(),
            "NEW".to_string(),
        ));
    }
    // Each published Workflow.Status submitted, with RecordState left to the service.
    for status in WORKFLOW_STATUS_VALUES {
        cases.push((
            format!("Workflow.Status {status} submitted, RecordState omitted"),
            json!({ "RecordState": null, "Workflow": { "Status": status } }),
            "ACTIVE".to_string(),
            status.to_string(),
        ));
    }

    let harness = Harness::new();
    let (status, _) = harness.enable(None).await;
    assert_eq!(status, 200, "EnableSecurityHub got {status}");

    for (index, (case, payload, expected_state, expected_status)) in cases.iter().enumerate() {
        let id = format!("lifecycle-{index}");
        let finding = apply_members(base_finding(&id), payload);

        let (status, body) = harness.import(vec![finding]).await;
        assert_eq!(status, 200, "case {case:?}: import got {status}: {body}");
        assert_eq!(
            (body["SuccessCount"].as_u64(), body["FailedCount"].as_u64()),
            (Some(1), Some(0)),
            "case {case:?}: expected the finding to import, got {body}"
        );

        let stored = read_back(&harness, &id).await;
        assert_eq!(
            stored["RecordState"].as_str(),
            Some(expected_state.as_str()),
            "case {case:?}: expected RecordState {expected_state}, got {:?}",
            stored["RecordState"]
        );
        assert_eq!(
            stored["Workflow"]["Status"].as_str(),
            Some(expected_status.as_str()),
            "case {case:?}: expected Workflow.Status {expected_status}, got {:?}",
            stored["Workflow"]
        );
    }
}

/// RULE: "When a new finding is sent to Security Hub CSPM by a finding provider,
/// Security Hub CSPM populates the `FindingProviderFields` object automatically, if it
/// is empty, based on the corresponding top-level fields"
/// (`smithy_query(query="shape", shape="FindingProviderFields")`) — and leaves an
/// object the provider did supply exactly as submitted.
///
/// The table spans the members `FindingProviderFields` mirrors (`Confidence`,
/// `Criticality`, `RelatedFindings`, `Severity`, `Types`), one row per source of the
/// value, and the row where the provider supplied the object itself.
#[tokio::test]
async fn finding_provider_fields_are_derived_from_the_top_level_fields_when_absent() {
    // (case, members added to the payload, what FindingProviderFields must then hold)
    let cases: [(&str, Value, Value); 5] = [
        (
            "Severity with Label only",
            json!({ "Severity": { "Label": "MEDIUM" } }),
            json!({ "Severity": { "Label": "MEDIUM" } }),
        ),
        (
            "Severity with Label and Original",
            json!({ "Severity": { "Label": "CRITICAL", "Original": "SEV0" } }),
            json!({ "Severity": { "Label": "CRITICAL", "Original": "SEV0" } }),
        ),
        (
            "Confidence and Criticality",
            json!({ "Confidence": 42, "Criticality": 99 }),
            json!({ "Confidence": 42, "Criticality": 99 }),
        ),
        (
            "RelatedFindings",
            json!({ "RelatedFindings": [
                { "ProductArn": "arn:aws:securityhub:us-east-1::product/aws/securityhub", "Id": "related-1" }
            ] }),
            json!({ "RelatedFindings": [
                { "ProductArn": "arn:aws:securityhub:us-east-1::product/aws/securityhub", "Id": "related-1" }
            ] }),
        ),
        (
            "an object the provider supplied itself",
            json!({
                "Severity": { "Label": "CRITICAL" },
                "FindingProviderFields": { "Severity": { "Label": "LOW" }, "Confidence": 7 },
            }),
            json!({ "Severity": { "Label": "LOW" }, "Confidence": 7 }),
        ),
    ];

    let harness = Harness::new();
    let (status, _) = harness.enable(None).await;
    assert_eq!(status, 200, "EnableSecurityHub got {status}");

    for (index, (case, payload, expected)) in cases.into_iter().enumerate() {
        let id = format!("provider-fields-{index}");
        let finding = apply_members(base_finding(&id), &payload);
        let (status, body) = harness.import(vec![finding]).await;
        assert_eq!(status, 200, "case {case:?}: import got {status}: {body}");
        assert_eq!(
            body["SuccessCount"].as_u64(),
            Some(1),
            "case {case:?}: expected the finding to import, got {body}"
        );

        let stored = read_back(&harness, &id).await;
        for (member, value) in expected.as_object().expect("an object of members") {
            assert_eq!(
                &stored["FindingProviderFields"][member], value,
                "case {case:?}: expected FindingProviderFields.{member} {value}, got {:?}",
                stored["FindingProviderFields"][member]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cause 2: the Amazon Finding Format anyOf over Severity / FindingProviderFields
// ---------------------------------------------------------------------------

/// RULE: the Amazon Web Services Security Finding Format requires a finding to populate
/// `Severity` or `FindingProviderFields`; a finding that populates neither is reported
/// in `FailedFindings` with `ErrorCode` `InvalidInput` and a message naming the missing
/// properties, is not stored, and is not counted in `SuccessCount`, while the rest of
/// the batch still lands.
///
/// The table spans the ways a payload can satisfy or break that `anyOf`: both branches
/// populated, each branch alone (including a `FindingProviderFields` object that
/// carries no `Severity` of its own), and neither.
#[tokio::test]
async fn a_finding_must_populate_severity_or_finding_provider_fields() {
    let cases: [FormatCase; 5] = [
        (
            "neither Severity nor FindingProviderFields",
            json!({ "Severity": null }),
            false,
        ),
        ("Severity alone", json!({}), true),
        (
            "FindingProviderFields alone, carrying a Severity",
            json!({ "Severity": null, "FindingProviderFields": { "Severity": { "Label": "HIGH" } } }),
            true,
        ),
        (
            "FindingProviderFields alone, carrying only Types",
            json!({
                "Severity": null,
                "FindingProviderFields": { "Types": ["Effects/Data Exposure"] },
            }),
            true,
        ),
        (
            "both branches populated",
            json!({ "FindingProviderFields": { "Severity": { "Label": "LOW" } } }),
            true,
        ),
    ];

    for (index, (case, payload, accepted)) in cases.into_iter().enumerate() {
        let harness = Harness::new();
        let (status, _) = harness.enable(None).await;
        assert_eq!(status, 200, "case {case:?}: enable got {status}");

        let id = format!("format-{index}");
        let candidate = apply_members(base_finding(&id), &payload);
        let sibling_id = format!("sibling-{index}");
        let (status, body) = harness
            .import(vec![candidate, base_finding(&sibling_id)])
            .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");

        let expected_counts = if accepted {
            (Some(2), Some(0))
        } else {
            (Some(1), Some(1))
        };
        assert_eq!(
            (body["SuccessCount"].as_u64(), body["FailedCount"].as_u64()),
            expected_counts,
            "case {case:?}: expected (SuccessCount, FailedCount) {expected_counts:?}, got {body}"
        );

        let failed = body["FailedFindings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut expected_ids = vec![sibling_id.clone()];
        if accepted {
            assert!(
                failed.is_empty(),
                "case {case:?}: expected no FailedFindings, got {body}"
            );
            expected_ids.push(id.clone());
        } else {
            assert_eq!(
                failed.len(),
                1,
                "case {case:?}: expected one FailedFindings entry, got {body}"
            );
            assert_eq!(
                failed[0]["Id"].as_str(),
                Some(id.as_str()),
                "case {case:?}: the failure must name the submitted Id, got {:?}",
                failed[0]["Id"]
            );
            assert_eq!(
                failed[0]["ErrorCode"].as_str(),
                Some("InvalidInput"),
                "case {case:?}: expected ErrorCode InvalidInput, got {:?}",
                failed[0]["ErrorCode"]
            );
            let message = failed[0]["ErrorMessage"].as_str().unwrap_or_default();
            for member in ["Severity", "FindingProviderFields"] {
                assert!(
                    message.contains(member),
                    "case {case:?}: expected ErrorMessage to name the missing property {member}, \
                     got {message:?}"
                );
            }
        }

        // Only what was accepted is retrievable.
        let (_, found) = harness.get_findings(json!({})).await;
        let mut ids = returned_ids(&found);
        ids.sort();
        expected_ids.sort();
        assert_eq!(
            ids, expected_ids,
            "case {case:?}: expected the stored ids {expected_ids:?}, got {found}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 3: a re-import updates provider-owned members only
// ---------------------------------------------------------------------------

/// RULE: once a finding exists, "BatchImportFindings cannot be used to update the
/// following finding fields and objects, which Security Hub CSPM customers use to manage
/// their investigation workflow. Note UserDefinedFields VerificationState Workflow"
/// (this operation's own modelled documentation) — a re-import of the same
/// `ProductArn` + `Id` updates provider-owned members and leaves those four at the values
/// the customer's `BatchUpdateFindings` left them, whether the provider's payload omits
/// them or carries a different value.
///
/// The table spans all four customer-managed members, each with the provider's re-import
/// both omitting the member and sending a conflicting value for it, plus a control row
/// showing a provider-owned member (`Title`) does update.
#[tokio::test]
async fn a_reimport_cannot_update_the_customer_managed_members() {
    let cases: [ReimportCase; 9] = [
        (
            "Workflow.Status NOTIFIED, re-import sends NEW",
            json!({ "Workflow": { "Status": "NOTIFIED" } }),
            json!({ "Workflow": { "Status": "NEW" } }),
            |f| {
                (f["Workflow"]["Status"].as_str() != Some("NOTIFIED"))
                    .then(|| format!("Workflow.Status is {:?}", f["Workflow"]["Status"]))
            },
        ),
        (
            "Workflow.Status SUPPRESSED, re-import omits Workflow",
            json!({ "Workflow": { "Status": "SUPPRESSED" } }),
            json!({ "Workflow": null }),
            |f| {
                (f["Workflow"]["Status"].as_str() != Some("SUPPRESSED"))
                    .then(|| format!("Workflow.Status is {:?}", f["Workflow"]["Status"]))
            },
        ),
        (
            "Note set by the customer, re-import sends its own Note",
            json!({ "Note": { "Text": "triaged by the owner", "UpdatedBy": "analyst" } }),
            json!({ "Note": { "Text": "provider note", "UpdatedBy": "provider", "UpdatedAt": "2026-10-21T23:00:00Z" } }),
            |f| {
                (f["Note"]["Text"].as_str() != Some("triaged by the owner"))
                    .then(|| format!("Note.Text is {:?}", f["Note"]["Text"]))
            },
        ),
        (
            "Note set by the customer, re-import omits Note",
            json!({ "Note": { "Text": "triaged by the owner", "UpdatedBy": "analyst" } }),
            json!({}),
            |f| {
                if f["Note"]["Text"].as_str() != Some("triaged by the owner") {
                    return Some(format!("Note.Text is {:?}", f["Note"]["Text"]));
                }
                (f["Note"]["UpdatedBy"].as_str() != Some("analyst"))
                    .then(|| format!("Note.UpdatedBy is {:?}", f["Note"]["UpdatedBy"]))
            },
        ),
        (
            "UserDefinedFields set by the customer, re-import sends its own map",
            json!({ "UserDefinedFields": { "owner": "team-sec" } }),
            json!({ "UserDefinedFields": { "owner": "provider" } }),
            |f| {
                (f["UserDefinedFields"]["owner"].as_str() != Some("team-sec")).then(|| {
                    format!(
                        "UserDefinedFields is {:?}",
                        f["UserDefinedFields"]
                    )
                })
            },
        ),
        (
            "UserDefinedFields set by the customer, re-import omits the map",
            json!({ "UserDefinedFields": { "owner": "team-sec" } }),
            json!({}),
            |f| {
                (f["UserDefinedFields"]["owner"].as_str() != Some("team-sec")).then(|| {
                    format!(
                        "UserDefinedFields is {:?}",
                        f["UserDefinedFields"]
                    )
                })
            },
        ),
        (
            "VerificationState set by the customer, re-import sends its own",
            json!({ "VerificationState": "TRUE_POSITIVE" }),
            json!({ "VerificationState": "BENIGN_POSITIVE" }),
            |f| {
                (f["VerificationState"].as_str() != Some("TRUE_POSITIVE"))
                    .then(|| format!("VerificationState is {:?}", f["VerificationState"]))
            },
        ),
        (
            "VerificationState set by the customer, re-import omits it",
            json!({ "VerificationState": "TRUE_POSITIVE" }),
            json!({}),
            |f| {
                (f["VerificationState"].as_str() != Some("TRUE_POSITIVE"))
                    .then(|| format!("VerificationState is {:?}", f["VerificationState"]))
            },
        ),
        // Control: a provider-owned member the re-import DOES update.
        (
            "Title is provider-owned and the re-import replaces it",
            json!({ "Workflow": { "Status": "RESOLVED" } }),
            json!({ "Title": "Retitled by the provider" }),
            |f| {
                if f["Workflow"]["Status"].as_str() != Some("RESOLVED") {
                    return Some(format!("Workflow.Status is {:?}", f["Workflow"]["Status"]));
                }
                None
            },
        ),
    ];

    for (index, (case, update, reimport, check)) in cases.into_iter().enumerate() {
        let id = format!("reimport-{index}");
        let mut v1 = base_finding(&id);
        v1["Description"] = json!("v1");
        let product_arn = v1["ProductArn"].as_str().expect("a ProductArn").to_string();
        let harness = Harness::with_findings(vec![v1.clone()]).await;

        // The customer takes the finding through its investigation workflow.
        let (status, body) = harness
            .update_findings(apply_members(
                json!({ "FindingIdentifiers": [{ "Id": id, "ProductArn": product_arn }] }),
                &update,
            ))
            .await;
        assert_eq!(status, 200, "case {case:?}: update got {status}: {body}");
        assert_eq!(
            body["ProcessedFindings"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the update must be applied, got {body}"
        );

        let stored = read_back(&harness, &id).await;
        assert!(
            check(&stored).is_none(),
            "case {case:?}: the customer's update must be observable before the re-import: {:?} in {stored}",
            check(&stored)
        );

        // The provider re-imports the same identifier with a new Description and a new
        // Title, both provider-owned, so every row proves a provider update lands and
        // not just that the customer's members survived.
        let mut v2 = base_finding(&id);
        v2["Description"] = json!("v2");
        v2["Title"] = json!(format!("Retitled by the provider in case {index}"));
        let v2 = apply_members(v2, &reimport);
        let expected_title = v2["Title"].as_str().expect("a Title").to_string();
        let (status, body) = harness.import(vec![v2]).await;
        assert_eq!(status, 200, "case {case:?}: re-import got {status}: {body}");
        assert_eq!(
            (body["SuccessCount"].as_u64(), body["FailedCount"].as_u64()),
            (Some(1), Some(0)),
            "case {case:?}: the re-import must succeed, got {body}"
        );

        let stored = read_back(&harness, &id).await;
        assert_eq!(
            stored["Description"].as_str(),
            Some("v2"),
            "case {case:?}: the provider-owned Description must be updated, got {:?}",
            stored["Description"]
        );
        assert_eq!(
            stored["Title"].as_str(),
            Some(expected_title.as_str()),
            "case {case:?}: the provider-owned Title must be what the re-import sent, got {:?}",
            stored["Title"]
        );
        assert!(
            check(&stored).is_none(),
            "case {case:?}: the customer-managed member must survive the re-import: {:?} in {stored}",
            check(&stored)
        );
    }
}

/// RULE: the one `Workflow.Status` change a re-import does make, because the service
/// and not the customer owns it — "Security Hub CSPM also resets the workflow status
/// from `NOTIFIED` or `RESOLVED` to `NEW` in the following cases: `RecordState` changes
/// from `ARCHIVED` to `ACTIVE`; `ComplianceStatus` changes from `PASSED` to either
/// `WARNING`, `FAILED`, or `NOT_AVAILABLE`"
/// (`smithy_query(query="shape", shape="Workflow")`, member `Status`).
///
/// The table spans both reactivation cases, every `ComplianceStatus` the second one
/// names, the reviewed statuses the doc lists against `SUPPRESSED` which it does not,
/// the rows where nothing was reactivated so the customer's status stands, and the rows
/// where the re-import carries no `RecordState` at all, so the stored state (and with it
/// the customer's status) is what survives. Every row also pins the `RecordState` the
/// stored finding ends up with.
#[tokio::test]
async fn a_reimport_that_reactivates_a_finding_resets_the_reviewed_workflow_status() {
    let cases: [ReactivationCase; 10] = [
        (
            "RecordState ARCHIVED -> ACTIVE with a NOTIFIED finding",
            json!({ "RecordState": "ARCHIVED" }),
            "NOTIFIED",
            json!({ "RecordState": "ACTIVE" }),
            "NEW",
        ),
        (
            "RecordState ARCHIVED -> ACTIVE with a RESOLVED finding",
            json!({ "RecordState": "ARCHIVED" }),
            "RESOLVED",
            json!({ "RecordState": "ACTIVE" }),
            "NEW",
        ),
        (
            "RecordState ARCHIVED -> ACTIVE with a SUPPRESSED finding, which is not reset",
            json!({ "RecordState": "ARCHIVED" }),
            "SUPPRESSED",
            json!({ "RecordState": "ACTIVE" }),
            "SUPPRESSED",
        ),
        (
            "RecordState stays ACTIVE, so nothing is reset",
            json!({ "RecordState": "ACTIVE" }),
            "NOTIFIED",
            json!({ "RecordState": "ACTIVE" }),
            "NOTIFIED",
        ),
        (
            "ComplianceStatus PASSED -> FAILED",
            json!({ "Compliance": { "Status": "PASSED" } }),
            "NOTIFIED",
            json!({ "Compliance": { "Status": "FAILED" } }),
            "NEW",
        ),
        (
            "ComplianceStatus PASSED -> WARNING",
            json!({ "Compliance": { "Status": "PASSED" } }),
            "RESOLVED",
            json!({ "Compliance": { "Status": "WARNING" } }),
            "NEW",
        ),
        (
            "ComplianceStatus PASSED -> NOT_AVAILABLE",
            json!({ "Compliance": { "Status": "PASSED" } }),
            "NOTIFIED",
            json!({ "Compliance": { "Status": "NOT_AVAILABLE" } }),
            "NEW",
        ),
        (
            "ComplianceStatus stays PASSED, so nothing is reset",
            json!({ "Compliance": { "Status": "PASSED" } }),
            "NOTIFIED",
            json!({ "Compliance": { "Status": "PASSED" } }),
            "NOTIFIED",
        ),
        // A re-import that carries no RecordState at all states nothing about the
        // record state, so the stored one stands: the finding is still ARCHIVED and
        // nothing was reactivated.
        (
            "an ARCHIVED finding re-imported with no RecordState stays ARCHIVED",
            json!({ "RecordState": "ARCHIVED" }),
            "NOTIFIED",
            json!({ "RecordState": null }),
            "NOTIFIED",
        ),
        (
            "an ARCHIVED finding re-imported with no RecordState and a RESOLVED status",
            json!({ "RecordState": "ARCHIVED" }),
            "RESOLVED",
            json!({ "RecordState": null }),
            "RESOLVED",
        ),
    ];

    // The doc names WARNING, FAILED and NOT_AVAILABLE, which is every value of the
    // generated `ComplianceStatus` catalog other than PASSED: each must appear as a
    // regression row, so the table cannot fall behind the enum.
    for status in COMPLIANCE_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| *status != "PASSED")
    {
        assert!(
            cases
                .iter()
                .any(|(_, _, _, reimport, _)| reimport["Compliance"]["Status"] == json!(status)),
            "the table must cover the ComplianceStatus {status} the doc names"
        );
    }

    for (index, (case, first, customer_status, reimport, expected)) in
        cases.into_iter().enumerate()
    {
        let id = format!("reactivate-{index}");
        let v1 = apply_members(base_finding(&id), &first);
        let product_arn = v1["ProductArn"].as_str().expect("a ProductArn").to_string();
        let harness = Harness::with_findings(vec![v1]).await;

        let (status, body) = harness
            .update_findings(json!({
                "FindingIdentifiers": [{ "Id": id, "ProductArn": product_arn }],
                "Workflow": { "Status": customer_status },
            }))
            .await;
        assert_eq!(status, 200, "case {case:?}: update got {status}: {body}");
        let stored = read_back(&harness, &id).await;
        assert_eq!(
            stored["Workflow"]["Status"].as_str(),
            Some(customer_status),
            "case {case:?}: the customer's status must be applied first, got {:?}",
            stored["Workflow"]
        );

        let (status, body) = harness
            .import(vec![apply_members(base_finding(&id), &reimport)])
            .await;
        assert_eq!(status, 200, "case {case:?}: re-import got {status}: {body}");
        assert_eq!(
            body["SuccessCount"].as_u64(),
            Some(1),
            "case {case:?}: the re-import must succeed, got {body}"
        );

        let stored = read_back(&harness, &id).await;
        assert_eq!(
            stored["Workflow"]["Status"].as_str(),
            Some(expected),
            "case {case:?}: expected Workflow.Status {expected}, got {:?}",
            stored["Workflow"]
        );
        // `RecordState` is service-owned: the re-import's value when it carries one,
        // else the state the record already had, else the ingest default.
        let expected_state = reimport["RecordState"]
            .as_str()
            .or_else(|| first["RecordState"].as_str())
            .unwrap_or("ACTIVE");
        assert_eq!(
            stored["RecordState"].as_str(),
            Some(expected_state),
            "case {case:?}: expected RecordState {expected_state}, got {:?}",
            stored["RecordState"]
        );
    }
}
