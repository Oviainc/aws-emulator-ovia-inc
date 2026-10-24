// SPDX-License-Identifier: CC0-1.0

//! `ValidatePullThroughCacheRule`, end to end over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.
//!
//! The rule under test: validation is a verdict on the rule the registry HOLDS, computed
//! from that stored record and nothing else. A rule validates — HTTP 200 with
//! `isValid: true` and an empty `failure` — when its stored configuration can
//! authenticate to its upstream: the upstream is one ECR documents as serving
//! anonymously, or the rule stores a credential of the kind its member documents (a
//! Secrets Manager `ecr-pullthroughcache/` secret for `credentialArn`, an IAM role for
//! `customRoleArn`). It is invalid — `isValid: false` — only with a `failure` naming the
//! stored configuration defect: a missing credential an authenticating upstream needs, a
//! credential that is not the ARN it has to be, or a rule naming no upstream at all.
//! Nothing about the emulator serving the call — in particular its lack of outbound
//! connectivity — is part of the verdict, and validating changes nothing.

use serde_json::{json, Value};

use crate::wire::{at, call, emulator, emulator_with_engine, ok, seed_pull_through_cache_rule};

/// A Secrets Manager secret ARN in the namespace `CredentialArn`'s modelled pattern
/// states (smithy_query operation=ValidatePullThroughCacheRule, member credentialArn:
/// `^arn:aws(-\w+)*:secretsmanager:[a-zA-Z0-9-:]+:secret:ecr\-pullthroughcache\/[a-zA-Z0-9\/_+=.@-]+$`).
const SECRET: &str =
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/upstream-AbCdEf";

/// A second secret, so a rule's credential is visibly its own.
const OTHER_SECRET: &str =
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/other-ZyXwVu";

/// An IAM role ARN for `customRoleArn`, documented as "the ARN of the IAM role associated
/// with the pull through cache rule"; the model constrains the member only by length.
const ROLE: &str = "arn:aws:iam::000000000000:role/ecr-pull-through-cache";

/// One row of the validation table: the rule to create and the verdict it earns.
struct Case<'a> {
    name: &'a str,
    prefix: &'a str,
    /// `upstreamRegistry` as the create request carries it; `None` leaves the member out,
    /// so the registry is resolved from the URL.
    upstream_registry: Option<&'a str>,
    upstream_registry_url: &'a str,
    credential_arn: Option<&'a str>,
    custom_role_arn: Option<&'a str>,
    /// The verdict `isValid` must carry.
    valid: bool,
    /// A fragment the `failure` must name, for a row that does not validate.
    failure_names: Option<&'a str>,
}

