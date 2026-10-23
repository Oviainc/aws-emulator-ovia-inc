// SPDX-License-Identifier: CC0-1.0

//! The ECR repository-policy triple — `SetRepositoryPolicy`, `GetRepositoryPolicy` and
//! `DeleteRepositoryPolicy` — end to end over the wire, through the shared
//! [`crate::wire`] harness.
//!
//! The rule under test: a repository holds at most one policy document, which is
//! whatever the last `SetRepositoryPolicy` stored on that repository, which the read
//! reports and the delete reports and removes; a repository with no policy is
//! reported with `RepositoryPolicyNotFoundException`, one that does not exist with
//! `RepositoryNotFoundException`, and a `policyText` that is not a resource-based IAM
//! policy document — or one that would deny every principal the right to set another
//! policy, absent `force` — with `InvalidParameterException`.
//!
//! The document the three operations report is the submitted one in the canonical
//! IAM-policy rendering, so what is compared here is the policy's CONTENT
//! ([`crate::policy_document_canonical_form::document_content`]); the rendering itself is
//! pinned byte for byte in `policy_document_canonical_form.rs`.
//!
//! The policy grammar is the crate's shared `state::validate_resource_policy`, the one
//! `PutRegistryPolicy` uses, so the rows below mirror `registry_policy.rs`'s — minus the
//! wildcard-principal rule, which is a REGISTRY policy's alone: a repository policy
//! grants to `*` when the repository is shared publicly.

use serde_json::{json, Value};

use crate::policy_document_canonical_form::document_content;
use crate::wire::{call, emulator, ok, refused, REGISTRY_ID};

/// A syntactically valid registry id (`RegistryId` is `^[0-9]{12}$`) that is NOT this
/// emulator's registry.
const OTHER_REGISTRY_ID: &str = "123456789012";

/// `RepositoryPolicyText`'s published maximum length, from the model
/// (`smithy_query(query="shape", shape="RepositoryPolicyText")` → `length 0..10240`).
const MAX_POLICY_TEXT_LEN: usize = 10_240;

/// The three operations that reach a repository's policy, by wire name.
const POLICY_OPERATIONS: [&str; 3] = [
    "SetRepositoryPolicy",
    "GetRepositoryPolicy",
    "DeleteRepositoryPolicy",
];

/// Create `name` in this emulator's registry, so the policy operations have a
/// repository to work on.
async fn create_repository(router: &smithy_server_runtime::MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
}

/// `SetRepositoryPolicy`'s request body.
fn set_request(name: &str, policy_text: &str) -> Value {
    json!({ "repositoryName": name, "policyText": policy_text })
}

/// A request naming only the repository: what `Get`/`DeleteRepositoryPolicy` take.
fn name_request(name: &str) -> Value {
    json!({ "repositoryName": name })
}

/// Assert the three identity members of a policy response: the registry, the repository
/// and the policy document, whose content must be the stored policy's.
fn assert_policy_response(case: &str, operation: &str, body: &Value, name: &str, policy: &str) {
    assert_eq!(
        body["registryId"].as_str(),
        Some(REGISTRY_ID),
        "case {case:?}: {operation} must report this registry, got {:?}",
        body["registryId"]
    );
    assert_eq!(
        body["repositoryName"].as_str(),
        Some(name),
        "case {case:?}: {operation} must report the repository it acted on, got {:?}",
        body["repositoryName"]
    );
    let reported = body["policyText"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} must report a policyText"));
    assert_eq!(
        document_content(reported),
        document_content(policy),
        "case {case:?}: {operation} must report the stored policy document, got {reported:?}"
    );
}

