// SPDX-License-Identifier: CC0-1.0

//! Which SECRETS a cluster's SASL/SCRAM credentials may live in, decided PER SECRET
//! against the service that owns them — AWS Secrets Manager, mounted beside this one on
//! the same engine (`kafka_harness::Emulator::with_secrets_manager`).
//!
//! RULE under test: `BatchAssociateScramSecret` decides each entry of its batch on its
//! own, so a secret this endpoint's Secrets Manager cannot serve a SASL/SCRAM credential
//! out of — one it holds no secret for in the endpoint's Region, or one encrypted with
//! the Amazon Web Services managed Secrets Manager key rather than with a customer
//! managed KMS key — comes back in `UnprocessedScramSecrets` named by its own
//! `SecretArn`, with a non-empty `ErrorCode` and an `ErrorMessage`, while the call
//! answers HTTP 200, stores every other secret of the same batch, and never stores the
//! unprocessed one: `ListScramSecrets` reports the stored set and nothing else.
//!
//! The rows differ along what the rule generalises over: HOW the secret names its
//! encryption key (a key ARN, a bare key id, an alias of its own, no key at all, the
//! managed key's alias with and without the `alias/` prefix), whether Secrets Manager
//! holds the secret at all (never created, or created in another Region), how many
//! secrets the batch carries and in which order the inadmissible one sits, and whether
//! the batch repeats one.
//!
//! Every secret a row names is MINTED through Secrets Manager's own `CreateSecret`, so
//! what this suite asserts rests on what that service holds rather than on a value
//! written out here.

use serde_json::{json, Value};

use crate::kafka_harness::{pct, sleep_past_activation, ClusterRequest, Emulator, Reply, REGION};

/// The Region this suite's cluster and its secrets live in — one row creates its secret
/// in another Region, which is what [`OTHER_REGION`] is for.
const OTHER_REGION: &str = "us-east-2";

/// The `ErrorCode` a secret ARN Secrets Manager holds no secret for comes back with, and
/// the sentence beside it. Written out here rather than read from the service's own
/// constants, so the pair is pinned by this suite instead of by the code under test.
const ABSENT_CODE: &str = "InvalidSecretArn";
const ABSENT_MESSAGE: &str = "The provided secret ARN is invalid.";

/// The same for a secret encrypted with the Amazon Web Services managed Secrets Manager
/// key instead of a customer managed KMS key.
const MANAGED_KEY_CODE: &str = "InvalidSecretKmsKeyId";
const MANAGED_KEY_MESSAGE: &str = "The secret must be encrypted with a customer managed AWS KMS key.";

/// A customer managed KMS key named by its full ARN — one of the three forms a
/// `KmsKeyId` takes ("The ARN, key ID, or alias of the KMS key", Secrets Manager's
/// `CreateSecretRequest$KmsKeyId`).
const CUSTOMER_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/9b1dca1e-7c4f-4a6e-8f0a-5b2d7c3e1f40";

/// The same key named by its bare id, and by an alias of its own.
const CUSTOMER_KEY_ID: &str = "9b1dca1e-7c4f-4a6e-8f0a-5b2d7c3e1f40";
const CUSTOMER_KEY_ALIAS: &str = "alias/msk-scram-credentials";

/// The resource URI all three SCRAM-secret operations are bound to.
fn secrets_uri(arn: &str) -> String {
    format!("/v1/clusters/{}/scram-secrets", pct(arn))
}

async fn associate(emulator: &Emulator, cluster: &str, secrets: &[String]) -> Reply {
    emulator
        .call(
            "POST",
            &secrets_uri(cluster),
            Some(json!({ "secretArnList": secrets })),
        )
        .await
}

async fn list(emulator: &Emulator, cluster: &str) -> Reply {
    emulator.call("GET", &secrets_uri(cluster), None).await
}