/// The validation table. The rows span the model's `UpstreamRegistry` values (smithy_query
/// shape=UpstreamRegistry) with the URL syntax the `upstreamRegistryUrl` member documents
/// for each, and differ in which credential the rule stores and in whether the upstream
/// needs one — including upstreams the issue never cited. The two anonymous upstreams are
/// the ECR user guide's supported-upstream table (Amazon ECR Public, Kubernetes); every
/// other upstream authenticates its callers.
fn cases() -> Vec<Case<'static>> {
    vec![
        Case {
            name: "ecr-public serves anonymously, so a rule with no credential is complete",
            prefix: "ecr-public-cache",
            upstream_registry: None,
            upstream_registry_url: "public.ecr.aws",
            credential_arn: None,
            custom_role_arn: None,
            valid: true,
            failure_names: None,
        },
        Case {
            name: "k8s serves anonymously too",
            prefix: "k8s-cache",
            upstream_registry: None,
            upstream_registry_url: "registry.k8s.io",
            credential_arn: None,
            custom_role_arn: None,
            valid: true,
            failure_names: None,
        },
        Case {
            name: "an anonymous upstream with a credential it does not need",
            prefix: "ecr-public-extra",
            upstream_registry: Some("ecr-public"),
            upstream_registry_url: "public.ecr.aws",
            credential_arn: Some(SECRET),
            custom_role_arn: None,
            valid: true,
            failure_names: None,
        },
        Case {
            name: "docker-hub with the secret its authentication needs",
            prefix: "docker-hub-cache",
            upstream_registry: Some("docker-hub"),
            upstream_registry_url: "registry-1.docker.io",
            credential_arn: Some(SECRET),
            custom_role_arn: None,
            valid: true,
            failure_names: None,
        },
        Case {
            name: "quay with a custom role instead of a secret",
            prefix: "quay-cache",
            upstream_registry: Some("quay"),
            upstream_registry_url: "quay.io",
            credential_arn: None,
            custom_role_arn: Some(ROLE),
            valid: true,
            failure_names: None,
        },
        Case {
            name: "azure-container-registry with both credentials",
            prefix: "azure-cache",
            upstream_registry: None,
            upstream_registry_url: "contoso.azurecr.io",
            credential_arn: Some(SECRET),
            custom_role_arn: Some(ROLE),
            valid: true,
            failure_names: None,
        },
        Case {
            name: "chainguard, a registry the model documents no URL for",
            prefix: "chainguard-cache",
            upstream_registry: Some("chainguard"),
            upstream_registry_url: "cgr.dev",
            credential_arn: Some(OTHER_SECRET),
            custom_role_arn: None,
            valid: true,
            failure_names: None,
        },
        Case {
            name: "an ECR upstream reached through a custom role",
            prefix: "ecr-cache",
            upstream_registry: None,
            upstream_registry_url: "000000000000.dkr.ecr.us-east-2.amazonaws.com",
            credential_arn: None,
            custom_role_arn: Some(ROLE),
            valid: true,
            failure_names: None,
        },
        Case {
            name: "github-container-registry authenticates and the rule stores no credential",
            prefix: "ghcr-cache",
            upstream_registry: None,
            upstream_registry_url: "ghcr.io",
            credential_arn: None,
            custom_role_arn: None,
            valid: false,
            failure_names: Some("requires authentication"),
        },
        Case {
            name: "gitlab-container-registry with an empty customRoleArn, which stores no role",
            prefix: "gitlab-cache",
            upstream_registry: Some("gitlab-container-registry"),
            upstream_registry_url: "registry.gitlab.com",
            credential_arn: None,
            // `customRoleArn` is modelled `@length(0..2048)`, so the empty string is a
            // legal value — and it names no role.
            custom_role_arn: Some(""),
            valid: false,
            failure_names: Some("requires authentication"),
        },
        Case {
            name: "a customRoleArn that is not an ARN at all",
            prefix: "role-plain-cache",
            upstream_registry: Some("docker-hub"),
            upstream_registry_url: "registry-1.docker.io",
            credential_arn: None,
            custom_role_arn: Some("ecr-pull-through-cache"),
            valid: false,
            failure_names: Some("customRoleArn"),
        },
        Case {
            name: "a customRoleArn naming another service's resource",
            prefix: "role-vendor-cache",
            upstream_registry: Some("docker-hub"),
            upstream_registry_url: "registry-1.docker.io",
            credential_arn: Some(SECRET),
            custom_role_arn: Some("arn:aws:s3:::ecr-pull-through-cache"),
            valid: false,
            failure_names: Some("customRoleArn"),
        },
        Case {
            name: "a customRoleArn naming an IAM resource that is no role",
            prefix: "role-type-cache",
            upstream_registry: Some("docker-hub"),
            upstream_registry_url: "registry-1.docker.io",
            credential_arn: None,
            custom_role_arn: Some("arn:aws:iam::000000000000:user/ecr-pull-through-cache"),
            valid: false,
            failure_names: Some("customRoleArn"),
        },
    ]
}

