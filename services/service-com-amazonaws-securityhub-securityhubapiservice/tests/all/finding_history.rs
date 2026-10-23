// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `GetFindingHistory`: the change events the two
//! finding writers — `BatchImportFindings` and `BatchUpdateFindings` — record, and the
//! window, ordering and paging this operation reports them under.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test signs for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// The access key id every request here is signed with; the emulator attributes every
/// signed request to one ambient identity regardless of the key.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// A product ARN of the shape Security Hub's own product uses. It is also the identity
/// of the importer in a `BATCH_IMPORT_FINDINGS` change event.
const PRODUCT_ARN: &str = "arn:aws:securityhub:us-east-1::product/aws/securityhub";

/// The only schema version the `AwsSecurityFinding` format defines.
const SCHEMA_VERSION: &str = "2018-10-08";

/// The `FindingHistoryUpdateSourceType` value for a change `BatchImportFindings` made
/// (`smithy_query(query="shape", shape="FindingHistoryUpdateSourceType")`).
const SOURCE_IMPORT: &str = "BATCH_IMPORT_FINDINGS";

/// The `FindingHistoryUpdateSourceType` value for a change `BatchUpdateFindings` made.
const SOURCE_UPDATE: &str = "BATCH_UPDATE_FINDINGS";

/// The ASFF timestamp fields `GetFindingHistory` excludes from a change event: the
/// five top-level ones the operation's docs name ("except top-level timestamp fields,
/// such as the CreatedAt and UpdatedAt fields" — every `AwsSecurityFinding` member
/// whose documentation calls it a timestamp, per `smithy_query(query="shape",
/// shape="AwsSecurityFinding")`), plus `Note.UpdatedAt`, which the service stamps
/// itself while applying a note update (`NoteUpdate` carries only `Text` and
/// `UpdatedBy`).
const EXCLUDED_FIELDS: [&str; 6] = [
    "CreatedAt",
    "UpdatedAt",
    "FirstObservedAt",
    "LastObservedAt",
    "ProcessedAt",
    "Note.UpdatedAt",
];

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

    /// Dispatch one request and return `(status, parsed body)`.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20261022/{REGION}/securityhub/aws4_request, \
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

    async fn enable(&self) -> (u16, Value) {
        self.call("POST", "/accounts", Some(json!({}))).await
    }

    async fn import(&self, findings: Vec<Value>) -> (u16, Value) {
        self.call("POST", "/findings/import", Some(json!({ "Findings": findings })))
            .await
    }

    async fn update_findings(&self, body: Value) -> (u16, Value) {
        self.call("PATCH", "/findings/batchupdate", Some(body)).await
    }

    async fn get_findings(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/findings", Some(body)).await
    }

    async fn finding_history(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/findingHistory/get", Some(body)).await
    }

    /// `GetFindingHistory` for `id`, asserting it succeeded, and its `Records`.
    async fn records_for(&self, case: &str, id: &str) -> Vec<Value> {
        let (status, body) = self.finding_history(json!({ "FindingIdentifier": identifier(id) })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetFindingHistory expected 200, got {status} with {body}"
        );
        records(&body)
    }

    /// Every record for `id`, paged through with the largest page the model allows.
    async fn all_records_for(&self, case: &str, id: &str) -> Vec<Value> {
        let mut all: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({
                "FindingIdentifier": identifier(id),
                "MaxResults": 100,
            });
            if let Some(cursor) = token.as_deref() {
                request["NextToken"] = json!(cursor);
            }
            let (status, body) = self.finding_history(request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetFindingHistory expected 200, got {status} with {body}"
            );
            all.extend(records(&body));
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                return all;
            }
        }
    }

    /// Enable the hub and import `findings`, asserting both succeed.
    async fn with_findings(findings: Vec<Value>) -> Self {
        let harness = Self::new();
        let (status, body) = harness.enable().await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        let count = findings.len();
        let (status, body) = harness.import(findings).await;
        assert_eq!(
            status, 200,
            "BatchImportFindings must succeed, got {status} with {body}"
        );
        assert_eq!(
            body["SuccessCount"].as_u64(),
            Some(count as u64),
            "every seeded finding must import: {body}"
        );
        harness
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

/// The modelled `AwsSecurityFindingIdentifier` for `id` under [`PRODUCT_ARN`].
fn identifier(id: &str) -> Value {
    json!({ "Id": id, "ProductArn": PRODUCT_ARN })
}

/// The `Records` of a `GetFindingHistory` response.
fn records(body: &Value) -> Vec<Value> {
    body["Records"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("GetFindingHistory must answer a Records list, got {body}"))
}

/// One record's `Updates`, as a `field -> (old, new)` map.
fn updates_of(record: &Value) -> BTreeMap<String, (Option<String>, Option<String>)> {
    record["Updates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|update| {
            (
                update["UpdatedField"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                (
                    update["OldValue"].as_str().map(str::to_string),
                    update["NewValue"].as_str().map(str::to_string),
                ),
            )
        })
        .collect()
}

/// One row of the update-history table: the case name, the `BatchUpdateFindings`
/// members to send, and the `(field, old, new)` triples the resulting change event
/// must carry — the ASFF field name, and the text the event reports for its value
/// before and after (a structure, list or map renders as its JSON).
type UpdateHistoryCase = (
    &'static str,
    Value,
    &'static [(&'static str, Option<&'static str>, Option<&'static str>)],
);

