// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `DeleteTemplate`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: deleting a template is a SOFT delete of the one record the
//! request names — the record stays retrievable through `GetTemplate` reporting
//! `deleted` true with every other member it stored untouched, it then refuses
//! `UpdateTemplate` and a `CreateCase` that names it with
//! `ResourceNotFoundException` (404), it drops out of `ListTemplates`, and no
//! other template, no other domain and no case already created from it is
//! affected; the call is `@idempotent`, so repeating it succeeds and changes
//! nothing; and a reference the store cannot resolve is refused with
//! `ResourceNotFoundException` (404) having written nothing. That is this
//! operation's own documentation: "After a cases template is deleted: You can
//! still retrieve the template by calling GetTemplate. You cannot update the
//! template. You cannot create a case by using the deleted template. Deleted
//! templates are not included in the ListTemplates response"
//! (`smithy_query(query="doc", shape="DeleteTemplate")`), and `@idempotent` is on
//! the operation (`smithy_query(query="operation", operation="DeleteTemplate")`).
//!
//! The rows below span what the rule generalises over: which optional members the
//! deleted template holds (description, every `TemplateStatus` the model declares,
//! `requiredFields` of several sizes and orders, a `layoutConfiguration`, a
//! `tagPropagationConfigurations` entry, and all of them at once), how many times
//! the delete is repeated, which member an update of a deleted template carries,
//! which of a domain's several templates is deleted, every subset of the status
//! catalog as a listing filter, and which reference a refused delete cannot
//! resolve.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::ops::types::{
    TAG_PROPAGATION_RESOURCE_TYPE_VALUES, TEMPLATE_STATUS_VALUES,
};

use crate::harness::{domain, string_at, Emulator};
use crate::template_listing::{list, listed_ids, status_query};
use crate::template_update::{create_template, get_template, layout, required, text_field};

/// One refusal row: the case name, the path to delete, and the status and wire
/// error code the model declares for that refusal.
type RefusalRow = (&'static str, String, u16, &'static str);

/// `DeleteTemplate`: `DELETE /domains/{domainId}/templates/{templateId}`, which
/// the model gives no body members and no output members.
async fn delete_template(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    template_id: &str,
) -> Value {
    emulator
        .ok(
            case,
            "DELETE",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            json!({}),
        )
        .await
}

/// `CreateCase` against `template_id`, writing `field_id`, as its answer body.
/// The template the rows below create requires no field, so this create can only
/// be refused over the template itself. `fields` is `@length(1..)`
/// (`smithy_query(query="operation", operation="CreateCase")`), so one value is
/// the smallest body the model accepts.
async fn create_case(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    template_id: &str,
    field_id: &str,
) -> Value {
    emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/cases"),
            case_body(template_id, field_id),
        )
        .await
}

/// The `CreateCase` body `create_case` sends, so a refusal row sends exactly the
/// body the accepted rows do.
fn case_body(template_id: &str, field_id: &str) -> Value {
    json!({
        "templateId": template_id,
        "fields": [{ "id": field_id, "value": { "stringValue": "a customer note" } }]
    })
}

/// `json` with `members` dropped, so two answers can be compared on everything
/// BUT the members a delete is allowed to move.
fn without(json: &Value, members: &[&str]) -> Value {
    let mut copy = json.clone();
    if let Some(object) = copy.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    copy
}

