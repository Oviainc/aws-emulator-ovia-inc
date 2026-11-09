// SPDX-License-Identifier: CC0-1.0

//! `BatchAssociateApprovalRuleTemplateWithRepositories` over the wire, driven as
//! awsJson1_1 requests through the router that serves the real emulator (the wire
//! harness comes from the sibling `repository_lifecycle` suite rather than a second
//! copy).
//!
//! The rule under test: for each DISTINCT repository name the request lists, in
//! request order, the call either records the association with the named template —
//! and reports that name in `associatedRepositoryNames` — or records nothing for it
//! and reports one `errors` entry naming it, its cause's `errorCode` and a message
//! built from the request; the template itself is the whole call's precondition, so a
//! template the service holds none of refuses the request and writes nothing. The
//! association it records is the SAME one the single-resource
//! `AssociateApprovalRuleTemplateWithRepository` records, so both listings
//! (`ListAssociatedApprovalRuleTemplatesForRepository`,
//! `ListRepositoriesForApprovalRuleTemplate`) read it back.
//!
//! The tables below span what the rule generalises over — how many names a request
//! carries, their order, whether each is held or absent, repeated names, the empty
//! array, the published limit on the number of names, and a repository already
//! holding as many associations as it may hold — rather than one recorded request.

use serde_json::{json, Value};

use crate::repository_lifecycle::{call, emulator, error, ok};

/// The schema version of an approval rule template document and the only statement
/// type its schema defines, as the CodeCommit User Guide documents them (and
/// `src/state.rs`'s `APPROVAL_RULE_TEMPLATE_VERSION` /
/// `APPROVAL_RULE_STATEMENT_TYPE`).
const TEMPLATE_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// "The maximum number of allowed repository names was exceeded. Currently, this
/// number is 100." — `MaximumRepositoryNamesExceededException`'s own documentation
/// (`smithy_query(query="shape",shape="MaximumRepositoryNamesExceededException")`).
const MAX_REPOSITORY_NAMES: usize = 100;

/// The emulator's own bound on one repository's association set
/// (`src/state.rs`'s `MAX_TEMPLATES_PER_REPOSITORY`), which the batch reports per
/// entry rather than refusing the whole call for.
const MAX_TEMPLATES_PER_REPOSITORY: usize = 100;

/// The simplest valid approval rule template document.
fn minimal_content() -> String {
    json!({
        "Version": TEMPLATE_VERSION,
        "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 }],
    })
    .to_string()
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

/// One `errors` entry, read as (`repositoryName`, `errorCode`), with the
/// `errorMessage` asserted to be present and to name the repository at fault.
fn error_entry(entry: &Value, case: &str) -> (String, String) {
    let name = entry
        .get("repositoryName")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no repositoryName in {entry}"))
        .to_string();
    let code = entry
        .get("errorCode")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no errorCode in {entry}"))
        .to_string();
    let message = entry
        .get("errorMessage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no errorMessage in {entry}"));
    assert!(
        message.contains(&name),
        "case {case:?}: the entry's message must name the repository at fault, \
         got {message:?} for {name:?}"
    );
    (name, code)
}

/// `(repositoryName, errorCode)` of every `errors` entry, in answer order.
fn error_entries(payload: &Value, case: &str) -> Vec<(String, String)> {
    list_at(payload, "errors")
        .iter()
        .map(|entry| error_entry(entry, case))
        .collect()
}

/// The template names `ListAssociatedApprovalRuleTemplatesForRepository` reports for
/// one repository.
async fn templates_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    repository: &str,
) -> Vec<String> {
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
async fn repositories_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    template: &str,
) -> Vec<String> {
    let payload = ok(
        router,
        "ListRepositoriesForApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": template }),
    )
    .await;
    strings_of(&list_at(&payload, "repositoryNames"), template)
}

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

/// The template this suite associates, and a second one so a repository can hold
/// more than one association.
const TEMPLATE: &str = "batch-template";
const OTHER_TEMPLATE: &str = "other-template";

async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    for repository in seeded_repositories() {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
    }
    for template in [TEMPLATE, OTHER_TEMPLATE] {
        ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": template,
                "approvalRuleTemplateContent": minimal_content(),
            }),
        )
        .await;
    }
    router
}

