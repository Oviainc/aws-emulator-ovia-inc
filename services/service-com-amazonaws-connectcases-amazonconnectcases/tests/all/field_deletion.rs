// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `DeleteField`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: deleting a field is a SOFT delete of the one record the
//! request names — the field stays retrievable through `BatchGetField`
//! reporting `deleted` true with its id, ARN, name, type, namespace and
//! `createdTime` untouched, and the values cases already hold for it stay
//! readable through `GetCase`, searchable through `SearchCases` and rewritable
//! through `UpdateCase`, while every view of the domain's LIVE schema leaves it
//! out (`ListFields`, and the field ids a template, a layout and the case event
//! configuration report) and a request that takes out a NEW reference to it —
//! `CreateCase`, an `UpdateCase` of a case holding no value for it,
//! `BatchPutFieldOptions` — is refused with `ValidationException` (400). No
//! other field of the domain is affected; the call is `@idempotent`, so
//! repeating it succeeds and changes nothing; and an id the store holds no
//! record for is refused with `ResourceNotFoundException` (404) having written
//! nothing.
//!
//! That is this operation's own documentation: "After a field is deleted: You
//! can still retrieve the field by calling BatchGetField ... Deleted fields are
//! not included in the ListFields response. Calling CreateCase with a deleted
//! field throws a ValidationException denoting which field identifiers in the
//! request have been deleted. Calling GetCase with a deleted field identifier
//! returns the deleted field's value if one exists. Calling UpdateCase with a
//! deleted field ID throws a ValidationException if the case does not already
//! contain a value for the deleted field. Otherwise it succeeds, allowing you
//! to update or remove (using emptyValue: {}) the field's value from the case.
//! GetTemplate does not return field IDs for deleted fields. GetLayout does not
//! return field IDs for deleted fields. Calling SearchCases with the deleted
//! field ID as a filter returns any cases that have a value for the deleted
//! field that matches the filter criteria ... Calling BatchPutFieldOptions with
//! a deleted field ID throw a ValidationException. Calling
//! GetCaseEventConfiguration does not return field IDs for deleted fields"
//! (`smithy_query(query="doc", shape="DeleteField")`) — and `@idempotent` is on
//! the operation itself
//! (`smithy_query(query="operation", operation="DeleteField")`).
//!
//! The rows below span what the rule generalises over: which `FieldType` the
//! deleted field has, which union variant the value written against it holds,
//! how many times the delete is repeated, which of a domain's several fields is
//! deleted, which list a deleted field has to drop out of, and which reference
//! a refused delete cannot resolve.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::harness::{
    domain, field_value_variants, fields_of_every_type, get_case_fields, string_at, template,
    Emulator,
};
use crate::layout_lifecycle::{create_layout, get_layout, top_panel, top_panel_reported};

/// `DeleteField`: `DELETE /domains/{domainId}/fields/{fieldId}`, which the
/// model gives no body members and no output members.
async fn delete_field(emulator: &Emulator, case: &str, domain_id: &str, field_id: &str) -> Value {
    emulator
        .ok(
            case,
            "DELETE",
            &format!("/domains/{domain_id}/fields/{field_id}"),
            json!({}),
        )
        .await
}

/// `BatchGetField` over `ids`, as its `(fields, errors)` lists.
async fn batch_get(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    ids: &[String],
) -> (Vec<Value>, Vec<Value>) {
    let requested: Vec<Value> = ids.iter().map(|id| json!({ "id": id })).collect();
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
        .unwrap_or_else(|| panic!("case {case:?}: BatchGetField reported no fields: {response:?}"))
        .clone();
    let errors = response
        .get("errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (fields, errors)
}

/// The one field `BatchGetField` reports for `field_id`.
async fn read_field(emulator: &Emulator, case: &str, domain_id: &str, field_id: &str) -> Value {
    let ids = vec![field_id.to_string()];
    let (fields, errors) = batch_get(emulator, case, domain_id, &ids).await;
    assert!(
        errors.is_empty(),
        "case {case:?}: BatchGetField must still retrieve {field_id}, got errors {errors:?}"
    );
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: expected exactly one field back for {field_id}, got {fields:?}"
    );
    fields[0].clone()
}

