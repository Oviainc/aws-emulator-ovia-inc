// SPDX-License-Identifier: CC0-1.0

//! `CreatePullRequest` and `GetPullRequest` over the wire, driven as awsJson1_1
//! requests through the router that serves the real emulator (the wire harness comes
//! from the sibling `repository_lifecycle` suite rather than a second copy).
//!
//! The rule under test: `CreatePullRequest` persists a pull request between two
//! branches of ONE stored repository and answers with the stored record — a
//! per-account id, `OPEN`, the caller as author, both references fully qualified, the
//! tips those branches point at in stored state, their merge base, and the approval
//! rules the repository's associated templates apply — so a later `GetPullRequest`
//! returns exactly what the create returned, and a request naming a reference the
//! repository does not hold as a branch persists nothing.
//!
//! The tables below span what that rule generalises over — the form each reference
//! takes (short name or fully qualified), whether the destination is named or left to
//! the default branch, the description's presence and emptiness, how far the two
//! branches have diverged, how many targets and repositories a request names, and
//! each reference that names no branch — rather than one recorded request.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::ops::types::UserInfo;
use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    Branch, FileEntry, Repository, FILE_MODE_NORMAL,
};
use service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::repository_lifecycle::{
    base64, emulator, emulator_with_engine, error, ok, string_at, REGION,
};
use crate::wire::{caller_arn, error_as, ok_as, DEFAULT_ACCESS_KEY_ID};

/// `PullRequestStatusEnum`'s values
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const STATUS_OPEN: &str = "OPEN";

/// The schema version of an approval rule template document and the only statement
/// type its schema defines, as the CodeCommit User Guide documents them (and
/// `src/state.rs`'s `APPROVAL_RULE_TEMPLATE_VERSION` /
/// `APPROVAL_RULE_STATEMENT_TYPE`).
const TEMPLATE_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

const REPOSITORY: &str = "pulls";

/// A repository holding `main` (the default branch, since it took the first commit)
/// and `feature`, each at its own tip, with a shared base commit.
struct Diverged {
    router: MultiServiceRouter,
    base_commit: String,
    main_commit: String,
    feature_commit: String,
}

/// Create a repository with `main` at a base commit and `feature` branched from it,
/// then move each side as many commits as asked. A side moved zero commits keeps the
/// base commit as its tip.
async fn diverged(repository: &str, main_commits: usize, feature_commits: usize) -> Diverged {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base = ok(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "commitMessage": "base",
            "putFiles": [{ "filePath": "shared.txt", "fileContent": base64(b"base\n") }],
        }),
    )
    .await;
    let base_commit = string_at(&base, "/commitId");
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": "feature",
            "commitId": base_commit,
        }),
    )
    .await;

    let mut main_commit = base_commit.clone();
    for step in 0..main_commits {
        main_commit = commit_on(&router, repository, "main", &main_commit, step).await;
    }
    let mut feature_commit = base_commit.clone();
    for step in 0..feature_commits {
        feature_commit = commit_on(&router, repository, "feature", &feature_commit, step).await;
    }
    Diverged {
        router,
        base_commit,
        main_commit,
        feature_commit,
    }
}

async fn commit_on(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    parent: &str,
    step: usize,
) -> String {
    let committed = ok(
        router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": branch,
            "parentCommitId": parent,
            "commitMessage": format!("{branch} {step}"),
            "putFiles": [{
                "filePath": format!("{branch}.txt"),
                "fileContent": base64(format!("{branch} {step}\n").as_bytes()),
            }],
        }),
    )
    .await;
    string_at(&committed, "/commitId")
}

/// A `CreatePullRequest` body naming one target.
fn create_body(
    repository: &str,
    title: &str,
    description: Option<&str>,
    source: &str,
    destination: Option<&str>,
) -> Value {
    let mut target = json!({ "repositoryName": repository, "sourceReference": source });
    if let Some(destination) = destination {
        target["destinationReference"] = json!(destination);
    }
    let mut body = json!({ "title": title, "targets": [target] });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    body
}