/// The `CreatePullThroughCacheRule` request one row describes.
fn create_request(case: &Case<'_>) -> Value {
    let mut request = json!({
        "ecrRepositoryPrefix": case.prefix,
        "upstreamRegistryUrl": case.upstream_registry_url,
    });
    let members = [
        ("upstreamRegistry", case.upstream_registry),
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

/// Assert the verdict `body` carries is the one `case` expects, and that its reason is
/// about the rule rather than about the emulator answering the call.
fn assert_verdict(case: &str, valid: bool, failure_names: Option<&str>, body: &Value) {
    assert_eq!(
        at(body, "/isValid"),
        json!(valid),
        "case {case:?}: expected isValid {valid}, got {:?}",
        at(body, "/isValid")
    );
    let failure = at(body, "/failure").as_str().unwrap_or_default().to_string();
    assert_eq!(
        failure.is_empty(),
        valid,
        "case {case:?}: a rule that validated carries an empty failure and one that did \
         not says why, got {failure:?}"
    );
    if let Some(fragment) = failure_names {
        assert!(
            failure.contains(fragment),
            "case {case:?}: the failure must name {fragment:?}, got {failure:?}"
        );
    }
    // The verdict is about the rule: the emulator's own lack of egress is not a property
    // of a pull through cache rule, so it is never the reason one is invalid.
    for forbidden in ["emulator", "outbound", "egress", "could not be verified"] {
        assert!(
            !failure.to_lowercase().contains(forbidden),
            "case {case:?}: the failure must describe the rule, not the service answering \
             the call, got {failure:?}"
        );
    }
}

/// The one rule the registry holds for `prefix`, as `DescribePullThroughCacheRules`
/// reports it.
async fn described(
    router: &::smithy_server_runtime::MultiServiceRouter,
    case: &str,
    prefix: &str,
) -> Value {
    let body = ok(
        router,
        case,
        "DescribePullThroughCacheRules",
        &json!({"ecrRepositoryPrefixes": [prefix]}),
    )
    .await;
    at(&body, "/pullThroughCacheRules/0")
}

#[tokio::test]
async fn a_rules_verdict_is_read_off_its_stored_configuration() {
    let router = emulator();

    for case in cases() {
        ok(
            &router,
            case.name,
            "CreatePullThroughCacheRule",
            &create_request(&case),
        )
        .await;

        let validated = ok(
            &router,
            case.name,
            "ValidatePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": case.prefix}),
        )
        .await;
        assert_verdict(case.name, case.valid, case.failure_names, &validated);

        // The answer is the stored rule projected onto this operation's output shape, so
        // the members it reports are the ones the reporting operation reports.
        let rule = described(&router, case.name, case.prefix).await;
        for member in [
            "/ecrRepositoryPrefix",
            "/upstreamRegistryUrl",
            "/registryId",
            "/credentialArn",
            "/customRoleArn",
            "/upstreamRepositoryPrefix",
        ] {
            assert_eq!(
                at(&validated, member),
                at(&rule, member),
                "case {:?}: the validate reports the stored {member}, got {:?} against \
                 the stored {:?}",
                case.name,
                at(&validated, member),
                at(&rule, member)
            );
        }

        // Validating is a read: the second call answers the same verdict and the stored
        // rule is untouched.
        let again = ok(
            &router,
            case.name,
            "ValidatePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": case.prefix}),
        )
        .await;
        assert_eq!(
            again, validated,
            "case {:?}: validating twice answers the same verdict, got {again:?}",
            case.name
        );
        assert_eq!(
            described(&router, case.name, case.prefix).await,
            rule,
            "case {:?}: validating changed the stored rule",
            case.name
        );
    }
}

/// One row of the credential-syntax table: the case, the `credentialArn` the rule stores,
/// and whether that value is a credential ECR could read the upstream's secret from.
type CredentialRow<'a> = (&'a str, &'a str, bool);