/// One row of the batch table: the names one request lists, and the names the
/// service must report as associated (in that order) and as failed.
#[derive(Debug)]
struct BatchCase {
    label: &'static str,
    requested: Vec<String>,
    associated: Vec<String>,
    failed: Vec<String>,
}

fn batch_cases(held: &[String]) -> Vec<BatchCase> {
    vec![
        BatchCase {
            label: "one held name",
            requested: vec![held[0].clone()],
            associated: vec![held[0].clone()],
            failed: Vec::new(),
        },
        BatchCase {
            label: "every held name, in an order that is not the store's",
            requested: held.iter().rev().cloned().collect(),
            associated: held.iter().rev().cloned().collect(),
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
            associated: vec![held[2].clone(), held[1].clone()],
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
            associated: vec![held[0].clone()],
            failed: vec!["absent-one".to_string()],
        },
        BatchCase {
            label: "only absent names",
            requested: vec!["absent-one".to_string(), "absent-three".to_string()],
            associated: Vec::new(),
            failed: vec!["absent-one".to_string(), "absent-three".to_string()],
        },
        BatchCase {
            // `repositoryNames`'s own documentation: "The array itself can be empty."
            label: "the empty array",
            requested: Vec::new(),
            associated: Vec::new(),
            failed: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn each_name_is_associated_or_reported_as_its_own_error() {
    let held = seeded_repositories();

    for case in batch_cases(&held) {
        // A fresh store per row, so each row's answer is about that row's names.
        let router = seeded().await;
        let label = case.label;

        let payload = ok(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": case.requested,
            }),
        )
        .await;

        let answered = strings_of(&list_at(&payload, "associatedRepositoryNames"), label);
        assert_eq!(
            answered, case.associated,
            "case {label:?}: expected {:?}, got {answered:?}",
            case.associated
        );

        let expected_errors: Vec<(String, String)> = case
            .failed
            .iter()
            .map(|name| (name.clone(), "RepositoryDoesNotExistException".to_string()))
            .collect();
        let errors = error_entries(&payload, label);
        assert_eq!(
            errors, expected_errors,
            "case {label:?}: expected {expected_errors:?}, got {errors:?}"
        );

        // ROUND TRIP: what the call recorded is what both listings read back — the
        // repositories it reported as associated, and no others.
        let listed = repositories_of(&router, TEMPLATE).await;
        let mut expected = case.associated.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {label:?}: the template's listing must hold exactly the associated \
             repositories, expected {expected:?}, got {listed:?}"
        );
        for repository in &held {
            let templates = templates_of(&router, repository).await;
            let expected: Vec<String> = if case.associated.contains(repository) {
                vec![TEMPLATE.to_string()]
            } else {
                Vec::new()
            };
            assert_eq!(
                templates, expected,
                "case {label:?}: {repository:?} must hold {expected:?}, got {templates:?}"
            );
        }

        // The same request again is the same answer and the same stored state: an
        // association is a set membership, so associating twice associates once.
        let again = ok(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
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
            relisted, expected_sorted(&case.associated),
            "case {label:?}: a repeated call must not duplicate an association, \
             got {relisted:?}"
        );
    }
}

/// The row's associated names in the order a listing reports them.
fn expected_sorted(names: &[String]) -> Vec<String> {
    let mut sorted = names.to_vec();
    sorted.sort();
    sorted
}

