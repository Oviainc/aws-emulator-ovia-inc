// SPDX-License-Identifier: CC0-1.0

//! A credential provider over a secret the CALLER manages: the secret has to be readable
//! before the provider is held at all.
//!
//! The rule these tests assert: a credential provider's EXTERNAL secret reference is
//! resolved by the call that DECRYPTS the secret, so a secret whose own KMS key cannot be
//! used to decrypt it refuses the create with the modelled `DecryptionFailure` (HTTP 400)
//! and leaves no provider behind under that name, while a secret that reads fine is held
//! and reported by the ARN its owner answered with. The rule is the SAME for every
//! credential-provider plane of this service, so the rows below span the API key provider
//! and the OAuth2 provider.
//!
//! Secrets Manager and KMS are mounted beside this service on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, so every secret below is a real secret created
//! through Secrets Manager's own operations, sealed under a real KMS key, and every key
//! state change is made through KMS's own operations.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for: a KMS key is regional, and Secrets
/// Manager calls the key in the region the request was addressed to, so the keys, the
/// secrets and the providers below live in the same one.
const REGION: &str = "us-west-2";

/// `SecretSourceType`'s value for a secret the CALLER manages
/// (`smithy_query shape=SecretSourceType`).
const SOURCE_EXTERNAL: &str = "EXTERNAL";

/// The sentence this service refuses a provider with when a referenced secret cannot be
/// decrypted. Entirely fixed text: the remedy does not depend on which secret or which
/// key was at fault.
const UNDECRYPTABLE_MESSAGE: &str =
    "Unable to decrypt the provided secret. Verify KMS key permissions.";

/// Which credential-provider plane a row drives. Both resolve a secret the caller
/// manages, so both are bound by the one rule under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plane {
    ApiKey,
    Oauth2,
}

impl Plane {
    /// The `restJson1` path of this plane's create
    /// (`smithy_query(query="operation", operation="CreateApiKeyCredentialProvider")` /
    /// `operation="CreateOauth2CredentialProvider"`).
    fn create_uri(self) -> &'static str {
        match self {
            Self::ApiKey => "/identities/CreateApiKeyCredentialProvider",
            Self::Oauth2 => "/identities/CreateOauth2CredentialProvider",
        }
    }

    /// The path of this plane's read.
    fn get_uri(self) -> &'static str {
        match self {
            Self::ApiKey => "/identities/GetApiKeyCredentialProvider",
            Self::Oauth2 => "/identities/GetOauth2CredentialProvider",
        }
    }

    /// The HTTP status this plane's create answers on success.
    fn created_status(self) -> u16 {
        201
    }

    /// A create of this plane whose secret is the caller's own `secret_id`.
    fn create_body(self, name: &str, secret_id: &str) -> Value {
        match self {
            Self::ApiKey => json!({
                "name": name,
                "apiKeySecretSource": SOURCE_EXTERNAL,
                "apiKeySecretConfig": {"secretId": secret_id, "jsonKey": "apiKey"},
            }),
            Self::Oauth2 => json!({
                "name": name,
                "credentialProviderVendor": "GoogleOauth2",
                "oauth2ProviderConfigInput": {"googleOauth2ProviderConfig": {
                    "clientId": "an-oauth2-client",
                    "clientSecretSource": SOURCE_EXTERNAL,
                    "clientSecretConfig": {"secretId": secret_id, "jsonKey": "clientSecret"},
                }},
            }),
        }
    }

    /// The member every response of this plane reports the stored secret's ARN under.
    fn secret_arn(self, body: &Value) -> Value {
        match self {
            Self::ApiKey => body["apiKeySecretArn"]["secretArn"].clone(),
            Self::Oauth2 => body["clientSecretArn"]["secretArn"].clone(),
        }
    }
}

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

    /// Create a secret holding `value` under `kms_key_id`, and answer the ARN its owner
    /// holds it at.
    async fn create_secret(
        &self,
        name: &str,
        kms_key_id: Option<&str>,
        value: &str,
        case: &str,
    ) -> String {
        let mut body = json!({
            "Name": name,
            "SecretString": value,
            "ClientRequestToken": client_request_token(name),
        });
        if let Some(kms_key_id) = kms_key_id {
            body["KmsKeyId"] = json!(kms_key_id);
        }
        let created = self.secrets_manager_ok("CreateSecret", &body, case).await;
        created["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateSecret reported no ARN in {created}"))
            .to_string()
    }
}

