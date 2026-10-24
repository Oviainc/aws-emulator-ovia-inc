// SPDX-License-Identifier: CC0-1.0

//! `DeleteApprovalRuleTemplate` over the wire, as awsJson1_1 requests through the
//! router that serves the real emulator (the harness lives in
//! `repository_lifecycle`, which this suite shares rather than rebuilding).
//!
//! The rules under test:
//!
//! 1. Deleting an approval rule template removes the STORED record the name
//!    addresses and reports that record's own `approvalRuleTemplateId`: every later
//!    read of the name is refused with `ApprovalRuleTemplateDoesNotExistException`,
//!    the template is gone from `ListApprovalRuleTemplates`, every other template is
//!    untouched, and the name is free for a new template again.
//! 2. The delete is idempotent and models no does-not-exist error: a name no
//!    template holds is answered 200 carrying no id, and the store is left exactly as
//!    it was ("If the template has been previously deleted, the only response is a
//!    200 OK").
//! 3. A template that any repository is associated with cannot be deleted: the call
//!    is refused with `ApprovalRuleTemplateInUseException` (HTTP 400) while ANY
//!    association remains, persisting nothing, and succeeds once the last one is
//!    released. The refusal's message is one fixed sentence group carrying no value
//!    from the request.
//! 4. A name that can name no template at all is refused with the error this model
//!    declares for that cause, and persists nothing.
//!
//! Each table spans the class its rule covers — name lengths and forms, description
//! and content shapes, deletion order, how many repositories hold a template, and
//! the forms an unusable name takes — rather than one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::approval_rule_templates::document;
use crate::repository_lifecycle::{emulator, error, is_uuid, ok, string_at};

/// One row of the deletion table: a template as it is created.
#[derive(Debug, Clone)]
struct Template {
    case: &'static str,
    name: String,
    description: Option<String>,
    content: String,
}

/// The templates rule 1 is asserted over. The rows differ along what the rule
/// generalises over: the length and form of the name (one character, ordinary, the
/// published 100-character maximum of `ApprovalRuleTemplateName`, punctuation,
/// non-ASCII), whether a description was stored, and the content document (how many
/// approvals it asks for).
fn deletion_table() -> Vec<Template> {
    vec![
        Template {
            case: "a one-character name",
            name: "a".to_string(),
            description: None,
            content: document(1),
        },
        Template {
            case: "an ordinary name with a description",
            name: "release-approvals".to_string(),
            description: Some("two reviewers on every pull request".to_string()),
            content: document(2),
        },
        Template {
            case: "a name at the published maximum of 100",
            name: "n".repeat(100),
            description: Some("d".repeat(1000)),
            content: document(5),
        },
        Template {
            case: "a name carrying punctuation",
            name: "tmpl.with-dots_1".to_string(),
            description: None,
            content: document(3),
        },
        Template {
            case: "a non-ASCII name and description",
            name: "plantilla-revisión".to_string(),
            description: Some("revisión obligatoria ⚙".to_string()),
            content: document(4),
        },
    ]
}

/// Create one template and answer the `approvalRuleTemplateId` the service minted
/// for it.
async fn create(router: &MultiServiceRouter, row: &Template) -> String {
    let mut body = json!({
        "approvalRuleTemplateName": row.name,
        "approvalRuleTemplateContent": row.content,
    });
    if let Some(description) = &row.description {
        body["approvalRuleTemplateDescription"] = json!(description);
    }
    let created = ok(router, "CreateApprovalRuleTemplate", &body).await;
    string_at(&created, "/approvalRuleTemplate/approvalRuleTemplateId")
}

/// Every template name the store holds, as `ListApprovalRuleTemplates` reports it.
async fn stored_names(router: &MultiServiceRouter) -> Vec<String> {
    let payload = ok(router, "ListApprovalRuleTemplates", &json!({})).await;
    payload
        .get("approvalRuleTemplateNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no approvalRuleTemplateNames in {payload}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("a name is not a string in {payload}"))
                .to_string()
        })
        .collect()
}

