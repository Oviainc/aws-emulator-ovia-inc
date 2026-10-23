// SPDX-License-Identifier: CC0-1.0

//! The message CodeCommit puts on its request-derived client errors, over the wire.
//!
//! The rule under test: each of these error codes carries ONE fixed template, the
//! same text whichever operation raises it — a slot filled from the request (and
//! the caller's account) where the template has one, and a constant where it has
//! none. So the tables below vary exactly what the template generalises over —
//! the repository name (length, digits, dots, hyphens, underscores, the 1- and
//! 100-character bounds of `RepositoryName`), the branch name, and the operation
//! that raises the code — and assert the template holds on every row, never the
//! prose of one request.
//!
//! The name bounds come from the model: `RepositoryName` is `len 1..100, pattern
//! ^[\w\.-]+$` (`smithy_query(query="shape",shape="RepositoryName")`, also quoted
//! in each `src/ops/<op>.rs` "## Input" header), and `BranchName` is `len 1..256`.

use serde_json::{json, Value};

// The emulator and the awsJson1_1 request path are this crate's shared harness in
// `crate::repository_lifecycle` (over the single request builder in `crate::wire`);
// `error_with_message` is the one helper this suite adds on top of them.
use crate::harness::error_with_message;
use crate::repository_lifecycle::{emulator, ok};

/// The HTTP status every client error asserted here models
/// (`smithy_query(query="shape",shape="RepositoryDoesNotExistException")` and
/// friends: `errorKind: client`, `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// A syntactically valid git object id (40 hex characters), so an operation that
/// takes a commit or blob id gets past its own `Invalid…IdException` check and
/// reaches the repository lookup under test. No repository holds it.
const FULL_OBJECT_ID: &str = "0123456789abcdef0123456789abcdef01234567";

/// Repository names spanning what `RepositoryName` allows: the 1-character
/// minimum, the 100-character maximum, and names mixing digits, dots, hyphens and
/// underscores — none of them a name the issue cited.
fn repository_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "x".repeat(100),
        "repo-with-hyphens".to_string(),
        "repo_with_underscores".to_string(),
        "repo.with.dots".to_string(),
        "MiXeD-Case_9.9".to_string(),
        "0123456789".to_string(),
    ]
}

/// Branch names spanning what `BranchName` allows, including a slash-separated
/// ref name and a 256-character maximum-length name.
fn branch_names() -> Vec<String> {
    vec![
        "b".to_string(),
        "feature/deeply/nested-ref".to_string(),
        "release_1.2.3".to_string(),
        "z".repeat(256),
    ]
}

/// A `kmsKeyId` that selects a key at all — one of the four forms KMS documents
/// for `KeyId` (see `crate::state::is_kms_key_selector`), here the alias form — so
/// `UpdateRepositoryEncryptionKey` gets past its own `EncryptionKeyInvalidIdException`
/// check and reaches the repository lookup under test.
const KMS_KEY_SELECTOR: &str = "alias/aws/codecommit";

/// The ARN form the tagging operations address a repository by:
/// `arn:<partition>:codecommit:<region>:<account-id>:<repository-name>`, the form
/// `RepositoryMetadata.arn` reports (see
/// `crate::state::repository_name_of_resource_arn`). The name in the last field is
/// what the message template's slot carries, so these rows prove the slot is the
/// repository the request addressed, not the text it addressed it with.
fn repository_arn(name: &str, account: &str) -> String {
    format!("arn:aws:codecommit:us-east-1:{account}:{name}")
}

/// The account id the template's second slot carries, checked here against the
/// shape AWS gives an account id — exactly 12 decimal digits — rather than taken
/// on trust from the helper the handler also uses.
fn account_id() -> String {
    let account = engine::emulator_account_id().to_string();
    assert!(
        account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit()),
        "the account id in a message template must be 12 decimal digits, got {account:?}"
    );
    account
}

