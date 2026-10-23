// SPDX-License-Identifier: CC0-1.0

//! The ECR operations that resolve a NAMED resource before they answer, end to end over
//! the wire: the two per-image status reads, the referrer listing, and the three
//! registry-level resources ECR keys by prefix or principal.
//!
//! The rule under test is one rule: an operation that names a resource resolves that name
//! against the registry's stored state first, and answers the modelled not-found error
//! for the resource it could not resolve — `RepositoryNotFoundException` for a repository,
//! `ImageNotFoundException` for an image, `ExclusionNotFoundException` for a pull time
//! update exclusion, `PullThroughCacheRuleNotFoundException` for a pull through cache
//! rule and `TemplateNotFoundException` for a repository creation template — and otherwise
//! projects its answer from the record it resolved. Every case below is one row of that
//! rule, not a recorded request.

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator_with_engine, given_layer, ok, refused, REGISTRY_ID};

/// An account id that is NOT this emulator's registry.
const OTHER_REGISTRY_ID: &str = "123456789012";

/// The media type of an OCI image manifest — what the implementation reports for a
/// manifest that declares no `mediaType` of its own.
const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Create a repository, or panic naming the case.
async fn create_repository(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
}

/// Push `manifest` and answer the digest the registry stored it under.
async fn push(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    manifest: &str,
    tag: Option<&str>,
) -> String {
    let mut request = json!({"repositoryName": repository, "imageManifest": manifest});
    if let Some(tag) = tag {
        request["imageTag"] = json!(tag);
    }
    let body = ok(router, case, "PutImage", &request).await;
    body["image"]["imageId"]["imageDigest"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: PutImage reported no digest: {body:?}"))
        .to_string()
}

/// Seed one pull through cache rule into the registry's store.
///
/// `CreatePullThroughCacheRule` is the operation that creates one, and it refuses an
/// upstream URL it cannot resolve to a supported registry, so the update and validate
/// paths are exercised here against a record placed straight into the same store those
/// handlers read — which is also how a rule naming an upstream the create would refuse
/// is arranged.
fn seed_pull_through_cache_rule(
    engine: &engine::InMemoryEngine,
    prefix: &str,
    upstream_registry: Option<&str>,
    upstream_url: &str,
    credential_arn: Option<&str>,
    custom_role_arn: Option<&str>,
) {
    let state = ecr::state::service_state(engine).expect("the ecr store is reachable");
    state.lock().pull_through_cache_rules.insert(
        prefix.to_string(),
        ecr::ops::types::PullThroughCacheRule {
            ecr_repository_prefix: Some(prefix.to_string()),
            upstream_registry_url: Some(upstream_url.to_string()),
            created_at: Some(::aws_smithy_types::DateTime::from(SystemTime::now())),
            registry_id: Some(REGISTRY_ID.to_string()),
            credential_arn: credential_arn.map(ToString::to_string),
            custom_role_arn: custom_role_arn.map(ToString::to_string),
            upstream_repository_prefix: Some("ROOT".to_string()),
            upstream_registry: upstream_registry.map(ToString::to_string),
            updated_at: None,
        },
    );
}

/// Create one repository creation template through the API the registry exposes for it.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    prefix: &str,
    description: &str,
) {
    ok(
        router,
        case,
        "CreateRepositoryCreationTemplate",
        &json!({
            "prefix": prefix,
            "description": description,
            "imageTagMutability": "IMMUTABLE",
            "appliedFor": ["PULL_THROUGH_CACHE"],
        }),
    )
    .await;
}

/// Register one pull time update exclusion through the API the registry exposes for it.
async fn register_exclusion(router: &MultiServiceRouter, case: &str, principal_arn: &str) {
    ok(
        router,
        case,
        "RegisterPullTimeUpdateExclusion",
        &json!({"principalArn": principal_arn}),
    )
    .await;
}

/// The principals currently excluded, straight out of the store.
fn stored_exclusions(engine: &engine::InMemoryEngine) -> BTreeSet<String> {
    let state = ecr::state::service_state(engine).expect("the ecr store is reachable");
    let principals = state
        .lock()
        .pull_time_update_exclusions
        .keys()
        .cloned()
        .collect();
    principals
}