/// The forms a repository policy document can take, as a table: the rule generalises
/// over the document's shape (one statement or many, object or list), its `Version`
/// (either published value, or none), its principal and action forms, its whitespace and
/// its length — and the stored policy is the caller's own content in every row.
fn policy_forms() -> Vec<(&'static str, String)> {
    vec![
        (
            "statement as a bare object, granting to every principal",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": { "AWS": "*" },
                    "Action": "ecr:BatchGetImage"
                }
            })
            .to_string(),
        ),
        (
            "single-entry statement list naming one principal and two actions",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowPull",
                    "Effect": "Allow",
                    "Principal": { "AWS": format!("arn:aws:iam::{REGISTRY_ID}:root") },
                    "Action": ["ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer"]
                }]
            })
            .to_string(),
        ),
        (
            "two statements, one of them a deny scoped to one principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "AllowPull",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "ecr:*"
                    },
                    {
                        "Sid": "DenyOnePrincipal",
                        "Effect": "Deny",
                        "Principal": { "AWS": format!("arn:aws:iam::{OTHER_REGISTRY_ID}:root") },
                        "Action": "ecr:SetRepositoryPolicy"
                    }
                ]
            })
            .to_string(),
        ),
        (
            // The legacy version the IAM policy reference documents beside 2012-10-17.
            "the legacy policy version, with a condition",
            json!({
                "Version": "2008-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "*" },
                    "Action": "ecr:BatchGetImage",
                    "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-abc123" } }
                }]
            })
            .to_string(),
        ),
        (
            "no Version element, and a service principal",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "ecr:BatchGetImage"
                }]
            })
            .to_string(),
        ),
        (
            "NotAction in place of Action",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "AWS": format!("arn:aws:iam::{OTHER_REGISTRY_ID}:root") },
                    "NotAction": ["ecr:BatchGetImage"]
                }]
            })
            .to_string(),
        ),
        (
            "pretty-printed text with newlines and tabs",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \
             \"Allow\",\n\t\t\t\"Principal\": \"*\",\n\t\t\t\"Action\": \
             \"ecr:DescribeImages\"\n\t\t}\n\t]\n}"
                .to_string(),
        ),
        (
            "a document at the published maximum length",
            policy_of_length(MAX_POLICY_TEXT_LEN),
        ),
        (
            "a document one character inside the maximum",
            policy_of_length(MAX_POLICY_TEXT_LEN - 1),
        ),
    ]
}

/// A valid policy document of exactly `len` characters: the padding goes in the `Sid`,
/// so the document stays a policy at any length at or above the skeleton's.
fn policy_of_length(len: usize) -> String {
    let document = |sid: &str| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": sid,
                "Effect": "Allow",
                "Principal": { "AWS": "*" },
                "Action": "ecr:BatchGetImage"
            }]
        })
        .to_string()
    };
    let skeleton = document("").len();
    assert!(
        len >= skeleton,
        "a policy document cannot be shorter than its {skeleton}-character skeleton"
    );
    let padded = document(&"a".repeat(len - skeleton));
    assert_eq!(
        padded.len(),
        len,
        "the padded policy document must be exactly {len} characters"
    );
    padded
}

