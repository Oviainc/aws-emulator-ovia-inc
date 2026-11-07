// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE each of this service's refusals states, over the wire.
//!
//! Every case drives a real `restJson1` request through the `MultiServiceRouter` the
//! production emulator serves with (the shared harness in `tests/all/harness.rs`), so what
//! is pinned here is the status, the error code and the `message` a CodeArtifact client
//! reads. A message is part of the wire contract — clients branch on it and operators read
//! it — so each rule below is asserted over a TABLE of inputs rather than on one example.
//!
//! The rules under test:
//!
//! 1. A conflict on a package-group PATTERN names the three things the pattern is unique
//!    within: the pattern itself (single-quoted), the domain holding it and the account
//!    owning that domain (both bare), with no sentence-final period.
//! 2. The refusal that states a PRECONDITION of the request's own target interpolates
//!    nothing: `DeletePackageGroup` on the root group answers one fixed sentence, naming
//!    neither the domain nor anything else the request carried.
//! 3. A `policyDocument` the policy parser cannot read as a policy is refused with ONE
//!    fixed sentence, whatever the document — the sentence names neither the member, the
//!    position the parse stopped at, nor the JSON type the document turned out to be.
//! 4. An ASSET the addressed package version does not hold is refused in a sentence naming
//!    the repository the request addressed and the whole coordinate that was resolved,
//!    rendered as a `PackageVersionCoordinateEntity` (format upper-cased, namespace
//!    included), plus the asset name that missed.
//! 5. A caller-supplied `resourceArn` that addresses nothing is refused in TWO different
//!    sentences, identically at every operation that takes one: a value that is not written
//!    as an ARN at all (fewer than the six colon-separated fields an ARN has) gets the ARN
//!    parser's sentence about ARN SYNTAX, while a value that IS written as an ARN but is
//!    not one of this service's gets the sentence about what it names.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT, REGION};
use crate::package_group_hierarchy::{create_package_group, describe_package_group, ROOT};
use crate::package_version_publish::{binary_content, publish, Publish};
use crate::resource_tags::{encode_query, list_tags, pairs, tag};

/// The error identities these refusals ride, with the status each carries. Source:
/// `smithy_query(query="operation",
/// operation="CreatePackageGroup,DeletePackageGroup,PutDomainPermissionsPolicy,GetPackageVersionAsset,TagResource")`
/// — `ConflictException` httpError 409, `ValidationException` 400,
/// `ResourceNotFoundException` 404.
const CONFLICT: (u16, &str) = (409, "ConflictException");
const INVALID: (u16, &str) = (400, "ValidationException");
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// Assert one refusal's identity and sentence, naming the case and the actual answer.
fn assert_refusal(case: &str, answer: &Answer, expected: (u16, &str), message: &str) {
    assert_eq!(
        (answer.status, answer.code()),
        (expected.0, expected.1.to_string()),
        "case {case:?}: expected {} {}, got {answer:?}",
        expected.0,
        expected.1
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        answer.message()
    );
}

// ---------------------------------------------------------------------------
// RULE 1: a package-group pattern is taken per domain per owning account.
// ---------------------------------------------------------------------------

/// RULE 1: the sentence a duplicate `CreatePackageGroup` states.
fn group_exists(domain: &str, owner: &str, pattern: &str) -> String {
    format!("Package Group '{pattern}' already exists in domain {domain} owned by account {owner}")
}

