// SPDX-License-Identifier: CC0-1.0

//! `UpdateApprovalRuleTemplateDescription` over the wire, driven as awsJson1_1
//! requests through the router that serves the real emulator.
//!
//! The rule under test: `UpdateApprovalRuleTemplateDescription` sets the named
//! template's stored description to exactly the text the request carries — and an
//! EMPTY text leaves the template with no description at all, so
//! `approvalRuleTemplateDescription` is absent from the answer and from every later
//! read (the recorded behaviour the corpus names
//! `codecommit.approval-rule-template-empty-description-normalized-to-absent`) —
//! while nothing else about the template changes: its id, name, content,
//! `ruleContentSha256` and `creationDate` carry over, `lastModifiedDate` advances to
//! the instant of the change, `lastModifiedUser` becomes the calling identity, and
//! no other template moves.
//!
//! The tables below span the class that rule covers: the text absent-by-emptiness,
//! at both modelled length bounds and past the upper one, over unicode and HTML
//! content, replacing a value, clearing it and setting one after it was cleared,
//! from both starting states (a template created with a description and one created
//! without) — rather than one recorded request.
//!
//! The sibling suite `tests/all/approval_rule_templates.rs` pins the same
//! normalization on CREATE, and `tests/all/approval_rules_and_commits.rs` owns the
//! template lifecycle; neither is repeated here.

use std::time::Duration;

use serde_json::{json, Value};

// The valid template document the sibling template suite already builds, so both
// suites create templates from one definition of "valid content".
use crate::approval_rule_templates::document;
use crate::harness::seconds_at;
use crate::repository_lifecycle::{emulator, error, ok, string_at};

