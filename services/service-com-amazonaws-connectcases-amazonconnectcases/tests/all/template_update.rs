// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `UpdateTemplate`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: an update of an existing resource writes exactly the
//! modifiable members its request carries onto the stored record and leaves every
//! member it omits at the value last written, so a later read reports, for each
//! member, the last value written to it; a request that carries no modifiable
//! member at all is refused with `ValidationException` (400); every id the
//! request references is resolved against persisted state before anything is
//! written (an unknown domain or template with `ResourceNotFoundException` (404),
//! an unknown field or layout id with `ValidationException` (400)); a rename onto
//! a name another live resource of the same container holds is refused with
//! `ConflictException` (409); and the update never re-mints the resource's
//! identity (id, ARN, creation instant), moving `lastModifiedTime` only when the
//! call actually changed something.
//!
//! The rows span what the rule generalises over: WHICH modifiable members a
//! request carries (each one alone, several at once, none), the values they
//! carry (every `TemplateStatus` the model declares, set from each of them, in
//! both directions), the SIZE and ORDER of the list-valued members (zero, one and
//! three `requiredFields`, in creation order and reversed), and each published
//! bound of `TemplateName`, `TemplateDescription`, `RequiredFieldList` and
//! `TagPropagationConfigurationList` at the bound and one past it.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::ops::types::TEMPLATE_STATUS_VALUES;

use crate::harness::{domain, string_at, Emulator};

/// The published bounds of the members `UpdateTemplate` writes, from the model:
/// `TemplateName` is `@length(1..100)`, `TemplateDescription` `@length(..255)`,
/// `RequiredFieldList` `@length(..100)` and `TagPropagationConfigurationList`
/// `@length(..1)`
/// (`smithy_query(query="operation", operation="UpdateTemplate")`).
const MAX_TEMPLATE_NAME: usize = 100;
const MAX_TEMPLATE_DESCRIPTION: usize = 255;
const MAX_REQUIRED_FIELDS: usize = 100;
const MAX_TAG_PROPAGATION_CONFIGURATIONS: usize = 1;

/// The members of a template a reader sees, as the JSON a read reports for each:
/// the running expectation a sequence of updates is checked against. `None` means
/// the member must be ABSENT from the answer.
#[derive(Debug, Clone)]
struct Record {
    name: String,
    description: Option<Value>,
    status: String,
    required_fields: Option<Value>,
    layout_configuration: Option<Value>,
    tag_propagation_configurations: Option<Value>,
}

/// One update row: the case name, the request body, and the whole record a read
/// must then report.
type UpdateRow = (String, Value, Record);

/// One refusal row: the case name, the path to `PUT`, the body, and the status
/// and wire error code the model declares for that refusal.
type RefusalRow = (String, String, Value, u16, &'static str);

/// `CreateField` of type `Text` in `domain_id`, as its minted id.
pub(crate) async fn text_field(emulator: &Emulator, domain_id: &str, name: &str) -> String {
    let case = format!("CreateField {name}");
    let created = emulator
        .ok(
            &case,
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": name, "type": "Text" }),
        )
        .await;
    string_at(&created, "fieldId", &case)
}

/// `CreateLayout` with an empty `basic` content in `domain_id`, as its minted id.
/// A template references a layout by id, so the content does not matter here.
pub(crate) async fn layout(emulator: &Emulator, domain_id: &str, name: &str) -> String {
    let case = format!("CreateLayout {name}");
    let created = emulator
        .ok(
            &case,
            "POST",
            &format!("/domains/{domain_id}/layouts"),
            json!({ "name": name, "content": { "basic": {} } }),
        )
        .await;
    string_at(&created, "layoutId", &case)
}

/// `CreateTemplate` with `body`, as `(templateId, templateArn)`.
pub(crate) async fn create_template(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    body: Value,
) -> (String, String) {
    let created = emulator
        .ok(case, "POST", &format!("/domains/{domain_id}/templates"), body)
        .await;
    (
        string_at(&created, "templateId", case),
        string_at(&created, "templateArn", case),
    )
}

/// `GetTemplate`'s whole body: the record every expectation below is read from.
pub(crate) async fn get_template(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    template_id: &str,
) -> Value {
    emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            json!({}),
        )
        .await
}