/// A UUID-shaped `ClientRequestToken` for the secret `name`, so each create of this suite
/// stakes its version on a distinct token of the form Secrets Manager asks for.
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

/// One row of the undecryptable-secret table.
struct UnreadableRow {
    case: &'static str,
    plane: Plane,
    provider: &'static str,
    secret_name: &'static str,
    /// The KMS operation that makes the key unusable, and the extra members it needs.
    kms_operation: &'static str,
    kms_members: Value,
    /// Whether the request references the secret by its whole ARN rather than by name.
    reference_by_arn: bool,
}

/// A referenced secret whose key KMS will not decrypt with refuses the create with
/// `DecryptionFailure`, and nothing is left behind under the name it asked for.
///
/// The rows differ along the credential-provider plane (so along which member of which
/// request shape carries the reference), along the KMS state that refuses the decryption
/// (a disabled key, a key scheduled for deletion) and along whether the caller referenced
/// the secret by name or by its whole ARN: the rule is about the SECRET'S KEY, not about
/// the request's shape.
#[tokio::test]
async fn a_referenced_secret_the_key_will_not_decrypt_refuses_every_provider_create() {
    let rows: [UnreadableRow; 4] = [
        UnreadableRow {
            case: "an API key provider over a disabled key, the secret named",
            plane: Plane::ApiKey,
            provider: "apikey-disabled",
            secret_name: "identities/apikey-disabled",
            kms_operation: "DisableKey",
            kms_members: json!({}),
            reference_by_arn: false,
        },
        UnreadableRow {
            case: "an API key provider over a key scheduled for deletion, the secret by ARN",
            plane: Plane::ApiKey,
            provider: "apikey-pending-deletion",
            secret_name: "identities/apikey-pending-deletion",
            kms_operation: "ScheduleKeyDeletion",
            kms_members: json!({"PendingWindowInDays": 7}),
            reference_by_arn: true,
        },
        UnreadableRow {
            case: "an OAuth2 provider over a disabled key, the secret named",
            plane: Plane::Oauth2,
            provider: "oauth2-disabled",
            secret_name: "identities/oauth2-disabled",
            kms_operation: "DisableKey",
            kms_members: json!({}),
            reference_by_arn: false,
        },
        UnreadableRow {
            case: "an OAuth2 provider over a key scheduled for deletion, the secret by ARN",
            plane: Plane::Oauth2,
            provider: "oauth2-pending-deletion",
            secret_name: "identities/oauth2-pending-deletion",
            kms_operation: "ScheduleKeyDeletion",
            kms_members: json!({"PendingWindowInDays": 30}),
            reference_by_arn: true,
        },
    ];

    for row in rows {
        let UnreadableRow {
            case,
            plane,
            provider,
            secret_name,
            kms_operation,
            kms_members,
            reference_by_arn,
        } = row;
        let emulator = Emulator::new();
        let key_arn = emulator.create_key(case).await;
        let secret_arn = emulator
            .create_secret(secret_name, Some(&key_arn), "s3cr3t-value", case)
            .await;

        // The secret reads fine while the key is usable, so the refusal below is the
        // key's state and nothing else.
        emulator
            .secrets_manager_ok("GetSecretValue", &json!({"SecretId": secret_name}), case)
            .await;

        let mut key_request = kms_members.clone();
        key_request["KeyId"] = json!(key_arn);
        emulator.kms_ok(kms_operation, &key_request, case).await;

        let referenced = if reference_by_arn {
            secret_arn.clone()
        } else {
            secret_name.to_string()
        };
        let (status, refused) = emulator
            .control_plane(plane.create_uri(), &plane.create_body(provider, &referenced))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "DecryptionFailure"),
            "case {case:?}: expected HTTP 400 DecryptionFailure, got {status} ({refused})"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            UNDECRYPTABLE_MESSAGE,
            "case {case:?}: unexpected refusal message, got {refused}"
        );

        let (status, fetched) = emulator
            .control_plane(plane.get_uri(), &json!({"name": provider}))
            .await;
        assert_eq!(
            (status, error_code(&fetched).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused create left no provider behind, got {status} ({fetched})"
        );
    }
}

