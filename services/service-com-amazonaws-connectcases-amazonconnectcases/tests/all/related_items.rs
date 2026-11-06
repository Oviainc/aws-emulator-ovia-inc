// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for ASSOCIATING A RELATED ITEM with a case,
//! driven through the real `MultiServiceRouter` so every assertion goes over
//! the wire the SDK speaks.
//!
//! The rule under test: `CreateRelatedItem` associates one new related item of
//! the type its content names with an existing case of an existing domain,
//! answering the item's own server-minted identifier and the ARN addressing it
//! under that case, and records the association in the case's audit history as
//! an event of the related-item kind naming the type of the item it is about.
//! Every reference the content makes into this service — the case a
//! `ConnectCase` item links to, the fields a `Custom` item writes values
//! against, the case field an `Sla` item tracks — is resolved before anything is
//! stored, and a request whose `type` and `content` describe different kinds of
//! item is refused — with the service's fixed sentence for a mismatched pair,
//! led by the type the REQUEST declared, upper-cased, and nothing else
//! interpolated — and nothing is associated.
//!
//! The table below is the class, not one recorded call: its rows differ in the
//! content type, the shape of the content, how many field values the item
//! carries, which entity the request names as having performed the association,
//! and (for the sizes and durations the model bounds) the value at each
//! published bound and one past it.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::ops::types::RELATED_ITEM_TYPE_VALUES;

// The request plumbing and the lifecycle helpers live in `tests/all/harness.rs`,
// shared with this crate's other suites.
use crate::harness::{
    domain, field_of_type, fields_of_every_type, is_minted_id, string_at, template, Emulator,
    DEFAULT_ACCESS_KEY_ID,
};

/// An Amazon Connect contact ARN: the external identifier a `Contact` item
/// links to, which belongs to another service and names no Cases resource.
const CONTACT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/contact/c-1";

/// An Amazon Connect attached-file ARN, as a `File` item links to one.
const FILE_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/file/f-1";

/// An Amazon Connect user ARN, for the entity a request names as having
/// performed the association.
const AGENT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1";

/// Shortest and longest `CommentContent$body` the model admits: `@length(1..15000)`
/// (`smithy_query(query="shape",shape="CommentContent")`).
const COMMENT_BODY_LENGTHS: (usize, usize) = (1, 15_000);

/// Smallest and largest `SlaInputConfiguration$targetSlaMinutes` the model
/// admits: `@range(1..1051200)`
/// (`smithy_query(query="shape",shape="SlaInputConfiguration")`).
const TARGET_SLA_MINUTES: (i64, i64) = (1, 1_051_200);

/// The IAM principal an access key id stands for, as every ARN this emulator
/// emits is built (`arn:aws:iam::<account>:user/<access key id>`).
fn principal_arn(access_key_id: &str) -> String {
    format!("arn:aws:iam::000000000000:user/{access_key_id}")
}

/// The `CreateRelatedItem` path of one case.
fn related_items_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}/related-items/")
}

/// The audit-history path of one case.
fn history_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}/audit-history")
}

/// One row of the association table: what it is a case of, the `type` the
/// request declares, the content it carries, the entity it names as having
/// performed the association, and the field values the item writes — which are
/// what its audit entry must report.
struct Association {
    case: &'static str,
    item_type: &'static str,
    content: Value,
    performed_by: Option<Value>,
    written_fields: Vec<(String, Value)>,
}