/// Cause 1 — `RepositoryDoesNotExistException` reads `<repositoryName> for
/// <accountId>`, on every operation that raises it and for every name shape.
#[tokio::test]
async fn repository_does_not_exist_message_is_the_name_then_the_account() {
    let router = emulator();
    let account = account_id();

    for name in repository_names() {
        // Each row is a repository the store has never held, so every one of these
        // operations has to answer with the not-found error rather than a lookup.
        let requests: Vec<(&str, Value)> = vec![
            ("GetRepository", json!({"repositoryName": name})),
            (
                "GetFile",
                json!({"repositoryName": name, "filePath": "README.md"}),
            ),
            (
                "GetBranch",
                json!({"repositoryName": name, "branchName": "main"}),
            ),
            (
                "PutFile",
                json!({
                    "repositoryName": name,
                    "branchName": "main",
                    "filePath": "README.md",
                    "fileContent": "aGk=",
                }),
            ),
            // UpdateRepositoryName names the repository in `oldName`, not
            // `repositoryName`: the template's slot is whichever member the request
            // used to address the repository.
            (
                "UpdateRepositoryName",
                json!({"oldName": name, "newName": "some-other-name"}),
            ),
            (
                "GetCommit",
                json!({"repositoryName": name, "commitId": FULL_OBJECT_ID}),
            ),
            (
                "GetBlob",
                json!({"repositoryName": name, "blobId": FULL_OBJECT_ID}),
            ),
            (
                "GetCommentsForComparedCommit",
                json!({"repositoryName": name, "afterCommitId": FULL_OBJECT_ID}),
            ),
            (
                "PostCommentForComparedCommit",
                json!({
                    "repositoryName": name,
                    "afterCommitId": FULL_OBJECT_ID,
                    "content": "a comment",
                }),
            ),
            (
                "ListFileCommitHistory",
                json!({"repositoryName": name, "filePath": "README.md"}),
            ),
            (
                "UpdateDefaultBranch",
                json!({"repositoryName": name, "defaultBranchName": "main"}),
            ),
            (
                "GetFolder",
                json!({"repositoryName": name, "folderPath": "/"}),
            ),
            (
                "CreateBranch",
                json!({
                    "repositoryName": name,
                    "branchName": "new-branch",
                    "commitId": FULL_OBJECT_ID,
                }),
            ),
            (
                "UpdateRepositoryDescription",
                json!({"repositoryName": name, "repositoryDescription": "a description"}),
            ),
            (
                "GetDifferences",
                json!({"repositoryName": name, "afterCommitSpecifier": FULL_OBJECT_ID}),
            ),
            (
                "CreateCommit",
                json!({
                    "repositoryName": name,
                    "branchName": "main",
                    "putFiles": [{"filePath": "a.txt", "fileContent": "eA=="}],
                }),
            ),
            (
                "ListAssociatedApprovalRuleTemplatesForRepository",
                json!({"repositoryName": name}),
            ),
            // The tagging operations address the repository by ARN, so the slot is
            // the name inside it rather than a `repositoryName` member.
            (
                "ListTagsForResource",
                json!({"resourceArn": repository_arn(&name, &account)}),
            ),
            (
                "TagResource",
                json!({
                    "resourceArn": repository_arn(&name, &account),
                    "tags": {"owner": "team"},
                }),
            ),
            (
                "UntagResource",
                json!({
                    "resourceArn": repository_arn(&name, &account),
                    "tagKeys": ["owner"],
                }),
            ),
            (
                "UpdateRepositoryEncryptionKey",
                json!({"repositoryName": name, "kmsKeyId": KMS_KEY_SELECTOR}),
            ),
        ];
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "RepositoryDoesNotExistException",
                "case ({operation}, {name:?}): expected code RepositoryDoesNotExistException, got {code:?}"
            );
            let expected = format!("{name} for {account}");
            assert_eq!(
                message, expected,
                "case ({operation}, {name:?}): expected message {expected:?}, got {message:?}"
            );
        }
    }
}