/// The documents a set must refuse. Each row is a way the text fails the resource-policy
/// grammar: it is not JSON, not an object, carries no statements, or carries a statement
/// missing or misusing one of the elements the grammar requires.
fn not_policy_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("empty text", ""),
        ("plain words", "not a policy at all"),
        ("truncated object", r#"{"Statement": "#),
        ("a JSON list", "[]"),
        ("a JSON string", r#""Statement""#),
        ("a JSON number", "42"),
        ("an object with no Statement", r#"{"Version":"2012-10-17"}"#),
        ("Statement as a string", r#"{"Statement":"Allow"}"#),
        ("Statement as an empty list", r#"{"Statement":[]}"#),
        ("Statement as a list of numbers", r#"{"Statement":[1,2]}"#),
        (
            "a Version the policy language does not define",
            r#"{"Version":"2019-01-01","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr:BatchGetImage"}]}"#,
        ),
        (
            "a statement with no Effect",
            r#"{"Statement":[{"Principal":"*","Action":"ecr:BatchGetImage"}]}"#,
        ),
        (
            "an Effect that is neither Allow nor Deny",
            r#"{"Statement":[{"Effect":"Permit","Principal":"*","Action":"ecr:BatchGetImage"}]}"#,
        ),
        (
            "an Effect in the wrong case",
            r#"{"Statement":[{"Effect":"deny","Principal":"*","Action":"ecr:SetRepositoryPolicy"}]}"#,
        ),
        (
            "a statement naming no action",
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Resource":"*"}]}"#,
        ),
        (
            "an empty Action list",
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":[]}]}"#,
        ),
        (
            "a statement naming no principal",
            r#"{"Statement":[{"Effect":"Allow","Action":"ecr:BatchGetImage"}]}"#,
        ),
        (
            "an empty Principal",
            r#"{"Statement":[{"Effect":"Allow","Principal":{},"Action":"ecr:BatchGetImage"}]}"#,
        ),
    ]
}

#[tokio::test]
async fn a_policy_set_on_a_repository_is_read_back_with_its_content_intact() {
    let router = emulator();
    // A second repository, never given a policy, shows the policy is held per
    // repository rather than per registry.
    create_repository(&router, "untouched", "without-policy").await;

    for (index, (case, policy)) in policy_forms().into_iter().enumerate() {
        let name = format!("policy-form-{index}");
        create_repository(&router, case, &name).await;

        let set = ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &set_request(&name, &policy),
        )
        .await;
        assert_policy_response(case, "SetRepositoryPolicy", &set, &name, &policy);

        // Two reads: the document comes back the same, and reading it does not consume
        // it — the second read sees exactly what the first did.
        for read in 1..=2 {
            let case = format!("{case} (read {read})");
            let got = ok(&router, &case, "GetRepositoryPolicy", &name_request(&name)).await;
            assert_policy_response(&case, "GetRepositoryPolicy", &got, &name, &policy);
            assert_eq!(
                got["policyText"], set["policyText"],
                "case {case:?}: the read reports the text the set did, got {:?}",
                got["policyText"]
            );
        }

        // And the neighbouring repository still carries nothing.
        refused(
            &router,
            case,
            "GetRepositoryPolicy",
            &name_request("without-policy"),
            "RepositoryPolicyNotFoundException",
        )
        .await;
    }
}

#[tokio::test]
async fn the_last_policy_set_on_a_repository_is_the_one_it_holds() {
    let router = emulator();
    let name = "policy-overwritten";
    create_repository(&router, "overwrite", name).await;

    // Every form in the table, applied in turn to ONE repository: after each set the
    // repository holds exactly that document — a set replaces, never accumulates.
    for (case, policy) in policy_forms() {
        ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &set_request(name, &policy),
        )
        .await;
        let got = ok(&router, case, "GetRepositoryPolicy", &name_request(name)).await;
        assert_policy_response(case, "GetRepositoryPolicy", &got, name, &policy);
    }
}

#[tokio::test]
async fn deleting_a_policy_reports_it_and_leaves_the_repository_without_one() {
    let router = emulator();

    for (index, (case, policy)) in policy_forms().into_iter().enumerate() {
        let name = format!("policy-deleted-{index}");
        create_repository(&router, case, &name).await;

        // Before any set, both readers report the repository carries no policy.
        for operation in ["GetRepositoryPolicy", "DeleteRepositoryPolicy"] {
            refused(
                &router,
                case,
                operation,
                &name_request(&name),
                "RepositoryPolicyNotFoundException",
            )
            .await;
        }

        ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &set_request(&name, &policy),
        )
        .await;

        let deleted = ok(
            &router,
            case,
            "DeleteRepositoryPolicy",
            &name_request(&name),
        )
        .await;
        assert_policy_response(case, "DeleteRepositoryPolicy", &deleted, &name, &policy);

        // The delete removed it: reading or deleting again reports nothing there.
        for operation in ["GetRepositoryPolicy", "DeleteRepositoryPolicy"] {
            refused(
                &router,
                case,
                operation,
                &name_request(&name),
                "RepositoryPolicyNotFoundException",
            )
            .await;
        }

        // And the repository still takes a policy afterwards.
        ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &set_request(&name, &policy),
        )
        .await;
        let got = ok(&router, case, "GetRepositoryPolicy", &name_request(&name)).await;
        assert_policy_response(case, "GetRepositoryPolicy", &got, &name, &policy);
    }
}