/// The association table: one row per value of `RelatedItemType`, each with the
/// content of that type, so the rule is asserted over the whole class rather
/// than over one content shape.
fn associations(text_field: &str, number_field: &str, linked_case_id: &str) -> Vec<Association> {
    vec![
        Association {
            case: "Comment item",
            item_type: "Comment",
            content: json!({
                "comment": { "body": "called the customer back", "contentType": "Text/Plain" },
            }),
            performed_by: Some(json!({ "userArn": AGENT_ARN })),
            written_fields: Vec::new(),
        },
        Association {
            case: "ConnectCase item",
            item_type: "ConnectCase",
            content: json!({ "connectCase": { "caseId": linked_case_id } }),
            performed_by: None,
            written_fields: Vec::new(),
        },
        Association {
            case: "Contact item",
            item_type: "Contact",
            content: json!({ "contact": { "contactArn": CONTACT_ARN } }),
            performed_by: Some(json!({ "customEntity": "night-shift-bot" })),
            written_fields: Vec::new(),
        },
        Association {
            case: "File item",
            item_type: "File",
            content: json!({ "file": { "fileArn": FILE_ARN } }),
            performed_by: None,
            written_fields: Vec::new(),
        },
        Association {
            case: "Custom item with two field values",
            item_type: "Custom",
            content: json!({
                "custom": {
                    "fields": [
                        { "id": text_field, "value": { "stringValue": "order-4711" } },
                        { "id": number_field, "value": { "doubleValue": 12.5 } },
                    ],
                },
            }),
            performed_by: Some(json!({ "userArn": AGENT_ARN })),
            written_fields: vec![
                (text_field.to_string(), json!({ "stringValue": "order-4711" })),
                (number_field.to_string(), json!({ "doubleValue": 12.5 })),
            ],
        },
        Association {
            case: "Sla item tracking a case field",
            item_type: "Sla",
            content: json!({
                "sla": {
                    "slaInputConfiguration": {
                        "name": "resolve before the target",
                        "type": "CaseField",
                        "fieldId": text_field,
                        "targetFieldValues": [{ "stringValue": "resolved" }],
                        "targetSlaMinutes": 60,
                    },
                },
            }),
            performed_by: None,
            written_fields: Vec::new(),
        },
    ]
}

/// The `CreateRelatedItem` body of one row.
fn association_body(row: &Association) -> Value {
    let mut body = json!({ "type": row.item_type, "content": row.content });
    if let Some(performed_by) = row.performed_by.clone() {
        body["performedBy"] = performed_by;
    }
    body
}

/// One page of a case's audit history, newest entry first.
async fn audit_events(emulator: &Emulator, case: &str, domain_id: &str, case_id: &str) -> Vec<Value> {
    let response = emulator
        .ok(case, "POST", &history_path(domain_id, case_id), json!({}))
        .await;
    assert_eq!(
        response.get("nextToken"),
        None,
        "case {case:?}: the history of this case fits one page, got {response:?}"
    );
    response
        .get("auditEvents")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no auditEvents list in {response:?}"))
        .clone()
}

/// The field entries of one recorded event, as `(eventFieldId, newValue)` in the
/// order reported, asserting that none of them carries an `oldValue`: the values
/// belong to the newly associated item, so the case held none of them before.
fn field_entries(event: &Value, case: &str) -> Vec<(String, Value)> {
    event
        .get("fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: an audit event with no fields list: {event:?}"))
        .iter()
        .map(|field| {
            assert_eq!(
                field.get("oldValue"),
                None,
                "case {case:?}: a newly associated item's value had no previous value, \
                 got {field:?}"
            );
            let id = string_at(field, "eventFieldId", case);
            let new_value = field
                .get("newValue")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an audit event field with no newValue: {field:?}")
                })
                .clone();
            (id, new_value)
        })
        .collect()
}

/// A domain with one field per modelled `FieldType`, a template requiring none
/// of them, and a case written against it: the resources an association is made
/// on. Returns `(domainId, templateId, caseId, fields)`.
async fn domain_with_a_case(
    emulator: &Emulator,
    name: &str,
) -> (String, String, String, Vec<(&'static str, String)>) {
    let domain_id = domain(emulator, name).await;
    let fields = fields_of_every_type(emulator, &domain_id).await;
    let template_id = template(emulator, &domain_id, &format!("{name}-template"), &[]).await;
    let case_id = create_case(emulator, &domain_id, &template_id, name).await;
    (domain_id, template_id, case_id, fields)
}

/// Create one case of `domain_id` and return its id.
async fn create_case(emulator: &Emulator, domain_id: &str, template_id: &str, case: &str) -> String {
    let created = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({ "templateId": template_id, "fields": [] }),
        )
        .await;
    string_at(&created, "caseId", case)
}