fn pull_request_of(payload: &Value, case: &str) -> Value {
    payload
        .get("pullRequest")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no pullRequest in {payload}"))
}

fn target_of(pull_request: &Value, case: &str) -> Value {
    pull_request
        .get("pullRequestTargets")
        .and_then(Value::as_array)
        .and_then(|targets| targets.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no pullRequestTargets in {pull_request}"))
}

/// One row of the creation table.
struct Case<'a> {
    label: &'a str,
    /// How many commits each side moved past the shared base.
    main_commits: usize,
    feature_commits: usize,
    title: &'a str,
    description: Option<&'a str>,
    /// The `sourceReference` the request names, and the `destinationReference`
    /// (`None` leaves it out, so the default branch is the destination).
    source: &'a str,
    destination: Option<&'a str>,
    /// What the stored record must report for `description`: `None` where an empty
    /// one is held as no description at all.
    expected_description: Option<&'a str>,
}

#[tokio::test]
async fn a_created_pull_request_is_the_stored_one_a_later_read_returns() {
    // Rows differ along the form each reference takes, whether the destination is
    // named at all, the description's presence and emptiness, the title's size, and
    // how far the two branches have diverged.
    let cases = vec![
        Case {
            label: "short names on both sides, one commit each",
            main_commits: 1,
            feature_commits: 1,
            title: "Add the feature",
            description: Some("why it should be merged"),
            source: "feature",
            destination: Some("main"),
            expected_description: Some("why it should be merged"),
        },
        Case {
            label: "fully qualified references on both sides",
            main_commits: 1,
            feature_commits: 2,
            title: "Qualified",
            description: None,
            source: "refs/heads/feature",
            destination: Some("refs/heads/main"),
            expected_description: None,
        },
        Case {
            label: "the destination is left to the default branch",
            main_commits: 0,
            feature_commits: 3,
            title: "Default destination",
            description: Some("d"),
            source: "feature",
            destination: None,
            expected_description: Some("d"),
        },
        Case {
            label: "an empty description is held as no description",
            main_commits: 2,
            feature_commits: 1,
            title: "No description",
            description: Some(""),
            source: "refs/heads/feature",
            destination: Some("main"),
            expected_description: None,
        },
        Case {
            label: "a title at the modelled 150-character maximum",
            main_commits: 1,
            feature_commits: 1,
            title: "t".repeat(150).leak(),
            description: None,
            source: "feature",
            destination: Some("refs/heads/main"),
            expected_description: None,
        },
        Case {
            label: "only the source moved: the base IS the destination tip",
            main_commits: 0,
            feature_commits: 1,
            title: "Fast forward shape",
            description: None,
            source: "feature",
            destination: Some("main"),
            expected_description: None,
        },
    ];

    for case in cases {
        let built = diverged(REPOSITORY, case.main_commits, case.feature_commits).await;
        let label = case.label;
        let created = ok(
            &built.router,
            "CreatePullRequest",
            &create_body(
                REPOSITORY,
                case.title,
                case.description,
                case.source,
                case.destination,
            ),
        )
        .await;
        let pull_request = pull_request_of(&created, label);

        let id = pull_request
            .get("pullRequestId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: no pullRequestId in {pull_request}"));
        assert!(
            id.chars().all(|character| character.is_ascii_digit()) && !id.is_empty(),
            "case {label:?}: expected a decimal pullRequestId, got {id:?}"
        );
        assert_eq!(
            pull_request.get("title").and_then(Value::as_str),
            Some(case.title),
            "case {label:?}: expected the submitted title, got {pull_request}"
        );
        assert_eq!(
            pull_request.get("description").and_then(Value::as_str),
            case.expected_description,
            "case {label:?}: expected description {:?}, got {pull_request}",
            case.expected_description
        );
        assert_eq!(
            pull_request.get("pullRequestStatus").and_then(Value::as_str),
            Some(STATUS_OPEN),
            "case {label:?}: a new pull request is OPEN, got {pull_request}"
        );
        assert_eq!(
            pull_request.get("authorArn").and_then(Value::as_str),
            Some(caller_arn(DEFAULT_ACCESS_KEY_ID).as_str()),
            "case {label:?}: expected the calling principal as author, got {pull_request}"
        );
        let revision = pull_request
            .get("revisionId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: no revisionId in {pull_request}"));
        assert!(
            !revision.is_empty()
                && revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "case {label:?}: expected a hex revisionId, got {revision:?}"
        );
        let creation = pull_request
            .get("creationDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: no numeric creationDate in {pull_request}"));
        let last_activity = pull_request
            .get("lastActivityDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {label:?}: no numeric lastActivityDate in {pull_request}")
            });
        assert_eq!(
            creation, last_activity,
            "case {label:?}: an untouched pull request's last activity is its creation, got \
             {pull_request}"
        );

        // Both references are reported fully qualified, whichever form the request
        // named, and each tip is the one stored state holds for that branch.
        let target = target_of(&pull_request, label);
        assert_eq!(
            target.get("repositoryName").and_then(Value::as_str),
            Some(REPOSITORY),
            "case {label:?}: expected the target repository, got {target}"
        );
        assert_eq!(
            target.get("sourceReference").and_then(Value::as_str),
            Some("refs/heads/feature"),
            "case {label:?}: expected a qualified sourceReference, got {target}"
        );
        assert_eq!(
            target.get("destinationReference").and_then(Value::as_str),
            Some("refs/heads/main"),
            "case {label:?}: expected a qualified destinationReference, got {target}"
        );
        assert_eq!(
            target.get("sourceCommit").and_then(Value::as_str),
            Some(built.feature_commit.as_str()),
            "case {label:?}: expected the stored source tip, got {target}"
        );
        assert_eq!(
            target.get("destinationCommit").and_then(Value::as_str),
            Some(built.main_commit.as_str()),
            "case {label:?}: expected the stored destination tip, got {target}"
        );
        assert_eq!(
            target.get("mergeBase").and_then(Value::as_str),
            Some(built.base_commit.as_str()),
            "case {label:?}: expected the commit both histories share, got {target}"
        );
        assert_eq!(
            target.pointer("/mergeMetadata/isMerged").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: a new pull request is not merged, got {target}"
        );

        // The round trip: the reader answers the record the create persisted, and
        // reading it twice answers the same thing, so the read changed nothing.
        let read = ok(
            &built.router,
            "GetPullRequest",
            &json!({ "pullRequestId": id }),
        )
        .await;
        assert_eq!(
            pull_request_of(&read, label),
            pull_request,
            "case {label:?}: GetPullRequest must answer the created pull request"
        );
        let read_again = ok(
            &built.router,
            "GetPullRequest",
            &json!({ "pullRequestId": id }),
        )
        .await;
        assert_eq!(
            pull_request_of(&read_again, label),
            pull_request,
            "case {label:?}: a second read must answer the same pull request"
        );
    }
}