#[tokio::test]
async fn an_association_joins_the_ones_the_repository_already_holds() {
    let router = seeded().await;
    let held = seeded_repositories();

    // The single-resource operation records one association; the batch records
    // another for the same repository. Both are stored, because a repository holds a
    // SET of templates rather than one.
    ok(
        &router,
        "AssociateApprovalRuleTemplateWithRepository",
        &json!({ "approvalRuleTemplateName": OTHER_TEMPLATE, "repositoryName": held[0] }),
    )
    .await;
    ok(
        &router,
        "BatchAssociateApprovalRuleTemplateWithRepositories",
        &json!({
            "approvalRuleTemplateName": TEMPLATE,
            "repositoryNames": [held[0].clone()],
        }),
    )
    .await;

    let templates = templates_of(&router, &held[0]).await;
    assert_eq!(
        templates,
        vec![TEMPLATE.to_string(), OTHER_TEMPLATE.to_string()],
        "case \"both templates\": the batch must add to the set rather than replace it, \
         got {templates:?}"
    );

    // And the two operations write the SAME association: what the batch recorded is
    // what the single-resource operation would have recorded, so re-recording it
    // through the other path changes nothing.
    ok(
        &router,
        "AssociateApprovalRuleTemplateWithRepository",
        &json!({ "approvalRuleTemplateName": TEMPLATE, "repositoryName": held[0] }),
    )
    .await;
    let again = templates_of(&router, &held[0]).await;
    assert_eq!(
        again, templates,
        "case \"the same association through both paths\": expected {templates:?}, \
         got {again:?}"
    );
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
        "batch-templates".to_string(),
        "BATCH-TEMPLATE".to_string(),
        "n".repeat(100),
    ] {
        let router = seeded().await;
        let code = error(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
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
        // The refusal wrote nothing: no repository holds an association.
        for repository in &held {
            let templates = templates_of(&router, repository).await;
            assert!(
                templates.is_empty(),
                "case {case:?}: a refused call must persist nothing, {repository:?} holds \
                 {templates:?}"
            );
        }
    }

    // A name at the published maximum that DOES name a template is served, so the
    // refusals above are about the template and not about the length.
    let router = seeded().await;
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
        "BatchAssociateApprovalRuleTemplateWithRepositories",
        &json!({
            "approvalRuleTemplateName": long_name,
            "repositoryNames": [held[1].clone()],
        }),
    )
    .await;
    assert_eq!(
        strings_of(
            &list_at(&payload, "associatedRepositoryNames"),
            "a template name at the published maximum"
        ),
        vec![held[1].clone()],
        "case \"a template name at the published maximum\": expected the held name, \
         got {payload}"
    );
}

#[tokio::test]
async fn a_bad_request_is_refused_with_the_error_modelled_for_its_member() {
    let router = seeded().await;
    let held = seeded_repositories();

    // `approvalRuleTemplateName` is `@required` and 1..100 characters
    // (`smithy_query(query="operation",
    //   operation="BatchAssociateApprovalRuleTemplateWithRepositories")`), so a name
    // that is absent or empty and a name past the bound each have their own modelled
    // error, and so does an absent `repositoryNames`.
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
            "BatchAssociateApprovalRuleTemplateWithRepositories",
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
        assert!(
            templates.is_empty(),
            "case \"after the refusals\": {repository:?} must hold no association, \
             got {templates:?}"
        );
    }
}

