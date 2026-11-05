// SPDX-License-Identifier: CC0-1.0

//! Deleting a DataSync transfer location, and what a read of a location the account
//! does not hold answers, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router. Nothing reaches into the state struct, so a response that is
//! not computed from persisted state cannot pass.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The discriminator DataSync reports, in both `errorCode` and `datasyncErrorCode`, for
/// a request that names a location the account does not hold.
const LOCATION_NOT_FOUND: &str = "LocationNotFound";

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

/// Assert the whole refusal envelope a request naming an absent location earns: the
/// message that names it, and the one discriminator in both code members.
fn assert_location_not_found(refusal: &serde_json::Value, location_arn: &str, case: &str) {
    assert_eq!(
        string(refusal, "message", case),
        format!("Location {location_arn} is not found."),
        "case {case:?}: the refusal must name the location it could not find, got {refusal}"
    );
    for member in ["errorCode", "datasyncErrorCode"] {
        assert_eq!(
            string(refusal, member, case),
            LOCATION_NOT_FOUND,
            "case {case:?}: expected {member} {LOCATION_NOT_FOUND:?}, got {refusal}"
        );
    }
}

/// An agent ARN in the modelled `AgentArn` shape.
fn agent_arn(tail: char) -> String {
    format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:agent/agent-0123456789abcdef{tail}")
}

/// One kind of transfer location: the create that mints it, and the describe that
/// reports it.
struct Kind {
    case: &'static str,
    create_operation: &'static str,
    describe_operation: &'static str,
    request: fn() -> serde_json::Value,
}

/// Every kind of location an implemented `CreateLocation*` mints, so the rule is
/// asserted over the whole set rather than the one kind the report cited. The members
/// of each request are the ones that operation models as required, plus what the
/// matching suite in this crate already exercises.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            case: "an object storage location",
            create_operation: "CreateLocationObjectStorage",
            describe_operation: "DescribeLocationObjectStorage",
            request: || {
                serde_json::json!({
                    "ServerHostname": "store.example.com",
                    "BucketName": "transfer-bucket",
                    "ServerProtocol": "HTTPS",
                    "Subdirectory": "/inbox",
                    "AccessKey": "transfer-user",
                    "SecretKey": "transfer-secret",
                    "AgentArns": [agent_arn('0')],
                })
            },
        },
        Kind {
            case: "an Amazon S3 location",
            create_operation: "CreateLocationS3",
            describe_operation: "DescribeLocationS3",
            request: || {
                serde_json::json!({
                    "S3BucketArn": "arn:aws:s3:::transfer-bucket",
                    "S3Config": {
                        "BucketAccessRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/datasync"),
                    },
                })
            },
        },
        Kind {
            case: "an Azure Blob Storage location",
            create_operation: "CreateLocationAzureBlob",
            describe_operation: "DescribeLocationAzureBlob",
            request: || {
                serde_json::json!({
                    "ContainerUrl": "https://hotstore.blob.core.windows.net/transfer-in",
                    "AuthenticationType": "SAS",
                    "SasConfiguration": { "Token": "sv=2026-01-01&sig=abc" },
                    "AgentArns": [agent_arn('1')],
                })
            },
        },
        Kind {
            case: "an Amazon EFS location",
            create_operation: "CreateLocationEfs",
            describe_operation: "DescribeLocationEfs",
            request: || {
                serde_json::json!({
                    "EfsFilesystemArn": format!(
                        "arn:aws:elasticfilesystem:{SIGNED_REGION}:{ACCOUNT}:file-system/fs-08e04cd4"
                    ),
                    "Ec2Config": {
                        "SubnetArn": format!(
                            "arn:aws:ec2:{SIGNED_REGION}:{ACCOUNT}:subnet/subnet-1234567890abcdef1"
                        ),
                        "SecurityGroupArns": [format!(
                            "arn:aws:ec2:{SIGNED_REGION}:{ACCOUNT}:security-group/sg-1234567890abcdef2"
                        )],
                    },
                })
            },
        },
    ]
}

