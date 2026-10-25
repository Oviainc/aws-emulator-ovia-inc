// SPDX-License-Identifier: CC0-1.0

//! Approval rules on a pull request, over the wire: `CreatePullRequestApprovalRule`
//! and `DeletePullRequestApprovalRule` read back through `GetPullRequest`, driven as
//! awsJson1_1 requests through the router that serves the real emulator (the wire
//! harness comes from the sibling `repository_lifecycle` suite rather than a second
//! copy).
//!
//! The rule under test: an approval rule's ORIGIN decides whether it can be removed
//! from a pull request. A rule created on the pull request itself carries no
//! `originApprovalRuleTemplate` and is deleted — HTTP 200 with its `approvalRuleId`,
//! and gone from every later read — while a rule an associated approval rule
//! template generated is refused with HTTP 400
//! `CannotDeleteApprovalRuleFromTemplateException` and STAYS attached. Deleting a
//! name the pull request holds no rule under is the state a delete leaves behind, so
//! it answers 200 with no rule id rather than a refusal.
//!
//! The tables below span what that rule generalises over: the rule's origin, the
//! name's size and character kind (the 1- and 100-character bounds of
//! `ApprovalRuleName` — `smithy_query(query="shape",shape="ApprovalRuleName")` gives
//! `len 1..100` and no pattern), the document's shape and formatting, how many rules
//! the pull request holds, and the state of the pull request itself.

use std::sync::Arc;

use serde_json::{json, Value};
use sha2::Digest;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator_with_engine, error, ok, string_at};
use crate::wire::caller_arn;
use crate::wire::DEFAULT_ACCESS_KEY_ID;

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// `ApprovalRuleName`'s modelled bounds (`len 1..100`).
const MIN_RULE_NAME: usize = 1;
const MAX_RULE_NAME: usize = 100;

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// A `PullRequestStatusEnum` value other than `OPEN`
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const STATUS_CLOSED: &str = "CLOSED";

/// The one sentence `CannotDeleteApprovalRuleFromTemplateException` carries,
/// whichever rule and template the refused request named.
const REFUSED_MESSAGE: &str = "The approval rule cannot be deleted from the pull request because \
                               it was created by an approval rule template and automatically \
                               applied to the pull request.";

const REPOSITORY: &str = "rules";
const TEMPLATE: &str = "repository-wide-review";

/// The digest `ruleContentSha256` reports — computed here independently of the
/// service, so it is checked against the rule rather than against itself.
fn content_sha256(content: &str) -> String {
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// An approval rule document, as compact JSON text.
fn document(approvals: u32, pool: &[&str]) -> String {
    let mut statement = json!({
        "Type": STATEMENT_TYPE,
        "NumberOfApprovalsNeeded": approvals,
    });
    if !pool.is_empty() {
        statement["ApprovalPoolMembers"] = json!(pool);
    }
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [statement],
    }))
    .expect("a JSON document")
}

/// An emulator holding one repository with two diverged branches and one open pull
/// request on them, plus the engine so a test can read and seed stored state.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    pull_request_id: String,
}

/// Build it. `templates` names the approval rule templates associated with the
/// repository BEFORE the pull request is opened, so each one's rule is applied to
/// it.
async fn fixture(templates: &[&str]) -> Fixture {
    let (router, engine) = emulator_with_engine();
    for template in templates {
        ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": template,
                "approvalRuleTemplateContent": document(1, &[]),
            }),
        )
        .await;
    }
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    for template in templates {
        ok(
            &router,
            "AssociateApprovalRuleTemplateWithRepository",
            &json!({
                "approvalRuleTemplateName": template,
                "repositoryName": REPOSITORY,
            }),
        )
        .await;
    }
    let destination = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "main",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\n"),
            "commitMessage": "the destination version",
        }),
    )
    .await;
    let main_commit = string_at(&destination, "/commitId");
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "commitId": main_commit,
        }),
    )
    .await;
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\nbeta\n"),
            "parentCommitId": main_commit,
            "commitMessage": "the source version",
        }),
    )
    .await;
    let created = ok(
        &router,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": "feature",
                "destinationReference": "main",
            }],
            "clientRequestToken": "fixture-token",
        }),
    )
    .await;
    Fixture {
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
        router,
        engine,
    }
}

