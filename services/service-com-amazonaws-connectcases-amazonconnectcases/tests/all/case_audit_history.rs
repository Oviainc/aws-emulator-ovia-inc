// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for a case's AUDIT HISTORY, driven through the
//! real `MultiServiceRouter` so every assertion goes over the wire the SDK
//! speaks.
//!
//! The rule under test: a case's audit history holds one entry per accepted edit
//! of that case — the creation included — most recent first, and each entry
//! reports the type of the edit, the instant it was performed, the IAM principal
//! whose credentials performed it (and the entity the request named), and for
//! every field value the edit wrote, the value the case held for that field
//! before the edit beside the value the edit stored. Reading the history writes
//! nothing, it is answered one bounded page at a time, and it belongs to the
//! case: deleting the case takes it with it.
//!
//! The table below is the class, not the recorded call: its rows differ in the
//! value written, the union variant and field type it is written with, how many
//! fields one edit writes, whether the field already held a value, and which
//! caller made the edit.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    MAX_AUDIT_EVENTS_PER_CASE, MAX_AUDIT_EVENTS_PER_PAGE,
};

// The request plumbing and the lifecycle helpers live in `tests/all/harness.rs`,
// shared with this crate's other suites.
use crate::harness::{
    domain, field_of_type, fields_of_every_type, is_minted_id, string_at, template, Emulator,
    DEFAULT_ACCESS_KEY_ID,
};

/// A second caller, so an edge attributed to the caller's own credentials is
/// told apart from one the default caller made.
const SECOND_ACCESS_KEY_ID: &str = "AKIDSECONDCALLER";

/// An Amazon Connect user ARN, for the `User`-typed field and for the entity a
/// request names as having performed an edit.
const AGENT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1";

/// The IAM principal an access key id stands for, as every ARN this emulator
/// emits is built (`arn:aws:iam::<account>:user/<access key id>`).
fn principal_arn(access_key_id: &str) -> String {
    format!("arn:aws:iam::000000000000:user/{access_key_id}")
}

/// One row of the edit table: what it is a case of, the access key id of the
/// caller that makes it, the entity the request names as having performed it,
/// and the field values it writes.
type Edit = (
    &'static str,
    &'static str,
    Option<Value>,
    Vec<(String, Value)>,
);

/// The audit-history path of one case.
fn history_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}/audit-history")
}

/// The `fields` list of a `CreateCase`/`UpdateCase` body that writes `writes`.
fn write_list(writes: &[(String, Value)]) -> Value {
    Value::Array(
        writes
            .iter()
            .map(|(id, value)| json!({ "id": id, "value": value }))
            .collect(),
    )
}

/// The `auditEvents` list of one page, which the model declares as a required
/// output member.
fn events_of(response: &Value, case: &str) -> Vec<Value> {
    response
        .get("auditEvents")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetCaseAuditEvents returned no auditEvents list: {response:?}")
        })
        .clone()
}

/// The field entries of one recorded edit, keyed by the field they are about,
/// each as `(oldValue, newValue)`.
fn field_entries(event: &Value, case: &str) -> HashMap<String, (Option<Value>, Value)> {
    event
        .get("fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: an audit event with no fields list: {event:?}"))
        .iter()
        .map(|field| {
            let id = string_at(field, "eventFieldId", case);
            let new_value = field
                .get("newValue")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an audit event field with no newValue: {field:?}")
                })
                .clone();
            (id, (field.get("oldValue").cloned(), new_value))
        })
        .collect()
}

/// What the history must report for one edit.
#[derive(Debug)]
struct ExpectedEvent {
    event_type: &'static str,
    /// The IAM principal ARN of the caller that made the edit.
    principal: String,
    /// The entity the request named as having performed the edit, when it named
    /// one.
    performed_by: Option<Value>,
    fields: HashMap<String, (Option<Value>, Value)>,
}

/// The entry an edge that writes `writes` must record, given the values the case
/// `held` before it: one field entry per written value, each carrying the value
/// previously held for that field as `oldValue` (absent when the case held
/// none). `held` is advanced to the values the case holds after the edit, which
/// is the merge the service itself performs.
fn record(
    event_type: &'static str,
    access_key_id: &str,
    performed_by: Option<Value>,
    writes: &[(String, Value)],
    held: &mut HashMap<String, Value>,
) -> ExpectedEvent {
    let fields = writes
        .iter()
        .map(|(id, value)| {
            let entry = (id.clone(), (held.get(id).cloned(), value.clone()));
            held.insert(id.clone(), value.clone());
            entry
        })
        .collect();
    ExpectedEvent {
        event_type,
        principal: principal_arn(access_key_id),
        performed_by,
        fields,
    }
}