/// `DeleteLocation` removes the location its ARN addresses, whatever kind it is, and
/// afterwards that ARN resolves to nothing: the matching `DescribeLocation*` refuses it
/// as a location the account does not hold, and so does a second delete.
#[tokio::test]
async fn deleting_a_location_of_any_kind_leaves_nothing_to_read() {
    let router = service();

    for kind in kinds() {
        let case = kind.case;
        let created = ok(&router, kind.create_operation, (kind.request)(), case).await;
        let arn = string(&created, "LocationArn", case);

        // The location is readable while it is held, so the refusal below is the
        // delete's doing and not the describe's.
        let described = ok(
            &router,
            kind.describe_operation,
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;
        assert_eq!(
            string(&described, "LocationArn", case),
            arn,
            "case {case:?}: the describe must report the location it was asked for, got {described}"
        );

        let deleted = ok(
            &router,
            "DeleteLocation",
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;
        assert_eq!(
            deleted,
            serde_json::json!({}),
            "case {case:?}: DeleteLocation reports no members, got {deleted}"
        );

        let refusal = refused(
            &router,
            kind.describe_operation,
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;
        assert_location_not_found(&refusal, &arn, case);

        // Nothing is left to delete either: the second delete is refused in the same
        // words, so the first one really removed the record.
        let second = refused(
            &router,
            "DeleteLocation",
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;
        assert_location_not_found(&second, &arn, case);
    }
}

/// A delete removes ONLY the location its ARN addresses: every other location the
/// account holds is still reported from state afterwards.
#[tokio::test]
async fn deleting_one_location_leaves_every_other_location_readable() {
    let router = service();
    let case = "four locations, one deleted at a time";

    let kinds = kinds();
    let mut held: Vec<(usize, String)> = Vec::new();
    for (index, kind) in kinds.iter().enumerate() {
        let created = ok(&router, kind.create_operation, (kind.request)(), case).await;
        held.push((index, string(&created, "LocationArn", case)));
    }

    while let Some((index, arn)) = held.pop() {
        ok(
            &router,
            "DeleteLocation",
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;
        let refusal = refused(
            &router,
            kinds[index].describe_operation,
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;
        assert_location_not_found(&refusal, &arn, case);

        for (survivor_index, survivor) in &held {
            let described = ok(
                &router,
                kinds[*survivor_index].describe_operation,
                serde_json::json!({ "LocationArn": survivor }),
                case,
            )
            .await;
            assert_eq!(
                &string(&described, "LocationArn", case),
                survivor,
                "case {case:?}: deleting {arn} must leave {survivor} readable, got {described}"
            );
        }
    }
}

/// A well-formed location ARN the account never held resolves to nothing: every
/// operation that resolves a `LocationArn` refuses it with the same envelope.
///
/// The ARNs span what the modelled `LocationArn` pattern generalises over — each
/// partition the pattern admits
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):datasync:[a-z\-0-9]+:[0-9]{12}:location/loc-[0-9a-z]{17}$`),
/// a region segment other than the signed one, another account's id, and a location id
/// of digits only, letters only and both.
#[tokio::test]
async fn a_location_the_account_never_held_is_refused_by_every_reader() {
    let router = service();

    let absent = [
        format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-0abcdef1234567890"),
        format!("arn:aws:datasync:eu-west-1:{ACCOUNT}:location/loc-00000000000000001"),
        "arn:aws-cn:datasync:cn-north-1:123456789012:location/loc-abcdefghijklmnopq".to_string(),
        "arn:aws-us-gov:datasync:us-gov-west-1:210987654321:location/loc-12345678901234567"
            .to_string(),
        "arn:aws-iso-b:datasync:us-isob-east-1:555555555555:location/loc-0a1b2c3d4e5f60718"
            .to_string(),
    ];

    for arn in &absent {
        for operation in [
            "DeleteLocation",
            "DescribeLocationObjectStorage",
            "DescribeLocationS3",
            "DescribeLocationAzureBlob",
            "DescribeLocationEfs",
        ] {
            let case = format!("{operation} on {arn}");
            let refusal = refused(
                &router,
                operation,
                serde_json::json!({ "LocationArn": arn }),
                &case,
            )
            .await;
            assert_location_not_found(&refusal, arn, &case);
        }

        // An update resolves the same ARN, so it owes the same refusal.
        let case = format!("UpdateLocationObjectStorage on {arn}");
        let refusal = refused(
            &router,
            "UpdateLocationObjectStorage",
            serde_json::json!({ "LocationArn": arn, "AccessKey": "someone" }),
            &case,
        )
        .await;
        assert_location_not_found(&refusal, arn, &case);
    }
}

/// A task cannot be created against a location that was deleted: `CreateTask` resolves
/// both of its location ARNs in state and reports the same discriminator for one that
/// names nothing.
#[tokio::test]
async fn create_task_refuses_a_deleted_location() {
    let router = service();
    let case = "a task whose source location was deleted";
    let kinds = kinds();

    let mut arns: Vec<String> = Vec::new();
    for kind in kinds.iter().take(2) {
        let created = ok(&router, kind.create_operation, (kind.request)(), case).await;
        arns.push(string(&created, "LocationArn", case));
    }

    ok(
        &router,
        "DeleteLocation",
        serde_json::json!({ "LocationArn": arns[0] }),
        case,
    )
    .await;

    let refusal = refused(
        &router,
        "CreateTask",
        serde_json::json!({
            "SourceLocationArn": arns[0],
            "DestinationLocationArn": arns[1],
        }),
        case,
    )
    .await;
    for member in ["errorCode", "datasyncErrorCode"] {
        assert_eq!(
            string(&refusal, member, case),
            LOCATION_NOT_FOUND,
            "case {case:?}: expected {member} {LOCATION_NOT_FOUND:?}, got {refusal}"
        );
    }
    // The location is identified by its own resource id under either wording of this
    // refusal, so the assertion holds whichever one CreateTask reports.
    let location_id = arns[0].rsplit('/').next().unwrap_or(arns[0].as_str());
    assert!(
        string(&refusal, "message", case).contains(location_id),
        "case {case:?}: the refusal must name the location it could not find, got {refusal}"
    );

    // The destination survived, so the refusal is about the deleted location only.
    ok(
        &router,
        kinds[1].describe_operation,
        serde_json::json!({ "LocationArn": arns[1] }),
        case,
    )
    .await;
}