/// One row of the refusal table: the body to send and the error code it must answer
/// with, all of them HTTP 400 client errors in this model.
struct Refusal<'a> {
    label: &'a str,
    body: Value,
    code: &'a str,
}

#[tokio::test]
async fn a_request_naming_no_pair_of_stored_branches_is_refused_and_persists_nothing() {
    let built = diverged(REPOSITORY, 1, 1).await;
    let router = &built.router;
    // A second repository, so a request can name two of them.
    ok(router, "CreateRepository", &json!({ "repositoryName": "other" })).await;

    // Rows differ along which member names nothing a pull request can be between:
    // the title, the target list's size and the repositories it spans, each
    // reference's namespace and form, and a branch the repository does not hold.
    let refusals = vec![
        Refusal {
            label: "an empty title",
            body: create_body(REPOSITORY, "", None, "feature", Some("main")),
            code: "TitleRequiredException",
        },
        Refusal {
            label: "a title of nothing but spaces",
            body: create_body(REPOSITORY, "   ", None, "feature", Some("main")),
            code: "TitleRequiredException",
        },
        Refusal {
            label: "no targets at all",
            body: json!({ "title": "t", "targets": [] }),
            code: "TargetsRequiredException",
        },
        Refusal {
            label: "two targets in two repositories",
            body: json!({
                "title": "t",
                "targets": [
                    { "repositoryName": REPOSITORY, "sourceReference": "feature" },
                    { "repositoryName": "other", "sourceReference": "feature" },
                ],
            }),
            code: "MultipleRepositoriesInPullRequestException",
        },
        Refusal {
            label: "two targets in one repository",
            body: json!({
                "title": "t",
                "targets": [
                    { "repositoryName": REPOSITORY, "sourceReference": "feature" },
                    { "repositoryName": REPOSITORY, "sourceReference": "main" },
                ],
            }),
            code: "InvalidTargetsException",
        },
        Refusal {
            label: "the same branch on both sides",
            body: create_body(REPOSITORY, "t", None, "feature", Some("feature")),
            code: "SourceAndDestinationAreSameException",
        },
        Refusal {
            label: "the same branch spelled two ways",
            body: create_body(REPOSITORY, "t", None, "refs/heads/main", Some("main")),
            code: "SourceAndDestinationAreSameException",
        },
        Refusal {
            label: "a repository the account does not hold",
            body: create_body("absent", "t", None, "feature", Some("main")),
            code: "RepositoryDoesNotExistException",
        },
        Refusal {
            label: "a source branch the repository does not hold",
            body: create_body(REPOSITORY, "t", None, "nope", Some("main")),
            code: "ReferenceDoesNotExistException",
        },
        Refusal {
            label: "a destination branch the repository does not hold",
            body: create_body(REPOSITORY, "t", None, "feature", Some("refs/heads/nope")),
            code: "ReferenceDoesNotExistException",
        },
        Refusal {
            label: "a tag reference as the source",
            body: create_body(REPOSITORY, "t", None, "refs/tags/v1", Some("main")),
            code: "ReferenceTypeNotSupportedException",
        },
        Refusal {
            label: "a remote-tracking reference as the destination",
            body: create_body(
                REPOSITORY,
                "t",
                None,
                "feature",
                Some("refs/remotes/origin/main"),
            ),
            code: "ReferenceTypeNotSupportedException",
        },
        Refusal {
            label: "a reference name git would refuse",
            body: create_body(REPOSITORY, "t", None, "bad..name", Some("main")),
            code: "InvalidReferenceNameException",
        },
        Refusal {
            label: "a qualified reference naming no branch at all",
            body: create_body(REPOSITORY, "t", None, "refs/heads/", Some("main")),
            code: "InvalidReferenceNameException",
        },
        Refusal {
            label: "the destination left to a repository with no default branch",
            body: create_body("other", "t", None, "feature", None),
            code: "ReferenceDoesNotExistException",
        },
    ];

    for refusal in refusals {
        let label = refusal.label;
        let code = error(router, "CreatePullRequest", &refusal.body, 400).await;
        assert_eq!(
            code, refusal.code,
            "case {label:?}: expected {}, got {code}",
            refusal.code
        );
    }

    // Nothing above persisted a pull request: the first id the account mints is
    // still the first one.
    let created = ok(
        router,
        "CreatePullRequest",
        &create_body(REPOSITORY, "the first one", None, "feature", Some("main")),
    )
    .await;
    assert_eq!(
        pull_request_of(&created, "after the refusals")
            .get("pullRequestId")
            .and_then(Value::as_str),
        Some("1"),
        "a refused request must persist no pull request, got {created}"
    );
}

