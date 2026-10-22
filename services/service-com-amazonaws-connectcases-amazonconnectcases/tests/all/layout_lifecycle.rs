// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the Cases LAYOUT slice
//! (`CreateLayout` -> `GetLayout` -> `UpdateLayout`), driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: a layout is a resource of its domain, so `CreateLayout`
//! mints an id and an ARN, stores the `content` it was given EXACTLY as given
//! (variant, sections and field order included) and resolves every field id the
//! content names against the domain before storing anything; `GetLayout`
//! computes its whole answer from that stored record; and `UpdateLayout`
//! replaces only the members its request carries. An id the store does not hold
//! is refused with the modelled error (an unknown domain or layout with
//! `ResourceNotFoundException` (404), an unknown field id with
//! `ValidationException` (400)) and a name another live layout of the domain
//! already holds with `ConflictException` (409).
//!
//! The content forms below span what the rule generalises over — which panels
//! are present, how many fields a group lists and in what order, whether a
//! group is named, an empty section list and an absent one — and the published
//! bounds come from the model (`SectionsList` is `..1` and `FieldGroup$fields`
//! is `..220`), each tested on the bound and one past it.

use serde_json::{json, Value};

use crate::case_lifecycle::{domain, is_minted_id, string_at, Emulator};

/// Create a `Text` field in `domain_id` and return its id. One field is enough
/// for a layout: a layout references a field, it does not carry its type.
async fn text_field(emulator: &Emulator, domain_id: &str, name: &str) -> String {
    let case = format!("CreateField {name}");
    let response = emulator
        .ok(
            &case,
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": name, "type": "Text" }),
        )
        .await;
    string_at(&response, "fieldId", &case)
}

/// `n` fields of `domain_id`, named apart so each is its own record.
async fn text_fields(emulator: &Emulator, domain_id: &str, n: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(n);
    for index in 0..n {
        ids.push(text_field(emulator, domain_id, &format!("field-{index}")).await);
    }
    ids
}

/// `CreateLayout`, returning `(layoutId, layoutArn)`.
async fn create_layout(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    name: &str,
    content: &Value,
) -> (String, String) {
    let response = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/layouts"),
            json!({ "name": name, "content": content }),
        )
        .await;
    (
        string_at(&response, "layoutId", case),
        string_at(&response, "layoutArn", case),
    )
}

/// `GetLayout`'s whole body.
async fn get_layout(emulator: &Emulator, case: &str, domain_id: &str, layout_id: &str) -> Value {
    emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/layouts/{layout_id}"),
            json!({}),
        )
        .await
}

/// One `UpdateLayout` row: the case name, the `name` the request carries (if
/// any) and the fields its replacement content lists (if any).
type UpdateRow<'a> = (&'static str, Option<&'a str>, Option<&'a [String]>);

/// One refusal row: the case name, the HTTP method and path to call, the body,
/// and the status and wire error code the model declares for that refusal.
type RefusalRow = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    u16,
    &'static str,
);

/// A `topPanel`-only content listing `fields` in the order given, as a request
/// spells it: `moreInfo` is left out entirely.
fn top_panel(fields: &[String]) -> Value {
    let items: Vec<Value> = fields.iter().map(|id| json!({ "id": id })).collect();
    json!({ "basic": { "topPanel": { "sections": [{ "fieldGroup": { "fields": items } }] } } })
}

/// The content a read answers for a layout written with [`top_panel`] over
/// `fields`: the one unnamed group in `topPanel`, and the `moreInfo` panel the
/// request never mentioned present and empty.
fn top_panel_reported(fields: &[String]) -> Value {
    let items: Vec<&String> = fields.iter().collect();
    reported(vec![field_group(None, &items)], vec![])
}

/// The content the service REPORTS for a `basic` layout whose panels hold
/// `top` and `more`: both panels present, each with its `sections` list. A
/// panel a request never mentioned comes back present and empty — the recorded
/// behaviour of the real service — so this is the form every read answers in.
fn reported(top: Vec<Value>, more: Vec<Value>) -> Value {
    json!({ "basic": {
        "topPanel": { "sections": top },
        "moreInfo": { "sections": more },
    } })
}