#[tokio::test]
async fn a_duplicate_package_group_names_the_pattern_its_domain_and_the_owning_account() {
    let router = emulator();

    // The rows differ along the domain (including the modelled extremes of `DomainName`,
    // 2 and 50 characters), along the pattern (a `PackageGroupPattern` is modelled
    // `@length(2..520)`, so `/a` is the shortest one a request can carry), along how deep
    // the pattern's path runs, and along whether the colliding group was created by this
    // suite or is the root group every domain holds from creation.
    let deep = format!("/npm/{}/*", "scope".repeat(8));
    let longest_domain = format!("d{}e", "x".repeat(48));
    let rows: Vec<(&str, String, String, bool)> = vec![
        (
            "a two-character domain, shortest pattern",
            "ab".to_string(),
            "/a".to_string(),
            true,
        ),
        (
            "a format-scoped pattern",
            "dup-groups".to_string(),
            "/npm/left/*".to_string(),
            true,
        ),
        (
            "a pattern naming one package",
            "dup-groups".to_string(),
            "/maven/com.acme/widget-core".to_string(),
            true,
        ),
        (
            "a deep pattern in a 50-character domain",
            longest_domain,
            deep,
            true,
        ),
        (
            // The root group is held from creation, so the collision needs no seed.
            "the root group every domain holds",
            "dup-groups".to_string(),
            ROOT.to_string(),
            false,
        ),
    ];

    for (case, domain, pattern, seed) in &rows {
        let created = create_domain(&router, domain, None).await;
        assert!(
            created.status == 200 || created.status == CONFLICT.0,
            "case {case:?}: CreateDomain must succeed or already be held, got {created:?}"
        );
        if *seed {
            let first = create_package_group(&router, domain, pattern, None, None).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first CreatePackageGroup must succeed, got {first:?}"
            );
        }

        let again = create_package_group(&router, domain, pattern, None, None).await;
        assert_refusal(case, &again, CONFLICT, &group_exists(domain, ACCOUNT, pattern));

        // The refusal is about stored state and changed nothing: the group it collided
        // with is still readable.
        let held = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            (held.status, held.group()["pattern"].as_str()),
            (200, Some(pattern.as_str())),
            "case {case:?}: the colliding group must still be readable, got {held:?}"
        );
    }

    // The same pattern in ANOTHER domain is not taken: the conflict is scoped to the
    // domain the sentence names.
    let elsewhere = create_domain(&router, "dup-groups-two", None).await;
    assert_eq!(
        elsewhere.status, 200,
        "CreateDomain for the second domain must succeed, got {elsewhere:?}"
    );
    let free = create_package_group(&router, "dup-groups-two", "/npm/left/*", None, None).await;
    assert_eq!(
        free.status, 200,
        "the same pattern in another domain must be free, got {free:?}"
    );

    // Pinning `domainOwner` to the account that owns the domain addresses the same group,
    // so the duplicate is still refused — with the same account in the sentence.
    let pinned = call(
        &router,
        "POST",
        &format!("/v1/package-group?domain=dup-groups&domain-owner={ACCOUNT}"),
        Some(json!({ "packageGroup": "/npm/left/*" })),
    )
    .await;
    assert_refusal(
        "the owner pinned to the account that owns the domain",
        &pinned,
        CONFLICT,
        &group_exists("dup-groups", ACCOUNT, "/npm/left/*"),
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the root group cannot be deleted, in one sentence that names nothing.
// ---------------------------------------------------------------------------

/// RULE 2: the whole sentence, which interpolates nothing — the root pattern in it is the
/// root group's own identifier, not a value read off the request.
const ROOT_NOT_DELETABLE: &str = "The root package group, '/*', cannot be deleted.";

#[tokio::test]
async fn deleting_the_root_package_group_states_one_fixed_sentence() {
    let router = emulator();

    // Rows differ along the domain name, including the modelled extremes, and along
    // whether the domain holds other groups at all.
    let longest_domain = format!("r{}t", "o".repeat(48));
    let rows: Vec<(&str, String, &[&str])> = vec![
        ("a two-character domain holding nothing else", "ab".to_string(), &[]),
        (
            "a domain holding sub-groups",
            "root-guard".to_string(),
            &["/npm/*", "/npm/scope/*"],
        ),
        ("a 50-character domain", longest_domain, &[]),
    ];

    for (case, domain, groups) in &rows {
        let created = create_domain(&router, domain, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateDomain must succeed, got {created:?}"
        );
        for pattern in *groups {
            let seeded = create_package_group(&router, domain, pattern, None, None).await;
            assert_eq!(
                seeded.status, 200,
                "case {case:?}: seeding {pattern:?} must succeed, got {seeded:?}"
            );
        }

        let refused = delete_package_group(&router, domain, ROOT).await;
        assert_refusal(case, &refused, INVALID, ROOT_NOT_DELETABLE);
        assert!(
            !refused.message().contains(domain.as_str()),
            "case {case:?}: the sentence names no domain, got {:?}",
            refused.message()
        );

        // The refusal removed nothing, and a group that is NOT the root still deletes —
        // so the sentence is about the root group, not about the operation.
        let root = describe_package_group(&router, domain, ROOT).await;
        assert_eq!(
            root.status, 200,
            "case {case:?}: the root group must still be held, got {root:?}"
        );
        for pattern in *groups {
            let deleted = delete_package_group(&router, domain, pattern).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: deleting {pattern:?} must succeed, got {deleted:?}"
            );
        }
    }
}