#[tokio::test]
async fn an_item_of_every_type_is_associated_with_its_case_and_recorded_in_its_history() {
    let emulator = Emulator::new();
    let (domain_id, template_id, case_id, fields) =
        domain_with_a_case(&emulator, "related-items").await;
    let text_field = field_of_type(&fields, "Text");
    let number_field = field_of_type(&fields, "Number");
    let linked_case_id = create_case(&emulator, &domain_id, &template_id, "the linked case").await;

    let rows = associations(&text_field, &number_field, &linked_case_id);
    // The table is the class: every value of the modelled enum is a row of it.
    for item_type in RELATED_ITEM_TYPE_VALUES {
        assert!(
            rows.iter().any(|row| row.item_type == *item_type),
            "the association table must carry a row for the {item_type} type"
        );
    }

    let mut minted: Vec<String> = Vec::new();
    // The case's creation is already one recorded edit, so the history is one
    // entry long before the first association.
    let mut expected_events = 1;
    for row in &rows {
        let created = emulator
            .ok(
                row.case,
                "POST",
                &related_items_path(&domain_id, &case_id),
                association_body(row),
            )
            .await;

        let related_item_id = string_at(&created, "relatedItemId", row.case);
        assert!(
            is_minted_id(&related_item_id),
            "case {:?}: the relatedItemId must be a server-minted opaque id, got \
             {related_item_id:?}",
            row.case
        );
        assert!(
            !minted.contains(&related_item_id),
            "case {:?}: each association is its own resource and gets its own id, but \
             {related_item_id:?} was already handed out in {minted:?}",
            row.case
        );
        assert_eq!(
            string_at(&created, "relatedItemArn", row.case),
            format!(
                "arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/case/{case_id}\
                 /related-item/{related_item_id}"
            ),
            "case {:?}: the relatedItemArn must address the stored item under its case, got \
             {created:?}",
            row.case
        );
        minted.push(related_item_id);

        expected_events += 1;
        let history = audit_events(&emulator, row.case, &domain_id, &case_id).await;
        assert_eq!(
            history.len(),
            expected_events,
            "case {:?}: the association must add exactly one entry to the case's history, \
             got {history:?}",
            row.case
        );
        let newest = &history[0];
        assert_eq!(
            string_at(newest, "type", row.case),
            "RelatedItem.Created",
            "case {:?}: the newest entry must record the association, got {newest:?}",
            row.case
        );
        assert_eq!(
            newest.get("relatedItemType").and_then(Value::as_str),
            Some(row.item_type),
            "case {:?}: the entry must name the type of the item it is about, got {newest:?}",
            row.case
        );
        let performed_by = newest
            .get("performedBy")
            .unwrap_or_else(|| panic!("case {:?}: an entry with no performedBy: {newest:?}", row.case));
        assert_eq!(
            string_at(performed_by, "iamPrincipalArn", row.case),
            principal_arn(DEFAULT_ACCESS_KEY_ID),
            "case {:?}: the association must be attributed to the caller that made it, got \
             {performed_by:?}",
            row.case
        );
        assert_eq!(
            performed_by.get("user"),
            row.performed_by.as_ref(),
            "case {:?}: the entry must name the entity the request declared, got {performed_by:?}",
            row.case
        );
        assert_eq!(
            field_entries(newest, row.case),
            row.written_fields,
            "case {:?}: the entry must report the field values the item wrote, got {newest:?}",
            row.case
        );
    }

    // The association belongs to the case it was made on: the case a
    // `ConnectCase` item links TO records nothing, and still holds only its own
    // creation.
    let linked_history = audit_events(&emulator, "the linked case", &domain_id, &linked_case_id).await;
    assert_eq!(
        linked_history.len(),
        1,
        "case \"the linked case\": a case linked to by another case's item records no edit of \
         its own, got {linked_history:?}"
    );
}