#[tokio::test]
async fn pull_request_ids_are_consecutive_per_account_and_only_address_stored_pull_requests() {
    let built = diverged(REPOSITORY, 1, 1).await;
    let router = &built.router;
    // A second repository with the same branch shape, so the ids can be seen to be
    // per account rather than per repository.
    ok(router, "CreateRepository", &json!({ "repositoryName": "second" })).await;
    let base = ok(
        router,
        "CreateCommit",
        &json!({
            "repositoryName": "second",
            "branchName": "main",
            "commitMessage": "base",
            "putFiles": [{ "filePath": "a.txt", "fileContent": base64(b"a\n") }],
        }),
    )
    .await;
    let second_base = string_at(&base, "/commitId");
    ok(
        router,
        "CreateBranch",
        &json!({
            "repositoryName": "second",
            "branchName": "feature",
            "commitId": second_base,
        }),
    )
    .await;
    commit_on(router, "second", "feature", &second_base, 0).await;

    // Rows differ along which repository the pull request is opened in; the ids must
    // still run 1, 2, 3 across them.
    let mut ids: Vec<String> = Vec::new();
    for (index, repository) in [REPOSITORY, "second", REPOSITORY].into_iter().enumerate() {
        let created = ok(
            router,
            "CreatePullRequest",
            &create_body(
                repository,
                &format!("pull request {index}"),
                None,
                "feature",
                Some("main"),
            ),
        )
        .await;
        ids.push(string_at(&created, "/pullRequest/pullRequestId"));
    }
    assert_eq!(
        ids,
        vec!["1".to_string(), "2".to_string(), "3".to_string()],
        "ids are minted consecutively per account, got {ids:?}"
    );

    // Each id reads back the pull request it names, in the repository it was opened
    // in.
    for (index, id) in ids.iter().enumerate() {
        let read = ok(router, "GetPullRequest", &json!({ "pullRequestId": id })).await;
        assert_eq!(
            read.pointer("/pullRequest/title").and_then(Value::as_str),
            Some(format!("pull request {index}").as_str()),
            "id {id} must read back its own pull request, got {read}"
        );
    }

    // Rows differ along WHY an id addresses no pull request: blank, not a decimal
    // integer at all, a signed or fractional spelling, surrounding space, and a
    // well-formed id nothing was minted for.
    let refusals = [
        ("", "PullRequestIdRequiredException"),
        ("   ", "PullRequestIdRequiredException"),
        ("not-an-id", "InvalidPullRequestIdException"),
        ("12.5", "InvalidPullRequestIdException"),
        ("-1", "InvalidPullRequestIdException"),
        ("+1", "InvalidPullRequestIdException"),
        (" 1 ", "InvalidPullRequestIdException"),
        ("1e3", "InvalidPullRequestIdException"),
        ("99999", "PullRequestDoesNotExistException"),
    ];
    for (id, expected) in refusals {
        let code = error(
            router,
            "GetPullRequest",
            &json!({ "pullRequestId": id }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {id:?}: expected {expected}, got {code}"
        );
    }
}

#[tokio::test]
async fn a_repeated_client_request_token_answers_the_first_pull_request() {
    let built = diverged(REPOSITORY, 1, 1).await;
    let router = &built.router;

    let body = |title: &str, token: &str| {
        let mut body = create_body(REPOSITORY, title, None, "feature", Some("main"));
        body["clientRequestToken"] = json!(token);
        body
    };

    let first = ok(router, "CreatePullRequest", &body("one", "token-a")).await;
    let first_id = string_at(&first, "/pullRequest/pullRequestId");

    // The same token with the same parameters answers the pull request the first
    // call created rather than opening a second one.
    let repeated = ok(router, "CreatePullRequest", &body("one", "token-a")).await;
    assert_eq!(
        pull_request_of(&repeated, "repeated token"),
        pull_request_of(&first, "repeated token"),
        "a repeated token must answer the first pull request"
    );

    // The same token with changed parameters is refused.
    let code = error(
        router,
        "CreatePullRequest",
        &body("two", "token-a"),
        400,
    )
    .await;
    assert_eq!(
        code, "IdempotencyParameterMismatchException",
        "a reused token with changed parameters must be refused, got {code}"
    );

    // A different token opens a second pull request, which gets the next id.
    let second = ok(router, "CreatePullRequest", &body("two", "token-b")).await;
    let second_id = string_at(&second, "/pullRequest/pullRequestId");
    assert_ne!(
        second_id, first_id,
        "a different token must open a second pull request, got {second_id}"
    );
    assert_eq!(
        second_id, "2",
        "the second pull request takes the next id, got {second_id}"
    );
    assert_eq!(
        pull_request_of(&first, "token echo")
            .get("clientRequestToken")
            .and_then(Value::as_str),
        Some("token-a"),
        "the token the request carried is reported back, got {first}"
    );
}

/// One row of the approval-rule-template table.
struct TemplateCase<'a> {
    label: &'a str,
    /// `DestinationReferences` the template document carries, if any.
    destination_references: Option<Vec<&'a str>>,
    /// Is the template's rule expected on a pull request into `main`?
    applies: bool,
}