/// Assert one page of history is the entries `expected` names, newest LAST in
/// `expected` and newest FIRST on the page, with every entry's own members
/// computed from the edit it records.
fn assert_history(case: &str, events: &[Value], expected: &[ExpectedEvent]) {
    assert_eq!(
        events.len(),
        expected.len(),
        "case {case:?}: expected one entry per edit of the case ({}), got {} in {events:?}",
        expected.len(),
        events.len()
    );
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut later_instant: Option<f64> = None;
    for (offset, event) in events.iter().enumerate() {
        let expected = &expected[expected.len() - 1 - offset];
        let case = format!("{case}, entry {offset} from the most recent");

        assert_eq!(
            string_at(event, "type", &case),
            expected.event_type,
            "case {case:?}: expected the type of the edit it records, got {event:?}"
        );

        let event_id = string_at(event, "eventId", &case);
        assert!(
            is_minted_id(&event_id),
            "case {case:?}: an eventId must be a server-minted opaque id, got {event_id:?}"
        );
        assert!(
            seen_ids.insert(event_id.clone()),
            "case {case:?}: two entries share the eventId {event_id:?}"
        );

        // The instant is the one the edit was performed at, so its VALUE is this
        // run's: what is asserted is that every entry reports one, and that the
        // entries run from the most recently performed edit to the oldest.
        let performed_time = event
            .get("performedTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: an entry with no performedTime instant: {event:?}")
            });
        assert!(
            performed_time > 0.0,
            "case {case:?}: performedTime must be the instant the edit was performed, \
             got {performed_time}"
        );
        if let Some(later) = later_instant {
            assert!(
                performed_time <= later,
                "case {case:?}: expected an entry no later than the one before it ({later}), \
                 got {performed_time}"
            );
        }
        later_instant = Some(performed_time);

        let performed_by = event
            .get("performedBy")
            .unwrap_or_else(|| panic!("case {case:?}: an entry with no performedBy: {event:?}"));
        assert_eq!(
            string_at(performed_by, "iamPrincipalArn", &case),
            expected.principal,
            "case {case:?}: the edit must be attributed to the caller that made it, \
             got {performed_by:?}"
        );
        assert_eq!(
            performed_by.get("user").cloned(),
            expected.performed_by,
            "case {case:?}: expected the entity the request named, got {performed_by:?}"
        );

        assert_eq!(
            field_entries(event, &case),
            expected.fields,
            "case {case:?}: expected the fields the edit wrote, each with the value held \
             before it, got {event:?}"
        );
    }
}

/// Every entry of a case's audit history, newest first, read by FOLLOWING the
/// pages the service hands out — so a history longer than one page is still read
/// whole, and no page may exceed the modelled maximum.
async fn full_history(emulator: &Emulator, case: &str, domain_id: &str, case_id: &str) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let body = match &next_token {
            Some(token) => json!({ "nextToken": token }),
            None => json!({}),
        };
        let response = emulator
            .ok(case, "POST", &history_path(domain_id, case_id), body)
            .await;
        let events = events_of(&response, case);
        assert!(
            events.len() <= MAX_AUDIT_EVENTS_PER_PAGE as usize,
            "case {case:?}: a page must hold at most {MAX_AUDIT_EVENTS_PER_PAGE} entries, \
             got {}",
            events.len()
        );
        all.extend(events);
        next_token = response
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            all.len() <= MAX_AUDIT_EVENTS_PER_CASE,
            "case {case:?}: the history cannot hold more than {MAX_AUDIT_EVENTS_PER_CASE} \
             entries, so following its tokens must terminate, got {} so far",
            all.len()
        );
    }
    all
}

