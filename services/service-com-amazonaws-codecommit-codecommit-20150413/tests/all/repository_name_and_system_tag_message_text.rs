// SPDX-License-Identifier: CC0-1.0

//! The TEXT CodeCommit puts on two client errors, over the wire: the one it
//! answers a repository name that is not one with, and the one it answers a tag
//! key reserved to AWS itself with.
//!
//! The rule under test: each of these codes carries ONE message — the service's
//! own fixed text for the class, the same whichever operation raises it and
//! whatever the request said — and nothing the emulator's own layers know is added
//! to it or substituted for it: not the member a validator rejected, not the name
//! or the ARN the caller sent, and not the key it asked to set.
//!
//! * `InvalidRepositoryNameException` states the naming rule IN FULL: the
//!   characters a name may combine, the length bounds and that names are case
//!   sensitive. Those bounds are the model's — `RepositoryName` is `len 1..100,
//!   pattern ^[\w\.-]+$` (`smithy_query(query="shape",shape="RepositoryName")`,
//!   also quoted in each `src/ops/<op>.rs` "## Input" header) — so the tables
//!   below span the ways a name falls outside it (a space, punctuation the pattern
//!   excludes, one character and far past the maximum length, a letter outside the
//!   ASCII set an ECMA-262 `\w` stands for, and the prohibited `.git` suffix),
//!   across the operations that take a name as a member, inside a pull request
//!   target, and inside a resource ARN. A second test pins that the rule the
//!   message STATES is the one the service applies: both published length bounds
//!   are served, a name combining every character class it names is served, and
//!   two names differing only in case are two repositories.
//! * `InvalidSystemTagUsageException` states that the CALLER may not mutate the
//!   tags AWS reserves to itself — the key is well formed, and the refusal is about
//!   who is asking — so it names neither the key nor the resource. The tables vary
//!   the key's length and shape, its position in the request, how many keys the
//!   request carries, and the operation, and a control row pins that the rule is
//!   about the reserved PREFIX rather than about the text appearing anywhere in a
//!   key.
//!
//! Sibling suites own the neighbouring rules: `client_error_messages.rs` pins the
//! repository-scoped codes' templates and `aws_error_message_text.rs` the
//! template-, document- and ARN-scoped ones.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{emulator, ok, REGION};
use crate::wire::fresh_client_request_token;

/// The HTTP status both codes model (`errorKind: client`, `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// `InvalidRepositoryNameException`'s message: the whole naming rule, as the
/// service words it.
const INVALID_REPOSITORY_NAME: &str =
    "The repository name is not valid. Repository names can be any valid combination of letters, \
     numbers, periods, underscores, and dashes between 1 and 100 characters in length. Names are \
     case sensitive. For more information, see Limits in the AWS CodeCommit User Guide.";

/// `InvalidSystemTagUsageException`'s message: the caller is not the service, so
/// it may not mutate the tags the service reserves to itself.
const SYSTEM_TAG_MUTATION: &str = "Caller is an end user and not allowed to mutate system tags";

/// `RepositoryName`'s published length bounds
/// (`smithy_query(query="shape",shape="RepositoryName")`).
const NAME_MIN_LENGTH: usize = 1;
const NAME_MAX_LENGTH: usize = 100;

/// A string of `length` 'a's — a name that satisfies the pattern and reaches a
/// length bound exactly.
fn filler(length: usize) -> String {
    "a".repeat(length)
}

/// The ARN form the tagging operations address a repository by:
/// `arn:aws:codecommit:<region>:<account-id>:<repository-name>`.
fn repository_arn(name: &str) -> String {
    format!(
        "arn:aws:codecommit:{REGION}:{}:{name}",
        engine::emulator_account_id()
    )
}

/// Repository names the MODEL itself refuses — outside `RepositoryName`'s
/// `pattern` or past its `length` — so the request boundary refuses them for every
/// operation that carries a name.
fn names_the_model_refuses() -> Vec<(&'static str, String)> {
    vec![
        ("a name holding a space", "not a name".to_string()),
        ("a name holding a slash", "team/repo".to_string()),
        ("a name holding a colon", "team:repo".to_string()),
        ("a name holding an asterisk", "repo*".to_string()),
        (
            "a name one character past the published maximum",
            filler(NAME_MAX_LENGTH + 1),
        ),
        (
            "a name far past the published maximum",
            filler(NAME_MAX_LENGTH * 3),
        ),
    ]
}

/// Repository names the model's `pattern` does not rule out but the naming rule
/// does: the `.git` suffix CodeCommit prohibits, and a letter outside the ASCII
/// set an ECMA-262 `\w` stands for. They are refused by the operations that apply
/// the whole rule to a name they are about to USE — the two that write a name, the
/// one that names a pull request's repository, and the three that address a
/// repository by ARN.
fn names_the_rule_refuses() -> Vec<(&'static str, String)> {
    vec![
        ("the prohibited .git suffix", "archive.git".to_string()),
        (
            "the prohibited .git suffix in another case",
            "archive.GIT".to_string(),
        ),
        ("a name holding a non-ASCII letter", "répo".to_string()),
        (
            "a name of non-ASCII letters only",
            "хранилище".to_string(),
        ),
    ]
}