#[tokio::test]
async fn templates_associated_with_the_repository_are_applied_to_a_new_pull_request() {
    // Rows differ along whether the template scopes itself to destination branches
    // and whether the pull request's destination is one of them. The scoping member
    // is the one `src/state.rs`'s `is_approval_rule_template_content` accepts.
    let cases = vec![
        TemplateCase {
            label: "a template scoped to no destination applies to every pull request",
            destination_references: None,
            applies: true,
        },
        TemplateCase {
            label: "a template listing the destination by short name applies",
            destination_references: Some(vec!["main"]),
            applies: true,
        },
        TemplateCase {
            label: "a template listing the destination fully qualified applies",
            destination_references: Some(vec!["refs/heads/main"]),
            applies: true,
        },
        TemplateCase {
            label: "a template listing only other destinations does not apply",
            destination_references: Some(vec!["refs/heads/release", "staging"]),
            applies: false,
        },
    ];

    for case in cases {
        let label = case.label;
        let built = diverged(REPOSITORY, 1, 1).await;
        let router = &built.router;

        let mut content = json!({
            "Version": TEMPLATE_VERSION,
            "Statements": [{
                "Type": STATEMENT_TYPE,
                "NumberOfApprovalsNeeded": 2,
                "ApprovalPoolMembers": ["arn:aws:sts::*:assumed-role/Reviewers/*"],
            }],
        });
        if let Some(references) = &case.destination_references {
            content["DestinationReferences"] = json!(references);
        }
        let content = content.to_string();
        let template = ok_as(
            router,
            DEFAULT_ACCESS_KEY_ID,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": "two-reviewers",
                "approvalRuleTemplateContent": content,
            }),
        )
        .await;
        let template_id = string_at(
            &template,
            "/approvalRuleTemplate/approvalRuleTemplateId",
        );
        let stored_content = string_at(
            &template,
            "/approvalRuleTemplate/approvalRuleTemplateContent",
        );
        ok(
            router,
            "AssociateApprovalRuleTemplateWithRepository",
            &json!({
                "approvalRuleTemplateName": "two-reviewers",
                "repositoryName": REPOSITORY,
            }),
        )
        .await;

        let created = ok(
            router,
            "CreatePullRequest",
            &create_body(REPOSITORY, "reviewed", None, "feature", Some("main")),
        )
        .await;
        let pull_request = pull_request_of(&created, label);
        let rules = pull_request
            .get("approvalRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !case.applies {
            assert!(
                rules.is_empty(),
                "case {label:?}: expected no approval rule, got {rules:?}"
            );
            continue;
        }
        assert_eq!(
            rules.len(),
            1,
            "case {label:?}: expected exactly one approval rule, got {rules:?}"
        );
        let rule = &rules[0];
        assert_eq!(
            rule.get("approvalRuleName").and_then(Value::as_str),
            Some("two-reviewers"),
            "case {label:?}: a template's rule carries the template's name, got {rule}"
        );
        assert_eq!(
            rule.get("approvalRuleContent").and_then(Value::as_str),
            Some(stored_content.as_str()),
            "case {label:?}: a template's rule carries the template's content, got {rule}"
        );
        assert_eq!(
            rule.pointer("/originApprovalRuleTemplate/approvalRuleTemplateId")
                .and_then(Value::as_str),
            Some(template_id.as_str()),
            "case {label:?}: the rule names the template it came from, got {rule}"
        );
        assert_eq!(
            rule.pointer("/originApprovalRuleTemplate/approvalRuleTemplateName")
                .and_then(Value::as_str),
            Some("two-reviewers"),
            "case {label:?}: the rule names the template it came from, got {rule}"
        );
        assert!(
            rule.get("ruleContentSha256")
                .and_then(Value::as_str)
                .is_some_and(|digest| digest.len() == 64
                    && digest.bytes().all(|byte| byte.is_ascii_hexdigit())),
            "case {label:?}: expected a hex content signature, got {rule}"
        );
        // A rule a template applied is written by the SERVICE, not by the caller
        // who opened the pull request (`src/state.rs`'s `SERVICE_PRINCIPAL`).
        assert_eq!(
            rule.get("lastModifiedUser").and_then(Value::as_str),
            Some("codecommit.amazonaws.com"),
            "case {label:?}: a template-applied rule is attributed to the service, got {rule}"
        );
        assert_ne!(
            rule.get("lastModifiedUser").and_then(Value::as_str),
            Some(caller_arn(DEFAULT_ACCESS_KEY_ID).as_str()),
            "case {label:?}: a template-applied rule is not attributed to the caller, got {rule}"
        );

        // The rule is part of the stored record, so the reader returns it too.
        let id = string_at(&created, "/pullRequest/pullRequestId");
        let read = ok(router, "GetPullRequest", &json!({ "pullRequestId": id })).await;
        assert_eq!(
            pull_request_of(&read, label),
            pull_request,
            "case {label:?}: the reader must answer the stored approval rules"
        );
    }
}

