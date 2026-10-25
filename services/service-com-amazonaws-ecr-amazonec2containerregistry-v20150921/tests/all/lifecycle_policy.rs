// SPDX-License-Identifier: CC0-1.0

//! The ECR lifecycle-policy document, end to end over the wire.
//!
//! Every case goes through the shared `wire` harness, so it exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a repository carries AT MOST ONE lifecycle policy; a put accepts
//! a document exactly when it satisfies the ECR lifecycle policy template's schema,
//! stores the document ECR reports for it (the same document with the whitespace between
//! its JSON tokens removed) and replaces whatever was there, a read reports the stored
//! document, and a delete removes it — with a repository that carries no policy refused by
//! the read and the delete rather than answered with an empty success. So the table rows
//! below differ along what the schema generalises over (which members are present, their
//! types, their enumerated values, the counts and the rule ordering), not along the names
//! of the repository or the tags.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator, ok, refused, REGISTRY_ID};

/// The modelled minimum length of `lifecyclePolicyText` (`@length(min: 100, max: 30720)`
/// on `LifecyclePolicyText`, per `smithy_query(query="operation",
/// operation="PutLifecyclePolicy")`). A shorter document is refused at the BOUNDARY with
/// `ValidationException`, which is a different contract from the document schema these
/// cases are about — so every case here is padded past it.
const MIN_POLICY_TEXT_LEN: usize = 100;

/// A repository to hang a policy on, created through `CreateRepository` so the test only
/// ever observes persisted state.
async fn repository(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        name,
        "CreateRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
}

/// Render a policy document as the wire carries it: a JSON STRING, padded with trailing
/// whitespace past [`MIN_POLICY_TEXT_LEN`] so the modelled length bound never decides a
/// case about the document's schema. Trailing whitespace is insignificant to a JSON
/// parser, so padding cannot turn a bad document into a good one or the other way round.
fn policy_text(document: &str) -> String {
    let mut text = document.to_string();
    while text.len() < MIN_POLICY_TEXT_LEN {
        text.push(' ');
    }
    text
}

/// A policy document built from its rules.
fn policy_of(rules: Value) -> String {
    policy_text(&json!({ "rules": rules }).to_string())
}

/// The smallest rule the template allows: one expiry action selected by a count of
/// images, with the catch-all `tagStatus`.
fn any_count_rule(priority: i64, count: i64) -> Value {
    json!({
        "rulePriority": priority,
        "selection": {
            "tagStatus": "any",
            "countType": "imageCountMoreThan",
            "countNumber": count,
        },
        "action": { "type": "expire" },
    })
}

