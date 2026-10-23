// SPDX-License-Identifier: CC0-1.0

//! The preconditions ECR checks BEFORE it changes or reports state, end to end over the
//! wire.
//!
//! One rule, in four places: a request whose precondition the registry's own stored state
//! does not satisfy is refused with the modelled client error for that precondition, and
//! nothing is stored — never answered 200 as though it had been legal.
//!
//! * `PutImage` resolves every blob the manifest references against the repository's
//!   layers and refuses with `LayersNotFoundException` while any is absent.
//! * `DescribeImages` resolves every `imageIds` entry against the repository's images and
//!   refuses with `ImageNotFoundException` when one names none — including after the image
//!   has been deleted.
//! * `UpdatePullThroughCacheRule` refuses with `InvalidParameterException` when the rule it
//!   names was created without credentials, because that rule has none to update.
//! * `PutReplicationConfiguration` refuses with `InvalidParameterException` when a
//!   destination names a Region that does not exist.
//!
//! Every case goes through the router via [`crate::wire`], so it exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, given_layer, layer_digest, ok, refused, REGION};

/// A `credentialArn` of the form the model constrains `CredentialArn` to
/// (`^arn:aws(-\w+)*:secretsmanager:[a-zA-Z0-9-:]+:secret:ecr\-pullthroughcache\/[a-zA-Z0-9\/_+=.@-]+$`,
/// length 50..612).
const SECRET_ARN: &str =
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/docker-hub";

/// A second one, so an update can be seen to replace the stored credential.
const OTHER_SECRET_ARN: &str =
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:ecr-pullthroughcache/other";

/// A `customRoleArn`, the other credential member this operation takes.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ecr-pull-through-cache";

/// The refusal AWS answers a credential update on an unauthenticated rule with: a fixed
/// template, carrying nothing of the request.
const UNAUTHENTICATED_RULE_MESSAGE: &str =
    "Invalid input: Please create an authenticated pull through cache rule to update the \
     credentials";

/// Create the repository `name`.
async fn given_repository(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
}

/// A `PutImage` request body.
fn push_request(repository: &str, manifest: &str, tag: Option<&str>) -> Value {
    let mut request = json!({"repositoryName": repository, "imageManifest": manifest});
    if let Some(tag) = tag {
        request["imageTag"] = json!(tag);
    }
    request
}