#[tokio::test]
async fn the_number_of_names_is_bounded_by_the_published_limit() {
    let router = seeded().await;
    let held = seeded_repositories();
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
    // either associated or as its own error — and the first count past it is refused.
    for count in [1usize, MAX_REPOSITORY_NAMES - 1, MAX_REPOSITORY_NAMES] {
        let label = format!("{count} names");
        let payload = ok(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": names(count),
            }),
        )
        .await;
        let associated = list_at(&payload, "associatedRepositoryNames").len();
        let failed = list_at(&payload, "errors").len();
        assert_eq!(
            associated + failed,
            count,
            "case {label:?}: every name must be reported exactly once, got {payload}"
        );
        assert_eq!(
            associated, 1,
            "case {label:?}: the one held repository must be associated, got {payload}"
        );
    }
    for count in [MAX_REPOSITORY_NAMES + 1, 2 * MAX_REPOSITORY_NAMES] {
        let label = format!("{count} names");
        let code = error(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
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

    // The refused calls left the service serving, and the only association any of
    // these calls recorded is the one the served counts recorded.
    let (status, payload) = call(
        &router,
        "BatchAssociateApprovalRuleTemplateWithRepositories",
        &json!({ "approvalRuleTemplateName": TEMPLATE, "repositoryNames": [] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the refused calls must leave the service serving, got {payload}"
    );
    let listed = repositories_of(&router, TEMPLATE).await;
    assert_eq!(
        listed,
        vec![held[0].clone()],
        "the refused calls must not have changed the store, got {listed:?}"
    );
}

#[tokio::test]
async fn a_repository_at_its_association_bound_fails_only_its_own_entry() {
    let router = emulator();
    let saturated = "saturated-repo";
    let roomy = "roomy-repo";
    for repository in [saturated, roomy] {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
    }

    // Fill one repository's association set to the bound through this very
    // operation, so the bound is reached by the path under test.
    for index in 0..MAX_TEMPLATES_PER_REPOSITORY {
        let template = format!("fill-template-{index:03}");
        ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": template,
                "approvalRuleTemplateContent": minimal_content(),
            }),
        )
        .await;
        let payload = ok(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
            &json!({
                "approvalRuleTemplateName": template,
                "repositoryNames": [saturated],
            }),
        )
        .await;
        assert!(
            list_at(&payload, "errors").is_empty(),
            "case \"filling to the bound\": association {index} must succeed, got {payload}"
        );
    }
    assert_eq!(
        templates_of(&router, saturated).await.len(),
        MAX_TEMPLATES_PER_REPOSITORY,
        "case \"filling to the bound\": the repository must hold {MAX_TEMPLATES_PER_REPOSITORY} \
         associations"
    );

    // One more template, requested for both repositories plus an absent one: the
    // saturated repository and the absent name each fail with their own code, and the
    // repository with room is still associated.
    ok(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": TEMPLATE,
            "approvalRuleTemplateContent": minimal_content(),
        }),
    )
    .await;
    let payload = ok(
        &router,
        "BatchAssociateApprovalRuleTemplateWithRepositories",
        &json!({
            "approvalRuleTemplateName": TEMPLATE,
            "repositoryNames": [saturated, "absent-repo", roomy],
        }),
    )
    .await;
    let label = "one past the association bound";
    assert_eq!(
        strings_of(&list_at(&payload, "associatedRepositoryNames"), label),
        vec![roomy.to_string()],
        "case {label:?}: only the repository with room is associated, got {payload}"
    );
    assert_eq!(
        error_entries(&payload, label),
        vec![
            (
                saturated.to_string(),
                "MaximumRuleTemplatesAssociatedWithRepositoryException".to_string()
            ),
            (
                "absent-repo".to_string(),
                "RepositoryDoesNotExistException".to_string()
            ),
        ],
        "case {label:?}: each failing name carries its own cause, got {payload}"
    );
    let templates = templates_of(&router, saturated).await;
    assert_eq!(
        templates.len(),
        MAX_TEMPLATES_PER_REPOSITORY,
        "case {label:?}: the failed entry must not have been recorded, got {} associations",
        templates.len()
    );
    assert!(
        !templates.contains(&TEMPLATE.to_string()),
        "case {label:?}: the refused template must not be associated, got {templates:?}"
    );
    assert_eq!(
        templates_of(&router, roomy).await,
        vec![TEMPLATE.to_string()],
        "case {label:?}: the served entry must be recorded"
    );
}

/// The per-entry `errorMessage` for a name the account holds no repository under: a
/// batch call reports a failing name ONLY through its entry, so the entry carries the
/// sentence CodeCommit words that cause with — fixed text whose single slot is the
/// name the request asked for — rather than an internal rendering of the lookup that
/// failed.
///
/// The rows span the names `RepositoryName` allows (`len 1..100`, pattern
/// `^[\w\.-]+$` — `smithy_query(query="shape",shape="RepositoryName")`): the
/// 1-character minimum, the 100-character maximum, and names mixing digits, dots,
/// hyphens and underscores, none of them a name this suite holds a repository for.
#[tokio::test]
async fn an_absent_name_s_entry_carries_the_sentence_for_that_cause() {
    let router = seeded().await;
    let absent: Vec<String> = vec![
        "q".to_string(),
        "n".repeat(100),
        "absent.repo-9".to_string(),
        "Absent_Repo".to_string(),
        "9".to_string(),
    ];

    for name in &absent {
        let payload = ok(
            &router,
            "BatchAssociateApprovalRuleTemplateWithRepositories",
            &json!({
                "approvalRuleTemplateName": TEMPLATE,
                "repositoryNames": [name],
            }),
        )
        .await;
        assert_eq!(
            strings_of(&list_at(&payload, "associatedRepositoryNames"), name),
            Vec::<String>::new(),
            "case {name:?}: a name no repository is held under associates nothing, got {payload}"
        );
        let entries = list_at(&payload, "errors");
        assert_eq!(
            entries.len(),
            1,
            "case {name:?}: exactly one entry reports the failing name, got {payload}"
        );
        let expected = format!(
            "No repository with the name {name} was found. Please specify a valid repository name"
        );
        let message = entries[0]
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: no errorMessage in {payload}"));
        assert_eq!(
            message, expected,
            "case {name:?}: expected the entry's message {expected:?}, got {message:?}"
        );
    }
}
