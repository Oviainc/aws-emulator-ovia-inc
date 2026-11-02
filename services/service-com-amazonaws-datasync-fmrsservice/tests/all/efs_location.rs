// SPDX-License-Identifier: CC0-1.0

//! Amazon EFS transfer locations, driven end to end over HTTP.
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

/// The `EfsInTransitEncryption` value that encrypts the mount, from the modelled enum
/// (`smithy_query(query="shape",shape="EfsInTransitEncryption")` -> `NONE | TLS1_2`).
const TLS: &str = "TLS1_2";

/// The other value that enum admits: no Transport Layer Security on the mount.
const NO_TLS: &str = "NONE";

/// An IAM role that lets DataSync reach a file system, as the modelled `IamRoleArn`
/// spells one.
const FILE_SYSTEM_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/datasync-efs-access";

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

/// The modelled `EfsFilesystemArn` for a file system in `region`
/// (`…:elasticfilesystem:<region>:<account>:file-system/fs-[0-9a-f]{8,40}`).
fn filesystem_arn(region: &str, filesystem_id: &str) -> String {
    format!("arn:aws:elasticfilesystem:{region}:{ACCOUNT}:file-system/{filesystem_id}")
}

/// The modelled `Ec2SubnetArn` for a subnet in `region`.
fn subnet_arn(region: &str, subnet_id: &str) -> String {
    format!("arn:aws:ec2:{region}:{ACCOUNT}:subnet/{subnet_id}")
}

/// The modelled `Ec2SecurityGroupArn`s for `ids`, in `region`.
fn security_group_arns(region: &str, ids: &[&str]) -> Vec<String> {
    ids.iter()
        .map(|id| format!("arn:aws:ec2:{region}:{ACCOUNT}:security-group/{id}"))
        .collect()
}

/// The modelled `EfsAccessPointArn` for an access point in `region`
/// (`…:elasticfilesystem:<region>:<account>:access-point/fsap-[0-9a-f]{8,40}`).
fn access_point_arn(region: &str, access_point_id: &str) -> String {
    format!("arn:aws:elasticfilesystem:{region}:{ACCOUNT}:access-point/{access_point_id}")
}

/// One `CreateLocationEfs` request and what the stored location must report.
struct Row {
    case: &'static str,
    /// The Region of the file system, its subnet and its security groups — not the
    /// Region the request is signed for.
    region: &'static str,
    filesystem_id: &'static str,
    subnet_id: &'static str,
    security_group_ids: &'static [&'static str],
    subdirectory: Option<&'static str>,
    access_point_id: Option<&'static str>,
    file_system_access_role_arn: Option<&'static str>,
    in_transit_encryption: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    expected_uri: &'static str,
}

/// The request body a row asks for.
fn request_of(row: &Row) -> serde_json::Value {
    let mut request = serde_json::json!({
        "EfsFilesystemArn": filesystem_arn(row.region, row.filesystem_id),
        "Ec2Config": {
            "SubnetArn": subnet_arn(row.region, row.subnet_id),
            "SecurityGroupArns": security_group_arns(row.region, row.security_group_ids),
        },
    });
    if let Some(subdirectory) = row.subdirectory {
        request["Subdirectory"] = serde_json::json!(subdirectory);
    }
    if let Some(access_point_id) = row.access_point_id {
        request["AccessPointArn"] = serde_json::json!(access_point_arn(row.region, access_point_id));
    }
    if let Some(role) = row.file_system_access_role_arn {
        request["FileSystemAccessRoleArn"] = serde_json::json!(role);
    }
    if let Some(in_transit_encryption) = row.in_transit_encryption {
        request["InTransitEncryption"] = serde_json::json!(in_transit_encryption);
    }
    if !row.tags.is_empty() {
        request["Tags"] = serde_json::json!(
            row.tags
                .iter()
                .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
                .collect::<Vec<_>>()
        );
    }
    request
}