/// The field ids `ListFields` reports for `domain_id`, following its pages to
/// exhaustion so the answer is every live field and not only the first page.
async fn listed_field_ids(emulator: &Emulator, case: &str, domain_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(token) => format!("?nextToken={token}"),
            None => String::new(),
        };
        let answer = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/fields-list{query}"),
                json!({}),
            )
            .await;
        let page = answer
            .get("fields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: ListFields reported no fields: {answer:?}"))
            .clone();
        ids.extend(page.iter().map(|field| string_at(field, "fieldId", case)));
        token = answer
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return ids;
        }
    }
}

/// `CreateField`, returning the id it minted.
async fn create_field(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    name: &str,
    field_type: &str,
) -> String {
    let created = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": name, "type": field_type }),
        )
        .await;
    string_at(&created, "fieldId", case)
}

/// `CreateCase` against `template_id` writing `fields`, returning the case id.
async fn create_case(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    template_id: &str,
    fields: Vec<Value>,
) -> String {
    let created = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({ "templateId": template_id, "fields": fields }),
        )
        .await;
    string_at(&created, "caseId", case)
}

/// One refusal row: the case name, the method and path to call, and the status
/// and wire error code the model declares for that refusal.
type RefusalRow = (&'static str, &'static str, String, u16, &'static str);

#[tokio::test]
async fn a_deleted_field_is_still_retrieved_whole_for_every_field_type() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-type-domain").await;
    // One row per `FieldType` the model declares: whichever type the field has,
    // the delete flags the one record and keeps the rest of it as stored.
    let created = fields_of_every_type(&emulator, &domain_id).await;
    let every_id: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();

    for (index, (field_type, field_id)) in created.iter().enumerate() {
        let case = format!("DeleteField of a {field_type} field");
        let case = case.as_str();

        let before = read_field(&emulator, case, &domain_id, field_id).await;
        assert_eq!(
            before.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a field that was never deleted reports deleted=false, got {before:?}"
        );
        assert!(
            listed_field_ids(&emulator, case, &domain_id)
                .await
                .contains(field_id),
            "case {case:?}: a live field must be listed by ListFields"
        );

        let deleted = delete_field(&emulator, case, &domain_id, field_id).await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: DeleteField answers HTTP 200 with no output members, got {deleted:?}"
        );

        let after = read_field(&emulator, case, &domain_id, field_id).await;
        assert_eq!(
            after.get("deleted"),
            Some(&json!(true)),
            "case {case:?}: a deleted field must report deleted=true, got {after:?}"
        );
        for member in ["fieldId", "fieldArn", "name", "type", "namespace"] {
            assert_eq!(
                string_at(&after, member, case),
                string_at(&before, member, case),
                "case {case:?}: a delete must not rewrite {member}, got {after:?}"
            );
        }
        assert_eq!(
            string_at(&after, "type", case),
            *field_type,
            "case {case:?}: the deleted field keeps its declared type, got {after:?}"
        );
        assert_eq!(
            after.get("createdTime"),
            before.get("createdTime"),
            "case {case:?}: a delete never moves createdTime, got {after:?}"
        );
        assert!(
            after.get("lastModifiedTime").is_some(),
            "case {case:?}: lastModifiedTime must still be reported, got {after:?}"
        );

        // A read reports; two reads of a deleted field answer the same body.
        let again = read_field(&emulator, case, &domain_id, field_id).await;
        assert_eq!(
            again, after,
            "case {case:?}: two BatchGetField calls must answer the same body, got {again:?}"
        );

        // The listing reports exactly the fields not yet deleted, in the live
        // schema the deleted ones have left.
        let listed = listed_field_ids(&emulator, case, &domain_id).await;
        for (position, other) in every_id.iter().enumerate() {
            let expected = position > index;
            assert_eq!(
                listed.contains(other),
                expected,
                "case {case:?}: after deleting field {index}, ListFields listing field \
                 {position} must be {expected}, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn repeating_the_delete_succeeds_and_changes_nothing() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-idempotent-domain").await;

    // A row per number of extra deletes after the first one: the operation is
    // `@idempotent`, so the record a reader sees is the record the FIRST delete
    // left, whatever the repeat count.
    for extra in [0_usize, 1, 2, 5] {
        let case = format!("{} delete(s) of one field", extra + 1);
        let case = case.as_str();
        let field_id = create_field(
            &emulator,
            case,
            &domain_id,
            &format!("idempotent-field-{extra}"),
            "Text",
        )
        .await;

        delete_field(&emulator, case, &domain_id, &field_id).await;
        let after_first = read_field(&emulator, case, &domain_id, &field_id).await;
        for repeat in 0..extra {
            let repeated = delete_field(&emulator, case, &domain_id, &field_id).await;
            assert_eq!(
                repeated,
                json!({}),
                "case {case:?}: repeat {repeat} must also answer HTTP 200 with no output \
                 members, got {repeated:?}"
            );
        }

        let after_repeats = read_field(&emulator, case, &domain_id, &field_id).await;
        assert_eq!(
            after_repeats, after_first,
            "case {case:?}: a repeat delete must leave the record exactly as the first \
             delete left it ({after_first:?}), got {after_repeats:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_field_it_names() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-siblings-domain").await;
    let other_domain = domain(&emulator, "field-delete-other-domain").await;

    // Three sibling fields of one domain, plus one of ANOTHER domain holding
    // the same name: a row per which sibling is deleted, so the position of the
    // target among its siblings cannot matter.
    for target in 0..3_usize {
        let case = format!("a delete of sibling {target} among three sibling fields");
        let case = case.as_str();
        let mut siblings = Vec::new();
        for index in 0..3 {
            siblings.push(
                create_field(
                    &emulator,
                    case,
                    &domain_id,
                    &format!("sibling-{target}-{index}"),
                    "Text",
                )
                .await,
            );
        }
        let elsewhere = create_field(
            &emulator,
            case,
            &other_domain,
            &format!("sibling-{target}-{target}"),
            "Text",
        )
        .await;

        delete_field(&emulator, case, &domain_id, &siblings[target]).await;

        for (index, field_id) in siblings.iter().enumerate() {
            let fetched = read_field(&emulator, case, &domain_id, field_id).await;
            let expected = index == target;
            assert_eq!(
                fetched.get("deleted"),
                Some(&json!(expected)),
                "case {case:?}: deleting sibling {target} must leave sibling {index} \
                 deleted={expected}, got {fetched:?}"
            );
        }
        // The same-named field of another domain is a different resource.
        let fetched = read_field(&emulator, case, &other_domain, &elsewhere).await;
        assert_eq!(
            fetched.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a delete in one domain must not touch a same-named field of \
             another domain, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_of_an_id_the_store_does_not_hold_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-refusal-domain").await;
    let other_domain = domain(&emulator, "field-delete-refusal-other-domain").await;
    let field_id = create_field(
        &emulator,
        "CreateField for the refusal rows",
        &domain_id,
        "the-only-field",
        "Text",
    )
    .await;

    // A row per reference the store cannot resolve: the domain, the field, and
    // a field of a DIFFERENT domain (a field is scoped to its domain).
    let rows: Vec<RefusalRow> = vec![
        (
            "DeleteField in a domain the store does not hold",
            "DELETE",
            format!("/domains/no-such-domain/fields/{field_id}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteField for a field the domain does not hold",
            "DELETE",
            format!("/domains/{domain_id}/fields/no-such-field"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteField for a field of another domain",
            "DELETE",
            format!("/domains/{other_domain}/fields/{field_id}"),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, method, path, status, code) in rows {
        let (actual_status, actual_code) = emulator
            .failure(case, method, &path, Some(json!({})))
            .await;
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {actual_status} {actual_code}"
        );
    }

    // Every refusal wrote nothing: the one stored field is still live.
    let case = "BatchGetField after the refused deletes";
    let still_live = read_field(&emulator, case, &domain_id, &field_id).await;
    assert_eq!(
        still_live.get("deleted"),
        Some(&json!(false)),
        "case {case:?}: a refused delete must not flag the stored field, got {still_live:?}"
    );
}

#[tokio::test]
async fn deleting_a_field_frees_its_name_for_a_new_one() {
    // A field's name identifies it within its domain, so a create under a name
    // a LIVE field holds is refused — and a deleted field is no longer one of
    // the domain's live fields, so the name it held is available again while
    // the new field is its own record.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-name-domain").await;
    let case = "CreateField under the name of a deleted field";

    let first = create_field(&emulator, case, &domain_id, "recycled-name", "Text").await;
    let (status, code) = emulator
        .failure(
            case,
            "POST",
            &format!("/domains/{domain_id}/fields"),
            Some(json!({ "name": "recycled-name", "type": "Text" })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: while the first field is live its name is taken, got {status} {code}"
    );

    delete_field(&emulator, case, &domain_id, &first).await;
    let second = create_field(&emulator, case, &domain_id, "recycled-name", "Number").await;
    assert_ne!(
        second, first,
        "case {case:?}: the new field must be its own record, got the deleted id {first:?}"
    );
    let reused = read_field(&emulator, case, &domain_id, &second).await;
    assert_eq!(
        reused.get("deleted"),
        Some(&json!(false)),
        "case {case:?}: the new field is live, got {reused:?}"
    );
    let old = read_field(&emulator, case, &domain_id, &first).await;
    assert_eq!(
        old.get("deleted"),
        Some(&json!(true)),
        "case {case:?}: the deleted field stays deleted, got {old:?}"
    );
}

#[tokio::test]
async fn the_case_values_of_a_deleted_field_stay_readable_and_rewritable() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-case-values-domain").await;
    let created = fields_of_every_type(&emulator, &domain_id).await;
    let template_id = template(&emulator, &domain_id, "values-template", &[]).await;
    let by_type: HashMap<&str, String> = created
        .iter()
        .map(|(field_type, id)| (*field_type, id.clone()))
        .collect();

    // A row per union variant a case value can hold, each against a field of
    // the `FieldType` whose values that variant carries: whichever variant the
    // stored value is, deleting the field leaves the value exactly where it was
    // and still rewritable, while a case holding NO value for the deleted field
    // refuses the write.
    for (variant, field_type, value) in field_value_variants() {
        let case = format!("a {variant} written against a {field_type} field");
        let case = case.as_str();
        let field_id = by_type
            .get(field_type)
            .unwrap_or_else(|| panic!("case {case:?}: no {field_type} field was created"))
            .clone();
        let ids = vec![field_id.clone()];

        let holder = create_case(
            &emulator,
            case,
            &domain_id,
            &template_id,
            vec![json!({ "id": field_id, "value": value })],
        )
        .await;
        let bare = create_case(&emulator, case, &domain_id, &template_id, vec![]).await;

        delete_field(&emulator, case, &domain_id, &field_id).await;

        // "Calling GetCase with a deleted field identifier returns the deleted
        // field's value if one exists": the stored value, in the variant it was
        // written with.
        let read_back = get_case_fields(&emulator, case, &domain_id, &holder, &ids).await;
        assert_eq!(
            read_back,
            vec![json!({ "id": field_id, "value": value })],
            "case {case:?}: GetCase must still answer the deleted field's stored value, \
             got {read_back:?}"
        );
        // A case that holds no value for the field is answered by the absence
        // of an entry, exactly as for a live field.
        let empty = get_case_fields(&emulator, case, &domain_id, &bare, &ids).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: a case holding no value for the deleted field reports none, \
             got {empty:?}"
        );

        // "Otherwise it succeeds, allowing you to update or remove (using
        // emptyValue: {}) the field's value from the case."
        emulator
            .ok(
                case,
                "PUT",
                &format!("/domains/{domain_id}/cases/{holder}"),
                json!({ "fields": [{ "id": field_id, "value": { "emptyValue": {} } }] }),
            )
            .await;
        let cleared = get_case_fields(&emulator, case, &domain_id, &holder, &ids).await;
        assert_eq!(
            cleared,
            vec![json!({ "id": field_id, "value": { "emptyValue": {} } })],
            "case {case:?}: the deleted field's value must be the one just written, \
             got {cleared:?}"
        );

        // "Calling UpdateCase with a deleted field ID throws a
        // ValidationException if the case does not already contain a value for
        // the deleted field."
        let (status, code, message) = emulator
            .failure_detail(
                case,
                "PUT",
                &format!("/domains/{domain_id}/cases/{bare}"),
                Some(json!({ "fields": [{ "id": field_id, "value": value }] })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException for an update of a case \
             holding no value for the deleted field, got {status} {code} {message}"
        );
        assert!(
            message.contains(&field_id),
            "case {case:?}: the refusal must name the deleted field id {field_id}, \
             got {message:?}"
        );
        let still_empty = get_case_fields(&emulator, case, &domain_id, &bare, &ids).await;
        assert!(
            still_empty.is_empty(),
            "case {case:?}: the refused update must have written nothing, got {still_empty:?}"
        );

        // "Calling CreateCase with a deleted field throws a
        // ValidationException denoting which field identifiers in the request
        // have been deleted."
        let (status, code, message) = emulator
            .failure_detail(
                case,
                "POST",
                &format!("/domains/{domain_id}/cases"),
                Some(json!({
                    "templateId": template_id,
                    "fields": [{ "id": field_id, "value": value }],
                })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException for a create against the \
             deleted field, got {status} {code} {message}"
        );
        assert!(
            message.contains(&field_id),
            "case {case:?}: the refusal must denote the deleted field id {field_id}, \
             got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_field_is_still_a_filter_and_a_projection_a_search_honours() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-search-domain").await;
    let field_id = create_field(
        &emulator,
        "CreateField for the search rows",
        &domain_id,
        "searchable",
        "Text",
    )
    .await;
    let template_id = template(&emulator, &domain_id, "search-template", &[]).await;
    let matching = create_case(
        &emulator,
        "the case the filter admits",
        &domain_id,
        &template_id,
        vec![json!({ "id": field_id, "value": { "stringValue": "Printer jam" } })],
    )
    .await;
    let other = create_case(
        &emulator,
        "the case the filter rejects",
        &domain_id,
        &template_id,
        vec![json!({ "id": field_id, "value": { "stringValue": "Network outage" } })],
    )
    .await;

    delete_field(&emulator, "DeleteField before the searches", &domain_id, &field_id).await;

    // A row per search criterion the deleted field id can appear in: the
    // documented filter, a `searchTerm` matching its stored value, and the same
    // id as the projected field and as the sort key.
    let rows: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "equalTo on the deleted field",
            json!({ "filter": { "field": { "equalTo": { "id": field_id, "value": { "stringValue": "Printer jam" } } } } }),
            vec![&matching],
        ),
        (
            "contains on the deleted field",
            json!({ "filter": { "field": { "contains": { "id": field_id, "value": { "stringValue": "jam" } } } } }),
            vec![&matching],
        ),
        (
            "a searchTerm matching the deleted field's value",
            json!({ "searchTerm": "outage" }),
            vec![&other],
        ),
        (
            "the deleted field as the sort key",
            json!({ "sorts": [{ "fieldId": field_id, "sortOrder": "Asc" }] }),
            vec![&other, &matching],
        ),
    ];

    for (case, mut body, expected) in rows {
        // Every row projects the deleted field, so its value comes back too.
        body["fields"] = json!([{ "id": field_id }]);
        let answer = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/cases-search"),
                body,
            )
            .await;
        let found: Vec<String> = answer
            .get("cases")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no cases list in {answer:?}"))
            .iter()
            .map(|item| string_at(item, "caseId", case))
            .collect();
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the cases {expected:?} the deleted field's values \
             select, got {found:?}"
        );
        let projected: Vec<Value> = answer
            .get("cases")
            .and_then(Value::as_array)
            .map(|cases| {
                cases
                    .iter()
                    .filter_map(|item| item.get("fields").cloned())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            projected
                .iter()
                .all(|fields| fields.as_array().is_some_and(|list| list.len() == 1)),
            "case {case:?}: the deleted field must still be projected, got {projected:?}"
        );
    }
}

#[tokio::test]
async fn the_field_lists_a_template_a_layout_and_the_event_configuration_report_drop_a_deleted_field()
{
    // A row per which of three fields is deleted: whichever one it is, the
    // lists these reads report leave exactly that id out and keep the rest in
    // the order they were written.
    for target in 0..3_usize {
        let emulator = Emulator::new();
        let case = format!("the field at position {target} of three deleted");
        let case = case.as_str();
        let domain_id = domain(&emulator, &format!("field-delete-lists-domain-{target}")).await;
        let mut fields = Vec::new();
        for index in 0..3 {
            fields.push(
                create_field(&emulator, case, &domain_id, &format!("listed-{index}"), "Text")
                    .await,
            );
        }
        let remaining: Vec<String> = fields
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != target)
            .map(|(_, id)| id.clone())
            .collect();

        let template_id = template(&emulator, &domain_id, "listing-template", &fields).await;
        let (layout_id, _) =
            create_layout(&emulator, case, &domain_id, "listing-layout", &top_panel(&fields)).await;
        let included: Vec<Value> = fields.iter().map(|id| json!({ "id": id })).collect();
        emulator
            .ok(
                case,
                "PUT",
                &format!("/domains/{domain_id}/case-event-configuration"),
                json!({ "eventBridge": {
                    "enabled": true,
                    "includedData": { "caseData": { "fields": included } },
                } }),
            )
            .await;

        delete_field(&emulator, case, &domain_id, &fields[target]).await;

        let fetched_template = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/templates/{template_id}"),
                json!({}),
            )
            .await;
        let required: Vec<String> = fetched_template
            .get("requiredFields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: GetTemplate reported no requiredFields: {fetched_template:?}")
            })
            .iter()
            .map(|entry| string_at(entry, "fieldId", case))
            .collect();
        assert_eq!(
            required, remaining,
            "case {case:?}: GetTemplate must report exactly the fields that are not \
             deleted, got {required:?}"
        );

        let fetched_layout = get_layout(&emulator, case, &domain_id, &layout_id).await;
        assert_eq!(
            fetched_layout.get("content"),
            Some(&top_panel_reported(&remaining)),
            "case {case:?}: GetLayout must leave the deleted field id out of its content, \
             got {fetched_layout:?}"
        );

        let configuration = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/case-event-configuration"),
                json!({}),
            )
            .await;
        let expected_included: Vec<Value> = remaining.iter().map(|id| json!({ "id": id })).collect();
        assert_eq!(
            configuration.pointer("/eventBridge/includedData/caseData/fields"),
            Some(&json!(expected_included)),
            "case {case:?}: GetCaseEventConfiguration must leave the deleted field id out \
             of its included data, got {configuration:?}"
        );
    }
}

#[tokio::test]
async fn field_options_cannot_be_put_on_a_deleted_field() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-delete-options-domain").await;
    let case = "BatchPutFieldOptions on a deleted SingleSelect field";
    let field_id = create_field(&emulator, case, &domain_id, "choices", "SingleSelect").await;
    let options = json!({ "options": [{ "name": "Urgent", "value": "urgent", "active": true }] });

    // While the field is live its options can be set.
    emulator
        .ok(
            case,
            "PUT",
            &format!("/domains/{domain_id}/fields/{field_id}/options"),
            options.clone(),
        )
        .await;

    delete_field(&emulator, case, &domain_id, &field_id).await;

    let (status, code, message) = emulator
        .failure_detail(
            case,
            "PUT",
            &format!("/domains/{domain_id}/fields/{field_id}/options"),
            Some(options),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException for options put on a deleted \
         field, got {status} {code} {message}"
    );
    assert!(
        message.contains(&field_id),
        "case {case:?}: the refusal must name the deleted field id {field_id}, got {message:?}"
    );
}
