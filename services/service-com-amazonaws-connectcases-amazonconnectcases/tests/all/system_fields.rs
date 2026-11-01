// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the reserved `System`-namespace fields a
//! Cases domain is provisioned with, for the per-item error code a batch read
//! reports, and for the message a create refused on a taken name renders.
//!
//! The rules under test:
//!
//! 1. A domain's field schema holds the service's reserved `System`-namespace
//!    fields from the moment the domain is created — under their reserved ids,
//!    verbatim and identical in every domain — so every operation that resolves
//!    a field id (`CreateCase`, `UpdateCase`, `GetCase`, `BatchGetField`)
//!    resolves them exactly as it resolves a field `CreateField` made.
//! 2. A batch read partially succeeds: HTTP 200, with each id it cannot resolve
//!    reported in its own `errors` list under the service's per-item error code
//!    (`ResourceNotFound`), not under the name of the modelled exception shape a
//!    whole call would fail with.
//! 3. A create refused because a live resource of the same kind already holds
//!    the requested name answers 409 `ConflictException` with the service's
//!    fixed message template, rendered from the REQUEST's own name and carrying
//!    no server-minted id.

use std::collections::HashMap;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    BATCH_ITEM_NOT_FOUND_CODE, FIELD_NAMESPACE_SYSTEM, RESERVED_SYSTEM_FIELDS,
};

use crate::harness::{
    domain, field_of_type, fields_of_every_type, get_case_fields, is_minted_id, string_at,
    template, Emulator,
};

/// The reserved id whose value every row below leaves unwritten, so a read can
/// tell an in-schema field that was never written (no entry at all) from one
/// that is not in the schema at all (a refusal).
const NEVER_WRITTEN_RESERVED_ID: &str = "reference_number";

/// The value of a `GetCase` entry for `field_id`, which the handler answers in
/// the order the ids were requested.
fn value_of(fields: &[Value], field_id: &str, case: &str) -> Value {
    fields
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(field_id))
        .and_then(|entry| entry.get("value").cloned())
        .unwrap_or_else(|| panic!("case {case:?}: no entry for {field_id:?} in {fields:?}"))
}

