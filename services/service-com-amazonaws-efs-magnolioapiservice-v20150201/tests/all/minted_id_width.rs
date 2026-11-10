// SPDX-License-Identifier: CC0-1.0

//! The FORM of every id this service mints, and of the ARNs built from them.
//!
//! RULE: an id EFS assigns to a resource it creates carries the body AWS mints that id
//! with — the resource's prefix followed by 17 lowercase hex digits — so the id, and every
//! ARN built from it, satisfies this service's own id patterns AND the narrower patterns
//! the services that consume these ids model for them.
//!
//! The narrowest published constraint on an EFS-minted value in this tree is Lambda's:
//! a function's file system is admitted only as
//! `arn:aws…:elasticfilesystem:<region>:<account>:access-point/fsap-[a-f0-9]{17}`
//! (`smithy_query(query="shape",shape="FileSystemConfig",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`),
//! which pins the width at exactly 17 and which an id of any other width fails. Amazon S3
//! Files models the same family as `fs-[0-9a-f]{17,40}` / `fsap-[0-9a-f]{17,40}`
//! (`services/service-com-amazonaws-s3files-s3files/src/ops/create_access_point.rs`), and
//! this service's own patterns admit 8..40 digits — so 17 is the one width every published
//! constraint over these ids accepts.
//!
//! The tables vary what the rule generalises over: WHICH resource the id names (file
//! system, mount target, the mount target's network interface and VPC, access point, and
//! the destination file system a replication configuration mints), and — for the ARN,
//! whose `<region>` segment the consuming pattern also constrains — which Region the
//! request is signed for.

use serde_json::{json, Value};

// `create_access_point` / `describe_access_points` are the `Emulator` methods the
// access-point suite already owns, and the rest of the harness is the one every suite in
// this crate drives this service through.
use crate::file_system_lifecycle::{settle, Emulator};

/// The number of hex digits AWS mints the body of these ids with, and the one width every
/// published pattern over them accepts; see this suite's own module docs for the sources.
const MINTED_ID_HEX: usize = 17;

/// The hex body of `id` when `id` is `prefix` followed by exactly [`MINTED_ID_HEX`]
/// lowercase hex digits, and `None` otherwise.
fn minted_body<'a>(id: &'a str, prefix: &str) -> Option<&'a str> {
    let body = id.strip_prefix(prefix)?;
    let hex = body.len() == MINTED_ID_HEX
        && body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    hex.then_some(body)
}

/// Assert `id` is a minted id of `prefix`, and report its hex body.
fn assert_minted(case: &str, id: &str, prefix: &str) -> String {
    let body = minted_body(id, prefix).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {prefix}<{MINTED_ID_HEX} lowercase hex digits>, got \
             {id:?}"
        )
    });
    body.to_string()
}

/// Whether `arn` is an EFS access point ARN Lambda admits as a function's file system,
/// decided by the pattern Lambda's own model carries for `FileSystemConfig$Arn` (quoted in
/// this suite's module docs); only the EFS alternative of that pattern is written here,
/// since an EFS access point is what this service mints.
fn lambda_mountable(arn: &str) -> bool {
    let pattern = ::regex::Regex::new(
        r"^arn:aws[a-zA-Z-]*:elasticfilesystem:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:access-point/fsap-[a-f0-9]{17}$",
    )
    .expect("the pattern Lambda's model carries compiles");
    pattern.is_match(arn)
}

