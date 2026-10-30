// SPDX-License-Identifier: CC0-1.0

//! `UpdateApprovalRuleTemplateName` over the wire, driven as awsJson1_1 requests
//! through the router that serves the real emulator.
//!
//! The rule under test: renaming an approval rule template MOVES the stored template
//! to the new name and changes nothing else about it — its id, description, content,
//! `ruleContentSha256` and `creationDate` carry over, `lastModifiedDate` advances to
//! the instant of the change, and the repositories it was associated with under the
//! old name are associated with it under the new one — so the new name reads back the
//! template the old name used to address and the old name addresses nothing.
//! Template names are unique per account, so a rename onto a name the account
//! already holds is refused with `ApprovalRuleTemplateNameAlreadyExistsException` and
//! moves NOTHING; once the occupying template is released, the same rename goes
//! through.
//!
//! The tables below span what that rule generalises over rather than one recorded
//! request: the FORM of the new name (ordinary text, both published bounds of
//! `ApprovalRuleTemplateName` — length 1..100, from
//! `smithy_query(query="operation",operation="UpdateApprovalRuleTemplateName")` —
//! one character past the upper bound, unicode, inner spaces, punctuation, and a
//! spelling differing from the old one only in case), the template's starting state
//! (with and without a description, with and without repository associations), and
//! why a request is refused (the old name addresses nothing, the new name is taken by
//! another template or by the template itself, either name names no template at all).
//!
//! The sibling suites own the neighbouring behaviour: `approval_rule_templates` and
//! `approval_rules_and_commits` the template lifecycle,
//! `approval_rule_template_content_update` the content update,
//! `approval_rule_template_description` the description update.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The valid template document the sibling template suite builds, so every suite
// creates templates from one definition of "valid content".
use crate::approval_rule_templates::document;
use crate::harness::{error_with_message, seconds_at};
use crate::repository_lifecycle::{emulator, error, ok, string_at};

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of this operation's error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The published bounds of `ApprovalRuleTemplateName` (`length` 1..100).
const MIN_NAME_CHARACTERS: usize = 1;
const MAX_NAME_CHARACTERS: usize = 100;

/// The FIXED sentence `ApprovalRuleTemplateNameAlreadyExistsException` carries: it
/// names neither template and no Region, so the same text answers a create and a
/// rename alike.
const NAME_TAKEN_MESSAGE: &str =
    "The approval rule template cannot be created because a template with that name already \
     exists in your AWS account.";

/// Members of `ApprovalRuleTemplate` a rename must NOT touch
/// (`smithy_query(query="shape",shape="ApprovalRuleTemplate")`);
/// `approvalRuleTemplateName`, `lastModifiedDate` and `lastModifiedUser` are the
/// three the rename moves, and each is asserted on its own.
const INVARIANT_MEMBERS: [&str; 4] = [
    "approvalRuleTemplateId",
    "approvalRuleTemplateContent",
    "ruleContentSha256",
    "creationDate",
];

/// The `approvalRuleTemplate` a `GetApprovalRuleTemplate` read reports — the
/// projection a stored template is read back through.
async fn template_of(router: &MultiServiceRouter, name: &str) -> Value {
    let got = ok(
        router,
        "GetApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": name }),
    )
    .await;
    got.get("approvalRuleTemplate")
        .unwrap_or_else(|| panic!("no approvalRuleTemplate for {name:?} in {got}"))
        .clone()
}

/// Create one template, answering the stored record.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    approvals: u32,
    description: Option<&str>,
) -> Value {
    let mut input = json!({
        "approvalRuleTemplateName": name,
        "approvalRuleTemplateContent": document(approvals),
    });
    if let Some(description) = description {
        input["approvalRuleTemplateDescription"] = json!(description);
    }
    let created = ok(router, "CreateApprovalRuleTemplate", &input).await;
    created
        .get("approvalRuleTemplate")
        .unwrap_or_else(|| panic!("no approvalRuleTemplate in {created}"))
        .clone()
}

/// The names `ListApprovalRuleTemplates` reports for the account.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let listed = ok(router, "ListApprovalRuleTemplates", &json!({})).await;
    listed
        .get("approvalRuleTemplateNames")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One rename: the name the template is moved to, and whether the row also gives the
/// template a description and associates it with a repository. Rows differ along the
/// form and size of the new name.
#[derive(Debug, Clone)]
struct Rename {
    case: &'static str,
    new_name: String,
    description: Option<&'static str>,
    associate: bool,
}

