// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ReplicateKey`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: `ReplicateKey` creates, in another Region, a NEW key of the
//! named key's multi-Region key set. The replica shares the set's properties — key
//! id, key material, key spec, key usage, key material origin and automatic rotation
//! status — takes its description, key policy and tags from the request, and joins the
//! set's topology, so every key of the set reports the same primary key and the same
//! replica keys whichever endpoint reports it. It reaches the caller in the transient
//! `Creating` key state, where it cannot be used in a cryptographic operation, and
//! the clock alone moves it to the state it was created for. The call is refused, and
//! persists nothing, when the named key is not the primary key of a multi-Region set,
//! when its key state is not compatible with the operation, when the Region already
//! holds a key of the set, and when the policy or tags the request carries are not
//! ones KMS stores.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in the Region
//! the replica is created in (two-, three- and four-segment Region ids), in the kind
//! of key the set is made of (symmetric encryption, HMAC, asymmetric signing), in
//! whether automatic rotation is on, and in which optional members the request
//! carries (description, key policy, zero/one/several tags).

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::key_arn;

use super::harness::{
    b64, call, create_alias, create_key, describe, error_code, failure, failure_in, ok, ok_in,
    reported_policy, router, settle_key_state, text, unb64, REGION,
};

/// The metadata members a replica key SHARES with the primary key it was replicated
/// from: "they have the same key ID and key material. They also have the same key
/// spec, key usage, key material origin, and automatic key rotation status"
/// (`smithy_query(query="doc",shape="ReplicateKey")`). `CurrentKeyMaterialId` is
/// derived from the key material itself, so it is how the wire shows the material is
/// the same one; the per-usage algorithm lists follow from the shared spec and usage.
const SHARED_METADATA_MEMBERS: [&str; 8] = [
    "AWSAccountId",
    "KeyId",
    "KeySpec",
    "KeyUsage",
    "Origin",
    "CurrentKeyMaterialId",
    "EncryptionAlgorithms",
    "SigningAlgorithms",
];

/// A key policy document KMS stores, naming the account root as its principal — the
/// criteria are the `Policy` member's own
/// (`smithy_query(query="doc",shape="ReplicateKeyRequest",member="Policy")`).
fn policy_document(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Id": "replica-key-policy",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": {
                "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
            },
            "Action": "kms:*",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// The tags `region`'s endpoint reports for `key_id`, as (tag key, tag value) pairs
/// in tag-key order.
async fn tags_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    region: &str,
    key_id: &str,
) -> Vec<(String, String)> {
    let listed = ok_in(
        router,
        region,
        "ListResourceTags",
        &json!({"KeyId": key_id}),
    )
    .await;
    let mut tags: Vec<(String, String)> = listed["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected Tags in {listed}"))
        .iter()
        .map(|tag| (text(case, tag, "TagKey"), text(case, tag, "TagValue")))
        .collect();
    tags.sort();
    tags
}