/// `UpdateTemplate` that must succeed, as its answer body.
pub(crate) async fn update_template(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    template_id: &str,
    body: Value,
) -> Value {
    emulator
        .ok(
            case,
            "PUT",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            body,
        )
        .await
}

/// A numeric member of a read's answer (the epoch-seconds timestamps).
fn number_at(json: &Value, member: &str, case: &str) -> f64 {
    json.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no numeric member {member:?} in {json:?}"))
}

/// A `requiredFields` list over `field_ids`, in the order given.
pub(crate) fn required(field_ids: &[&String]) -> Value {
    Value::Array(
        field_ids
            .iter()
            .map(|id| json!({ "fieldId": id }))
            .collect(),
    )
}

/// Assert that `fetched` reports exactly the members of `expected`.
fn assert_record(case: &str, fetched: &Value, expected: &Record) {
    assert_eq!(
        string_at(fetched, "name", case),
        expected.name,
        "case {case:?}: expected name {:?}, got {fetched:?}",
        expected.name
    );
    assert_eq!(
        string_at(fetched, "status", case),
        expected.status,
        "case {case:?}: expected status {:?}, got {fetched:?}",
        expected.status
    );
    for (member, want) in [
        ("description", &expected.description),
        ("requiredFields", &expected.required_fields),
        ("layoutConfiguration", &expected.layout_configuration),
        (
            "tagPropagationConfigurations",
            &expected.tag_propagation_configurations,
        ),
    ] {
        let actual = fetched.get(member);
        assert_eq!(
            actual,
            want.as_ref(),
            "case {case:?}: expected {member} to be {want:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn update_template_writes_only_the_members_its_request_carries() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-update-domain").await;
    let first = text_field(&emulator, &domain_id, "field-0").await;
    let second = text_field(&emulator, &domain_id, "field-1").await;
    let third = text_field(&emulator, &domain_id, "field-2").await;
    let layout_a = layout(&emulator, &domain_id, "layout-a").await;
    let layout_b = layout(&emulator, &domain_id, "layout-b").await;

    // The template every row below updates: created with a name and ONE required
    // field, no description, no status (which `CreateTemplate` stores as Active)
    // and no layout configuration.
    let (template_id, template_arn) = create_template(
        &emulator,
        "the template under test",
        &domain_id,
        json!({ "name": "template-under-test", "requiredFields": required(&[&first]) }),
    )
    .await;

    let mut expected = Record {
        name: "template-under-test".to_string(),
        description: None,
        status: "Active".to_string(),
        required_fields: Some(required(&[&first])),
        layout_configuration: None,
        tag_propagation_configurations: None,
    };
    let mut rows: Vec<UpdateRow> = Vec::new();

    // Every `TemplateStatus` the model declares, each written from the other one
    // so the toggle is exercised in both directions. `TEMPLATE_STATUS_VALUES` is
    // the model's own enum (`smithy_query(query="shape", shape="TemplateStatus")`).
    for status in TEMPLATE_STATUS_VALUES
        .iter()
        .rev()
        .chain(TEMPLATE_STATUS_VALUES.iter())
    {
        expected.status = (*status).to_string();
        rows.push((
            format!("status only, set to {status}"),
            json!({ "status": status }),
            expected.clone(),
        ));
    }

    // The name alone: every other member is preserved.
    expected.name = "renamed-template".to_string();
    rows.push((
        "name only".to_string(),
        json!({ "name": "renamed-template" }),
        expected.clone(),
    ));

    // The name at the published maximum length, and back to a short one.
    let longest_name = "n".repeat(MAX_TEMPLATE_NAME);
    expected.name = longest_name.clone();
    rows.push((
        format!("name of the published maximum {MAX_TEMPLATE_NAME} characters"),
        json!({ "name": longest_name }),
        expected.clone(),
    ));
    expected.name = "x".to_string();
    rows.push((
        "name of the published minimum 1 character".to_string(),
        json!({ "name": "x" }),
        expected.clone(),
    ));

    // The description alone, then replaced, then at the published maximum length.
    expected.description = Some(json!("a brief description"));
    rows.push((
        "description only".to_string(),
        json!({ "description": "a brief description" }),
        expected.clone(),
    ));
    expected.description = Some(json!(""));
    rows.push((
        "description replaced with the empty string".to_string(),
        json!({ "description": "" }),
        expected.clone(),
    ));
    let longest_description = "d".repeat(MAX_TEMPLATE_DESCRIPTION);
    expected.description = Some(json!(longest_description));
    rows.push((
        format!("description of the published maximum {MAX_TEMPLATE_DESCRIPTION} characters"),
        json!({ "description": longest_description }),
        expected.clone(),
    ));

    // `requiredFields` over sizes and orders: three fields in creation order,
    // the same three reversed, one field, and the empty list — which a read
    // reports as no `requiredFields` member at all.
    expected.required_fields = Some(required(&[&first, &second, &third]));
    rows.push((
        "requiredFields of three fields in creation order".to_string(),
        json!({ "requiredFields": required(&[&first, &second, &third]) }),
        expected.clone(),
    ));
    expected.required_fields = Some(required(&[&third, &second, &first]));
    rows.push((
        "requiredFields of the same three fields reversed".to_string(),
        json!({ "requiredFields": required(&[&third, &second, &first]) }),
        expected.clone(),
    ));
    expected.required_fields = Some(required(&[&second]));
    rows.push((
        "requiredFields of one field".to_string(),
        json!({ "requiredFields": required(&[&second]) }),
        expected.clone(),
    ));
    expected.required_fields = None;
    rows.push((
        "requiredFields of no fields".to_string(),
        json!({ "requiredFields": [] }),
        expected.clone(),
    ));

    // `layoutConfiguration`: a layout of the domain, another one, and the
    // configuration with no `defaultLayout` member at all.
    expected.layout_configuration = Some(json!({ "defaultLayout": layout_a }));
    rows.push((
        "layoutConfiguration naming a layout of the domain".to_string(),
        json!({ "layoutConfiguration": { "defaultLayout": layout_a } }),
        expected.clone(),
    ));
    expected.layout_configuration = Some(json!({ "defaultLayout": layout_b }));
    rows.push((
        "layoutConfiguration naming another layout of the domain".to_string(),
        json!({ "layoutConfiguration": { "defaultLayout": layout_b } }),
        expected.clone(),
    ));
    expected.layout_configuration = Some(json!({}));
    rows.push((
        "layoutConfiguration with no defaultLayout".to_string(),
        json!({ "layoutConfiguration": {} }),
        expected.clone(),
    ));

    // `tagPropagationConfigurations` at the published maximum of one entry.
    // `TagPropagationResourceType` has one value, `Cases`
    // (`smithy_query(query="shape", shape="TagPropagationResourceType")`).
    let propagation = json!([{ "resourceType": "Cases", "tagMap": { "team": "support" } }]);
    expected.tag_propagation_configurations = Some(propagation.clone());
    rows.push((
        format!(
            "tagPropagationConfigurations of the published maximum \
             {MAX_TAG_PROPAGATION_CONFIGURATIONS} entry"
        ),
        json!({ "tagPropagationConfigurations": propagation }),
        expected.clone(),
    ));
    expected.tag_propagation_configurations = None;
    rows.push((
        "tagPropagationConfigurations of no entries".to_string(),
        json!({ "tagPropagationConfigurations": [] }),
        expected.clone(),
    ));

    // Every modifiable member at once.
    expected.name = "everything-at-once".to_string();
    expected.description = Some(json!("all members in one call"));
    expected.status = "Inactive".to_string();
    expected.required_fields = Some(required(&[&first, &third]));
    expected.layout_configuration = Some(json!({ "defaultLayout": layout_a }));
    rows.push((
        "every modifiable member at once".to_string(),
        json!({
            "name": "everything-at-once",
            "description": "all members in one call",
            "status": "Inactive",
            "requiredFields": required(&[&first, &third]),
            "layoutConfiguration": { "defaultLayout": layout_a },
        }),
        expected.clone(),
    ));

    // A rename onto the name the template already holds is not a conflict with
    // itself, and changes nothing else.
    rows.push((
        "renamed to the name it already holds".to_string(),
        json!({ "name": "everything-at-once" }),
        expected.clone(),
    ));

    let created_time = number_at(
        &get_template(&emulator, "creation instant", &domain_id, &template_id).await,
        "createdTime",
        "creation instant",
    );

    for (case, body, expected) in rows {
        let answer = update_template(&emulator, &case, &domain_id, &template_id, body).await;
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: UpdateTemplate answers HTTP 200 with no output members, got {answer:?}"
        );

        let fetched = get_template(&emulator, &case, &domain_id, &template_id).await;
        assert_record(&case, &fetched, &expected);

        // An update writes members; it never re-mints the resource's identity.
        assert_eq!(
            string_at(&fetched, "templateId", &case),
            template_id,
            "case {case:?}: an update never re-mints the templateId, got {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "templateArn", &case),
            template_arn,
            "case {case:?}: an update never re-mints the templateArn, got {fetched:?}"
        );
        assert_eq!(
            number_at(&fetched, "createdTime", &case),
            created_time,
            "case {case:?}: an update never moves createdTime, got {fetched:?}"
        );
        assert_eq!(
            fetched.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a template that was never deleted reports deleted=false, \
             got {fetched:?}"
        );

        // Reading reports; it must not change what the next reader sees.
        let again = get_template(&emulator, &case, &domain_id, &template_id).await;
        assert_eq!(
            again, fetched,
            "case {case:?}: two GetTemplate calls must answer the same body, got {again:?}"
        );
    }
}

