// SPDX-License-Identifier: CC0-1.0

//! The credential a `CreateLocationAzureBlob` request has to carry, driven end to end
//! over HTTP.
//!
//! An Azure Blob Storage location that authenticates with a shared access signature
//! reaches its container only with that signature, so the request must say where the
//! signature is: inline in `SasConfiguration`, or in the Secrets Manager secret
//! `CustomSecretConfig` names (with `CmkSecretConfig` naming the key of the secret
//! DataSync manages for an inline token). A request that selects that authentication
//! method and names no credential at all is refused, and persists nothing — which the
//! final `ListLocations` here is what proves.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// A shared access signature token, shaped like the one the modelled `AzureBlobSasToken`
/// member documents.
const SAS_TOKEN: &str = "sp=r&st=2026-03-01T00:00:00Z&se=2026-03-02T00:00:00Z&sv=2021-06-08&sr=c&sig=ZXhhbXBsZQ%3D%3D";

/// A customer-managed KMS key, as the modelled `KmsKeyArn` member spells one.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// A Secrets Manager secret the caller manages, as the modelled `SecretArn` spells one.
const CUSTOMER_SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:azure-blob-token-AbCdEf";

/// The IAM role that grants DataSync access to [`CUSTOMER_SECRET_ARN`].
const SECRET_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/datasync-secret-reader";

/// The class discriminator both code members of the modelled client error carry when
/// DataSync refuses a request over the parameters it names.
const INVALID_PARAMETER: &str = "InvalidParameter";

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("FmrsService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{SIGNED_REGION}\
                 /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|_| {
            serde_json::json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}

/// Which credential members a request carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Credential {
    /// None of the three credential members.
    Absent,
    /// An inline SAS token and nothing else.
    InlineToken,
    /// An inline SAS token plus the customer-managed key of the secret DataSync manages
    /// for it.
    InlineTokenWithCustomerKey,
    /// A Secrets Manager secret the caller manages, named directly.
    CustomerSecret,
    /// The customer-managed key of a managed secret, with no token for it to hold.
    CustomerKeyOnly,
}

impl Credential {
    /// The request members this credential source contributes.
    fn apply(self, request: &mut serde_json::Value) {
        let token = serde_json::json!({ "Token": SAS_TOKEN });
        let cmk = serde_json::json!({ "KmsKeyArn": KMS_KEY_ARN });
        let custom = serde_json::json!({
            "SecretArn": CUSTOMER_SECRET_ARN,
            "SecretAccessRoleArn": SECRET_ACCESS_ROLE_ARN,
        });
        match self {
            Self::Absent => {}
            Self::InlineToken => request["SasConfiguration"] = token,
            Self::InlineTokenWithCustomerKey => {
                request["SasConfiguration"] = token;
                request["CmkSecretConfig"] = cmk;
            }
            Self::CustomerSecret => request["CustomSecretConfig"] = custom,
            Self::CustomerKeyOnly => request["CmkSecretConfig"] = cmk,
        }
    }
}

