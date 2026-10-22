// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `DeleteLayout`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: deleting a layout is a SOFT delete of the one record the
//! request names — the layout stays retrievable through `GetLayout` reporting
//! `deleted` true with its id, ARN, name, content and `createdTime` untouched,
//! it then refuses `UpdateLayout` with `ResourceNotFoundException` (404), and no other
//! layout of the domain is affected; the call is `@idempotent`, so repeating it
//! succeeds and changes nothing; and an id the store does not hold is refused
//! with `ResourceNotFoundException` (404) having written nothing. That is this
//! operation's own documentation: "After a layout is deleted: You can still
//! retrieve the layout by calling GetLayout. You cannot update a deleted layout
//! by calling UpdateLayout"
//! (`smithy_query(query="doc", shape="DeleteLayout")`) — the prose calls that
//! refusal a `ValidationException`, while the service's RECORDED answer is
//! `ResourceNotFoundException`, which is what is asserted here: the behavioural
//! unit `connectcases.layout-create-update-content-round-trip` (corpus
//! `simparity-behavioral-aws-v1`) asserts the code of that refusal is
//! `ResourceNotFoundException` — and `@idempotent` is on the operation
//! (`smithy_query(query="operation", operation="DeleteLayout")`).
//!
//! The rows below span what the rule generalises over: which form of
//! `LayoutContent` the deleted layout holds, how many times the delete is
//! repeated, which member an update of a deleted layout carries, which of a
//! domain's several layouts is deleted, and which reference a refused delete
//! cannot resolve.

use serde_json::{json, Value};

use crate::harness::{domain, string_at, Emulator};
use crate::layout_lifecycle::{
    content_forms, create_layout, get_layout, text_fields, top_panel, top_panel_reported,
};

/// `DeleteLayout`: `DELETE /domains/{domainId}/layouts/{layoutId}`, which the
/// model gives no body members and no output members.
async fn delete_layout(emulator: &Emulator, case: &str, domain_id: &str, layout_id: &str) -> Value {
    emulator
        .ok(
            case,
            "DELETE",
            &format!("/domains/{domain_id}/layouts/{layout_id}"),
            json!({}),
        )
        .await
}

/// One refusal row: the case name, the path to delete, and the status and wire
/// error code the model declares for that refusal.
type RefusalRow = (&'static str, String, u16, &'static str);

#[tokio::test]
async fn a_deleted_layout_is_still_read_back_whole_for_every_form_of_content() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-delete-content-domain").await;
    let fields = text_fields(&emulator, &domain_id, 3).await;

    // A row per form of `LayoutContent` a request can spell: the delete must
    // flag the record without re-encoding or dropping any of it.
    for (index, (case, content, expected_content)) in
        content_forms(&fields).into_iter().enumerate()
    {
        let name = format!("deleted-layout-{index}");
        let (layout_id, layout_arn) =
            create_layout(&emulator, case, &domain_id, &name, &content).await;

        let before = get_layout(&emulator, case, &domain_id, &layout_id).await;
        assert_eq!(
            before.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a layout that was never deleted reports deleted=false, got {before:?}"
        );

        let deleted = delete_layout(&emulator, case, &domain_id, &layout_id).await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: DeleteLayout answers HTTP 200 with no output members, got {deleted:?}"
        );

        let after = get_layout(&emulator, case, &domain_id, &layout_id).await;
        assert_eq!(
            after.get("deleted"),
            Some(&json!(true)),
            "case {case:?}: a deleted layout must report deleted=true, got {after:?}"
        );
        assert_eq!(
            after.get("content"),
            Some(&expected_content),
            "case {case:?}: expected the stored content {expected_content:?} after the \
             delete, got {after:?}"
        );
        assert_eq!(
            string_at(&after, "layoutId", case),
            layout_id,
            "case {case:?}: a delete never re-mints the layoutId, got {after:?}"
        );
        assert_eq!(
            string_at(&after, "layoutArn", case),
            layout_arn,
            "case {case:?}: a delete never re-mints the layoutArn, got {after:?}"
        );
        assert_eq!(
            string_at(&after, "name", case),
            name,
            "case {case:?}: a deleted layout keeps the name it was created with, got {after:?}"
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

        // A read reports; two reads of a deleted layout answer the same body.
        let again = get_layout(&emulator, case, &domain_id, &layout_id).await;
        assert_eq!(
            again, after,
            "case {case:?}: two GetLayout calls must answer the same body, got {again:?}"
        );
    }
}

