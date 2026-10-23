// SPDX-License-Identifier: CC0-1.0

//! Deleting a pull through cache rule, end to end over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.
//!
//! The rule under test: `DeletePullThroughCacheRule` removes the registry's record for
//! one `ecrRepositoryPrefix` — addressed by either spelling of the trailing slash the
//! prefix documentation calls assumed — and reports that removed record's own stored
//! members (`ecrRepositoryPrefix`, `upstreamRegistryUrl`, `upstreamRepositoryPrefix`,
//! `registryId`, `credentialArn`, `customRoleArn`, `createdAt`); afterwards no operation
//! reading the rule table finds that prefix, every other rule is untouched, and a prefix
//! the registry holds no rule for is refused with `PullThroughCacheRuleNotFoundException`
//! without changing anything.

use serde_json::{json, Value};

use crate::wire::{at, call, emulator, ok, refused, REGISTRY_ID};

/// A Secrets Manager secret ARN that satisfies `CredentialArn`'s modelled pattern
/// (smithy_query shape=CredentialArn:
/// `^arn:aws(-\w+)*:secretsmanager:[a-zA-Z0-9-:]+:secret:ecr\-pullthroughcache\/...$`).
const CREDENTIAL_ARN: &str =
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/upstream-AbCdEf";

/// An IAM role ARN for `customRoleArn`, which the model constrains only by length.
const CUSTOM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ecr-pull-through-cache";

/// `upstreamRepositoryPrefix`'s documented default: "When this field isn't specified,
/// Amazon ECR will use the ROOT."
const ROOT: &str = "ROOT";

/// The members `DeletePullThroughCacheRule` reports, from the model's
/// `DeletePullThroughCacheRuleResponse` (smithy_query operation=DeletePullThroughCacheRule).
/// `upstreamRegistry` is deliberately absent: the response shape does not carry it.
const DELETE_OUTPUT_MEMBERS: [&str; 7] = [
    "ecrRepositoryPrefix",
    "upstreamRegistryUrl",
    "createdAt",
    "registryId",
    "credentialArn",
    "customRoleArn",
    "upstreamRepositoryPrefix",
];

/// `PullThroughCacheRuleRepositoryPrefix`'s modelled maximum length (smithy_query
/// operation=DeletePullThroughCacheRule: `length {min: 2, max: 30}`).
const MAX_PREFIX_LENGTH: usize = 30;

/// One row of the delete table: a rule to create, and the spelling of its prefix the
/// delete is addressed with.
struct Case<'a> {
    name: &'a str,
    /// The prefix `CreatePullThroughCacheRule` is given.
    created_prefix: &'a str,
    /// The prefix `DeletePullThroughCacheRule` is given — the same namespace, sometimes
    /// spelled with the assumed trailing slash the prefix doc describes.
    deleted_prefix: &'a str,
    upstream_registry: Option<&'a str>,
    upstream_registry_url: &'a str,
    upstream_repository_prefix: Option<&'a str>,
    credential_arn: Option<&'a str>,
    custom_role_arn: Option<&'a str>,
}