/// The secrets `ListScramSecrets` reports for `cluster`.
async fn listed(emulator: &Emulator, cluster: &str, case: &str) -> Vec<String> {
    let reply = list(emulator, cluster).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected ListScramSecrets to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/secretArnList")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Mint a secret through Secrets Manager's own `CreateSecret` in `region` and answer the
/// ARN that service reports for it.
///
/// `kms_key_id` is the member as the row names it: `None` for a request that omits it,
/// which is the secret left on the Amazon Web Services managed key.
async fn create_secret(
    emulator: &Emulator,
    region: &str,
    name: &str,
    kms_key_id: Option<&str>,
) -> String {
    let mut body = json!({
        "Name": name,
        // A raw request generates its own idempotency token, as an SDK would
        // (`CreateSecretRequest$ClientRequestToken`).
        "ClientRequestToken": format!("00000000-0000-4000-8000-{:012x}", name.len() * 7919),
        "SecretString": "{\"username\":\"scram-user\",\"password\":\"scram-Secret-99\"}",
    });
    if let Some(key) = kms_key_id {
        body["KmsKeyId"] = json!(key);
    }
    let reply = emulator.secrets_manager(region, "CreateSecret", body).await;
    assert_eq!(
        reply.status, 200,
        "setup {name:?}: Secrets Manager must create the secret, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/ARN")
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: Secrets Manager reported no ARN: {}", reply.body))
        .to_string()
}

/// A cluster that authenticates SASL/SCRAM clients, settled past the emulator's
/// `CREATING` -> `ACTIVE` span so its secret set can be acted on.
async fn scram_cluster(emulator: &Emulator, name: &str) -> String {
    let arn = emulator
        .create_ok(&ClusterRequest {
            sasl_scram: Some(true),
            ..ClusterRequest::named(name)
        })
        .await;
    sleep_past_activation();
    arn
}

/// The `unprocessedScramSecrets` entries of an association, as (secretArn, errorCode,
/// errorMessage) triples in the order the response reports them.
fn unprocessed(reply: &Reply) -> Vec<(String, String, String)> {
    reply
        .at("/unprocessedScramSecrets")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            (member("secretArn"), member("errorCode"), member("errorMessage"))
        })
        .collect()
}

/// One row: a secret Secrets Manager was asked to create (or not), and what the
/// association must answer for it.
struct Row {
    case: &'static str,
    /// The `KmsKeyId` the secret is created with, or `None` for a request that names
    /// none; ignored when the secret is never created.
    kms_key_id: Option<&'static str>,
    /// The Region the secret is created in.
    region: &'static str,
    /// `false` for a row whose secret Secrets Manager never holds: the ARN is still a
    /// well-formed secret ARN of this Region, it just names nothing.
    created: bool,
    /// The `ErrorCode` and `ErrorMessage` the entry must carry, or `None` when the
    /// secret must be associated instead.
    refusal: Option<(&'static str, &'static str)>,
}

/// Every form of secret the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a key of its own, named by ARN",
            kms_key_id: Some(CUSTOMER_KEY_ARN),
            region: REGION,
            created: true,
            refusal: None,
        },
        Row {
            case: "a key of its own, named by id",
            kms_key_id: Some(CUSTOMER_KEY_ID),
            region: REGION,
            created: true,
            refusal: None,
        },
        Row {
            case: "a key of its own, named by alias",
            kms_key_id: Some(CUSTOMER_KEY_ALIAS),
            region: REGION,
            created: true,
            refusal: None,
        },
        Row {
            case: "no key named at all, so the managed key",
            kms_key_id: None,
            region: REGION,
            created: true,
            refusal: Some((MANAGED_KEY_CODE, MANAGED_KEY_MESSAGE)),
        },
        Row {
            case: "the managed key named by its alias",
            kms_key_id: Some("alias/aws/secretsmanager"),
            region: REGION,
            created: true,
            refusal: Some((MANAGED_KEY_CODE, MANAGED_KEY_MESSAGE)),
        },
        Row {
            case: "the managed key named without the alias prefix",
            kms_key_id: Some("aws/secretsmanager"),
            region: REGION,
            created: true,
            refusal: Some((MANAGED_KEY_CODE, MANAGED_KEY_MESSAGE)),
        },
        Row {
            case: "the empty string, which names no key either",
            kms_key_id: Some(""),
            region: REGION,
            created: true,
            refusal: Some((MANAGED_KEY_CODE, MANAGED_KEY_MESSAGE)),
        },
        Row {
            case: "a secret Secrets Manager never held",
            kms_key_id: Some(CUSTOMER_KEY_ARN),
            region: REGION,
            created: false,
            refusal: Some((ABSENT_CODE, ABSENT_MESSAGE)),
        },
        Row {
            case: "a secret of another Region",
            kms_key_id: Some(CUSTOMER_KEY_ARN),
            region: OTHER_REGION,
            created: true,
            refusal: Some((ABSENT_CODE, ABSENT_MESSAGE)),
        },
    ]
}