/// Delete one template name and answer the id the call reported, which is empty
/// when the call removed nothing.
async fn delete(router: &MultiServiceRouter, name: &str) -> String {
    let payload = ok(
        router,
        "DeleteApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": name }),
    )
    .await;
    payload
        .get("approvalRuleTemplateId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn deleting_a_template_removes_it_and_reports_the_stored_id() {
    let router = emulator();
    let table = deletion_table();

    let mut ids: Vec<String> = Vec::new();
    for row in &table {
        ids.push(create(&router, row).await);
    }

    // Delete in an order that is NOT the creation order, so each delete is checked
    // against a store that still holds the others.
    let order: Vec<usize> = (0..table.len())
        .filter(|index| index % 2 == 1)
        .chain((0..table.len()).filter(|index| index % 2 == 0))
        .collect();
    let mut remaining: Vec<String> = table.iter().map(|row| row.name.clone()).collect();

    for index in order {
        let row = &table[index];
        let reported = delete(&router, &row.name).await;
        assert_eq!(
            reported, ids[index],
            "case {:?}: the delete must report the stored id, got {reported:?}",
            row.case
        );
        assert!(
            is_uuid(&reported),
            "case {:?}: the reported id must be the stored template id, got {reported:?}",
            row.case
        );

        // The record is gone from state, so every later read of the name is refused.
        let code = error(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": row.name }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateDoesNotExistException",
            "case {:?}: a deleted template cannot be read back, got {code:?}",
            row.case
        );

        // And exactly the templates not yet deleted remain.
        remaining.retain(|name| *name != row.name);
        let mut expected = remaining.clone();
        expected.sort();
        let listed = stored_names(&router).await;
        assert_eq!(
            listed, expected,
            "case {:?}: after the delete the store must hold {expected:?}, got {listed:?}",
            row.case
        );
    }

    // The name a delete released names no template, so it can hold a new one: the
    // record that comes back is the NEW template's, content and all.
    let reborn = &table[0];
    let fresh_id = create(
        &router,
        &Template {
            case: reborn.case,
            name: reborn.name.clone(),
            description: Some("created again after the delete".to_string()),
            content: document(7),
        },
    )
    .await;
    let fetched = ok(
        &router,
        "GetApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": reborn.name }),
    )
    .await;
    assert_eq!(
        string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateId"),
        fresh_id,
        "case \"a name reused after its template was deleted\": the read must be the new \
         template, got {fetched}"
    );
    assert_eq!(
        string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateContent"),
        document(7),
        "case \"a name reused after its template was deleted\": the new content must be \
         stored, got {fetched}"
    );
}

#[tokio::test]
async fn deleting_a_name_no_template_holds_answers_200_and_changes_nothing() {
    let router = emulator();

    // Templates the rows must leave exactly as they are.
    let guarded = Template {
        case: "a stored template",
        name: "guarded".to_string(),
        description: Some("not the target of any row below".to_string()),
        content: document(2),
    };
    let guarded_id = create(&router, &guarded).await;
    let once = Template {
        case: "a template deleted once",
        name: "deleted-once".to_string(),
        description: None,
        content: document(1),
    };
    let once_id = create(&router, &once).await;
    let reported = delete(&router, &once.name).await;
    assert_eq!(
        reported, once_id,
        "case {:?}: the first delete reports the stored id, got {reported:?}",
        once.case
    );

    let before = stored_names(&router).await;

    // Rows: the forms a name that holds no template takes — never created, already
    // deleted, a near miss of a stored name, a stored name in another case, a name at
    // the published maximum length, and a one-character name.
    let rows: [(&str, String); 6] = [
        ("a name never created", "no-such-template".to_string()),
        ("a name already deleted", once.name.clone()),
        ("a near miss of a stored name", "guardeds".to_string()),
        ("a stored name in another case", "GUARDED".to_string()),
        ("a name at the published maximum", "z".repeat(100)),
        ("a one-character name", "q".to_string()),
    ];

    for (case, name) in rows {
        let reported = delete(&router, &name).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a delete that removed nothing reports no id, got {reported:?}"
        );
        let listed = stored_names(&router).await;
        assert_eq!(
            listed, before,
            "case {case:?}: the store must be unchanged ({before:?}), got {listed:?}"
        );
    }

    // And the stored template is untouched: same id, same description.
    let fetched = ok(
        &router,
        "GetApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": guarded.name }),
    )
    .await;
    assert_eq!(
        string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateId"),
        guarded_id,
        "case \"a stored template\": it must survive every delete of another name, got {fetched}"
    );
    assert_eq!(
        fetched
            .pointer("/approvalRuleTemplate/approvalRuleTemplateDescription")
            .and_then(Value::as_str),
        guarded.description.as_deref(),
        "case \"a stored template\": its description must survive, got {fetched}"
    );
}

/// The in-use table: one row per template, with the repositories associated with it.
/// The rows differ along how MANY repositories hold the template, which is what the
/// rule generalises over; each row owns its repositories, so releasing one row's
/// associations cannot release another's.
fn in_use_table() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        ("one repository", "template-lone", vec!["lone-repo"]),
        (
            "two repositories",
            "template-pair",
            vec!["pair-repo-a", "pair-repo-b"],
        ),
        (
            "three repositories",
            "template-trio",
            vec!["trio-repo-a", "trio-repo-b", "trio-repo-c"],
        ),
    ]
}

