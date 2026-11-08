// SPDX-License-Identifier: CC0-1.0

//! A payment credential provider over a secret the CALLER manages: the secret has to be
//! decryptable before the provider is held at all.
//!
//! The rule these tests assert: `CreatePaymentCredentialProvider` resolves every secret
//! the request references (`*Source` `EXTERNAL`) against AWS Secrets Manager before it
//! persists anything, so a secret whose KMS key will not decrypt refuses the call with
//! the modelled `DecryptionFailure` and leaves no provider behind under that name, while
//! a secret that reads fine is held and reported by its ARN.
//!
//! Secrets Manager and KMS are mounted beside this service on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, so every secret below is a real secret
//! created through Secrets Manager's own operations, sealed under a real KMS key, and
//! every key state change is made through KMS's own operations.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for: a KMS key is regional, and Secrets
/// Manager calls the key in the region the request was addressed to, so the keys, the
/// secrets and the providers below live in the same one.
const REGION: &str = "us-west-2";

/// The canonical account id every emulated ARN carries
/// (`emulator_server::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// `SecretSourceType`'s value for a secret the CALLER manages
/// (`smithy_query shape=SecretSourceType`).
const SOURCE_EXTERNAL: &str = "EXTERNAL";

/// The sentence this service refuses a provider with when a referenced secret cannot be
/// decrypted. Entirely fixed text: the remedy does not depend on which secret or which
/// key was at fault.
const UNDECRYPTABLE_MESSAGE: &str =
    "Unable to decrypt the provided secret. Verify KMS key permissions.";

/// This service, Secrets Manager and KMS over one shared engine, behind the same
/// `MultiServiceRouter` the production router builds.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    /// POST one `awsJson1_1` operation of `service`, signed for [`REGION`].
    async fn json_rpc(
        &self,
        service: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{service}.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        answer(self.router.dispatch(request).await)
    }

    /// POST one `restJson1` operation of this service, signed for [`REGION`].
    async fn control_plane(&self, uri: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", "bedrock-agentcore-control.amazonaws.com")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        answer(self.router.dispatch(request).await)
    }

    /// One KMS operation that must succeed.
    async fn kms_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.json_rpc("kms", "TrentService", operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from KMS {operation}, got {status} ({parsed})"
        );
        parsed
    }

    /// One Secrets Manager operation that must succeed.
    async fn secrets_manager_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self
            .json_rpc("secretsmanager", "secretsmanager", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Secrets Manager {operation}, got {status} \
             ({parsed})"
        );
        parsed
    }

    /// Mint a symmetric KMS key in [`REGION`], answered as its ARN.
    async fn create_key(&self, case: &str) -> String {
        let created = self.kms_ok("CreateKey", &json!({}), case).await;
        created["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateKey reported no Arn in {created}"))
            .to_string()
    }

    /// Create a secret holding `value`, under `kms_key_id` when one is named, and answer
    /// its name.
    async fn create_secret(
        &self,
        name: &str,
        kms_key_id: Option<&str>,
        value: &str,
        case: &str,
    ) -> String {
        // Secrets Manager stakes the version this create stores on a
        // `ClientRequestToken` the caller names ("We recommend a UUID-type value"), so
        // each secret of this suite carries one derived from its own name.
        let mut body = json!({
            "Name": name,
            "SecretString": value,
            "ClientRequestToken": client_request_token(name),
        });
        if let Some(kms_key_id) = kms_key_id {
            body["KmsKeyId"] = json!(kms_key_id);
        }
        self.secrets_manager_ok("CreateSecret", &body, case).await;
        name.to_string()
    }

    /// `CreatePaymentCredentialProvider`: `POST /identities/CreatePaymentCredentialProvider`.
    async fn create_provider(&self, body: &Value) -> (u16, Value) {
        self.control_plane("/identities/CreatePaymentCredentialProvider", body)
            .await
    }

    /// `GetPaymentCredentialProvider`: `POST /identities/GetPaymentCredentialProvider`.
    async fn get_provider(&self, name: &str) -> (u16, Value) {
        self.control_plane(
            "/identities/GetPaymentCredentialProvider",
            &json!({"name": name}),
        )
        .await
    }
}