/// The delete table. Rows differ along everything the rule generalises over: which
/// optional members the stored rule carries, which upstream registry it caches from (the
/// values of the model's `UpstreamRegistry` enum — smithy_query shape=UpstreamRegistry),
/// how deep the prefix is, and which spelling of the prefix the delete names.
fn delete_cases() -> Vec<Case<'static>> {
    vec![
        Case {
            name: "a bare rule, every optional member omitted",
            created_prefix: "bare-cache",
            deleted_prefix: "bare-cache",
            upstream_registry: None,
            upstream_registry_url: "public.ecr.aws",
            upstream_repository_prefix: None,
            credential_arn: None,
            custom_role_arn: None,
        },
        Case {
            name: "a rule with a credential and an upstream prefix",
            created_prefix: "docker-hub-cache",
            deleted_prefix: "docker-hub-cache",
            upstream_registry: Some("docker-hub"),
            upstream_registry_url: "registry-1.docker.io",
            upstream_repository_prefix: Some("library"),
            credential_arn: Some(CREDENTIAL_ARN),
            custom_role_arn: None,
        },
        Case {
            name: "a rule with both ARNs, deleted by the prefix plus the assumed slash",
            created_prefix: "ghcr-cache",
            deleted_prefix: "ghcr-cache/",
            upstream_registry: None,
            upstream_registry_url: "ghcr.io",
            upstream_repository_prefix: Some("group/project"),
            credential_arn: Some(CREDENTIAL_ARN),
            custom_role_arn: Some(CUSTOM_ROLE_ARN),
        },
        Case {
            name: "a rule created with the trailing slash, deleted without it",
            created_prefix: "quay-cache/",
            deleted_prefix: "quay-cache",
            upstream_registry: Some("quay"),
            upstream_registry_url: "quay.io",
            upstream_repository_prefix: None,
            credential_arn: Some(CREDENTIAL_ARN),
            custom_role_arn: None,
        },
        Case {
            name: "a nested prefix with a custom role only",
            created_prefix: "team/k8s",
            deleted_prefix: "team/k8s",
            upstream_registry: None,
            upstream_registry_url: "registry.k8s.io",
            upstream_repository_prefix: Some("kube"),
            credential_arn: None,
            custom_role_arn: Some(CUSTOM_ROLE_ARN),
        },
        Case {
            name: "the shortest prefix the model's length constraint allows",
            created_prefix: "az",
            deleted_prefix: "az",
            upstream_registry: None,
            upstream_registry_url: "contoso.azurecr.io",
            upstream_repository_prefix: None,
            credential_arn: Some(CREDENTIAL_ARN),
            custom_role_arn: None,
        },
        Case {
            name: "a prefix at the model's 30-character maximum",
            created_prefix: "abcdefghij-klmnopqrst-uvwxyz12",
            deleted_prefix: "abcdefghij-klmnopqrst-uvwxyz12",
            upstream_registry: Some("gitlab-container-registry"),
            upstream_registry_url: "registry.gitlab.com",
            upstream_repository_prefix: Some("group/project"),
            credential_arn: None,
            custom_role_arn: Some(CUSTOM_ROLE_ARN),
        },
        Case {
            name: "an ecr upstream whose registry is resolved from its URL",
            created_prefix: "ecr-cache",
            deleted_prefix: "ecr-cache",
            upstream_registry: None,
            upstream_registry_url: "000000000000.dkr.ecr.us-east-2.amazonaws.com",
            upstream_repository_prefix: Some("team"),
            credential_arn: Some(CREDENTIAL_ARN),
            custom_role_arn: Some(CUSTOM_ROLE_ARN),
        },
        Case {
            name: "chainguard, named because the model documents no URL for it",
            created_prefix: "chainguard-cache",
            deleted_prefix: "chainguard-cache/",
            upstream_registry: Some("chainguard"),
            upstream_registry_url: "cgr.dev",
            upstream_repository_prefix: None,
            credential_arn: Some(CREDENTIAL_ARN),
            custom_role_arn: None,
        },
    ]
}

/// The `CreatePullThroughCacheRule` request one table row describes.
fn create_request(case: &Case<'_>) -> Value {
    let mut request = json!({
        "ecrRepositoryPrefix": case.created_prefix,
        "upstreamRegistryUrl": case.upstream_registry_url,
    });
    let members = [
        ("upstreamRegistry", case.upstream_registry),
        ("upstreamRepositoryPrefix", case.upstream_repository_prefix),
        ("credentialArn", case.credential_arn),
        ("customRoleArn", case.custom_role_arn),
    ];
    for (member, value) in members {
        if let Some(value) = value {
            request[member] = json!(value);
        }
    }
    request
}

