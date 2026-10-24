// SPDX-License-Identifier: CC0-1.0

//! `DisassociateApprovalRuleTemplateFromRepository` over the wire, as awsJson1_1
//! requests through the router that serves the real emulator (the harness lives in
//! `repository_lifecycle`, which this suite shares rather than rebuilding).
//!
//! The rules under test:
//!
//! 1. The call releases the ONE stored association the pair names: afterwards the
//!    template is gone from that repository's
//!    `ListAssociatedApprovalRuleTemplatesForRepository` and the repository is gone
//!    from that template's `ListRepositoriesForApprovalRuleTemplate`, and EVERY other
//!    association in the store — the same template with another repository, another
//!    template with the same repository — is left exactly as it was.
//! 2. Releasing the last association a template has is what makes
//!    `DeleteApprovalRuleTemplate` stop refusing it as in use.
//! 3. The call models no error for a pair that holds no association, so a pair that
//!    was never associated, and a second identical call, are answered 200 and change
//!    nothing: the release is idempotent, and re-associating the pair afterwards
//!    restores exactly the association that was released.
//! 4. A name either side never held is refused with that side's modelled
//!    does-not-exist error, a name that can name nothing at all with the modelled
//!    error for that cause, and a refusal releases nothing.
//!
//! Each table spans the class its rule covers — template name lengths and forms, how
//! many repositories hold a template, how many templates a repository holds, and the
//! order the associations are released in — rather than one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::approval_rule_templates::document;
use crate::repository_lifecycle::{emulator, error, ok};

/// The repositories this suite associates templates with, created in an order that is
/// not their name order.
const REPOSITORIES: [&str; 4] = ["zeta-repo", "mid-repo", "alpha-repo", "solo-repo"];

/// One row of the association table: a template, and the repositories it is
/// associated with.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    template: String,
    /// The content document stored for the template, so a released association is
    /// seen not to have touched the template's own record.
    approvals: u32,
    repositories: Vec<&'static str>,
}

/// The table rule 1 is asserted over. The rows differ along what the rule
/// generalises over: how MANY repositories hold the template (one, two, every one),
/// and the length and form of the template name — one character, ordinary, the
/// published 100-character maximum of `ApprovalRuleTemplateName`
/// (`smithy_query(query="operation",
/// operation="DisassociateApprovalRuleTemplateFromRepository")`), punctuation, and
/// non-ASCII (that member carries a length bound but no pattern).
fn association_table() -> Vec<Row> {
    vec![
        Row {
            case: "a one-character name on one repository",
            template: "t".to_string(),
            approvals: 1,
            repositories: vec!["mid-repo"],
        },
        Row {
            case: "an ordinary name on two repositories",
            template: "release-approvals".to_string(),
            approvals: 2,
            repositories: vec!["zeta-repo", "alpha-repo"],
        },
        Row {
            case: "a name at the published maximum of 100 on every repository",
            template: "n".repeat(100),
            approvals: 3,
            repositories: REPOSITORIES.to_vec(),
        },
        Row {
            case: "a name carrying punctuation on two repositories",
            template: "tmpl.with-dots_1".to_string(),
            approvals: 4,
            repositories: vec!["solo-repo", "mid-repo"],
        },
        Row {
            case: "a non-ASCII name on one repository",
            template: "plantilla-revisión".to_string(),
            approvals: 5,
            repositories: vec!["alpha-repo"],
        },
    ]
}

/// An emulator holding [`REPOSITORIES`] and every template of
/// [`association_table`], associated as the table says.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    for repository in REPOSITORIES {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
    }
    for row in association_table() {
        ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": row.template,
                "approvalRuleTemplateContent": document(row.approvals),
            }),
        )
        .await;
        for repository in &row.repositories {
            ok(
                &router,
                "AssociateApprovalRuleTemplateWithRepository",
                &json!({
                    "approvalRuleTemplateName": row.template,
                    "repositoryName": repository,
                }),
            )
            .await;
        }
    }
    router
}

/// The names one listing answer carries under `member`.
fn names_of(payload: &Value, member: &str, case: &str) -> Vec<String> {
    payload
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {payload}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a name is not a string in {payload}"))
                .to_string()
        })
        .collect()
}

/// Every association the store holds, read through BOTH listings: the templates each
/// repository reports, then the repositories each template reports. Comparing this
/// whole snapshot is what proves a release touched nothing but the pair it named.
async fn snapshot(router: &MultiServiceRouter, case: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for repository in REPOSITORIES {
        let payload = ok(
            router,
            "ListAssociatedApprovalRuleTemplatesForRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        out.push((
            format!("repository/{repository}"),
            names_of(&payload, "approvalRuleTemplateNames", case),
        ));
    }
    for row in association_table() {
        let payload = ok(
            router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": row.template }),
        )
        .await;
        out.push((
            format!("template/{}", row.template),
            names_of(&payload, "repositoryNames", case),
        ));
    }
    out
}