/// The rows every success case in this suite spans.
///
/// They differ along what the rule generalises over: the `Subdirectory` spellings that
/// denote one mount path (absent, `/`, with and without surrounding slashes, nested),
/// the Region and id of the file system the URI is assembled from, the number of
/// security groups the `Ec2Config` carries (the modelled list is `len 1..5`, so one and
/// five are its bounds), a restricted mount reached through an access point or an IAM
/// role (either of which requires `TLS1_2`), an explicit `NONE` encryption, and tags
/// versus none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "file system root, nothing optional declared",
            region: "us-east-2",
            filesystem_id: "fs-08e04cd4",
            subnet_id: "subnet-1234567890abcdef1",
            security_group_ids: &["sg-1234567890abcdef2"],
            subdirectory: None,
            access_point_id: None,
            file_system_access_role_arn: None,
            in_transit_encryption: None,
            tags: &[],
            expected_uri: "efs://us-east-2.fs-08e04cd4/",
        },
        Row {
            case: "the root spelled as a single slash",
            region: "us-east-2",
            filesystem_id: "fs-08e04cd4",
            subnet_id: "subnet-1234567890abcdef1",
            security_group_ids: &["sg-1234567890abcdef2"],
            subdirectory: Some("/"),
            access_point_id: None,
            file_system_access_role_arn: None,
            in_transit_encryption: Some(NO_TLS),
            tags: &[("Name", "efs-root")],
            expected_uri: "efs://us-east-2.fs-08e04cd4/",
        },
        Row {
            case: "a mount path with a leading slash",
            region: "eu-west-1",
            filesystem_id: "fs-0123456789abcdef0",
            subnet_id: "subnet-00ff11ee22dd33cc4",
            security_group_ids: &["sg-00ff11ee22dd33cc5", "sg-00ff11ee22dd33cc6"],
            subdirectory: Some("/path/to/folder"),
            access_point_id: None,
            file_system_access_role_arn: None,
            in_transit_encryption: None,
            tags: &[],
            expected_uri: "efs://eu-west-1.fs-0123456789abcdef0/path/to/folder/",
        },
        Row {
            case: "the same mount path spelled with a trailing slash",
            region: "eu-west-1",
            filesystem_id: "fs-0123456789abcdef0",
            subnet_id: "subnet-00ff11ee22dd33cc4",
            security_group_ids: &["sg-00ff11ee22dd33cc5"],
            subdirectory: Some("path/to/folder/"),
            access_point_id: None,
            file_system_access_role_arn: None,
            in_transit_encryption: None,
            tags: &[("Name", "efs-folder"), ("Stage", "test")],
            expected_uri: "efs://eu-west-1.fs-0123456789abcdef0/path/to/folder/",
        },
        Row {
            case: "a restricted mount through an access point",
            region: "ap-southeast-2",
            filesystem_id: "fs-abcdef0123456789abcdef0123456789abcdef01",
            subnet_id: "subnet-0a1b2c3d4e5f60718",
            security_group_ids: &[
                "sg-0a1b2c3d4e5f60719",
                "sg-0a1b2c3d4e5f6071a",
                "sg-0a1b2c3d4e5f6071b",
                "sg-0a1b2c3d4e5f6071c",
                "sg-0a1b2c3d4e5f6071d",
            ],
            subdirectory: Some("/restricted"),
            access_point_id: Some("fsap-0123456789abcdef0"),
            file_system_access_role_arn: None,
            in_transit_encryption: Some(TLS),
            tags: &[("Name", "efs-restricted")],
            expected_uri:
                "efs://ap-southeast-2.fs-abcdef0123456789abcdef0123456789abcdef01/restricted/",
        },
        Row {
            case: "a restricted mount through an IAM role",
            region: "us-west-2",
            filesystem_id: "fs-deadbeef",
            subnet_id: "subnet-0f1e2d3c4b5a69788",
            security_group_ids: &["sg-0f1e2d3c4b5a69789"],
            subdirectory: None,
            access_point_id: None,
            file_system_access_role_arn: Some(FILE_SYSTEM_ACCESS_ROLE_ARN),
            in_transit_encryption: Some(TLS),
            tags: &[],
            expected_uri: "efs://us-west-2.fs-deadbeef/",
        },
        Row {
            case: "an access point and an IAM role together",
            region: "us-west-2",
            filesystem_id: "fs-deadbeef",
            subnet_id: "subnet-0f1e2d3c4b5a69788",
            security_group_ids: &["sg-0f1e2d3c4b5a69789", "sg-0f1e2d3c4b5a6978a"],
            subdirectory: Some("shared/inbox"),
            access_point_id: Some("fsap-abcdef0123456789"),
            file_system_access_role_arn: Some(FILE_SYSTEM_ACCESS_ROLE_ARN),
            in_transit_encryption: Some(TLS),
            tags: &[("Name", "efs-both")],
            expected_uri: "efs://us-west-2.fs-deadbeef/shared/inbox/",
        },
    ]
}