/// The prefixes of the templates currently stored.
fn stored_template_prefixes(engine: &engine::InMemoryEngine) -> BTreeSet<String> {
    let state = ecr::state::service_state(engine).expect("the ecr store is reachable");
    let prefixes = state.lock().creation_templates.keys().cloned().collect();
    prefixes
}

/// An image manifest a push can store: distinct `salt` gives it a distinct digest.
///
/// It references no blob, because a manifest cannot be pushed before every blob it
/// references is in the repository (`PutImage`'s `LayersNotFoundException`) and these
/// cases are about resolving a NAME rather than about blobs — the `layer_availability`
/// suite covers those.
fn image_manifest(salt: &str) -> String {
    json!({
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST_MEDIA_TYPE,
        "annotations": {"salt": salt},
        "layers": [],
    })
    .to_string()
}

/// The two per-image status reads resolve the repository, then the image, and project
/// their answer from the record they resolved.
///
/// Both operations model the same two not-found errors over the same two names, so the
/// table runs every row against both of them.
#[tokio::test]
async fn image_status_reads_resolve_the_repository_then_the_image() {
    const READS: [(&str, &str); 2] = [
        ("DescribeImageSigningStatus", "signingStatuses"),
        ("DescribeImageReplicationStatus", "replicationStatuses"),
    ];

    let (router, _engine) = emulator_with_engine();
    create_repository(&router, "setup", "status-reads").await;
    let digest = push(
        &router,
        "setup",
        "status-reads",
        &image_manifest("1a"),
        Some("v1"),
    )
    .await;
    let untagged = push(&router, "setup", "status-reads", &image_manifest("2b"), None).await;
    let absent_digest = format!("sha256:{:0>64}", "dead");

    // Every row differs in WHICH name cannot be resolved, or in how the identifier is
    // spelled — the dimensions the rule generalises over.
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "the repository was never created",
            json!({"repositoryName": "never-created-repo", "imageId": {"imageTag": "v1"}}),
            "RepositoryNotFoundException",
        ),
        (
            "a tag the repository does not carry",
            json!({"repositoryName": "status-reads", "imageId": {"imageTag": "absent"}}),
            "ImageNotFoundException",
        ),
        (
            "a digest the repository does not hold",
            json!({"repositoryName": "status-reads", "imageId": {"imageDigest": absent_digest}}),
            "ImageNotFoundException",
        ),
        (
            "a stored digest with a tag that points elsewhere",
            json!({
                "repositoryName": "status-reads",
                "imageId": {"imageDigest": untagged, "imageTag": "v1"},
            }),
            "ImageNotFoundException",
        ),
        (
            "an identifier that names neither a digest nor a tag",
            json!({"repositoryName": "status-reads", "imageId": {}}),
            "InvalidParameterException",
        ),
        (
            "another account's registry",
            json!({
                "repositoryName": "status-reads",
                "imageId": {"imageTag": "v1"},
                "registryId": OTHER_REGISTRY_ID,
            }),
            "InvalidParameterException",
        ),
    ];

    for (operation, statuses) in READS {
        for (case, request, expected) in &refusals {
            refused(&router, case, operation, request, expected).await;
        }

        // A resolved image is reported by the digest the registry stored, whichever way
        // the request addressed it, and the tag is reported only when it really points at
        // that image.
        let addressed: Vec<(&str, Value, &str, Option<&str>)> = vec![
            (
                "addressed by tag",
                json!({"repositoryName": "status-reads", "imageId": {"imageTag": "v1"}}),
                digest.as_str(),
                Some("v1"),
            ),
            (
                "addressed by digest",
                json!({
                    "repositoryName": "status-reads",
                    "imageId": {"imageDigest": digest.clone()},
                }),
                digest.as_str(),
                None,
            ),
            (
                "addressed by digest and tag",
                json!({
                    "repositoryName": "status-reads",
                    "imageId": {"imageDigest": digest.clone(), "imageTag": "v1"},
                }),
                digest.as_str(),
                Some("v1"),
            ),
            (
                "an untagged image addressed by digest",
                json!({
                    "repositoryName": "status-reads",
                    "imageId": {"imageDigest": untagged.clone()},
                }),
                untagged.as_str(),
                None,
            ),
        ];

        for (case, request, expected_digest, expected_tag) in &addressed {
            let body = ok(&router, case, operation, request).await;
            assert_eq!(
                body["repositoryName"].as_str(),
                Some("status-reads"),
                "case {case:?}: {operation} reported the wrong repository: {body:?}"
            );
            assert_eq!(
                body["imageId"]["imageDigest"].as_str(),
                Some(*expected_digest),
                "case {case:?}: {operation} reported the wrong digest: {body:?}"
            );
            assert_eq!(
                body["imageId"]["imageTag"].as_str(),
                *expected_tag,
                "case {case:?}: {operation} reported the wrong tag: {body:?}"
            );
            // This registry holds no signing record and no replication destination for
            // any image, so a resolved image has no status to report.
            assert_eq!(
                body[statuses].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: {operation} must report no {statuses} for a registry with \
                 none configured: {body:?}"
            );
        }
    }

    // `DescribeImageSigningStatus` also reports the registry the repository belongs to.
    let body = ok(
        &router,
        "the registry id of a resolved image",
        "DescribeImageSigningStatus",
        &json!({"repositoryName": "status-reads", "imageId": {"imageTag": "v1"}}),
    )
    .await;
    assert_eq!(
        body["registryId"].as_str(),
        Some(REGISTRY_ID),
        "the signing status read must report this emulator's registry: {body:?}"
    );
}

