// SPDX-License-Identifier: CC0-1.0

//! `BatchDisassociateApprovalRuleTemplateFromRepositories` over the wire, driven as
//! awsJson1_1 requests through the router that serves the real emulator (the wire
//! harness comes from the sibling `repository_lifecycle` suite rather than a second
//! copy).
//!
//! The rule under test: for each DISTINCT repository name the request lists, in
//! request order, the call releases the ONE stored association between that
//! repository and the named template — the same record the single-resource
//! `AssociateApprovalRuleTemplateWithRepository` writes and
//! `DisassociateApprovalRuleTemplateFromRepository` releases — and reports the name
//! in `disassociatedRepositoryNames`, or records nothing for it and reports one
//! `errors` entry naming it, its cause's `errorCode` and that cause's message; the
//! template is the whole call's precondition, so a name no template holds refuses
//! the request and releases nothing.
//!
//! Because the association is the one stored record, releasing the last one is what
//! makes the template deletable, and a template addressed by the name a rename moved
//! it to is the same template — so the release, the listings and the delete all
//! follow the stored state rather than the name the association was made under.
//!
//! The tables below span what the rule generalises over — how many names a request
//! carries, their order, whether each is held or absent, whether it was associated at
//! all, repeated names, the empty array, and the published limit on the number of
//! names — rather than one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::approval_rule_templates::document;
use crate::repository_lifecycle::{emulator, error, ok, string_at};

/// "The maximum number of allowed repository names was exceeded. Currently, this
/// number is 100." — `MaximumRepositoryNamesExceededException`'s own documentation
/// (`smithy_query(query="shape",shape="MaximumRepositoryNamesExceededException")`).
const MAX_REPOSITORY_NAMES: usize = 100;

/// The template this suite associates and releases, and a second one so a
/// repository can hold an association the rows must leave alone.
const TEMPLATE: &str = "batch-release-template";
const OTHER_TEMPLATE: &str = "untouched-template";

/// The repositories this suite seeds, spanning `RepositoryName`'s modeled bounds
/// (`length` 1..100, pattern `^[\w\.-]+$`).
fn seeded_repositories() -> Vec<String> {
    vec![
        "zeta-repo".to_string(),
        "mid.repo_1".to_string(),
        "a".to_string(),
        "x".repeat(100),
    ]
}

/// The list at `member`, with a missing member read as the empty list.
fn list_at(payload: &Value, member: &str) -> Vec<Value> {
    payload
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The strings of a list of JSON strings.
fn strings_of(values: &[Value], case: &str) -> Vec<String> {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a name is not a string in {value}"))
                .to_string()
        })
        .collect()
}

/// One `errors` entry, read as (`repositoryName`, `errorCode`, `errorMessage`).
fn error_entry(entry: &Value, case: &str) -> (String, String, String) {
    let read = |member: &str| -> String {
        entry
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no {member} in {entry}"))
            .to_string()
    };
    (
        read("repositoryName"),
        read("errorCode"),
        read("errorMessage"),
    )
}

/// `(repositoryName, errorCode, errorMessage)` of every `errors` entry, in answer
/// order.
fn error_entries(payload: &Value, case: &str) -> Vec<(String, String, String)> {
    list_at(payload, "errors")
        .iter()
        .map(|entry| error_entry(entry, case))
        .collect()
}

/// The entry a repository name the account holds no repository for must produce:
/// the name, the code for that cause, and the sentence the service words it with.
fn absent_entry(name: &str) -> (String, String, String) {
    (
        name.to_string(),
        "RepositoryDoesNotExistException".to_string(),
        format!("No repository with the name {name} was found. Please specify a valid repository name"),
    )
}

/// The template names `ListAssociatedApprovalRuleTemplatesForRepository` reports for
/// one repository.
async fn templates_of(router: &MultiServiceRouter, repository: &str) -> Vec<String> {
    let payload = ok(
        router,
        "ListAssociatedApprovalRuleTemplatesForRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    strings_of(&list_at(&payload, "approvalRuleTemplateNames"), repository)
}

/// The repository names `ListRepositoriesForApprovalRuleTemplate` reports for one
/// template.
async fn repositories_of(router: &MultiServiceRouter, template: &str) -> Vec<String> {
    let payload = ok(
        router,
        "ListRepositoriesForApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": template }),
    )
    .await;
    strings_of(&list_at(&payload, "repositoryNames"), template)
}