#[tokio::test]
async fn repeating_the_delete_succeeds_and_changes_nothing() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-delete-idempotent-domain").await;
    let fields = text_fields(&emulator, &domain_id, 1).await;

    // A row per number of extra deletes after the first one: the operation is
    // `@idempotent`, so the record a reader sees is the record the FIRST delete
    // left, whatever the repeat count.
    for extra in [0_usize, 1, 2, 5] {
        let case = format!("{} delete(s) of one layout", extra + 1);
        let (layout_id, _) = create_layout(
            &emulator,
            &case,
            &domain_id,
            &format!("idempotent-layout-{extra}"),
            &top_panel(&fields),
        )
        .await;

        delete_layout(&emulator, &case, &domain_id, &layout_id).await;
        let after_first = get_layout(&emulator, &case, &domain_id, &layout_id).await;
        for repeat in 0..extra {
            let repeated = delete_layout(&emulator, &case, &domain_id, &layout_id).await;
            assert_eq!(
                repeated,
                json!({}),
                "case {case:?}: repeat {repeat} must also answer HTTP 200 with no output \
                 members, got {repeated:?}"
            );
        }

        let after_repeats = get_layout(&emulator, &case, &domain_id, &layout_id).await;
        assert_eq!(
            after_repeats, after_first,
            "case {case:?}: a repeat delete must leave the record exactly as the first \
             delete left it ({after_first:?}), got {after_repeats:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_layout_refuses_every_form_of_update() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-delete-update-domain").await;
    let fields = text_fields(&emulator, &domain_id, 2).await;

    // A row per shape of update request: whichever members it carries, an
    // update of a DELETED layout is refused.
    let rows: Vec<(&str, Value)> = vec![
        ("UpdateLayout carrying neither member", json!({})),
        (
            "UpdateLayout carrying only a name",
            json!({ "name": "renamed-after-delete" }),
        ),
        (
            "UpdateLayout carrying only content",
            json!({ "content": top_panel(&fields[1..2]) }),
        ),
        (
            "UpdateLayout carrying both members",
            json!({ "name": "renamed-again", "content": top_panel(&fields[0..1]) }),
        ),
    ];

    for (index, (case, body)) in rows.into_iter().enumerate() {
        let created_name = format!("update-after-delete-{index}");
        let created_fields = &fields[0..1];
        let (layout_id, _) = create_layout(
            &emulator,
            case,
            &domain_id,
            &created_name,
            &top_panel(created_fields),
        )
        .await;
        delete_layout(&emulator, case, &domain_id, &layout_id).await;

        let (status, code) = emulator
            .failure(
                case,
                "PUT",
                &format!("/domains/{domain_id}/layouts/{layout_id}"),
                Some(body),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException for an update of a \
             deleted layout, got {status} {code}"
        );

        // The refused update wrote nothing: the deleted record is unchanged.
        let fetched = get_layout(&emulator, case, &domain_id, &layout_id).await;
        let expected_content = top_panel_reported(created_fields);
        assert_eq!(
            string_at(&fetched, "name", case),
            created_name,
            "case {case:?}: a refused update must not rename the deleted layout, got {fetched:?}"
        );
        assert_eq!(
            fetched.get("content"),
            Some(&expected_content),
            "case {case:?}: a refused update must not rewrite the content, got {fetched:?}"
        );
        assert_eq!(
            fetched.get("deleted"),
            Some(&json!(true)),
            "case {case:?}: the layout must still report deleted=true, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_layout_it_names() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-delete-siblings-domain").await;
    let other_domain = domain(&emulator, "layout-delete-other-domain").await;
    let fields = text_fields(&emulator, &domain_id, 1).await;
    let content = top_panel(&fields);

    // Three sibling layouts of one domain, plus one of ANOTHER domain holding
    // the same name: a row per which sibling is deleted, so the position of the
    // target among its siblings cannot matter.
    for target in 0..3_usize {
        let case = format!("a delete of sibling {target} among three sibling layouts");
        let case = case.as_str();
        let mut siblings = Vec::new();
        for index in 0..3 {
            let (layout_id, _) = create_layout(
                &emulator,
                case,
                &domain_id,
                &format!("sibling-{target}-{index}"),
                &content,
            )
            .await;
            siblings.push(layout_id);
        }
        let (elsewhere, _) = create_layout(
            &emulator,
            case,
            &other_domain,
            &format!("sibling-{target}-{target}"),
            &json!({ "basic": {} }),
        )
        .await;

        delete_layout(&emulator, case, &domain_id, &siblings[target]).await;

        for (index, layout_id) in siblings.iter().enumerate() {
            let fetched = get_layout(&emulator, case, &domain_id, layout_id).await;
            let expected = index == target;
            assert_eq!(
                fetched.get("deleted"),
                Some(&json!(expected)),
                "case {case:?}: deleting sibling {target} must leave sibling {index} \
                 deleted={expected}, got {fetched:?}"
            );
        }
        // The same-named layout of another domain is a different resource.
        let fetched = get_layout(&emulator, case, &other_domain, &elsewhere).await;
        assert_eq!(
            fetched.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: a delete in one domain must not touch a same-named layout of \
             another domain, got {fetched:?}"
        );

        // A live sibling still accepts an update, so the delete did not flag
        // the domain rather than the layout.
        let live = siblings.iter().enumerate().find(|(index, _)| *index != target);
        let (_, live_id) = live.expect("three siblings leave a live one");
        emulator
            .ok(
                case,
                "PUT",
                &format!("/domains/{domain_id}/layouts/{live_id}"),
                json!({ "name": format!("sibling-{target}-renamed") }),
            )
            .await;
    }
}

#[tokio::test]
async fn deleting_a_layout_frees_its_name_for_a_new_one() {
    // A layout name "must be unique for the Cases domain"
    // (`smithy_query(query="doc", shape="CreateLayoutRequest", member="name")`),
    // and a deleted layout is no longer one of the domain's live layouts, so the
    // name it held is available again — and the new layout is its own record.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-delete-name-domain").await;
    let case = "CreateLayout under the name of a deleted layout";

    let (first, _) = create_layout(
        &emulator,
        case,
        &domain_id,
        "recycled-name",
        &json!({ "basic": {} }),
    )
    .await;
    let (status, code) = emulator
        .failure(
            case,
            "POST",
            &format!("/domains/{domain_id}/layouts"),
            Some(json!({ "name": "recycled-name", "content": { "basic": {} } })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: while the first layout is live its name is taken, got {status} {code}"
    );

    delete_layout(&emulator, case, &domain_id, &first).await;
    let (second, _) = create_layout(
        &emulator,
        case,
        &domain_id,
        "recycled-name",
        &json!({ "basic": {} }),
    )
    .await;
    assert_ne!(
        second, first,
        "case {case:?}: the new layout must be its own record, got the deleted id {first:?}"
    );
    let reused = get_layout(&emulator, case, &domain_id, &second).await;
    assert_eq!(
        reused.get("deleted"),
        Some(&json!(false)),
        "case {case:?}: the new layout is live, got {reused:?}"
    );
    let old = get_layout(&emulator, case, &domain_id, &first).await;
    assert_eq!(
        old.get("deleted"),
        Some(&json!(true)),
        "case {case:?}: the deleted layout stays deleted, got {old:?}"
    );
}

#[tokio::test]
async fn a_delete_of_an_id_the_store_does_not_hold_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-delete-refusal-domain").await;
    let other_domain = domain(&emulator, "layout-delete-refusal-other-domain").await;
    let (layout_id, _) = create_layout(
        &emulator,
        "CreateLayout for the refusal rows",
        &domain_id,
        "the-only-layout",
        &json!({ "basic": {} }),
    )
    .await;

    // A row per reference the store cannot resolve: the domain, the layout, and
    // a layout of a DIFFERENT domain (a layout is scoped to its domain).
    let rows: Vec<RefusalRow> = vec![
        (
            "DeleteLayout in a domain the store does not hold",
            format!("/domains/no-such-domain/layouts/{layout_id}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteLayout for a layout the domain does not hold",
            format!("/domains/{domain_id}/layouts/no-such-layout"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteLayout for a layout of another domain",
            format!("/domains/{other_domain}/layouts/{layout_id}"),
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

    // Every refusal wrote nothing: the one stored layout is still live.
    let case = "GetLayout after the refused deletes";
    let still_live = get_layout(&emulator, case, &domain_id, &layout_id).await;
    assert_eq!(
        still_live.get("deleted"),
        Some(&json!(false)),
        "case {case:?}: a refused delete must not flag the stored layout, got {still_live:?}"
    );
}