/// An artifact manifest that refers to `subject`, as an OCI artifact does.
///
/// A `config` descriptor names a blob, and a manifest naming a blob the repository does
/// not hold cannot be pushed, so the caller passes the digest of a blob it has already
/// uploaded (`config_digest`).
fn artifact_manifest(
    subject: &str,
    media_type: Option<&str>,
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    config_digest: &str,
    annotations: &[(&str, &str)],
) -> String {
    let mut manifest = json!({
        "schemaVersion": 2,
        "subject": {
            "mediaType": OCI_MANIFEST_MEDIA_TYPE,
            "digest": subject,
            "size": 11,
        },
        "layers": [],
    });
    if let Some(media_type) = media_type {
        manifest["mediaType"] = json!(media_type);
    }
    if let Some(artifact_type) = artifact_type {
        manifest["artifactType"] = json!(artifact_type);
    }
    if let Some(config_media_type) = config_media_type {
        manifest["config"] = json!({
            "mediaType": config_media_type,
            "digest": config_digest,
            "size": 2,
        });
    }
    if !annotations.is_empty() {
        manifest["annotations"] = Value::Object(
            annotations
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    manifest.to_string()
}

/// `ListImageReferrers` reports exactly the stored manifests whose own `subject` names the
/// requested digest, with every member read out of the stored manifest.
#[tokio::test]
async fn list_image_referrers_reports_the_artifacts_that_name_the_subject() {
    /// case, declared mediaType, declared artifactType, config mediaType, annotations,
    /// expected mediaType, expected artifactType.
    type Row<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        &'a [(&'a str, &'a str)],
        &'a str,
        Option<&'a str>,
    );

    let (router, _engine) = emulator_with_engine();
    create_repository(&router, "setup", "referrers").await;
    let subject = push(
        &router,
        "setup",
        "referrers",
        &image_manifest("5c"),
        Some("v1"),
    )
    .await;
    let other_subject = push(&router, "setup", "referrers", &image_manifest("6d"), None).await;

    // The rows differ along what the rule generalises over: which member declares the
    // artifact type, whether a media type is declared at all, and whether the artifact
    // carries annotations.
    let rows: Vec<Row<'_>> = vec![
        (
            "artifactType declared on the manifest",
            Some(OCI_MANIFEST_MEDIA_TYPE),
            Some("application/vnd.dev.cosign.simplesigning.v1+json"),
            Some("application/vnd.oci.empty.v1+json"),
            &[("org.opencontainers.image.created", "2026-01-01T00:00:00Z")],
            OCI_MANIFEST_MEDIA_TYPE,
            Some("application/vnd.dev.cosign.simplesigning.v1+json"),
        ),
        (
            "no artifactType, so the config media type is the artifact type",
            Some(OCI_MANIFEST_MEDIA_TYPE),
            None,
            Some("application/vnd.in-toto+json"),
            &[],
            OCI_MANIFEST_MEDIA_TYPE,
            Some("application/vnd.in-toto+json"),
        ),
        (
            "neither an artifactType nor a config",
            Some(OCI_MANIFEST_MEDIA_TYPE),
            None,
            None,
            &[("kind", "attestation")],
            OCI_MANIFEST_MEDIA_TYPE,
            None,
        ),
        (
            "a docker manifest media type",
            Some("application/vnd.docker.distribution.manifest.v2+json"),
            Some("application/vnd.example.sbom.v1+json"),
            None,
            &[],
            "application/vnd.docker.distribution.manifest.v2+json",
            Some("application/vnd.example.sbom.v1+json"),
        ),
        (
            "no media type declared at all",
            None,
            Some("application/vnd.example.report.v1+json"),
            None,
            &[],
            OCI_MANIFEST_MEDIA_TYPE,
            Some("application/vnd.example.report.v1+json"),
        ),
    ];

    // Every row is pushed as an untagged artifact of the same repository, beside two
    // manifests that must NOT be reported: one referring to another subject, and the
    // subject itself, which refers to nothing.
    // The one blob every artifact below names as its config: in the repository first,
    // because a manifest cannot be pushed before the blobs it references are.
    let config_digest = given_layer(&router, "setup", "referrers", b"artifact config").await;
    let mut expected: HashMap<String, Row<'_>> = HashMap::new();
    for row in &rows {
        let (case, media_type, artifact_type, config_media_type, annotations, _, _) = *row;
        let manifest = artifact_manifest(
            &subject,
            media_type,
            artifact_type,
            config_media_type,
            &config_digest,
            annotations,
        );
        let digest = push(&router, case, "referrers", &manifest, None).await;
        expected.insert(digest, *row);
    }
    let elsewhere = artifact_manifest(
        &other_subject,
        Some(OCI_MANIFEST_MEDIA_TYPE),
        Some("application/vnd.example.sbom.v1+json"),
        None,
        &config_digest,
        &[],
    );
    push(&router, "another subject", "referrers", &elsewhere, None).await;

    let body = ok(
        &router,
        "every referrer of the subject",
        "ListImageReferrers",
        &json!({"repositoryName": "referrers", "subjectId": {"imageDigest": subject}}),
    )
    .await;
    let referrers = body["referrers"]
        .as_array()
        .unwrap_or_else(|| panic!("ListImageReferrers reported no referrers list: {body:?}"));
    assert_eq!(
        referrers.len(),
        expected.len(),
        "expected {} referrers of the subject, got {:?}",
        expected.len(),
        body
    );

    for referrer in referrers {
        let digest = referrer["digest"].as_str().unwrap_or_default();
        let Some((case, _, _, _, annotations, expected_media_type, expected_artifact_type)) =
            expected.get(digest)
        else {
            panic!("ListImageReferrers reported an artifact of another subject: {referrer:?}");
        };
        assert_eq!(
            referrer["mediaType"].as_str(),
            Some(*expected_media_type),
            "case {case:?}: wrong mediaType, got {referrer:?}"
        );
        assert_eq!(
            referrer["artifactType"].as_str(),
            *expected_artifact_type,
            "case {case:?}: wrong artifactType, got {referrer:?}"
        );
        assert_eq!(
            referrer["artifactStatus"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: every stored artifact is ACTIVE, got {referrer:?}"
        );
        assert!(
            referrer["size"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: the artifact size must be the stored manifest's, got {referrer:?}"
        );
        let reported: HashMap<String, String> = referrer["annotations"]
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let declared: HashMap<String, String> = annotations
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, declared,
            "case {case:?}: the annotations must be the manifest's own, got {referrer:?}"
        );
    }

    // A subject the repository does not hold, or that nothing refers to, is an empty
    // list rather than a refusal: this operation models no image-not-found error.
    for (case, subject_digest, expected_count) in [
        ("a subject with one artifact of its own", other_subject.as_str(), 1),
        ("a digest the repository does not hold", "sha256:beef", 0),
        ("the subject of no artifact at all", "sha256:feed", 0),
    ] {
        let body = ok(
            &router,
            case,
            "ListImageReferrers",
            &json!({
                "repositoryName": "referrers",
                "subjectId": {"imageDigest": subject_digest},
            }),
        )
        .await;
        let count = body["referrers"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, expected_count,
            "case {case:?}: expected {expected_count} referrers, got {body:?}"
        );
    }

    // An unknown repository is refused before anything is listed.
    refused(
        &router,
        "the repository was never created",
        "ListImageReferrers",
        &json!({
            "repositoryName": "never-created-repo",
            "subjectId": {"imageDigest": subject},
        }),
        "RepositoryNotFoundException",
    )
    .await;
}

/// The `filter` and the page size select from the same computed referrer set.
#[tokio::test]
async fn list_image_referrers_filters_and_paginates_the_computed_set() {
    let (router, _engine) = emulator_with_engine();
    create_repository(&router, "setup", "referrer-filters").await;
    let subject = push(
        &router,
        "setup",
        "referrer-filters",
        &image_manifest("7e"),
        Some("v1"),
    )
    .await;

    let artifact_types = [
        "application/vnd.dev.cosign.simplesigning.v1+json",
        "application/vnd.in-toto+json",
        "application/vnd.example.sbom.v1+json",
    ];
    for artifact_type in artifact_types {
        let manifest = artifact_manifest(
            &subject,
            Some(OCI_MANIFEST_MEDIA_TYPE),
            Some(artifact_type),
            None,
            "",
            &[("type", artifact_type)],
        );
        push(&router, artifact_type, "referrer-filters", &manifest, None).await;
    }

    // `artifactTypes` keeps the artifacts whose own type is named, for any subset of the
    // types stored.
    let subsets: Vec<(&str, Vec<&str>, usize)> = vec![
        ("no filter at all", vec![], 3),
        ("one type", vec![artifact_types[1]], 1),
        ("two types", vec![artifact_types[0], artifact_types[2]], 2),
        ("every type", artifact_types.to_vec(), 3),
        ("a type nothing declares", vec!["application/vnd.absent"], 0),
    ];
    for (case, types, expected) in &subsets {
        let mut request = json!({
            "repositoryName": "referrer-filters",
            "subjectId": {"imageDigest": subject},
        });
        if !types.is_empty() {
            request["filter"] = json!({"artifactTypes": types});
        }
        let body = ok(&router, case, "ListImageReferrers", &request).await;
        assert_eq!(
            body["referrers"].as_array().map_or(0, Vec::len),
            *expected,
            "case {case:?}: expected {expected} referrers, got {body:?}"
        );
    }

    // `artifactStatus`: every stored artifact is ACTIVE, so ACTIVE and ANY report them
    // all and the two non-standard storage classes report none.
    for (case, status, expected) in [
        ("ACTIVE", "ACTIVE", 3),
        ("ANY", "ANY", 3),
        ("ARCHIVED", "ARCHIVED", 0),
        ("ACTIVATING", "ACTIVATING", 0),
    ] {
        let body = ok(
            &router,
            case,
            "ListImageReferrers",
            &json!({
                "repositoryName": "referrer-filters",
                "subjectId": {"imageDigest": subject},
                "filter": {"artifactStatus": status},
            }),
        )
        .await;
        assert_eq!(
            body["referrers"].as_array().map_or(0, Vec::len),
            expected,
            "case {case:?}: expected {expected} referrers for artifactStatus {status}, got \
             {body:?}"
        );
    }

    // Paging reports every referrer exactly once, for any page size.
    for page_size in [1_i64, 2, 3, 50] {
        let case = format!("maxResults {page_size}");
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({
                "repositoryName": "referrer-filters",
                "subjectId": {"imageDigest": subject},
                "maxResults": page_size,
            });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let body = ok(&router, &case, "ListImageReferrers", &request).await;
            let page = body["referrers"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case:?}: a page must not exceed maxResults, got {body:?}"
            );
            for referrer in &page {
                let digest = referrer["digest"].as_str().unwrap_or_default().to_string();
                assert!(
                    seen.insert(digest.clone()),
                    "case {case:?}: {digest} was reported twice"
                );
            }
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            3,
            "case {case:?}: paging must report every referrer exactly once, saw {seen:?}"
        );
    }
}