/// The operations that take the name as a `repositoryName` member, where the
/// request boundary answers the model's own `length` and `pattern`.
fn member_requests_naming(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetRepository", json!({ "repositoryName": name })),
        ("DeleteRepository", json!({ "repositoryName": name })),
        ("ListBranches", json!({ "repositoryName": name })),
        ("ListPullRequests", json!({ "repositoryName": name })),
        (
            "UpdateRepositoryDescription",
            json!({ "repositoryName": name, "repositoryDescription": "d" }),
        ),
    ]
}

/// The operations that apply the whole naming rule themselves, whether the name
/// arrives as a member, inside a pull request target, or inside a resource ARN.
fn rule_requests_naming(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("CreateRepository", json!({ "repositoryName": name })),
        (
            "UpdateRepositoryName",
            json!({ "oldName": name, "newName": "a-valid-name" }),
        ),
        (
            "CreatePullRequest",
            json!({
                "title": "a title",
                "clientRequestToken": fresh_client_request_token(),
                "targets": [{ "repositoryName": name, "sourceReference": "feature" }],
            }),
        ),
        (
            "TagResource",
            json!({ "resourceArn": repository_arn(name), "tags": { "owner": "team" } }),
        ),
        (
            "ListTagsForResource",
            json!({ "resourceArn": repository_arn(name) }),
        ),
        (
            "UntagResource",
            json!({ "resourceArn": repository_arn(name), "tagKeys": ["owner"] }),
        ),
    ]
}

/// One row of the naming table: why the name is not one, the name, and the
/// requests that must all answer the same refusal for it.
struct NameCase {
    label: &'static str,
    name: String,
    requests: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn invalid_repository_name_message_states_the_whole_naming_rule() {
    let router = emulator();

    let mut cases: Vec<NameCase> = Vec::new();
    for (label, name) in names_the_model_refuses() {
        let mut requests = member_requests_naming(&name);
        requests.extend(rule_requests_naming(&name));
        cases.push(NameCase {
            label,
            name,
            requests,
        });
    }
    for (label, name) in names_the_rule_refuses() {
        let requests = rule_requests_naming(&name);
        cases.push(NameCase {
            label,
            name,
            requests,
        });
    }

    for NameCase {
        label,
        name,
        requests,
    } in cases
    {
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidRepositoryNameException",
                "case ({operation}, {label}): expected code InvalidRepositoryNameException, \
                 got {code:?}"
            );
            assert_eq!(
                message, INVALID_REPOSITORY_NAME,
                "case ({operation}, {label}): expected message {INVALID_REPOSITORY_NAME:?}, \
                 got {message:?}"
            );
            // The rejected value is the caller's; the message is the service's, so
            // nothing of the request appears in it.
            assert!(
                !message.contains(&name) && !message.contains("resourceArn"),
                "case ({operation}, {label}): the message must carry no value from the \
                 request, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_naming_rule_the_message_states_is_the_one_the_service_applies() {
    let router = emulator();

    // The message says 1 to 100 characters: both published bounds are served, so
    // the prose states the rule the service actually applies rather than a wider
    // or narrower one.
    for length in [NAME_MIN_LENGTH, NAME_MAX_LENGTH] {
        let name = filler(length);
        let created = ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            created["repositoryMetadata"]["repositoryName"],
            json!(name.clone()),
            "case a {length}-character name: the published bound must be served, got {created}"
        );
        // And read back through the implemented reader, so the name was stored
        // rather than echoed.
        let read_back = ok(
            &router,
            "GetRepository",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            read_back["repositoryMetadata"]["repositoryName"],
            json!(name.clone()),
            "case a {length}-character name: GetRepository must report the stored name, \
             got {read_back}"
        );
    }

    // The message says letters, numbers, periods, underscores and dashes: a name
    // combining all of them is served.
    for name in ["MiXeD-Case_9.9", "0123456789", "repo.with.dots"] {
        let created = ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            created["repositoryMetadata"]["repositoryName"],
            json!(name),
            "case {name:?}: a name the stated rule allows must be served, got {created}"
        );
    }

    // The message says names are case sensitive: two names differing only in case
    // are two repositories, so the second create is not a duplicate.
    for name in ["Case-Sensitive", "case-sensitive"] {
        let created = ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            created["repositoryMetadata"]["repositoryName"],
            json!(name),
            "case {name:?}: names differing in case must be distinct repositories, got {created}"
        );
    }
}

/// A `TagResource` body carrying exactly these pairs, for a key a JSON literal
/// cannot spell.
fn tag_body(resource_arn: &str, pairs: &[(String, &str)]) -> Value {
    let tags: serde_json::Map<String, Value> = pairs
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect();
    json!({ "resourceArn": resource_arn, "tags": Value::Object(tags) })
}