/// `DeletePackageGroup`: both members are `httpQuery`-bound
/// (`smithy_query(query="operation",operation="DeletePackageGroup")`).
async fn delete_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!(
            "/v1/package-group?domain={domain}&package-group={}",
            encode_query(pattern)
        ),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE 3: one sentence for every document that is not a policy.
// ---------------------------------------------------------------------------

/// RULE 3: the whole sentence, which names nothing about the submitted document.
const NOT_A_POLICY: &str = "Policy document isn't a valid policy document";

#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_in_one_sentence() {
    let router = emulator();

    let domain = "policy-sentences";
    let repository = "policy-repo";
    let created = create_domain(&router, domain, None).await;
    assert_eq!(created.status, 200, "CreateDomain must succeed, got {created:?}");
    let created = create_repository(&router, domain, repository, None, &[]).await;
    assert_eq!(
        created.status, 200,
        "CreateRepository must succeed, got {created:?}"
    );

    // Rows differ along HOW the document fails to be a policy: a parse that stops at the
    // first byte, one that stops in the middle, one that stops at the end, and documents
    // that parse as each other JSON type. Every row satisfies `PolicyDocument`'s modelled
    // `@length(1..7168)` and `@pattern(^[\P{C}\s]+$)`, so the handler is what refuses it.
    let documents = [
        "{",
        "not json at all",
        "{\"Version\"",
        r#"{"Version":"2012-10-17",}"#,
        r#"{"Statement":[{"Effect":"Allow"}"#,
        "[]",
        r#"["Version","2012-10-17"]"#,
        "\"a string\"",
        "42",
        "true",
        "null",
    ];

    for document in documents {
        for (family, answer) in [
            (
                "domain",
                put_domain_policy(&router, domain, document).await,
            ),
            (
                "repository",
                put_repository_policy(&router, domain, repository, document).await,
            ),
        ] {
            let case = format!("{family}: {document:?}");
            assert_refusal(&case, &answer, INVALID, NOT_A_POLICY);
            // The sentence carries nothing of the parser or the request: no member name,
            // no position, no JSON type.
            for leak in ["policyDocument", "line ", "column", "expected"] {
                assert!(
                    !answer.message().contains(leak),
                    "case {case:?}: the sentence must not carry {leak:?}, got {:?}",
                    answer.message()
                );
            }
        }
    }

    // ...and a JSON object IS a policy document, so both families still accept one: the
    // rows above are refused for what they are, not because every write is refused.
    for (family, answer) in [
        (
            "domain",
            put_domain_policy(&router, domain, r#"{"Version":"2012-10-17"}"#).await,
        ),
        (
            "repository",
            put_repository_policy(
                &router,
                domain,
                repository,
                r#"{"Version":"2012-10-17"}"#,
            )
            .await,
        ),
    ] {
        assert_eq!(
            answer.status, 200,
            "case {family:?}: a JSON policy object must be accepted, got {answer:?}"
        );
    }
}

/// `PutDomainPermissionsPolicy`: every member travels in the body.
async fn put_domain_policy(router: &MultiServiceRouter, domain: &str, document: &str) -> Answer {
    call(
        router,
        "PUT",
        "/v1/domain/permissions/policy",
        Some(json!({ "domain": domain, "policyDocument": document })),
    )
    .await
}

/// `PutRepositoryPermissionsPolicy`: the document travels in the body, the resource in the
/// query string.
async fn put_repository_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    document: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={repository}"),
        Some(json!({ "policyDocument": document })),
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE 4: an absent asset names the repository, the coordinate and the asset.
// ---------------------------------------------------------------------------

/// One version a row publishes, and the asset name the row then asks for.
#[derive(Debug)]
struct AssetCase {
    case: &'static str,
    repository: &'static str,
    namespace: &'static str,
    package: &'static str,
    version: &'static str,
    /// The asset the version is published with — the one it DOES hold.
    held: &'static str,
    /// The asset the row asks for, which the version does not hold.
    asked: &'static str,
}