/// The tag list of a request body, as (tag key, tag value) pairs in tag-key order.
fn requested_tags(body: &Value) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = body["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["TagKey"].as_str().unwrap_or_default().to_string(),
                        tag["TagValue"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    tags.sort();
    tags
}

/// The Regions of the multi-Region configuration of a key: which member of the set
/// this key is, the Region of its primary key, and the Regions of its replica keys.
fn topology_of(case: &str, metadata: &Value) -> (String, String, Vec<String>) {
    let configuration = metadata["MultiRegionConfiguration"].clone();
    let key_type = text(case, &configuration, "MultiRegionKeyType");
    let key_id = text(case, metadata, "KeyId");
    let primary = configuration["PrimaryKey"].clone();
    let primary_region = text(case, &primary, "Region");
    assert_eq!(
        primary["Arn"],
        json!(key_arn(&primary_region, &key_id)),
        "case {case}: the primary key is named by the key ARN of its own Region, got {primary}"
    );
    let mut replicas: Vec<String> = configuration["ReplicaKeys"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected ReplicaKeys in {configuration}"))
        .iter()
        .map(|replica| {
            let region = text(case, replica, "Region");
            assert_eq!(
                replica["Arn"],
                json!(key_arn(&region, &key_id)),
                "case {case}: a replica key is named by the key ARN of its own Region, got \
                 {replica}"
            );
            region
        })
        .collect();
    replicas.sort();
    (key_type, primary_region, replicas)
}

/// `ReplicateKey` creates a key of the set in the replica Region: it shares the set's
/// properties, carries the description, policy and tags the request gave it, starts
/// in the transient `Creating` state and then reaches the state it was created for —
/// and every row is read back through `DescribeKey`, `GetKeyPolicy` and
/// `ListResourceTags` at the replica's OWN endpoint.
#[tokio::test]
async fn replicate_key_creates_a_key_of_the_set_in_the_replica_region() {
    let router = router();

    // (case, replica Region, CreateKey request, ReplicateKey members beside KeyId and
    // ReplicaRegion, whether automatic rotation is enabled on the set first, whether
    // the set's keys encrypt and decrypt). The Region ids span the forms
    // `RegionType`'s modelled `^([a-z]+-){2,3}\d+$` pattern admits
    // (`smithy_query(query="operation",operation="ReplicateKey")`): two segments
    // before the digit, three of them, and a longer name.
    let rows: Vec<(&str, &str, Value, Value, bool, bool)> = vec![
        (
            "a symmetric encryption key, no optional member",
            "eu-west-2",
            json!({"MultiRegion": true}),
            json!({}),
            false,
            true,
        ),
        (
            "a description and one tag",
            "ap-southeast-2",
            json!({"MultiRegion": true, "Description": "the primary"}),
            json!({
                "Description": "the replica",
                "Tags": [{"TagKey": "env", "TagValue": "prod"}],
            }),
            false,
            true,
        ),
        (
            "a key policy of its own and three tags",
            "us-gov-west-1",
            json!({"MultiRegion": true, "Description": "policy and tags"}),
            json!({
                "Policy": policy_document("replica"),
                "Tags": [
                    {"TagKey": "env", "TagValue": "staging"},
                    {"TagKey": "owner", "TagValue": "platform"},
                    {"TagKey": "empty", "TagValue": ""},
                ],
            }),
            false,
            true,
        ),
        (
            "a set with automatic key rotation enabled",
            "eu-central-1",
            json!({"MultiRegion": true}),
            json!({"Description": "rotating set"}),
            true,
            true,
        ),
        (
            "an HMAC key",
            "ap-northeast-1",
            json!({"MultiRegion": true, "KeySpec": "HMAC_256",
                   "KeyUsage": "GENERATE_VERIFY_MAC"}),
            json!({}),
            false,
            false,
        ),
        (
            "an asymmetric signing key",
            "sa-east-1",
            json!({"MultiRegion": true, "KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
            json!({"Tags": [{"TagKey": "kind", "TagValue": "asymmetric"}]}),
            false,
            false,
        ),
    ];

    for (case, replica_region, create, extras, rotating, cryptographic) in rows {
        let primary = create_key(&router, &create).await;
        let key_id = text(case, &primary, "KeyId");
        if rotating {
            ok(
                &router,
                "EnableKeyRotation",
                &json!({"KeyId": key_id, "RotationPeriodInDays": 180}),
            )
            .await;
        }

        // A ciphertext the PRIMARY key sealed, before the replica exists at all: a
        // replica shares the set's key material, so this must open at the replica's
        // endpoint later without being re-encrypted.
        let sealed = if cryptographic {
            Some(
                ok(
                    &router,
                    "Encrypt",
                    &json!({"KeyId": key_id, "Plaintext": b64(b"multi-region plaintext"),
                            "EncryptionContext": {"purpose": "replication"}}),
                )
                .await,
            )
        } else {
            None
        };

        let mut request = json!({"KeyId": key_id, "ReplicaRegion": replica_region});
        for (member, value) in extras.as_object().expect("the extras are an object") {
            request[member] = value.clone();
        }
        let answer = ok(&router, "ReplicateKey", &request).await;
        let metadata = answer["ReplicaKeyMetadata"].clone();

        // The replica is a key of the SET: one key id, one key ARN per Region.
        assert_eq!(
            metadata["Arn"],
            json!(key_arn(replica_region, &key_id)),
            "case {case}: the replica is named by the key ARN of its own Region, got {metadata}"
        );
        for member in SHARED_METADATA_MEMBERS {
            assert_eq!(
                metadata[member], primary[member],
                "case {case}: {member} is shared with the primary key, got {metadata}"
            );
        }
        assert_eq!(
            metadata["MultiRegion"],
            json!(true),
            "case {case}: the replica is a multi-Region key, got {metadata}"
        );
        assert!(
            !metadata["CreationDate"].is_null(),
            "case {case}: the replica carries its own creation date, got {metadata}"
        );

        // "When this operation completes, the new replica key has a transient key
        // state of Creating", and only the clock ends it.
        assert_eq!(
            (metadata["KeyState"].clone(), metadata["Enabled"].clone()),
            (json!("Creating"), json!(false)),
            "case {case}: a new replica key is Creating and not yet enabled, got {metadata}"
        );
        if cryptographic {
            let (status, payload) = call(
                &router,
                replica_region,
                "Encrypt",
                &json!({"KeyId": key_id, "Plaintext": b64(b"too early")}),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "KMSInvalidStateException"),
                "case {case}: a replica still being created cannot be used in a cryptographic \
                 operation, got HTTP {status} with {payload}"
            );
        }

        // The description, key policy and tags are the request's own, and the replica
        // Region's endpoint reports exactly what the answer did.
        let expected_description = extras["Description"].as_str().unwrap_or_default();
        assert_eq!(
            metadata["Description"],
            json!(expected_description),
            "case {case}: the replica's description is the one the request gave, got {metadata}"
        );
        let expected_policy = extras["Policy"].as_str().map(str::to_string);
        let replica_policy = text(case, &answer, "ReplicaPolicy");
        if let Some(policy) = &expected_policy {
            assert_eq!(
                replica_policy,
                reported_policy(policy),
                "case {case}: the replica's policy is the one the request gave, got \
                 {replica_policy}"
            );
        } else {
            // No policy in the request: "if you do not provide a key policy, KMS
            // attaches the default key policy to the KMS key", which is a document
            // naming a principal rather than nothing at all.
            assert!(
                replica_policy.contains("Statement") && replica_policy.contains("Principal"),
                "case {case}: a replica with no policy in the request gets the default key \
                 policy, got {replica_policy}"
            );
        }
        let expected_tags = requested_tags(&extras);
        let answered_tags = {
            let mut tags: Vec<(String, String)> = answer["ReplicaTags"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected ReplicaTags in {answer}"))
                .iter()
                .map(|tag| (text(case, tag, "TagKey"), text(case, tag, "TagValue")))
                .collect();
            tags.sort();
            tags
        };
        assert_eq!(
            answered_tags, expected_tags,
            "case {case}: the answer's tags are the ones the request gave, got {answered_tags:?}"
        );

        // The transient state ends by itself, leaving the key state the replica was
        // created for, and the record the replica Region serves is the one the answer
        // described.
        let settled = settle_key_state(&router, replica_region, case, &key_id, "Creating").await;
        assert_eq!(
            (settled["KeyState"].clone(), settled["Enabled"].clone()),
            (json!("Enabled"), json!(true)),
            "case {case}: a replica of an enabled key becomes Enabled, got {settled}"
        );
        for member in SHARED_METADATA_MEMBERS.iter().chain(
            ["Arn", "Description", "MultiRegion", "KeyManager", "CreationDate"].iter(),
        ) {
            assert_eq!(
                settled[*member], metadata[*member],
                "case {case}: DescribeKey at the replica's endpoint reports the {member} the \
                 replicate answered, got {settled}"
            );
        }
        assert_eq!(
            ok_in(
                &router,
                replica_region,
                "GetKeyPolicy",
                &json!({"KeyId": key_id})
            )
            .await["Policy"],
            json!(replica_policy),
            "case {case}: the replica's endpoint serves the policy the replicate answered"
        );
        let listed_tags = tags_in(&router, case, replica_region, &key_id).await;
        assert_eq!(
            listed_tags, expected_tags,
            "case {case}: the replica's endpoint serves the tags the request gave, got \
             {listed_tags:?}"
        );

        // Both keys report ONE set: the primary key in the creation Region and a
        // replica key in the replica Region, whichever endpoint reports it.
        let primary_now = describe(&router, &key_id).await;
        assert_eq!(
            topology_of(case, &primary_now),
            (
                "PRIMARY".to_string(),
                REGION.to_string(),
                vec![replica_region.to_string()]
            ),
            "case {case}: the primary key reports the new replica, got {primary_now}"
        );
        assert_eq!(
            topology_of(case, &settled),
            (
                "REPLICA".to_string(),
                REGION.to_string(),
                vec![replica_region.to_string()]
            ),
            "case {case}: the replica key reports the same set, got {settled}"
        );

        // The automatic key rotation status is shared, so the replica is on the set's
        // schedule rather than on none.
        let rotation = ok_in(
            &router,
            replica_region,
            "GetKeyRotationStatus",
            &json!({"KeyId": key_id}),
        )
        .await;
        assert_eq!(
            rotation["KeyRotationEnabled"],
            json!(rotating),
            "case {case}: the replica carries the set's automatic rotation status, got {rotation}"
        );

        // The shared key material is what makes the two interoperable: a ciphertext
        // the primary key produced opens at the replica's endpoint, under the same
        // encryption context, without being re-encrypted.
        if let Some(sealed) = sealed {
            let blob = b64(&unb64(case, &sealed, "CiphertextBlob"));
            let opened = ok_in(
                &router,
                replica_region,
                "Decrypt",
                &json!({"CiphertextBlob": blob,
                        "EncryptionContext": {"purpose": "replication"}}),
            )
            .await;
            assert_eq!(
                unb64(case, &opened, "Plaintext"),
                b"multi-region plaintext".to_vec(),
                "case {case}: the replica must open a ciphertext its primary key produced"
            );
            assert_eq!(
                opened["KeyId"],
                json!(key_arn(replica_region, &key_id)),
                "case {case}: the decrypt names the key of the Region that served it, got \
                 {opened}"
            );
        }
    }
}

/// `ReplicateKey` refuses what it cannot replicate, and persists nothing when it
/// does: each row names a modelled error with its modelled status, and the Region the
/// row aimed at still holds no key of the set afterwards.
#[tokio::test]
async fn replicate_key_refuses_what_it_cannot_replicate() {
    let router = router();
    let account = engine::emulator_account_id();

    // A multi-Region set whose primary key is replicable, already replicated once:
    // the rows that are not about the key's own state name it, so a refusal is never
    // explained by the key being unreplicable.
    let primary = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    ok(
        &router,
        "ReplicateKey",
        &json!({"KeyId": primary, "ReplicaRegion": "eu-west-2"}),
    )
    .await;

    // The keys the other rows name, each in the state its row is about.
    let single = text("setup", &create_key(&router, &json!({})).await, "KeyId");
    create_alias(&router, "setup", "alias/single-region", &single).await;
    let disabled = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    ok(&router, "DisableKey", &json!({"KeyId": disabled})).await;
    let pending_deletion = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_deletion}),
    )
    .await;
    let pending_import = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true, "Origin": "EXTERNAL"})).await,
        "KeyId",
    );

    // (case, the Region the call is signed for, request, expected HTTP status,
    // expected error code). Every client refusal of this service answers HTTP 400 and
    // carries the error shape in the body's `__type`, whatever the per-shape
    // `@httpError` the model records.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a single-Region key belongs to no set",
            REGION,
            json!({"KeyId": single, "ReplicaRegion": "ap-south-1"}),
            400,
            "UnsupportedOperationException",
        ),
        (
            "an alias naming a single-Region key",
            REGION,
            json!({"KeyId": "alias/single-region", "ReplicaRegion": "ap-south-1"}),
            400,
            "InvalidArnException",
        ),
        (
            "a replica key is not the primary key of its set",
            "eu-west-2",
            json!({"KeyId": primary, "ReplicaRegion": "ap-south-1"}),
            400,
            "UnsupportedOperationException",
        ),
        (
            "the Region that already holds the primary key",
            REGION,
            json!({"KeyId": primary, "ReplicaRegion": REGION}),
            400,
            "AlreadyExistsException",
        ),
        (
            "a Region that already holds a replica of the set",
            REGION,
            json!({"KeyId": primary, "ReplicaRegion": "eu-west-2"}),
            400,
            "AlreadyExistsException",
        ),
        (
            "a disabled key",
            REGION,
            json!({"KeyId": disabled, "ReplicaRegion": "ap-south-1"}),
            400,
            "DisabledException",
        ),
        (
            "a key pending deletion",
            REGION,
            json!({"KeyId": pending_deletion, "ReplicaRegion": "ap-south-1"}),
            400,
            "KMSInvalidStateException",
        ),
        (
            "a key awaiting imported key material",
            REGION,
            json!({"KeyId": pending_import, "ReplicaRegion": "ap-south-1"}),
            400,
            "KMSInvalidStateException",
        ),
        (
            "a key id no key of this Region has",
            REGION,
            json!({"KeyId": format!("mrk-{}", "0".repeat(32)), "ReplicaRegion": "ap-south-1"}),
            400,
            "NotFoundException",
        ),
        (
            "a key ARN of another Region's endpoint",
            REGION,
            json!({"KeyId": key_arn("eu-west-2", &primary), "ReplicaRegion": "ap-south-1"}),
            400,
            "NotFoundException",
        ),
        (
            "an identifier that is an ARN of no KMS resource",
            REGION,
            json!({"KeyId": format!("arn:aws:kms:{REGION}:{account}:mrk-{}", "0".repeat(32)),
                   "ReplicaRegion": "ap-south-1"}),
            400,
            "InvalidArnException",
        ),
        (
            "an ARN whose account segment names no account",
            REGION,
            json!({"KeyId": format!("arn:aws:kms:{REGION}:not-an-account:key/{primary}"),
                   "ReplicaRegion": "ap-south-1"}),
            400,
            "AccessDeniedException",
        ),
        (
            "a key policy that is not a policy document",
            REGION,
            json!({"KeyId": primary, "ReplicaRegion": "ap-south-1", "Policy": "{}"}),
            400,
            "MalformedPolicyDocumentException",
        ),
        (
            "a policy statement naming no principal",
            REGION,
            json!({"KeyId": primary, "ReplicaRegion": "ap-south-1",
                   "Policy": json!({"Statement": [{"Effect": "Allow", "Action": "kms:*"}]})
                       .to_string()}),
            400,
            "MalformedPolicyDocumentException",
        ),
        (
            "one tag key twice",
            REGION,
            json!({"KeyId": primary, "ReplicaRegion": "ap-south-1",
                   "Tags": [{"TagKey": "env", "TagValue": "a"},
                            {"TagKey": "env", "TagValue": "b"}]}),
            400,
            "TagException",
        ),
        (
            "a tag key AWS reserves",
            REGION,
            json!({"KeyId": primary, "ReplicaRegion": "ap-south-1",
                   "Tags": [{"TagKey": "aws:owner", "TagValue": "nobody"}]}),
            400,
            "TagException",
        ),
    ];

    for (case, region, request, expected_status, expected_code) in rows {
        let (status, payload) = call(&router, region, "ReplicateKey", &request).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (expected_status, expected_code),
            "case {case}: expected HTTP {expected_status} {expected_code}, got {status} with \
             {payload}"
        );

        // Nothing was persisted: the Region the row aimed at serves no key of any
        // set, and the set the rows name still has exactly its one replica.
        let (_, code) = failure_in(
            &router,
            "ap-south-1",
            "DescribeKey",
            &json!({"KeyId": primary}),
        )
        .await;
        assert_eq!(
            code, "NotFoundException",
            "case {case}: a refused replicate must create no key in the replica Region"
        );
        let described = describe(&router, &primary).await;
        assert_eq!(
            topology_of(case, &described),
            (
                "PRIMARY".to_string(),
                REGION.to_string(),
                vec!["eu-west-2".to_string()]
            ),
            "case {case}: a refused replicate must leave the set exactly as it was, got \
             {described}"
        );
    }
}