#[tokio::test]
async fn a_lifecycle_policy_put_is_read_back_as_stored_and_then_deleted() {
    let router = emulator();
    let name = "platform/api";
    repository(&router, name).await;

    // Nothing is set yet, so neither the read nor the delete may answer an empty success.
    for operation in ["GetLifecyclePolicy", "DeleteLifecyclePolicy"] {
        refused(
            &router,
            "no policy set",
            operation,
            &json!({ "repositoryName": name }),
            "LifecyclePolicyNotFoundException",
        )
        .await;
    }

    // Deliberately irregular whitespace: what comes back is the SAME document with the
    // whitespace between its tokens removed, which is what ECR reports — the member order
    // and every token are the ones that were put.
    // `lifecycle_policy_document_identity` is where that spelling is pinned over a table
    // of documents; here one document stands for it, to keep this suite about the
    // policy's lifecycle.
    let text = policy_text(
        "{ \"rules\" : [ {\n  \"rulePriority\": 1,\n  \"description\": \"keep ten\",\n  \
         \"action\": {\"type\": \"expire\"},\n  \"selection\": {\"tagStatus\": \"any\", \
         \"countType\": \"imageCountMoreThan\", \"countNumber\": 10}\n} ] }",
    );
    let reported = "{\"rules\":[{\"rulePriority\":1,\"description\":\"keep ten\",\"action\":\
         {\"type\":\"expire\"},\"selection\":{\"tagStatus\":\"any\",\"countType\":\
         \"imageCountMoreThan\",\"countNumber\":10}}]}";

    let put = ok(
        &router,
        "put",
        "PutLifecyclePolicy",
        &json!({ "repositoryName": name, "lifecyclePolicyText": text }),
    )
    .await;
    assert_eq!(
        put["lifecyclePolicyText"].as_str(),
        Some(reported),
        "case {:?}: the put must report the document it stored, got {put:?}",
        "put"
    );
    assert_eq!(
        put["repositoryName"].as_str(),
        Some(name),
        "case {:?}: the put must report the repository it wrote to, got {put:?}",
        "put"
    );
    assert_eq!(
        put["registryId"].as_str(),
        Some(REGISTRY_ID),
        "case {:?}: the put must report this registry, got {put:?}",
        "put"
    );

    let got = ok(
        &router,
        "read back",
        "GetLifecyclePolicy",
        &json!({ "repositoryName": name }),
    )
    .await;
    assert_eq!(
        got["lifecyclePolicyText"].as_str(),
        Some(reported),
        "case {:?}: the read must report the stored document, got {got:?}",
        "read back"
    );
    assert_eq!(
        got["repositoryName"].as_str(),
        Some(name),
        "case {:?}: the read must report the repository, got {got:?}",
        "read back"
    );
    assert_eq!(
        got["registryId"].as_str(),
        Some(REGISTRY_ID),
        "case {:?}: the read must report this registry, got {got:?}",
        "read back"
    );
    // Storing a policy evaluates it, so the read reports the instant it was last
    // evaluated; `lifecycle_policy_document_identity` is where that instant is pinned to
    // the put that set it.
    assert!(
        got["lastEvaluatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {:?}: the read must report the instant the policy was evaluated, got {got:?}",
        "read back"
    );

    // A put REPLACES: the second document is what the repository carries afterwards.
    let replacement = policy_of(json!([any_count_rule(1, 3)]));
    ok(
        &router,
        "replace",
        "PutLifecyclePolicy",
        &json!({ "repositoryName": name, "lifecyclePolicyText": replacement }),
    )
    .await;
    let got = ok(
        &router,
        "read replacement",
        "GetLifecyclePolicy",
        &json!({ "repositoryName": name }),
    )
    .await;
    assert_eq!(
        got["lifecyclePolicyText"].as_str(),
        Some(replacement.as_str()),
        "case {:?}: a put must replace the stored policy, got {got:?}",
        "read replacement"
    );

    // A read must not change what a later caller sees.
    let again = ok(
        &router,
        "read twice",
        "GetLifecyclePolicy",
        &json!({ "repositoryName": name }),
    )
    .await;
    assert_eq!(
        again["lifecyclePolicyText"], got["lifecyclePolicyText"],
        "case {:?}: two reads must report the same policy, got {again:?}",
        "read twice"
    );

    // The delete reports what it removed, and leaves the repository carrying nothing.
    let deleted = ok(
        &router,
        "delete",
        "DeleteLifecyclePolicy",
        &json!({ "repositoryName": name }),
    )
    .await;
    assert_eq!(
        deleted["lifecyclePolicyText"].as_str(),
        Some(replacement.as_str()),
        "case {:?}: the delete must report the policy it removed, got {deleted:?}",
        "delete"
    );
    for operation in ["GetLifecyclePolicy", "DeleteLifecyclePolicy"] {
        refused(
            &router,
            "after delete",
            operation,
            &json!({ "repositoryName": name }),
            "LifecyclePolicyNotFoundException",
        )
        .await;
    }

    // Deleting the policy leaves the repository itself in place.
    let described = ok(
        &router,
        "repository survives",
        "DescribeRepositories",
        &json!({ "repositoryNames": [name] }),
    )
    .await;
    assert_eq!(
        described["repositories"]
            .as_array()
            .map(|repositories| repositories.len()),
        Some(1),
        "case {:?}: deleting a policy must not delete the repository, got {described:?}",
        "repository survives"
    );
}

#[tokio::test]
async fn every_document_the_template_allows_is_accepted_and_round_trips() {
    let router = emulator();
    let name = "catalog/service";
    repository(&router, name).await;

    // Each row differs from the next along one axis the template generalises over: which
    // `tagStatus` selects the images, whether the count is a number of images or a span
    // of days, how many tags the filter names, how many rules the policy carries, and
    // whether the optional `description` is there at all.
    let accepted: Vec<(&str, String)> = vec![
        (
            "count of images, catch-all tag status",
            policy_of(json!([any_count_rule(1, 1)])),
        ),
        (
            "age in days, untagged images",
            policy_of(json!([{
                "rulePriority": 1,
                "description": "expire old untagged images",
                "selection": {
                    "tagStatus": "untagged",
                    "countType": "sinceImagePushed",
                    "countUnit": "days",
                    "countNumber": 30,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "tagged by several prefixes",
            policy_of(json!([{
                "rulePriority": 7,
                "selection": {
                    "tagStatus": "tagged",
                    "tagPrefixList": ["prod", "release", "rc"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 25,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "tagged by a wildcard pattern",
            policy_of(json!([{
                "rulePriority": 2,
                "selection": {
                    "tagStatus": "tagged",
                    "tagPatternList": ["*-dev"],
                    "countType": "sinceImagePushed",
                    "countUnit": "days",
                    "countNumber": 7,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "three rules with the catch-all evaluated last",
            policy_of(json!([
                {
                    "rulePriority": 1,
                    "selection": {
                        "tagStatus": "tagged",
                        "tagPrefixList": ["prod"],
                        "countType": "imageCountMoreThan",
                        "countNumber": 50,
                    },
                    "action": { "type": "expire" },
                },
                {
                    "rulePriority": 2,
                    "selection": {
                        "tagStatus": "untagged",
                        "countType": "sinceImagePushed",
                        "countUnit": "days",
                        "countNumber": 1,
                    },
                    "action": { "type": "expire" },
                },
                any_count_rule(999, 100),
            ])),
        ),
        (
            "a very large count",
            policy_of(json!([any_count_rule(1, 1_000_000)])),
        ),
        // The document's enumerated values are the modelled `TagStatus`,
        // `ImageActionType` and count types, which the policy format writes in lower
        // camel case: `crate::state::parse_lifecycle_policy` matches them against the
        // model's own spellings without regard to case, so the modelled spelling is
        // accepted too.
        (
            "the enumerated values in the modelled spelling",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "ANY",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                },
                "action": { "type": "EXPIRE" },
            }])),
        ),
        // A rule transitions images to another storage class instead of expiring them:
        // the modelled `ImageActionType` carries both, and a transition names the
        // `targetStorageClass` it moves them to.
        (
            "a transition to another storage class",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "untagged",
                    "countType": "sinceImagePushed",
                    "countUnit": "days",
                    "countNumber": 90,
                },
                "action": { "type": "transition", "targetStorageClass": "ARCHIVE" },
            }])),
        ),
        // A member the policy format does not define is read by nothing and refuses
        // nothing: the parser reads the members the format names.
        (
            "members the format does not define",
            policy_text(
                &json!({
                    "rules": [{
                        "rulePriority": 1,
                        "description": 17,
                        "enabled": true,
                        "selection": {
                            "tagStatus": "any",
                            "countType": "imageCountMoreThan",
                            "countNumber": 5,
                            "tagSuffixList": ["dev"],
                        },
                        "action": { "type": "expire" },
                    }],
                    "version": "2008-10-17",
                })
                .to_string(),
            ),
        ),
    ];

    for (case, text) in &accepted {
        let put = ok(
            &router,
            case,
            "PutLifecyclePolicy",
            &json!({ "repositoryName": name, "lifecyclePolicyText": text }),
        )
        .await;
        assert_eq!(
            put["lifecyclePolicyText"].as_str(),
            Some(text.as_str()),
            "case {case:?}: the put must report the text it stored, got {put:?}"
        );
        let got = ok(
            &router,
            case,
            "GetLifecyclePolicy",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            got["lifecyclePolicyText"].as_str(),
            Some(text.as_str()),
            "case {case:?}: the read must report the stored text, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_document_the_template_forbids_is_refused_and_stores_nothing() {
    let router = emulator();
    let name = "catalog/rejected";
    repository(&router, name).await;

    // A policy already in place, so each refusal can be checked to have left the stored
    // document untouched: a rejected put must not be a partial write.
    let baseline = policy_of(json!([any_count_rule(1, 5)]));
    ok(
        &router,
        "baseline",
        "PutLifecyclePolicy",
        &json!({ "repositoryName": name, "lifecyclePolicyText": baseline }),
    )
    .await;

    // A document that parses as JSON but drops its last byte: still long enough to pass
    // the modelled length bound, no longer a JSON value.
    let truncated = {
        let mut text = policy_of(json!([any_count_rule(1, 5)]));
        text.truncate(text.trim_end().len() - 1);
        policy_text(&text)
    };

    let refused_cases: Vec<(&str, String)> = vec![
        ("not JSON at all", truncated),
        (
            "a JSON array, not an object",
            policy_text(&json!([any_count_rule(1, 5)]).to_string()),
        ),
        ("no rules member", policy_text("{}")),
        ("rules is not an array", policy_text("{\"rules\": {}}")),
        ("rules is empty", policy_of(json!([]))),
        ("a rule is not an object", policy_of(json!(["expire"]))),
        ("a zero image count", policy_of(json!([any_count_rule(1, 0)]))),
        (
            "a negative image count",
            policy_of(json!([any_count_rule(1, -5)])),
        ),
        (
            "a fractional image count",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countNumber": 1.5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "no count number",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": { "tagStatus": "any", "countType": "imageCountMoreThan" },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "a tagged rule with no tag filter",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "tagged",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "a tagged rule with an empty tag filter",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "tagged",
                    "tagPrefixList": [],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "a tagged rule naming both tag filters",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "tagged",
                    "tagPrefixList": ["prod"],
                    "tagPatternList": ["prod*"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "an untagged rule carrying a tag filter",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "untagged",
                    "tagPrefixList": ["prod"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "no tag status",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": { "countType": "imageCountMoreThan", "countNumber": 5 },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "a count type the template does not define",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "any",
                    "countType": "sinceImagePulled",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "an age rule with no count unit",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "a count unit the template does not define",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "hours",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "a count of images carrying a count unit",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countUnit": "days",
                    "countNumber": 5,
                },
                "action": { "type": "expire" },
            }])),
        ),
        (
            "an action the template does not define",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                },
                "action": { "type": "delete" },
            }])),
        ),
        (
            "no action",
            policy_of(json!([{
                "rulePriority": 1,
                "selection": {
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                },
            }])),
        ),
        (
            "no selection",
            policy_of(json!([{ "rulePriority": 1, "action": { "type": "expire" } }])),
        ),
        (
            "a rule priority below the first",
            policy_of(json!([any_count_rule(0, 5)])),
        ),
        (
            "two rules sharing one priority",
            policy_of(json!([
                {
                    "rulePriority": 1,
                    "selection": {
                        "tagStatus": "tagged",
                        "tagPrefixList": ["prod"],
                        "countType": "imageCountMoreThan",
                        "countNumber": 5,
                    },
                    "action": { "type": "expire" },
                },
                any_count_rule(1, 10),
            ])),
        ),
        (
            "the catch-all rule evaluated before another",
            policy_of(json!([
                any_count_rule(1, 10),
                {
                    "rulePriority": 2,
                    "selection": {
                        "tagStatus": "tagged",
                        "tagPrefixList": ["prod"],
                        "countType": "imageCountMoreThan",
                        "countNumber": 5,
                    },
                    "action": { "type": "expire" },
                },
            ])),
        ),
    ];

    for (case, text) in &refused_cases {
        assert!(
            text.len() >= MIN_POLICY_TEXT_LEN,
            "case {case:?}: the document must clear the modelled length bound so the \
             handler is what refuses it, got {} characters",
            text.len()
        );
        refused(
            &router,
            case,
            "PutLifecyclePolicy",
            &json!({ "repositoryName": name, "lifecyclePolicyText": text }),
            "InvalidParameterException",
        )
        .await;
        let got = ok(
            &router,
            case,
            "GetLifecyclePolicy",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            got["lifecyclePolicyText"].as_str(),
            Some(baseline.as_str()),
            "case {case:?}: a refused put must leave the stored policy untouched, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_lifecycle_policy_call_naming_an_unknown_repository_or_registry_is_refused() {
    let router = emulator();
    let known = "known/repository";
    repository(&router, known).await;
    let text = policy_of(json!([any_count_rule(1, 5)]));

    // No repository of that name: every one of the three operations reports the missing
    // repository rather than inventing one.
    for operation in [
        "PutLifecyclePolicy",
        "GetLifecyclePolicy",
        "DeleteLifecyclePolicy",
    ] {
        refused(
            &router,
            "unknown repository",
            operation,
            &json!({ "repositoryName": "never/created", "lifecyclePolicyText": text }),
            "RepositoryNotFoundException",
        )
        .await;
    }

    // Another account's registry is not this one's to serve, whichever operation asks.
    for operation in [
        "PutLifecyclePolicy",
        "GetLifecyclePolicy",
        "DeleteLifecyclePolicy",
    ] {
        refused(
            &router,
            "another registry",
            operation,
            &json!({
                "registryId": "210987654321",
                "repositoryName": known,
                "lifecyclePolicyText": text,
            }),
            "InvalidParameterException",
        )
        .await;
    }

    // This registry, named explicitly, is accepted.
    ok(
        &router,
        "this registry named explicitly",
        "PutLifecyclePolicy",
        &json!({
            "registryId": REGISTRY_ID,
            "repositoryName": known,
            "lifecyclePolicyText": text,
        }),
    )
    .await;
}