/// Cause 2 — `RepositoryNameExistsException` reads `Repository named <name>
/// already exists`, for every name shape, and the first create still succeeds.
#[tokio::test]
async fn repository_name_exists_message_names_the_requested_repository() {
    let router = emulator();

    for name in repository_names() {
        // The first create carries a description, so the non-mutation assertion
        // below compares a stored value against itself rather than one absent
        // field against another.
        let first_description = format!("the repository {name} as first created");
        let created = ok(
            &router,
            "CreateRepository",
            &json!({"repositoryName": name, "repositoryDescription": first_description}),
        )
        .await;
        assert_eq!(
            created["repositoryMetadata"]["repositoryName"],
            json!(name.clone()),
            "case {name:?}: the first CreateRepository must store the repository, got {created}"
        );

        let (code, message) = error_with_message(
            &router,
            "CreateRepository",
            // A second create of the same name, with a different description, so the
            // rejection cannot be mistaken for byte-identical-request idempotence.
            &json!({"repositoryName": name, "repositoryDescription": "second attempt"}),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "RepositoryNameExistsException",
            "case {name:?}: expected code RepositoryNameExistsException, got {code:?}"
        );
        let expected = format!("Repository named {name} already exists");
        assert_eq!(
            message, expected,
            "case {name:?}: expected message {expected:?}, got {message:?}"
        );

        // The refusal left the stored repository as the first create made it: the
        // description the rejected request carried was NOT written.
        let read_back = ok(&router, "GetRepository", &json!({"repositoryName": name})).await;
        assert_eq!(
            read_back["repositoryMetadata"]["repositoryDescription"],
            json!(first_description.clone()),
            "case {name:?}: the rejected create must not touch the stored repository \
             (expected the description {first_description:?}), got {read_back}"
        );

        // The same code, raised by the other operation that takes a name the
        // account must not already hold, carries the same template with the name
        // the request asked FOR in the slot.
        let (rename_code, rename_message) = error_with_message(
            &router,
            "UpdateRepositoryName",
            &json!({"oldName": name, "newName": name}),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            rename_code, "RepositoryNameExistsException",
            "case {name:?}: expected code RepositoryNameExistsException from \
             UpdateRepositoryName, got {rename_code:?}"
        );
        assert_eq!(
            rename_message, expected,
            "case {name:?}: UpdateRepositoryName must carry the same template as \
             CreateRepository ({expected:?}), got {rename_message:?}"
        );
    }
}

/// Cause 3 — `BranchDoesNotExistException` reads the same constant whatever branch
/// and repository the request named, and whichever operation raises it.
#[tokio::test]
async fn branch_does_not_exist_message_is_a_constant() {
    let router = emulator();
    // The constant AWS returns: it names neither the branch nor the repository.
    const EXPECTED: &str = "The specified branch does not exist";

    for (index, repository) in repository_names().into_iter().enumerate() {
        ok(
            &router,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        // Give the repository a branch, so a write to another branch is refused
        // instead of creating it (PutFile creates the first branch of an empty
        // repository).
        let seeded = ok(
            &router,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": "main",
                "filePath": "seed.txt",
                "fileContent": "c2VlZA==",
            }),
        )
        .await;
        assert!(
            seeded["commitId"].is_string(),
            "case {repository:?}: seeding the repository must commit, got {seeded}"
        );

        // One branch name per repository, cycling the table so every row is used.
        let branches = branch_names();
        let ghost = branches[index % branches.len()].clone();
        let requests: Vec<(&str, Value)> = vec![
            (
                "GetBranch",
                json!({"repositoryName": repository, "branchName": ghost}),
            ),
            (
                "PutFile",
                json!({
                    "repositoryName": repository,
                    "branchName": ghost,
                    "filePath": "f.txt",
                    "fileContent": "eA==",
                }),
            ),
            (
                "UpdateDefaultBranch",
                json!({"repositoryName": repository, "defaultBranchName": ghost}),
            ),
            (
                "CreateCommit",
                json!({
                    "repositoryName": repository,
                    "branchName": ghost,
                    "putFiles": [{"filePath": "f.txt", "fileContent": "eA=="}],
                }),
            ),
        ];
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "BranchDoesNotExistException",
                "case ({operation}, {repository:?}, {ghost:?}): expected code \
                 BranchDoesNotExistException, got {code:?}"
            );
            assert_eq!(
                message, EXPECTED,
                "case ({operation}, {repository:?}, {ghost:?}): expected message \
                 {EXPECTED:?}, got {message:?}"
            );
        }
    }
}