/// `CreateLocationEfs` stores the location it mints, and `DescribeLocationEfs` reports
/// it from state.
#[tokio::test]
async fn created_efs_locations_are_reported_from_state() {
    let router = service();
    let mut arns: Vec<String> = Vec::new();

    for row in &rows() {
        let case = row.case;
        let created = ok(&router, "CreateLocationEfs", request_of(row), case).await;
        let arn = string(&created, "LocationArn", case);
        assert_location_arn(&arn, case);
        assert!(
            !arns.contains(&arn),
            "case {case:?}: every created location gets its own ARN, got {arn:?} again"
        );
        assert_eq!(
            created.as_object().map(|members| members.len()),
            Some(1),
            "case {case:?}: the response carries only the minted ARN, got {created}"
        );
        arns.push(arn.clone());

        let described = ok(
            &router,
            "DescribeLocationEfs",
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;

        assert_eq!(
            string(&described, "LocationArn", case),
            arn,
            "case {case:?}: the describe must report the ARN it was asked about, got {described}"
        );
        assert_eq!(
            string(&described, "LocationUri", case),
            row.expected_uri,
            "case {case:?}: expected the file system and mount path the request named, \
             got {described}"
        );
        assert_eq!(
            described["Ec2Config"],
            serde_json::json!({
                "SubnetArn": subnet_arn(row.region, row.subnet_id),
                "SecurityGroupArns": security_group_arns(row.region, row.security_group_ids),
            }),
            "case {case:?}: expected the subnet and security groups the request named, \
             got {described}"
        );

        let expected_access_point = row
            .access_point_id
            .map(|id| serde_json::json!(access_point_arn(row.region, id)))
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            described["AccessPointArn"], expected_access_point,
            "case {case:?}: expected the access point the request named, and none otherwise, \
             got {described}"
        );
        assert_eq!(
            described["FileSystemAccessRoleArn"],
            row.file_system_access_role_arn
                .map(|role| serde_json::json!(role))
                .unwrap_or(serde_json::Value::Null),
            "case {case:?}: expected the IAM role the request named, and none otherwise, \
             got {described}"
        );
        assert_eq!(
            described["InTransitEncryption"],
            row.in_transit_encryption
                .map(|value| serde_json::json!(value))
                .unwrap_or(serde_json::Value::Null),
            "case {case:?}: expected the encryption the request named, and none otherwise, \
             got {described}"
        );

        let creation_time = described["CreationTime"].as_f64().unwrap_or_default();
        assert!(
            creation_time > 0.0,
            "case {case:?}: expected the creation time the location was stored with, \
             got {described}"
        );

        // A read reports; it never changes what the next reader sees.
        let again = ok(
            &router,
            "DescribeLocationEfs",
            serde_json::json!({ "LocationArn": arn }),
            case,
        )
        .await;
        assert_eq!(
            again, described,
            "case {case:?}: two reads of one location must agree, got {again} then {described}"
        );
    }
}

/// Two requests carrying the same file system address two independent locations, each
/// reporting its own ARN: nothing about a location is derived from a hash of its request.
#[tokio::test]
async fn duplicate_efs_configurations_are_independent_locations() {
    let router = service();
    let mut arns: Vec<String> = Vec::new();

    for row in &rows() {
        let case = row.case;
        for attempt in 0..3 {
            let created = ok(&router, "CreateLocationEfs", request_of(row), case).await;
            let arn = string(&created, "LocationArn", case);
            assert_location_arn(&arn, case);
            assert!(
                !arns.contains(&arn),
                "case {case:?}: attempt {attempt} must mint a new location, got {arn:?} again"
            );
            arns.push(arn.clone());

            let described = ok(
                &router,
                "DescribeLocationEfs",
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
                row.expected_uri,
                "case {case:?}: attempt {attempt} must report its own file system, \
                 got {described}"
            );
        }
    }
}