#[tokio::test]
async fn an_update_moves_last_modified_time_only_when_it_changed_something() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-timestamp-domain").await;
    let field_id = text_field(&emulator, &domain_id, "field-0").await;
    let (template_id, _) = create_template(
        &emulator,
        "the template under test",
        &domain_id,
        json!({
            "name": "timestamps",
            "description": "the first description",
            "status": "Active",
            "requiredFields": required(&[&field_id]),
        }),
    )
    .await;

    // Rows differ along whether the value written DIFFERS from the stored one:
    // writing a member the value it already holds modified nothing, so the
    // modification instant must not move.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("the name it already holds", json!({ "name": "timestamps" }), false),
        (
            "the description it already holds",
            json!({ "description": "the first description" }),
            false,
        ),
        ("the status it already holds", json!({ "status": "Active" }), false),
        (
            "every unchanged member at once",
            json!({
                "name": "timestamps",
                "description": "the first description",
                "status": "Active",
            }),
            false,
        ),
        ("a different status", json!({ "status": "Inactive" }), true),
        ("a different name", json!({ "name": "timestamps-renamed" }), true),
        (
            "a different description",
            json!({ "description": "the second description" }),
            true,
        ),
        (
            "a replacement requiredFields",
            json!({ "requiredFields": [] }),
            true,
        ),
    ];

    for (case, body, moves) in rows {
        let before = get_template(&emulator, case, &domain_id, &template_id).await;
        let was = number_at(&before, "lastModifiedTime", case);
        let created = number_at(&before, "createdTime", case);

        update_template(&emulator, case, &domain_id, &template_id, body).await;

        let after = get_template(&emulator, case, &domain_id, &template_id).await;
        let now = number_at(&after, "lastModifiedTime", case);
        if moves {
            assert!(
                now > was,
                "case {case:?}: an update that changed a member must move lastModifiedTime past \
                 {was}, got {now}"
            );
        } else {
            assert_eq!(
                now, was,
                "case {case:?}: an update that changed nothing must leave lastModifiedTime at \
                 {was}, got {now}"
            );
        }
        assert_eq!(
            number_at(&after, "createdTime", case),
            created,
            "case {case:?}: an update never moves createdTime, got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_update_the_store_cannot_satisfy_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-refusal-domain").await;
    let other_domain = domain(&emulator, "template-refusal-other-domain").await;
    let field_id = text_field(&emulator, &domain_id, "field-0").await;
    let other_field = text_field(&emulator, &other_domain, "other-field").await;
    let layout_id = layout(&emulator, &domain_id, "layout-a").await;
    let other_layout = layout(&emulator, &other_domain, "other-layout").await;

    let (taken, _) = create_template(
        &emulator,
        "the template holding the taken name",
        &domain_id,
        json!({ "name": "name-already-taken" }),
    )
    .await;
    let (template_id, _) = create_template(
        &emulator,
        "the template under test",
        &domain_id,
        json!({ "name": "refusals", "requiredFields": required(&[&field_id]) }),
    )
    .await;
    let path = format!("/domains/{domain_id}/templates/{template_id}");

    let mut rows: Vec<RefusalRow> = vec![
        // "At least one of these attributes must not be null": a request that
        // asks for no change is a bad request, not a silent no-op.
        (
            "no modifiable member at all".to_string(),
            path.clone(),
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "a template the domain does not hold".to_string(),
            format!("/domains/{domain_id}/templates/no-such-template"),
            json!({ "status": "Inactive" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a domain the store does not hold".to_string(),
            format!("/domains/no-such-domain/templates/{template_id}"),
            json!({ "status": "Inactive" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a template of ANOTHER domain".to_string(),
            format!("/domains/{other_domain}/templates/{template_id}"),
            json!({ "status": "Inactive" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a rename onto a name another live template holds".to_string(),
            path.clone(),
            json!({ "name": "name-already-taken" }),
            409,
            "ConflictException",
        ),
        (
            "requiredFields naming a field id the domain does not hold".to_string(),
            path.clone(),
            json!({ "requiredFields": [{ "fieldId": "field-that-was-never-created" }] }),
            400,
            "ValidationException",
        ),
        (
            "requiredFields naming a field id of ANOTHER domain".to_string(),
            path.clone(),
            json!({ "requiredFields": [{ "fieldId": other_field }] }),
            400,
            "ValidationException",
        ),
        (
            "requiredFields naming the same field id twice".to_string(),
            path.clone(),
            json!({ "requiredFields": [{ "fieldId": field_id }, { "fieldId": field_id }] }),
            400,
            "ValidationException",
        ),
        (
            "layoutConfiguration naming a layout the domain does not hold".to_string(),
            path.clone(),
            // A layout id one character longer than one this domain really
            // holds: the refusal is about the id not resolving, not about the
            // domain holding no layout at all.
            json!({ "layoutConfiguration": { "defaultLayout": format!("{layout_id}x") } }),
            400,
            "ValidationException",
        ),
        (
            "layoutConfiguration naming a layout of ANOTHER domain".to_string(),
            path.clone(),
            json!({ "layoutConfiguration": { "defaultLayout": other_layout } }),
            400,
            "ValidationException",
        ),
        (
            // This emulator has no case-rule resource yet (`CreateCaseRule` is
            // still a scaffolded 501), so no template can reference one.
            "rules naming a case rule".to_string(),
            path.clone(),
            json!({ "rules": [{ "caseRuleId": "no-such-case-rule" }] }),
            400,
            "ValidationException",
        ),
        (
            format!("a name one past the published maximum {MAX_TEMPLATE_NAME} characters"),
            path.clone(),
            json!({ "name": "n".repeat(MAX_TEMPLATE_NAME + 1) }),
            400,
            "ValidationException",
        ),
        (
            "a name one short of the published minimum 1 character".to_string(),
            path.clone(),
            json!({ "name": "" }),
            400,
            "ValidationException",
        ),
        (
            // `TemplateName` is `@pattern("^.*[\\S]$")`: a name ending in
            // whitespace is not one.
            "a name that does not end in a non-whitespace character".to_string(),
            path.clone(),
            json!({ "name": "trailing space " }),
            400,
            "ValidationException",
        ),
        (
            format!(
                "a description one past the published maximum {MAX_TEMPLATE_DESCRIPTION} \
                 characters"
            ),
            path.clone(),
            json!({ "description": "d".repeat(MAX_TEMPLATE_DESCRIPTION + 1) }),
            400,
            "ValidationException",
        ),
        (
            format!("requiredFields one past the published maximum {MAX_REQUIRED_FIELDS} entries"),
            path.clone(),
            json!({ "requiredFields": (0..=MAX_REQUIRED_FIELDS)
                .map(|index| json!({ "fieldId": format!("field-{index}") }))
                .collect::<Vec<Value>>() }),
            400,
            "ValidationException",
        ),
        (
            format!(
                "tagPropagationConfigurations one past the published maximum \
                 {MAX_TAG_PROPAGATION_CONFIGURATIONS} entry"
            ),
            path.clone(),
            json!({ "tagPropagationConfigurations": [
                { "resourceType": "Cases", "tagMap": { "team": "support" } },
                { "resourceType": "Cases", "tagMap": { "team": "billing" } },
            ] }),
            400,
            "ValidationException",
        ),
    ];
    // A value that is not a `TemplateStatus` cannot be a template's status, so it
    // is refused rather than stored.
    for unknown in ["Draft", "active", "ACTIVE", "Deleted", ""] {
        rows.push((
            format!("a status that is not a TemplateStatus: {unknown:?}"),
            path.clone(),
            json!({ "status": unknown }),
            400,
            "ValidationException",
        ));
    }

    // The record as it stands before any refusal: a refused update writes
    // nothing, so every row must leave this body exactly as it is.
    let before = get_template(&emulator, "before the refusals", &domain_id, &template_id).await;

    for (case, path, body, expected_status, expected_code) in rows {
        let (status, code) = emulator
            .failure(&case, "PUT", &path, Some(body.clone()))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {code}"
        );
        let after = get_template(&emulator, &case, &domain_id, &template_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must write nothing, got {after:?}"
        );
    }

    // The template that holds the taken name is untouched by the refused rename.
    let holder = get_template(&emulator, "the name holder", &domain_id, &taken).await;
    assert_eq!(
        string_at(&holder, "name", "the name holder"),
        "name-already-taken",
        "case \"the name holder\": a refused rename must not touch the template whose name was \
         taken, got {holder:?}"
    );
}

#[tokio::test]
async fn an_update_is_scoped_to_the_template_it_names() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-scope-domain").await;
    let other_domain = domain(&emulator, "template-scope-other-domain").await;

    // Two templates of one domain and one of another, all three created Active
    // with the same name — a name is unique per DOMAIN, so the third is fine.
    let (first, _) = create_template(
        &emulator,
        "first",
        &domain_id,
        json!({ "name": "scoped-first" }),
    )
    .await;
    let (second, _) = create_template(
        &emulator,
        "second",
        &domain_id,
        json!({ "name": "scoped-second" }),
    )
    .await;
    let (elsewhere, _) = create_template(
        &emulator,
        "elsewhere",
        &other_domain,
        json!({ "name": "scoped-first" }),
    )
    .await;

    update_template(
        &emulator,
        "update only the first",
        &domain_id,
        &first,
        json!({ "status": "Inactive", "description": "only this one" }),
    )
    .await;

    for (case, domain_of, template_id, status, description) in [
        ("the updated template", &domain_id, &first, "Inactive", Some("only this one")),
        ("its sibling in the same domain", &domain_id, &second, "Active", None),
        ("the same-named template of another domain", &other_domain, &elsewhere, "Active", None),
    ] {
        let fetched = get_template(&emulator, case, domain_of, template_id).await;
        assert_eq!(
            string_at(&fetched, "status", case),
            status,
            "case {case:?}: expected status {status:?}, got {fetched:?}"
        );
        assert_eq!(
            fetched.get("description").and_then(Value::as_str),
            description,
            "case {case:?}: expected description {description:?}, got {fetched:?}"
        );
    }
}