/// Create one template, answering the `approvalRuleTemplateId` the service minted.
async fn create_template(router: &MultiServiceRouter, name: &str, approvals: u32) -> String {
    let created = ok(
        router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": name,
            "approvalRuleTemplateContent": document(approvals),
        }),
    )
    .await;
    string_at(&created, "/approvalRuleTemplate/approvalRuleTemplateId")
}

/// Every seeded repository associated with [`TEMPLATE`], plus one repository holding
/// [`OTHER_TEMPLATE`] so a row's release cannot reach another template's record.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    for repository in seeded_repositories() {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
    }
    create_template(&router, TEMPLATE, 1).await;
    create_template(&router, OTHER_TEMPLATE, 2).await;
    for repository in seeded_repositories() {
        for template in [TEMPLATE, OTHER_TEMPLATE] {
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
    }
    router
}

/// One row of the batch table: the names one request lists, and the names the
/// service must report as released (in that order) and as failed.
#[derive(Debug)]
struct BatchCase {
    label: &'static str,
    requested: Vec<String>,
    released: Vec<String>,
    failed: Vec<String>,
}

fn batch_cases(held: &[String]) -> Vec<BatchCase> {
    vec![
        BatchCase {
            label: "one held name",
            requested: vec![held[0].clone()],
            released: vec![held[0].clone()],
            failed: Vec::new(),
        },
        BatchCase {
            label: "every held name, in an order that is not the store's",
            requested: held.iter().rev().cloned().collect(),
            released: held.iter().rev().cloned().collect(),
            failed: Vec::new(),
        },
        BatchCase {
            label: "held and absent names interleaved",
            requested: vec![
                "absent-one".to_string(),
                held[2].clone(),
                "absent.two".to_string(),
                held[1].clone(),
            ],
            released: vec![held[2].clone(), held[1].clone()],
            failed: vec!["absent-one".to_string(), "absent.two".to_string()],
        },
        BatchCase {
            label: "a repeated name names one repository",
            requested: vec![
                held[0].clone(),
                held[0].clone(),
                "absent-one".to_string(),
                "absent-one".to_string(),
            ],
            released: vec![held[0].clone()],
            failed: vec!["absent-one".to_string()],
        },
        BatchCase {
            label: "only absent names",
            requested: vec!["absent-one".to_string(), "absent-three".to_string()],
            released: Vec::new(),
            failed: vec!["absent-one".to_string(), "absent-three".to_string()],
        },
        BatchCase {
            // `repositoryNames`'s own documentation: "The array itself can be empty."
            label: "the empty array",
            requested: Vec::new(),
            released: Vec::new(),
            failed: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn each_name_is_released_or_reported_as_its_own_error() {
    let held = seeded_repositories();

    for case in batch_cases(&held) {
        // A fresh store per row, so each row's answer is about that row's names.
        let router = seeded().await;
        let label = case.label;

        let payload = ok(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": case.requested,
            }),
        )
        .await;

        let answered = strings_of(&list_at(&payload, "disassociatedRepositoryNames"), label);
        assert_eq!(
            answered, case.released,
            "case {label:?}: expected {:?}, got {answered:?}",
            case.released
        );

        let expected_errors: Vec<(String, String, String)> =
            case.failed.iter().map(|name| absent_entry(name)).collect();
        let errors = error_entries(&payload, label);
        assert_eq!(
            errors, expected_errors,
            "case {label:?}: expected {expected_errors:?}, got {errors:?}"
        );

        // ROUND TRIP: what the call released is what both listings read back — the
        // repositories it reported, and no others; and the OTHER template's
        // associations are untouched, because the release names one template.
        let listed = repositories_of(&router, TEMPLATE).await;
        let mut expected: Vec<String> = held
            .iter()
            .filter(|name| !case.released.contains(name))
            .cloned()
            .collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {label:?}: the template must still hold {expected:?}, got {listed:?}"
        );
        let mut untouched = held.clone();
        untouched.sort();
        assert_eq!(
            repositories_of(&router, OTHER_TEMPLATE).await,
            untouched,
            "case {label:?}: the other template keeps every association"
        );
        for repository in &held {
            let templates = templates_of(&router, repository).await;
            let expected: Vec<String> = if case.released.contains(repository) {
                vec![OTHER_TEMPLATE.to_string()]
            } else {
                vec![TEMPLATE.to_string(), OTHER_TEMPLATE.to_string()]
            };
            assert_eq!(
                templates, expected,
                "case {label:?}: {repository:?} must hold {expected:?}, got {templates:?}"
            );
        }

        // The same request again is the same answer and the same stored state:
        // releasing an association that is not there is a no-op, so the name is
        // reported released again and nothing else moves.
        let again = ok(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": case.requested,
            }),
        )
        .await;
        assert_eq!(
            again, payload,
            "case {label:?}: a repeated call must answer the same, got {again}"
        );
        let relisted = repositories_of(&router, TEMPLATE).await;
        assert_eq!(
            relisted, expected,
            "case {label:?}: a repeated call must change nothing, got {relisted:?}"
        );
    }
}