/// One error code is worded one way: the message a given code carries does not
/// depend on which operation raised it, and it is literally the requested name
/// followed by ` for ` and a 12-digit account — asserted here by taking the
/// template apart rather than by rebuilding it.
#[tokio::test]
async fn one_code_is_worded_the_same_by_every_operation() {
    let router = emulator();
    let missing = "never-created-repo";
    let account = account_id();
    let arn = repository_arn(missing, &account);

    let mut messages: Vec<(&str, String)> = Vec::new();
    for (operation, body) in [
        ("GetRepository", json!({"repositoryName": missing})),
        (
            "GetFile",
            json!({"repositoryName": missing, "filePath": "a.txt"}),
        ),
        (
            "GetBranch",
            json!({"repositoryName": missing, "branchName": "main"}),
        ),
        (
            "PutFile",
            json!({
                "repositoryName": missing,
                "branchName": "main",
                "filePath": "a.txt",
                "fileContent": "eA==",
            }),
        ),
        (
            "UpdateRepositoryName",
            json!({"oldName": missing, "newName": "another-name"}),
        ),
        (
            "GetCommit",
            json!({"repositoryName": missing, "commitId": FULL_OBJECT_ID}),
        ),
        (
            "GetBlob",
            json!({"repositoryName": missing, "blobId": FULL_OBJECT_ID}),
        ),
        (
            "GetCommentsForComparedCommit",
            json!({"repositoryName": missing, "afterCommitId": FULL_OBJECT_ID}),
        ),
        (
            "PostCommentForComparedCommit",
            json!({
                "repositoryName": missing,
                "afterCommitId": FULL_OBJECT_ID,
                "content": "a comment",
            }),
        ),
        (
            "ListFileCommitHistory",
            json!({"repositoryName": missing, "filePath": "a.txt"}),
        ),
        (
            "UpdateDefaultBranch",
            json!({"repositoryName": missing, "defaultBranchName": "main"}),
        ),
        (
            "GetFolder",
            json!({"repositoryName": missing, "folderPath": "/"}),
        ),
        (
            "CreateBranch",
            json!({
                "repositoryName": missing,
                "branchName": "new-branch",
                "commitId": FULL_OBJECT_ID,
            }),
        ),
        (
            "UpdateRepositoryDescription",
            json!({"repositoryName": missing, "repositoryDescription": "a description"}),
        ),
        (
            "GetDifferences",
            json!({"repositoryName": missing, "afterCommitSpecifier": FULL_OBJECT_ID}),
        ),
        (
            "CreateCommit",
            json!({
                "repositoryName": missing,
                "branchName": "main",
                "putFiles": [{"filePath": "a.txt", "fileContent": "eA=="}],
            }),
        ),
        (
            "ListAssociatedApprovalRuleTemplatesForRepository",
            json!({"repositoryName": missing}),
        ),
        ("ListTagsForResource", json!({"resourceArn": arn})),
        (
            "TagResource",
            json!({"resourceArn": arn, "tags": {"owner": "team"}}),
        ),
        (
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": ["owner"]}),
        ),
        (
            "UpdateRepositoryEncryptionKey",
            json!({"repositoryName": missing, "kmsKeyId": KMS_KEY_SELECTOR}),
        ),
    ] {
        let (code, message) =
            error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {operation}: expected code RepositoryDoesNotExistException, got {code:?}"
        );
        let (name, account) = message.split_once(" for ").unwrap_or_else(|| {
            panic!("case {operation}: message must read \"<name> for <account>\", got {message:?}")
        });
        assert_eq!(
            name, missing,
            "case {operation}: the first slot must be the requested repository name, got {name:?}"
        );
        assert!(
            account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit()),
            "case {operation}: the second slot must be a 12-digit account id, got {account:?}"
        );
        messages.push((operation, message));
    }
    let first = messages[0].1.clone();
    for (operation, message) in &messages {
        assert_eq!(
            *message, first,
            "case {operation}: RepositoryDoesNotExistException must carry the same \
             message as every other operation ({first:?}), got {message:?}"
        );
    }
}