#[tokio::test]
async fn a_policy_belongs_to_its_repository_and_goes_with_it() {
    let router = emulator();
    let case = "policy follows the repository";
    let policy = policy_forms()[0].1.clone();
    let name = "policy-lifecycle";
    create_repository(&router, case, name).await;
    ok(
        &router,
        case,
        "SetRepositoryPolicy",
        &set_request(name, &policy),
    )
    .await;

    ok(
        &router,
        case,
        "DeleteRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
    // The repository is gone, so its policy is not there to read.
    refused(
        &router,
        case,
        "GetRepositoryPolicy",
        &name_request(name),
        "RepositoryNotFoundException",
    )
    .await;

    // A repository re-created under the same name starts without a policy: the policy
    // belonged to the record that was deleted, not to the name.
    create_repository(&router, case, name).await;
    refused(
        &router,
        case,
        "GetRepositoryPolicy",
        &name_request(name),
        "RepositoryPolicyNotFoundException",
    )
    .await;
}

#[tokio::test]
async fn every_policy_operation_refuses_a_repository_that_does_not_exist() {
    let router = emulator();
    // Names differing in form — length, separators and the nested path the
    // `RepositoryName` pattern allows — none of them created.
    let missing = ["ab", "no-such-repository", "team_a/service.api", "x9"];
    let policy = policy_forms()[0].1.clone();

    for name in missing {
        for operation in POLICY_OPERATIONS {
            let case = format!("{operation} on missing {name:?}");
            let body = if operation == "SetRepositoryPolicy" {
                set_request(name, &policy)
            } else {
                name_request(name)
            };
            refused(
                &router,
                &case,
                operation,
                &body,
                "RepositoryNotFoundException",
            )
            .await;
        }

        // The repository comes first: a set whose policy is ALSO wrong — unreadable, or
        // one that would lock the policy out — is still reported as the missing
        // repository rather than as the bad document.
        let also_wrong = [
            ("unreadable policy", "not a policy at all".to_string()),
            (
                "locking policy without force",
                json!({"Statement": [{"Effect": "Deny", "Principal": "*", "Action": "ecr:SetRepositoryPolicy"}]})
                    .to_string(),
            ),
        ];
        for (why, text) in also_wrong {
            let case = format!("SetRepositoryPolicy on missing {name:?} with an {why}");
            refused(
                &router,
                &case,
                "SetRepositoryPolicy",
                &set_request(name, &text),
                "RepositoryNotFoundException",
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_registry_other_than_this_one_is_refused_and_this_one_is_served() {
    let router = emulator();
    let name = "policy-registry-scope";
    let policy = policy_forms()[0].1.clone();
    create_repository(&router, "registry scope", name).await;
    // Give the repository a policy first, so a refusal below cannot pass for want of
    // one: each of the three operations has something to act on.
    ok(
        &router,
        "registry scope",
        "SetRepositoryPolicy",
        &set_request(name, &policy),
    )
    .await;

    // A registry that is not this one is refused, whichever operation names it, and
    // the stored policy is untouched by the refusal.
    for operation in POLICY_OPERATIONS {
        let case = format!("{operation} naming another registry");
        let mut body = if operation == "SetRepositoryPolicy" {
            set_request(name, &policy)
        } else {
            name_request(name)
        };
        body["registryId"] = json!(OTHER_REGISTRY_ID);
        refused(
            &router,
            &case,
            operation,
            &body,
            "InvalidParameterException",
        )
        .await;
        let got = ok(&router, &case, "GetRepositoryPolicy", &name_request(name)).await;
        assert_policy_response(&case, "GetRepositoryPolicy", &got, name, &policy);
    }

    // Naming THIS registry explicitly is served, in the order a caller uses the three:
    // the set stores the document, the read reports it, the delete reports and removes
    // it. Spelled out rather than looped, so the sequence is the test's own.
    let mut set = set_request(name, &policy);
    set["registryId"] = json!(REGISTRY_ID);
    let case = "SetRepositoryPolicy naming this registry";
    let answered = ok(&router, case, "SetRepositoryPolicy", &set).await;
    assert_policy_response(case, "SetRepositoryPolicy", &answered, name, &policy);

    let mut read = name_request(name);
    read["registryId"] = json!(REGISTRY_ID);
    let case = "GetRepositoryPolicy naming this registry";
    let answered = ok(&router, case, "GetRepositoryPolicy", &read).await;
    assert_policy_response(case, "GetRepositoryPolicy", &answered, name, &policy);

    let case = "DeleteRepositoryPolicy naming this registry";
    let answered = ok(&router, case, "DeleteRepositoryPolicy", &read).await;
    assert_policy_response(case, "DeleteRepositoryPolicy", &answered, name, &policy);
    refused(
        &router,
        case,
        "GetRepositoryPolicy",
        &read,
        "RepositoryPolicyNotFoundException",
    )
    .await;
}

#[tokio::test]
async fn text_that_is_not_a_policy_document_is_refused_and_changes_nothing() {
    let router = emulator();
    let name = "policy-invalid-text";
    let kept = policy_forms()[0].1.clone();
    create_repository(&router, "invalid policy text", name).await;
    ok(
        &router,
        "invalid policy text",
        "SetRepositoryPolicy",
        &set_request(name, &kept),
    )
    .await;

    for (case, text) in not_policy_documents() {
        refused(
            &router,
            case,
            "SetRepositoryPolicy",
            &set_request(name, text),
            "InvalidParameterException",
        )
        .await;
        let got = ok(&router, case, "GetRepositoryPolicy", &name_request(name)).await;
        assert_policy_response(case, "GetRepositoryPolicy", &got, name, &kept);
    }

    // One character past the published length bound: `RepositoryPolicyText` is
    // `@length(0..10240)`, so the boundary refuses the longer document before the
    // handler sees it, and the stored policy is untouched. (The bound itself, and one
    // character inside it, are rows of the accepted table.)
    let past_bound = policy_of_length(MAX_POLICY_TEXT_LEN + 1);
    let answer = call(
        &router,
        "SetRepositoryPolicy",
        &set_request(name, &past_bound),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"one past the maximum policy length\": expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    let got = ok(
        &router,
        "one past the maximum policy length",
        "GetRepositoryPolicy",
        &name_request(name),
    )
    .await;
    assert_policy_response(
        "one past the maximum policy length",
        "GetRepositoryPolicy",
        &got,
        name,
        &kept,
    );
}

#[tokio::test]
async fn a_policy_that_would_lock_out_future_policies_needs_force() {
    let router = emulator();

    // Each row is a policy and whether it would deny EVERY principal the right to set
    // another policy — the condition the `force` flag exists for. The rows differ along
    // the action element (`Action` or `NotAction`), the pattern that covers
    // `ecr:SetRepositoryPolicy`, the form the wildcard principal takes, and the effect.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "deny the action itself to everyone",
            json!({"Statement": [{"Effect": "Deny", "Principal": "*", "Action": "ecr:SetRepositoryPolicy"}]}),
            true,
        ),
        (
            "deny every ecr action to everyone",
            json!({"Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"}, "Action": "ecr:*"}]}),
            true,
        ),
        (
            "deny everything to everyone",
            json!({"Statement": [{"Effect": "Deny", "Principal": {"AWS": ["*"]}, "Action": "*"}]}),
            true,
        ),
        (
            "deny a wildcard action prefix to everyone",
            json!({"Statement": [{"Effect": "Deny", "Principal": "*", "Action": ["ecr:Get*", "ecr:Set*"]}]}),
            true,
        ),
        (
            // `NotAction` names what the statement does NOT apply to, so this denies
            // every action except the one it names — this one included.
            "deny everything except another action, to everyone",
            json!({"Statement": [{"Effect": "Deny", "Principal": "*", "NotAction": "ecr:BatchGetImage"}]}),
            true,
        ),
        (
            "one locking statement among several",
            json!({"Statement": [
                {"Effect": "Allow", "Principal": "*", "Action": "ecr:BatchGetImage"},
                {"Effect": "Deny", "Principal": "*", "Action": "ecr:SetRepositoryPolicy"}
            ]}),
            true,
        ),
        (
            "deny everything except this action, to everyone",
            json!({"Statement": [{"Effect": "Deny", "Principal": "*", "NotAction": ["ecr:SetRepositoryPolicy"]}]}),
            false,
        ),
        (
            "deny the action to one named principal",
            json!({"Statement": [{"Effect": "Deny", "Principal": {"AWS": format!("arn:aws:iam::{OTHER_REGISTRY_ID}:root")}, "Action": "ecr:SetRepositoryPolicy"}]}),
            false,
        ),
        (
            "deny another action to everyone",
            json!({"Statement": [{"Effect": "Deny", "Principal": "*", "Action": "ecr:BatchGetImage"}]}),
            false,
        ),
        (
            "allow the action to everyone",
            json!({"Statement": [{"Effect": "Allow", "Principal": "*", "Action": "ecr:SetRepositoryPolicy"}]}),
            false,
        ),
    ];

    for (index, (case, policy, needs_force)) in cases.into_iter().enumerate() {
        let name = format!("policy-force-{index}");
        create_repository(&router, case, &name).await;
        let text = policy.to_string();

        if needs_force {
            // Unforced, it is refused and nothing is stored.
            refused(
                &router,
                case,
                "SetRepositoryPolicy",
                &set_request(&name, &text),
                "InvalidParameterException",
            )
            .await;
            refused(
                &router,
                case,
                "GetRepositoryPolicy",
                &name_request(&name),
                "RepositoryPolicyNotFoundException",
            )
            .await;

            // Forced, the caller gets the lock-out they asked for.
            let mut forced = set_request(&name, &text);
            forced["force"] = json!(true);
            let set = ok(&router, case, "SetRepositoryPolicy", &forced).await;
            assert_policy_response(case, "SetRepositoryPolicy", &set, &name, &text);
        } else {
            let set = ok(
                &router,
                case,
                "SetRepositoryPolicy",
                &set_request(&name, &text),
            )
            .await;
            assert_policy_response(case, "SetRepositoryPolicy", &set, &name, &text);
        }

        let got = ok(&router, case, "GetRepositoryPolicy", &name_request(&name)).await;
        assert_policy_response(case, "GetRepositoryPolicy", &got, &name, &text);
    }
}