#[tokio::test]
async fn the_author_is_the_calling_principal_and_two_callers_are_told_apart() {
    let built = diverged(REPOSITORY, 1, 1).await;
    let router = &built.router;

    // Rows differ along the principal that signs the call.
    for access_key_id in ["AKIDAUTHORONE", "AKIDAUTHORTWO"] {
        let created = ok_as(
            router,
            access_key_id,
            "CreatePullRequest",
            &create_body(
                REPOSITORY,
                &format!("opened by {access_key_id}"),
                None,
                "feature",
                Some("main"),
            ),
        )
        .await;
        assert_eq!(
            created.pointer("/pullRequest/authorArn").and_then(Value::as_str),
            Some(caller_arn(access_key_id).as_str()),
            "case {access_key_id:?}: expected the signing principal as author, got {created}"
        );
    }

    // A read is a read whoever makes it: the record names its own author, not the
    // caller.
    let read = ok_as(
        router,
        "AKIDREADER",
        "GetPullRequest",
        &json!({ "pullRequestId": "1" }),
    )
    .await;
    assert_eq!(
        read.pointer("/pullRequest/authorArn").and_then(Value::as_str),
        Some(caller_arn("AKIDAUTHORONE").as_str()),
        "a reader must not be reported as the author, got {read}"
    );
}