/// A UUID-shaped `ClientRequestToken` for the secret `name`, so each create of this
/// suite stakes its version on a distinct token of the form Secrets Manager asks for.
fn client_request_token(name: &str) -> String {
    let digits: String = name
        .bytes()
        .map(|byte| format!("{:02x}", byte % 16))
        .collect::<String>()
        .chars()
        .cycle()
        .take(32)
        .collect();
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

fn answer(response: http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A Coinbase CDP configuration whose API key secret is the caller's own secret
/// `secret_id`, and whose wallet secret this service manages.
fn coinbase_with_external_api_key(secret_id: &str) -> Value {
    json!({
        "coinbaseCdpConfiguration": {
            "apiKeyId": "cdp-api-key",
            "apiKeySecretSource": SOURCE_EXTERNAL,
            "apiKeySecretConfig": {"secretId": secret_id, "jsonKey": "apiKeySecret"},
            "walletSecret": "bWFuYWdlZC13YWxsZXQ=",
        }
    })
}

/// A Stripe Privy configuration whose authorization private key is the caller's own
/// secret `secret_id`, and whose app secret this service manages.
fn privy_with_external_private_key(secret_id: &str) -> Value {
    json!({
        "stripePrivyConfiguration": {
            "appId": "privy-app",
            "authorizationId": "privy-authorization",
            "appSecret": "bWFuYWdlZC1hcHAtc2VjcmV0",
            "authorizationPrivateKeySource": SOURCE_EXTERNAL,
            "authorizationPrivateKeyConfig": {
                "secretId": secret_id,
                "jsonKey": "authorizationPrivateKey",
            },
        }
    })
}

/// A referenced secret whose key KMS will not decrypt with refuses the create with
/// `DecryptionFailure`, and nothing is left behind under the name it asked for.
///
/// The rows differ along the vendor (so along which secret member of which union member
/// is the external one), along the KMS state that refuses the decryption (a disabled key,
/// a key scheduled for deletion) and along whether the caller referenced the secret by
/// name or by its whole ARN: the rule is about the SECRET'S KEY, not about the request's
/// shape.
#[tokio::test]
async fn a_referenced_secret_the_key_will_not_decrypt_refuses_the_create() {
    // (case, provider name, secret name, the KMS operation that makes the key unusable
    //  and its extra members, whether the secret is referenced by ARN, the configuration)
    let rows: [(&str, &str, &str, &str, Value, bool); 4] = [
        (
            "Coinbase CDP over a disabled key, the secret named",
            "coinbase-disabled",
            "payments/coinbase-disabled",
            "DisableKey",
            json!({}),
            false,
        ),
        (
            "Coinbase CDP over a key scheduled for deletion, the secret named by ARN",
            "coinbase-pending-deletion",
            "payments/coinbase-pending-deletion",
            "ScheduleKeyDeletion",
            json!({"PendingWindowInDays": 7}),
            true,
        ),
        (
            "Stripe Privy over a disabled key, the secret named",
            "privy-disabled",
            "payments/privy-disabled",
            "DisableKey",
            json!({}),
            false,
        ),
        (
            "Stripe Privy over a key scheduled for deletion, the secret named",
            "privy-pending-deletion",
            "payments/privy-pending-deletion",
            "ScheduleKeyDeletion",
            json!({"PendingWindowInDays": 30}),
            false,
        ),
    ];

    for (case, provider, secret_name, kms_operation, extra, reference_by_arn) in rows {
        let emulator = Emulator::new();
        let key_arn = emulator.create_key(case).await;
        emulator
            .create_secret(secret_name, Some(&key_arn), "s3cr3t-value", case)
            .await;

        // The secret reads fine while the key is usable, so the refusal below is the
        // key's state and nothing else.
        let read = emulator
            .secrets_manager_ok("GetSecretValue", &json!({"SecretId": secret_name}), case)
            .await;
        let secret_arn = read["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the secret reported no ARN in {read}"))
            .to_string();

        let mut key_request = extra.clone();
        key_request["KeyId"] = json!(key_arn);
        emulator.kms_ok(kms_operation, &key_request, case).await;

        let secret_id = if reference_by_arn {
            secret_arn.clone()
        } else {
            secret_name.to_string()
        };
        let configuration = if provider.starts_with("coinbase") {
            coinbase_with_external_api_key(&secret_id)
        } else {
            privy_with_external_private_key(&secret_id)
        };
        let vendor = if provider.starts_with("coinbase") {
            "CoinbaseCDP"
        } else {
            "StripePrivy"
        };

        let (status, answer) = emulator
            .create_provider(&json!({
                "name": provider,
                "credentialProviderVendor": vendor,
                "providerConfigurationInput": configuration,
            }))
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "DecryptionFailure"),
            "case {case:?}: expected HTTP 400 DecryptionFailure, got {status} ({answer})"
        );
        assert_eq!(
            answer["message"].as_str().unwrap_or_default(),
            UNDECRYPTABLE_MESSAGE,
            "case {case:?}: unexpected refusal message, got {answer}"
        );

        let (status, fetched) = emulator.get_provider(provider).await;
        assert_eq!(
            (status, error_code(&fetched).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused create left no provider behind, got {status} ({fetched})"
        );
    }
}