/// Release one association, asserting the call is answered 200 with an empty payload
/// (the operation's output is modelled `Unit`).
async fn disassociate(router: &MultiServiceRouter, template: &str, repository: &str, case: &str) {
    let payload = ok(
        router,
        "DisassociateApprovalRuleTemplateFromRepository",
        &json!({
            "approvalRuleTemplateName": template,
            "repositoryName": repository,
        }),
    )
    .await;
    assert!(
        payload.as_object().is_none_or(serde_json::Map::is_empty),
        "case {case:?}: the release answers an empty payload, got {payload}"
    );
}

/// The expected snapshot for an association matrix: for every repository the
/// templates still associated with it, and for every template the repositories still
/// associated with it, both in name order (what both listings report).
fn expected_snapshot(matrix: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for repository in REPOSITORIES {
        let mut templates: Vec<String> = matrix
            .iter()
            .filter(|(_, repositories)| repositories.iter().any(|name| name == repository))
            .map(|(template, _)| template.clone())
            .collect();
        templates.sort();
        out.push((format!("repository/{repository}"), templates));
    }
    for (template, repositories) in matrix {
        let mut names = repositories.clone();
        names.sort();
        out.push((format!("template/{template}"), names));
    }
    out
}

/// The association matrix as the table seeds it.
fn seeded_matrix() -> Vec<(String, Vec<String>)> {
    association_table()
        .into_iter()
        .map(|row| {
            (
                row.template,
                row.repositories
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<String>>(),
            )
        })
        .collect()
}

#[tokio::test]
async fn releasing_an_association_removes_exactly_that_pair_from_both_listings() {
    let router = seeded().await;
    let mut matrix = seeded_matrix();

    // Release every association, one row at a time and within a row in an order that
    // is not the order it was created in, so each release is checked against a store
    // that still holds the others.
    for (index, row) in association_table().into_iter().enumerate() {
        let mut order: Vec<&str> = row.repositories.clone();
        order.reverse();
        for repository in order {
            disassociate(&router, &row.template, repository, row.case).await;

            matrix[index].1.retain(|name| name != repository);
            let expected = expected_snapshot(&matrix);
            let answered = snapshot(&router, row.case).await;
            assert_eq!(
                answered, expected,
                "case {:?}: after releasing {repository:?} the store must hold {expected:?}, \
                 got {answered:?}",
                row.case
            );
        }

        // The template's own record is untouched: "this does not delete any approval
        // rules previously created ... through the template association".
        let fetched = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": row.template }),
        )
        .await;
        assert_eq!(
            fetched
                .pointer("/approvalRuleTemplate/approvalRuleTemplateContent")
                .and_then(Value::as_str),
            Some(document(row.approvals).as_str()),
            "case {:?}: the release leaves the template's content stored, got {fetched}",
            row.case
        );
    }

    // Every association is released, so BOTH listings are empty for every repository
    // and every template.
    let empty = expected_snapshot(&matrix);
    for (key, names) in &empty {
        assert!(
            names.is_empty(),
            "case \"every association released\": {key} must report nothing, got {names:?}"
        );
    }
}

#[tokio::test]
async fn the_delete_is_refused_until_the_association_is_released() {
    let router = seeded().await;

    for row in association_table() {
        let mut left: Vec<&str> = row.repositories.clone();
        while !left.is_empty() {
            let code = error(
                &router,
                "DeleteApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": row.template }),
                400,
            )
            .await;
            assert_eq!(
                code, "ApprovalRuleTemplateInUseException",
                "case {:?}: with {} association(s) left the delete is refused, got {code:?}",
                row.case,
                left.len()
            );
            let released = left.remove(0);
            disassociate(&router, &row.template, released, row.case).await;
        }
        let deleted = ok(
            &router,
            "DeleteApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": row.template }),
        )
        .await;
        assert!(
            deleted
                .get("approvalRuleTemplateId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {:?}: the identical delete succeeds once the last association is \
             released, got {deleted}",
            row.case
        );
    }
}