/// Every prefix the registry currently holds a rule for, as
/// `DescribePullThroughCacheRules` reports them.
async fn every_prefix(
    router: &::smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> Vec<String> {
    let body = ok(router, case, "DescribePullThroughCacheRules", &json!({})).await;
    let rules = at(&body, "/pullThroughCacheRules");
    let rules = rules
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: pullThroughCacheRules must be a list: {body:?}"))
        .clone();
    rules
        .iter()
        .map(|rule| {
            at(rule, "/ecrRepositoryPrefix")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_delete_reports_the_removed_rule_and_leaves_the_others_alone() {
    let router = emulator();
    let cases = delete_cases();

    // Every rule exists first, so each delete is made against a populated registry and
    // can be seen to touch exactly one record.
    let mut created: Vec<Value> = Vec::new();
    for case in &cases {
        created.push(
            ok(
                &router,
                case.name,
                "CreatePullThroughCacheRule",
                &create_request(case),
            )
            .await,
        );
    }

    let mut remaining = every_prefix(&router, "before any delete").await;
    assert_eq!(
        remaining.len(),
        cases.len(),
        "every created rule is held before the deletes: {remaining:?}"
    );

    for (case, created) in cases.iter().zip(&created) {
        let deleted = ok(
            &router,
            case.name,
            "DeletePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": case.deleted_prefix}),
        )
        .await;

        // The delete reports the record that was removed, member for member, as the
        // create that stored it reported it — not what the delete request carried.
        for member in DELETE_OUTPUT_MEMBERS {
            let pointer = format!("/{member}");
            assert_eq!(
                at(&deleted, &pointer),
                at(created, &pointer),
                "case {:?}: the delete reports the stored {member} {:?}, got {:?}",
                case.name,
                at(created, &pointer),
                at(&deleted, &pointer)
            );
        }
        assert_eq!(
            at(&deleted, "/registryId"),
            json!(REGISTRY_ID),
            "case {:?}: the delete reports this registry, got {deleted:?}",
            case.name
        );
        assert_eq!(
            at(&deleted, "/upstreamRepositoryPrefix"),
            json!(case.upstream_repository_prefix.unwrap_or(ROOT)),
            "case {:?}: the delete reports the stored upstream prefix, got {deleted:?}",
            case.name
        );
        assert!(
            at(&deleted, "/createdAt").as_f64().is_some_and(|t| t > 0.0),
            "case {:?}: the delete reports the rule's creation instant, got {:?}",
            case.name,
            at(&deleted, "/createdAt")
        );
        // `upstreamRegistry` is not a member of this operation's output shape.
        assert_eq!(
            at(&deleted, "/upstreamRegistry"),
            Value::Null,
            "case {:?}: the delete response carries no upstreamRegistry, got {deleted:?}",
            case.name
        );

        // The record is gone: the reporting operation no longer finds the prefix, by
        // either spelling, and a second delete of the same prefix cannot find it either.
        // The slash spelling is only probed where it still fits
        // `PullThroughCacheRuleRepositoryPrefix`'s modelled `length(2..30)` — past that
        // the boundary refuses the request before any handler sees it.
        let normalized = case.created_prefix.trim_end_matches('/');
        let mut spellings = vec![normalized.to_string()];
        if normalized.len() < MAX_PREFIX_LENGTH {
            spellings.push(format!("{normalized}/"));
        }
        for prefix in spellings {
            refused(
                &router,
                case.name,
                "DescribePullThroughCacheRules",
                &json!({"ecrRepositoryPrefixes": [prefix]}),
                "PullThroughCacheRuleNotFoundException",
            )
            .await;
            refused(
                &router,
                case.name,
                "DeletePullThroughCacheRule",
                &json!({"ecrRepositoryPrefix": prefix}),
                "PullThroughCacheRuleNotFoundException",
            )
            .await;
        }

        // And the other operations reading the same record agree it is gone.
        for operation in [
            "UpdatePullThroughCacheRule",
            "ValidatePullThroughCacheRule",
        ] {
            refused(
                &router,
                case.name,
                operation,
                &json!({"ecrRepositoryPrefix": normalized}),
                "PullThroughCacheRuleNotFoundException",
            )
            .await;
        }

        // Exactly this one record left the registry. The stored record spells the prefix
        // the way the create was given it, so that is the spelling the listing carries.
        remaining.retain(|prefix| prefix != case.created_prefix);
        let listed = every_prefix(&router, case.name).await;
        assert_eq!(
            listed, remaining,
            "case {:?}: the delete removed only its own rule, expected {remaining:?}, \
             got {listed:?}",
            case.name
        );
    }

    assert!(
        every_prefix(&router, "after every delete").await.is_empty(),
        "every rule was deleted"
    );
}

#[tokio::test]
async fn a_prefix_no_rule_answers_to_is_refused_and_changes_nothing() {
    let router = emulator();
    ok(
        &router,
        "the one rule",
        "CreatePullThroughCacheRule",
        &json!({
            "ecrRepositoryPrefix": "known-cache",
            "upstreamRegistryUrl": "public.ecr.aws",
        }),
    )
    .await;

    // Prefixes the registry holds no rule for: a plain one, a nested one, the reserved
    // `ROOT` the prefix pattern admits, a prefix of the known one and a prefix the known
    // one is a prefix of — a rule answers to its whole prefix, not to a fragment of it.
    for (case, prefix) in [
        ("never created", "unknown-cache"),
        ("nested and never created", "team/unknown"),
        ("the reserved ROOT", "ROOT"),
        ("a proper prefix of the known one", "known"),
        ("the known one with more path", "known-cache/more"),
        ("the shortest allowed prefix", "ab"),
    ] {
        let body = refused(
            &router,
            case,
            "DeletePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": prefix}),
            "PullThroughCacheRuleNotFoundException",
        )
        .await;
        assert!(
            at(&body, "/message")
                .as_str()
                .is_some_and(|message| message.contains(prefix)),
            "case {case:?}: the refusal names the prefix it could not find, got {body:?}"
        );
    }

    // None of those refusals removed the rule the registry does hold.
    let listed = every_prefix(&router, "after the refusals").await;
    assert_eq!(
        listed,
        vec!["known-cache".to_string()],
        "a refused delete removes nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn another_accounts_registry_is_refused() {
    let router = emulator();
    ok(
        &router,
        "the rule to protect",
        "CreatePullThroughCacheRule",
        &json!({
            "ecrRepositoryPrefix": "own-cache",
            "upstreamRegistryUrl": "public.ecr.aws",
        }),
    )
    .await;

    // `registryId` is modelled `^[0-9]{12}$`, so each of these is a well-formed account
    // id that is simply not this registry's.
    for other in ["123456789012", "999999999999", "000000000001"] {
        refused(
            &router,
            other,
            "DeletePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": "own-cache", "registryId": other}),
            "InvalidParameterException",
        )
        .await;
    }
    let listed = every_prefix(&router, "after the refusals").await;
    assert_eq!(
        listed,
        vec!["own-cache".to_string()],
        "a delete for another registry removes nothing, got {listed:?}"
    );

    // This registry's own id, named explicitly, is accepted.
    let deleted = ok(
        &router,
        "this registry named explicitly",
        "DeletePullThroughCacheRule",
        &json!({"ecrRepositoryPrefix": "own-cache", "registryId": REGISTRY_ID}),
    )
    .await;
    assert_eq!(
        at(&deleted, "/ecrRepositoryPrefix"),
        json!("own-cache"),
        "the delete reports the rule it removed, got {deleted:?}"
    );
    assert!(
        every_prefix(&router, "after the delete").await.is_empty(),
        "the accepted delete removed the rule"
    );
}