/// A domain with one field per modelled `FieldType` and a template that requires
/// none of them, so an edit can write any subset of them.
async fn domain_with_a_field_of_every_type(
    emulator: &Emulator,
    name: &str,
) -> (String, Vec<(&'static str, String)>, String) {
    let domain_id = domain(emulator, name).await;
    let fields = fields_of_every_type(emulator, &domain_id).await;
    let template_id = template(emulator, &domain_id, &format!("{name}-template"), &[]).await;
    (domain_id, fields, template_id)
}

#[tokio::test]
async fn the_audit_history_reports_one_entry_per_edit_of_the_case_newest_first() {
    let emulator = Emulator::new();
    let (domain_id, fields, template_id) =
        domain_with_a_field_of_every_type(&emulator, "audit-history").await;
    let text = field_of_type(&fields, "Text");
    let number = field_of_type(&fields, "Number");
    let boolean = field_of_type(&fields, "Boolean");
    let user = field_of_type(&fields, "User");
    let single_select = field_of_type(&fields, "SingleSelect");
    let date_time = field_of_type(&fields, "DateTime");
    let url = field_of_type(&fields, "Url");

    // The values the case is created with: its first edit.
    let created_writes = vec![
        (text.clone(), json!({ "stringValue": "a customer note" })),
        (number.clone(), json!({ "doubleValue": 42.5 })),
    ];
    let created = emulator
        .ok(
            "CreateCase",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": write_list(&created_writes),
                "performedBy": { "userArn": AGENT_ARN },
            }),
        )
        .await;
    let case_id = string_at(&created, "caseId", "CreateCase");

    let mut held: HashMap<String, Value> = HashMap::new();
    let mut expected = vec![record(
        "Case.Created",
        DEFAULT_ACCESS_KEY_ID,
        Some(json!({ "userArn": AGENT_ARN })),
        &created_writes,
        &mut held,
    )];
    let history = full_history(&emulator, "GetCaseAuditEvents after CreateCase", &domain_id, &case_id).await;
    assert_history(
        "GetCaseAuditEvents after CreateCase",
        &history,
        &expected,
    );

    // The class of edits, one row per way an edit can differ: the value written,
    // the union variant and field type it is written with, how many fields it
    // writes, whether the field already held a value, who performed it, and the
    // edit that writes nothing at all.
    let edits: Vec<Edit> = vec![
        (
            "a new value for a field the case already holds",
            DEFAULT_ACCESS_KEY_ID,
            None,
            vec![(text.clone(), json!({ "stringValue": "an edited note" }))],
        ),
        (
            "a first value for a field the case holds none for",
            DEFAULT_ACCESS_KEY_ID,
            Some(json!({ "customEntity": "the-night-shift" })),
            vec![(boolean.clone(), json!({ "booleanValue": true }))],
        ),
        (
            "two fields at once, one held and one not",
            DEFAULT_ACCESS_KEY_ID,
            None,
            vec![
                (number.clone(), json!({ "doubleValue": 7.5 })),
                (user.clone(), json!({ "userArnValue": AGENT_ARN })),
            ],
        ),
        (
            "the union's empty value",
            DEFAULT_ACCESS_KEY_ID,
            None,
            vec![(single_select.clone(), json!({ "emptyValue": {} }))],
        ),
        (
            "the value the field started with, written again",
            DEFAULT_ACCESS_KEY_ID,
            None,
            vec![(text.clone(), json!({ "stringValue": "a customer note" }))],
        ),
        (
            "the string-valued field types",
            DEFAULT_ACCESS_KEY_ID,
            None,
            vec![
                (
                    date_time.clone(),
                    json!({ "stringValue": "2026-01-01T00:00:00Z" }),
                ),
                (
                    url.clone(),
                    json!({ "stringValue": "https://example.test/ticket/1" }),
                ),
            ],
        ),
        (
            "an edit made by another caller",
            SECOND_ACCESS_KEY_ID,
            None,
            vec![(text.clone(), json!({ "stringValue": "reassigned" }))],
        ),
        (
            "an update that writes no field value",
            DEFAULT_ACCESS_KEY_ID,
            None,
            vec![],
        ),
    ];

    for (row, access_key_id, performed_by, writes) in edits {
        let case = format!("UpdateCase with {row}");
        let mut body = json!({ "fields": write_list(&writes) });
        if let Some(performed_by) = performed_by.clone() {
            body["performedBy"] = performed_by;
        }
        emulator
            .ok_as(
                access_key_id,
                &case,
                "PUT",
                &format!("/domains/{domain_id}/cases/{case_id}"),
                body,
            )
            .await;
        // An audit entry reports the field values an edit wrote, and this row
        // writes none.
        if !writes.is_empty() {
            expected.push(record(
                "Case.Updated",
                access_key_id,
                performed_by,
                &writes,
                &mut held,
            ));
        }

        let history = full_history(&emulator, &case, &domain_id, &case_id).await;
        assert_history(&case, &history, &expected);
    }

    // Reading the history changes nothing a later caller sees: the same read,
    // made twice in a row, answers the same entries.
    let again = full_history(
        &emulator,
        "GetCaseAuditEvents read twice",
        &domain_id,
        &case_id,
    )
    .await;
    assert_history("GetCaseAuditEvents read twice", &again, &expected);
}