#[tokio::test]
async fn a_source_branch_with_no_shared_history_reports_no_merge_base() {
    // Two root commits in one repository: histories that share nothing. The control
    // plane cannot build that (every commit it makes is a child of a branch tip),
    // so the state is seeded here as a push of an unrelated history would leave it.
    let (router, engine) = emulator_with_engine();
    let mut repository = Repository::new(
        REPOSITORY,
        None,
        None,
        BTreeMap::new(),
        REGION,
        SystemTime::now(),
    );
    for (branch, path, content) in [
        ("main", "left.txt", &b"left\n"[..]),
        ("feature", "right.txt", &b"right\n"[..]),
    ] {
        let blob_id = repository.insert_blob(content.to_vec());
        let file = FileEntry {
            blob_id,
            file_mode: FILE_MODE_NORMAL.to_string(),
            size: i64::try_from(content.len()).unwrap_or(i64::MAX),
        };
        let identity = UserInfo {
            name: Some("tester".to_string()),
            email: Some("tester@example.com".to_string()),
            date: Some("1700000000 +0000".to_string()),
        };
        let (commit_id, _) = repository
            .commit_snapshot(
                BTreeMap::from([(path.to_string(), file)]),
                Vec::new(),
                identity.clone(),
                identity,
                Some("a seeded root commit"),
            )
            .expect("the snapshot's blobs are stored");
        repository.branches.insert(
            branch.to_string(),
            Branch {
                name: branch.to_string(),
                commit_id,
            },
        );
    }
    repository.default_branch = Some("main".to_string());
    engine
        .state::<CodeCommitState>()
        .lock()
        .repositories
        .insert(REPOSITORY.to_string(), repository);

    let created = ok(
        &router,
        "CreatePullRequest",
        &create_body(REPOSITORY, "unrelated histories", None, "feature", Some("main")),
    )
    .await;
    let target = target_of(
        &pull_request_of(&created, "unrelated histories"),
        "unrelated histories",
    );
    assert!(
        target.get("mergeBase").is_none_or(Value::is_null),
        "two histories sharing no commit have no merge base, got {target}"
    );
    // The pull request itself is still persisted and readable: a missing merge base
    // is an answer, not a refusal.
    let read = ok(&router, "GetPullRequest", &json!({ "pullRequestId": "1" })).await;
    assert_eq!(
        read.pointer("/pullRequest/title").and_then(Value::as_str),
        Some("unrelated histories"),
        "the pull request must be readable, got {read}"
    );
}