#[tokio::test]
async fn releasing_the_last_association_lets_the_template_be_deleted() {
    // The rows differ along how MANY repositories hold the template and in how many
    // calls the batch releases them, which is what the rule generalises over.
    for (case, repositories, per_call) in [
        ("one repository, one call", 1usize, 1usize),
        ("three repositories, one call", 3, 3),
        ("four repositories, two at a time", 4, 2),
    ] {
        let router = emulator();
        let names: Vec<String> = (0..repositories)
            .map(|index| format!("release-repo-{index}"))
            .collect();
        for name in &names {
            ok(
                &router,
                "CreateRepository",
                &json!({ "repositoryName": name }),
            )
            .await;
        }
        let template_id = create_template(&router, TEMPLATE, 1).await;
        ok(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
            &json!({ "approvalRuleTemplateName": TEMPLATE, "repositoryNames": names }),
        )
        .await;

        // While ANY association remains the template cannot be deleted, so the
        // release is what the delete depends on.
        let code = error(
            &router,
            "DeleteApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": TEMPLATE }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateInUseException",
            "case {case:?}: an associated template cannot be deleted, got {code:?}"
        );

        for chunk in names.chunks(per_call) {
            let payload = ok(
                &router,
                "BatchDisassociateApprovalRuleTemplateFromRepositories",
                &json!({ "approvalRuleTemplateName": TEMPLATE, "repositoryNames": chunk }),
            )
            .await;
            assert_eq!(
                strings_of(&list_at(&payload, "disassociatedRepositoryNames"), case),
                chunk.to_vec(),
                "case {case:?}: every name in the call must be released, got {payload}"
            );
            assert!(
                list_at(&payload, "errors").is_empty(),
                "case {case:?}: a held name fails nothing, got {payload}"
            );
        }

        let listed = repositories_of(&router, TEMPLATE).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: no repository may hold the template, got {listed:?}"
        );
        let deleted = ok(
            &router,
            "DeleteApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": TEMPLATE }),
        )
        .await;
        assert_eq!(
            deleted
                .get("approvalRuleTemplateId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            template_id,
            "case {case:?}: the delete must report the stored id, got {deleted}"
        );
        let code = error(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": TEMPLATE }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateDoesNotExistException",
            "case {case:?}: the deleted template cannot be read back, got {code:?}"
        );
    }
}