#[tokio::test]
async fn an_association_resolves_every_reference_it_makes_before_storing_anything() {
    let emulator = Emulator::new();
    let (domain_id, _template_id, case_id, fields) =
        domain_with_a_case(&emulator, "related-item-refusals").await;
    let text_field = field_of_type(&fields, "Text");
    let path = related_items_path(&domain_id, &case_id);

    // (case, path, body, status, error code)
    let refusals: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "unknown domain",
            related_items_path("no-such-domain", &case_id),
            json!({ "type": "Comment", "content": { "comment": { "body": "x", "contentType": "Text/Plain" } } }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown case",
            related_items_path(&domain_id, "no-such-case"),
            json!({ "type": "Comment", "content": { "comment": { "body": "x", "contentType": "Text/Plain" } } }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "ConnectCase link to a case the domain does not hold",
            path.clone(),
            json!({ "type": "ConnectCase", "content": { "connectCase": { "caseId": "no-such-case" } } }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "type names another kind of content",
            path.clone(),
            json!({ "type": "Comment", "content": { "contact": { "contactArn": CONTACT_ARN } } }),
            400,
            "ValidationException",
        ),
        (
            "type is no RelatedItemType at all",
            path.clone(),
            json!({ "type": "Booking", "content": { "contact": { "contactArn": CONTACT_ARN } } }),
            400,
            "ValidationException",
        ),
        (
            "Custom item naming a field the domain does not hold",
            path.clone(),
            json!({
                "type": "Custom",
                "content": { "custom": { "fields": [
                    { "id": "no-such-field", "value": { "stringValue": "x" } },
                ] } },
            }),
            400,
            "ValidationException",
        ),
        (
            "Custom item writing a value of the wrong type",
            path.clone(),
            json!({
                "type": "Custom",
                "content": { "custom": { "fields": [
                    { "id": text_field, "value": { "booleanValue": true } },
                ] } },
            }),
            400,
            "ValidationException",
        ),
        (
            "Custom item writing one field twice",
            path.clone(),
            json!({
                "type": "Custom",
                "content": { "custom": { "fields": [
                    { "id": text_field, "value": { "stringValue": "first" } },
                    { "id": text_field, "value": { "stringValue": "second" } },
                ] } },
            }),
            400,
            "ValidationException",
        ),
        (
            "Sla tracking a field the domain does not hold",
            path.clone(),
            json!({
                "type": "Sla",
                "content": { "sla": { "slaInputConfiguration": {
                    "name": "a target on a dangling field",
                    "type": "CaseField",
                    "fieldId": "no-such-field",
                    "targetSlaMinutes": 60,
                } } },
            }),
            400,
            "ValidationException",
        ),
        (
            "Sla targeting a value of the wrong type",
            path.clone(),
            json!({
                "type": "Sla",
                "content": { "sla": { "slaInputConfiguration": {
                    "name": "a target of the wrong type",
                    "type": "CaseField",
                    "fieldId": text_field,
                    "targetFieldValues": [{ "booleanValue": true }],
                    "targetSlaMinutes": 60,
                } } },
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, refused_path, body, status, code) in &refusals {
        let (seen_status, seen_code, message) = emulator
            .failure_detail(case, "POST", refused_path, Some(body.clone()))
            .await;
        assert_eq!(
            (seen_status, seen_code.as_str()),
            (*status, *code),
            "case {case:?}: expected HTTP {status} {code}, got {seen_status} {seen_code} with \
             message {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: a refusal must say what is wrong, got {message:?}"
        );
    }

    // Nothing above was stored: the case still holds only its own creation.
    let history = audit_events(&emulator, "after the refusals", &domain_id, &case_id).await;
    assert_eq!(
        history.len(),
        1,
        "case \"after the refusals\": a refused association records no edit, got {history:?}"
    );
}

#[tokio::test]
async fn an_association_is_accepted_at_each_modelled_bound_and_refused_past_it() {
    let emulator = Emulator::new();
    let (domain_id, _template_id, case_id, _fields) =
        domain_with_a_case(&emulator, "related-item-bounds").await;
    let path = related_items_path(&domain_id, &case_id);

    let (shortest_body, longest_body) = COMMENT_BODY_LENGTHS;
    let (fewest_minutes, most_minutes) = TARGET_SLA_MINUTES;

    // (case, body, whether the model admits it)
    let rows: Vec<(String, Value, bool)> = vec![
        (
            format!("comment body of {shortest_body} character"),
            comment_body(shortest_body),
            true,
        ),
        (
            format!("comment body of {longest_body} characters"),
            comment_body(longest_body),
            true,
        ),
        (
            format!("comment body of {} characters", longest_body + 1),
            comment_body(longest_body + 1),
            false,
        ),
        (
            format!("comment body of {} characters", shortest_body - 1),
            comment_body(shortest_body - 1),
            false,
        ),
        (
            format!("an SLA target of {fewest_minutes} minute"),
            sla_target(fewest_minutes),
            true,
        ),
        (
            format!("an SLA target of {most_minutes} minutes"),
            sla_target(most_minutes),
            true,
        ),
        (
            format!("an SLA target of {} minutes", most_minutes + 1),
            sla_target(most_minutes + 1),
            false,
        ),
        (
            format!("an SLA target of {} minutes", fewest_minutes - 1),
            sla_target(fewest_minutes - 1),
            false,
        ),
    ];

    let mut accepted = 0;
    for (case, body, admitted) in &rows {
        if *admitted {
            let created = emulator.ok(case, "POST", &path, body.clone()).await;
            let related_item_id = string_at(&created, "relatedItemId", case);
            assert!(
                is_minted_id(&related_item_id),
                "case {case:?}: an association at a modelled bound is stored under a minted id, \
                 got {created:?}"
            );
            accepted += 1;
        } else {
            let (status, code, message) = emulator
                .failure_detail(case, "POST", &path, Some(body.clone()))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: a value past a modelled bound is a bad request, got {status} \
                 {code} with message {message:?}"
            );
        }
    }

    // Exactly the admitted rows were stored, each as one more recorded edit of
    // the case beside its creation.
    let history = audit_events(&emulator, "bounds", &domain_id, &case_id).await;
    assert_eq!(
        history.len(),
        accepted + 1,
        "case \"bounds\": one recorded association per accepted request, got {history:?}"
    );
}

/// One content of every kind an association can carry, as
/// `(RelatedItemType value, content)`, taken from the ONE association table
/// this suite keeps ([`associations`]) so a content shape is spelled once. The
/// model names the six members of `RelatedItemInputContent` after the six values
/// of `RelatedItemType`
/// (`smithy_query(query="shape",shape="RelatedItemInputContent,RelatedItemType")`),
/// so each entry is the content that one declared type agrees with and that
/// every other one contradicts.
fn content_of_every_kind(
    text_field: &str,
    number_field: &str,
    linked_case_id: &str,
) -> Vec<(&'static str, Value)> {
    associations(text_field, number_field, linked_case_id)
        .into_iter()
        .map(|row| (row.item_type, row.content))
        .collect()
}

/// The refusal a mismatched pair earns: the DECLARED type upper-cased, then the
/// service's fixed sentence, and nothing else — no content kind, no accepted
/// value list, no domain or case id.
fn mismatch_message(declared: &str) -> String {
    format!(
        "{} type was specified with a mismatched related-item content.",
        declared.to_uppercase()
    )
}

#[tokio::test]
async fn every_mismatched_type_and_content_pair_is_refused_with_the_services_fixed_sentence() {
    let emulator = Emulator::new();
    let (domain_id, template_id, case_id, fields) =
        domain_with_a_case(&emulator, "related-item-mismatch").await;
    let text_field = field_of_type(&fields, "Text");
    let number_field = field_of_type(&fields, "Number");
    let boolean_field = field_of_type(&fields, "Boolean");
    let linked_case_id = create_case(&emulator, &domain_id, &template_id, "the linked case").await;
    let path = related_items_path(&domain_id, &case_id);

    let contents = content_of_every_kind(&text_field, &number_field, &linked_case_id);
    // The catalog above is the whole content vocabulary, so the cross product
    // below is every ordered mismatched pair rather than the one recorded pair.
    for item_type in RELATED_ITEM_TYPE_VALUES {
        assert!(
            contents.iter().any(|(kind, _)| kind == item_type),
            "the content catalog must carry content of the {item_type} kind"
        );
    }

    // (case, declared type, content it contradicts)
    let mut rows: Vec<(String, &str, Value)> = Vec::new();
    for declared in RELATED_ITEM_TYPE_VALUES {
        for (kind, content) in &contents {
            if kind == declared {
                continue;
            }
            rows.push((
                format!("{declared} declared with {kind} content"),
                declared,
                content.clone(),
            ));
        }
    }
    // The sentence is the same whatever the contradicting content carries, so
    // two rows vary the content's own detail as well as its kind.
    rows.push((
        "Comment declared with Custom content carrying three values".to_string(),
        "Comment",
        json!({ "custom": { "fields": [
            { "id": text_field, "value": { "stringValue": "first" } },
            { "id": number_field, "value": { "doubleValue": 12.5 } },
            { "id": boolean_field, "value": { "booleanValue": true } },
        ] } }),
    ));
    rows.push((
        "Sla declared with a one-character comment".to_string(),
        "Sla",
        json!({ "comment": { "body": "c", "contentType": "Text/Plain" } }),
    ));

    for (case, declared, content) in &rows {
        let (status, code, message) = emulator
            .failure_detail(
                case,
                "POST",
                &path,
                Some(json!({ "type": declared, "content": content })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code} with \
             message {message:?}"
        );
        let expected = mismatch_message(declared);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // `SearchRelatedItems` is still a scaffolded 501 (its own open stub issue),
    // so the case's audit history is the reader that shows whether anything was
    // associated: a refused request leaves the case with only its own creation
    // recorded.
    let history = audit_events(&emulator, "after the mismatches", &domain_id, &case_id).await;
    assert_eq!(
        history.len(),
        1,
        "case \"after the mismatches\": a refused association stores nothing, got {history:?}"
    );

    // The control: each declared type agrees with the content of its own kind,
    // and is accepted — so the refusals above are about the contradiction and
    // not about the operation or the content.
    for (kind, content) in &contents {
        let case = format!("{kind} declared with {kind} content");
        let created = emulator
            .ok(
                &case,
                "POST",
                &path,
                json!({ "type": kind, "content": content }),
            )
            .await;
        let related_item_id = string_at(&created, "relatedItemId", &case);
        assert!(
            is_minted_id(&related_item_id),
            "case {case:?}: an agreeing pair is stored under a minted id, got {created:?}"
        );
    }
    let history = audit_events(&emulator, "after the agreeing pairs", &domain_id, &case_id).await;
    assert_eq!(
        history.len(),
        contents.len() + 1,
        "case \"after the agreeing pairs\": one recorded association per accepted request, \
         got {history:?}"
    );
}

/// A `Comment` association whose body is `length` characters long.
fn comment_body(length: usize) -> Value {
    json!({
        "type": "Comment",
        "content": { "comment": { "body": "c".repeat(length), "contentType": "Text/Plain" } },
    })
}

/// An `Sla` association whose target is `minutes` minutes away.
fn sla_target(minutes: i64) -> Value {
    json!({
        "type": "Sla",
        "content": { "sla": { "slaInputConfiguration": {
            "name": "a target at a bound",
            "type": "CaseField",
            "targetSlaMinutes": minutes,
        } } },
    })
}