impl AssetCase {
    /// RULE 4: the sentence the asset-absent read must state.
    ///
    /// `format` is `generic` for every row because `PublishPackageVersion` accepts only
    /// that format ("The only supported value is generic" —
    /// `smithy_query(query="doc",shape="PublishPackageVersionRequest",member="format")`),
    /// and a generic version's identity carries a namespace, so the entity renders the
    /// `namespace=` component here. The namespace-less rendering of the same entity is
    /// pinned by `tests/all/package_version_not_found_coordinate.rs`, which drives the one
    /// shared coordinate renderer.
    fn expected_message(&self) -> String {
        format!(
            "Asset not found in repo. RepositoryName = '{}', packageCoordinate = \
             'PackageVersionCoordinateEntity{{name={}, type=GENERIC, namespace={}, version={}, \
             displayName={}/{}/{}}}', assetName = '{}'.",
            self.repository,
            self.package,
            self.namespace,
            self.version,
            self.namespace,
            self.package,
            self.version,
            self.asked
        )
    }
}

/// The rows RULE 4 is asserted over: they differ along the repository, the namespace, the
/// package and version names, and along HOW the asked-for asset differs from the held one
/// (another name, a case difference, a prefix of it, a name carrying a space, a non-ASCII
/// name). `AssetName` is modelled `@length(1..255) @pattern(^\P{C}+$)`
/// (`smithy_query(query="operation",operation="GetPackageVersionAsset")`), so every asked
/// name below is one a request may carry.
const ASSET_CASES: [AssetCase; 6] = [
    AssetCase {
        case: "an unrelated asset name",
        repository: "assets-one",
        namespace: "sim-ns",
        package: "release.bundle",
        version: "1.0.0",
        held: "payload.bin",
        asked: "absent-asset.bin",
    },
    AssetCase {
        case: "the held name in another case",
        repository: "assets-one",
        namespace: "sim-ns",
        package: "release.bundle",
        version: "1.0.0",
        held: "payload.bin",
        asked: "PAYLOAD.BIN",
    },
    AssetCase {
        case: "a prefix of the held name",
        repository: "assets-one",
        namespace: "other-ns",
        package: "tool",
        version: "2.1.3-rc.4",
        held: "payload.bin",
        asked: "payload",
    },
    AssetCase {
        case: "a name carrying a space",
        repository: "assets-two",
        namespace: "sim-ns",
        package: "release.bundle",
        version: "1.0.0",
        held: "payload.bin",
        asked: "release notes.txt",
    },
    AssetCase {
        case: "a non-ASCII name",
        repository: "assets-two",
        namespace: "ns.with.dots",
        package: "paquete",
        version: "0.0.1",
        held: "carga.bin",
        asked: "añadido.bin",
    },
    AssetCase {
        case: "a path-like name",
        repository: "assets-two",
        namespace: "sim-ns",
        package: "nested",
        version: "9.9.9",
        held: "top.bin",
        asked: "dir-sub.bin",
    },
];

#[tokio::test]
async fn an_absent_asset_names_the_repository_the_coordinate_and_the_asset() {
    let router = emulator();

    let domain = "asset-sentences";
    let created = create_domain(&router, domain, None).await;
    assert_eq!(created.status, 200, "CreateDomain must succeed, got {created:?}");
    for repository in ["assets-one", "assets-two"] {
        let created = create_repository(&router, domain, repository, None, &[]).await;
        assert_eq!(
            created.status, 200,
            "CreateRepository {repository:?} must succeed, got {created:?}"
        );
    }

    for row in &ASSET_CASES {
        let mut request = Publish::new(row.package, row.version, row.held, binary_content(64, 3));
        request.domain = domain.to_string();
        request.repository = row.repository.to_string();
        request = request.namespace(row.namespace);
        let published = publish(&router, &request).await;
        assert!(
            published.status == 200 || published.status == CONFLICT.0,
            "case {:?}: the publish must land, got {published:?}",
            row.case
        );

        let refused = get_asset(&router, domain, row.repository, row, row.asked).await;
        assert_refusal(row.case, &refused, NOT_FOUND, &row.expected_message());

        // The version DOES hold the asset it was published with, so the refusal is about
        // the asset name and not about the coordinate.
        let held = get_asset(&router, domain, row.repository, row, row.held).await;
        assert_eq!(
            held.status, 200,
            "case {:?}: the held asset must still download, got {held:?}",
            row.case
        );
    }
}