/// The ARN the row's secret is named by: the one Secrets Manager minted, or — for the
/// row whose secret was never created — a well-formed secret ARN of this Region naming
/// nothing.
async fn arn_of(emulator: &Emulator, row: &Row, name: &str) -> String {
    if row.created {
        return create_secret(emulator, row.region, name, row.kms_key_id).await;
    }
    format!("arn:aws:secretsmanager:{REGION}:000000000000:secret:{name}-aBcDeF")
}

/// RULE: each secret of a batch is decided on its own — an admissible one is stored and
/// listed, an inadmissible one comes back unprocessed with a code and a message and is
/// not stored, and either way the call answers 200 naming the cluster.
#[tokio::test]
async fn each_secret_of_a_batch_is_admitted_on_its_own() {
    let emulator = Emulator::with_secrets_manager();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let cluster = scram_cluster(&emulator, &format!("scram-admission-{index}")).await;
        let secret = arn_of(&emulator, row, &format!("AmazonMSK_admission_{index}")).await;

        let reply = associate(&emulator, &cluster, std::slice::from_ref(&secret)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected BatchAssociateScramSecret to answer 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/clusterArn").as_str(),
            Some(cluster.as_str()),
            "case {case:?}: expected the answer to name the cluster, got {}",
            reply.body
        );

        let reported = unprocessed(&reply);
        let stored = listed(&emulator, &cluster, case).await;
        match row.refusal {
            None => {
                assert!(
                    reported.is_empty(),
                    "case {case:?}: expected a secret on a key of its own to be associated, got \
                     unprocessed {reported:?}"
                );
                assert_eq!(
                    stored,
                    vec![secret.clone()],
                    "case {case:?}: expected the listing to report the associated secret, got \
                     {stored:?}"
                );
            }
            Some((code, message)) => {
                assert_eq!(
                    reported,
                    vec![(secret.clone(), code.to_string(), message.to_string())],
                    "case {case:?}: expected the secret to come back unprocessed with {code:?}, \
                     got {reported:?}"
                );
                assert!(
                    stored.is_empty(),
                    "case {case:?}: expected an unprocessed secret not to be stored, got {stored:?}"
                );
            }
        }
    }
}

/// One row of the mixed-batch table: the batch as submitted, the secrets it must store
/// and the ones that must come back unprocessed.
struct MixedBatch {
    case: &'static str,
    submitted: Vec<String>,
    admitted: Vec<String>,
    refused: Vec<String>,
}