/// A Docker image manifest V2 schema 2 over `config` and `layers`, as a push sends it.
fn docker_manifest(config: &str, layers: &[&str]) -> String {
    json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "config": {"mediaType": "application/vnd.docker.container.image.v1+json", "digest": config},
        "layers": layers
            .iter()
            .map(|digest| json!({
                "mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip",
                "digest": digest,
            }))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// An OCI image manifest over `config` and `layers`.
fn oci_manifest(config: &str, layers: &[&str]) -> String {
    json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config},
        "layers": layers
            .iter()
            .map(|digest| json!({
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": digest,
            }))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// A Docker image manifest V2 schema 1, which names its blobs as `fsLayers[].blobSum`.
fn schema1_manifest(layers: &[&str]) -> String {
    json!({
        "schemaVersion": 1,
        "name": "app",
        "tag": "v1",
        "fsLayers": layers
            .iter()
            .map(|digest| json!({"blobSum": digest}))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// A push is refused while any blob its manifest references is absent from the
/// repository, and accepted once every one of them has been uploaded.
///
/// The rows differ along what the rule generalises over: which manifest format declares
/// the blobs, how many blobs the manifest references, how many of them are missing,
/// whether the missing one is the configuration blob or a filesystem layer, whether the
/// same missing digest is referenced twice, and whether the document references any blob
/// at all.
#[tokio::test]
async fn a_push_is_refused_while_any_blob_its_manifest_references_is_absent() {
    /// case, the blob contents already uploaded, the blob contents still absent, and the
    /// manifest built over both — `present` first, then `absent`, in the order the
    /// manifest references them.
    type Row = (
        &'static str,
        Vec<&'static str>,
        Vec<&'static str>,
        fn(&[String], &[String]) -> String,
    );

    let rows: Vec<Row> = vec![
        (
            "a docker manifest whose config and both layers were never uploaded",
            vec![],
            vec!["push-a-config", "push-a-layer-1", "push-a-layer-2"],
            |_present, absent| {
                docker_manifest(&absent[0], &[absent[1].as_str(), absent[2].as_str()])
            },
        ),
        (
            "a docker manifest with its config uploaded and one layer missing",
            vec!["push-b-config", "push-b-layer-1"],
            vec!["push-b-layer-2"],
            |present, absent| {
                docker_manifest(&present[0], &[present[1].as_str(), absent[0].as_str()])
            },
        ),
        (
            "an OCI manifest whose layer is uploaded but whose config is not",
            vec!["push-c-layer"],
            vec!["push-c-config"],
            |present, absent| oci_manifest(&absent[0], &[present[0].as_str()]),
        ),
        (
            "a schema 1 manifest with one of two fsLayers uploaded",
            vec!["push-d-layer-1"],
            vec!["push-d-layer-2"],
            |present, absent| schema1_manifest(&[present[0].as_str(), absent[0].as_str()]),
        ),
        (
            "a manifest referencing the same absent blob twice",
            vec![],
            vec!["push-e-layer"],
            |_present, absent| schema1_manifest(&[absent[0].as_str(), absent[0].as_str()]),
        ),
    ];

    let router = emulator();
    for (index, (case, present, absent, build)) in rows.iter().enumerate() {
        let repository = format!("push-preconditions-{index}");
        given_repository(&router, case, &repository).await;

        let present_digests: Vec<String> = {
            let mut digests = Vec::new();
            for content in present {
                digests.push(given_layer(&router, case, &repository, content.as_bytes()).await);
            }
            digests
        };
        let absent_digests: Vec<String> = absent
            .iter()
            .map(|content| layer_digest(content.as_bytes()))
            .collect();
        let manifest = build(&present_digests, &absent_digests);

        let refusal = refused(
            &router,
            case,
            "PutImage",
            &push_request(&repository, &manifest, Some("v1")),
            "LayersNotFoundException",
        )
        .await;
        let message = refusal["message"].as_str().unwrap_or_default();
        for digest in &absent_digests {
            assert!(
                message.contains(digest.as_str()),
                "case {case:?}: the refusal must name the missing digest {digest}, got \
                 {message:?}"
            );
        }
        for digest in &present_digests {
            assert!(
                !message.contains(digest.as_str()),
                "case {case:?}: the refusal must not name the uploaded digest {digest}, got \
                 {message:?}"
            );
        }
        // A digest referenced twice is one missing blob, named once.
        for digest in &absent_digests {
            assert_eq!(
                message.matches(digest.as_str()).count(),
                1,
                "case {case:?}: expected {digest} named once, got {message:?}"
            );
        }

        // The refused push stored nothing: the repository still holds no image.
        let described = ok(
            &router,
            case,
            "DescribeImages",
            &json!({"repositoryName": repository}),
        )
        .await;
        assert_eq!(
            described["imageDetails"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused push must store no image, got {described:?}"
        );

        // Upload what was missing, and the very same push is accepted — and the image
        // reads back through the implemented reader of the same record.
        for content in absent {
            given_layer(&router, case, &repository, content.as_bytes()).await;
        }
        let pushed = ok(
            &router,
            case,
            "PutImage",
            &push_request(&repository, &manifest, Some("v1")),
        )
        .await;
        let digest = pushed["image"]["imageId"]["imageDigest"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            digest,
            layer_digest(manifest.as_bytes()),
            "case {case:?}: the image is addressed by the digest of its manifest, got \
             {pushed:?}"
        );
        let described = ok(
            &router,
            case,
            "DescribeImages",
            &json!({"repositoryName": repository, "imageIds": [{"imageDigest": digest}]}),
        )
        .await;
        assert_eq!(
            described["imageDetails"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the accepted push must be readable back, got {described:?}"
        );
    }
}

/// A manifest that references no blob at all has no blob to be missing, so it is stored
/// as it stands — whatever else the document carries.
///
/// The rows differ along what makes the document reference nothing: an empty layer list,
/// an image index (whose `manifests` entries are other images rather than this one's
/// blobs), a descriptor carrying no digest, and a manifest that is not JSON at all.
#[tokio::test]
async fn a_manifest_that_references_no_blob_is_stored_as_it_stands() {
    let index = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": layer_digest(b"a child manifest nothing uploaded"),
            "size": 7143,
        }],
    })
    .to_string();

    let rows: Vec<(&str, String)> = vec![
        (
            "an OCI manifest with an empty layer list and no config",
            json!({"schemaVersion": 2, "layers": []}).to_string(),
        ),
        ("an image index referencing another manifest", index),
        (
            "a config descriptor carrying no digest",
            json!({"schemaVersion": 2, "config": {"size": 7}}).to_string(),
        ),
        (
            "a manifest that is not JSON at all",
            "not a manifest".to_string(),
        ),
    ];

    let router = emulator();
    given_repository(&router, "setup", "blobless-manifests").await;
    for (index, (case, manifest)) in rows.iter().enumerate() {
        let pushed = ok(
            &router,
            case,
            "PutImage",
            &push_request("blobless-manifests", manifest, Some(&format!("v{index}"))),
        )
        .await;
        assert_eq!(
            pushed["image"]["imageManifest"].as_str(),
            Some(manifest.as_str()),
            "case {case:?}: the stored manifest is the one pushed, got {pushed:?}"
        );
    }
}

/// A completed layer upload is what puts a blob in a repository, and the registry reports
/// the blob under the digest it computed over the bytes it assembled.
///
/// The rows differ along the layer's size, which is what the rule generalises over: the
/// digest is of the content, not of the request.
#[tokio::test]
async fn a_completed_upload_registers_the_blob_it_assembled() {
    let router = emulator();
    given_repository(&router, "setup", "completed-uploads").await;

    // (case, the layer content)
    let rows: Vec<(&str, Vec<u8>)> = vec![
        ("a one-byte layer", vec![7]),
        ("a small text layer", b"a layer of text".to_vec()),
        (
            "a layer spanning the whole byte range",
            (0..=255_u8).collect(),
        ),
        ("a layer of tens of kilobytes", vec![0xab; 40_960]),
    ];

    for (case, content) in &rows {
        let digest = given_layer(&router, case, "completed-uploads", content).await;
        assert_eq!(
            digest,
            layer_digest(content),
            "case {case:?}: the blob is registered under the digest of its content"
        );
        let checked = ok(
            &router,
            case,
            "BatchCheckLayerAvailability",
            &json!({"repositoryName": "completed-uploads", "layerDigests": [digest]}),
        )
        .await;
        assert_eq!(
            checked["layers"][0]["layerAvailability"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: the completed upload's blob is available, got {checked:?}"
        );
        assert_eq!(
            checked["layers"][0]["layerSize"].as_u64(),
            Some(content.len() as u64),
            "case {case:?}: the reported size is the registry's own byte count, got \
             {checked:?}"
        );
    }
}

/// `DescribeImages` refuses an `imageIds` entry that names no stored image, including the
/// identifier of an image that has been deleted — and reports the repository's images for
/// a request naming none.
///
/// The rows differ along what the entry carries and why it resolves to nothing: an absent
/// tag, an absent digest, a digest and tag that each name something but not the same
/// image, a malformed digest (a bad parameter rather than an absent image), an entry
/// naming neither part, and an identifier that resolved until the image was deleted.
#[tokio::test]
async fn describe_images_refuses_an_identifier_that_names_no_stored_image() {
    let router = emulator();
    given_repository(&router, "setup", "image-probes").await;
    let first = json!({"schemaVersion": 2, "layers": [], "n": 1}).to_string();
    let second = json!({"schemaVersion": 2, "layers": [], "n": 2}).to_string();
    let kept = ok(
        &router,
        "setup",
        "PutImage",
        &push_request("image-probes", &first, Some("v1")),
    )
    .await;
    let kept_digest = kept["image"]["imageId"]["imageDigest"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let doomed = ok(
        &router,
        "setup",
        "PutImage",
        &push_request("image-probes", &second, Some("doomed")),
    )
    .await;
    let doomed_digest = doomed["image"]["imageId"]["imageDigest"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // An identifier that resolves is answered, before anything is deleted.
    let described = ok(
        &router,
        "the stored image",
        "DescribeImages",
        &json!({
            "repositoryName": "image-probes",
            "imageIds": [{"imageDigest": doomed_digest.clone()}],
        }),
    )
    .await;
    assert_eq!(
        described["imageDetails"][0]["imageDigest"].as_str(),
        Some(doomed_digest.as_str()),
        "the identifier of a stored image is answered from the stored record, got \
         {described:?}"
    );

    // Deleting it by digest takes the image and its tag with it.
    let deleted = ok(
        &router,
        "the delete",
        "BatchDeleteImage",
        &json!({
            "repositoryName": "image-probes",
            "imageIds": [{"imageDigest": doomed_digest.clone()}],
        }),
    )
    .await;
    assert_eq!(
        deleted["imageIds"][0]["imageDigest"].as_str(),
        Some(doomed_digest.as_str()),
        "the delete must report the image it removed, got {deleted:?}"
    );
    assert_eq!(
        deleted["failures"].as_array().map(Vec::len),
        Some(0),
        "deleting a stored image fails nothing, got {deleted:?}"
    );

    // (case, the imageIds entry, the error code the refusal carries)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "the digest of an image that was deleted",
            json!({"imageDigest": doomed_digest.clone()}),
            "ImageNotFoundException",
        ),
        (
            "the tag of an image that was deleted",
            json!({"imageTag": "doomed"}),
            "ImageNotFoundException",
        ),
        (
            "a digest nothing was ever pushed under",
            json!({"imageDigest": layer_digest(b"never pushed")}),
            "ImageNotFoundException",
        ),
        (
            "a tag the repository does not carry",
            json!({"imageTag": "no-such-tag"}),
            "ImageNotFoundException",
        ),
        (
            "a stored digest beside a tag that points elsewhere",
            json!({"imageDigest": kept_digest.clone(), "imageTag": "doomed"}),
            "ImageNotFoundException",
        ),
        (
            "a digest that is not a content digest at all",
            json!({"imageDigest": "not-a-digest"}),
            "InvalidParameterException",
        ),
        (
            "an entry carrying neither a digest nor a tag",
            json!({}),
            "InvalidParameterException",
        ),
        (
            "a resolvable entry beside one that resolves to nothing",
            json!({"imageDigest": doomed_digest.clone()}),
            "ImageNotFoundException",
        ),
    ];

    for (case, entry, code) in &rows {
        let mut ids = vec![entry.clone()];
        if *case == "a resolvable entry beside one that resolves to nothing" {
            ids.insert(0, json!({"imageDigest": kept_digest.clone()}));
        }
        refused(
            &router,
            case,
            "DescribeImages",
            &json!({"repositoryName": "image-probes", "imageIds": ids}),
            code,
        )
        .await;
    }

    // A request naming no identifier reports what the repository still holds: the one
    // image that was not deleted.
    let described = ok(
        &router,
        "every image",
        "DescribeImages",
        &json!({"repositoryName": "image-probes"}),
    )
    .await;
    let digests: Vec<&str> = described["imageDetails"]
        .as_array()
        .map(|details| {
            details
                .iter()
                .filter_map(|detail| detail["imageDigest"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        digests,
        vec![kept_digest.as_str()],
        "only the image that was not deleted is left, got {described:?}"
    );
}

/// Create a pull through cache rule for `prefix`, with the credentials `patch` carries.
async fn given_rule(router: &MultiServiceRouter, case: &str, prefix: &str, patch: &Value) -> Value {
    let mut request = json!({
        "ecrRepositoryPrefix": prefix,
        "upstreamRegistryUrl": "registry-1.docker.io",
        "upstreamRegistry": "docker-hub",
    });
    for (member, value) in patch.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    ok(router, case, "CreatePullThroughCacheRule", &request).await
}

/// The rule the registry holds for `prefix`.
async fn described_rule(router: &MultiServiceRouter, case: &str, prefix: &str) -> Value {
    let body = ok(
        router,
        case,
        "DescribePullThroughCacheRules",
        &json!({"ecrRepositoryPrefixes": [prefix]}),
    )
    .await;
    body["pullThroughCacheRules"][0].clone()
}

/// Credentials are updatable only on a rule that was created authenticated: an update
/// carrying a credential against a rule created without one is refused, and the rule is
/// left exactly as it was.
///
/// The rows differ along what the rule was created with and which credential member the
/// update carries — the two dimensions the rule generalises over.
#[tokio::test]
async fn credentials_are_updatable_only_on_an_authenticated_rule() {
    // (case, the credentials the rule is created with, the credentials the update sends,
    //  whether the update is accepted)
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "a credentialArn update on a rule created with none",
            json!({}),
            json!({"credentialArn": SECRET_ARN}),
            false,
        ),
        (
            "a customRoleArn update on a rule created with none",
            json!({}),
            json!({"customRoleArn": ROLE_ARN}),
            false,
        ),
        (
            "both credentials at once on a rule created with none",
            json!({}),
            json!({"credentialArn": SECRET_ARN, "customRoleArn": ROLE_ARN}),
            false,
        ),
        (
            "an update carrying no credential at all, on a rule created with none",
            json!({}),
            json!({}),
            true,
        ),
        (
            "a credentialArn update on a rule created with a credentialArn",
            json!({"credentialArn": SECRET_ARN}),
            json!({"credentialArn": OTHER_SECRET_ARN}),
            true,
        ),
        (
            "a customRoleArn update on a rule created with a credentialArn",
            json!({"credentialArn": SECRET_ARN}),
            json!({"customRoleArn": ROLE_ARN}),
            true,
        ),
        (
            "a credentialArn update on a rule created with a customRoleArn",
            json!({"customRoleArn": ROLE_ARN}),
            json!({"credentialArn": SECRET_ARN}),
            true,
        ),
    ];

    let router = emulator();
    for (index, (case, created_with, update, accepted)) in rows.iter().enumerate() {
        let prefix = format!("ptc-rule-{index}");
        given_rule(&router, case, &prefix, created_with).await;
        let before = described_rule(&router, case, &prefix).await;

        let mut request = json!({"ecrRepositoryPrefix": prefix.clone()});
        for (member, value) in update.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let answer = call(&router, "UpdatePullThroughCacheRule", &request).await;

        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            for (member, value) in update.as_object().into_iter().flatten() {
                assert_eq!(
                    &answer.body[member], value,
                    "case {case:?}: the update must store {member}, got {:?}",
                    answer.body
                );
            }
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                UNAUTHENTICATED_RULE_MESSAGE,
                "case {case:?}: expected the unauthenticated-rule refusal, got {:?}",
                answer.body
            );
            // A refused update changes nothing: the rule is still the created one.
            let after = described_rule(&router, case, &prefix).await;
            assert_eq!(
                after, before,
                "case {case:?}: the refused update must leave the rule alone, got {after:?}"
            );
        }
    }
}

/// A replication destination names a registry in a Region that exists: a configuration
/// whose destination Region is not an AWS Region is refused, and nothing is stored.
///
/// The accepted rows are Region codes of `emulator_telemetry::policy::KNOWN_REGIONS` —
/// the catalog the handler checks against — spanning the commercial, GovCloud and
/// ISO partitions, none of them the Region the issue cited. The refused rows differ along
/// how the label fails to be a Region: a name no partition has, a numbered Region past
/// what exists, a Region code missing its number, an unknown geography, and a real
/// Region's code with an Availability Zone letter stuck on it. Every one of them
/// satisfies the modelled `ReplicationDestination$region` pattern
/// (`^[0-9a-z-]{2,25}$`), so only the handler can refuse it.
#[tokio::test]
async fn a_replication_destination_must_name_a_region_that_exists() {
    /// Another account's registry: a well-formed `RegistryId` that is not this one, so a
    /// destination naming it is cross-account replication rather than the source.
    const FOREIGN_REGISTRY_ID: &str = "111122223333";

    // (case, the destination Region, whether the configuration is accepted)
    let rows: [(&str, &str, bool); 11] = [
        ("a commercial Region", "ap-northeast-3", true),
        ("a Region in another geography", "sa-east-1", true),
        ("a recently added Region", "mx-central-1", true),
        ("a GovCloud Region", "us-gov-west-1", true),
        ("an ISO partition Region", "eu-isoe-west-1", true),
        ("a China partition Region", "cn-northwest-1", true),
        ("a Region name no partition has", "not-a-real-region-xyz", false),
        ("a Region number past what exists", "us-east-99", false),
        ("a Region code with no number", "eu-west", false),
        ("an unknown geography", "xx-south-1", false),
        ("a Region code with a zone letter", "us-east-1a", false),
    ];

    let router = emulator();
    // What the registry holds right now: only an accepted put changes it.
    let mut stored = json!({"rules": []});

    for (case, region, accepted) in rows {
        let configuration = json!({
            "rules": [{
                "destinations": [{"region": region, "registryId": FOREIGN_REGISTRY_ID}],
            }],
        });
        let answer = call_in_region(
            &router,
            REGION,
            "PutReplicationConfiguration",
            &json!({"replicationConfiguration": configuration}),
        )
        .await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 for region {region}, got {} with {:?}",
                answer.status, answer.body
            );
            stored = configuration;
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 for region {region}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {:?}",
                answer.body
            );
            let message = answer.message();
            assert!(
                message.contains("'replicationConfiguration'") && message.contains(region),
                "case {case:?}: the refusal must name the parameter and the region \
                 {region}, got {message:?}"
            );
        }

        // Whatever the verdict, the registry holds the last ACCEPTED configuration.
        let described = ok(&router, case, "DescribeRegistry", &json!({})).await;
        assert_eq!(
            described["replicationConfiguration"], stored,
            "case {case:?}: expected the registry to still hold {stored:?}, got {described:?}"
        );
    }
}