/// A `fieldGroup` section over `fields`, optionally named.
fn field_group(name: Option<&str>, fields: &[&String]) -> Value {
    let items: Vec<Value> = fields.iter().map(|id| json!({ "id": id })).collect();
    match name {
        Some(name) => json!({ "fieldGroup": { "name": name, "fields": items } }),
        None => json!({ "fieldGroup": { "fields": items } }),
    }
}

/// Every form of `LayoutContent` a request can spell, each paired with the
/// content a read must then answer. `basic` is the union's only member
/// (`smithy_query(query="shape", shape="LayoutContent")`), `topPanel` and
/// `moreInfo` are both optional, `sections` is optional and capped at one entry,
/// and a `fieldGroup`'s `name` is optional while its `fields` list is ordered:
/// the rows differ along exactly those — which panel is given, how many fields
/// a group lists and in what order, whether the group is named, an empty
/// section list and an absent one. Whatever a row spells, the sections, the
/// group name and the field order come back exactly as given, and only the
/// panels a request omitted are filled in.
fn content_forms(fields: &[String]) -> Vec<(&'static str, Value, Value)> {
    let (first, second, third) = (&fields[0], &fields[1], &fields[2]);
    vec![
        (
            "topPanel with a named group of one field",
            json!({ "basic": { "topPanel": { "sections": [
                field_group(Some("Top panel"), &[first])
            ] } } }),
            reported(vec![field_group(Some("Top panel"), &[first])], vec![]),
        ),
        (
            "moreInfo only, with an unnamed group",
            json!({ "basic": { "moreInfo": { "sections": [
                field_group(None, &[second])
            ] } } }),
            reported(vec![], vec![field_group(None, &[second])]),
        ),
        (
            "both panels, the top one listing three fields in reverse creation order",
            json!({ "basic": {
                "topPanel": { "sections": [field_group(Some("Top"), &[third, second, first])] },
                "moreInfo": { "sections": [field_group(Some("More"), &[first])] },
            } }),
            reported(
                vec![field_group(Some("Top"), &[third, second, first])],
                vec![field_group(Some("More"), &[first])],
            ),
        ),
        (
            "both panels, the top one listing the same three fields in creation order",
            json!({ "basic": {
                "topPanel": { "sections": [field_group(Some("Top"), &[first, second, third])] },
                "moreInfo": { "sections": [field_group(Some("More"), &[first])] },
            } }),
            reported(
                vec![field_group(Some("Top"), &[first, second, third])],
                vec![field_group(Some("More"), &[first])],
            ),
        ),
        (
            "a panel with an empty sections list",
            json!({ "basic": { "topPanel": { "sections": [] } } }),
            reported(vec![], vec![]),
        ),
        (
            "a panel with no sections member at all",
            json!({ "basic": { "topPanel": {} } }),
            reported(vec![], vec![]),
        ),
        (
            "neither panel",
            json!({ "basic": {} }),
            reported(vec![], vec![]),
        ),
    ]
}

#[tokio::test]
async fn a_layout_s_content_round_trips_through_state_for_every_form_of_content() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-content-domain").await;
    let fields = text_fields(&emulator, &domain_id, 3).await;

    for (index, (case, content, expected)) in content_forms(&fields).into_iter().enumerate() {
        let name = format!("layout-{index}");
        let (layout_id, layout_arn) =
            create_layout(&emulator, case, &domain_id, &name, &content).await;
        assert!(
            is_minted_id(&layout_id),
            "case {case:?}: the layoutId must be a server-minted opaque id, got {layout_id:?}"
        );
        assert_eq!(
            layout_arn,
            format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/layout/{layout_id}"),
            "case {case:?}: layoutArn must address the stored layout, got {layout_arn:?}"
        );

        let fetched = get_layout(&emulator, case, &domain_id, &layout_id).await;
        assert_eq!(
            fetched.get("content"),
            Some(&expected),
            "case {case:?}: GetLayout must report the stored content as {expected:?}, \
             got {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "layoutId", case),
            layout_id,
            "case {case:?}: the layoutId must be the one CreateLayout minted, got {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "layoutArn", case),
            layout_arn,
            "case {case:?}: the layoutArn must be the stored one, got {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "name", case),
            name,
            "case {case:?}: the name must be the one it was created with, got {fetched:?}"
        );
        assert_eq!(
            fetched.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a layout that was never deleted reports deleted=false, got {fetched:?}"
        );
        assert!(
            fetched.get("createdTime").is_some() && fetched.get("lastModifiedTime").is_some(),
            "case {case:?}: both stored timestamps must be reported, got {fetched:?}"
        );
        assert!(
            fetched.get("tags").is_none(),
            "case {case:?}: a layout that was never tagged reports no tags, got {fetched:?}"
        );

        // A read reports; it must not change what the next reader sees.
        let again = get_layout(&emulator, case, &domain_id, &layout_id).await;
        assert_eq!(
            again, fetched,
            "case {case:?}: two GetLayout calls must answer the same body, got {again:?}"
        );
    }
}