/// The refusals carry the wording KMS words them with: a Region that already holds a
/// key of the set is refused with the rule itself, whichever key of the set that
/// Region holds, and a key belonging to no set is refused by its own ARN.
#[tokio::test]
async fn the_refusals_name_what_they_refuse() {
    let router = router();

    let primary = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    ok(
        &router,
        "ReplicateKey",
        &json!({"KeyId": primary, "ReplicaRegion": "eu-west-2"}),
    )
    .await;

    // Both Regions of the set already hold a key of it: the one holding the primary
    // key and the one holding the replica.
    for replica_region in [REGION, "eu-west-2"] {
        let (status, payload) = call(
            &router,
            REGION,
            "ReplicateKey",
            &json!({"KeyId": primary, "ReplicaRegion": replica_region}),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "AlreadyExistsException"),
            "case a key of the set is already in {replica_region}: expected HTTP 400 \
             AlreadyExistsException, got {status} with {payload}"
        );
        assert_eq!(
            message_of(&payload),
            "You can create only one replica of each primary key in each AWS Region.",
            "case a key of the set is already in {replica_region}: the refusal states the rule, \
             got {payload}"
        );
    }

    // A key that is not part of a multi-Region set at all is refused by the ARN the
    // identifier resolved to, in each form of identifier this operation accepts for it:
    // a key id and a key ARN. An ALIAS is not one of them ("specify the key ID or key
    // ARN of a multi-Region primary key"), and is refused for its KIND before anything
    // is resolved — asserted by `an_alias_is_not_an_identifier_this_operation_accepts`.
    let single = create_key(&router, &json!({"Description": "single"})).await;
    let key_id = text("setup", &single, "KeyId");
    let arn = text("setup", &single, "Arn");
    create_alias(&router, "setup", "alias/not-in-a-set", &key_id).await;
    for identifier in [key_id.clone(), arn.clone()] {
        let (status, payload) = call(
            &router,
            REGION,
            "ReplicateKey",
            &json!({"KeyId": identifier, "ReplicaRegion": "eu-west-2"}),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "UnsupportedOperationException"),
            "case a single-Region key named by {identifier}: expected HTTP 400 \
             UnsupportedOperationException, got {status} with {payload}"
        );
        assert_eq!(
            message_of(&payload),
            format!("{arn} is not a multi-region key."),
            "case a single-Region key named by {identifier}: the refusal must name the resolved \
             key by its ARN, got {payload}"
        );
    }

    // A replica key of the set is refused as not being the set's primary key, named
    // by its own ARN and by the Region that does hold the primary key.
    let (status, payload) = call(
        &router,
        "eu-west-2",
        "ReplicateKey",
        &json!({"KeyId": primary, "ReplicaRegion": "ap-south-1"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload).as_str()),
        (400, "UnsupportedOperationException"),
        "case replicating a replica key: expected HTTP 400 UnsupportedOperationException, got \
         {status} with {payload}"
    );
    assert_eq!(
        message_of(&payload),
        format!(
            "{} is not a multi-region primary key. The primary key of this multi-region key is \
             in {REGION}.",
            key_arn("eu-west-2", &primary)
        ),
        "case replicating a replica key: the refusal must name the key and the primary key's \
         Region, got {payload}"
    );

    // Every refused call above created nothing anywhere.
    let (_, code) = failure(
        &router,
        "DescribeKey",
        &json!({"KeyId": key_arn("ap-south-1", &primary)}),
    )
    .await;
    assert_eq!(
        code, "NotFoundException",
        "case refusals persist nothing: no key of the set may exist in ap-south-1"
    );
}

/// The `message` of an error body, under either casing the protocols use.
fn message_of(payload: &Value) -> String {
    payload["message"]
        .as_str()
        .or_else(|| payload["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}
