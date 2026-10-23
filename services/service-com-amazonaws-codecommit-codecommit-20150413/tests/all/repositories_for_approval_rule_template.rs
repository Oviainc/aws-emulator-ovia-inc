// SPDX-License-Identifier: CC0-1.0

//! `ListRepositoriesForApprovalRuleTemplate` over the wire, as awsJson1_1 requests
//! through the router that serves the real emulator (the harness lives in
//! `repository_lifecycle`, which this suite shares rather than rebuilding).
//!
//! The rules under test:
//!
//! 1. The operation reports exactly the repositories whose stored association set
//!    holds the named template, in `repositoryName` order — the other end of the
//!    association `AssociateApprovalRuleTemplateWithRepository` records and
//!    `ListAssociatedApprovalRuleTemplatesForRepository` reads from the repository's
//!    side, so the two listings can never disagree. It is a read: calling it twice
//!    answers the same thing, and a repository that is deleted drops out of it.
//! 2. A template name no template holds is refused with the error the model declares
//!    for that cause (`ApprovalRuleTemplateDoesNotExistException`, HTTP 400), as are
//!    a missing name and a name past the published length bound, each with its own
//!    modelled error.
//! 3. Paging follows this service's single rule: `maxResults` inside the published
//!    1..=1000 range, a `nextToken` only one THIS listing issued, a page of at most
//!    `maxResults` names and a token only while a further page remains.
//!
//! Each table spans the class its rule covers — association counts, creation versus
//! name order, page sizes, both published `maxResults` bounds and one past each, and
//! the forms a token can take — rather than one recorded request.

use serde_json::{json, Value};

use crate::repository_lifecycle::{emulator, error, ok, string_at};

/// The schema version of an approval rule template document and the only statement
/// type its schema defines, as the CodeCommit User Guide documents them (and
/// `src/state.rs`'s `APPROVAL_RULE_TEMPLATE_VERSION` /
/// `APPROVAL_RULE_STATEMENT_TYPE`).
const TEMPLATE_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// The simplest valid approval rule template document.
fn minimal_content() -> String {
    json!({
        "Version": TEMPLATE_VERSION,
        "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 }],
    })
    .to_string()
}

/// The repository names one `ListRepositoriesForApprovalRuleTemplate` answer carries.
fn repository_names_of(payload: &Value, case: &str) -> Vec<String> {
    payload
        .get("repositoryNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no repositoryNames in {payload}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a name is not a string in {payload}"))
                .to_string()
        })
        .collect()
}

/// The approval rule template names one
/// `ListAssociatedApprovalRuleTemplatesForRepository` answer carries.
fn template_names_of(payload: &Value, case: &str) -> Vec<String> {
    payload
        .get("approvalRuleTemplateNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no approvalRuleTemplateNames in {payload}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a name is not a string in {payload}"))
                .to_string()
        })
        .collect()
}

/// Every repository this suite creates, in an order that is NOT their name order, so
/// a listing that reported creation order would differ from one that reports name
/// order.
const REPOSITORIES: [&str; 4] = ["zeta-repo", "mid-repo", "alpha-repo", "lonely-repo"];

/// The association table: one row per template, with the repositories it is
/// associated with. The rows differ along what the rule generalises over — how MANY
/// repositories a template is associated with (none, one, some, every one) and
/// whether they were created before or after each other.
fn association_table() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("template-unassociated", vec![]),
        ("template-one", vec!["mid-repo"]),
        ("template-some", vec!["zeta-repo", "alpha-repo"]),
        ("template-every", REPOSITORIES.to_vec()),
    ]
}