#[tokio::test]
async fn a_template_a_repository_is_associated_with_cannot_be_deleted_until_released() {
    let router = emulator();
    let mut messages: Vec<String> = Vec::new();

    for (case, template, repositories) in in_use_table() {
        let id = create(
            &router,
            &Template {
                case,
                name: template.to_string(),
                description: None,
                content: document(1),
            },
        )
        .await;
        for repository in &repositories {
            ok(
                &router,
                "CreateRepository",
                &json!({ "repositoryName": repository }),
            )
            .await;
            ok(
                &router,
                "AssociateApprovalRuleTemplateWithRepository",
                &json!({
                    "approvalRuleTemplateName": template,
                    "repositoryName": repository,
                }),
            )
            .await;
        }

        // Releasing the associations one at a time: the delete is refused while ANY
        // of them remains, and the refusal persists nothing — the template is still
        // readable with the same id and still associated with every repository left.
        let mut left = repositories.clone();
        while !left.is_empty() {
            let (code, message) = crate::wire::refusal_as(
                &router,
                crate::wire::DEFAULT_ACCESS_KEY_ID,
                "DeleteApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": template }),
                400,
            )
            .await;
            assert_eq!(
                code, "ApprovalRuleTemplateInUseException",
                "case {case:?}: with {} association(s) left, expected \
                 ApprovalRuleTemplateInUseException, got {code:?}",
                left.len()
            );
            messages.push(message);

            let fetched = ok(
                &router,
                "GetApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": template }),
            )
            .await;
            assert_eq!(
                string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateId"),
                id,
                "case {case:?}: a refused delete leaves the template stored, got {fetched}"
            );
            let still = ok(
                &router,
                "ListRepositoriesForApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": template }),
            )
            .await;
            let mut expected: Vec<String> = left.iter().map(|name| (*name).to_string()).collect();
            expected.sort();
            let answered: Vec<String> = still
                .get("repositoryNames")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no repositoryNames in {still}"))
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect();
            assert_eq!(
                answered, expected,
                "case {case:?}: a refused delete leaves the associations intact, expected \
                 {expected:?}, got {answered:?}"
            );

            // Deleting a repository releases its association with the template, which
            // is the release path this emulator serves end to end.
            let released = left.remove(0);
            ok(
                &router,
                "DeleteRepository",
                &json!({ "repositoryName": released }),
            )
            .await;
        }

        // With the last association released the identical delete succeeds, reports
        // the stored id, and the template is gone.
        let reported = delete(&router, template).await;
        assert_eq!(
            reported, id,
            "case {case:?}: once no repository holds it, the delete reports the stored id, \
             got {reported:?}"
        );
        let code = error(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": template }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateDoesNotExistException",
            "case {case:?}: the released template is deleted, got {code:?}"
        );
    }

    // The refusal is one fixed sentence group: every row is told the same thing, and
    // it carries no value from the request — neither the template nor any repository.
    let first = messages
        .first()
        .expect("every row refuses the delete at least once")
        .clone();
    for message in &messages {
        assert_eq!(
            *message, first,
            "case \"the in-use refusal\": every refusal carries the same message, got \
             {message:?}"
        );
    }
    for named in in_use_table()
        .into_iter()
        .flat_map(|(_, template, repositories)| {
            std::iter::once(template).chain(repositories.into_iter())
        })
    {
        assert!(
            !first.contains(named),
            "case \"the in-use refusal\": the message names no value from the request, \
             but {first:?} contains {named:?}"
        );
    }
    assert!(
        first.contains("associated"),
        "case \"the in-use refusal\": the message must state the cause, got {first:?}"
    );
}

#[tokio::test]
async fn a_name_that_can_name_no_template_is_refused_with_its_modelled_error() {
    let router = emulator();
    let guarded = Template {
        case: "a stored template",
        name: "guarded".to_string(),
        description: None,
        content: document(2),
    };
    let guarded_id = create(&router, &guarded).await;
    let before = stored_names(&router).await;

    // `approvalRuleTemplateName` is `@required` and `@length` 1..100
    // (`smithy_query(query="operation",operation="DeleteApprovalRuleTemplate")`), and
    // "Template names must be between 1 and 100 valid characters in length"
    // (`InvalidApprovalRuleTemplateNameException`). The rows differ along the FORM of
    // the unusable name: absent, empty, whitespace only, carrying a control
    // character, and one character past the published maximum.
    let rows: [(&str, Value, &str); 7] = [
        (
            "no approvalRuleTemplateName at all",
            json!({}),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "a name shorter than the modelled minimum of 1",
            json!({ "approvalRuleTemplateName": "" }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "a name of nothing but spaces",
            json!({ "approvalRuleTemplateName": "   " }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a name of nothing but other whitespace",
            json!({ "approvalRuleTemplateName": "\t\u{b}" }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a name carrying a control character",
            json!({ "approvalRuleTemplateName": "guarded\u{7}" }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a stored name carrying a control character",
            json!({ "approvalRuleTemplateName": "\u{1}guarded" }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a name one character past the modelled maximum of 100",
            json!({ "approvalRuleTemplateName": "n".repeat(101) }),
            "InvalidApprovalRuleTemplateNameException",
        ),
    ];

    for (case, body, expected) in rows {
        let code = error(&router, "DeleteApprovalRuleTemplate", &body, 400).await;
        assert_eq!(
            code, expected, "case {case:?}: expected {expected}, got {code:?}"
        );
        let listed = stored_names(&router).await;
        assert_eq!(
            listed, before,
            "case {case:?}: a refused delete persists nothing, expected {before:?}, got \
             {listed:?}"
        );
    }

    let fetched = ok(
        &router,
        "GetApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": guarded.name }),
    )
    .await;
    assert_eq!(
        string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateId"),
        guarded_id,
        "case \"a stored template\": it survives every refused delete, got {fetched}"
    );
}