#[tokio::test]
async fn a_renamed_template_is_released_and_deleted_under_its_new_name() {
    // The association is a record of the TEMPLATE, so a rename takes it along: the
    // rows differ along the form of the name the template is renamed to and along how
    // the association was made (the single-resource call or the batch).
    for (case, new_name, batch_associated) in [
        ("an ordinary new name", "renamed-template".to_string(), false),
        ("a one-character new name", "r".to_string(), true),
        ("a new name at the published maximum", "r".repeat(100), false),
        (
            "a new name carrying punctuation",
            "renamed.template_2".to_string(),
            true,
        ),
    ] {
        let router = emulator();
        let repositories = ["renamed-repo-a".to_string(), "renamed-repo-b".to_string()];
        for name in &repositories {
            ok(
                &router,
                "CreateRepository",
                &json!({ "repositoryName": name }),
            )
            .await;
        }
        let template_id = create_template(&router, TEMPLATE, 2).await;
        if batch_associated {
            ok(
                &router,
                "BatchAssociateApprovalRuleTemplateWithRepositories",
                &json!({
                    "approvalRuleTemplateName": TEMPLATE,
                    "repositoryNames": repositories,
                }),
            )
            .await;
        } else {
            for name in &repositories {
                ok(
                    &router,
                    "AssociateApprovalRuleTemplateWithRepository",
                    &json!({ "approvalRuleTemplateName": TEMPLATE, "repositoryName": name }),
                )
                .await;
            }
        }

        let renamed = ok(
            &router,
            "UpdateApprovalRuleTemplateName",
            &json!({
                "oldApprovalRuleTemplateName": TEMPLATE,
                "newApprovalRuleTemplateName": new_name,
            }),
        )
        .await;
        assert_eq!(
            string_at(&renamed, "/approvalRuleTemplate/approvalRuleTemplateId"),
            template_id,
            "case {case:?}: a rename keeps the template's id, got {renamed}"
        );

        // The new name addresses the template AND its associations; the old name
        // addresses nothing at all.
        let mut expected = repositories.to_vec();
        expected.sort();
        assert_eq!(
            repositories_of(&router, &new_name).await,
            expected,
            "case {case:?}: the renamed template keeps its associations"
        );
        for name in &repositories {
            assert_eq!(
                templates_of(&router, name).await,
                vec![new_name.clone()],
                "case {case:?}: {name:?} must hold the template under its new name"
            );
        }
        let code = error(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": repositories,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateDoesNotExistException",
            "case {case:?}: the pre-rename name names no template, got {code:?}"
        );
        assert_eq!(
            repositories_of(&router, &new_name).await,
            expected,
            "case {case:?}: the refused release must persist nothing"
        );

        // Released by the new name, the template is deletable by the new name, and
        // the delete reports the id the create minted.
        let payload = ok(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &json!({
                "approvalRuleTemplateName": new_name,
                "repositoryNames": repositories,
            }),
        )
        .await;
        assert_eq!(
            strings_of(&list_at(&payload, "disassociatedRepositoryNames"), case),
            repositories.to_vec(),
            "case {case:?}: both names must be released, got {payload}"
        );
        let deleted = ok(
            &router,
            "DeleteApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": new_name }),
        )
        .await;
        assert_eq!(
            deleted
                .get("approvalRuleTemplateId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            template_id,
            "case {case:?}: the delete must report the stored id, got {deleted}"
        );
        for name in &repositories {
            let templates = templates_of(&router, name).await;
            assert!(
                templates.is_empty(),
                "case {case:?}: {name:?} must hold no association, got {templates:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_template_the_service_does_not_hold_refuses_the_whole_call() {
    let held = seeded_repositories();

    // Rows: the FORM of a name no template holds — never created, the name of a
    // repository rather than a template, a near miss of a stored name, a different
    // case, and a name at the published 100-character maximum.
    for case in [
        "no-such-template".to_string(),
        held[0].clone(),
        format!("{TEMPLATE}s"),
        TEMPLATE.to_uppercase(),
        "n".repeat(100),
    ] {
        let router = seeded().await;
        let code = error(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &json!({
                "approvalRuleTemplateName": case,
                "repositoryNames": held.clone(),
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateDoesNotExistException",
            "case {case:?}: expected ApprovalRuleTemplateDoesNotExistException, got {code:?}"
        );
        // The refusal released nothing: every seeded association is still stored.
        for repository in &held {
            let templates = templates_of(&router, repository).await;
            assert_eq!(
                templates,
                vec![TEMPLATE.to_string(), OTHER_TEMPLATE.to_string()],
                "case {case:?}: a refused call must persist nothing, {repository:?} holds \
                 {templates:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_bad_request_is_refused_with_the_error_modelled_for_its_member() {
    let router = seeded().await;
    let held = seeded_repositories();

    // `approvalRuleTemplateName` is `@required` and 1..100 characters, and
    // `repositoryNames` is `@required`
    // (`smithy_query(query="operation",
    //   operation="BatchDisassociateApprovalRuleTemplateFromRepositories")`), so each
    // form of an unusable request has its own modelled error. A name of nothing but
    // whitespace, or one carrying a control character, names no template either:
    // "Template names must be between 1 and 100 valid characters in length."
    for (case, body, expected) in [
        (
            "no template name",
            json!({ "repositoryNames": held.clone() }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "an empty template name",
            json!({ "approvalRuleTemplateName": "", "repositoryNames": held.clone() }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "a name of nothing but spaces",
            json!({ "approvalRuleTemplateName": "   ", "repositoryNames": held.clone() }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a name carrying a control character",
            json!({
                "approvalRuleTemplateName": format!("{TEMPLATE}\u{7}"),
                "repositoryNames": held.clone(),
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "one character past the published maximum",
            json!({
                "approvalRuleTemplateName": "n".repeat(101),
                "repositoryNames": held.clone(),
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "no repositoryNames member",
            json!({ "approvalRuleTemplateName": TEMPLATE }),
            "RepositoryNamesRequiredException",
        ),
    ] {
        let code = error(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &body,
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?}"
        );
    }

    // Every refusal above left the store exactly as it was.
    for repository in &held {
        let templates = templates_of(&router, repository).await;
        assert_eq!(
            templates,
            vec![TEMPLATE.to_string(), OTHER_TEMPLATE.to_string()],
            "case \"after the refusals\": {repository:?} must keep its associations, \
             got {templates:?}"
        );
    }
}

#[tokio::test]
async fn the_number_of_names_is_bounded_by_the_published_limit() {
    let router = seeded().await;
    let held = seeded_repositories();
    // Each request carries one held name and fills the rest with names the account
    // holds no repository for, so a count is reported entry by entry.
    let names = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| {
                if index == 0 {
                    held[0].clone()
                } else {
                    format!("absent-{index}")
                }
            })
            .collect()
    };

    // Each count at or below the limit is served — every name reported exactly once,
    // either released or as its own error — and the first count past it is refused.
    for count in [1usize, MAX_REPOSITORY_NAMES - 1, MAX_REPOSITORY_NAMES] {
        let label = format!("{count} names");
        let payload = ok(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": names(count),
            }),
        )
        .await;
        let released = list_at(&payload, "disassociatedRepositoryNames").len();
        let failed = list_at(&payload, "errors").len();
        assert_eq!(
            released + failed,
            count,
            "case {label:?}: every name must be reported exactly once, got {payload}"
        );
        assert_eq!(
            released, 1,
            "case {label:?}: the one held repository must be released, got {payload}"
        );
    }
    for count in [MAX_REPOSITORY_NAMES + 1, 2 * MAX_REPOSITORY_NAMES] {
        let label = format!("{count} names");
        let code = error(
            &router,
            "BatchDisassociateApprovalRuleTemplateFromRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": names(count),
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "MaximumRepositoryNamesExceededException",
            "case {label:?}: past the limit the call must be refused, got {code:?}"
        );
    }

    // The refused calls changed nothing: the only association released is the one the
    // served counts released, and every other seeded association is still stored.
    let mut expected: Vec<String> = held[1..].to_vec();
    expected.sort();
    let listed = repositories_of(&router, TEMPLATE).await;
    assert_eq!(
        listed, expected,
        "case \"after the refused counts\": expected {expected:?}, got {listed:?}"
    );
}
