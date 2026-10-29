// SPDX-License-Identifier: CC0-1.0

//! DataSync object storage transfer locations, driven end to end over HTTP.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
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

/// A call that must be refused with HTTP 400 `InvalidRequestException`, returning the
/// refusal body so its message and discriminators can be asserted.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    case: &str,
) -> serde_json::Value {
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
    parsed
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
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a location ARN starting {prefix:?}, got {arn:?}"
    );
    let id = &arn[prefix.len()..];
    assert!(
        id.len() == 17 && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()),
        "case {case:?}: expected a 17-character lowercase alphanumeric location id, got {id:?}"
    );
}

/// An agent ARN in the modelled `AgentArn` shape, distinct per `tail`.
fn agent_arn(tail: char) -> String {
    format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:agent/agent-0123456789abcdef{tail}")
}

/// A Secrets Manager secret ARN the caller manages themselves.
fn customer_secret_arn() -> String {
    format!("arn:aws:secretsmanager:{SIGNED_REGION}:{ACCOUNT}:secret:object-storage-AbCdEf")
}

/// The IAM role that reaches a caller-managed secret.
fn secret_access_role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/datasync-secret-reader")
}

/// A customer-managed KMS key ARN.
fn kms_key_arn() -> String {
    format!("arn:aws:kms:{SIGNED_REGION}:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab")
}

/// Where a location's credential is kept, as the three mutually exclusive secret
/// members of `DescribeLocationObjectStorage` report it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Secret {
    /// No credential at all: none of the three members is reported.
    Absent,
    /// A DataSync-managed secret encrypted with the Amazon Web Services-managed key.
    Managed,
    /// A DataSync-managed secret encrypted with the caller's own KMS key.
    CustomerKey,
    /// A secret the caller manages, reached through the role they named.
    Custom,
}

/// Assert the reported secret members are exactly the ones `expected` names, and that
/// a DataSync-managed secret is reported with an ARN this emulator minted.
fn assert_secret(described: &serde_json::Value, expected: &Secret, case: &str) {
    let managed = &described["ManagedSecretConfig"];
    let cmk = &described["CmkSecretConfig"];
    let custom = &described["CustomSecretConfig"];
    let secret_prefix = format!("arn:aws:secretsmanager:{SIGNED_REGION}:{ACCOUNT}:secret:");
    match expected {
        Secret::Absent => assert!(
            managed.is_null() && cmk.is_null() && custom.is_null(),
            "case {case:?}: a location with no credential reports no secret member, got {described}"
        ),
        Secret::Managed => {
            let arn = string(managed, "SecretArn", case);
            assert!(
                arn.starts_with(&secret_prefix) && cmk.is_null() && custom.is_null(),
                "case {case:?}: expected only a managed secret under {secret_prefix:?}, got {described}"
            );
        }
        Secret::CustomerKey => {
            let arn = string(cmk, "SecretArn", case);
            assert!(
                arn.starts_with(&secret_prefix) && managed.is_null() && custom.is_null(),
                "case {case:?}: expected only a CMK-encrypted secret under {secret_prefix:?}, got {described}"
            );
            assert_eq!(
                string(cmk, "KmsKeyArn", case),
                kms_key_arn(),
                "case {case:?}: the CMK secret keeps the key the request named, got {described}"
            );
        }
        Secret::Custom => {
            assert_eq!(
                (
                    string(custom, "SecretArn", case),
                    string(custom, "SecretAccessRoleArn", case),
                    managed.is_null(),
                    cmk.is_null()
                ),
                (customer_secret_arn(), secret_access_role_arn(), true, true),
                "case {case:?}: expected only the caller's own secret, got {described}"
            );
        }
    }
}

/// What a created object storage location must report when it is read back.
struct Expected {
    uri: String,
    port: i64,
    protocol: &'static str,
    access_key: Option<&'static str>,
    agents: Option<Vec<String>>,
    secret: Secret,
    certificate: Option<&'static str>,
}

/// One row of the create/describe table: a request and the location it must store.
struct Row {
    case: &'static str,
    request: serde_json::Value,
    expected: Expected,
}

