// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS encryption-association slice:
//! `AssociateEncryptionConfig` and the `DescribeUpdate` that reads its update back,
//! driven over the wire through the same `MultiServiceRouter` the production binary
//! mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded request:
//! the rows differ along what the rule generalises over (the form of the KMS key
//! reference, whether `resources` is supplied, the cluster's status, which part of the
//! request is wrong, which region the call is signed for), and each assertion names its
//! case and prints the actual value.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, UPDATE_COMPLETION_DELAY,
};
use service_com_amazonaws_eks_awswesleyfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answer from the emulator, reduced to what a client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error envelope's `__type`, absent on success.
    error_type: Option<String>,
    body: serde_json::Value,
}

impl Answer {
    fn cluster(&self) -> &serde_json::Value {
        &self.body["cluster"]
    }

    fn update(&self) -> &serde_json::Value {
        &self.body["update"]
    }

    fn update_id(&self) -> String {
        self.update()["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }
}

/// The emulator under test: one shared engine, one service, authentication off so a
/// test signs nothing (the credential scope below is still parsed for its region).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/eks/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.body();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null)
    };
    let error_type = error_type.or_else(|| {
        body.get("__type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    });
    Answer {
        status,
        error_type,
        body,
    }
}

const REGION: &str = "us-west-2";

/// A valid KMS key ARN in `region`, in the form `Provider$keyArn` documents.
fn key_arn_in(region: &str) -> String {
    format!("arn:aws:kms:{region}:000000000000:key/9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d")
}

/// The smallest `CreateCluster` request this service accepts.
fn create_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

async fn create(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/clusters",
        &create_request(name).to_string(),
    )
    .await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, REGION, "GET", &format!("/clusters/{name}"), "").await
}

async fn associate_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!("/clusters/{name}/encryption-config/associate"),
        &body.to_string(),
    )
    .await
}

async fn associate(
    router: &MultiServiceRouter,
    name: &str,
    body: &serde_json::Value,
) -> Answer {
    associate_in(router, REGION, name, body).await
}

async fn describe_update(router: &MultiServiceRouter, name: &str, update_id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{name}/updates/{update_id}"),
        "",
    )
    .await
}

/// A request body associating `encryption_config`, with no idempotency token.
fn association_of(encryption_config: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "encryptionConfig": encryption_config })
}