/// A bad Region is refused wherever it sits in the configuration, not only in the first
/// destination of the first rule.
///
/// The rows differ along WHERE the bad destination is: the only one, the second
/// destination of one rule, the second rule's destination, and beside a Region that is
/// real — the positions the rule generalises over.
#[tokio::test]
async fn a_bad_region_is_refused_wherever_it_sits_in_the_configuration() {
    /// A Region no partition has, satisfying the modelled pattern.
    const ABSENT_REGION: &str = "nowhere-central-1";

    let destination = |region: &str| json!({"region": region, "registryId": "111122223333"});

    let rows: Vec<(&str, Value)> = vec![
        (
            "the only destination",
            json!([{"destinations": [destination(ABSENT_REGION)]}]),
        ),
        (
            "the second destination of one rule",
            json!([{"destinations": [destination("eu-west-3"), destination(ABSENT_REGION)]}]),
        ),
        (
            "the destination of the second rule",
            json!([
                {"destinations": [destination("eu-west-3")]},
                {"destinations": [destination(ABSENT_REGION)]},
            ]),
        ),
        (
            "a rule carrying repository filters",
            json!([{
                "destinations": [destination(ABSENT_REGION)],
                "repositoryFilters": [{"filter": "team-a", "filterType": "PREFIX_MATCH"}],
            }]),
        ),
    ];

    let router = emulator();
    for (case, rules) in &rows {
        let answer = call_in_region(
            &router,
            REGION,
            "PutReplicationConfiguration",
            &json!({"replicationConfiguration": {"rules": rules}}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.message().contains(ABSENT_REGION),
            "case {case:?}: the refusal must name the offending region, got {:?}",
            answer.body
        );
        let described = ok(&router, case, "DescribeRegistry", &json!({})).await;
        assert_eq!(
            described["replicationConfiguration"],
            json!({"rules": []}),
            "case {case:?}: a refused put must store nothing, got {described:?}"
        );
    }
}