/// RULE: the outcomes are ISOLATED per item — a mixed batch stores every admissible
/// secret it names and reports only the inadmissible ones, once each, whatever their
/// order in the batch and however often the batch repeats one.
#[tokio::test]
async fn a_mixed_batch_stores_the_admissible_secrets_and_reports_only_the_rest() {
    let emulator = Emulator::with_secrets_manager();

    let good_first = create_secret(
        &emulator,
        REGION,
        "AmazonMSK_mixed_good_1",
        Some(CUSTOMER_KEY_ARN),
    )
    .await;
    let good_second = create_secret(
        &emulator,
        REGION,
        "AmazonMSK_mixed_good_2",
        Some(CUSTOMER_KEY_ALIAS),
    )
    .await;
    let managed_key = create_secret(&emulator, REGION, "AmazonMSK_mixed_managed", None).await;
    let absent =
        format!("arn:aws:secretsmanager:{REGION}:000000000000:secret:AmazonMSK_mixed_absent-aBcDeF");

    let batches = [
        MixedBatch {
            case: "the inadmissible secret last",
            submitted: vec![good_first.clone(), managed_key.clone()],
            admitted: vec![good_first.clone()],
            refused: vec![managed_key.clone()],
        },
        MixedBatch {
            case: "the inadmissible secret first",
            submitted: vec![absent.clone(), good_first.clone()],
            admitted: vec![good_first.clone()],
            refused: vec![absent.clone()],
        },
        MixedBatch {
            case: "two inadmissible secrets among two admissible ones",
            submitted: vec![
                managed_key.clone(),
                good_first.clone(),
                absent.clone(),
                good_second.clone(),
            ],
            admitted: vec![good_first.clone(), good_second.clone()],
            refused: vec![managed_key.clone(), absent.clone()],
        },
        MixedBatch {
            case: "the same inadmissible secret three times",
            submitted: vec![
                managed_key.clone(),
                good_second.clone(),
                managed_key.clone(),
                managed_key.clone(),
            ],
            admitted: vec![good_second.clone()],
            refused: vec![managed_key.clone()],
        },
    ];

    for (
        index,
        MixedBatch {
            case,
            submitted,
            admitted,
            refused,
        },
    ) in batches.iter().enumerate()
    {
        let cluster = scram_cluster(&emulator, &format!("scram-mixed-{index}")).await;

        let reply = associate(&emulator, &cluster, submitted).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected a mixed batch to answer 200, got {} {}",
            reply.status, reply.body
        );
        let reported: Vec<String> = unprocessed(&reply)
            .into_iter()
            .map(|(secret_arn, code, message)| {
                assert!(
                    !code.is_empty() && !message.is_empty(),
                    "case {case:?}: expected {secret_arn} to carry a code and a message, got \
                     {code:?} and {message:?}"
                );
                secret_arn
            })
            .collect();
        assert_eq!(
            &reported, refused,
            "case {case:?}: expected exactly the inadmissible secrets to come back, got \
             {reported:?}"
        );

        let mut stored = listed(&emulator, &cluster, case).await;
        stored.sort();
        let mut expected = admitted.clone();
        expected.sort();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the listing to report the admitted secrets, got {stored:?}"
        );
    }
}

/// RULE: a secret that becomes admissible is admitted on the next call — the admission
/// is a question asked of Secrets Manager each time, not a verdict remembered from the
/// first batch. Pointing the secret at a key of its own through Secrets Manager's own
/// `UpdateSecret` is what makes it admissible.
#[tokio::test]
async fn a_secret_moved_onto_a_key_of_its_own_is_admitted_on_the_next_call() {
    let emulator = Emulator::with_secrets_manager();
    let cluster = scram_cluster(&emulator, "scram-rekeyed").await;
    let secret = create_secret(&emulator, REGION, "AmazonMSK_rekeyed", None).await;

    let refused = associate(&emulator, &cluster, std::slice::from_ref(&secret)).await;
    assert_eq!(
        unprocessed(&refused)
            .into_iter()
            .map(|(secret_arn, ..)| secret_arn)
            .collect::<Vec<String>>(),
        vec![secret.clone()],
        "case \"on the managed key\": expected the secret to come back unprocessed, got {}",
        refused.body
    );

    let updated = emulator
        .secrets_manager(
            REGION,
            "UpdateSecret",
            json!({ "SecretId": secret, "KmsKeyId": CUSTOMER_KEY_ARN }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "setup: Secrets Manager must point the secret at a key of its own, got {} {}",
        updated.status, updated.body
    );

    let accepted = associate(&emulator, &cluster, std::slice::from_ref(&secret)).await;
    assert!(
        unprocessed(&accepted).is_empty(),
        "case \"moved onto a key of its own\": expected the secret to be associated, got {}",
        accepted.body
    );
    let stored = listed(&emulator, &cluster, "moved onto a key of its own").await;
    assert_eq!(
        stored,
        vec![secret],
        "case \"moved onto a key of its own\": expected the listing to report it, got {stored:?}"
    );
}