/// Wait out the span the clock owes a newly created cluster before it is `ACTIVE`,
/// plus a margin so the test does not race the due instant.
async fn settle_creation() {
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Wait out the span the clock owes an in-flight update before it is `Successful`.
async fn settle_update() {
    tokio::time::sleep(UPDATE_COMPLETION_DELAY + Duration::from_millis(250)).await;
}

/// `true` when `id` has the shape of a version-4 UUID, which is what `Update$id` is
/// documented to be ("a UUID that is used to track the update").
fn is_uuid_v4(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    let [a, b, c, d, e] = groups.as_slice() else {
        return false;
    };
    [(a, 8), (b, 4), (c, 4), (d, 4), (e, 12)]
        .iter()
        .all(|(group, len)| {
            group.len() == *len && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && c.starts_with('4')
        && d.starts_with(['8', '9', 'a', 'b'])
}

// ---------------------------------------------------------------------------
// RULE: associating an encryption configuration with a running cluster starts one
// asynchronous update, which reports `InProgress` with the requested configuration
// as its parameter until the clock completes it; completing it is what puts the
// configuration on the cluster and hands the cluster back to `ACTIVE`.
// ---------------------------------------------------------------------------

/// The table spans the forms `Provider$keyArn` documents — "Amazon Resource Name
/// (ARN) or alias of the KMS key", in any partition and in any account, since a key
/// "created in a different account" is explicitly allowed — and whether the
/// deprecated `resources` member is supplied at all.
#[tokio::test]
async fn an_association_starts_an_update_that_completes_onto_the_cluster() {
    let router = emulator();

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "a key ARN with resources named",
            "assoc-key-arn",
            serde_json::json!([{
                "resources": ["secrets"],
                "provider": { "keyArn": key_arn_in(REGION) },
            }]),
        ),
        (
            "a key ARN with resources omitted",
            "assoc-no-resources",
            serde_json::json!([{
                "provider": { "keyArn": format!("arn:aws:kms:{REGION}:000000000000:key/abcd") },
            }]),
        ),
        (
            "an alias ARN rather than a key ARN",
            "assoc-alias-arn",
            serde_json::json!([{
                "resources": ["secrets"],
                "provider": { "keyArn": format!("arn:aws:kms:{REGION}:000000000000:alias/eks-secrets") },
            }]),
        ),
        (
            "a bare alias name",
            "assoc-bare-alias",
            serde_json::json!([{
                "provider": { "keyArn": "alias/eks-secrets" },
            }]),
        ),
        (
            "a key ARN in another partition",
            "assoc-other-partition",
            serde_json::json!([{
                "resources": ["secrets"],
                "provider": { "keyArn": format!("arn:aws-us-gov:kms:{REGION}:000000000000:key/gov-1") },
            }]),
        ),
        (
            "a key ARN in another account",
            "assoc-cross-account",
            serde_json::json!([{
                "resources": ["secrets"],
                "provider": { "keyArn": format!("arn:aws:kms:{REGION}:210987654321:key/shared-1") },
            }]),
        ),
    ];

    for (case, name, _) in &cases {
        let created = create(&router, name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle_creation().await;

    let mut ids = BTreeSet::new();
    for (case, name, encryption_config) in &cases {
        let requested = association_of(encryption_config.clone());
        let started = associate(&router, name, &requested).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200 from AssociateEncryptionConfig, got {started:?}"
        );

        let id = started.update_id();
        assert!(
            is_uuid_v4(&id),
            "case {case:?}: expected a v4 UUID update id, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: update id {id:?} was already minted for another cluster"
        );
        assert_eq!(
            started.update()["status"],
            serde_json::json!("InProgress"),
            "case {case:?}: a freshly started update is InProgress, got {:?}",
            started.update()["status"]
        );
        assert_eq!(
            started.update()["type"],
            serde_json::json!("AssociateEncryptionConfig"),
            "case {case:?}: expected the AssociateEncryptionConfig update type, got {:?}",
            started.update()["type"]
        );
        assert!(
            started.update()["createdAt"].is_number(),
            "case {case:?}: expected an epoch-seconds createdAt, got {:?}",
            started.update()["createdAt"]
        );

        // The update reports what was asked for, under the `UpdateParamType` key for
        // an encryption configuration.
        let params = started.update()["params"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            params.len(),
            1,
            "case {case:?}: expected exactly one update param, got {params:?}"
        );
        assert_eq!(
            params[0]["type"],
            serde_json::json!("EncryptionConfig"),
            "case {case:?}: expected the EncryptionConfig param type, got {:?}",
            params[0]["type"]
        );
        let reported: serde_json::Value = serde_json::from_str(
            params[0]["value"].as_str().unwrap_or_default(),
        )
        .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            reported[0]["provider"]["keyArn"],
            encryption_config[0]["provider"]["keyArn"],
            "case {case:?}: expected the requested keyArn in the update param, got {reported:?}"
        );

        // The update is asynchronous: the cluster is UPDATING and still reports no
        // encryption configuration while it runs, and the update itself reads back
        // through DescribeUpdate in that transient state.
        let polled = describe_update(&router, name, &id).await;
        assert_eq!(
            polled.status, 200,
            "case {case:?}: expected 200 from DescribeUpdate, got {polled:?}"
        );
        assert_eq!(
            polled.update(),
            started.update(),
            "case {case:?}: DescribeUpdate must report the update that was started, got {polled:?}"
        );

        let during = describe(&router, name).await;
        assert_eq!(
            during.cluster()["status"],
            serde_json::json!("UPDATING"),
            "case {case:?}: a cluster under an update is UPDATING, got {:?}",
            during.cluster()["status"]
        );
        assert_eq!(
            during.cluster()["encryptionConfig"],
            serde_json::Value::Null,
            "case {case:?}: the configuration is not on the cluster until the update \
             completes, got {:?}",
            during.cluster()["encryptionConfig"]
        );
    }

    settle_update().await;

    for (case, name, encryption_config) in &cases {
        let cluster = describe(&router, name).await;
        assert_eq!(
            cluster.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: a completed update hands the cluster back to ACTIVE, got {:?}",
            cluster.cluster()["status"]
        );
        assert_eq!(
            cluster.cluster()["encryptionConfig"],
            *encryption_config,
            "case {case:?}: expected the associated configuration on the cluster, got {:?}",
            cluster.cluster()["encryptionConfig"]
        );
    }

    assert_eq!(
        ids.len(),
        cases.len(),
        "every association must mint its own update id, got {ids:?}"
    );
}

/// The same rule, read through `DescribeUpdate`: the update a caller polls moves to
/// `Successful` once the clock owes it, and reading it does not move it again.
#[tokio::test]
async fn a_started_update_reads_back_and_settles_to_successful() {
    let router = emulator();

    // The rows differ along the configuration being associated, so the rule is
    // asserted over more than the one shape the recorded request used.
    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "a key ARN",
            "settle-key-arn",
            serde_json::json!([{
                "resources": ["secrets"],
                "provider": { "keyArn": key_arn_in(REGION) },
            }]),
        ),
        (
            "a bare alias name",
            "settle-alias",
            serde_json::json!([{ "provider": { "keyArn": "alias/settling" } }]),
        ),
    ];

    for (case, name, _) in &cases {
        let created = create(&router, name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle_creation().await;

    let mut started = Vec::new();
    for (case, name, encryption_config) in &cases {
        let answer = associate(&router, name, &association_of(encryption_config.clone())).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from AssociateEncryptionConfig, got {answer:?}"
        );
        started.push(answer.update_id());
    }

    settle_update().await;

    for ((case, name, encryption_config), id) in cases.iter().zip(started) {
        let settled = describe_update(&router, name, &id).await;
        assert_eq!(
            settled.update()["status"],
            serde_json::json!("Successful"),
            "case {case:?}: once the completion delay has passed the update is \
             Successful, got {:?}",
            settled.update()["status"]
        );
        // Re-reading does not move it again or disturb anything else.
        let twice = describe_update(&router, name, &id).await;
        assert_eq!(
            twice.update(),
            settled.update(),
            "case {case:?}: re-reading a settled update must report the same update, \
             got {twice:?}"
        );
        assert_eq!(
            twice.update()["params"],
            settled.update()["params"],
            "case {case:?}: a settled update keeps the parameters it was started with"
        );

        let cluster = describe(&router, name).await;
        assert_eq!(
            cluster.cluster()["encryptionConfig"],
            *encryption_config,
            "case {case:?}: expected the associated configuration on the cluster, got {:?}",
            cluster.cluster()["encryptionConfig"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the cluster named by the request's path is resolved FIRST and in the
// region the request is signed for — a call against a cluster this region does not
// hold is 404 `ResourceNotFoundException`, whatever the body carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_against_a_cluster_this_region_does_not_hold_is_not_found() {
    let router = emulator();

    let created = create(&router, "present").await;
    assert_eq!(
        created.status, 200,
        "the fixture cluster must be created, got {created:?}"
    );
    let removed = call(&router, REGION, "DELETE", "/clusters/deleted-one", "").await;
    assert_eq!(
        removed.status, 404,
        "the deleted-one fixture does not exist yet, got {removed:?}"
    );
    let created = create(&router, "deleted-one").await;
    assert_eq!(
        created.status, 200,
        "the deletable fixture must be created, got {created:?}"
    );
    settle_creation().await;
    let removed = call(&router, REGION, "DELETE", "/clusters/deleted-one", "").await;
    assert_eq!(
        removed.status, 200,
        "the deletable fixture must be deleted, got {removed:?}"
    );

    let valid = association_of(serde_json::json!([{
        "resources": ["secrets"],
        "provider": { "keyArn": key_arn_in(REGION) },
    }]));
    // The last row carries a body that is ALSO invalid: resolving the cluster comes
    // first, so the answer is still 404 and not the 400 the body would earn.
    let invalid = association_of(serde_json::json!([]));

    let cases: Vec<(&str, &str, &str, &serde_json::Value)> = vec![
        ("a name never created", REGION, "no-such-cluster", &valid),
        ("a deleted cluster", REGION, "deleted-one", &valid),
        (
            "a cluster that exists only in another region",
            "us-east-1",
            "present",
            &valid,
        ),
        (
            "a name never created, with an invalid body",
            REGION,
            "no-such-cluster",
            &invalid,
        ),
    ];

    for (case, region, name, body) in &cases {
        let answer = associate_in(&router, region, name, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // The cluster that does exist was not touched by any of those refusals.
    let untouched = describe(&router, "present").await;
    assert_eq!(
        untouched.cluster()["status"],
        serde_json::json!("ACTIVE"),
        "a refused association must leave the cluster alone, got {untouched:?}"
    );
    assert_eq!(
        untouched.cluster()["encryptionConfig"],
        serde_json::Value::Null,
        "a refused association must store nothing, got {untouched:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an `encryptionConfig` that does not name exactly one KMS key the cluster
// can use — "ARN or alias of the KMS key", in the cluster's own region, encrypting
// only `secrets` — is refused with 400 `InvalidParameterException` and nothing is
// persisted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invalid_encryption_config_is_refused_and_persists_nothing() {
    let router = emulator();
    let name = "validation";
    let created = create(&router, name).await;
    assert_eq!(
        created.status, 200,
        "the fixture cluster must be created, got {created:?}"
    );
    settle_creation().await;

    // The rows span every way the configuration can fail the rule: the wrong number
    // of entries, no key named at all, a key reference that is not one of the two
    // documented forms, a key outside the cluster's region, and a resource the
    // member does not support.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("no entry at all", serde_json::json!([])),
        (
            "an entry with no provider",
            serde_json::json!([{ "resources": ["secrets"] }]),
        ),
        (
            "an entry whose provider names no key",
            serde_json::json!([{ "provider": {} }]),
        ),
        (
            "a blank keyArn",
            serde_json::json!([{ "provider": { "keyArn": "   " } }]),
        ),
        (
            "a keyArn that is not an ARN or an alias",
            serde_json::json!([{ "provider": { "keyArn": "my-kms-key" } }]),
        ),
        (
            "a bare key id, which is neither the ARN nor the alias form",
            serde_json::json!([{
                "provider": { "keyArn": "9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d" },
            }]),
        ),
        (
            "an ARN of another service",
            serde_json::json!([{
                "provider": { "keyArn": format!("arn:aws:s3:{REGION}:000000000000:bucket/keys") },
            }]),
        ),
        (
            "a KMS ARN naming no key resource",
            serde_json::json!([{
                "provider": { "keyArn": format!("arn:aws:kms:{REGION}:000000000000:key/") },
            }]),
        ),
        (
            "a KMS ARN with no account",
            serde_json::json!([{
                "provider": { "keyArn": format!("arn:aws:kms:{REGION}::key/abcd") },
            }]),
        ),
        (
            "an empty alias name",
            serde_json::json!([{ "provider": { "keyArn": "alias/" } }]),
        ),
        (
            "a key in another region than the cluster",
            serde_json::json!([{
                "resources": ["secrets"],
                "provider": { "keyArn": key_arn_in("eu-west-1") },
            }]),
        ),
        (
            "a resources value other than secrets",
            serde_json::json!([{
                "resources": ["configmaps"],
                "provider": { "keyArn": key_arn_in(REGION) },
            }]),
        ),
        (
            "secrets plus an unsupported resource",
            serde_json::json!([{
                "resources": ["secrets", "configmaps"],
                "provider": { "keyArn": key_arn_in(REGION) },
            }]),
        ),
    ];

    for (case, encryption_config) in &cases {
        let answer = associate(&router, name, &association_of(encryption_config.clone())).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );

        // Nothing was started and nothing was stored: the cluster is still ACTIVE
        // with no encryption configuration.
        let cluster = describe(&router, name).await;
        assert_eq!(
            cluster.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: a refused association must not move the cluster, got {:?}",
            cluster.cluster()["status"]
        );
        assert_eq!(
            cluster.cluster()["encryptionConfig"],
            serde_json::Value::Null,
            "case {case:?}: a refused association must store nothing, got {:?}",
            cluster.cluster()["encryptionConfig"]
        );
    }

    // And a valid configuration on the same cluster is still accepted afterwards.
    let accepted = associate(
        &router,
        name,
        &association_of(serde_json::json!([{
            "resources": ["secrets"],
            "provider": { "keyArn": key_arn_in(REGION) },
        }])),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a valid configuration must still be accepted after the refusals, got {accepted:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a cluster takes one encryption association at a time and only once — it must
// be ACTIVE to take one (else 400 `InvalidRequestException`), another cannot start
// while one is in flight (409 `ResourceInUseException`), none can start once
// encryption is enabled (400 `InvalidRequestException`), and a retry carrying the
// same `clientRequestToken` is answered with the update it already started.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_takes_one_encryption_association_at_a_time_and_only_once() {
    let router = emulator();

    let config = |key: &str| {
        association_of(serde_json::json!([{
            "resources": ["secrets"],
            "provider": { "keyArn": format!("arn:aws:kms:{REGION}:000000000000:key/{key}") },
        }]))
    };

    // Rows differ along the status the cluster is in when the association is tried;
    // each is a different point of the lifecycle, not a different name.
    for (case, name) in [
        ("a cluster still being created", "not-yet-active"),
        ("a second cluster still being created", "also-creating"),
    ] {
        let created = create(&router, name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        let refused = associate(&router, name, &config("k1")).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a cluster that is not ACTIVE cannot take an update, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
    }

    let name = "one-at-a-time";
    let created = create(&router, name).await;
    assert_eq!(
        created.status, 200,
        "the fixture cluster must be created, got {created:?}"
    );
    settle_creation().await;

    let mut first = config("k1");
    first["clientRequestToken"] = serde_json::json!("token-one");
    let started = associate(&router, name, &first).await;
    assert_eq!(
        started.status, 200,
        "the first association must be accepted, got {started:?}"
    );
    let id = started.update_id();

    // A retry of that same call — same token — is the same update, not a second one.
    let retried = associate(&router, name, &first).await;
    assert_eq!(
        retried.status, 200,
        "case \"a retry with the same token\": expected 200, got {retried:?}"
    );
    assert_eq!(
        retried.update_id(),
        id,
        "case \"a retry with the same token\": expected update {id:?}, got {retried:?}"
    );

    // A DIFFERENT call while the first update runs is refused: the cluster is in use.
    for (case, key, token) in [
        ("another key, no token", "k2", serde_json::Value::Null),
        ("another key, another token", "k3", serde_json::json!("token-two")),
    ] {
        let mut body = config(key);
        if !token.is_null() {
            body["clientRequestToken"] = token;
        }
        let refused = associate(&router, name, &body).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: a cluster under an update is in use, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {refused:?}"
        );
    }

    settle_update().await;

    // The retry is still absorbed once the update has completed: the token names that
    // update for as long as it is held.
    let late_retry = associate(&router, name, &first).await;
    assert_eq!(
        late_retry.update_id(),
        id,
        "case \"a retry after completion\": expected update {id:?}, got {late_retry:?}"
    );
    assert_eq!(
        late_retry.update()["status"],
        serde_json::json!("Successful"),
        "case \"a retry after completion\": expected the completed update, got {late_retry:?}"
    );

    // And a fresh association is refused for good: encryption is already enabled.
    for (case, key, token) in [
        ("a new key, no token", "k4", serde_json::Value::Null),
        ("a new key, a new token", "k5", serde_json::json!("token-three")),
    ] {
        let mut body = config(key);
        if !token.is_null() {
            body["clientRequestToken"] = token;
        }
        let refused = associate(&router, name, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: encryption is already enabled, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {refused:?}"
        );
    }

    // Exactly one configuration is on the cluster: the one the first call asked for.
    let cluster = describe(&router, name).await;
    assert_eq!(
        cluster.cluster()["encryptionConfig"],
        first["encryptionConfig"],
        "expected only the first association on the cluster, got {:?}",
        cluster.cluster()["encryptionConfig"]
    );
}

// ---------------------------------------------------------------------------
// RULE: `DescribeUpdate` reports an update only for the cluster that holds it, and
// only when the request names no resource inside the cluster; anything else is 404
// `ResourceNotFoundException`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_update_refuses_an_update_it_does_not_hold() {
    let router = emulator();

    for name in ["holder", "other"] {
        let created = create(&router, name).await;
        assert_eq!(
            created.status, 200,
            "the {name:?} fixture must be created, got {created:?}"
        );
    }
    settle_creation().await;

    let started = associate(
        &router,
        "holder",
        &association_of(serde_json::json!([{
            "resources": ["secrets"],
            "provider": { "keyArn": key_arn_in(REGION) },
        }])),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the fixture association must be accepted, got {started:?}"
    );
    let id = started.update_id();

    // The update is found on the cluster that holds it.
    let found = describe_update(&router, "holder", &id).await;
    assert_eq!(
        found.status, 200,
        "the holding cluster reports its own update, got {found:?}"
    );

    // The rows span the ways a lookup can miss: an unknown cluster, the right id on
    // the wrong cluster, an id nothing minted, and the three filters that select a
    // resource inside the cluster (no nodegroup, add-on or capability exists here).
    let cases: Vec<(&str, String)> = vec![
        ("an unknown cluster", format!("/clusters/nope/updates/{id}")),
        (
            "the right id on another cluster",
            format!("/clusters/other/updates/{id}"),
        ),
        (
            "an id nothing minted",
            "/clusters/holder/updates/00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "a nodegroup-scoped lookup",
            format!("/clusters/holder/updates/{id}?nodegroupName=ng-1"),
        ),
        (
            "an addon-scoped lookup",
            format!("/clusters/holder/updates/{id}?addonName=vpc-cni"),
        ),
        (
            "a capability-scoped lookup",
            format!("/clusters/holder/updates/{id}?capabilityName=cap-1"),
        ),
    ];

    for (case, uri) in &cases {
        let answer = call(&router, REGION, "GET", uri, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }
}