fn renames() -> Vec<Rename> {
    vec![
        Rename {
            case: "ordinary text",
            new_name: "two-reviewers-renamed".to_string(),
            description: Some("two reviewers on every pull request"),
            associate: true,
        },
        Rename {
            case: "the published minimum length",
            new_name: "x".repeat(MIN_NAME_CHARACTERS),
            description: None,
            associate: false,
        },
        Rename {
            case: "the published maximum length",
            new_name: "n".repeat(MAX_NAME_CHARACTERS),
            description: Some("at the bound"),
            associate: true,
        },
        Rename {
            case: "unicode text",
            new_name: "revisión-obligatoria-⚙".to_string(),
            description: None,
            associate: false,
        },
        Rename {
            case: "a name with inner spaces",
            new_name: "two reviewers, please".to_string(),
            description: Some("spaces are part of a name"),
            associate: true,
        },
        Rename {
            case: "a dotted, punctuated name",
            new_name: "team.review/v2+2".to_string(),
            description: None,
            associate: false,
        },
    ]
}

/// The name every row starts from, so the row's own new name is the only thing that
/// differs between them.
const ORIGINAL: &str = "needs-two-reviewers";

#[tokio::test]
async fn a_rename_moves_the_template_and_changes_nothing_else_about_it() {
    for rename in renames() {
        let case = rename.case;
        let router = emulator();
        let created = create(&router, ORIGINAL, 2, rename.description).await;
        if rename.associate {
            for repository in ["alpha", "beta"] {
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
                        "approvalRuleTemplateName": ORIGINAL,
                        "repositoryName": repository,
                    }),
                )
                .await;
            }
        }
        // The change has to be distinguishable from the creation in time.
        tokio::time::sleep(Duration::from_millis(5)).await;

        let answered = ok(
            &router,
            "UpdateApprovalRuleTemplateName",
            &json!({
                "oldApprovalRuleTemplateName": ORIGINAL,
                "newApprovalRuleTemplateName": rename.new_name,
            }),
        )
        .await;
        let renamed = answered
            .get("approvalRuleTemplate")
            .unwrap_or_else(|| panic!("case {case:?}: no approvalRuleTemplate in {answered}"))
            .clone();

        assert_eq!(
            renamed.get("approvalRuleTemplateName").and_then(Value::as_str),
            Some(rename.new_name.as_str()),
            "case {case:?}: expected the new name on the answer, got {renamed}"
        );
        for member in INVARIANT_MEMBERS {
            assert_eq!(
                renamed.get(member),
                created.get(member),
                "case {case:?}: a rename must not change {member}, got {renamed}"
            );
        }
        assert_eq!(
            renamed.get("approvalRuleTemplateDescription").and_then(Value::as_str),
            rename.description,
            "case {case:?}: expected the description to carry over, got {renamed}"
        );
        let created_at = seconds_at(&created, "lastModifiedDate");
        let modified_at = seconds_at(&renamed, "lastModifiedDate");
        assert!(
            modified_at > created_at,
            "case {case:?}: expected lastModifiedDate to advance past {created_at}, got \
             {modified_at}"
        );

        // The new name reads back exactly what the rename answered, and the old name
        // addresses no template at all.
        assert_eq!(
            template_of(&router, &rename.new_name).await,
            renamed,
            "case {case:?}: the new name must read back the renamed template"
        );
        let code = error(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": ORIGINAL }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateDoesNotExistException",
            "case {case:?}: the old name must address nothing, got {code:?}"
        );
        assert_eq!(
            listed_names(&router).await,
            vec![rename.new_name.clone()],
            "case {case:?}: the account must hold exactly the renamed template"
        );

        // An association is the same association: it follows the template to its new
        // name, on both sides of the relation.
        if rename.associate {
            let listed = ok(
                &router,
                "ListRepositoriesForApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": rename.new_name }),
            )
            .await;
            assert_eq!(
                listed.get("repositoryNames"),
                Some(&json!(["alpha", "beta"])),
                "case {case:?}: the renamed template keeps its repositories, got {listed}"
            );
            for repository in ["alpha", "beta"] {
                let associated = ok(
                    &router,
                    "ListAssociatedApprovalRuleTemplatesForRepository",
                    &json!({ "repositoryName": repository }),
                )
                .await;
                assert_eq!(
                    associated.get("approvalRuleTemplateNames"),
                    Some(&json!([rename.new_name])),
                    "case {case:?}: {repository} must be associated with the new name, got \
                     {associated}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_rename_onto_a_name_the_account_holds_is_refused_and_moves_nothing() {
    let router = emulator();
    // Two templates with DIFFERENT content, so a template that moved where it must
    // not have is visible in the content each name reads back.
    let occupying = create(&router, "occupied-name", 1, Some("the occupant")).await;
    let moving = create(&router, ORIGINAL, 2, None).await;

    // Rows differ along WHICH held name the rename asks for: another template's, and
    // the renamed template's own — a name the account holds either way.
    for (case, new_name) in [
        ("another template's name", "occupied-name"),
        ("the template's own name", ORIGINAL),
    ] {
        let (code, message) = error_with_message(
            &router,
            "UpdateApprovalRuleTemplateName",
            &json!({
                "oldApprovalRuleTemplateName": ORIGINAL,
                "newApprovalRuleTemplateName": new_name,
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateNameAlreadyExistsException",
            "case {case:?}: expected the name-taken refusal, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message, NAME_TAKEN_MESSAGE,
            "case {case:?}: expected the exact sentence AWS words this class with, got {message:?}"
        );
        // The sentence names neither template.
        for named in ["occupied-name", ORIGINAL] {
            assert!(
                !message.contains(named),
                "case {case:?}: the message must name no template, but {message:?} carries \
                 {named:?}"
            );
        }
        // Both templates are exactly as they were.
        assert_eq!(
            template_of(&router, "occupied-name").await,
            occupying,
            "case {case:?}: the occupying template must be untouched"
        );
        assert_eq!(
            template_of(&router, ORIGINAL).await,
            moving,
            "case {case:?}: the renamed template must be untouched"
        );
    }

    // A name differing from the held one ONLY in case is a name the account does not
    // hold, so that rename goes through.
    let cased = ok(
        &router,
        "UpdateApprovalRuleTemplateName",
        &json!({
            "oldApprovalRuleTemplateName": ORIGINAL,
            "newApprovalRuleTemplateName": "OCCUPIED-NAME",
        }),
    )
    .await;
    assert_eq!(
        string_at(&cased, "/approvalRuleTemplate/approvalRuleTemplateName"),
        "OCCUPIED-NAME",
        "case \"a name differing only in case\": expected the rename to go through, got {cased}"
    );

    // And once the occupying template is released, the name it held is free: the
    // rename that was refused above now goes through, and the template that moves
    // keeps its OWN content and digest rather than the occupant's.
    ok(
        &router,
        "DeleteApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": "occupied-name" }),
    )
    .await;
    let freed = ok(
        &router,
        "UpdateApprovalRuleTemplateName",
        &json!({
            "oldApprovalRuleTemplateName": "OCCUPIED-NAME",
            "newApprovalRuleTemplateName": "occupied-name",
        }),
    )
    .await;
    let freed = freed
        .get("approvalRuleTemplate")
        .expect("the renamed template")
        .clone();
    assert_eq!(
        freed.get("approvalRuleTemplateName").and_then(Value::as_str),
        Some("occupied-name"),
        "case \"the released name\": expected the rename to go through, got {freed}"
    );
    for member in INVARIANT_MEMBERS {
        assert_eq!(
            freed.get(member),
            moving.get(member),
            "case \"the released name\": the moved template keeps its own {member}, got {freed}"
        );
    }
}

#[tokio::test]
async fn a_rename_naming_no_template_or_no_name_at_all_is_refused() {
    let router = emulator();
    create(&router, ORIGINAL, 2, None).await;

    // Rows differ along which member is at fault and why: an old name nothing was
    // created under (several forms), a name outside the published length bound at
    // either end, and a name that holds characters but names no template.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an old name nothing was created under",
            json!({
                "oldApprovalRuleTemplateName": "never-created",
                "newApprovalRuleTemplateName": "whatever",
            }),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "an old name at the published maximum length that was never created",
            json!({
                "oldApprovalRuleTemplateName": "z".repeat(MAX_NAME_CHARACTERS),
                "newApprovalRuleTemplateName": "whatever",
            }),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "an old name differing from the stored one only in case",
            json!({
                "oldApprovalRuleTemplateName": ORIGINAL.to_uppercase(),
                "newApprovalRuleTemplateName": "whatever",
            }),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "no old name at all",
            json!({ "newApprovalRuleTemplateName": "whatever" }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "no new name at all",
            json!({ "oldApprovalRuleTemplateName": ORIGINAL }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "an empty old name: one character below the published minimum",
            json!({
                "oldApprovalRuleTemplateName": "",
                "newApprovalRuleTemplateName": "whatever",
            }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "an empty new name: one character below the published minimum",
            json!({
                "oldApprovalRuleTemplateName": ORIGINAL,
                "newApprovalRuleTemplateName": "",
            }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "a new name one character past the published maximum",
            json!({
                "oldApprovalRuleTemplateName": ORIGINAL,
                "newApprovalRuleTemplateName": "n".repeat(MAX_NAME_CHARACTERS + 1),
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "an old name one character past the published maximum",
            json!({
                "oldApprovalRuleTemplateName": "n".repeat(MAX_NAME_CHARACTERS + 1),
                "newApprovalRuleTemplateName": "whatever",
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a new name of nothing but space",
            json!({
                "oldApprovalRuleTemplateName": ORIGINAL,
                "newApprovalRuleTemplateName": "   ",
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a new name carrying a control character",
            json!({
                "oldApprovalRuleTemplateName": ORIGINAL,
                "newApprovalRuleTemplateName": "two\u{0007}reviewers",
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
    ];

    for (case, body, expected) in rows {
        let code = error(
            &router,
            "UpdateApprovalRuleTemplateName",
            &body,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?}"
        );
    }

    // Every refusal above wrote nothing: the account still holds the one template it
    // started with, under the name it was created with.
    assert_eq!(
        listed_names(&router).await,
        vec![ORIGINAL.to_string()],
        "a refused rename must move nothing"
    );
}