/// One request that would set or remove a tag key AWS reserves to itself: its
/// label, the operation, and the body.
fn system_tag_requests(resource_arn: &str) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "the bare reserved prefix as the whole key",
            "TagResource",
            json!({ "resourceArn": resource_arn, "tags": { "aws:": "v" } }),
        ),
        (
            "a reserved key with one segment",
            "TagResource",
            json!({ "resourceArn": resource_arn, "tags": { "aws:created-by": "me" } }),
        ),
        (
            "a reserved key with several segments",
            "TagResource",
            json!({
                "resourceArn": resource_arn,
                "tags": { "aws:cloudformation:stack-name": "a-stack" },
            }),
        ),
        (
            "a reserved key at the published key-length maximum",
            "TagResource",
            tag_body(resource_arn, &[(format!("aws:{}", "k".repeat(124)), "v")]),
        ),
        (
            "a reserved key sorting after an acceptable one",
            "TagResource",
            json!({
                "resourceArn": resource_arn,
                "tags": { "team": "a", "aws:internal": "no" },
            }),
        ),
        (
            "a reserved key among several acceptable ones",
            "TagResource",
            json!({
                "resourceArn": resource_arn,
                "tags": { "a": "1", "b": "2", "zzz:aws": "3", "aws:z": "no" },
            }),
        ),
        (
            "a reserved key to remove",
            "UntagResource",
            json!({ "resourceArn": resource_arn, "tagKeys": ["aws:created-by"] }),
        ),
        (
            "a reserved key among the keys to remove",
            "UntagResource",
            json!({ "resourceArn": resource_arn, "tagKeys": ["team", "aws:internal"] }),
        ),
        (
            "the bare reserved prefix among the keys to remove",
            "UntagResource",
            json!({ "resourceArn": resource_arn, "tagKeys": ["aws:"] }),
        ),
        (
            "a reserved key on the create that mints the resource",
            "CreateRepository",
            json!({
                "repositoryName": "system-tagged-at-create",
                "tags": { "aws:created-by": "me" },
            }),
        ),
        (
            "several reserved keys on the create that mints the resource",
            "CreateRepository",
            json!({
                "repositoryName": "system-tagged-at-create",
                "tags": { "aws:a": "1", "aws:b": "2" },
            }),
        ),
    ]
}

/// The tags `ListTagsForResource` reports, as a JSON object (an absent map reads
/// as an empty one).
async fn listed_tags(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let payload = ok(
        router,
        "ListTagsForResource",
        &json!({ "resourceArn": resource_arn }),
    )
    .await;
    payload
        .get("tags")
        .cloned()
        .unwrap_or_else(|| json!({}))
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_as_a_mutation_an_end_user_may_not_make() {
    let router = emulator();
    let repository = "reserved-tag-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository, "tags": { "team": "platform" } }),
    )
    .await;
    let arn = repository_arn(repository);
    let before = listed_tags(&router, &arn).await;

    for (label, operation, body) in system_tag_requests(&arn) {
        let (code, message) =
            error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "InvalidSystemTagUsageException",
            "case ({operation}, {label}): expected code InvalidSystemTagUsageException, \
             got {code:?}"
        );
        assert_eq!(
            message, SYSTEM_TAG_MUTATION,
            "case ({operation}, {label}): expected message {SYSTEM_TAG_MUTATION:?}, \
             got {message:?}"
        );
        // The refusal is about the caller, not about the key's form: it names
        // neither the key the request carried nor the resource it addressed.
        assert!(
            !message.contains("aws:") && !message.contains(repository),
            "case ({operation}, {label}): the message must name neither the key nor the \
             resource, got {message:?}"
        );
    }

    // Nothing any refused request asked for was written.
    let after = listed_tags(&router, &arn).await;
    assert_eq!(
        after, before,
        "a refused reserved-tag request must leave the stored tags as they were \
         (expected {before}), got {after}"
    );
    let (code, _) = error_with_message(
        &router,
        "GetRepository",
        &json!({ "repositoryName": "system-tagged-at-create" }),
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "RepositoryDoesNotExistException",
        "a refused create must mint no repository, got {code:?}"
    );
}

#[tokio::test]
async fn only_the_reserved_prefix_is_refused_and_other_keys_are_stored() {
    let router = emulator();
    let repository = "prefix-control-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let arn = repository_arn(repository);

    // The rule is about the reserved PREFIX: a key that merely carries the text
    // elsewhere, or differs from it, is an ordinary key and is stored.
    let acceptable = ["my-aws:thing", "team:aws:owner", "aws", "awsome", "AWS:x"];
    for key in acceptable {
        ok(
            &router,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": { key: "v" } }),
        )
        .await;
        let tags = listed_tags(&router, &arn).await;
        assert_eq!(
            tags[key],
            json!("v"),
            "case {key:?}: a key outside the reserved prefix must be stored, got {tags}"
        );
    }
}