/// What a row expects of the create.
#[derive(Clone, Copy, Debug)]
enum Outcome {
    /// The location is created, and reports the authentication method it was created
    /// with.
    Created,
    /// The request is refused with HTTP 400 `InvalidRequestException`, both code members
    /// carrying [`INVALID_PARAMETER`], and the message given here in full.
    Refused(&'static str),
}

/// One `CreateLocationAzureBlob` request and the outcome the credential rule gives it.
#[derive(Clone, Copy, Debug)]
struct Row {
    case: &'static str,
    container: &'static str,
    authentication_type: &'static str,
    credential: Credential,
    outcome: Outcome,
}

/// DataSync's own wording of the refusal a shared-access-signature location without a
/// credential earns. The whole sentence is fixed text for this refusal class — including
/// the `Invalid parameter: ` prefix every parameter refusal carries and the doubled
/// terminal period.
const MISSING_CREDENTIAL: &str = "Invalid parameter: SasConfiguration or CustomConfiguration \
     needs to be provided when authentication type is SAS for Azure Blob locations..";

/// The authentication method decides whether a credential is required, and the credential
/// members decide where the credential is: a request that selects SAS without naming one
/// is refused and stores nothing, while every documented way of naming one is accepted.
///
/// The rows span the two modelled `AzureBlobAuthenticationType` values (`SAS | NONE`,
/// from `smithy_query(query="shape", shape="AzureBlobAuthenticationType")`) against the
/// credential members the operation takes: none of them, an inline token, an inline token
/// with a customer-managed key, a secret the caller manages, and a customer-managed key
/// with no token for it to hold.
#[tokio::test]
async fn azure_blob_sas_authentication_requires_a_credential() {
    let rows = [
        Row {
            case: "sas authentication with no credential member at all",
            container: "https://sasnocred.blob.core.windows.net/transfer-in",
            authentication_type: "SAS",
            credential: Credential::Absent,
            outcome: Outcome::Refused(MISSING_CREDENTIAL),
        },
        Row {
            case: "sas authentication with an inline token",
            container: "https://sasinline.blob.core.windows.net/transfer-in",
            authentication_type: "SAS",
            credential: Credential::InlineToken,
            outcome: Outcome::Created,
        },
        Row {
            case: "sas authentication with an inline token under a customer key",
            container: "https://sascmk.blob.core.windows.net/transfer-in",
            authentication_type: "SAS",
            credential: Credential::InlineTokenWithCustomerKey,
            outcome: Outcome::Created,
        },
        Row {
            case: "sas authentication by reference to a secret the caller manages",
            container: "https://sascustom.blob.core.windows.net/transfer-in",
            authentication_type: "SAS",
            credential: Credential::CustomerSecret,
            outcome: Outcome::Created,
        },
        Row {
            case: "sas authentication with a customer key but no token for the secret",
            container: "https://sascmkonly.blob.core.windows.net/transfer-in",
            authentication_type: "SAS",
            credential: Credential::CustomerKeyOnly,
            outcome: Outcome::Refused(
                "You can use either CmkSecretConfig (with SasConfiguration) or \
                 CustomSecretConfig (without SasConfiguration) to provide credentials for a \
                 CreateLocationAzureBlob request. Do not provide CmkSecretConfig without \
                 SasConfiguration.",
            ),
        },
        Row {
            case: "no authentication, so no credential is required",
            container: "https://noauth.blob.core.windows.net/public-container",
            authentication_type: "NONE",
            credential: Credential::Absent,
            outcome: Outcome::Created,
        },
    ];

    let router = service();
    let mut created: Vec<String> = Vec::new();

    for row in &rows {
        let mut request = serde_json::json!({
            "ContainerUrl": row.container,
            "AuthenticationType": row.authentication_type,
        });
        row.credential.apply(&mut request);
        let case = row.case;
        let (status, body) = call(&router, "CreateLocationAzureBlob", request).await;

        match row.outcome {
            Outcome::Created => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to answer 200, got {status} with {body}"
                );
                let arn = body["LocationArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a LocationArn, got {body}")
                    })
                    .to_string();

                // Read the location back through the describe, so the create is only
                // accepted when it really persisted the location it reported.
                let (describe_status, described) = call(
                    &router,
                    "DescribeLocationAzureBlob",
                    serde_json::json!({ "LocationArn": arn }),
                )
                .await;
                assert_eq!(
                    describe_status, 200,
                    "case {case:?}: expected the describe to answer 200, got \
                     {describe_status} with {described}"
                );
                assert_eq!(
                    described["AuthenticationType"].as_str(),
                    Some(row.authentication_type),
                    "case {case:?}: expected the authentication method the request named, \
                     got {described}"
                );
                created.push(arn);
            }
            Outcome::Refused(expected_message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the create to answer 400, got {status} with {body}"
                );
                let wire_type = body["__type"].as_str().unwrap_or_default();
                assert!(
                    wire_type.contains("InvalidRequestException"),
                    "case {case:?}: expected an InvalidRequestException, got {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(expected_message),
                    "case {case:?}: expected DataSync's own refusal wording, got {body}"
                );
                for member in ["errorCode", "datasyncErrorCode"] {
                    assert_eq!(
                        body[member].as_str(),
                        Some(INVALID_PARAMETER),
                        "case {case:?}: expected {member} to classify the refusal, got {body}"
                    );
                }
                assert!(
                    body["LocationArn"].is_null(),
                    "case {case:?}: a refused create mints no location, got {body}"
                );
            }
        }
    }

    // Only the accepted rows are held: a refused create persists nothing, so the store
    // lists exactly the locations the table expected to be created.
    let (status, listed) = call(&router, "ListLocations", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListLocations to answer 200, got {status} with {listed}"
    );
    let held: Vec<String> = listed["Locations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Locations list, got {listed}"))
        .iter()
        .map(|entry| entry["LocationArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        held, created,
        "expected only the accepted creates to be held, got {listed}"
    );
}