/// A `CreateTemplate` body per form a template can take, each WITHOUT its
/// `name` (the caller names it per row so the names stay unique within a
/// domain). The statuses come from the model's own `TemplateStatus` enum and the
/// tag-propagation resource type from `TagPropagationResourceType`
/// (`smithy_query(query="shape", shape="TemplateStatus,TagPropagationConfiguration")`),
/// so the catalog is the model's and not this test's.
fn template_forms(fields: &[String], layout_id: &str) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        ("no optional member at all".to_string(), json!({})),
        (
            "a description".to_string(),
            json!({ "description": "the template under test" }),
        ),
        (
            "an empty requiredFields list".to_string(),
            json!({ "requiredFields": [] }),
        ),
        (
            "one requiredField".to_string(),
            json!({ "requiredFields": required(&[&fields[0]]) }),
        ),
        (
            "three requiredFields in creation order".to_string(),
            json!({ "requiredFields": required(&[&fields[0], &fields[1], &fields[2]]) }),
        ),
        (
            "three requiredFields reversed".to_string(),
            json!({ "requiredFields": required(&[&fields[2], &fields[1], &fields[0]]) }),
        ),
        (
            "a layoutConfiguration".to_string(),
            json!({ "layoutConfiguration": { "defaultLayout": layout_id } }),
        ),
    ];
    for resource_type in TAG_PROPAGATION_RESOURCE_TYPE_VALUES {
        rows.push((
            format!("a tagPropagationConfiguration for {resource_type}"),
            json!({
                "tagPropagationConfigurations": [
                    { "resourceType": resource_type, "tagMap": { "team": "support" } }
                ]
            }),
        ));
    }
    for status in TEMPLATE_STATUS_VALUES {
        rows.push((format!("status {status}"), json!({ "status": status })));
        rows.push((
            format!("every optional member at once, status {status}"),
            json!({
                "description": "every member at once",
                "status": status,
                "requiredFields": required(&[&fields[1], &fields[0]]),
                "layoutConfiguration": { "defaultLayout": layout_id },
                "tagPropagationConfigurations": [
                    {
                        "resourceType": TAG_PROPAGATION_RESOURCE_TYPE_VALUES[0],
                        "tagMap": { "team": "support" }
                    }
                ]
            }),
        ));
    }
    rows
}

/// The three `Text` fields and the one layout every row above references.
async fn schema(emulator: &Emulator, domain_id: &str, prefix: &str) -> (Vec<String>, String) {
    let mut fields = Vec::new();
    for index in 0..3 {
        fields.push(text_field(emulator, domain_id, &format!("{prefix}-field-{index}")).await);
    }
    let layout_id = layout(emulator, domain_id, &format!("{prefix}-layout")).await;
    (fields, layout_id)
}

/// `CreateTemplate` of `form` under `name`, as its minted id.
async fn template_of_form(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    name: &str,
    form: &Value,
) -> String {
    let mut body = form.clone();
    body["name"] = json!(name);
    let (template_id, _) = create_template(emulator, case, domain_id, body).await;
    template_id
}

#[tokio::test]
async fn a_deleted_template_is_still_read_back_whole_for_every_form_of_template() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-record-domain").await;
    let (fields, layout_id) = schema(&emulator, &domain_id, "record").await;

    // A row per form a stored template can take: the delete must flag the
    // record without dropping or re-encoding any member of it.
    for (index, (case, form)) in template_forms(&fields, &layout_id).into_iter().enumerate() {
        let case = case.as_str();
        let name = format!("deleted-template-{index}");
        let template_id = template_of_form(&emulator, case, &domain_id, &name, &form).await;

        let before = get_template(&emulator, case, &domain_id, &template_id).await;
        assert_eq!(
            before.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a template that was never deleted reports deleted=false, got \
             {before:?}"
        );

        let deleted = delete_template(&emulator, case, &domain_id, &template_id).await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: DeleteTemplate answers HTTP 200 with no output members, got \
             {deleted:?}"
        );

        let after = get_template(&emulator, case, &domain_id, &template_id).await;
        assert_eq!(
            after.get("deleted"),
            Some(&json!(true)),
            "case {case:?}: a deleted template must report deleted=true, got {after:?}"
        );
        // Every member but `deleted` and `lastModifiedTime` is the record the
        // create stored: the delete changes the flag, nothing else.
        let moved = ["deleted", "lastModifiedTime"];
        assert_eq!(
            without(&after, &moved),
            without(&before, &moved),
            "case {case:?}: a delete must leave every other member as stored ({before:?}), got \
             {after:?}"
        );
        assert_eq!(
            string_at(&after, "templateId", case),
            template_id,
            "case {case:?}: a delete never re-mints the templateId, got {after:?}"
        );
        assert_eq!(
            string_at(&after, "name", case),
            name,
            "case {case:?}: a deleted template keeps the name it was created with, got {after:?}"
        );
        assert!(
            after.get("lastModifiedTime").is_some(),
            "case {case:?}: lastModifiedTime must still be reported, got {after:?}"
        );

        // A read reports; two reads of a deleted template answer the same body.
        let again = get_template(&emulator, case, &domain_id, &template_id).await;
        assert_eq!(
            again, after,
            "case {case:?}: two GetTemplate calls must answer the same body, got {again:?}"
        );
    }
}