/// A restricted mount is refused unless the request asks for TLS 1.2.
///
/// `InTransitEncryption`'s own documentation states the rule: an `AccessPointArn` or a
/// `FileSystemAccessRoleArn` makes `TLS1_2` the only accepted value, so each way of
/// leaving it unset or setting it to `NONE` is a client error.
#[tokio::test]
async fn a_restricted_efs_mount_requires_tls_in_transit() {
    let router = service();
    let region = "us-east-1";
    let rule = "you must set InTransitEncryption to TLS1_2";

    let base = serde_json::json!({
        "EfsFilesystemArn": filesystem_arn(region, "fs-01234567"),
        "Ec2Config": {
            "SubnetArn": subnet_arn(region, "subnet-0123456789abcdef0"),
            "SecurityGroupArns": security_group_arns(region, &["sg-0123456789abcdef1"]),
        },
    });

    let restricted: [(&str, serde_json::Value); 2] = [
        (
            "access point",
            serde_json::json!(access_point_arn(region, "fsap-0123456789abcdef0")),
        ),
        (
            "IAM role",
            serde_json::json!(FILE_SYSTEM_ACCESS_ROLE_ARN),
        ),
    ];

    for (named, value) in restricted {
        let member = if named == "access point" {
            "AccessPointArn"
        } else {
            "FileSystemAccessRoleArn"
        };
        for encryption in [None, Some(NO_TLS)] {
            let case = format!("{named} with encryption {encryption:?}");
            let mut request = base.clone();
            request[member] = value.clone();
            if let Some(encryption) = encryption {
                request["InTransitEncryption"] = serde_json::json!(encryption);
            }
            invalid_request(&router, "CreateLocationEfs", request, rule, &case).await;
        }

        // The same request with TLS 1.2 is accepted, so the refusal is about the
        // encryption and nothing else.
        let case = format!("{named} with encryption TLS1_2");
        let mut request = base.clone();
        request[member] = value.clone();
        request["InTransitEncryption"] = serde_json::json!(TLS);
        let created = ok(&router, "CreateLocationEfs", request, &case).await;
        assert_location_arn(&string(&created, "LocationArn", &case), &case);
    }
}

/// A location is reported by the describe of its own kind, and by no other; an ARN this
/// service never minted resolves to nothing.
#[tokio::test]
async fn an_efs_location_is_resolved_by_arn_and_by_kind() {
    let router = service();
    let case = "kind separation";
    let region = "us-east-1";

    let never_minted =
        format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-ffffffffffffffffa");
    invalid_request(
        &router,
        "DescribeLocationEfs",
        serde_json::json!({ "LocationArn": never_minted }),
        &format!("Location {never_minted} is not found."),
        "unknown location",
    )
    .await;

    let efs = string(
        &ok(
            &router,
            "CreateLocationEfs",
            serde_json::json!({
                "EfsFilesystemArn": filesystem_arn(region, "fs-0abcdef1"),
                "Ec2Config": {
                    "SubnetArn": subnet_arn(region, "subnet-0abcdef1234567890"),
                    "SecurityGroupArns": security_group_arns(region, &["sg-0abcdef1234567891"]),
                },
            }),
            case,
        )
        .await,
        "LocationArn",
        case,
    );
    let s3 = string(
        &ok(
            &router,
            "CreateLocationS3",
            serde_json::json!({
                "S3BucketArn": "arn:aws:s3:::other-kind-bucket",
                "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
            }),
            case,
        )
        .await,
        "LocationArn",
        case,
    );

    invalid_request(
        &router,
        "DescribeLocationEfs",
        serde_json::json!({ "LocationArn": s3 }),
        &format!("Location {s3} is not an Amazon EFS location."),
        "an S3 location read as Amazon EFS",
    )
    .await;
    invalid_request(
        &router,
        "DescribeLocationS3",
        serde_json::json!({ "LocationArn": efs }),
        &format!("Location {efs} is not an Amazon S3 location."),
        "an Amazon EFS location read as S3",
    )
    .await;
}

/// A transfer task can be built from Amazon EFS locations: `CreateTask` resolves a
/// stored EFS location like any other kind.
#[tokio::test]
async fn efs_locations_can_be_both_ends_of_a_task() {
    let case = "efs to efs";
    let router = service();
    let region = "us-west-2";

    let location = |filesystem_id: &'static str, subdirectory: &'static str| {
        serde_json::json!({
            "EfsFilesystemArn": filesystem_arn(region, filesystem_id),
            "Subdirectory": subdirectory,
            "Ec2Config": {
                "SubnetArn": subnet_arn(region, "subnet-0123456789abcdef0"),
                "SecurityGroupArns": security_group_arns(region, &["sg-0123456789abcdef1"]),
            },
        })
    };

    let source = string(
        &ok(
            &router,
            "CreateLocationEfs",
            location("fs-aaaa1111", "/outbox"),
            case,
        )
        .await,
        "LocationArn",
        case,
    );
    let destination = string(
        &ok(
            &router,
            "CreateLocationEfs",
            location("fs-bbbb2222", "/inbox"),
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
            "Name": "efs-task",
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
        "case {case:?}: expected the Amazon EFS source the task was created with, got {described}"
    );
    assert_eq!(
        string(&described, "DestinationLocationArn", case),
        destination,
        "case {case:?}: expected the Amazon EFS destination the task was created with, \
         got {described}"
    );
}