#[tokio::test]
async fn the_audit_history_is_paged_newest_first_within_the_modelled_page_size() {
    let emulator = Emulator::new();
    let (domain_id, fields, template_id) =
        domain_with_a_field_of_every_type(&emulator, "audit-paging").await;
    let text = field_of_type(&fields, "Text");

    // One edit more than three pages' worth, each writing a value that names the
    // edit it belongs to, so the order the history reports them in is readable
    // off the page.
    let edits = MAX_AUDIT_EVENTS_PER_PAGE + 3;
    let value_of_edit = |edit: i32| json!({ "stringValue": format!("edit-{edit}") });
    let created = emulator
        .ok(
            "CreateCase",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": value_of_edit(0) }],
            }),
        )
        .await;
    let case_id = string_at(&created, "caseId", "CreateCase");
    for edit in 1..=edits {
        emulator
            .ok(
                &format!("UpdateCase edit-{edit}"),
                "PUT",
                &format!("/domains/{domain_id}/cases/{case_id}"),
                json!({ "fields": [{ "id": text, "value": value_of_edit(edit) }] }),
            )
            .await;
    }
    let recorded = usize::try_from(edits + 1).expect("the number of edits fits a usize");

    // A request naming no maxResults gets the documented default page size, and
    // a token for the rest.
    let first = emulator
        .ok(
            "GetCaseAuditEvents with no maxResults",
            "POST",
            &history_path(&domain_id, &case_id),
            json!({}),
        )
        .await;
    assert_eq!(
        events_of(&first, "GetCaseAuditEvents with no maxResults").len(),
        MAX_AUDIT_EVENTS_PER_PAGE as usize,
        "case \"GetCaseAuditEvents with no maxResults\": expected the default page size \
         {MAX_AUDIT_EVENTS_PER_PAGE}, got {first:?}"
    );
    assert!(
        first.get("nextToken").and_then(Value::as_str).is_some(),
        "case \"GetCaseAuditEvents with no maxResults\": entries remain, so a token must be \
         handed out, got {first:?}"
    );

    // Following the tokens reports every recorded edit exactly once, most
    // recent first.
    let history = full_history(
        &emulator,
        "GetCaseAuditEvents followed to exhaustion",
        &domain_id,
        &case_id,
    )
    .await;
    assert_eq!(
        history.len(),
        recorded,
        "case \"GetCaseAuditEvents followed to exhaustion\": expected every recorded edit \
         once, got {}",
        history.len()
    );
    let order: Vec<Value> = history
        .iter()
        .map(|event| {
            field_entries(event, "GetCaseAuditEvents followed to exhaustion")
                .remove(&text)
                .expect("every edit here writes the Text field")
                .1
        })
        .collect();
    let newest_first: Vec<Value> = (0..=edits).rev().map(value_of_edit).collect();
    assert_eq!(
        order, newest_first,
        "case \"GetCaseAuditEvents followed to exhaustion\": expected the edits most recent \
         first, got {order:?}"
    );

    // Every page size the model admits, including both published bounds, is
    // honoured exactly, and the page still starts at the most recent edit.
    for requested in [1, 2, MAX_AUDIT_EVENTS_PER_PAGE - 1, MAX_AUDIT_EVENTS_PER_PAGE] {
        let case = format!("GetCaseAuditEvents maxResults={requested}");
        let page = emulator
            .ok(
                &case,
                "POST",
                &history_path(&domain_id, &case_id),
                json!({ "maxResults": requested }),
            )
            .await;
        let events = events_of(&page, &case);
        assert_eq!(
            events.len(),
            requested as usize,
            "case {case:?}: expected exactly the page size asked for, got {}",
            events.len()
        );
        assert_eq!(
            field_entries(&events[0], &case).remove(&text).map(|held| held.1),
            Some(value_of_edit(edits)),
            "case {case:?}: a page must start at the most recent edit, got {events:?}"
        );
    }

    // One past each published bound is refused rather than clamped.
    for requested in [0, MAX_AUDIT_EVENTS_PER_PAGE + 1] {
        let case = format!("GetCaseAuditEvents maxResults={requested}");
        let (status, code, message) = emulator
            .failure_detail(
                &case,
                "POST",
                &history_path(&domain_id, &case_id),
                Some(json!({ "maxResults": requested })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a page size outside 1..{MAX_AUDIT_EVENTS_PER_PAGE} must be \
             refused, got {status} {code}: {message}"
        );
    }
}