/// Assert `described` reports exactly `expected`.
fn assert_location(described: &serde_json::Value, expected: &Expected, case: &str) {
    assert_eq!(
        string(described, "LocationUri", case),
        expected.uri,
        "case {case:?}: wrong LocationUri, got {described}"
    );
    assert_eq!(
        described["ServerPort"].as_i64(),
        Some(expected.port),
        "case {case:?}: wrong ServerPort, got {described}"
    );
    assert_eq!(
        described["ServerProtocol"].as_str(),
        Some(expected.protocol),
        "case {case:?}: wrong ServerProtocol, got {described}"
    );
    assert_eq!(
        described["AccessKey"].as_str(),
        expected.access_key,
        "case {case:?}: wrong AccessKey, got {described}"
    );
    let reported_agents = described["AgentArns"].as_array().map(|agents| {
        agents
            .iter()
            .map(|agent| agent.as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>()
    });
    assert_eq!(
        reported_agents, expected.agents,
        "case {case:?}: wrong AgentArns, got {described}"
    );
    assert_eq!(
        described["ServerCertificate"].as_str(),
        expected.certificate,
        "case {case:?}: wrong ServerCertificate, got {described}"
    );
    assert!(
        described["CreationTime"].is_number(),
        "case {case:?}: expected a CreationTime instant, got {described}"
    );
    assert_secret(described, &expected.secret, case);
}

/// `CreateLocationObjectStorage` stores the location it mints, and
/// `DescribeLocationObjectStorage` reports it from that state.
///
/// The rows span what the rule generalises over: a server named by domain name and by
/// IPv4 address, both modelled `ServerProtocol` values, a declared port versus the
/// port the protocol defaults to, the `Subdirectory` spellings that denote one object
/// prefix, bucket names at the modelled length bound and with the punctuation the
/// member's pattern admits, no agents through the modelled maximum of eight, and each
/// of the three ways the model gives a location to hold its credential.
#[tokio::test]
async fn created_object_storage_locations_are_reported_from_state() {
    let rows = vec![
        Row {
            case: "defaults: no protocol, port, prefix or credential",
            request: serde_json::json!({
                "ServerHostname": "storage.example.com",
                "BucketName": "my-bucket",
            }),
            expected: Expected {
                uri: "object-storage://storage.example.com/my-bucket/".to_string(),
                port: 443,
                protocol: "HTTPS",
                access_key: None,
                agents: None,
                secret: Secret::Absent,
                certificate: None,
            },
        },
        Row {
            case: "HTTP without a port",
            request: serde_json::json!({
                "ServerHostname": "obj.example.net",
                "BucketName": "logs-bucket",
                "ServerProtocol": "HTTP",
            }),
            expected: Expected {
                uri: "object-storage://obj.example.net/logs-bucket/".to_string(),
                port: 80,
                protocol: "HTTP",
                access_key: None,
                agents: None,
                secret: Secret::Absent,
                certificate: None,
            },
        },
        Row {
            case: "IPv4 server, declared port, nested prefix in slashes",
            request: serde_json::json!({
                "ServerHostname": "10.0.0.7",
                "BucketName": "obj_bucket-1",
                "ServerProtocol": "HTTP",
                "ServerPort": 8080,
                "Subdirectory": "/data/archive/",
                "AccessKey": "datasync-user",
            }),
            expected: Expected {
                uri: "object-storage://10.0.0.7/obj_bucket-1/data/archive/".to_string(),
                port: 8080,
                protocol: "HTTP",
                access_key: Some("datasync-user"),
                agents: None,
                secret: Secret::Absent,
                certificate: None,
            },
        },
        Row {
            case: "prefix spelled without slashes, port at the modelled maximum",
            request: serde_json::json!({
                "ServerHostname": "storage-1.example.com",
                "BucketName": "abc",
                "ServerPort": 65536,
                "Subdirectory": "exports",
            }),
            expected: Expected {
                uri: "object-storage://storage-1.example.com/abc/exports/".to_string(),
                port: 65536,
                protocol: "HTTPS",
                access_key: None,
                agents: None,
                secret: Secret::Absent,
                certificate: None,
            },
        },
        Row {
            case: "two agents and a secret key DataSync manages",
            request: serde_json::json!({
                "ServerHostname": "nas.example.com",
                "BucketName": "bucket.name+1",
                "ServerPort": 1,
                "AccessKey": "reader",
                "SecretKey": "s3cr3t",
                "AgentArns": [agent_arn('0'), agent_arn('1')],
                "Tags": [{ "Key": "team", "Value": "storage" }],
            }),
            expected: Expected {
                uri: "object-storage://nas.example.com/bucket.name+1/".to_string(),
                port: 1,
                protocol: "HTTPS",
                access_key: Some("reader"),
                agents: Some(vec![agent_arn('0'), agent_arn('1')]),
                secret: Secret::Managed,
                certificate: None,
            },
        },
        Row {
            case: "secret key in a secret encrypted with the caller's KMS key",
            request: serde_json::json!({
                "ServerHostname": "cmk.example.com",
                "BucketName": "cmk-bucket",
                "SecretKey": "s3cr3t",
                "CmkSecretConfig": { "KmsKeyArn": kms_key_arn() },
            }),
            expected: Expected {
                uri: "object-storage://cmk.example.com/cmk-bucket/".to_string(),
                port: 443,
                protocol: "HTTPS",
                access_key: None,
                agents: None,
                secret: Secret::CustomerKey,
                certificate: None,
            },
        },
        Row {
            case: "a secret the caller manages, with no secret key",
            request: serde_json::json!({
                "ServerHostname": "custom.example.com",
                "BucketName": "custom-bucket",
                "CustomSecretConfig": {
                    "SecretArn": customer_secret_arn(),
                    "SecretAccessRoleArn": secret_access_role_arn(),
                },
            }),
            expected: Expected {
                uri: "object-storage://custom.example.com/custom-bucket/".to_string(),
                port: 443,
                protocol: "HTTPS",
                access_key: None,
                agents: None,
                secret: Secret::Custom,
                certificate: None,
            },
        },
        Row {
            case: "a certificate chain and the modelled maximum of eight agents",
            request: serde_json::json!({
                "ServerHostname": "pki.example.com",
                "BucketName": "pki-bucket",
                "ServerCertificate": "cGVtLWNoYWluCg==",
                "AgentArns": (0..8)
                    .map(|index| agent_arn(char::from_digit(index, 10).expect("a digit")))
                    .collect::<Vec<String>>(),
            }),
            expected: Expected {
                uri: "object-storage://pki.example.com/pki-bucket/".to_string(),
                port: 443,
                protocol: "HTTPS",
                access_key: None,
                agents: Some(
                    (0..8)
                        .map(|index| agent_arn(char::from_digit(index, 10).expect("a digit")))
                        .collect(),
                ),
                secret: Secret::Absent,
                certificate: Some("cGVtLWNoYWluCg=="),
            },
        },
    ];

    let router = service();
    let mut minted: Vec<String> = Vec::new();

    for row in &rows {
        let created = ok(
            &router,
            "CreateLocationObjectStorage",
            row.request.clone(),
            row.case,
        )
        .await;
        let arn = string(&created, "LocationArn", row.case);
        assert_location_arn(&arn, row.case);
        assert!(
            !minted.contains(&arn),
            "case {:?}: every create mints a fresh ARN, but {arn:?} was minted before",
            row.case
        );
        minted.push(arn.clone());

        let described = ok(
            &router,
            "DescribeLocationObjectStorage",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        assert_eq!(
            string(&described, "LocationArn", row.case),
            arn,
            "case {:?}: the read reports the ARN it was asked about, got {described}",
            row.case
        );
        assert_location(&described, &row.expected, row.case);

        // A read must not change what the next caller sees.
        let described_again = ok(
            &router,
            "DescribeLocationObjectStorage",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        assert_eq!(
            described_again, described,
            "case {:?}: two reads of one location must agree, got {described_again}",
            row.case
        );
    }

    // Every location is still readable after the later creates, so the store holds
    // them all rather than one.
    for (arn, row) in minted.iter().zip(&rows) {
        let described = ok(
            &router,
            "DescribeLocationObjectStorage",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        assert_location(&described, &row.expected, row.case);
    }
}

/// `UpdateLocationObjectStorage` writes the members the request names and leaves the
/// rest as they were, and `DescribeLocationObjectStorage` reports the result.
///
/// Each row is one update applied to the same location, with the whole configuration
/// it must report afterwards — so every row also asserts that the members the request
/// did not name kept their value.
#[tokio::test]
async fn object_storage_updates_write_only_the_members_the_request_names() {
    let case = "update one member at a time";
    let router = service();
    let created = ok(
        &router,
        "CreateLocationObjectStorage",
        serde_json::json!({
            "ServerHostname": "before.example.com",
            "BucketName": "update-bucket",
            "ServerProtocol": "HTTPS",
            "ServerPort": 443,
            "Subdirectory": "/first",
            "AccessKey": "first-user",
            "SecretKey": "first-secret",
            "AgentArns": [agent_arn('0')],
        }),
        case,
    )
    .await;
    let arn = string(&created, "LocationArn", case);

    let steps = vec![
        (
            "nothing but the ARN changes nothing",
            serde_json::json!({ "LocationArn": arn }),
            Expected {
                uri: "object-storage://before.example.com/update-bucket/first/".to_string(),
                port: 443,
                protocol: "HTTPS",
                access_key: Some("first-user"),
                agents: Some(vec![agent_arn('0')]),
                secret: Secret::Managed,
                certificate: None,
            },
        ),
        (
            "a port on its own",
            serde_json::json!({ "LocationArn": arn, "ServerPort": 9000 }),
            Expected {
                uri: "object-storage://before.example.com/update-bucket/first/".to_string(),
                port: 9000,
                protocol: "HTTPS",
                access_key: Some("first-user"),
                agents: Some(vec![agent_arn('0')]),
                secret: Secret::Managed,
                certificate: None,
            },
        ),
        (
            "a protocol with no port moves to that protocol's port",
            serde_json::json!({ "LocationArn": arn, "ServerProtocol": "HTTP" }),
            Expected {
                uri: "object-storage://before.example.com/update-bucket/first/".to_string(),
                port: 80,
                protocol: "HTTP",
                access_key: Some("first-user"),
                agents: Some(vec![agent_arn('0')]),
                secret: Secret::Managed,
                certificate: None,
            },
        ),
        (
            "a new prefix and server, which the URI reports",
            serde_json::json!({
                "LocationArn": arn,
                "ServerHostname": "after.example.com",
                "Subdirectory": "second/level/",
            }),
            Expected {
                uri: "object-storage://after.example.com/update-bucket/second/level/".to_string(),
                port: 80,
                protocol: "HTTP",
                access_key: Some("first-user"),
                agents: Some(vec![agent_arn('0')]),
                secret: Secret::Managed,
                certificate: None,
            },
        ),
        (
            "new credentials, a certificate, and the agents the location already has",
            serde_json::json!({
                "LocationArn": arn,
                "AccessKey": "second-user",
                "CustomSecretConfig": {
                    "SecretArn": customer_secret_arn(),
                    "SecretAccessRoleArn": secret_access_role_arn(),
                },
                "ServerCertificate": "cGVtLWNoYWluCg==",
                "AgentArns": [agent_arn('0')],
            }),
            Expected {
                uri: "object-storage://after.example.com/update-bucket/second/level/".to_string(),
                port: 80,
                protocol: "HTTP",
                access_key: Some("second-user"),
                agents: Some(vec![agent_arn('0')]),
                secret: Secret::Custom,
                certificate: Some("cGVtLWNoYWluCg=="),
            },
        ),
        (
            "another agent replaces the one an agent-based location had",
            serde_json::json!({ "LocationArn": arn, "AgentArns": [agent_arn('1'), agent_arn('2')] }),
            Expected {
                uri: "object-storage://after.example.com/update-bucket/second/level/".to_string(),
                port: 80,
                protocol: "HTTP",
                access_key: Some("second-user"),
                agents: Some(vec![agent_arn('1'), agent_arn('2')]),
                secret: Secret::Custom,
                certificate: Some("cGVtLWNoYWluCg=="),
            },
        ),
    ];

    for (step, request, expected) in &steps {
        ok(
            &router,
            "UpdateLocationObjectStorage",
            request.clone(),
            step,
        )
        .await;
        let described = ok(
            &router,
            "DescribeLocationObjectStorage",
            serde_json::json!({ "LocationArn": arn }),
            step,
        )
        .await;
        assert_location(&described, expected, step);
    }
}

/// The refusals an object storage location read or update owes its caller.
#[tokio::test]
async fn object_storage_location_reads_and_updates_refuse_what_datasync_refuses() {
    let router = service();

    // A well-formed location ARN naming a location this account does not hold.
    let absent = format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-0abcdef1234567890");
    for operation in ["DescribeLocationObjectStorage", "UpdateLocationObjectStorage"] {
        let case = "an absent location";
        let refusal = refused(
            &router,
            operation,
            serde_json::json!({ "LocationArn": absent }),
            case,
        )
        .await;
        assert_eq!(
            string(&refusal, "message", case),
            format!("Location {absent} is not found."),
            "case {case:?}: {operation} must report the location it could not find, got {refusal}"
        );
    }

    // A location of another storage family is not an object storage location.
    let case = "an S3 location read as object storage";
    let s3 = ok(
        &router,
        "CreateLocationS3",
        serde_json::json!({
            "S3BucketArn": "arn:aws:s3:::other-family",
            "S3Config": { "BucketAccessRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/datasync") },
        }),
        case,
    )
    .await;
    let s3_arn = string(&s3, "LocationArn", case);
    for (operation, request) in [
        (
            "DescribeLocationObjectStorage",
            serde_json::json!({ "LocationArn": s3_arn }),
        ),
        (
            // Even the member an object storage update refuses outright is answered
            // with the wrong-kind refusal: this location is of another family.
            "UpdateLocationObjectStorage",
            serde_json::json!({ "LocationArn": s3_arn, "AgentArns": [agent_arn('0')] }),
        ),
    ] {
        let refusal = refused(&router, operation, request, case).await;
        assert_eq!(
            string(&refusal, "message", case),
            format!("Location {s3_arn} is not an object storage location."),
            "case {case:?}: wrong refusal from {operation}, got {refusal}"
        );
    }

    // The credential parameters have to arrive in one of the combinations the two
    // secret-configuration members document.
    let secret_configs = serde_json::json!({
        "CmkSecretConfig": { "KmsKeyArn": kms_key_arn() },
        "CustomSecretConfig": {
            "SecretArn": customer_secret_arn(),
            "SecretAccessRoleArn": secret_access_role_arn(),
        },
    });
    let create_rule = "You can use either CmkSecretConfig (with SecretKey) or \
                       CustomSecretConfig (without SecretKey) to provide credentials for a \
                       CreateLocationObjectStorage request.";
    let create_combinations = [
        (
            "both secret configurations on a create",
            serde_json::json!({
                "ServerHostname": "both.example.com",
                "BucketName": "both-bucket",
                "SecretKey": "s3cr3t",
                "CmkSecretConfig": secret_configs["CmkSecretConfig"],
                "CustomSecretConfig": secret_configs["CustomSecretConfig"],
            }),
            format!("{create_rule} Do not provide both parameters for the same request."),
        ),
        (
            "a caller-managed secret together with a secret key",
            serde_json::json!({
                "ServerHostname": "custom.example.com",
                "BucketName": "custom-bucket",
                "SecretKey": "s3cr3t",
                "CustomSecretConfig": secret_configs["CustomSecretConfig"],
            }),
            format!("{create_rule} Do not provide CustomSecretConfig with SecretKey."),
        ),
        (
            "a KMS key with no secret key to encrypt",
            serde_json::json!({
                "ServerHostname": "cmk.example.com",
                "BucketName": "cmk-bucket",
                "CmkSecretConfig": secret_configs["CmkSecretConfig"],
            }),
            format!("{create_rule} Do not provide CmkSecretConfig without SecretKey."),
        ),
    ];
    for (case, request, expected_message) in create_combinations {
        let refusal = refused(&router, "CreateLocationObjectStorage", request, case).await;
        assert_eq!(
            string(&refusal, "message", case),
            expected_message,
            "case {case:?}: wrong credential-parameter refusal, got {refusal}"
        );
    }

    // An update names one place to find the credential, like a create.
    let case = "both secret configurations on an update";
    let refusal = refused(
        &router,
        "UpdateLocationObjectStorage",
        serde_json::json!({
            "LocationArn": absent,
            "CmkSecretConfig": secret_configs["CmkSecretConfig"],
            "CustomSecretConfig": secret_configs["CustomSecretConfig"],
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&refusal, "message", case),
        "You can use either CmkSecretConfig or CustomSecretConfig to provide credentials for \
         an UpdateLocationObjectStorage request. Do not provide both parameters for the same \
         request.",
        "case {case:?}: wrong credential-parameter refusal, got {refusal}"
    );

    // A location created without an agent cannot be given one later.
    let case = "an agent for an agentless location";
    let agentless = ok(
        &router,
        "CreateLocationObjectStorage",
        serde_json::json!({
            "ServerHostname": "agentless.example.com",
            "BucketName": "agentless-bucket",
        }),
        case,
    )
    .await;
    let agentless_arn = string(&agentless, "LocationArn", case);
    let refusal = refused(
        &router,
        "UpdateLocationObjectStorage",
        serde_json::json!({ "LocationArn": agentless_arn, "AgentArns": [agent_arn('0')] }),
        case,
    )
    .await;
    assert_eq!(
        string(&refusal, "message", case),
        "Invalid parameter: Updating AgentArns is not permitted for agentless object \
         storage locations.",
        "case {case:?}: wrong agentless refusal, got {refusal}"
    );

    // The refused update left the location exactly as it was.
    let described = ok(
        &router,
        "DescribeLocationObjectStorage",
        serde_json::json!({ "LocationArn": agentless_arn }),
        case,
    )
    .await;
    assert_location(
        &described,
        &Expected {
            uri: "object-storage://agentless.example.com/agentless-bucket/".to_string(),
            port: 443,
            protocol: "HTTPS",
            access_key: None,
            agents: None,
            secret: Secret::Absent,
            certificate: None,
        },
        case,
    );
}

/// A `CmkSecretConfig` on its own re-keys the secret DataSync already holds for the
/// location rather than discarding the credential.
#[tokio::test]
async fn a_cmk_only_update_rekeys_the_secret_the_location_already_has() {
    let case = "re-key an existing managed secret";
    let router = service();
    let created = ok(
        &router,
        "CreateLocationObjectStorage",
        serde_json::json!({
            "ServerHostname": "rekey.example.com",
            "BucketName": "rekey-bucket",
            "SecretKey": "s3cr3t",
        }),
        case,
    )
    .await;
    let arn = string(&created, "LocationArn", case);
    let described = ok(
        &router,
        "DescribeLocationObjectStorage",
        serde_json::json!({ "LocationArn": arn }),
        case,
    )
    .await;
    let secret_arn = string(&described["ManagedSecretConfig"], "SecretArn", case);

    ok(
        &router,
        "UpdateLocationObjectStorage",
        serde_json::json!({
            "LocationArn": arn,
            "CmkSecretConfig": { "KmsKeyArn": kms_key_arn() },
        }),
        case,
    )
    .await;
    let described = ok(
        &router,
        "DescribeLocationObjectStorage",
        serde_json::json!({ "LocationArn": arn }),
        case,
    )
    .await;
    assert_eq!(
        (
            string(&described["CmkSecretConfig"], "SecretArn", case),
            string(&described["CmkSecretConfig"], "KmsKeyArn", case),
            described["ManagedSecretConfig"].is_null(),
        ),
        (secret_arn, kms_key_arn(), true),
        "case {case:?}: the secret must stay where it is and only its key change, got {described}"
    );
}

/// An object storage location is a transfer endpoint like any other: `CreateTask`
/// resolves it from the same store the `CreateLocation*` operations write to.
#[tokio::test]
async fn an_object_storage_location_can_be_a_task_endpoint() {
    let case = "a task between two object storage locations";
    let router = service();
    let mut endpoints: Vec<String> = Vec::new();
    for bucket in ["task-source", "task-destination"] {
        let created = ok(
            &router,
            "CreateLocationObjectStorage",
            serde_json::json!({
                "ServerHostname": "endpoints.example.com",
                "BucketName": bucket,
            }),
            case,
        )
        .await;
        endpoints.push(string(&created, "LocationArn", case));
    }

    let created = ok(
        &router,
        "CreateTask",
        serde_json::json!({
            "SourceLocationArn": endpoints[0],
            "DestinationLocationArn": endpoints[1],
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
        (
            described["SourceLocationArn"].as_str(),
            described["DestinationLocationArn"].as_str(),
        ),
        (
            Some(endpoints[0].as_str()),
            Some(endpoints[1].as_str()),
        ),
        "case {case:?}: the task must report the object storage locations it was given, got {described}"
    );
}