#[tokio::test]
async fn releasing_a_pair_that_holds_no_association_changes_nothing() {
    let router = seeded().await;
    let matrix = seeded_matrix();
    let before = expected_snapshot(&matrix);

    // Rows: the forms a pair holding no association takes — a template and a
    // repository that were never associated, a pair whose association this test
    // released first (so the second call is the repeat), and a pair in which each
    // side IS associated, but with something else.
    let first = association_table()[0].clone();
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a pair never associated",
            first.template.clone(),
            "solo-repo",
        ),
        (
            "a pair whose sides are each associated elsewhere",
            association_table()[1].template.clone(),
            "mid-repo",
        ),
        (
            "a non-ASCII template and a repository it never held",
            association_table()[4].template.clone(),
            "zeta-repo",
        ),
    ];

    for (case, template, repository) in &rows {
        // Twice, so the call is seen to be idempotent whichever way round it is read.
        for attempt in 1..=2 {
            disassociate(&router, template, repository, case).await;
            let answered = snapshot(&router, case).await;
            assert_eq!(
                answered, before,
                "case {case:?}: call {attempt} must leave the store at {before:?}, got \
                 {answered:?}"
            );
        }
    }

    // And a pair that DOES hold an association is released once and is then in the
    // same no-op class: the repeat is answered 200 and changes nothing further.
    let held = &first.repositories[0];
    disassociate(&router, &first.template, held, first.case).await;
    let mut released_matrix = matrix.clone();
    released_matrix[0].1.retain(|name| name != held);
    let after = expected_snapshot(&released_matrix);
    for attempt in 1..=2 {
        disassociate(&router, &first.template, held, "the repeated release").await;
        let answered = snapshot(&router, "the repeated release").await;
        assert_eq!(
            answered, after,
            "case \"the repeated release\": call {attempt} must leave the store at {after:?}, \
             got {answered:?}"
        );
    }

    // Re-associating restores exactly the association that was released: the store is
    // back at the snapshot it started from.
    ok(
        &router,
        "AssociateApprovalRuleTemplateWithRepository",
        &json!({
            "approvalRuleTemplateName": first.template,
            "repositoryName": held,
        }),
    )
    .await;
    let answered = snapshot(&router, "re-associated after the release").await;
    assert_eq!(
        answered, before,
        "case \"re-associated after the release\": the store must be back at {before:?}, got \
         {answered:?}"
    );
}

#[tokio::test]
async fn a_name_either_side_never_held_is_refused_and_releases_nothing() {
    let router = seeded().await;
    let before = expected_snapshot(&seeded_matrix());
    let stored_template = association_table()[1].template.clone();
    let stored_repository = "mid-repo";

    // `approvalRuleTemplateName` and `repositoryName` are both `@required` and
    // `@length` 1..100, and `repositoryName` carries the pattern `^[\w\.-]+$`
    // (`smithy_query(query="operation",
    // operation="DisassociateApprovalRuleTemplateFromRepository")`); the operation
    // models one error per cause. The rows differ along WHICH side is unusable and in
    // what way: unknown, absent, empty, past the length bound, whitespace only, or
    // outside the repository-name pattern.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a template name never created",
            json!({
                "approvalRuleTemplateName": "no-such-template",
                "repositoryName": stored_repository,
            }),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "a stored template name in another case",
            json!({
                "approvalRuleTemplateName": stored_template.to_uppercase(),
                "repositoryName": stored_repository,
            }),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "a repository name never created",
            json!({
                "approvalRuleTemplateName": stored_template,
                "repositoryName": "no-such-repo",
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "a stored repository name in another case",
            json!({
                "approvalRuleTemplateName": stored_template,
                "repositoryName": "MID-REPO",
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "no template name at all",
            json!({ "repositoryName": stored_repository }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "an empty template name",
            json!({
                "approvalRuleTemplateName": "",
                "repositoryName": stored_repository,
            }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "a template name one character past the modelled maximum of 100",
            json!({
                "approvalRuleTemplateName": "n".repeat(101),
                "repositoryName": stored_repository,
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a template name of nothing but whitespace",
            json!({
                "approvalRuleTemplateName": " \t",
                "repositoryName": stored_repository,
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "no repository name at all",
            json!({ "approvalRuleTemplateName": stored_template }),
            "RepositoryNameRequiredException",
        ),
        (
            "an empty repository name",
            json!({
                "approvalRuleTemplateName": stored_template,
                "repositoryName": "",
            }),
            "InvalidRepositoryNameException",
        ),
        (
            "a repository name outside the modelled pattern",
            json!({
                "approvalRuleTemplateName": stored_template,
                "repositoryName": "mid repo",
            }),
            "InvalidRepositoryNameException",
        ),
        (
            "a repository name one character past the modelled maximum of 100",
            json!({
                "approvalRuleTemplateName": stored_template,
                "repositoryName": "r".repeat(101),
            }),
            "InvalidRepositoryNameException",
        ),
    ];

    for (case, body, expected) in &rows {
        let code = error(
            &router,
            "DisassociateApprovalRuleTemplateFromRepository",
            body,
            400,
        )
        .await;
        assert_eq!(
            code, *expected,
            "case {case:?}: expected {expected}, got {code:?}"
        );
        let answered = snapshot(&router, case).await;
        assert_eq!(
            answered, before,
            "case {case:?}: a refused release persists nothing, expected {before:?}, got \
             {answered:?}"
        );
    }
}