/// `GetPackageVersionAsset` for `row`'s coordinate, asking for `asset`.
async fn get_asset(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    row: &AssetCase,
    asset: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/v1/package/version/asset?domain={domain}&repository={repository}&format=generic\
             &namespace={}&package={}&version={}&asset={}",
            encode_query(row.namespace),
            encode_query(row.package),
            encode_query(row.version),
            encode_query(asset)
        ),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE 5: two sentences for a `resourceArn` that addresses nothing.
// ---------------------------------------------------------------------------

/// RULE 5: the ARN parser's sentence for a value that is not written as an ARN — fewer than
/// the six colon-separated fields `arn:<partition>:<vendor>:<region>:<account>:<resource>`
/// has.
fn not_an_arn(value: &str) -> String {
    format!("Invalid ARN: {value} can't be parsed because it doesn't have enough separators.")
}

/// RULE 5: the sentence for a value that IS written as an ARN but is not one of this
/// service's.
fn not_this_services_arn(value: &str) -> String {
    format!("resourceArn {value} is not a CodeArtifact resource ARN.")
}

#[tokio::test]
async fn an_unaddressable_resource_arn_separates_bad_syntax_from_another_vendor() {
    let router = emulator();

    let domain = "arn-sentences";
    let created = create_domain(&router, domain, None).await;
    assert_eq!(created.status, 200, "CreateDomain must succeed, got {created:?}");

    // Rows differ along how many of an ARN's six fields the value carries (1, 2, 3, 4, 5
    // and 6) and, for the ones that carry all six, along WHY they are still not one of
    // this service's ARNs: another vendor, a vendor differing only in case, an empty
    // vendor, and a first field that is not `arn`. `Arn` is modelled
    // `@length(1..1011) @pattern(^\S+$)`
    // (`smithy_query(query="operation",operation="TagResource")`), so every value below
    // reaches the handler.
    let rows: Vec<(&str, String)> = vec![
        ("no separator at all", "arn-sentences".to_string()),
        ("two fields", "arn:aws".to_string()),
        ("three fields", "arn:aws:codeartifact".to_string()),
        (
            "four fields",
            "arn:aws:codeartifact:not-a-real-arn".to_string(),
        ),
        (
            "five fields, stopping before the resource",
            format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}"),
        ),
        ("an ARN of another vendor", "arn:aws:s3:::assets-bucket".to_string()),
        (
            "an ARN of another vendor mimicking the resource field",
            format!("arn:aws:codebuild:{REGION}:{ACCOUNT}:domain/{domain}"),
        ),
        (
            "a vendor differing only in case",
            format!("arn:aws:CodeArtifact:{REGION}:{ACCOUNT}:domain/{domain}"),
        ),
        (
            "an empty vendor",
            format!("arn:aws::{REGION}:{ACCOUNT}:domain/{domain}"),
        ),
        (
            "six fields whose first is not arn",
            format!("urn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}"),
        ),
    ];

    for (case, value) in &rows {
        // The rule is about ARN SYNTAX, so the expected sentence is decided by the value's
        // own field count and by nothing else.
        let expected = if value.split(':').take(6).count() == 6 {
            not_this_services_arn(value)
        } else {
            not_an_arn(value)
        };

        for (operation, answer) in [
            ("TagResource", tag(&router, value, &pairs(&[("probe", "1")])).await),
            ("ListTagsForResource", list_tags(&router, value).await),
            ("UntagResource", untag(&router, value, &["probe"]).await),
        ] {
            let case = format!("{operation}: {case}");
            assert_refusal(&case, &answer, INVALID, &expected);
        }
    }

    // A well-formed ARN of this service naming a held resource is still accepted by all
    // three, so the rows above are refused for the ARN they carry.
    let arn = format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}");
    let tagged = tag(&router, &arn, &pairs(&[("probe", "1")])).await;
    assert_eq!(
        tagged.status, 200,
        "tagging the held domain must succeed, got {tagged:?}"
    );
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"],
        json!([{ "key": "probe", "value": "1" }]),
        "the held domain must report the tag it was given, got {listed:?}"
    );
    let untagged = untag(&router, &arn, &["probe"]).await;
    assert_eq!(
        untagged.status, 200,
        "untagging the held domain must succeed, got {untagged:?}"
    );
}

/// `UntagResource`: the keys travel in the body, the ARN in the query string.
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/untag?resourceArn={}", encode_query(arn)),
        Some(json!({ "tagKeys": keys })),
    )
    .await
}