#[tokio::test]
async fn a_prefix_can_be_created_again_after_its_rule_is_deleted() {
    let router = emulator();
    // Deleting frees the namespace: the same prefix takes a new rule, with a different
    // upstream, and the new record is the one the registry reports.
    for (case, first_url, first_registry, second_url, second_registry) in [
        (
            "the same spelling both times",
            "public.ecr.aws",
            "ecr-public",
            "quay.io",
            "quay",
        ),
        (
            "recreated after a slash-spelled delete",
            "quay.io",
            "quay",
            "registry.k8s.io",
            "k8s",
        ),
    ] {
        let prefix = "recycled-cache";
        let created = ok(
            &router,
            case,
            "CreatePullThroughCacheRule",
            &json!({
                "ecrRepositoryPrefix": prefix,
                "upstreamRegistryUrl": first_url,
                "upstreamRegistry": first_registry,
            }),
        )
        .await;
        let deleted = ok(
            &router,
            case,
            "DeletePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": format!("{prefix}/")}),
        )
        .await;
        assert_eq!(
            at(&deleted, "/upstreamRegistryUrl"),
            at(&created, "/upstreamRegistryUrl"),
            "case {case:?}: the delete reports the first rule's upstream, got {deleted:?}"
        );

        let recreated = ok(
            &router,
            case,
            "CreatePullThroughCacheRule",
            &json!({
                "ecrRepositoryPrefix": prefix,
                "upstreamRegistryUrl": second_url,
                "upstreamRegistry": second_registry,
            }),
        )
        .await;
        assert_eq!(
            at(&recreated, "/upstreamRegistry"),
            json!(second_registry),
            "case {case:?}: the prefix took a new rule after the delete, got {recreated:?}"
        );
        let described = ok(
            &router,
            case,
            "DescribePullThroughCacheRules",
            &json!({"ecrRepositoryPrefixes": [prefix]}),
        )
        .await;
        assert_eq!(
            at(&described, "/pullThroughCacheRules/0/upstreamRegistryUrl"),
            json!(second_url),
            "case {case:?}: the registry reports the new rule, got {described:?}"
        );

        // Clean up so the next row starts from the same empty namespace.
        let answer = call(
            &router,
            "DeletePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": prefix}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the recreated rule is deletable too, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