#[tokio::test]
async fn a_credential_the_rule_stores_has_to_be_the_arn_its_member_documents() {
    // The request boundary refuses a `credentialArn` outside `CredentialArn`'s modelled
    // pattern, so these STORED records are seeded: the verdict still has to be read off
    // them rather than assumed.
    let rows: Vec<CredentialRow<'_>> = vec![
        ("the documented secret ARN", SECRET, true),
        ("another secret of the same namespace", OTHER_SECRET, true),
        ("a value that is not an ARN", "ecr-pullthroughcache/upstream", false),
        (
            "an ARN of another service",
            "arn:aws:s3:::ecr-pullthroughcache/upstream",
            false,
        ),
        (
            "a secret outside the pull through cache namespace",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:other/upstream-AbCdEf",
            false,
        ),
        (
            "a secret ARN carrying no Region, which the modelled class still admits (its \
             `[a-zA-Z0-9-:]+` span covers the Region and account together, colon included)",
            "arn:aws:secretsmanager::000000000000:secret:ecr-pullthroughcache/upstream-AbCdEf",
            true,
        ),
        (
            "a secret of another AWS partition, which `aws(-\\w+)*` admits",
            "arn:aws-us-gov:secretsmanager:us-gov-west-1:000000000000:secret:ecr-pullthroughcache/upstream",
            true,
        ),
        (
            "a partition outside the modelled `aws(-\\w+)*`",
            "arn:azure:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/upstream",
            false,
        ),
        (
            "a secret name carrying a character outside the modelled class",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/upstream#1",
            false,
        ),
        (
            "a secret ARN whose name stops at the namespace",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/",
            false,
        ),
        (
            "a secret ARN naming another resource type",
            "arn:aws:secretsmanager:eu-west-1:000000000000:other:ecr-pullthroughcache/upstream",
            false,
        ),
    ];

    let (router, engine) = emulator_with_engine();
    for (case, credential, usable) in rows {
        // The upstream authenticates its callers, so the credential is what the rule's
        // validity turns on.
        seed_pull_through_cache_rule(
            &engine,
            "credential-cache",
            Some("docker-hub"),
            "registry-1.docker.io",
            Some(credential),
            None,
        );
        let validated = ok(
            &router,
            case,
            "ValidatePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": "credential-cache"}),
        )
        .await;
        assert_verdict(
            case,
            usable,
            (!usable).then_some("credentialArn"),
            &validated,
        );
        assert_eq!(
            at(&validated, "/credentialArn"),
            json!(credential),
            "case {case:?}: the validate reports the credential the rule stores, got {:?}",
            at(&validated, "/credentialArn")
        );
    }
}

#[tokio::test]
async fn a_rule_naming_no_upstream_registry_does_not_validate() {
    // `CreatePullThroughCacheRule` refuses a request that resolves to no upstream
    // registry, so a stored rule with no upstream URL is seeded.
    let (router, engine) = emulator_with_engine();
    for (case, url) in [("no upstream URL", ""), ("a blank upstream URL", "   ")] {
        seed_pull_through_cache_rule(&engine, "upstreamless", None, url, None, None);
        let validated = ok(
            &router,
            case,
            "ValidatePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": "upstreamless"}),
        )
        .await;
        assert_verdict(case, false, Some("upstreamRegistryUrl"), &validated);
    }
}

#[tokio::test]
async fn a_rule_is_validated_by_either_spelling_of_its_prefix() {
    let router = emulator();
    // "There is always an assumed / applied to the end of the prefix", so both spellings
    // address the one rule — and a prefix no rule answers to is a not-found rather than an
    // invalid rule.
    for (case, created, asked) in [
        ("the same spelling", "slashless-cache", "slashless-cache"),
        ("a trailing slash added", "added-slash-cache", "added-slash-cache/"),
        ("a trailing slash dropped", "dropped-slash-cache/", "dropped-slash-cache"),
    ] {
        ok(
            &router,
            case,
            "CreatePullThroughCacheRule",
            &json!({
                "ecrRepositoryPrefix": created,
                "upstreamRegistryUrl": "public.ecr.aws",
            }),
        )
        .await;
        let validated = ok(
            &router,
            case,
            "ValidatePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": asked}),
        )
        .await;
        assert_verdict(case, true, None, &validated);
        assert_eq!(
            at(&validated, "/ecrRepositoryPrefix"),
            json!(created),
            "case {case:?}: the rule answers with the prefix it was created under, got {:?}",
            at(&validated, "/ecrRepositoryPrefix")
        );
    }

    let answer = call(
        &router,
        "ValidatePullThroughCacheRule",
        &json!({"ecrRepositoryPrefix": "no-rule-cache"}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a prefix no rule answers to is refused with HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer
            .error_code()
            .ends_with("PullThroughCacheRuleNotFoundException"),
        "expected PullThroughCacheRuleNotFoundException, got {:?}",
        answer.error_code()
    );
}