/// Deregistering a pull time update exclusion removes the one record the registry holds
/// for that principal, and refuses a principal that carries none.
#[tokio::test]
async fn deregister_pull_time_update_exclusion_removes_one_registered_principal() {
    // The rows differ in the principal form the modelled `PrincipalArn` pattern allows:
    // a role, a user, and a role with a path.
    const PRINCIPALS: [(&str, &str); 3] = [
        (
            "a role",
            "arn:aws:iam::000000000000:role/pull-time-exclusion",
        ),
        ("a user", "arn:aws:iam::000000000000:user/release-bot"),
        (
            "a role with a path",
            "arn:aws:iam::000000000000:role/team/deploy-role",
        ),
    ];

    let (router, engine) = emulator_with_engine();
    for (case, principal) in PRINCIPALS {
        register_exclusion(&router, case, principal).await;
    }

    let mut remaining: BTreeSet<String> = PRINCIPALS
        .iter()
        .map(|(_, principal)| (*principal).to_string())
        .collect();

    for (case, principal) in PRINCIPALS {
        let body = ok(
            &router,
            case,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": principal}),
        )
        .await;
        assert_eq!(
            body["principalArn"].as_str(),
            Some(principal),
            "case {case:?}: the deregister must report the principal it removed: {body:?}"
        );

        // Exactly that one record is gone, and every other principal still carries its
        // exclusion.
        remaining.remove(principal);
        assert_eq!(
            stored_exclusions(&engine),
            remaining,
            "case {case:?}: the deregister must remove only {principal}"
        );

        // The second deregister of the same principal is refused: "already removed" is
        // not a silent success.
        refused(
            &router,
            case,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": principal}),
            "ExclusionNotFoundException",
        )
        .await;
    }

    // And a principal that was never registered is refused the same way.
    refused(
        &router,
        "never registered",
        "DeregisterPullTimeUpdateExclusion",
        &json!({"principalArn": "arn:aws:iam::000000000000:role/never-registered"}),
        "ExclusionNotFoundException",
    )
    .await;
}