#[tokio::test]
async fn the_audit_history_resolves_its_references_and_dies_with_the_case() {
    let emulator = Emulator::new();
    let (domain_id, fields, template_id) =
        domain_with_a_field_of_every_type(&emulator, "audit-refusals").await;
    let text = field_of_type(&fields, "Text");
    let body = json!({
        "templateId": template_id,
        "fields": [{ "id": text, "value": { "stringValue": "a customer note" } }],
    });
    let first = emulator
        .ok(
            "CreateCase",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            body.clone(),
        )
        .await;
    let case_id = string_at(&first, "caseId", "CreateCase");
    let second = emulator
        .ok(
            "CreateCase another case",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            body,
        )
        .await;
    let other_case_id = string_at(&second, "caseId", "CreateCase another case");

    // Every id the request carries is resolved against persisted state.
    for (case, path) in [
        (
            "a domain that was never created",
            history_path("domain-that-was-never-created", &case_id),
        ),
        (
            "a case that was never created",
            history_path(&domain_id, "case-that-was-never-created"),
        ),
    ] {
        let (status, code) = emulator
            .failure(case, "POST", &path, Some(json!({})))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {code}"
        );
    }

    // A cursor is scoped to the history it was issued for: one from another
    // case, or one this service never issued, is refused rather than resumed at
    // a key it never ordered.
    let other = emulator
        .ok(
            "GetCaseAuditEvents for the other case",
            "POST",
            &history_path(&domain_id, &other_case_id),
            json!({ "maxResults": 1 }),
        )
        .await;
    let foreign_token = other
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut tokens = vec![("a token this service never issued", "not-a-token".to_string())];
    if let Some(foreign_token) = foreign_token {
        tokens.push(("a token issued for another case's history", foreign_token));
    }
    for (case, token) in tokens {
        let (status, code) = emulator
            .failure(
                case,
                "POST",
                &history_path(&domain_id, &case_id),
                Some(json!({ "nextToken": token })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}"
        );
    }

    // The history belongs to the case: deleting the case takes it with it, as
    // DeleteCase documents ("After a successful deletion, you cannot ... Access
    // audit history").
    emulator
        .ok(
            "DeleteCase",
            "DELETE",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            json!({}),
        )
        .await;
    let (status, code) = emulator
        .failure(
            "GetCaseAuditEvents after DeleteCase",
            "POST",
            &history_path(&domain_id, &case_id),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetCaseAuditEvents after DeleteCase\": a deleted case has no audit history \
         to read, got {status} {code}"
    );
}

#[tokio::test]
async fn the_audit_history_keeps_at_most_the_most_recent_recorded_edits() {
    let emulator = Emulator::new();
    let (domain_id, fields, template_id) =
        domain_with_a_field_of_every_type(&emulator, "audit-ceiling").await;
    let text = field_of_type(&fields, "Text");
    let value_of_edit = |edit: usize| json!({ "stringValue": format!("edit-{edit}") });
    let created = emulator
        .ok(
            "CreateCase",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": value_of_edit(0) }],
            }),
        )
        .await;
    let case_id = string_at(&created, "caseId", "CreateCase");

    // Two edits more than the history's ceiling: the store drops the oldest
    // entries rather than growing with the number of edits a caller makes.
    let edits = MAX_AUDIT_EVENTS_PER_CASE + 2;
    for edit in 1..=edits {
        emulator
            .ok(
                &format!("UpdateCase edit-{edit}"),
                "PUT",
                &format!("/domains/{domain_id}/cases/{case_id}"),
                json!({ "fields": [{ "id": text, "value": value_of_edit(edit) }] }),
            )
            .await;
    }

    let case = "GetCaseAuditEvents past the history's ceiling";
    let history = full_history(&emulator, case, &domain_id, &case_id).await;
    assert_eq!(
        history.len(),
        MAX_AUDIT_EVENTS_PER_CASE,
        "case {case:?}: expected the ceiling of {MAX_AUDIT_EVENTS_PER_CASE} entries, got {}",
        history.len()
    );
    let order: Vec<Value> = history
        .iter()
        .map(|event| {
            field_entries(event, case)
                .remove(&text)
                .expect("every edit here writes the Text field")
                .1
        })
        .collect();
    let newest_first: Vec<Value> = (edits + 1 - MAX_AUDIT_EVENTS_PER_CASE..=edits)
        .rev()
        .map(value_of_edit)
        .collect();
    assert_eq!(
        order, newest_first,
        "case {case:?}: expected the most recent {MAX_AUDIT_EVENTS_PER_CASE} edits, most \
         recent first, got {order:?}"
    );
    assert!(
        history
            .iter()
            .all(|event| string_at(event, "type", case) == "Case.Updated"),
        "case {case:?}: the creation is the oldest entry, so it is the first one the \
         ceiling drops, got {history:?}"
    );
}
