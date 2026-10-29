// SPDX-License-Identifier: CC0-1.0

//! Azure Blob Storage transfer locations, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router. Nothing reaches into the state struct, so a response that is
//! not computed from persisted state cannot pass.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. It is deliberately NOT the
/// emulator's default, so an ARN that carries it proves the region came from the
/// request's SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A shared access signature token, shaped like the one the modelled
/// `AzureBlobSasToken` member documents. It is sensitive, so no response may carry it.
const SAS_TOKEN: &str = "sp=r&st=2026-01-01T00:00:00Z&se=2026-01-02T00:00:00Z&sv=2021-06-08&sr=c&sig=ZXhhbXBsZQ%3D%3D";

/// A customer-managed KMS key, as the modelled `KmsKeyArn` member spells one.
const KMS_KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// A Secrets Manager secret the caller manages, as the modelled `SecretArn` spells one.
const CUSTOMER_SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:azure-blob-token-AbCdEf";

/// The IAM role that grants DataSync access to [`CUSTOMER_SECRET_ARN`].
const SECRET_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/datasync-secret-reader";

/// Two DataSync agents, as the modelled `AgentArn` spells them.
const AGENT_ARNS: [&str; 2] = [
    "arn:aws:datasync:us-west-2:000000000000:agent/agent-01234567890abcdef",
    "arn:aws:datasync:us-west-2:000000000000:agent/agent-abcdef01234567890",
];

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    service_with_engine().0
}

/// The same router, with the engine its state lives in — for a case that has to arrange
/// a store too large to fill one request at a time.
fn service_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
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

/// A call that must succeed, returning its body.
async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    case: &str,
) -> serde_json::Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 `InvalidRequestException` whose message
/// contains `expected_message`.
async fn invalid_request(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    expected_message: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("InvalidRequestException"),
        "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
    );
    let message = parsed["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(expected_message),
        "case {case:?}: expected the refusal to say {expected_message:?}, got {message:?}"
    );
}

/// A required string member of a response body.
fn string(body: &serde_json::Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The modelled `LocationArn` shape: `…:location/loc-[0-9a-z]{17}`.
fn assert_location_arn(arn: &str, case: &str) {
    let prefix = format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-");
    let id = arn.strip_prefix(&prefix).unwrap_or_default();
    assert!(
        id.len() == 17 && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()),
        "case {case:?}: expected a {prefix}<17 lowercase alphanumerics> ARN, got {arn:?}"
    );
}

/// Which credential source a request carries — what decides which one of the three
/// secret members `DescribeLocationAzureBlob` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Credential {
    /// An inline SAS token and nothing else: DataSync keeps it in a secret it manages
    /// with the Amazon Web Services managed key.
    SasToken,
    /// An inline SAS token plus the customer-managed KMS key for the managed secret.
    SasTokenWithCustomerKey,
    /// A Secrets Manager secret the caller manages, named directly.
    CustomerSecret,
    /// No credential at all.
    None,
}

impl Credential {
    /// The request members this credential source contributes.
    fn apply(self, request: &mut serde_json::Value) {
        match self {
            Self::SasToken => {
                request["SasConfiguration"] = serde_json::json!({ "Token": SAS_TOKEN });
            }
            Self::SasTokenWithCustomerKey => {
                request["SasConfiguration"] = serde_json::json!({ "Token": SAS_TOKEN });
                request["CmkSecretConfig"] = serde_json::json!({ "KmsKeyArn": KMS_KEY_ARN });
            }
            Self::CustomerSecret => {
                request["CustomSecretConfig"] = serde_json::json!({
                    "SecretArn": CUSTOMER_SECRET_ARN,
                    "SecretAccessRoleArn": SECRET_ACCESS_ROLE_ARN,
                });
            }
            Self::None => {}
        }
    }