/// One row of the validation table: the case, the rule's `upstreamRegistry`, its
/// `upstreamRegistryUrl`, the credential it stores, and whether the stored configuration
/// is missing a credential the upstream requires.
type ValidationRow<'a> = (&'a str, Option<&'a str>, &'a str, Option<&'a str>, bool);

/// Updating and validating a pull through cache rule resolve the prefix against the
/// stored rules, and the update is visible to the next read of the same rule.
#[tokio::test]
async fn pull_through_cache_rule_update_and_validate_resolve_the_prefix() {
    const SECRET: &str =
        "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/docker-hub";
    const OTHER_SECRET: &str =
        "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/other";
    /// The credential the updated rule is created with.
    const INITIAL_SECRET: &str =
        "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/initial";
    const ROLE: &str = "arn:aws:iam::000000000000:role/ecr-pull-through-cache";

    let (router, engine) = emulator_with_engine();

    // Neither operation creates a rule, so a prefix no rule answers to is refused by
    // both — including a prefix whose rule was never created and the reserved `ROOT`.
    for prefix in ["docker-hub", "ROOT", "nested/prefix"] {
        for operation in [
            "UpdatePullThroughCacheRule",
            "ValidatePullThroughCacheRule",
        ] {
            refused(
                &router,
                prefix,
                operation,
                &json!({"ecrRepositoryPrefix": prefix}),
                "PullThroughCacheRuleNotFoundException",
            )
            .await;
        }
    }

    // The update replaces only the credentials the request carries, and keeps the ones it
    // omits: the rows differ in which of the two credential members is sent.
    let updates: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "a credentialArn alone",
            json!({"credentialArn": SECRET}),
            Some(SECRET),
            None,
        ),
        (
            "a customRoleArn alone",
            json!({"customRoleArn": ROLE}),
            Some(SECRET),
            Some(ROLE),
        ),
        (
            "both credentials at once",
            json!({"credentialArn": OTHER_SECRET, "customRoleArn": ROLE}),
            Some(OTHER_SECRET),
            Some(ROLE),
        ),
        ("neither credential", json!({}), Some(OTHER_SECRET), Some(ROLE)),
    ];

    seed_pull_through_cache_rule(
        &engine,
        "docker-hub",
        Some("docker-hub"),
        "registry-1.docker.io",
        // Created WITH a credential: credentials are updatable only on a rule that was
        // created authenticated, so an unauthenticated rule has none of these rows'
        // updates to apply.
        Some(INITIAL_SECRET),
        None,
    );
    for (case, patch, expected_credential, expected_role) in &updates {
        let mut request = json!({"ecrRepositoryPrefix": "docker-hub"});
        for (member, value) in patch.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let body = ok(&router, case, "UpdatePullThroughCacheRule", &request).await;
        assert_eq!(
            body["ecrRepositoryPrefix"].as_str(),
            Some("docker-hub"),
            "case {case:?}: the update must report the rule it changed: {body:?}"
        );
        assert_eq!(
            body["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the update must report this registry: {body:?}"
        );
        assert_eq!(
            body["credentialArn"].as_str(),
            *expected_credential,
            "case {case:?}: wrong credentialArn after the update: {body:?}"
        );
        assert_eq!(
            body["customRoleArn"].as_str(),
            *expected_role,
            "case {case:?}: wrong customRoleArn after the update: {body:?}"
        );
        assert_eq!(
            body["upstreamRepositoryPrefix"].as_str(),
            Some("ROOT"),
            "case {case:?}: the upstream prefix is the stored one: {body:?}"
        );
        assert!(
            body["updatedAt"].is_number(),
            "case {case:?}: an update records when it happened: {body:?}"
        );

        // Read the rule back through the other implemented reader of the same record:
        // what the update stored is what the next call reports.
        let validated = ok(
            &router,
            case,
            "ValidatePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": "docker-hub"}),
        )
        .await;
        assert_eq!(
            validated["credentialArn"].as_str(),
            *expected_credential,
            "case {case:?}: the validate must report the stored credentialArn: {validated:?}"
        );
        assert_eq!(
            validated["customRoleArn"].as_str(),
            *expected_role,
            "case {case:?}: the validate must report the stored customRoleArn: {validated:?}"
        );
        assert_eq!(
            validated["upstreamRegistryUrl"].as_str(),
            Some("registry-1.docker.io"),
            "case {case:?}: the validate must report the stored upstream: {validated:?}"
        );
    }

    // Validation never claims a successful upstream round trip, because none can run
    // here; what it does distinguish is a rule whose stored configuration cannot even
    // authenticate. The rows span the upstream registries the model's `UpstreamRegistry`
    // enum carries, including the two the ECR user guide documents as anonymous, and a
    // rule that names no upstream registry at all.
    let validations: Vec<ValidationRow<'_>> = vec![
        (
            "docker-hub without a credential",
            Some("docker-hub"),
            "registry-1.docker.io",
            None,
            true,
        ),
        (
            "quay without a credential",
            Some("quay"),
            "quay.io",
            None,
            true,
        ),
        (
            "chainguard with a credential",
            Some("chainguard"),
            "cgr.dev",
            Some(SECRET),
            false,
        ),
        (
            "ecr-public needs no credential",
            Some("ecr-public"),
            "public.ecr.aws",
            None,
            false,
        ),
        (
            "k8s needs no credential",
            Some("k8s"),
            "registry.k8s.io",
            None,
            false,
        ),
        (
            "no upstream registry named, an anonymous host",
            None,
            "https://public.ecr.aws",
            None,
            false,
        ),
        (
            "no upstream registry named, an anonymous host with a port and odd case",
            None,
            "https://PUBLIC.ECR.AWS:443/v2/",
            None,
            false,
        ),
        (
            "no upstream registry named, an authenticating host",
            None,
            "https://ghcr.io",
            None,
            true,
        ),
    ];

    for (case, upstream_registry, url, credential, expect_missing_credential) in &validations {
        let prefix = "validated";
        seed_pull_through_cache_rule(&engine, prefix, *upstream_registry, url, *credential, None);
        let body = ok(
            &router,
            case,
            "ValidatePullThroughCacheRule",
            &json!({"ecrRepositoryPrefix": prefix}),
        )
        .await;
        assert_eq!(
            body["isValid"].as_bool(),
            Some(false),
            "case {case:?}: isValid is only true of an upstream round trip that ran: {body:?}"
        );
        let failure = body["failure"].as_str().unwrap_or_default();
        assert!(
            !failure.is_empty(),
            "case {case:?}: a validation that did not succeed must say why: {body:?}"
        );
        assert_eq!(
            failure.contains("requires authentication"),
            *expect_missing_credential,
            "case {case:?}: wrong failure reason {failure:?}"
        );
        assert_eq!(
            body["upstreamRegistryUrl"].as_str(),
            Some(*url),
            "case {case:?}: the validate reports the stored upstream URL: {body:?}"
        );
    }
}