/// The `approvalRuleTemplate` a `GetApprovalRuleTemplate` read reports — the
/// projection a stored template is read back through.
async fn template_of(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> Value {
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

/// Members of `ApprovalRuleTemplate` that a description change must not touch
/// (`smithy_query(query="shape",shape="ApprovalRuleTemplate")`); `lastModifiedDate`
/// and `lastModifiedUser` are the two that the change DOES move, and they are
/// asserted separately.
const INVARIANT_MEMBERS: [&str; 5] = [
    "approvalRuleTemplateId",
    "approvalRuleTemplateName",
    "approvalRuleTemplateContent",
    "ruleContentSha256",
    "creationDate",
];

/// The table. Rows differ along what the rule generalises over: the text the
/// request carries — both modelled length bounds of
/// `ApprovalRuleTemplateDescription` (0 and 1000, from
/// `smithy_query(query="operation",operation="UpdateApprovalRuleTemplateDescription")`:
/// `length` 0..1000), unicode and HTML content, and the transitions between them.
/// Each row carries what a read must then report: `None` means the member is ABSENT.
/// The rows are applied in sequence to one template, so each also exercises
/// replacing the previous value — including setting one after it was cleared.
fn description_cases() -> Vec<(&'static str, String, Option<String>)> {
    vec![
        (
            "ordinary text",
            "two reviewers on every pull request".to_string(),
            Some("two reviewers on every pull request".to_string()),
        ),
        (
            "the modelled minimum length: an empty description is held as none",
            String::new(),
            None,
        ),
        (
            "set again after the empty one cleared it",
            "restored after clearing".to_string(),
            Some("restored after clearing".to_string()),
        ),
        (
            "a single character",
            "x".to_string(),
            Some("x".to_string()),
        ),
        (
            "unicode text",
            "revisión obligatoria ⚙ \u{1F1E6}\u{1F1FA}".to_string(),
            Some("revisión obligatoria ⚙ \u{1F1E6}\u{1F1FA}".to_string()),
        ),
        (
            "html-looking text, stored verbatim",
            "<b>review</b> &amp; approve".to_string(),
            Some("<b>review</b> &amp; approve".to_string()),
        ),
        (
            "the same text again: an idempotent write",
            "<b>review</b> &amp; approve".to_string(),
            Some("<b>review</b> &amp; approve".to_string()),
        ),
        (
            "the modelled maximum length",
            "z".repeat(1000),
            Some("z".repeat(1000)),
        ),
        (
            "whitespace is text, not emptiness",
            "   ".to_string(),
            Some("   ".to_string()),
        ),
        (
            "emptied again from a non-empty value",
            String::new(),
            None,
        ),
    ]
}

#[tokio::test]
async fn an_updated_template_description_is_read_back_from_state() {
    let router = emulator();

    // Two starting states — a template created WITH a description and one created
    // without — plus a third that is never updated, so the write is shown to reach
    // only the template the request names.
    for (name, description) in [
        ("with-description", Some("created with this")),
        ("without-description", None),
        ("untouched", Some("never updated")),
    ] {
        let mut input = json!({
            "approvalRuleTemplateName": name,
            "approvalRuleTemplateContent": document(2),
        });
        if let Some(description) = description {
            input["approvalRuleTemplateDescription"] = json!(description);
        }
        ok(&router, "CreateApprovalRuleTemplate", &input).await;
    }
    let untouched_before = template_of(&router, "untouched").await;
    let cases = description_cases();

    for target in ["with-description", "without-description"] {
        let identity = template_of(&router, target).await;
        let created = seconds_at(&identity, "creationDate");
        let mut previous_modified = seconds_at(&identity, "lastModifiedDate");

        for (case, sent, stored) in &cases {
            let label = format!("{target}: {case}");
            // Separate the modification instants so lastModifiedDate strictly orders.
            tokio::time::sleep(Duration::from_millis(5)).await;

            let response = ok(
                &router,
                "UpdateApprovalRuleTemplateDescription",
                &json!({
                    "approvalRuleTemplateName": target,
                    "approvalRuleTemplateDescription": sent,
                }),
            )
            .await;
            let answered = response
                .get("approvalRuleTemplate")
                .unwrap_or_else(|| panic!("case {label:?}: no approvalRuleTemplate in {response}"))
                .clone();
            assert_eq!(
                answered
                    .get("approvalRuleTemplateDescription")
                    .and_then(Value::as_str),
                stored.as_deref(),
                "case {label:?}: the answered description must be the stored one, got {answered}"
            );

            // The normalization is in what was PERSISTED, not in one response: the
            // stored template reads back exactly the same way.
            let after = template_of(&router, target).await;
            assert_eq!(
                after.get("approvalRuleTemplateDescription"),
                answered.get("approvalRuleTemplateDescription"),
                "case {label:?}: the description read back must be the one answered, got {after}"
            );
            assert_eq!(
                after
                    .get("approvalRuleTemplateDescription")
                    .and_then(Value::as_str),
                stored.as_deref(),
                "case {label:?}: the stored description must be the text the request carried, got {after}"
            );

            // A read does not change what the next caller sees.
            let again = template_of(&router, target).await;
            assert_eq!(
                again, after,
                "case {label:?}: reading the template must not change it, got {again}"
            );

            for member in INVARIANT_MEMBERS {
                assert_eq!(
                    after.get(member),
                    identity.get(member),
                    "case {label:?}: {member} must not change, expected {:?}, got {:?}",
                    identity.get(member),
                    after.get(member)
                );
            }

            let modified = seconds_at(&after, "lastModifiedDate");
            assert!(
                modified > previous_modified,
                "case {label:?}: lastModifiedDate must advance to the instant of the change, expected later than {previous_modified}, got {modified}"
            );
            assert!(
                modified > created,
                "case {label:?}: lastModifiedDate must be later than creationDate {created}, got {modified}"
            );
            previous_modified = modified;
        }
    }

    let untouched_after = template_of(&router, "untouched").await;
    assert_eq!(
        untouched_after, untouched_before,
        "updating one template's description must not touch another, expected {untouched_before}, got {untouched_after}"
    );
}

#[tokio::test]
async fn the_change_records_the_calling_identity_as_the_last_modifier() {
    let router = emulator();
    let creator = "AKIDCREATOR";
    let updater = "AKIDUPDATER";

    crate::wire::ok_as(
        &router,
        creator,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "attributed",
            "approvalRuleTemplateContent": document(1),
            "approvalRuleTemplateDescription": "created by one principal",
        }),
    )
    .await;
    let created = template_of(&router, "attributed").await;
    assert_eq!(
        string_at(&created, "/lastModifiedUser"),
        crate::wire::caller_arn(creator),
        "the creator is the template's first lastModifiedUser, got {created}"
    );

    // Each caller's own update is attributed to that caller, so lastModifiedUser is
    // read from the request rather than fixed at creation.
    for principal in [updater, creator] {
        let answered = crate::wire::ok_as(
            &router,
            principal,
            "UpdateApprovalRuleTemplateDescription",
            &json!({
                "approvalRuleTemplateName": "attributed",
                "approvalRuleTemplateDescription": format!("set by {principal}"),
            }),
        )
        .await;
        assert_eq!(
            string_at(&answered, "/approvalRuleTemplate/lastModifiedUser"),
            crate::wire::caller_arn(principal),
            "case {principal:?}: the answered lastModifiedUser, got {answered}"
        );
        let stored = template_of(&router, "attributed").await;
        assert_eq!(
            string_at(&stored, "/lastModifiedUser"),
            crate::wire::caller_arn(principal),
            "case {principal:?}: the stored lastModifiedUser, got {stored}"
        );
    }
}