impl Fixture {
    /// The approval rules `GetPullRequest` reports for this pull request.
    async fn reported_rules(&self) -> Vec<Value> {
        let read = ok(
            &self.router,
            "GetPullRequest",
            &json!({ "pullRequestId": self.pull_request_id }),
        )
        .await;
        read.pointer("/pullRequest/approvalRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The rule reported under this name, if any.
    async fn reported_rule(&self, name: &str) -> Option<Value> {
        self.reported_rules().await.into_iter().find(|rule| {
            rule.get("approvalRuleName").and_then(Value::as_str) == Some(name)
        })
    }

    async fn create_rule(&self, name: &str, content: &str) -> Value {
        ok(
            &self.router,
            "CreatePullRequestApprovalRule",
            &json!({
                "pullRequestId": self.pull_request_id,
                "approvalRuleName": name,
                "approvalRuleContent": content,
            }),
        )
        .await
    }

    fn delete_body(&self, name: &str) -> Value {
        json!({
            "pullRequestId": self.pull_request_id,
            "approvalRuleName": name,
        })
    }

    /// Move the stored pull request out of `OPEN`, as a close would: the operations
    /// that change a pull request refuse one that is no longer open, and no
    /// implemented operation closes one yet.
    fn close_pull_request(&self) {
        let state = self.engine.state::<CodeCommitState>();
        let mut store = state.lock();
        let id: u64 = self
            .pull_request_id
            .parse()
            .expect("the id the service minted");
        store
            .pull_request_mut(id)
            .expect("the pull request opened above")
            .status = STATUS_CLOSED.to_string();
    }
}

/// One row of the pull-request-local rule table.
struct RuleCase {
    label: &'static str,
    name: String,
    /// The document as SENT, and the canonical form the service stores and reports.
    sent: String,
    canonical: String,
}

fn rule_cases() -> Vec<RuleCase> {
    let one = document(1, &[]);
    let pooled = document(
        3,
        &[
            "arn:aws:iam::000000000000:user/Mary_Major",
            "arn:aws:sts::000000000000:assumed-role/CodeCommitReview/*",
        ],
    );
    vec![
        RuleCase {
            label: "a one-approval rule under a plain name",
            name: "needs-one-approval".to_string(),
            sent: one.clone(),
            canonical: one.clone(),
        },
        RuleCase {
            label: "the 1-character minimum name",
            name: "r".repeat(MIN_RULE_NAME),
            sent: one.clone(),
            canonical: one.clone(),
        },
        RuleCase {
            label: "the 100-character maximum name",
            name: "x".repeat(MAX_RULE_NAME),
            sent: one.clone(),
            canonical: one.clone(),
        },
        RuleCase {
            label: "a name carrying punctuation and non-ASCII letters",
            name: "revisión.de-código_9".to_string(),
            sent: one.clone(),
            canonical: one,
        },
        RuleCase {
            label: "a pooled multi-approval document, sent pretty-printed",
            name: "two-of-the-team".to_string(),
            sent: serde_json::to_string_pretty(
                &serde_json::from_str::<Value>(&pooled).expect("a JSON document"),
            )
            .expect("pretty JSON"),
            canonical: pooled,
        },
    ]
}

#[tokio::test]
async fn a_rule_created_on_the_pull_request_is_reported_then_deleted() {
    let fixture = fixture(&[TEMPLATE]).await;

    for case in rule_cases() {
        let label = case.label;
        let created = fixture.create_rule(&case.name, &case.sent).await;
        let rule_id = string_at(&created, "/approvalRule/approvalRuleId");

        assert_eq!(
            string_at(&created, "/approvalRule/approvalRuleContent"),
            case.canonical,
            "case {label:?}: the rule must report the canonical document it stored"
        );
        assert_eq!(
            string_at(&created, "/approvalRule/ruleContentSha256"),
            content_sha256(&case.canonical),
            "case {label:?}: ruleContentSha256 must be the digest of the stored content"
        );
        assert_eq!(
            string_at(&created, "/approvalRule/lastModifiedUser"),
            caller_arn(DEFAULT_ACCESS_KEY_ID),
            "case {label:?}: a rule a caller creates is attributed to that caller"
        );
        assert_eq!(
            created.pointer("/approvalRule/originApprovalRuleTemplate"),
            None,
            "case {label:?}: a rule created on the pull request has no origin template, \
             got {created}"
        );

        // The round trip: the reader answers the rule the create stored.
        let reported = fixture
            .reported_rule(&case.name)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: the created rule must be reported"));
        assert_eq!(
            reported,
            created
                .pointer("/approvalRule")
                .cloned()
                .expect("the created rule"),
            "case {label:?}: the reader must answer exactly what the create returned"
        );

        // And the delete answers that rule's id and takes it out of every read.
        let deleted = ok(
            &fixture.router,
            "DeletePullRequestApprovalRule",
            &fixture.delete_body(&case.name),
        )
        .await;
        assert_eq!(
            string_at(&deleted, "/approvalRuleId"),
            rule_id,
            "case {label:?}: the delete must answer the deleted rule's id, got {deleted}"
        );
        assert_eq!(
            fixture.reported_rule(&case.name).await,
            None,
            "case {label:?}: the deleted rule must be gone from the pull request"
        );
        // The template-derived rule is untouched by any of this.
        assert!(
            fixture.reported_rule(TEMPLATE).await.is_some(),
            "case {label:?}: the template-derived rule must stay attached"
        );
    }
}

#[tokio::test]
async fn a_template_derived_rule_is_refused_and_stays_attached() {
    // Rows differ along how many templates the repository is associated with, so
    // each pull request carries one and two template-derived rules.
    for templates in [vec![TEMPLATE], vec![TEMPLATE, "second-template"]] {
        let fixture = fixture(&templates).await;
        let before = fixture.reported_rules().await;
        assert_eq!(
            before.len(),
            templates.len(),
            "case {templates:?}: each associated template applies one rule, got {before:?}"
        );

        for template in &templates {
            let (code, message) = error_with_message(
                &fixture.router,
                "DeletePullRequestApprovalRule",
                &fixture.delete_body(template),
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "CannotDeleteApprovalRuleFromTemplateException",
                "case {template:?}: expected CannotDeleteApprovalRuleFromTemplateException, \
                 got {code:?}"
            );
            // One sentence for the whole class: the refusal names neither the rule
            // nor the template it came from.
            assert_eq!(
                message, REFUSED_MESSAGE,
                "case {template:?}: expected {REFUSED_MESSAGE:?}, got {message:?}"
            );
            let still = fixture.reported_rule(template).await;
            assert_eq!(
                still
                    .as_ref()
                    .and_then(|rule| rule
                        .pointer("/originApprovalRuleTemplate/approvalRuleTemplateName"))
                    .and_then(Value::as_str),
                Some(*template),
                "case {template:?}: the refused rule must stay attached, got {still:?}"
            );
        }

        assert_eq!(
            fixture.reported_rules().await,
            before,
            "case {templates:?}: a refused delete must leave every rule as it was"
        );
    }
}

#[tokio::test]
async fn deleting_a_name_the_pull_request_holds_no_rule_under_answers_no_rule_id() {
    let fixture = fixture(&[]).await;
    fixture.create_rule("once", &document(1, &[])).await;
    let first = ok(
        &fixture.router,
        "DeletePullRequestApprovalRule",
        &fixture.delete_body("once"),
    )
    .await;
    assert!(
        !string_at(&first, "/approvalRuleId").is_empty(),
        "the first delete answers the rule's id, got {first}"
    );

    // Rows differ along why the pull request holds no such rule: the name was never
    // created, and the name was deleted by an earlier call.
    for name in ["never-created", "once"] {
        let answered = ok(
            &fixture.router,
            "DeletePullRequestApprovalRule",
            &fixture.delete_body(name),
        )
        .await;
        let rule_id = answered
            .pointer("/approvalRuleId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            rule_id.is_empty(),
            "case {name:?}: a rule the pull request does not hold leaves no id to answer, \
             got {rule_id:?}"
        );
    }
}

/// One row of the refusal table: the operation, the body to send and the error code
/// it must answer with.
struct Refusal {
    label: &'static str,
    operation: &'static str,
    body: Value,
    code: &'static str,
}

#[tokio::test]
async fn a_request_naming_no_rule_the_pull_request_can_take_is_refused() {
    let fixture = fixture(&[TEMPLATE]).await;
    fixture.create_rule("already-here", &document(1, &[])).await;
    let valid = document(1, &[]);

    // Rows differ along which member names nothing a rule can be created or deleted
    // against: the pull request id's form, a name of nothing but space, a name the
    // pull request already holds, and a document that is not an approval rule
    // document.
    let refusals = vec![
        Refusal {
            label: "a blank pull request id",
            operation: "DeletePullRequestApprovalRule",
            body: json!({ "pullRequestId": "", "approvalRuleName": "already-here" }),
            code: "PullRequestIdRequiredException",
        },
        Refusal {
            label: "a pull request id that is not a decimal integer",
            operation: "DeletePullRequestApprovalRule",
            body: json!({ "pullRequestId": "not-an-id", "approvalRuleName": "already-here" }),
            code: "InvalidPullRequestIdException",
        },
        Refusal {
            label: "a well-formed id nothing was minted for",
            operation: "DeletePullRequestApprovalRule",
            body: json!({ "pullRequestId": "99999", "approvalRuleName": "already-here" }),
            code: "PullRequestDoesNotExistException",
        },
        Refusal {
            label: "a rule name of nothing but space",
            operation: "DeletePullRequestApprovalRule",
            body: fixture.delete_body("   "),
            code: "ApprovalRuleNameRequiredException",
        },
        Refusal {
            label: "a blank pull request id on the create",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": "",
                "approvalRuleName": "another",
                "approvalRuleContent": valid,
            }),
            code: "PullRequestIdRequiredException",
        },
        Refusal {
            label: "a well-formed id nothing was minted for, on the create",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": "99999",
                "approvalRuleName": "another",
                "approvalRuleContent": valid,
            }),
            code: "PullRequestDoesNotExistException",
        },
        Refusal {
            label: "a rule name of nothing but space, on the create",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": " ",
                "approvalRuleContent": valid,
            }),
            code: "ApprovalRuleNameRequiredException",
        },
        Refusal {
            label: "a name the pull request already holds",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": "already-here",
                "approvalRuleContent": valid,
            }),
            code: "ApprovalRuleNameAlreadyExistsException",
        },
        Refusal {
            label: "a name a template's rule already holds",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": TEMPLATE,
                "approvalRuleContent": valid,
            }),
            code: "ApprovalRuleNameAlreadyExistsException",
        },
        Refusal {
            label: "content of nothing but space",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": "another",
                "approvalRuleContent": "   ",
            }),
            code: "ApprovalRuleContentRequiredException",
        },
        Refusal {
            label: "content that is not JSON at all",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": "another",
                "approvalRuleContent": "two approvals please",
            }),
            code: "InvalidApprovalRuleContentException",
        },
        Refusal {
            label: "a document with no statements",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": "another",
                "approvalRuleContent": json!({
                    "Version": DOCUMENT_VERSION,
                    "Statements": [],
                })
                .to_string(),
            }),
            code: "InvalidApprovalRuleContentException",
        },
        Refusal {
            label: "a document asking for no approvals",
            operation: "CreatePullRequestApprovalRule",
            body: json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": "another",
                "approvalRuleContent": document(0, &[]),
            }),
            code: "InvalidApprovalRuleContentException",
        },
    ];

    let before = fixture.reported_rules().await;
    for refusal in refusals {
        let label = refusal.label;
        let code = error(
            &fixture.router,
            refusal.operation,
            &refusal.body,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, refusal.code,
            "case {label:?}: expected {}, got {code:?}",
            refusal.code
        );
    }
    assert_eq!(
        fixture.reported_rules().await,
        before,
        "a refused request must leave the pull request's rules as they were"
    );
}