#[tokio::test]
async fn repeating_the_delete_succeeds_and_changes_nothing() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-idempotent-domain").await;

    // A row per number of extra deletes after the first one: the operation is
    // `@idempotent`, so the record a reader sees is the record the FIRST delete
    // left, whatever the repeat count.
    for extra in [0_usize, 1, 2, 5] {
        let case = format!("{} delete(s) of one template", extra + 1);
        let case = case.as_str();
        let template_id = template_of_form(
            &emulator,
            case,
            &domain_id,
            &format!("idempotent-template-{extra}"),
            &json!({ "description": "repeat me" }),
        )
        .await;

        delete_template(&emulator, case, &domain_id, &template_id).await;
        let after_first = get_template(&emulator, case, &domain_id, &template_id).await;
        for repeat in 0..extra {
            let repeated = delete_template(&emulator, case, &domain_id, &template_id).await;
            assert_eq!(
                repeated,
                json!({}),
                "case {case:?}: repeat {repeat} must also answer HTTP 200 with no output \
                 members, got {repeated:?}"
            );
        }

        let after_repeats = get_template(&emulator, case, &domain_id, &template_id).await;
        assert_eq!(
            after_repeats, after_first,
            "case {case:?}: a repeat delete must leave the record exactly as the first delete \
             left it ({after_first:?}), got {after_repeats:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_template_refuses_every_form_of_update() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-update-domain").await;
    let (fields, layout_id) = schema(&emulator, &domain_id, "update").await;

    // A row per shape of update request: whichever modifiable member it
    // carries, an update of a DELETED template cannot be made.
    let mut rows: Vec<(String, Value)> = vec![
        (
            "UpdateTemplate carrying only a name".to_string(),
            json!({ "name": "renamed-after-delete" }),
        ),
        (
            "UpdateTemplate carrying only a description".to_string(),
            json!({ "description": "described after delete" }),
        ),
        (
            "UpdateTemplate carrying only requiredFields".to_string(),
            json!({ "requiredFields": required(&[&fields[1]]) }),
        ),
        (
            "UpdateTemplate carrying only a layoutConfiguration".to_string(),
            json!({ "layoutConfiguration": { "defaultLayout": layout_id } }),
        ),
        (
            "UpdateTemplate carrying several members".to_string(),
            json!({ "name": "renamed-again", "description": "and described" }),
        ),
    ];
    // Every `TemplateStatus` the model declares, each as the status an update of
    // a deleted template tries to set.
    for status in TEMPLATE_STATUS_VALUES {
        rows.push((
            format!("UpdateTemplate carrying only status {status}"),
            json!({ "status": status }),
        ));
    }

    for (index, (case, body)) in rows.into_iter().enumerate() {
        let case = case.as_str();
        let created_name = format!("update-after-delete-{index}");
        let template_id = template_of_form(
            &emulator,
            case,
            &domain_id,
            &created_name,
            &json!({ "requiredFields": required(&[&fields[0]]) }),
        )
        .await;
        delete_template(&emulator, case, &domain_id, &template_id).await;
        let before = get_template(&emulator, case, &domain_id, &template_id).await;

        let (status, code) = emulator
            .failure(
                case,
                "PUT",
                &format!("/domains/{domain_id}/templates/{template_id}"),
                Some(body),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException for an update of a \
             deleted template, got {status} {code}"
        );

        // The refused update wrote nothing: the deleted record is unchanged.
        let after = get_template(&emulator, case, &domain_id, &template_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the deleted record exactly as it was \
             ({before:?}), got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_case_cannot_be_created_from_a_deleted_template_and_existing_cases_survive() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-case-domain").await;
    let field_id = text_field(&emulator, &domain_id, "case-field").await;

    // A row per number of cases already created from the template before it is
    // deleted: the delete refuses a NEW case and touches none of the old ones.
    for existing in [0_usize, 1, 3] {
        let case = format!("{existing} case(s) created before the delete");
        let case = case.as_str();
        let template_id = template_of_form(
            &emulator,
            case,
            &domain_id,
            &format!("case-template-{existing}"),
            &json!({}),
        )
        .await;

        let mut case_ids = Vec::new();
        for _ in 0..existing {
            let created = create_case(&emulator, case, &domain_id, &template_id, &field_id).await;
            case_ids.push(string_at(&created, "caseId", case));
        }

        delete_template(&emulator, case, &domain_id, &template_id).await;

        let (status, code) = emulator
            .failure(
                case,
                "POST",
                &format!("/domains/{domain_id}/cases"),
                Some(case_body(&template_id, &field_id)),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException for a case created from \
             a deleted template, got {status} {code}"
        );

        // A case created while the template was live is a record of its own: the
        // template's delete does not cascade to it.
        for case_id in &case_ids {
            let fetched = emulator
                .ok(
                    case,
                    "POST",
                    &format!("/domains/{domain_id}/cases/{case_id}"),
                    json!({ "fields": [{ "id": field_id }] }),
                )
                .await;
            assert_eq!(
                string_at(&fetched, "templateId", case),
                template_id,
                "case {case:?}: a case created before the delete must still report its \
                 templateId, got {fetched:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_template_drops_out_of_every_listing_of_its_domain() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-listing-domain").await;

    // One template per `TemplateStatus` the model declares, so every subset of
    // the status catalog below has something to report.
    let mut created: Vec<(String, &str)> = Vec::new();
    for status in TEMPLATE_STATUS_VALUES {
        let case = format!("CreateTemplate status={status}");
        let id = template_of_form(
            &emulator,
            &case,
            &domain_id,
            &format!("listed-template-{status}"),
            &json!({ "status": status }),
        )
        .await;
        created.push((id, status));
    }

    // A row per which template is deleted: the listing must report exactly the
    // live ones, under no filter and under every subset of the catalog.
    for (target, (target_id, target_status)) in created.iter().enumerate() {
        let case = format!("a delete of the {target_status} template");
        let case = case.as_str();
        delete_template(&emulator, case, &domain_id, target_id).await;

        let subsets: Vec<Vec<&str>> = vec![
            vec![],
            vec![TEMPLATE_STATUS_VALUES[0]],
            vec![TEMPLATE_STATUS_VALUES[1]],
            TEMPLATE_STATUS_VALUES.to_vec(),
        ];
        for subset in subsets {
            let query = status_query(&subset);
            let answer = list(&emulator, case, &domain_id, &query).await;
            let listed = listed_ids(case, &answer);
            let expected: Vec<String> = created
                .iter()
                .enumerate()
                .filter(|(index, (_, status))| {
                    *index > target && (subset.is_empty() || subset.contains(status))
                })
                .map(|(_, (id, _))| id.clone())
                .collect();
            assert_eq!(
                listed, expected,
                "case {case:?}: a listing filtered by {subset:?} must report exactly the live \
                 templates {expected:?}, got {listed:?}"
            );
        }

        // The deleted template is still the record a single-resource read sees.
        let fetched = get_template(&emulator, case, &domain_id, target_id).await;
        assert_eq!(
            fetched.get("deleted"),
            Some(&json!(true)),
            "case {case:?}: GetTemplate must still report the deleted template, got {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "status", case),
            *target_status,
            "case {case:?}: a delete must not move the template's status, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_template_it_names() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-siblings-domain").await;
    let other_domain = domain(&emulator, "template-delete-other-domain").await;

    // Three sibling templates of one domain, plus one of ANOTHER domain holding
    // the same name: a row per which sibling is deleted, so the position of the
    // target among its siblings cannot matter.
    for target in 0..3_usize {
        let case = format!("a delete of sibling {target} among three sibling templates");
        let case = case.as_str();
        let mut siblings = Vec::new();
        for index in 0..3 {
            siblings.push(
                template_of_form(
                    &emulator,
                    case,
                    &domain_id,
                    &format!("sibling-{target}-{index}"),
                    &json!({}),
                )
                .await,
            );
        }
        let elsewhere = template_of_form(
            &emulator,
            case,
            &other_domain,
            &format!("sibling-{target}-{target}"),
            &json!({}),
        )
        .await;

        delete_template(&emulator, case, &domain_id, &siblings[target]).await;

        for (index, template_id) in siblings.iter().enumerate() {
            let fetched = get_template(&emulator, case, &domain_id, template_id).await;
            let expected = index == target;
            assert_eq!(
                fetched.get("deleted"),
                Some(&json!(expected)),
                "case {case:?}: deleting sibling {target} must leave sibling {index} \
                 deleted={expected}, got {fetched:?}"
            );
        }
        // The same-named template of another domain is a different resource.
        let fetched = get_template(&emulator, case, &other_domain, &elsewhere).await;
        assert_eq!(
            fetched.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a delete in one domain must not touch a same-named template of \
             another domain, got {fetched:?}"
        );

        // A live sibling still accepts an update, so the delete did not flag the
        // domain rather than the template.
        let live = siblings
            .iter()
            .enumerate()
            .find(|(index, _)| *index != target);
        let (_, live_id) = live.expect("three siblings leave a live one");
        emulator
            .ok(
                case,
                "PUT",
                &format!("/domains/{domain_id}/templates/{live_id}"),
                json!({ "name": format!("sibling-{target}-renamed") }),
            )
            .await;
    }
}

#[tokio::test]
async fn deleting_a_template_frees_its_name_for_a_new_one() {
    // A template name "must be unique per domain"
    // (`smithy_query(query="doc", shape="CreateTemplateRequest", member="name")`),
    // and a deleted template is no longer one of the domain's live templates, so
    // the name it held is available again — and the new template is its own
    // record.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-name-domain").await;
    let case = "CreateTemplate under the name of a deleted template";

    let first = template_of_form(&emulator, case, &domain_id, "recycled-name", &json!({})).await;
    let (status, code) = emulator
        .failure(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates"),
            Some(json!({ "name": "recycled-name" })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: while the first template is live its name is taken, got {status} {code}"
    );

    delete_template(&emulator, case, &domain_id, &first).await;
    let second = template_of_form(&emulator, case, &domain_id, "recycled-name", &json!({})).await;
    assert_ne!(
        second, first,
        "case {case:?}: the new template must be its own record, got the deleted id {first:?}"
    );
    let reused = get_template(&emulator, case, &domain_id, &second).await;
    assert_eq!(
        reused.get("deleted"),
        Some(&json!(false)),
        "case {case:?}: the new template is live, got {reused:?}"
    );
    let old = get_template(&emulator, case, &domain_id, &first).await;
    assert_eq!(
        old.get("deleted"),
        Some(&json!(true)),
        "case {case:?}: the deleted template stays deleted, got {old:?}"
    );
}

#[tokio::test]
async fn a_delete_of_an_id_the_store_does_not_hold_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-delete-refusal-domain").await;
    let other_domain = domain(&emulator, "template-delete-refusal-other-domain").await;
    let template_id = template_of_form(
        &emulator,
        "CreateTemplate for the refusal rows",
        &domain_id,
        "the-only-template",
        &json!({}),
    )
    .await;

    // A row per reference the store cannot resolve: the domain, the template,
    // and a template of a DIFFERENT domain (a template is scoped to its domain).
    let rows: Vec<RefusalRow> = vec![
        (
            "DeleteTemplate in a domain the store does not hold",
            format!("/domains/no-such-domain/templates/{template_id}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteTemplate for a template the domain does not hold",
            format!("/domains/{domain_id}/templates/no-such-template"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteTemplate for a template of another domain",
            format!("/domains/{other_domain}/templates/{template_id}"),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, path, status, code) in rows {
        let (actual_status, actual_code) = emulator
            .failure(case, "DELETE", &path, Some(json!({})))
            .await;
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {actual_status} {actual_code}"
        );
    }

    // Every refusal wrote nothing: the one stored template is still live.
    let case = "GetTemplate after the refused deletes";
    let still_live = get_template(&emulator, case, &domain_id, &template_id).await;
    assert_eq!(
        still_live.get("deleted"),
        Some(&json!(false)),
        "case {case:?}: a refused delete must not flag the stored template, got {still_live:?}"
    );
}