#[tokio::test]
async fn a_description_update_is_refused_with_the_modeled_error() {
    let router = emulator();
    ok(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "guarded",
            "approvalRuleTemplateContent": document(3),
            "approvalRuleTemplateDescription": "the stored description",
        }),
    )
    .await;
    // Each row is a request outside the operation's contract and the error this model
    // declares for it: a template that does not exist, an `approvalRuleTemplateName`
    // outside its modelled `length` 1..100 or naming no template at all, and an
    // `approvalRuleTemplateDescription` one character past its modelled maximum of
    // 1000.
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "a template that was never created",
            json!({
                "approvalRuleTemplateName": "absent-template",
                "approvalRuleTemplateDescription": "anything",
            }),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "a name that differs from a stored one only in case",
            json!({
                "approvalRuleTemplateName": "GUARDED",
                "approvalRuleTemplateDescription": "anything",
            }),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "no approvalRuleTemplateName at all",
            json!({ "approvalRuleTemplateDescription": "anything" }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "a name shorter than the modelled minimum of 1",
            json!({
                "approvalRuleTemplateName": "",
                "approvalRuleTemplateDescription": "anything",
            }),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "a name longer than the modelled maximum of 100",
            json!({
                "approvalRuleTemplateName": "n".repeat(101),
                "approvalRuleTemplateDescription": "anything",
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a name of nothing but whitespace names no template",
            json!({
                "approvalRuleTemplateName": "   ",
                "approvalRuleTemplateDescription": "anything",
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a name carrying a control character",
            json!({
                "approvalRuleTemplateName": "guarded\u{7}",
                "approvalRuleTemplateDescription": "anything",
            }),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "a description one character past the modelled maximum of 1000",
            json!({
                "approvalRuleTemplateName": "guarded",
                "approvalRuleTemplateDescription": "z".repeat(1001),
            }),
            "InvalidApprovalRuleTemplateDescriptionException",
        ),
    ];

    for (label, input, expected) in refusals {
        let code = error(
            &router,
            "UpdateApprovalRuleTemplateDescription",
            &input,
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }

    // A refused update persists nothing: the guarded template keeps the description
    // it was created with.
    let after = template_of(&router, "guarded").await;
    assert_eq!(
        after
            .get("approvalRuleTemplateDescription")
            .and_then(Value::as_str),
        Some("the stored description"),
        "a refused update must leave the stored description in place, got {after}"
    );
}