/// One row of the readable-secret table.
struct ReadableRow {
    case: &'static str,
    plane: Plane,
    provider: &'static str,
    secret_name: &'static str,
    /// The key the secret names, or `None` for the Amazon Web Services managed key a
    /// secret naming no key of its own is held under.
    customer_managed_key: bool,
    reference_by_arn: bool,
}

/// A referenced secret the service CAN read is held, and the provider reports the ARN its
/// OWNER holds it at — not one composed from the name the request referenced.
///
/// The rows differ along the credential-provider plane, along the key the secret is
/// encrypted under (a usable customer managed key, and the managed key a secret naming no
/// key of its own is held under) and along how the request referenced it.
#[tokio::test]
async fn a_referenced_secret_that_reads_is_reported_by_the_arn_its_owner_holds_it_at() {
    let emulator = Emulator::new();
    let usable_key = emulator.create_key("a usable customer managed key").await;

    let rows: [ReadableRow; 4] = [
        ReadableRow {
            case: "an API key provider over a usable customer managed key, the secret named",
            plane: Plane::ApiKey,
            provider: "apikey-usable",
            secret_name: "identities/apikey-usable",
            customer_managed_key: true,
            reference_by_arn: false,
        },
        ReadableRow {
            case: "an API key provider over the service managed key, the secret by ARN",
            plane: Plane::ApiKey,
            provider: "apikey-managed-key",
            secret_name: "identities/apikey-managed-key",
            customer_managed_key: false,
            reference_by_arn: true,
        },
        ReadableRow {
            case: "an OAuth2 provider over a usable customer managed key, the secret named",
            plane: Plane::Oauth2,
            provider: "oauth2-usable",
            secret_name: "identities/oauth2-usable",
            customer_managed_key: true,
            reference_by_arn: false,
        },
        ReadableRow {
            case: "an OAuth2 provider over the service managed key, the secret by ARN",
            plane: Plane::Oauth2,
            provider: "oauth2-managed-key",
            secret_name: "identities/oauth2-managed-key",
            customer_managed_key: false,
            reference_by_arn: true,
        },
    ];

    for row in rows {
        let ReadableRow {
            case,
            plane,
            provider,
            secret_name,
            customer_managed_key,
            reference_by_arn,
        } = row;
        let key = customer_managed_key.then(|| usable_key.clone());
        let owners_arn = emulator
            .create_secret(secret_name, key.as_deref(), "s3cr3t-value", case)
            .await;
        // A created secret's ARN ends in characters that belong to the secret rather than
        // to its name, so the assertion below cannot be satisfied by composing the ARN
        // out of the name the request carried.
        assert!(
            owners_arn.contains(secret_name) && !owners_arn.ends_with(secret_name),
            "case {case:?}: the owner's ARN is expected to carry a suffix of its own beyond \
             the secret's name, so this suite can tell it from one composed out of the name, \
             got {owners_arn}"
        );

        let referenced = if reference_by_arn {
            owners_arn.clone()
        } else {
            secret_name.to_string()
        };
        let (status, created) = emulator
            .control_plane(plane.create_uri(), &plane.create_body(provider, &referenced))
            .await;
        assert_eq!(
            status,
            plane.created_status(),
            "case {case:?}: expected HTTP {} from the create, got {status} ({created})",
            plane.created_status()
        );
        assert_eq!(
            plane.secret_arn(&created),
            json!(owners_arn),
            "case {case:?}: the provider reports the ARN Secrets Manager holds the secret at, \
             got {created}"
        );
        assert!(
            !created.to_string().contains("s3cr3t-value"),
            "case {case:?}: the create carried the referenced secret's value, in {created}"
        );

        // The provider is really held: the read reports the same secret the create did.
        let (status, fetched) = emulator
            .control_plane(plane.get_uri(), &json!({"name": provider}))
            .await;
        assert_eq!(
            (status, plane.secret_arn(&fetched)),
            (200, json!(owners_arn)),
            "case {case:?}: the read reports the secret the create stored, got {status} \
             ({fetched})"
        );
    }
}
