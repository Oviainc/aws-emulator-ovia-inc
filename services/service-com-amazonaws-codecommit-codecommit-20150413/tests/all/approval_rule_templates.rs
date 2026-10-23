// SPDX-License-Identifier: CC0-1.0

//! The approval rule template rules that `tests/all/approval_rules_and_commits.rs`
//! (which owns this service's template lifecycle suite) does not pin: how an EMPTY
//! description is held, and what each published bound of the three input members
//! accepts and refuses.
//!
//! The rules under test, over awsJson1_1 requests through the router that serves the
//! real emulator:
//!
//! 1. A template carries no empty description: created with one, the stored template
//!    simply has none, so `approvalRuleTemplateDescription` is ABSENT on the create
//!    answer and on every later read (the recorded behaviour the corpus names
//!    `codecommit.approval-rule-template-empty-description-normalized-to-absent`) —
//!    asserted over a table of description lengths rather than one value.
//! 2. Each published bound of `ApprovalRuleTemplateName` (length 1..100),
//!    `ApprovalRuleTemplateDescription` (0..1000) and `ApprovalRuleTemplateContent`
//!    (1..3000) is accepted, and one character past it is refused with the error this
//!    model declares for that member.
//!
//! The refusal of content that is not an approval rule document, and the lifecycle of
//! a stored template, are pinned by that sibling suite rather than repeated here.

use serde_json::{json, Value};

// The harness `tests/all/repository_lifecycle.rs` owns and shares with every sibling
// suite: one emulator on one engine, and the awsJson1_1 call helpers.
use crate::repository_lifecycle::{emulator, error, ok, string_at};

/// A valid approval rule template document with `approvals` approvers needed. The
/// structure (`Version`, `Statements` of `Type` `Approvers` with
/// `NumberOfApprovalsNeeded` and `ApprovalPoolMembers`) is the one the CodeCommit
/// User Guide documents for approval rule template content.
///
/// `pub(crate)` so the sibling suite that updates a template's description creates
/// its templates from this same definition of valid content.
pub(crate) fn document(approvals: u32) -> String {
    serde_json::to_string(&json!({
        "Version": "2018-11-08",
        "Statements": [{
            "Type": "Approvers",
            "NumberOfApprovalsNeeded": approvals,
            "ApprovalPoolMembers": ["arn:aws:sts::123456789012:assumed-role/CodeCommitReview/*"],
        }],
    }))
    .expect("the document serializes")
}

#[tokio::test]
async fn an_empty_template_description_is_held_as_no_description() {
    let router = emulator();

    // (case, the description as sent — `None` omits the member, what a read must
    // report — `None` means the member is absent). The rows differ along what the
    // rule generalises over: whether the member is sent at all, and the length and
    // kind of what it carries, including the modelled maximum of 1000.
    let cases: [(&str, Option<String>, Option<String>); 6] = [
        ("the member is omitted", None, None),
        ("an empty description", Some(String::new()), None),
        (
            "one character",
            Some("d".to_string()),
            Some("d".to_string()),
        ),
        (
            "ordinary text",
            Some("two reviewers on every pull request".to_string()),
            Some("two reviewers on every pull request".to_string()),
        ),
        (
            "unicode text",
            Some("revisión obligatoria ⚙".to_string()),
            Some("revisión obligatoria ⚙".to_string()),
        ),
        (
            "the modelled maximum",
            Some("d".repeat(1000)),
            Some("d".repeat(1000)),
        ),
    ];

    for (index, (case, sent, stored)) in cases.into_iter().enumerate() {
        let name = format!("described-{index}");
        let mut input = json!({
            "approvalRuleTemplateName": name,
            "approvalRuleTemplateContent": document(2),
        });
        if let Some(description) = &sent {
            input["approvalRuleTemplateDescription"] = json!(description);
        }

        let created = ok(&router, "CreateApprovalRuleTemplate", &input).await;
        assert_eq!(
            created
                .pointer("/approvalRuleTemplate/approvalRuleTemplateDescription")
                .and_then(Value::as_str),
            stored.as_deref(),
            "case {case:?}: the created description, got {created}"
        );

        // And the stored template reads back the same way: the normalization is in
        // what was PERSISTED, not in one response.
        let fetched = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": name }),
        )
        .await;
        assert_eq!(
            fetched
                .pointer("/approvalRuleTemplate/approvalRuleTemplateDescription")
                .and_then(Value::as_str),
            stored.as_deref(),
            "case {case:?}: the description read back, got {fetched}"
        );
    }
}