#[tokio::test]
async fn every_new_domain_is_provisioned_with_the_reserved_system_fields() {
    // The catalog in `state::RESERVED_SYSTEM_FIELDS` is the table: every
    // reserved id resolves in a domain nothing but `CreateDomain` has touched,
    // in the `System` namespace, under its own declared `FieldType` — including
    // the ids the issue never cited.
    let emulator = Emulator::new();
    let first = domain(&emulator, "system-fields-domain").await;
    let second = domain(&emulator, "another-system-fields-domain").await;

    // Two entries of the catalog pinned as literals, so a wrong catalog entry
    // fails here rather than being carried into the expectations below.
    for (field_id, name, field_type) in [
        ("title", "Title", "Text"),
        ("created_datetime", "Date/time created", "DateTime"),
    ] {
        assert!(
            RESERVED_SYSTEM_FIELDS.contains(&(field_id, name, field_type)),
            "case \"the reserved catalog\": {field_id} must be reserved as a {field_type} field \
             named {name:?}, got {RESERVED_SYSTEM_FIELDS:?}"
        );
    }

    for domain_id in [&first, &second] {
        let case = format!("BatchGetField every reserved id in {domain_id}");
        let requested: Vec<Value> = RESERVED_SYSTEM_FIELDS
            .iter()
            .map(|(field_id, _, _)| json!({ "id": field_id }))
            .collect();
        let response = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/fields-batch"),
                json!({ "fields": requested }),
            )
            .await;
        let errors = response
            .get("errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no errors list in {response:?}"))
            .clone();
        assert!(
            errors.is_empty(),
            "case {case:?}: every reserved id must resolve in a new domain, got {errors:?}"
        );
        let read = response
            .get("fields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no fields list in {response:?}"))
            .clone();
        let by_id: HashMap<String, Value> = read
            .iter()
            .map(|field| (string_at(field, "fieldId", &case), field.clone()))
            .collect();
        assert_eq!(
            by_id.len(),
            RESERVED_SYSTEM_FIELDS.len(),
            "case {case:?}: expected one entry per reserved id, got {read:?}"
        );
        for (field_id, name, field_type) in RESERVED_SYSTEM_FIELDS {
            let field = by_id
                .get(*field_id)
                .unwrap_or_else(|| panic!("case {case:?}: {field_id} missing from {read:?}"));
            assert_eq!(
                string_at(field, "type", &case),
                *field_type,
                "case {case:?}: {field_id} must carry its reserved FieldType, got {field:?}"
            );
            assert_eq!(
                string_at(field, "namespace", &case),
                FIELD_NAMESPACE_SYSTEM,
                "case {case:?}: {field_id} is a service-defined field, got {field:?}"
            );
            assert_eq!(
                string_at(field, "name", &case),
                *name,
                "case {case:?}: a reserved field's name is the service's display name for it, \
                 got {field:?}"
            );
            assert_eq!(
                field.get("deleted").and_then(Value::as_bool),
                Some(false),
                "case {case:?}: {field_id} must be live, got {field:?}"
            );
            assert_eq!(
                string_at(field, "fieldArn", &case),
                format!(
                    "arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/field/{field_id}"
                ),
                "case {case:?}: {field_id} must be addressable in its own domain, got {field:?}"
            );
            assert!(
                !is_minted_id(field_id),
                "case {case:?}: a reserved id is the service's own word, not a minted id, \
                 got {field_id:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_case_is_written_and_read_through_the_reserved_system_fields() {
    // A reserved field is in the domain's schema, so a case writes and reads it
    // like any other field — including the type check its own `FieldType`
    // carries — and one that was never written is left out of the read.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-on-system-fields").await;
    let custom = emulator
        .ok(
            "CreateField beside the reserved ones",
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": "a-custom-note", "type": "Text" }),
        )
        .await;
    let custom_id = string_at(&custom, "fieldId", "CreateField beside the reserved ones");
    let template_id = template(
        &emulator,
        &domain_id,
        "system-fields-template",
        std::slice::from_ref(&custom_id),
    )
    .await;

    // One row per reserved field this case writes, each in the union variant
    // its catalogued `FieldType` takes: the string-valued types in
    // `stringValue`, a `User` field in `userArnValue`, and `emptyValue` for a
    // field written with the union's absent value.
    let written: Vec<(&str, Value)> = vec![
        ("title", json!({ "stringValue": "a lost order" })),
        (
            "customer_id",
            json!({ "stringValue": "arn:aws:profile:us-east-1:000000000000:domains/d-1/profiles/p-1" }),
        ),
        ("status", json!({ "stringValue": "open" })),
        (
            "summary",
            json!({ "stringValue": "the customer never received the order" }),
        ),
        (
            "created_datetime",
            json!({ "stringValue": "2026-10-22T14:45:10.000Z" }),
        ),
        (
            "assigned_queue",
            json!({ "stringValue": "arn:aws:connect:us-east-1:000000000000:instance/i-1/queue/q-1" }),
        ),
        (
            "assigned_user",
            json!({ "userArnValue": "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1" }),
        ),
        ("case_reason", json!({ "emptyValue": {} })),
    ];

    let mut body_fields: Vec<Value> = written
        .iter()
        .map(|(field_id, value)| json!({ "id": field_id, "value": value }))
        .collect();
    body_fields.push(json!({
        "id": custom_id,
        "value": { "stringValue": "written beside the reserved fields" },
    }));
    let created = emulator
        .ok(
            "CreateCase over the reserved fields",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({ "templateId": template_id, "fields": body_fields }),
        )
        .await;
    let case_id = string_at(&created, "caseId", "CreateCase over the reserved fields");
    assert!(
        is_minted_id(&case_id),
        "case \"CreateCase over the reserved fields\": the caseId must be a minted id, \
         got {case_id:?}"
    );
    assert_eq!(
        string_at(&created, "caseArn", "CreateCase over the reserved fields"),
        format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/case/{case_id}"),
        "case \"CreateCase over the reserved fields\": the caseArn must address the stored case, \
         got {created:?}"
    );

    let mut requested: Vec<String> = written
        .iter()
        .map(|(field_id, _)| (*field_id).to_string())
        .collect();
    requested.push(custom_id.clone());
    requested.push(NEVER_WRITTEN_RESERVED_ID.to_string());
    let read = get_case_fields(
        &emulator,
        "GetCase over the reserved fields",
        &domain_id,
        &case_id,
        &requested,
    )
    .await;
    for (field_id, value) in &written {
        assert_eq!(
            &value_of(&read, field_id, "GetCase over the reserved fields"),
            value,
            "case \"GetCase over the reserved fields\": {field_id} must read back what was \
             written, got {read:?}"
        );
    }
    assert!(
        !read
            .iter()
            .any(|entry| entry.get("id").and_then(Value::as_str) == Some(NEVER_WRITTEN_RESERVED_ID)),
        "case \"GetCase unwritten reserved field\": a reserved field the case never wrote holds \
         no value, so it must not be reported, got {read:?}"
    );

    // An update writes a reserved field the same way, and the read reports the
    // new value while the rest of the case is untouched.
    emulator
        .ok(
            "UpdateCase a reserved field",
            "PUT",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            json!({ "fields": [{ "id": "title", "value": { "stringValue": "a lost order, reopened" } }] }),
        )
        .await;
    let after = get_case_fields(
        &emulator,
        "GetCase after the reserved-field update",
        &domain_id,
        &case_id,
        &["title".to_string(), "summary".to_string()],
    )
    .await;
    assert_eq!(
        value_of(&after, "title", "GetCase after the reserved-field update"),
        json!({ "stringValue": "a lost order, reopened" }),
        "case \"GetCase after the reserved-field update\": the update must be reported, \
         got {after:?}"
    );
    assert_eq!(
        value_of(&after, "summary", "GetCase after the reserved-field update"),
        json!({ "stringValue": "the customer never received the order" }),
        "case \"GetCase after the reserved-field update\": an unwritten field must be left \
         alone, got {after:?}"
    );

    // The reserved field's own declared type is enforced, so a value that is
    // not a value of that type is refused rather than stored.
    for (field_id, field_type, bad_value) in [
        ("title", "Text", json!({ "booleanValue": true })),
        ("assigned_user", "User", json!({ "stringValue": "not a user arn" })),
        ("case_reason", "SingleSelect", json!({ "doubleValue": 1.5 })),
    ] {
        let case = format!("UpdateCase {field_id} with a non-{field_type} value");
        let (status, code, message) = emulator
            .failure_detail(
                &case,
                "PUT",
                &format!("/domains/{domain_id}/cases/{case_id}"),
                Some(json!({ "fields": [{ "id": field_id, "value": bad_value }] })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
        );
        assert!(
            message.contains(field_id) && message.contains(field_type),
            "case {case:?}: the refusal must name the field and its type, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_read_reports_an_unresolvable_id_under_the_per_item_error_code() {
    // The whole call succeeds (HTTP 200) and each id it could not resolve is
    // reported in `errors` under the per-item code, whatever the FORM of the id
    // and however many of them a request mixes with resolvable ones.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "batch-per-item-errors").await;
    let created = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&created, "Text");
    let number = field_of_type(&created, "Number");

    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one unresolvable id alone",
            vec![],
            vec!["no-such-field".to_string()],
        ),
        (
            "a resolvable id and an unresolvable one",
            vec![text.clone()],
            vec!["sim2real-nonexistent-field-id-0000000000".to_string()],
        ),
        (
            "an id one character long, the modelled FieldId minimum",
            vec![text.clone(), number.clone()],
            vec!["a".to_string()],
        ),
        (
            "an id at the modelled FieldId maximum of 500",
            vec![number.clone()],
            vec!["x".repeat(500)],
        ),
        (
            "an id that only looks minted",
            vec![],
            vec!["00000000-0000-4000-8000-000000000000".to_string()],
        ),
        (
            "ids that differ from a reserved word only in case or spelling",
            vec![text.clone()],
            vec![
                "Title".to_string(),
                "titles".to_string(),
                "customer-id".to_string(),
            ],
        ),
        (
            "a reserved id of another domain's schema is still in this one",
            vec!["title".to_string(), "customer_id".to_string()],
            vec!["not_a_reserved_id".to_string()],
        ),
    ];

    for (case, resolvable, unresolvable) in rows {
        // The unresolvable ids go FIRST, so the partition cannot be an artefact
        // of the order the request listed them in.
        let requested: Vec<Value> = unresolvable
            .iter()
            .chain(resolvable.iter())
            .map(|id| json!({ "id": id }))
            .collect();
        let response = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/fields-batch"),
                json!({ "fields": requested }),
            )
            .await;
        let fields = response
            .get("fields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no fields list in {response:?}"))
            .clone();
        let errors = response
            .get("errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no errors list in {response:?}"))
            .clone();
        assert_eq!(
            fields.len(),
            resolvable.len(),
            "case {case:?}: expected {} resolved field(s), got {fields:?}",
            resolvable.len()
        );
        assert_eq!(
            errors.len(),
            unresolvable.len(),
            "case {case:?}: expected {} per-item error(s), got {errors:?}",
            unresolvable.len()
        );
        for (index, id) in unresolvable.iter().enumerate() {
            let error = &errors[index];
            assert_eq!(
                &string_at(error, "id", case),
                id,
                "case {case:?}: the error must name the id that failed, got {error:?}"
            );
            let code = string_at(error, "errorCode", case);
            assert_eq!(
                code, BATCH_ITEM_NOT_FOUND_CODE,
                "case {case:?}: expected the per-item code {BATCH_ITEM_NOT_FOUND_CODE:?}, \
                 got {code:?}"
            );
            assert_ne!(
                code, "ResourceNotFoundException",
                "case {case:?}: the per-item code must not be the modelled exception shape's \
                 name, got {error:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_field_name_already_taken_is_refused_with_the_services_message_template() {
    // Whatever the form of the requested name, a loser's message is the
    // service's fixed template filled from the name the REQUEST carried, and
    // only one create wins the name.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "duplicate-field-names").await;
    let path = format!("/domains/{domain_id}/fields");

    // Rows differ along the form of the name; the last one is a name the domain
    // already holds because it is a reserved System field, so nothing creates it
    // first.
    let rows: Vec<(&str, String, bool)> = vec![
        ("an ascii name", "customer-note".to_string(), false),
        ("a name with spaces", "Customer note two".to_string(), false),
        ("a name outside ascii", "título del caso".to_string(), false),
        (
            "a name at the modelled 100-character maximum",
            "n".repeat(100),
            false,
        ),
        (
            "a reserved System field's own name",
            "Title".to_string(),
            true,
        ),
    ];

    for (form, name, already_held) in rows {
        let case = format!("CreateField with {form}");
        let winner = if already_held {
            None
        } else {
            let created = emulator
                .ok(&case, "POST", &path, json!({ "name": name, "type": "Text" }))
                .await;
            let field_id = string_at(&created, "fieldId", &case);
            assert!(
                is_minted_id(&field_id),
                "case {case:?}: the winner's fieldId must be a minted id, got {field_id:?}"
            );
            Some(field_id)
        };
        let expected =
            format!("Field creation failed: field resource with the name {name} already exists!");
        for attempt in 0..3 {
            let conflict = format!("{case}, loser #{attempt}");
            let (status, code, message) = emulator
                .failure_detail(
                    &conflict,
                    "POST",
                    &path,
                    Some(json!({ "name": name, "type": "Text" })),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (409, "ConflictException"),
                "case {conflict:?}: expected 409 ConflictException, got {status} {code}: {message}"
            );
            assert_eq!(
                message, expected,
                "case {conflict:?}: expected the service's template {expected:?}, got {message:?}"
            );
            if let Some(winner) = winner.as_deref() {
                assert!(
                    !message.contains(winner),
                    "case {conflict:?}: the message must carry no server-minted id the loser \
                     never saw, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_layout_may_not_place_a_reserved_field_that_is_not_configurable() {
    // The model states this for the layout operations in particular: "Title and
    // Status fields cannot be part of layouts since they are not configurable"
    // (`smithy_query(query="doc",shape="CreateLayout")`). The domain's schema
    // DOES hold those reserved fields now, so the refusal is its own check
    // rather than a side effect of the id not resolving — while every other
    // reserved field is placeable like a Custom one.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-reserved-fields").await;
    let content = |ids: &[&str]| {
        let fields: Vec<Value> = ids.iter().map(|id| json!({ "id": id })).collect();
        json!({ "basic": { "topPanel": { "sections": [{ "fieldGroup": { "fields": fields } }] } } })
    };

    // Rows differ in which non-configurable id is placed, where in the list it
    // sits, which panel holds it, and how many of them a request carries.
    for (case, placed, refused_ids) in [
        ("title alone", vec!["title"], vec!["title"]),
        ("status alone", vec!["status"], vec!["status"]),
        (
            "a non-configurable id after a placeable one",
            vec!["summary", "status"],
            vec!["status"],
        ),
        (
            "both non-configurable ids, title first",
            vec!["title", "summary", "status"],
            vec!["title", "status"],
        ),
    ] {
        let (status, code, message) = emulator
            .failure_detail(
                case,
                "POST",
                &format!("/domains/{domain_id}/layouts"),
                Some(json!({ "name": format!("layout-{case}"), "content": content(&placed) })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
        );
        for refused in &refused_ids {
            assert!(
                message.contains(refused),
                "case {case:?}: the refusal must name {refused}, got {message:?}"
            );
        }
    }

    // Every other reserved field is a field of the domain like any other, so a
    // layout may place it, and the stored layout reads back with it.
    let placeable = ["summary", "customer_id", "assigned_user", "case_reason"];
    let created = emulator
        .ok(
            "a layout over the placeable reserved fields",
            "POST",
            &format!("/domains/{domain_id}/layouts"),
            json!({ "name": "placeable-reserved", "content": content(&placeable) }),
        )
        .await;
    let layout_id = string_at(&created, "layoutId", "a layout over the placeable reserved fields");
    let fetched = emulator
        .ok(
            "GetLayout over the placeable reserved fields",
            "POST",
            &format!("/domains/{domain_id}/layouts/{layout_id}"),
            json!({}),
        )
        .await;
    let listed = fetched
        .get("content")
        .and_then(|content| content.get("basic"))
        .and_then(|basic| basic.get("topPanel"))
        .and_then(|panel| panel.get("sections"))
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("fieldGroup"))
        .and_then(|group| group.get("fields"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case \"GetLayout over the placeable reserved fields\": no fields in {fetched:?}")
        })
        .clone();
    let ids: Vec<String> = listed
        .iter()
        .map(|field| string_at(field, "id", "GetLayout over the placeable reserved fields"))
        .collect();
    assert_eq!(
        ids,
        placeable.map(str::to_string).to_vec(),
        "case \"GetLayout over the placeable reserved fields\": the stored layout must report \
         the reserved fields it placed, in order, got {fetched:?}"
    );

    // And the same refusal guards the update path, so a layout cannot be
    // rewritten into one the create path would have refused.
    let (status, code, message) = emulator
        .failure_detail(
            "UpdateLayout placing a non-configurable id",
            "PUT",
            &format!("/domains/{domain_id}/layouts/{layout_id}"),
            Some(json!({ "content": content(&["title"]) })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"UpdateLayout placing a non-configurable id\": expected 400 ValidationException, \
         got {status} {code}: {message}"
    );
}

#[tokio::test]
async fn a_name_conflict_renders_the_same_template_for_every_container() {
    // One rule, one template: the kind of resource is the only thing that
    // differs between a domain, a field and a template refused on a taken name.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "conflict-template-domain").await;

    let rows: Vec<(&str, String, Value, &str)> = vec![
        (
            "domain",
            "/domains".to_string(),
            json!({ "name": "conflict-template-domain" }),
            "Domain creation failed: domain resource with the name conflict-template-domain \
             already exists!",
        ),
        (
            "field",
            format!("/domains/{domain_id}/fields"),
            json!({ "name": "conflicting-name", "type": "Text" }),
            "Field creation failed: field resource with the name conflicting-name already exists!",
        ),
        (
            "template",
            format!("/domains/{domain_id}/templates"),
            json!({ "name": "conflicting-name" }),
            "Template creation failed: template resource with the name conflicting-name \
             already exists!",
        ),
        (
            "layout",
            format!("/domains/{domain_id}/layouts"),
            json!({ "name": "conflicting-name", "content": { "basic": {} } }),
            "Layout creation failed: layout resource with the name conflicting-name \
             already exists!",
        ),
    ];

    for (kind, path, body, expected) in rows {
        let case = format!("Create a second {kind} under a taken name");
        if kind != "domain" {
            emulator
                .ok(&format!("Create the first {kind}"), "POST", &path, body.clone())
                .await;
        }
        let (status, code, message) = emulator
            .failure_detail(&case, "POST", &path, Some(body.clone()))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {code}: {message}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the service's template {expected:?}, got {message:?}"
        );
    }

    // A RENAME onto a name another live resource of the same kind holds is the
    // same clash, so it renders the same sentence — and still carries no
    // server-minted id.
    let taken = "the-name-that-is-taken";
    for (kind, collection, body, expected) in [
        (
            "template",
            "templates",
            json!({ "name": taken }),
            "Template creation failed: template resource with the name the-name-that-is-taken \
             already exists!",
        ),
        (
            "layout",
            "layouts",
            json!({ "name": taken, "content": { "basic": {} } }),
            "Layout creation failed: layout resource with the name the-name-that-is-taken \
             already exists!",
        ),
    ] {
        let holder = emulator
            .ok(
                &format!("the {kind} that holds the name"),
                "POST",
                &format!("/domains/{domain_id}/{collection}"),
                body.clone(),
            )
            .await;
        let mut renamed_body = body.clone();
        renamed_body["name"] = json!(format!("the-{kind}-to-rename"));
        let renamed = emulator
            .ok(
                &format!("the {kind} to rename"),
                "POST",
                &format!("/domains/{domain_id}/{collection}"),
                renamed_body,
            )
            .await;
        let id_member = format!("{kind}Id");
        let holder_id = string_at(&holder, &id_member, "the holder");
        let renamed_id = string_at(&renamed, &id_member, "the one to rename");

        let case = format!("Rename a {kind} onto a taken name");
        let (status, code, message) = emulator
            .failure_detail(
                &case,
                "PUT",
                &format!("/domains/{domain_id}/{collection}/{renamed_id}"),
                Some(json!({ "name": taken })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {code}: {message}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the service's template {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(&holder_id),
            "case {case:?}: the message must carry no server-minted id, got {message:?}"
        );

        // Renaming a resource to the name it already has is not a clash with
        // itself, so the same call against the holder succeeds.
        emulator
            .ok(
                &format!("Rename the {kind} to the name it already has"),
                "PUT",
                &format!("/domains/{domain_id}/{collection}/{holder_id}"),
                json!({ "name": taken }),
            )
            .await;
    }
}