#[tokio::test]
async fn a_pull_request_that_is_no_longer_open_takes_and_gives_up_no_rule() {
    let fixture = fixture(&[TEMPLATE]).await;
    fixture.create_rule("local", &document(1, &[])).await;
    let before = fixture.reported_rules().await;
    fixture.close_pull_request();

    // Rows differ along the operation and the rule it names: the pull-request-local
    // rule and the template-derived one, created and deleted alike.
    let cases: Vec<(&str, Value)> = vec![
        (
            "CreatePullRequestApprovalRule",
            json!({
                "pullRequestId": fixture.pull_request_id,
                "approvalRuleName": "added-after-the-close",
                "approvalRuleContent": document(1, &[]),
            }),
        ),
        (
            "DeletePullRequestApprovalRule",
            fixture.delete_body("local"),
        ),
        (
            "DeletePullRequestApprovalRule",
            fixture.delete_body(TEMPLATE),
        ),
    ];
    for (operation, body) in cases {
        let code = error(&fixture.router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "PullRequestAlreadyClosedException",
            "case {operation:?} on a closed pull request: \
             expected PullRequestAlreadyClosedException, got {code:?}"
        );
    }
    assert_eq!(
        fixture.reported_rules().await,
        before,
        "a closed pull request's rules must be exactly what they were"
    );
}