#[tokio::test]
async fn each_published_bound_of_the_input_members_is_accepted_and_one_past_it_refused() {
    let router = emulator();
    let valid = document(1);

    // `ApprovalRuleTemplateName`, length 1..100
    // (`smithy_query(query="operation",operation="CreateApprovalRuleTemplate")`).
    for length in [1usize, 2, 99, 100] {
        let name = "b".repeat(length);
        let created = ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": valid,
            }),
        )
        .await;
        assert_eq!(
            string_at(&created, "/approvalRuleTemplate/approvalRuleTemplateName"),
            name,
            "case name length {length}: must be accepted"
        );
    }
    let code = error(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "c".repeat(101),
            "approvalRuleTemplateContent": valid,
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidApprovalRuleTemplateNameException",
        "a name one character past the modelled maximum"
    );
    for (case, body) in [
        (
            "an empty name",
            json!({
                "approvalRuleTemplateName": "",
                "approvalRuleTemplateContent": valid,
            }),
        ),
        (
            "no name member",
            json!({ "approvalRuleTemplateContent": valid }),
        ),
    ] {
        let code = error(&router, "CreateApprovalRuleTemplate", &body, 400).await;
        assert_eq!(
            code, "ApprovalRuleTemplateNameRequiredException",
            "case {case:?}: names no template"
        );
    }

    // `ApprovalRuleTemplateDescription`, length 0..1000 (the accepted lengths are
    // covered by the description table above; this is the first value past the bound).
    let code = error(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "too-long-description",
            "approvalRuleTemplateContent": valid,
            "approvalRuleTemplateDescription": "d".repeat(1001),
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidApprovalRuleTemplateDescriptionException",
        "a description one character past the modelled maximum"
    );

    // `ApprovalRuleTemplateContent`, length 1..3000: a document padded to exactly
    // 3000 characters is accepted, one more is refused. The padded document is
    // written minified, so it is already in the canonical form the service stores
    // (the canonicalization itself is pinned by
    // `tests/all/approval_rule_template_content_canonicalization.rs`) and the stored
    // content is the submitted text character for character.
    let padded = |total: usize| -> String {
        let prefix = "{\"Version\":\"2018-11-08\",\"Statements\":[{\"Type\":\"Approvers\",\"NumberOfApprovalsNeeded\":1,\"ApprovalPoolMembers\":[\"";
        let suffix = "\"]}]}";
        let pad = total - prefix.len() - suffix.len();
        format!("{prefix}{}{suffix}", "p".repeat(pad))
    };
    let at_bound = padded(3000);
    assert_eq!(at_bound.len(), 3000, "the padded document is at the bound");
    let created = ok(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "content-at-bound",
            "approvalRuleTemplateContent": at_bound,
        }),
    )
    .await;
    assert_eq!(
        string_at(&created, "/approvalRuleTemplate/approvalRuleTemplateContent"),
        at_bound,
        "a minified document at the modelled maximum length is stored as submitted"
    );
    let code = error(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "content-past-bound",
            "approvalRuleTemplateContent": padded(3001),
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidApprovalRuleTemplateContentException",
        "a document one character past the modelled maximum"
    );
    for (case, body) in [
        (
            "an empty content member",
            json!({
                "approvalRuleTemplateName": "empty-content",
                "approvalRuleTemplateContent": "",
            }),
        ),
        (
            "no content member",
            json!({ "approvalRuleTemplateName": "no-content" }),
        ),
    ] {
        let code = error(&router, "CreateApprovalRuleTemplate", &body, 400).await;
        assert_eq!(
            code, "ApprovalRuleTemplateContentRequiredException",
            "case {case:?}: content is required"
        );
    }
}