    /// The secret ARN the describe reports for this credential source, checked against
    /// the member that is supposed to carry it, with the other two absent.
    fn assert_reported(self, described: &serde_json::Value, case: &str) -> Option<String> {
        let managed = &described["ManagedSecretConfig"];
        let cmk = &described["CmkSecretConfig"];
        let custom = &described["CustomSecretConfig"];
        let minted_prefix = format!("arn:aws:secretsmanager:{SIGNED_REGION}:{ACCOUNT}:secret:");

        match self {
            Self::SasToken => {
                let secret = string(managed, "SecretArn", case);
                assert!(
                    secret.starts_with(&minted_prefix),
                    "case {case:?}: expected a DataSync-managed secret under {minted_prefix:?}, \
                     got {secret:?}"
                );
                assert!(
                    cmk.is_null() && custom.is_null(),
                    "case {case:?}: expected only ManagedSecretConfig, got {described}"
                );
                Some(secret)
            }
            Self::SasTokenWithCustomerKey => {
                let secret = string(cmk, "SecretArn", case);
                assert!(
                    secret.starts_with(&minted_prefix),
                    "case {case:?}: expected a DataSync-managed secret under {minted_prefix:?}, \
                     got {secret:?}"
                );
                assert_eq!(
                    string(cmk, "KmsKeyArn", case),
                    KMS_KEY_ARN,
                    "case {case:?}: expected the KMS key the request named, got {described}"
                );
                assert!(
                    managed.is_null() && custom.is_null(),
                    "case {case:?}: expected only CmkSecretConfig, got {described}"
                );
                Some(secret)
            }
            Self::CustomerSecret => {
                assert_eq!(
                    string(custom, "SecretArn", case),
                    CUSTOMER_SECRET_ARN,
                    "case {case:?}: expected the secret the request named, got {described}"
                );
                assert_eq!(
                    string(custom, "SecretAccessRoleArn", case),
                    SECRET_ACCESS_ROLE_ARN,
                    "case {case:?}: expected the role the request named, got {described}"
                );
                assert!(
                    managed.is_null() && cmk.is_null(),
                    "case {case:?}: expected only CustomSecretConfig, got {described}"
                );
                None
            }
            Self::None => {
                assert!(
                    managed.is_null() && cmk.is_null() && custom.is_null(),
                    "case {case:?}: expected no secret configuration at all, got {described}"
                );
                None
            }
        }
    }
}

/// One `CreateLocationAzureBlob` request and what the stored location must report.
struct Row {
    case: &'static str,
    container_url: &'static str,
    authentication_type: &'static str,
    subdirectory: Option<&'static str>,
    blob_type: Option<&'static str>,
    access_tier: Option<&'static str>,
    agent_arns: &'static [&'static str],
    credential: Credential,
    expected_uri: &'static str,
    expected_blob_type: &'static str,
    expected_access_tier: &'static str,
}

/// The request body a row asks for.
fn request_of(row: &Row) -> serde_json::Value {
    let mut request = serde_json::json!({
        "ContainerUrl": row.container_url,
        "AuthenticationType": row.authentication_type,
    });
    if let Some(subdirectory) = row.subdirectory {
        request["Subdirectory"] = serde_json::json!(subdirectory);
    }
    if let Some(blob_type) = row.blob_type {
        request["BlobType"] = serde_json::json!(blob_type);
    }
    if let Some(access_tier) = row.access_tier {
        request["AccessTier"] = serde_json::json!(access_tier);
    }
    if !row.agent_arns.is_empty() {
        request["AgentArns"] = serde_json::json!(row.agent_arns);
    }
    row.credential.apply(&mut request);
    request
}