#[tokio::test]
async fn update_layout_writes_only_the_members_its_request_carries() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-update-domain").await;
    let fields = text_fields(&emulator, &domain_id, 3).await;

    // Rows differ along WHICH members the update carries: neither, the name
    // only, the content only, and both. The expectation is the same rule each
    // time — an absent member leaves the stored one alone.
    let rows: Vec<UpdateRow<'_>> = vec![
        ("UpdateLayout carrying neither member", None, None),
        ("UpdateLayout carrying only a name", Some("renamed"), None),
        (
            "UpdateLayout carrying only content",
            None,
            Some(&fields[1..3]),
        ),
        (
            "UpdateLayout carrying both members",
            Some("renamed-again"),
            Some(&fields[0..1]),
        ),
    ];

    for (index, (case, new_name, new_fields)) in rows.into_iter().enumerate() {
        let created_name = format!("update-layout-{index}");
        let created_fields = &fields[0..2];
        let (layout_id, layout_arn) = create_layout(
            &emulator,
            case,
            &domain_id,
            &created_name,
            &top_panel(created_fields),
        )
        .await;

        let mut body = json!({});
        if let Some(name) = new_name {
            body["name"] = json!(name);
        }
        if let Some(new_fields) = new_fields {
            body["content"] = top_panel(new_fields);
        }
        let updated = emulator
            .ok(
                case,
                "PUT",
                &format!("/domains/{domain_id}/layouts/{layout_id}"),
                body,
            )
            .await;
        assert_eq!(
            updated,
            json!({}),
            "case {case:?}: UpdateLayout answers HTTP 200 with no output members, \
             got {updated:?}"
        );

        let fetched = get_layout(&emulator, case, &domain_id, &layout_id).await;
        let expected_name = new_name.unwrap_or(&created_name).to_string();
        let expected_content = top_panel_reported(new_fields.unwrap_or(created_fields));
        assert_eq!(
            string_at(&fetched, "name", case),
            expected_name,
            "case {case:?}: expected name {expected_name:?}, got {fetched:?}"
        );
        assert_eq!(
            fetched.get("content"),
            Some(&expected_content),
            "case {case:?}: expected content {expected_content:?}, got {fetched:?}"
        );
        assert_eq!(
            string_at(&fetched, "layoutArn", case),
            layout_arn,
            "case {case:?}: an update never re-mints the ARN, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn an_id_the_store_does_not_hold_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-refusal-domain").await;
    let other_domain = domain(&emulator, "layout-refusal-other-domain").await;
    let fields = text_fields(&emulator, &domain_id, 1).await;
    let content = top_panel(&fields);
    let (layout_id, _) = create_layout(
        &emulator,
        "CreateLayout for the refusal rows",
        &domain_id,
        "the-only-layout",
        &content,
    )
    .await;

    // Each row: one reference the store cannot resolve (or a name already
    // taken), and the status and wire error code the model declares for it.
    let rows: Vec<RefusalRow> = vec![
        (
            "CreateLayout in a domain the store does not hold",
            "POST",
            "/domains/no-such-domain/layouts".to_string(),
            Some(json!({ "name": "orphan", "content": content })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateLayout naming a field id the domain does not hold",
            "POST",
            format!("/domains/{domain_id}/layouts"),
            Some(json!({
                "name": "layout-with-a-dangling-field",
                "content": top_panel(&["field-that-was-never-created".to_string()]),
            })),
            400,
            "ValidationException",
        ),
        (
            "CreateLayout naming a field id of ANOTHER domain",
            "POST",
            format!("/domains/{other_domain}/layouts"),
            Some(json!({ "name": "cross-domain-layout", "content": content })),
            400,
            "ValidationException",
        ),
        (
            "CreateLayout under a name the domain already holds",
            "POST",
            format!("/domains/{domain_id}/layouts"),
            Some(json!({ "name": "the-only-layout", "content": content })),
            409,
            "ConflictException",
        ),
        (
            "GetLayout for a layout the domain does not hold",
            "POST",
            format!("/domains/{domain_id}/layouts/no-such-layout"),
            Some(json!({})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetLayout in a domain the store does not hold",
            "POST",
            format!("/domains/no-such-domain/layouts/{layout_id}"),
            Some(json!({})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetLayout for a layout of another domain",
            "POST",
            format!("/domains/{other_domain}/layouts/{layout_id}"),
            Some(json!({})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateLayout for a layout the domain does not hold",
            "PUT",
            format!("/domains/{domain_id}/layouts/no-such-layout"),
            Some(json!({ "name": "renamed" })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateLayout naming a field id the domain does not hold",
            "PUT",
            format!("/domains/{domain_id}/layouts/{layout_id}"),
            Some(json!({
                "content": top_panel(&["field-that-was-never-created".to_string()]),
            })),
            400,
            "ValidationException",
        ),
    ];

    for (case, method, path, body, status, code) in rows {
        let (actual_status, actual_code) = emulator.failure(case, method, &path, body).await;
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {actual_status} {actual_code}"
        );
    }

    // The refused creates stored nothing: the domain still holds exactly the
    // one layout it was given.
    let still_there = get_layout(&emulator, "GetLayout after the refusals", &domain_id, &layout_id)
        .await;
    assert_eq!(
        string_at(&still_there, "name", "GetLayout after the refusals"),
        "the-only-layout",
        "a refused create must not have touched the stored layout, got {still_there:?}"
    );
}

#[tokio::test]
async fn a_layout_name_is_unique_within_its_domain_only() {
    let emulator = Emulator::new();
    let first_domain = domain(&emulator, "layout-name-domain-one").await;
    let second_domain = domain(&emulator, "layout-name-domain-two").await;
    let content = json!({ "basic": {} });

    let (first, _) = create_layout(
        &emulator,
        "CreateLayout in the first domain",
        &first_domain,
        "shared-layout-name",
        &content,
    )
    .await;
    // The same name in another domain is a different layout, not a conflict.
    let (second, _) = create_layout(
        &emulator,
        "CreateLayout in the second domain",
        &second_domain,
        "shared-layout-name",
        &content,
    )
    .await;
    assert_ne!(
        first, second,
        "case \"two domains, one layout name\": each domain holds its own layout, \
         got the same id {first:?}"
    );

    // A rename onto a name a live layout of the SAME domain holds is refused,
    // while renaming a layout to the name it already has is not a conflict with
    // itself.
    let (renamable, _) = create_layout(
        &emulator,
        "CreateLayout to rename",
        &first_domain,
        "renamable-layout",
        &content,
    )
    .await;
    let case = "UpdateLayout renaming onto a name the domain already holds";
    let (status, code) = emulator
        .failure(
            case,
            "PUT",
            &format!("/domains/{first_domain}/layouts/{renamable}"),
            Some(json!({ "name": "shared-layout-name" })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: expected HTTP 409 ConflictException, got {status} {code}"
    );
    let case = "UpdateLayout renaming a layout to the name it already has";
    emulator
        .ok(
            case,
            "PUT",
            &format!("/domains/{first_domain}/layouts/{renamable}"),
            json!({ "name": "renamable-layout" }),
        )
        .await;
    let fetched = get_layout(&emulator, case, &first_domain, &renamable).await;
    assert_eq!(
        string_at(&fetched, "name", case),
        "renamable-layout",
        "case {case:?}: the name must still be the stored one, got {fetched:?}"
    );
}

#[tokio::test]
async fn a_layout_s_lists_are_bounded_by_the_published_caps() {
    // The model's own caps: `SectionsList` is `..1` and `FieldGroup$fields` is
    // `..220` (`smithy_query(query="shape", shape="SectionsList,FieldGroup")`),
    // restated in `state.rs` as MAX_LAYOUT_SECTIONS / MAX_LAYOUT_SECTION_FIELDS.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-bounds-domain").await;
    let fields = text_fields(
        &emulator,
        &domain_id,
        service_com_amazonaws_connectcases_amazonconnectcases::state::MAX_LAYOUT_SECTION_FIELDS,
    )
    .await;

    // ON the field bound: a group listing exactly 220 fields is accepted, and
    // read back with all 220 in the order given.
    let case = "a field group of exactly MAX_LAYOUT_SECTION_FIELDS fields";
    let at_bound = top_panel(&fields);
    let (layout_id, _) = create_layout(&emulator, case, &domain_id, "layout-at-bound", &at_bound)
        .await;
    let fetched = get_layout(&emulator, case, &domain_id, &layout_id).await;
    let expected = top_panel_reported(&fields);
    assert_eq!(
        fetched.get("content"),
        Some(&expected),
        "case {case:?}: every field of the group must be reported, in order, got {fetched:?}"
    );

    // ONE PAST each bound is refused as invalid, and nothing is stored.
    let mut past_field_bound = fields.clone();
    past_field_bound.push(fields[0].clone());
    let one_section = json!({ "fieldGroup": { "fields": [{ "id": fields[0] }] } });
    let rows: Vec<(&str, Value)> = vec![
        (
            "a field group one field past the cap",
            top_panel(&past_field_bound),
        ),
        (
            "a panel one section past the cap",
            json!({ "basic": { "topPanel": { "sections": [one_section.clone(), one_section] } } }),
        ),
    ];
    for (case, content) in rows {
        let (status, code) = emulator
            .failure(
                case,
                "POST",
                &format!("/domains/{domain_id}/layouts"),
                Some(json!({ "name": format!("refused-{case}"), "content": content })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_layout_arn_is_built_in_the_region_the_caller_signed_for() {
    // A row per region the caller can sign for, the second one not the
    // emulator's default: an ARN names the region of the request that minted it.
    for region in ["us-east-1", "eu-west-2"] {
        let emulator = Emulator::in_region(region);
        let domain_id = domain(&emulator, "layout-arn-domain").await;
        let case = format!("CreateLayout signed for {region}");
        let (layout_id, layout_arn) = create_layout(
            &emulator,
            &case,
            &domain_id,
            "regional-layout",
            &json!({ "basic": {} }),
        )
        .await;
        assert_eq!(
            layout_arn,
            format!("arn:aws:cases:{region}:000000000000:domain/{domain_id}/layout/{layout_id}"),
            "case {case:?}: the layoutArn must name the signed region, got {layout_arn:?}"
        );
    }
}

#[tokio::test]
async fn a_template_s_default_layout_is_resolved_against_the_domain_s_layouts() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-layout-domain").await;
    let (layout_id, _) = create_layout(
        &emulator,
        "CreateLayout for a template default",
        &domain_id,
        "default-layout",
        &json!({ "basic": {} }),
    )
    .await;

    // A template may now default to a layout the domain holds, and GetTemplate
    // reports the stored reference.
    let case = "CreateTemplate defaulting to a layout the domain holds";
    let created = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates"),
            json!({
                "name": "template-with-a-layout",
                "layoutConfiguration": { "defaultLayout": layout_id },
            }),
        )
        .await;
    let template_id = string_at(&created, "templateId", case);
    let fetched = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            json!({}),
        )
        .await;
    assert_eq!(
        fetched.get("layoutConfiguration"),
        Some(&json!({ "defaultLayout": layout_id })),
        "case {case:?}: the stored defaultLayout must be reported, got {fetched:?}"
    );

    // A layout id the domain does not hold is still refused.
    let case = "CreateTemplate defaulting to a layout the domain does not hold";
    let (status, code) = emulator
        .failure(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates"),
            Some(json!({
                "name": "template-with-a-dangling-layout",
                "layoutConfiguration": { "defaultLayout": "no-such-layout" },
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
    );
}