#[tokio::test]
async fn a_malformed_member_is_refused_with_the_error_codecommit_names_for_it() {
    let built = diverged(REPOSITORY, 1, 1).await;
    let router = &built.router;

    // The boundary validator refuses a modelled constraint before the handler runs;
    // these rows span the members `CreatePullRequest` and `GetPullRequest` carry
    // whose constraint the model states, and assert the code CodeCommit names for
    // each rather than a generic validation error.
    let rows = [
        (
            "a title past the modelled 150 characters",
            create_body(REPOSITORY, &"t".repeat(151), None, "feature", Some("main")),
            "CreatePullRequest",
            "InvalidTitleException",
        ),
        (
            "a description past the modelled 10240 characters",
            create_body(
                REPOSITORY,
                "t",
                Some(&"d".repeat(10_241)),
                "feature",
                Some("main"),
            ),
            "CreatePullRequest",
            "InvalidDescriptionException",
        ),
        (
            "no title member at all",
            json!({ "targets": [{ "repositoryName": REPOSITORY, "sourceReference": "feature" }] }),
            "CreatePullRequest",
            "TitleRequiredException",
        ),
        (
            "no targets member at all",
            json!({ "title": "t" }),
            "CreatePullRequest",
            "TargetsRequiredException",
        ),
        (
            "a target with no source reference",
            json!({ "title": "t", "targets": [{ "repositoryName": REPOSITORY }] }),
            "CreatePullRequest",
            "ReferenceNameRequiredException",
        ),
        (
            "a target repository name outside the modelled pattern",
            json!({
                "title": "t",
                "targets": [{ "repositoryName": "not a name", "sourceReference": "feature" }],
            }),
            "CreatePullRequest",
            "InvalidRepositoryNameException",
        ),
        (
            "no pull request id at all",
            json!({}),
            "GetPullRequest",
            "PullRequestIdRequiredException",
        ),
    ];

    for (label, body, operation, expected) in rows {
        let code = error_as(router, DEFAULT_ACCESS_KEY_ID, operation, &body, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }
}