/// One row of the re-import table: the case name, the mutation applied to the
/// re-imported finding, and the fields the resulting change event must name.
type ReimportCase = (&'static str, fn(&mut Value), &'static [&'static str]);

/// The principal the emulator attributes a signed request to: the ambient IAM user
/// STS's `GetCallerIdentity` answers with.
fn caller_identity() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:user/emulator")
}

// ---------------------------------------------------------------------------
// The creating import
// ---------------------------------------------------------------------------

/// RULE: a finding's history is the list of writes it has received; the import that
/// created the record is one event, marked `FindingCreated` true, sourced to
/// `BATCH_IMPORT_FINDINGS` and identified by the product the finding was submitted
/// under.
///
/// The table spans finding shapes that differ in the members they carry — the fullest
/// form, the bare required-member form, and one carrying a note and user-defined
/// fields — because the creating event is one event whatever the finding holds.
#[tokio::test]
async fn the_creating_import_is_one_history_event() {
    let mut rich = base_finding("rich");
    rich["Note"] = json!({
        "Text": "seeded",
        "UpdatedBy": "importer",
        "UpdatedAt": "2026-10-21T21:30:00Z",
    });
    rich["UserDefinedFields"] = json!({ "owner": "alice" });
    rich["Confidence"] = json!(10);

    let mut minimal = json!({
        "SchemaVersion": SCHEMA_VERSION,
        "Id": "minimal",
        "ProductArn": PRODUCT_ARN,
        "GeneratorId": "generator/beta",
        "AwsAccountId": ACCOUNT_ID,
        "CreatedAt": "2026-10-21T20:00:00Z",
        "UpdatedAt": "2026-10-21T20:00:00Z",
        "Title": "Minimal",
        "Description": "Only the required members",
        "Resources": [{ "Type": "AwsEc2Instance", "Id": "i-1" }],
        // The Amazon Finding Format requires a conforming finding to populate
        // `Severity` or `FindingProviderFields`, so the bare form carries one of them.
        "Severity": { "Label": "LOW" },
    });
    minimal["Types"] = json!(["TTPs/Initial Access"]);

    let cases: [(&str, Value); 3] = [
        ("full finding", base_finding("full")),
        ("rich finding", rich),
        ("required members only", minimal),
    ];

    for (case, finding) in cases {
        let id = finding["Id"].as_str().expect("the finding carries an Id").to_string();
        let harness = Harness::with_findings(vec![finding]).await;

        let found = harness.records_for(case, &id).await;
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected exactly the creating event, got {found:?}"
        );
        let record = &found[0];
        assert_eq!(
            record["FindingCreated"].as_bool(),
            Some(true),
            "case {case:?}: expected FindingCreated true, got {:?}",
            record["FindingCreated"]
        );
        assert_eq!(
            record["UpdateSource"]["Type"].as_str(),
            Some(SOURCE_IMPORT),
            "case {case:?}: expected UpdateSource.Type {SOURCE_IMPORT}, got {:?}",
            record["UpdateSource"]
        );
        assert_eq!(
            record["UpdateSource"]["Identity"].as_str(),
            Some(PRODUCT_ARN),
            "case {case:?}: expected UpdateSource.Identity {PRODUCT_ARN}, got {:?}",
            record["UpdateSource"]
        );
        assert_eq!(
            record["FindingIdentifier"],
            identifier(&id),
            "case {case:?}: expected the record to carry the finding's identifier, got {:?}",
            record["FindingIdentifier"]
        );
        // The model marks `Timestamp` `@timestampFormat("date-time")`, so the wire
        // form is an RFC 3339 string, never an epoch number.
        let update_time = record["UpdateTime"].as_str().unwrap_or_default();
        assert!(
            update_time.contains('T') && update_time.ends_with('Z'),
            "case {case:?}: expected an RFC 3339 UpdateTime, got {:?}",
            record["UpdateTime"]
        );
        assert!(
            !updates_of(record).is_empty(),
            "case {case:?}: the creating event names the fields the finding was created \
             with, got {:?}",
            record["Updates"]
        );
        let recorded = updates_of(record);
        // Every ASFF field the submitted finding carries is a field this event
        // created, with no prior value.
        for field in ["Id", "ProductArn", "Title", "Description", "Resources"] {
            let (old_value, new_value) = recorded
                .get(field)
                .unwrap_or_else(|| panic!("case {case:?}: expected {field} in {recorded:?}"));
            assert_eq!(
                old_value, &None,
                "case {case:?}: a created field has no prior value, got {old_value:?} for {field}"
            );
            assert!(
                new_value.is_some(),
                "case {case:?}: expected a value for {field}, got {new_value:?}"
            );
        }
        for excluded in EXCLUDED_FIELDS {
            assert!(
                !recorded.contains_key(excluded),
                "case {case:?}: {excluded} is a top-level timestamp field and must not be \
                 reported as a change, got {recorded:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What a write records
// ---------------------------------------------------------------------------

/// RULE: a write that changes a finding appends one change event naming, for each ASFF
/// field whose value differs after the write, that field and the text of its value
/// before and after — every ASFF field except the top-level timestamp members, with a
/// structure, list or map reported as one field carrying its JSON.
///
/// The table spans the member kinds `BatchUpdateFindings` can change: a structure with
/// one member, a structure whose members are partly derived, two scalars, an enum, a
/// list and a map.
#[tokio::test]
async fn an_update_records_every_changed_asff_field() {
    let cases: &[UpdateHistoryCase] = &[
        (
            "workflow status",
            json!({ "Workflow": { "Status": "NOTIFIED" } }),
            &[(
                "Workflow",
                Some(r#"{"Status":"NEW"}"#),
                Some(r#"{"Status":"NOTIFIED"}"#),
            )],
        ),
        (
            // `Severity$Normalized` is derived from `Label`, so one submitted member
            // changes two stored ones: "LOW - 1 ... HIGH - 70".
            "severity label and its derived normalized score",
            json!({ "Severity": { "Label": "LOW" } }),
            &[(
                "Severity",
                Some(r#"{"Label":"HIGH","Normalized":70}"#),
                Some(r#"{"Label":"LOW","Normalized":1}"#),
            )],
        ),
        (
            "confidence set for the first time",
            json!({ "Confidence": 42 }),
            &[("Confidence", None, Some("42"))],
        ),
        (
            "criticality set for the first time",
            json!({ "Criticality": 90 }),
            &[("Criticality", None, Some("90"))],
        ),
        (
            "verification state",
            json!({ "VerificationState": "TRUE_POSITIVE" }),
            &[("VerificationState", None, Some("TRUE_POSITIVE"))],
        ),
        (
            "types list replaced",
            json!({ "Types": ["Effects/Data Exposure"] }),
            &[(
                "Types",
                Some(r#"["Software and Configuration Checks/Vulnerabilities/CVE"]"#),
                Some(r#"["Effects/Data Exposure"]"#),
            )],
        ),
        (
            "user defined fields added",
            json!({ "UserDefinedFields": { "owner": "alice" } }),
            &[("UserDefinedFields", None, Some(r#"{"owner":"alice"}"#))],
        ),
    ];

    for (case, update, expected) in cases {
        let id = "subject";
        let harness = Harness::with_findings(vec![base_finding(id)]).await;

        let mut request = update.clone();
        request["FindingIdentifiers"] = json!([identifier(id)]);
        let (status, body) = harness.update_findings(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchUpdateFindings expected 200, got {status} with {body}"
        );

        let found = harness.records_for(case, id).await;
        assert_eq!(
            found.len(),
            2,
            "case {case:?}: expected the creating import plus one update event, got {found:?}"
        );
        // Most recent first: the update is the head, the creating import the tail.
        let record = &found[0];
        assert_eq!(
            record["FindingCreated"].as_bool(),
            Some(false),
            "case {case:?}: an update does not create a finding, got {:?}",
            record["FindingCreated"]
        );
        assert_eq!(
            record["UpdateSource"]["Type"].as_str(),
            Some(SOURCE_UPDATE),
            "case {case:?}: expected UpdateSource.Type {SOURCE_UPDATE}, got {:?}",
            record["UpdateSource"]
        );
        assert_eq!(
            record["UpdateSource"]["Identity"].as_str(),
            Some(caller_identity().as_str()),
            "case {case:?}: expected the signing principal as Identity, got {:?}",
            record["UpdateSource"]
        );

        let recorded = updates_of(record);
        for (field, old_value, new_value) in *expected {
            let actual = recorded.get(*field);
            let expected_pair = (
                old_value.map(str::to_string),
                new_value.map(str::to_string),
            );
            assert_eq!(
                actual,
                Some(&expected_pair),
                "case {case:?}: expected {field} to change {old_value:?} -> {new_value:?}, \
                 got {recorded:?}"
            );
        }
        assert_eq!(
            recorded.len(),
            expected.len(),
            "case {case:?}: expected exactly {} changed fields, got {recorded:?}",
            expected.len()
        );
        for excluded in EXCLUDED_FIELDS {
            assert!(
                !recorded.contains_key(excluded),
                "case {case:?}: {excluded} is a top-level timestamp field and must not be \
                 reported as a change, got {recorded:?}"
            );
        }
    }
}

/// RULE: the note a `BatchUpdateFindings` call writes is one changed ASFF field,
/// `Note`, whose reported new value is the stored note — the submitted `Text` and
/// `UpdatedBy` plus the `UpdatedAt` the service stamps (`NoteUpdate` carries only
/// `Text` and `UpdatedBy`, per `smithy_query(query="shape", shape="NoteUpdate")`).
///
/// The table spans a note written onto a finding that had none and a note that
/// replaces an existing one, so the reported old value is asserted both absent and
/// present.
#[tokio::test]
async fn a_note_update_records_the_note_field() {
    // (case, does the imported finding already carry a note?)
    for (case, seeded) in [("first note", false), ("replacing a note", true)] {
        let id = "annotated";
        let mut finding = base_finding(id);
        if seeded {
            finding["Note"] = json!({
                "Text": "first pass",
                "UpdatedBy": "importer",
                "UpdatedAt": "2026-10-21T21:30:00Z",
            });
        }
        let harness = Harness::with_findings(vec![finding]).await;

        let (status, body) = harness
            .update_findings(json!({
                "FindingIdentifiers": [identifier(id)],
                "Note": { "Text": "triaged", "UpdatedBy": "analyst" },
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchUpdateFindings expected 200, got {status} with {body}"
        );

        let found = harness.records_for(case, id).await;
        let recorded = updates_of(&found[0]);
        let named: Vec<&str> = recorded.keys().map(String::as_str).collect();
        assert_eq!(
            named,
            vec!["Note"],
            "case {case:?}: a note write changes the Note field and nothing else, got \
             {recorded:?}"
        );
        let (old_value, new_value) = recorded.get("Note").expect("the Note field is reported");
        let reported: Value = serde_json::from_str(new_value.as_deref().unwrap_or("null"))
            .unwrap_or(Value::Null);
        assert_eq!(
            reported["Text"].as_str(),
            Some("triaged"),
            "case {case:?}: expected the new note text, got {reported:?}"
        );
        assert_eq!(
            reported["UpdatedBy"].as_str(),
            Some("analyst"),
            "case {case:?}: expected the new note author, got {reported:?}"
        );
        let stamped = reported["UpdatedAt"].as_str().unwrap_or_default();
        assert!(
            stamped.contains('T') && stamped.ends_with('Z'),
            "case {case:?}: expected the service's note timestamp, got {reported:?}"
        );
        match seeded {
            false => assert_eq!(
                old_value, &None,
                "case {case:?}: a finding with no note has no prior value, got {old_value:?}"
            ),
            true => {
                let previous: Value = serde_json::from_str(old_value.as_deref().unwrap_or("null"))
                    .unwrap_or(Value::Null);
                assert_eq!(
                    previous["Text"].as_str(),
                    Some("first pass"),
                    "case {case:?}: expected the replaced note as the old value, got \
                     {previous:?}"
                );
            }
        }
    }
}

/// RULE: a re-import is a write like any other — it records a
/// `BATCH_IMPORT_FINDINGS` event naming the fields it changed, and records nothing
/// when it changes no ASFF field.
///
/// The table spans re-imports that change one member, several members, and none.
#[tokio::test]
async fn a_reimport_records_only_what_it_changed() {
    // (case, mutation of the re-imported finding, the fields the event must name)
    let cases: [ReimportCase; 4] = [
        ("title changed", |f| f["Title"] = json!("Retitled"), &["Title"]),
        (
            "title and description changed",
            |f| {
                f["Title"] = json!("Retitled");
                f["Description"] = json!("Redescribed");
            },
            &["Description", "Title"],
        ),
        (
            // Only top-level timestamps differ, and those are excluded from history.
            "timestamps only",
            |f| {
                f["UpdatedAt"] = json!("2026-10-23T09:00:00Z");
                f["LastObservedAt"] = json!("2026-10-23T09:00:00Z");
            },
            &[],
        ),
        ("identical re-import", |_| {}, &[]),
    ];

    for (case, mutate, expected_fields) in cases {
        let id = "reimported";
        let harness = Harness::with_findings(vec![base_finding(id)]).await;

        let mut again = base_finding(id);
        mutate(&mut again);
        let (status, body) = harness.import(vec![again]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-import expected 200, got {status} with {body}"
        );

        let found = harness.records_for(case, id).await;
        let expected_records = if expected_fields.is_empty() { 1 } else { 2 };
        assert_eq!(
            found.len(),
            expected_records,
            "case {case:?}: expected {expected_records} records, got {found:?}"
        );
        if expected_fields.is_empty() {
            continue;
        }
        let record = &found[0];
        assert_eq!(
            record["UpdateSource"]["Type"].as_str(),
            Some(SOURCE_IMPORT),
            "case {case:?}: expected UpdateSource.Type {SOURCE_IMPORT}, got {:?}",
            record["UpdateSource"]
        );
        assert_eq!(
            record["FindingCreated"].as_bool(),
            Some(false),
            "case {case:?}: a re-import does not create the finding, got {:?}",
            record["FindingCreated"]
        );
        let recorded = updates_of(record);
        let named: Vec<&str> = recorded.keys().map(String::as_str).collect();
        assert_eq!(
            named, *expected_fields,
            "case {case:?}: expected the event to name {expected_fields:?}, got {recorded:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Ordering, window and paging
// ---------------------------------------------------------------------------

/// RULE: the history is reported most recent change first, and every event the finding
/// has received is reported — one per write that changed it.
///
/// The table spans write counts: one update, three updates and six updates, so the
/// ordering is asserted on a history longer than one page boundary could hide.
#[tokio::test]
async fn history_is_reported_most_recent_first() {
    // The confidence scores to walk, in order; each distinct value is one write.
    let scores = [11, 22, 33, 44, 55, 66];

    for write_count in [1usize, 3, 6] {
        let case = format!("{write_count} updates");
        let id = "ordered";
        let harness = Harness::with_findings(vec![base_finding(id)]).await;

        for score in scores.iter().take(write_count) {
            let (status, body) = harness
                .update_findings(json!({
                    "FindingIdentifiers": [identifier(id)],
                    "Confidence": score,
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchUpdateFindings expected 200, got {status} with {body}"
            );
        }

        let found = harness.records_for(&case, id).await;
        assert_eq!(
            found.len(),
            write_count + 1,
            "case {case:?}: expected one event per write plus the creating import, got {found:?}"
        );
        // The creating import is the oldest event, so it sorts last.
        assert_eq!(
            found[write_count]["FindingCreated"].as_bool(),
            Some(true),
            "case {case:?}: expected the creating import last, got {found:?}"
        );
        // Reading the scores from newest to oldest reverses the order they were
        // written in.
        let reported: Vec<String> = found
            .iter()
            .take(write_count)
            .map(|record| {
                updates_of(record)
                    .get("Confidence")
                    .and_then(|(_, new_value)| new_value.clone())
                    .unwrap_or_default()
            })
            .collect();
        let mut expected: Vec<String> = scores
            .iter()
            .take(write_count)
            .map(|score| score.to_string())
            .collect();
        expected.reverse();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the newest change first, got {reported:?}"
        );
    }
}

/// RULE: a finding's history is a BOUNDED store — a caller may write one finding
/// without limit, so the oldest events are dropped once the store is full, and the
/// write just made is always the newest record.
///
/// The two rows span the two bounds: many small events hit the event-count bound,
/// while fewer large ones hit the text-byte bound first. The bounds themselves are
/// `MAX_HISTORY_EVENTS_PER_FINDING` (256) and `MAX_HISTORY_BYTES_PER_FINDING`
/// (128 KiB) in the crate's `state.rs`. The large row writes the longest note the
/// service accepts — `Note$Text` is bounded at 512 characters
/// (`smithy_query(query="doc", shape="Note", member="Text")`), which
/// `state::MAX_NOTE_TEXT_LEN` holds every note write to — so 200 of them still pass
/// the byte bound while staying under the event-count bound.
#[tokio::test]
async fn the_recorded_history_stays_bounded() {
    /// The event-count bound in `state::MAX_HISTORY_EVENTS_PER_FINDING`.
    const MAX_EVENTS: usize = 256;

    // (case, note text length, number of updates)
    let cases: [(&str, usize, usize); 2] = [
        ("small events past the count bound", 8, MAX_EVENTS + 40),
        ("large events past the byte bound", 512, 200),
    ];

    for (case, note_len, writes) in cases {
        let id = "bounded-history";
        let harness = Harness::with_findings(vec![base_finding(id)]).await;

        let mut last_note = String::new();
        for write in 0..writes {
            // A distinct note each time, so every write really changes a field.
            last_note = format!("{write:0note_len$}");
            let (status, body) = harness
                .update_findings(json!({
                    "FindingIdentifiers": [identifier(id)],
                    "Note": { "Text": last_note, "UpdatedBy": "analyst" },
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchUpdateFindings expected 200, got {status} with {body}"
            );
        }

        let found = harness.all_records_for(case, id).await;
        assert!(
            !found.is_empty() && found.len() <= MAX_EVENTS,
            "case {case:?}: expected between 1 and {MAX_EVENTS} retained records, got {}",
            found.len()
        );
        // The newest write is always retained, and it is reported first.
        let newest = updates_of(&found[0]);
        let (_, new_value) = newest
            .get("Note")
            .unwrap_or_else(|| panic!("case {case:?}: expected the newest note, got {newest:?}"));
        assert!(
            new_value
                .as_deref()
                .is_some_and(|value| value.contains(last_note.as_str())),
            "case {case:?}: expected the last note {last_note:?} in the newest record, got \
             {new_value:?}"
        );
        // The store is full, so the creating import has aged out.
        assert!(
            found
                .iter()
                .all(|record| record["FindingCreated"].as_bool() == Some(false)),
            "case {case:?}: the oldest events, the creating import among them, must be \
             dropped once the store is full"
        );
        if note_len > 8 {
            // Each event carries the old and the new note, so the byte budget bites
            // before the count bound does.
            assert!(
                found.len() < MAX_EVENTS,
                "case {case:?}: expected the byte bound to retain fewer than {MAX_EVENTS} \
                 records, got {}",
                found.len()
            );
        }
    }
}

/// RULE: `MaxResults` bounds one page and `NextToken` resumes the next, so paging a
/// history reports every event exactly once and then stops issuing a token.
///
/// The table spans page sizes from one record to more than the history holds.
#[tokio::test]
async fn next_token_pages_the_history_exactly_once() {
    for page_size in [1usize, 2, 3, 5] {
        let case = format!("MaxResults {page_size}");
        let id = "paged";
        let harness = Harness::with_findings(vec![base_finding(id)]).await;
        // Four writes plus the creating import: five events.
        for confidence in [11, 22, 33, 44] {
            let (status, body) = harness
                .update_findings(json!({
                    "FindingIdentifiers": [identifier(id)],
                    "Confidence": confidence,
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchUpdateFindings expected 200, got {status} with {body}"
            );
        }
        let expected = harness.records_for(&case, id).await;
        assert_eq!(
            expected.len(),
            5,
            "case {case:?}: expected five events in one page, got {expected:?}"
        );

        let mut paged: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..10 {
            let mut request = json!({
                "FindingIdentifier": identifier(id),
                "MaxResults": page_size,
            });
            if let Some(cursor) = token.as_deref() {
                request["NextToken"] = json!(cursor);
            }
            let (status, body) = harness.finding_history(request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetFindingHistory expected 200, got {status} with {body}"
            );
            let page = records(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} records, got {}",
                page.len()
            );
            paged.extend(page);
            token = body["NextToken"].as_str().map(str::to_string);
            if let Some(cursor) = token.as_deref() {
                assert!(
                    cursor.parse::<usize>().is_err(),
                    "case {case:?}: the token must be opaque, got {cursor:?}"
                );
            } else {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: paging must end, still holding {token:?}"
        );
        assert_eq!(
            paged, expected,
            "case {case:?}: paging must report every event exactly once, in order"
        );
    }

    // A token this operation never issued is refused rather than read as an offset.
    let harness = Harness::with_findings(vec![base_finding("paged")]).await;
    for token in ["0", "not-a-token", "ffff"] {
        let (status, body) = harness
            .finding_history(json!({
                "FindingIdentifier": identifier("paged"),
                "NextToken": token,
            }))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: an invented NextToken must be refused, got {status} with {body}"
        );
    }
}

/// RULE: `StartTime` and `EndTime` bound the reported window — an event outside it is
/// not reported — and an omitted bound is the widest the member documents, so it
/// excludes nothing.
#[tokio::test]
async fn the_time_window_selects_which_events_are_reported() {
    let id = "windowed";
    let harness = Harness::with_findings(vec![base_finding(id)]).await;
    let (status, body) = harness
        .update_findings(json!({
            "FindingIdentifiers": [identifier(id)],
            "Workflow": { "Status": "RESOLVED" },
        }))
        .await;
    assert_eq!(
        status, 200,
        "BatchUpdateFindings expected 200, got {status} with {body}"
    );
    let all = harness.records_for("no window", id).await;
    assert_eq!(all.len(), 2, "expected two events, got {all:?}");

    // Every event was recorded by the calls above, so a window around now holds them
    // all, one entirely in the past or entirely in the future holds none.
    let distant_past = "2000-01-01T00:00:00Z";
    let recent_past = "2020-01-01T00:00:00Z";
    let distant_future = "2200-01-01T00:00:00Z";
    let cases: [(&str, Value, usize); 6] = [
        ("start only, in the past", json!({ "StartTime": recent_past }), 2),
        ("end only, in the future", json!({ "EndTime": distant_future }), 2),
        (
            "window spanning now",
            json!({ "StartTime": recent_past, "EndTime": distant_future }),
            2,
        ),
        ("start after every event", json!({ "StartTime": distant_future }), 0),
        ("end before every event", json!({ "EndTime": recent_past }), 0),
        (
            "window entirely in the past",
            json!({ "StartTime": distant_past, "EndTime": recent_past }),
            0,
        ),
    ];

    for (case, window, expected) in cases {
        let mut request = window;
        request["FindingIdentifier"] = identifier(id);
        let (status, body) = harness.finding_history(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let found = records(&body);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} records, got {found:?}"
        );
    }

    // A window whose start is after its end is not a window.
    let (status, body) = harness
        .finding_history(json!({
            "FindingIdentifier": identifier(id),
            "StartTime": distant_future,
            "EndTime": recent_past,
        }))
        .await;
    assert_eq!(
        status, 400,
        "an inverted window must be refused, got {status} with {body}"
    );
}

/// RULE: `MaxResults` is modelled `@range(min: 1, max: 100)`, so each published bound
/// is accepted and a value past either one is refused.
#[tokio::test]
async fn max_results_honours_its_published_range() {
    let id = "bounded";
    let harness = Harness::with_findings(vec![base_finding(id)]).await;

    // (case, MaxResults, expected status)
    let cases: [(&str, i64, u16); 4] = [
        ("minimum", 1, 200),
        ("maximum", 100, 200),
        ("below the minimum", 0, 400),
        ("above the maximum", 101, 400),
    ];
    for (case, max_results, expected) in cases {
        let (status, body) = harness
            .finding_history(json!({
                "FindingIdentifier": identifier(id),
                "MaxResults": max_results,
            }))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals and read-only behaviour
// ---------------------------------------------------------------------------

/// RULE: a finding identifier this account and Region holds no record for has no
/// history, which is an empty report and not an error.
#[tokio::test]
async fn an_unknown_identifier_has_an_empty_history() {
    let harness = Harness::with_findings(vec![base_finding("known")]).await;

    // (case, identifier)
    let cases: [(&str, Value); 3] = [
        ("unknown id", identifier("never-imported")),
        (
            "unknown product",
            json!({ "Id": "known", "ProductArn": "arn:aws:securityhub:us-east-1::product/other/scanner" }),
        ),
        (
            "both unknown",
            json!({ "Id": "nope", "ProductArn": "arn:aws:securityhub:us-east-1::product/other/scanner" }),
        ),
    ];
    for (case, finding_identifier) in cases {
        let (status, body) = harness
            .finding_history(json!({ "FindingIdentifier": finding_identifier }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let found = records(&body);
        assert!(
            found.is_empty(),
            "case {case:?}: expected no records, got {found:?}"
        );
    }
}

/// RULE: an account that is not subscribed to Security Hub CSPM in the Region cannot
/// read findings there — the operation answers `InvalidAccessException` (401).
#[tokio::test]
async fn an_unsubscribed_account_cannot_read_finding_history() {
    let harness = Harness::new();
    let (status, body) = harness
        .finding_history(json!({ "FindingIdentifier": identifier("any") }))
        .await;
    assert_eq!(
        status, 401,
        "expected InvalidAccessException (401), got {status} with {body}"
    );
}

/// RULE: reporting a history changes nothing a later caller sees — neither the history
/// itself nor the finding it belongs to.
#[tokio::test]
async fn reading_the_history_changes_nothing() {
    let id = "stable";
    let harness = Harness::with_findings(vec![base_finding(id)]).await;
    let (status, body) = harness
        .update_findings(json!({
            "FindingIdentifiers": [identifier(id)],
            "Severity": { "Label": "CRITICAL" },
        }))
        .await;
    assert_eq!(
        status, 200,
        "BatchUpdateFindings expected 200, got {status} with {body}"
    );

    let first = harness.records_for("first read", id).await;
    let second = harness.records_for("second read", id).await;
    assert_eq!(
        second, first,
        "the reported history must be stable across calls, got {second:?} after {first:?}"
    );

    let (status, findings) = harness.get_findings(json!({})).await;
    assert_eq!(
        status, 200,
        "GetFindings expected 200, got {status} with {findings}"
    );
    let stored = findings["Findings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        stored.len(),
        1,
        "reading history must not add or drop a finding, got {stored:?}"
    );
    assert_eq!(
        stored[0]["Severity"]["Label"].as_str(),
        Some("CRITICAL"),
        "reading history must not change the finding, got {:?}",
        stored[0]["Severity"]
    );
}

/// RULE: a finding's history belongs to that finding — a write to one finding records
/// nothing on another.
#[tokio::test]
async fn history_is_per_finding() {
    let harness = Harness::with_findings(vec![base_finding("left"), base_finding("right")]).await;
    let (status, body) = harness
        .update_findings(json!({
            "FindingIdentifiers": [identifier("left")],
            "Confidence": 55,
        }))
        .await;
    assert_eq!(
        status, 200,
        "BatchUpdateFindings expected 200, got {status} with {body}"
    );

    // (case, finding id, expected record count)
    for (case, id, expected) in [("updated finding", "left", 2), ("untouched finding", "right", 1)] {
        let found = harness.records_for(case, id).await;
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} records, got {found:?}"
        );
    }
}