/// A referenced secret the service CAN read is held, and the provider reports the ARN its
/// OWNER holds it at — not one composed from the name the request referenced.
///
/// The rows differ along the key the secret is encrypted under (a customer managed key
/// that is usable, and the Amazon Web Services managed key a secret naming no key of its
/// own is held under), along the vendor whose secret it is, and along how the request
/// referenced the secret (by name, which only its owner can resolve, and by the whole ARN
/// the owner reported): a readable secret is never what the refusal above is about.
/// One row of the readable-secret table: a provider over a secret the service can read.
struct ReadableRow {
    case: &'static str,
    provider: &'static str,
    secret_name: &'static str,
    /// The key the secret names, or `None` for the Amazon Web Services managed key a
    /// secret naming no key of its own is held under.
    kms_key_id: Option<String>,
    vendor: &'static str,
    /// Whether the request references the secret by its whole ARN rather than by name.
    reference_by_arn: bool,
}

#[tokio::test]
async fn a_referenced_secret_that_reads_is_reported_by_the_arn_its_owner_holds_it_at() {
    let emulator = Emulator::new();
    let usable_key = emulator.create_key("a usable customer managed key").await;

    let rows: [ReadableRow; 4] = [
        ReadableRow {
            case: "Coinbase CDP over a usable customer managed key, the secret named",
            provider: "coinbase-usable",
            secret_name: "payments/coinbase-usable",
            kms_key_id: Some(usable_key.clone()),
            vendor: "CoinbaseCDP",
            reference_by_arn: false,
        },
        ReadableRow {
            case: "Coinbase CDP over the service managed key, the secret named",
            provider: "coinbase-managed-key",
            secret_name: "payments/coinbase-managed-key",
            kms_key_id: None,
            vendor: "CoinbaseCDP",
            reference_by_arn: false,
        },
        ReadableRow {
            case: "Stripe Privy over a usable customer managed key, the secret named",
            provider: "privy-usable",
            secret_name: "payments/privy-usable",
            kms_key_id: Some(usable_key.clone()),
            vendor: "StripePrivy",
            reference_by_arn: false,
        },
        ReadableRow {
            case: "Stripe Privy over the service managed key, the secret named by its ARN",
            provider: "privy-by-arn",
            secret_name: "payments/privy-by-arn",
            kms_key_id: None,
            vendor: "StripePrivy",
            reference_by_arn: true,
        },
    ];

    for ReadableRow {
        case,
        provider,
        secret_name,
        kms_key_id,
        vendor,
        reference_by_arn,
    } in rows
    {
        emulator
            .create_secret(secret_name, kms_key_id.as_deref(), "s3cr3t-value", case)
            .await;
        // The ARN the OWNER holds the secret at, which is what the provider must report:
        // a created secret's ARN ends in characters that belong to the secret rather than
        // to its name, so it cannot be composed from the name the request carried.
        let read = emulator
            .secrets_manager_ok("GetSecretValue", &json!({"SecretId": secret_name}), case)
            .await;
        let owners_arn = read["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the secret reported no ARN in {read}"))
            .to_string();
        assert_ne!(
            owners_arn,
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{secret_name}"),
            "case {case:?}: the owner's ARN is expected to carry a suffix of its own, so this \
             suite can tell it from one composed out of the name"
        );

        let referenced = if reference_by_arn {
            owners_arn.clone()
        } else {
            secret_name.to_string()
        };
        let configuration = if vendor == "CoinbaseCDP" {
            coinbase_with_external_api_key(&referenced)
        } else {
            privy_with_external_private_key(&referenced)
        };

        let (status, created) = emulator
            .create_provider(&json!({
                "name": provider,
                "credentialProviderVendor": vendor,
                "providerConfigurationInput": configuration,
            }))
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreatePaymentCredentialProvider, got {status} \
             ({created})"
        );

        let configuration_output = &created["providerConfigurationOutput"];
        let reported = if vendor == "CoinbaseCDP" {
            configuration_output["coinbaseCdpConfiguration"]["apiKeySecretArn"]["secretArn"].clone()
        } else {
            configuration_output["stripePrivyConfiguration"]["authorizationPrivateKeyArn"]
                ["secretArn"]
                .clone()
        };
        assert_eq!(
            reported,
            json!(owners_arn),
            "case {case:?}: the provider reports the ARN Secrets Manager holds the secret at, \
             got {created}"
        );
        assert!(
            !created.to_string().contains("s3cr3t-value"),
            "case {case:?}: the create carried the referenced secret's value, in {created}"
        );

        let (status, fetched) = emulator.get_provider(provider).await;
        assert_eq!(
            (status, fetched["providerConfigurationOutput"].clone()),
            (200, configuration_output.clone()),
            "case {case:?}: the read reports the configuration the create stored, got {fetched}"
        );
    }
}