#[tokio::test]
async fn every_minted_id_carries_the_width_aws_mints_it_with() {
    let efs = Emulator::new();

    // One of every resource this service mints an id for, created through the modelled
    // routes so each id is the one a client is handed.
    let created = efs
        .create_file_system(json!({ "CreationToken": "minted-id-width" }))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status, created.body
    );
    let file_system_id = created.string("FileSystemId");
    settle().await;

    let mount = efs
        .create_mount_target(json!({
            "FileSystemId": file_system_id,
            "SubnetId": "subnet-0123456789abcdef0",
        }))
        .await;
    assert_eq!(
        mount.status, 200,
        "CreateMountTarget: expected 200, got {} {:?}",
        mount.status, mount.body
    );

    let access_point = efs
        .create_access_point(json!({
            "ClientToken": "minted-id-width",
            "FileSystemId": file_system_id,
        }))
        .await;
    assert_eq!(
        access_point.status, 200,
        "CreateAccessPoint: expected 200, got {} {:?}",
        access_point.status, access_point.body
    );

    let replication = efs
        .create_replication_configuration(&file_system_id, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        replication.status, 200,
        "CreateReplicationConfiguration: expected 200, got {} {:?}",
        replication.status, replication.body
    );
    let destination_id = replication.get("Destinations")[0]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the id's prefix, the id the service minted). Every resource whose id this
    // service assigns is a row.
    let rows: Vec<(&str, &str, String)> = vec![
        ("the file system a create minted", "fs-", file_system_id),
        (
            "the mount target a create minted",
            "fsmt-",
            mount.string("MountTargetId"),
        ),
        (
            "the mount target's network interface",
            "eni-",
            mount.string("NetworkInterfaceId"),
        ),
        ("the VPC the mount target is on", "vpc-", mount.string("VpcId")),
        (
            "the access point a create minted",
            "fsap-",
            access_point.string("AccessPointId"),
        ),
        (
            "the destination file system a replication configuration minted",
            "fs-",
            destination_id,
        ),
    ];

    // (case, the hex body the id carries), one per row above.
    let bodies: Vec<(&str, String)> = rows
        .iter()
        .map(|(case, prefix, id)| (*case, assert_minted(case, id, prefix)))
        .collect();

    // Ids minted for distinct resources are distinct: the width is not bought by
    // truncating every id to one shared body.
    for (index, (case, body)) in bodies.iter().enumerate() {
        for (other_case, other_body) in bodies.iter().skip(index + 1) {
            assert_ne!(
                body, other_body,
                "case {case:?} vs {other_case:?}: two minted ids share the body {body:?}"
            );
        }
    }

    // The ARN a description reports is built from the minted id, so it carries the same
    // body.
    let file_system_arn = created.string("FileSystemArn");
    assert!(
        file_system_arn.ends_with(&format!("file-system/{}", created.string("FileSystemId"))),
        "the file system ARN names the minted id, got {file_system_arn:?}"
    );
    let access_point_arn = access_point.string("AccessPointArn");
    assert!(
        access_point_arn.ends_with(&format!(
            "access-point/{}",
            access_point.string("AccessPointId")
        )),
        "the access point ARN names the minted id, got {access_point_arn:?}"
    );
}

#[tokio::test]
async fn a_minted_access_point_arn_is_one_lambda_admits_as_a_file_system() {
    // (case, the Region the EFS request is signed for). The consuming pattern constrains
    // the ARN's `<region>` segment as well as its id, so the rule is asserted across
    // Regions of different shapes: two-letter and three-part names, a single-digit
    // ordinal, and names from four partitions' worth of geographies.
    let regions = [
        ("the Region the recorded flow ran in", "us-west-2"),
        ("the default Region of this suite", "us-east-1"),
        ("a European Region", "eu-central-1"),
        ("an Asia-Pacific Region", "ap-southeast-2"),
        ("a South American Region", "sa-east-1"),
    ];

    for (case, region) in regions {
        let efs = Emulator::in_region(region);
        let created = efs
            .create_file_system(json!({ "CreationToken": format!("mountable-{region}") }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status, created.body
        );
        let file_system_id = created.string("FileSystemId");
        settle().await;

        // Two access points per Region, so the assertion holds for every draw of the id
        // counter rather than for the first one only.
        for ordinal in 0..2 {
            let row = format!("{case} (access point {ordinal})");
            let access_point = efs
                .create_access_point(json!({
                    "ClientToken": format!("mountable-{region}-{ordinal}"),
                    "FileSystemId": file_system_id,
                    "PosixUser": { "Uid": 1001, "Gid": 1001 },
                    "RootDirectory": {
                        "Path": "/export/lambda",
                        "CreationInfo": {
                            "OwnerUid": 1001,
                            "OwnerGid": 1001,
                            "Permissions": "750",
                        },
                    },
                }))
                .await;
            assert_eq!(
                access_point.status, 200,
                "case {row:?}: CreateAccessPoint expected 200, got {} {:?}",
                access_point.status, access_point.body
            );
            let access_point_id = access_point.string("AccessPointId");
            assert_minted(&row, &access_point_id, "fsap-");

            let arn = access_point.string("AccessPointArn");
            assert!(
                lambda_mountable(&arn),
                "case {row:?}: the minted ARN must satisfy the pattern a consumer admits \
                 an EFS access point by, got {arn:?}"
            );

            // ROUND TRIP: the ARN a later read reports is the same mountable ARN, so a
            // client that discovers the access point instead of creating it gets a value
            // it can hand on too.
            let described = efs
                .describe_access_points(&format!("AccessPointId={access_point_id}"))
                .await;
            assert_eq!(
                described.status, 200,
                "case {row:?}: DescribeAccessPoints expected 200, got {} {:?}",
                described.status, described.body
            );
            let reported = described.get("AccessPoints")[0]["AccessPointArn"].clone();
            assert_eq!(
                reported,
                Value::String(arn.clone()),
                "case {row:?}: the read reports the ARN the create minted, got {reported:?}"
            );
        }
    }
}