/// `CreateLocationAzureBlob` stores the location it mints, and
/// `DescribeLocationAzureBlob` reports it from state.
///
/// The rows span what the rule generalises over: the `Subdirectory` spellings that denote
/// one virtual directory, a declared blob type and access tier versus the values a
/// request that declares none ends up with, every `AzureAccessTier` the model admits
/// (`HOT | COOL | ARCHIVE`), both `AzureBlobAuthenticationType` values (`SAS | NONE`),
/// agents and no agents, and each of the credential sources the operation accepts.
#[tokio::test]
async fn created_azure_blob_locations_are_reported_from_state() {
    let rows = [
        Row {
            case: "sas token, container root, nothing declared",
            container_url: "https://hotstore.blob.core.windows.net/transfer-in",
            authentication_type: "SAS",
            subdirectory: None,
            blob_type: None,
            access_tier: None,
            agent_arns: &[],
            credential: Credential::SasToken,
            expected_uri: "azure-blob://hotstore.blob.core.windows.net/transfer-in/",
            expected_blob_type: "BLOCK",
            expected_access_tier: "HOT",
        },
        Row {
            case: "cool tier, subdirectory with a leading slash",
            container_url: "https://coolstore.blob.core.windows.net/transfer-out",
            authentication_type: "SAS",
            subdirectory: Some("/images/raw"),
            blob_type: Some("BLOCK"),
            access_tier: Some("COOL"),
            agent_arns: &[AGENT_ARNS[0]],
            credential: Credential::SasToken,
            expected_uri: "azure-blob://coolstore.blob.core.windows.net/transfer-out/images/raw/",
            expected_blob_type: "BLOCK",
            expected_access_tier: "COOL",
        },
        Row {
            case: "archive tier, the same subdirectory spelled with trailing slashes",
            container_url: "https://coolstore.blob.core.windows.net/transfer-out",
            authentication_type: "SAS",
            subdirectory: Some("images/raw/"),
            blob_type: None,
            access_tier: Some("ARCHIVE"),
            agent_arns: &AGENT_ARNS,
            credential: Credential::SasToken,
            expected_uri: "azure-blob://coolstore.blob.core.windows.net/transfer-out/images/raw/",
            expected_blob_type: "BLOCK",
            expected_access_tier: "ARCHIVE",
        },
        Row {
            case: "sas token stored under a customer-managed key",
            container_url: "https://cmkstore.blob.core.windows.net/encrypted-data",
            authentication_type: "SAS",
            subdirectory: Some("/nested/deeper/still"),
            blob_type: Some("BLOCK"),
            access_tier: Some("HOT"),
            agent_arns: &[],
            credential: Credential::SasTokenWithCustomerKey,
            expected_uri:
                "azure-blob://cmkstore.blob.core.windows.net/encrypted-data/nested/deeper/still/",
            expected_blob_type: "BLOCK",
            expected_access_tier: "HOT",
        },
        Row {
            case: "a secret the caller manages",
            container_url: "https://ownsecret.blob.core.windows.net/byo-secret",
            authentication_type: "SAS",
            subdirectory: None,
            blob_type: None,
            access_tier: Some("COOL"),
            agent_arns: &[AGENT_ARNS[1]],
            credential: Credential::CustomerSecret,
            expected_uri: "azure-blob://ownsecret.blob.core.windows.net/byo-secret/",
            expected_blob_type: "BLOCK",
            expected_access_tier: "COOL",
        },
        Row {
            case: "no authentication and no credential at all",
            container_url: "https://openstore.blob.core.windows.net/public-container",
            authentication_type: "NONE",
            subdirectory: Some("/"),
            blob_type: None,
            access_tier: None,
            agent_arns: &[],
            credential: Credential::None,
            expected_uri: "azure-blob://openstore.blob.core.windows.net/public-container/",
            expected_blob_type: "BLOCK",
            expected_access_tier: "HOT",
        },
    ];

    let router = service();
    let mut arns: Vec<String> = Vec::new();
    let mut managed_secrets: Vec<String> = Vec::new();

    for row in &rows {
        let created = ok(&router, "CreateLocationAzureBlob", request_of(row), row.case).await;
        let arn = string(&created, "LocationArn", row.case);
        assert_location_arn(&arn, row.case);
        assert!(
            !arns.contains(&arn),
            "case {:?}: every created location gets its own ARN, got {arn:?} again",
            row.case
        );
        arns.push(arn.clone());

        let described = ok(
            &router,
            "DescribeLocationAzureBlob",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;

        assert_eq!(
            string(&described, "LocationArn", row.case),
            arn,
            "case {:?}: the describe must report the ARN it was asked about, got {described}",
            row.case
        );
        assert_eq!(
            string(&described, "LocationUri", row.case),
            row.expected_uri,
            "case {:?}: expected the container and virtual directory the request named, \
             got {described}",
            row.case
        );
        assert_eq!(
            string(&described, "AuthenticationType", row.case),
            row.authentication_type,
            "case {:?}: expected the authentication method the request named, got {described}",
            row.case
        );
        assert_eq!(
            string(&described, "BlobType", row.case),
            row.expected_blob_type,
            "case {:?}: expected blob type {:?}, got {described}",
            row.case,
            row.expected_blob_type
        );
        assert_eq!(
            string(&described, "AccessTier", row.case),
            row.expected_access_tier,
            "case {:?}: expected access tier {:?}, got {described}",
            row.case,
            row.expected_access_tier
        );

        let reported_agents = described["AgentArns"].clone();
        if row.agent_arns.is_empty() {
            assert!(
                reported_agents.is_null(),
                "case {:?}: `AgentArns` is modelled len 1..8, so a location with no agents \
                 reports no member, got {described}",
                row.case
            );
        } else {
            assert_eq!(
                reported_agents,
                serde_json::json!(row.agent_arns),
                "case {:?}: expected the agents the request named, got {described}",
                row.case
            );
        }

        let creation_time = described["CreationTime"].as_f64().unwrap_or_default();
        assert!(
            creation_time > 0.0,
            "case {:?}: expected the creation time the location was stored with, got {described}",
            row.case
        );

        if let Some(secret) = row.credential.assert_reported(&described, row.case) {
            assert!(
                !managed_secrets.contains(&secret),
                "case {:?}: each location gets its own DataSync-managed secret, got {secret:?} \
                 again",
                row.case
            );
            managed_secrets.push(secret);
        }

        // The SAS token is a credential: it is stored in a secret, never reported.
        let rendered = described.to_string();
        assert!(
            !rendered.contains("sig=") && !rendered.contains("SasConfiguration"),
            "case {:?}: the authentication token must never be reported, got {rendered}",
            row.case
        );

        // A read reports; it never changes what the next reader sees.
        let again = ok(
            &router,
            "DescribeLocationAzureBlob",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        assert_eq!(
            again, described,
            "case {:?}: two reads of one location must agree, got {again} then {described}",
            row.case
        );
    }
}

/// Two requests carrying the same configuration address two independent locations.
///
/// The rows are the credential sources, so the rule holds whichever one a duplicate
/// request repeats: nothing about a location is derived from a hash of its request.
#[tokio::test]
async fn duplicate_azure_blob_configurations_are_independent_locations() {
    let credentials = [
        ("inline sas token", Credential::SasToken),
        ("customer-managed key", Credential::SasTokenWithCustomerKey),
        ("customer-managed secret", Credential::CustomerSecret),
        ("no credential", Credential::None),
    ];
    let router = service();
    let mut arns: Vec<String> = Vec::new();

    for (case, credential) in credentials {
        let row = Row {
            case,
            container_url: "https://duplicate.blob.core.windows.net/shared-container",
            authentication_type: if credential == Credential::None {
                "NONE"
            } else {
                "SAS"
            },
            subdirectory: Some("/same/prefix"),
            blob_type: Some("BLOCK"),
            access_tier: Some("HOT"),
            agent_arns: &AGENT_ARNS,
            credential,
            expected_uri: "",
            expected_blob_type: "BLOCK",
            expected_access_tier: "HOT",
        };

        let mut described_bodies: Vec<serde_json::Value> = Vec::new();
        for attempt in 0..3 {
            let created = ok(&router, "CreateLocationAzureBlob", request_of(&row), case).await;
            let arn = string(&created, "LocationArn", case);
            assert_location_arn(&arn, case);
            assert!(
                !arns.contains(&arn),
                "case {case:?}: attempt {attempt} must mint a new location, got {arn:?} again"
            );
            arns.push(arn.clone());

            let described = ok(
                &router,
                "DescribeLocationAzureBlob",
                serde_json::json!({ "LocationArn": arn }),
                case,
            )
            .await;
            assert_eq!(
                string(&described, "LocationArn", case),
                arn,
                "case {case:?}: attempt {attempt} must report its own ARN, got {described}"
            );
            assert_eq!(
                string(&described, "LocationUri", case),
                "azure-blob://duplicate.blob.core.windows.net/shared-container/same/prefix/",
                "case {case:?}: attempt {attempt} must report the container it named, \
                 got {described}"
            );
            described_bodies.push(described);
        }

        // The duplicates are separate resources, yet each describes the configuration it
        // was created with: everything but the identifiers it was issued agrees.
        for body in &mut described_bodies {
            body["LocationArn"] = serde_json::Value::Null;
            body["CreationTime"] = serde_json::Value::Null;
            for member in ["ManagedSecretConfig", "CmkSecretConfig"] {
                if let Some(secret) = body.get_mut(member).and_then(|m| m.get_mut("SecretArn")) {
                    *secret = serde_json::Value::Null;
                }
            }
        }
        assert!(
            described_bodies.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: duplicates must describe the same configuration, got \
             {described_bodies:?}"
        );
    }
}

/// The credential parameters may only be combined the way both members document, and a
/// location ARN is reported only by the `DescribeLocation*` of its own kind.
#[tokio::test]
async fn azure_blob_location_requests_outside_the_documented_rules_are_refused() {
    let router = service();
    let container = "https://refused.blob.core.windows.net/refused-container";

    // Both secret configurations at once: the model says not to provide both.
    invalid_request(
        &router,
        "CreateLocationAzureBlob",
        serde_json::json!({
            "ContainerUrl": container,
            "AuthenticationType": "SAS",
            "SasConfiguration": { "Token": SAS_TOKEN },
            "CmkSecretConfig": { "KmsKeyArn": KMS_KEY_ARN },
            "CustomSecretConfig": {
                "SecretArn": CUSTOMER_SECRET_ARN,
                "SecretAccessRoleArn": SECRET_ACCESS_ROLE_ARN,
            },
        }),
        "Do not provide both parameters for the same request.",
        "both secret configurations",
    )
    .await;

    // A customer-managed secret is used *without* an inline token.
    invalid_request(
        &router,
        "CreateLocationAzureBlob",
        serde_json::json!({
            "ContainerUrl": container,
            "AuthenticationType": "SAS",
            "SasConfiguration": { "Token": SAS_TOKEN },
            "CustomSecretConfig": {
                "SecretArn": CUSTOMER_SECRET_ARN,
                "SecretAccessRoleArn": SECRET_ACCESS_ROLE_ARN,
            },
        }),
        "Do not provide CustomSecretConfig with SasConfiguration.",
        "customer secret with an inline token",
    )
    .await;

    // A DataSync-managed secret under a customer key needs the token it is to hold.
    invalid_request(
        &router,
        "CreateLocationAzureBlob",
        serde_json::json!({
            "ContainerUrl": container,
            "AuthenticationType": "SAS",
            "CmkSecretConfig": { "KmsKeyArn": KMS_KEY_ARN },
        }),
        "Do not provide CmkSecretConfig without SasConfiguration.",
        "customer key without a token",
    )
    .await;

    // An ARN this service never minted resolves to nothing.
    let never_minted =
        format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-ffffffffffffffffa");
    invalid_request(
        &router,
        "DescribeLocationAzureBlob",
        serde_json::json!({ "LocationArn": never_minted }),
        &format!("Location {never_minted} is not found."),
        "unknown location",
    )
    .await;

    // A location is reported by the describe of its own kind, and by no other.
    let azure_blob = string(
        &ok(
            &router,
            "CreateLocationAzureBlob",
            serde_json::json!({
                "ContainerUrl": container,
                "AuthenticationType": "SAS",
                "SasConfiguration": { "Token": SAS_TOKEN },
            }),
            "kind separation",
        )
        .await,
        "LocationArn",
        "kind separation",
    );
    let s3 = string(
        &ok(
            &router,
            "CreateLocationS3",
            serde_json::json!({
                "S3BucketArn": "arn:aws:s3:::refused-bucket",
                "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
            }),
            "kind separation",
        )
        .await,
        "LocationArn",
        "kind separation",
    );

    invalid_request(
        &router,
        "DescribeLocationAzureBlob",
        serde_json::json!({ "LocationArn": s3 }),
        &format!("Location {s3} is not an Azure Blob Storage location."),
        "an S3 location read as Azure Blob Storage",
    )
    .await;
    invalid_request(
        &router,
        "DescribeLocationS3",
        serde_json::json!({ "LocationArn": azure_blob }),
        &format!("Location {azure_blob} is not an Amazon S3 location."),
        "an Azure Blob Storage location read as S3",
    )
    .await;
}

/// A transfer task can be built from Azure Blob Storage locations: `CreateTask` resolves
/// any kind of stored location, not only the S3 ones.
#[tokio::test]
async fn azure_blob_locations_can_be_both_ends_of_a_task() {
    let case = "azure to azure";
    let router = service();

    let location = |container: &'static str| {
        serde_json::json!({
            "ContainerUrl": format!("https://taskstore.blob.core.windows.net/{container}"),
            "AuthenticationType": "SAS",
            "SasConfiguration": { "Token": SAS_TOKEN },
        })
    };

    let source = string(
        &ok(&router, "CreateLocationAzureBlob", location("task-source"), case).await,
        "LocationArn",
        case,
    );
    let destination = string(
        &ok(
            &router,
            "CreateLocationAzureBlob",
            location("task-destination"),
            case,
        )
        .await,
        "LocationArn",
        case,
    );

    let created = ok(
        &router,
        "CreateTask",
        serde_json::json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": "azure-blob-task",
        }),
        case,
    )
    .await;
    let task_arn = string(&created, "TaskArn", case);

    let described = ok(
        &router,
        "DescribeTask",
        serde_json::json!({ "TaskArn": task_arn }),
        case,
    )
    .await;
    assert_eq!(
        string(&described, "SourceLocationArn", case),
        source,
        "case {case:?}: expected the Azure Blob Storage source the task was created with, \
         got {described}"
    );
    assert_eq!(
        string(&described, "DestinationLocationArn", case),
        destination,
        "case {case:?}: expected the Azure Blob Storage destination the task was created with, \
         got {described}"
    );
}