/// An emulator holding [`REPOSITORIES`] and every template of
/// [`association_table`], associated as the table says.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    for repository in REPOSITORIES {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
    }
    for (template, repositories) in association_table() {
        ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": template,
                "approvalRuleTemplateContent": minimal_content(),
            }),
        )
        .await;
        for repository in repositories {
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

/// The expected answer for one row: the row's repositories in name order.
fn expected_names(repositories: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = repositories.iter().map(|name| (*name).to_string()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn the_listing_is_the_repositories_associated_with_the_template_in_name_order() {
    let router = seeded().await;

    for (template, repositories) in association_table() {
        let expected = expected_names(&repositories);
        let payload = ok(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": template }),
        )
        .await;
        let answered = repository_names_of(&payload, template);
        assert_eq!(
            answered, expected,
            "case {template:?}: expected {expected:?}, got {answered:?}"
        );
        assert!(
            payload.get("nextToken").is_none_or(Value::is_null),
            "case {template:?}: a listing that returned everything carries no token, \
             got {payload}"
        );

        // A read changes nothing a later caller sees: the same request answers the
        // same page again.
        let again = ok(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": template }),
        )
        .await;
        let repeated = repository_names_of(&again, template);
        assert_eq!(
            repeated, expected,
            "case {template:?}: a repeated read answers the same, got {repeated:?}"
        );
    }

    // Both ends of every association agree, because there is one stored association:
    // a repository appears in a template's listing exactly when that template appears
    // in the repository's listing.
    for repository in REPOSITORIES {
        let payload = ok(
            &router,
            "ListAssociatedApprovalRuleTemplatesForRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        let from_repository = template_names_of(&payload, repository);
        let mut expected: Vec<String> = association_table()
            .into_iter()
            .filter(|(_, repositories)| repositories.contains(&repository))
            .map(|(template, _)| template.to_string())
            .collect();
        expected.sort();
        assert_eq!(
            from_repository, expected,
            "case {repository:?}: the two ends of the association must agree, \
             expected {expected:?}, got {from_repository:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_repository_drops_out_of_every_template_listing() {
    let router = seeded().await;

    // Deleting a repository releases its associations, so it is gone from the
    // listings of both templates it was associated with, and the other repositories
    // are untouched.
    ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": "mid-repo" }),
    )
    .await;

    for (template, repositories) in association_table() {
        let expected: Vec<String> = expected_names(&repositories)
            .into_iter()
            .filter(|name| name != "mid-repo")
            .collect();
        let payload = ok(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": template }),
        )
        .await;
        let answered = repository_names_of(&payload, template);
        assert_eq!(
            answered, expected,
            "case {template:?}: after the delete, expected {expected:?}, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_template_name_the_service_cannot_resolve_is_refused_with_its_modelled_error() {
    let router = seeded().await;

    // Names no template holds. The rows differ along the FORM of the name: never
    // created, the name of a repository rather than a template, a near miss of a
    // stored name, and a name at the published 100-character maximum.
    for case in [
        "no-such-template",
        "mid-repo",
        "template-ones",
        "TEMPLATE-ONE",
        &"n".repeat(100),
    ] {
        let code = error(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": case }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateDoesNotExistException",
            "case {case:?}: expected ApprovalRuleTemplateDoesNotExistException, got {code:?}"
        );
    }

    // A name at the published maximum that DOES name a template is served, so the
    // refusals above are about the template and not about the length.
    let long_name = "n".repeat(100);
    ok(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": long_name,
            "approvalRuleTemplateContent": minimal_content(),
        }),
    )
    .await;
    let payload = ok(
        &router,
        "ListRepositoriesForApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": long_name }),
    )
    .await;
    assert_eq!(
        repository_names_of(&payload, "a name at the published maximum"),
        Vec::<String>::new(),
        "case \"a name at the published maximum\": a template with no associations \
         answers an empty listing, got {payload}"
    );

    // `ApprovalRuleTemplateName` is required and 1..100 characters long
    // (`smithy_query(query="operation",
    //   operation="ListRepositoriesForApprovalRuleTemplate")`), so a name one
    // character past the bound and a name that is not there each have their own
    // modelled error.
    let code = error(
        &router,
        "ListRepositoriesForApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": "n".repeat(101) }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidApprovalRuleTemplateNameException",
        "case \"one character past the published maximum\": expected \
         InvalidApprovalRuleTemplateNameException, got {code:?}"
    );
    for (case, body) in [
        ("an empty name", json!({ "approvalRuleTemplateName": "" })),
        ("no name member", json!({})),
    ] {
        let code = error(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &body,
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateNameRequiredException",
            "case {case:?}: expected ApprovalRuleTemplateNameRequiredException, got {code:?}"
        );
    }
}

#[tokio::test]
async fn pagination_follows_the_services_published_rule() {
    let router = seeded().await;
    let all_names = expected_names(&REPOSITORIES);

    // Rows: every page size from one name to one more than the sequence holds. Paging
    // must answer every name exactly once, in order, and carry a token only while a
    // further page remains.
    for page_size in 1..=(all_names.len() + 1) {
        let label = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({
                "approvalRuleTemplateName": "template-every",
                "maxResults": page_size,
            });
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let payload = ok(&router, "ListRepositoriesForApprovalRuleTemplate", &body).await;
            let page = repository_names_of(&payload, &label);
            assert!(
                page.len() <= page_size,
                "case {label:?}: a page carries at most {page_size} names, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= all_names.len() + 1,
                "case {label:?}: paging must terminate, saw {pages} pages"
            );
            match payload.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all_names,
            "case {label:?}: paging must answer every name exactly once, in order, \
             got {seen:?}"
        );
    }

    // `maxResults`: CodeCommit publishes "A valid limit is between 1 and 1000", so
    // both bounds are served and one past each is refused. `nextToken` is opaque, so
    // a value this listing did not issue — including a bare offset into the
    // sequence — is not a token.
    let accepted: [i64; 4] = [1, 2, 999, 1000];
    for limit in accepted {
        let label = format!("maxResults {limit}");
        let payload = ok(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": "template-every", "maxResults": limit }),
        )
        .await;
        let page = repository_names_of(&payload, &label);
        let expected = usize::try_from(limit).unwrap_or(0).min(all_names.len());
        assert_eq!(
            page.len(),
            expected,
            "case {label:?}: expected {expected} names, got {page:?}"
        );
    }
    for limit in [0i64, -1, 1001, i64::from(i32::MIN), i64::from(i32::MAX)] {
        let label = format!("maxResults {limit}");
        let code = error(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": "template-every", "maxResults": limit }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidMaxResultsException",
            "case {label:?}: expected InvalidMaxResultsException, got {code:?}"
        );
    }
    for token in ["0", "2", "", "-1", "1e3", "not-a-real-continuation-token"] {
        let code = error(
            &router,
            "ListRepositoriesForApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": "template-every", "nextToken": token }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidContinuationTokenException",
            "case {token:?}: a token this listing never issued, expected \
             InvalidContinuationTokenException, got {code:?}"
        );
    }

    // A token belongs to the listing that issued it: the token one template's listing
    // hands out is not a token for another template's, nor for the per-repository
    // listing.
    let issued = ok(
        &router,
        "ListRepositoriesForApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": "template-every", "maxResults": 1 }),
    )
    .await;
    let token = string_at(&issued, "/nextToken");
    let code = error(
        &router,
        "ListRepositoriesForApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": "template-some", "nextToken": token.clone() }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "case \"a token issued for another template's listing\": expected \
         InvalidContinuationTokenException, got {code:?}"
    );
    let code = error(
        &router,
        "ListAssociatedApprovalRuleTemplatesForRepository",
        &json!({ "repositoryName": "mid-repo", "nextToken": token }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "case \"a token issued for another operation's listing\": expected \
         InvalidContinuationTokenException, got {code:?}"
    );

    // And a token issued by the per-repository listing is not a token for this one.
    let issued = ok(
        &router,
        "ListAssociatedApprovalRuleTemplatesForRepository",
        &json!({ "repositoryName": "mid-repo", "maxResults": 1 }),
    )
    .await;
    let code = error(
        &router,
        "ListRepositoriesForApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "template-every",
            "nextToken": string_at(&issued, "/nextToken"),
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "case \"a token issued by the per-repository listing\": expected \
         InvalidContinuationTokenException, got {code:?}"
    );
}