/// A secret referenced by a NAME its owner holds no secret under is the modelled
/// `ResourceNotFoundException`, and nothing is left behind under the provider's name.
///
/// The rows differ along the vendor whose secret is referenced and along the shape of the
/// name — including one that only differs from a secret this account really holds — because
/// the rule is about what AWS Secrets Manager answers for the name, not about its spelling.
#[tokio::test]
async fn a_referenced_name_no_secret_exists_under_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_secret(
            "payments/present",
            None,
            "s3cr3t-value",
            "the one secret this account holds",
        )
        .await;

    // (case, provider name, the name referenced, the vendor)
    let rows: [(&str, &str, &str, &str); 4] = [
        (
            "Coinbase CDP referencing a name nothing was created under",
            "coinbase-absent",
            "payments/absent",
            "CoinbaseCDP",
        ),
        (
            "Coinbase CDP referencing a name that only looks like the stored one",
            "coinbase-near-miss",
            "payments/present2",
            "CoinbaseCDP",
        ),
        (
            "Stripe Privy referencing the stored name in another case",
            "privy-wrong-case",
            "payments/PRESENT",
            "StripePrivy",
        ),
        (
            "Stripe Privy referencing a name nothing was created under",
            "privy-absent",
            "privy/absent",
            "StripePrivy",
        ),
    ];

    for (case, provider, referenced, vendor) in rows {
        let configuration = if vendor == "CoinbaseCDP" {
            coinbase_with_external_api_key(referenced)
        } else {
            privy_with_external_private_key(referenced)
        };

        let (status, answer) = emulator
            .create_provider(&json!({
                "name": provider,
                "credentialProviderVendor": vendor,
                "providerConfigurationInput": configuration,
            }))
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} ({answer})"
        );

        let (status, fetched) = emulator.get_provider(provider).await;
        assert_eq!(
            (status, error_code(&fetched).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused create left no provider behind, got {status} ({fetched})"
        );
    }

    // The same reference, to the secret this account DOES hold, is served.
    let (status, created) = emulator
        .create_provider(&json!({
            "name": "coinbase-present",
            "credentialProviderVendor": "CoinbaseCDP",
            "providerConfigurationInput": coinbase_with_external_api_key("payments/present"),
        }))
        .await;
    assert_eq!(
        status, 201,
        "a reference to the secret this account holds is served, got {status} ({created})"
    );
}