/// Deleting a repository creation template removes the one template the registry holds
/// under that prefix and answers with it; a prefix carrying none is refused.
#[tokio::test]
async fn delete_repository_creation_template_removes_one_template() {
    // The rows differ in the prefix form the modelled `Prefix` pattern allows: a plain
    // namespace, a nested one, a trailing slash, a single character, and the reserved
    // `ROOT`.
    const PREFIXES: [(&str, &str); 5] = [
        ("a plain prefix", "team-images"),
        ("a nested prefix", "team/images"),
        ("a trailing slash", "team-images-2/"),
        ("a single character", "x"),
        ("the reserved ROOT prefix", "ROOT"),
    ];

    let (router, engine) = emulator_with_engine();
    for (case, prefix) in PREFIXES {
        create_template(&router, case, prefix, case).await;
    }
    // The stored key is the prefix without the single slash ECR assumes at its end
    // (`Prefix`: "There is always an assumed / applied to the end of the prefix"), which
    // is how the create stored it.
    let stored_key = |prefix: &str| prefix.strip_suffix('/').unwrap_or(prefix).to_string();
    let mut remaining: BTreeSet<String> = PREFIXES
        .iter()
        .map(|(_, prefix)| stored_key(prefix))
        .collect();

    for (case, prefix) in PREFIXES {
        let body = ok(
            &router,
            case,
            "DeleteRepositoryCreationTemplate",
            &json!({"prefix": prefix}),
        )
        .await;
        assert_eq!(
            body["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the delete must report this registry: {body:?}"
        );
        let template = &body["repositoryCreationTemplate"];
        assert_eq!(
            template["prefix"].as_str(),
            Some(stored_key(prefix).as_str()),
            "case {case:?}: the delete must answer with the template it removed: {body:?}"
        );
        assert_eq!(
            template["description"].as_str(),
            Some(case),
            "case {case:?}: the deleted template carries the stored description: {body:?}"
        );
        assert_eq!(
            template["imageTagMutability"].as_str(),
            Some("IMMUTABLE"),
            "case {case:?}: the deleted template carries the stored settings: {body:?}"
        );

        // Exactly that template is gone; the others are untouched.
        remaining.remove(&stored_key(prefix));
        assert_eq!(
            stored_template_prefixes(&engine),
            remaining,
            "case {case:?}: the delete must remove only the template for {prefix}"
        );

        // A re-delete of the same prefix is refused, which is what makes the
        // already-deleted state observable.
        refused(
            &router,
            case,
            "DeleteRepositoryCreationTemplate",
            &json!({"prefix": prefix}),
            "TemplateNotFoundException",
        )
        .await;
    }

    // The assumed trailing slash is not part of a prefix's identity, so a template
    // created with one is deleted by the prefix written without it, and the other way
    // round.
    for (case, created_as, deleted_as) in [
        ("created with a slash, deleted without", "slashed/", "slashed"),
        ("created without a slash, deleted with", "plain", "plain/"),
    ] {
        create_template(&router, case, created_as, case).await;
        let body = ok(
            &router,
            case,
            "DeleteRepositoryCreationTemplate",
            &json!({"prefix": deleted_as}),
        )
        .await;
        assert_eq!(
            body["repositoryCreationTemplate"]["description"].as_str(),
            Some(case),
            "case {case:?}: {deleted_as} and {created_as} name one template: {body:?}"
        );
    }

    // And a prefix no template was ever created for is refused the same way.
    refused(
        &router,
        "never created",
        "DeleteRepositoryCreationTemplate",
        &json!({"prefix": "never-created"}),
        "TemplateNotFoundException",
    )
    .await;
}
