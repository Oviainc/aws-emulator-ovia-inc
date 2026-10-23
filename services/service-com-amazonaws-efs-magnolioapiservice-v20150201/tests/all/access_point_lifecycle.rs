// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EFS access-point slice: CreateAccessPoint and the
//! DescribeAccessPoints read it round-trips through, driven over HTTP through the same
//! `MultiServiceRouter` the production entrypoint serves.
//!
//! Every rule is asserted over a SET of inputs rather than on one call: the tables below
//! vary the member each rule generalises over (the POSIX identity, the root directory's
//! depth and creation info, the tag list, the id form, the Region, the page size), and
//! the single-row cases are the published or modelled boundaries.
//!
//! The harness (`Emulator`, `Reply`, `settle`, `matches_id_pattern`) is the one
//! `file_system_lifecycle` already drives this service through.

use serde_json::{json, Value};

use crate::file_system_lifecycle::{matches_id_pattern, settle, Emulator, Reply};

impl Emulator {
    /// `POST /2015-02-01/access-points`, the modelled `CreateAccessPoint` binding.
    pub(crate) async fn create_access_point(&self, body: Value) -> Reply {
        self.send("POST", "/2015-02-01/access-points", Some(body))
            .await
    }

    /// `GET /2015-02-01/access-points`, the modelled `DescribeAccessPoints` binding.
    pub(crate) async fn describe_access_points(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/2015-02-01/access-points".to_string()
        } else {
            format!("/2015-02-01/access-points?{query}")
        };
        self.send("GET", &path, None).await
    }
}

/// Assert every member `expected` names is reported with that value.
fn assert_members(label: &str, actual: &Value, expected: &Value) {
    for (key, want) in expected.as_object().expect("an object of members") {
        assert_eq!(
            &actual[key], want,
            "{label}: expected {key} {want:?}, got {:?} in {actual:?}",
            actual[key]
        );
    }
}

/// The access point descriptions one `DescribeAccessPoints` answer carries.
fn access_points(reply: &Reply) -> Vec<Value> {
    reply
        .get("AccessPoints")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The ids of those descriptions, in the order they were reported.
pub(crate) fn ids_of(reply: &Reply) -> Vec<String> {
    access_points(reply)
        .iter()
        .map(|description| {
            description["AccessPointId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the round-trip table: a creation request and the description it must
/// produce, both on the create answer and on the later read.
struct Shape {
    case: &'static str,
    /// `PosixUser`, or `None` to leave the member out of the request.
    posix_user: Option<Value>,
    /// `RootDirectory`, or `None` to leave the member out of the request.
    root_directory: Option<Value>,
    tags: Vec<Value>,
    /// The `RootDirectory` the description must report.
    expected_root_directory: Value,
    /// "The name of the access point. This is the value of the Name tag."
    expected_name: Option<&'static str>,
}

fn round_trip_table() -> Vec<Shape> {
    vec![
        Shape {
            case: "neither a POSIX identity nor a root directory",
            posix_user: None,
            root_directory: None,
            // With no RootDirectory named, the access point exposes the file system root.
            expected_root_directory: json!({ "Path": "/" }),
            tags: Vec::new(),
            expected_name: None,
        },
        Shape {
            case: "a POSIX identity without secondary groups",
            posix_user: Some(json!({ "Uid": 1000, "Gid": 1000 })),
            root_directory: None,
            expected_root_directory: json!({ "Path": "/" }),
            tags: Vec::new(),
            expected_name: None,
        },
        Shape {
            // `PosixUser$Uid`/`$Gid` accept "values from 0 to 2^32 (4294967295)" and
            // `SecondaryGids` is modelled `length 0..16`, so this row is every bound at
            // once.
            case: "a POSIX identity at the modelled bounds, with 16 secondary groups",
            posix_user: Some(json!({
                "Uid": 4_294_967_295u64,
                "Gid": 4_294_967_295u64,
                "SecondaryGids": (1..=16).collect::<Vec<i64>>(),
            })),
            root_directory: None,
            expected_root_directory: json!({ "Path": "/" }),
            tags: Vec::new(),
            expected_name: None,
        },
        Shape {
            case: "a root directory at the file system root, with no creation info",
            posix_user: None,
            root_directory: Some(json!({ "Path": "/" })),
            expected_root_directory: json!({ "Path": "/" }),
            tags: Vec::new(),
            expected_name: None,
        },
        Shape {
            // `CreationInfo$Permissions` is modelled `^[0-7]{3,4}$`: three digits here,
            // four in the next row.
            case: "a one-level root directory with three-digit permissions",
            posix_user: Some(json!({ "Uid": 0, "Gid": 0 })),
            root_directory: Some(json!({
                "Path": "/app",
                "CreationInfo": { "OwnerUid": 0, "OwnerGid": 0, "Permissions": "755" },
            })),
            expected_root_directory: json!({
                "Path": "/app",
                "CreationInfo": { "OwnerUid": 0, "OwnerGid": 0, "Permissions": "755" },
            }),
            tags: Vec::new(),
            expected_name: None,
        },
        Shape {
            // "A path can have up to four subdirectories" (`RootDirectory$Path`), and
            // `CreationInfo$OwnerUid`/`$OwnerGid` accept up to 4294967295.
            case: "a four-level root directory with four-digit permissions",
            posix_user: None,
            root_directory: Some(json!({
                "Path": "/one/two/three/four",
                "CreationInfo": {
                    "OwnerUid": 4_294_967_295u64,
                    "OwnerGid": 4_294_967_295u64,
                    "Permissions": "0777",
                },
            })),
            expected_root_directory: json!({
                "Path": "/one/two/three/four",
                "CreationInfo": {
                    "OwnerUid": 4_294_967_295u64,
                    "OwnerGid": 4_294_967_295u64,
                    "Permissions": "0777",
                },
            }),
            tags: Vec::new(),
            expected_name: None,
        },
        Shape {
            case: "tags carrying a Name",
            posix_user: None,
            root_directory: Some(json!({ "Path": "/shared" })),
            expected_root_directory: json!({ "Path": "/shared" }),
            tags: vec![
                json!({ "Key": "Name", "Value": "app-view" }),
                json!({ "Key": "env", "Value": "test" }),
            ],
            expected_name: Some("app-view"),
        },
        Shape {
            case: "tags without a Name",
            posix_user: None,
            root_directory: None,
            expected_root_directory: json!({ "Path": "/" }),
            tags: vec![json!({ "Key": "team", "Value": "storage" })],
            expected_name: None,
        },
    ]
}

#[tokio::test]
async fn create_access_point_stores_every_shape_and_reads_it_back() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("access-points").await;

    let mut expectations: Vec<(&'static str, String, Value)> = Vec::new();
    for (index, shape) in round_trip_table().into_iter().enumerate() {
        let token = format!("round-trip-{index}");
        let mut body = json!({ "ClientToken": token, "FileSystemId": file_system_id });
        if let Some(posix_user) = shape.posix_user.clone() {
            body["PosixUser"] = posix_user;
        }
        if let Some(root_directory) = shape.root_directory.clone() {
            body["RootDirectory"] = root_directory;
        }
        if !shape.tags.is_empty() {
            body["Tags"] = Value::Array(shape.tags.clone());
        }

        let created = efs.create_access_point(body).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            shape.case, created.status, created.body
        );
        let access_point_id = created.string("AccessPointId");
        assert!(
            matches_id_pattern(&access_point_id, "fsap-"),
            "case {:?}: expected a minted fsap- id, got {access_point_id:?}",
            shape.case
        );

        let mut expected = json!({
            "AccessPointId": access_point_id,
            "AccessPointArn": format!(
                "arn:aws:elasticfilesystem:us-east-1:000000000000:access-point/{access_point_id}"
            ),
            "ClientToken": token,
            "FileSystemId": file_system_id,
            "OwnerId": "000000000000",
            "RootDirectory": shape.expected_root_directory.clone(),
            "Tags": Value::Array(shape.tags.clone()),
        });
        expected["PosixUser"] = shape.posix_user.clone().unwrap_or(Value::Null);
        expected["Name"] = match shape.expected_name {
            Some(name) => Value::String(name.to_string()),
            None => Value::Null,
        };

        assert_members(&format!("case {:?}: CreateAccessPoint", shape.case), &created.body, &expected);
        assert_eq!(
            created.string("LifeCycleState"),
            "creating",
            "case {:?}: a new access point is reported while still creating, got {:?}",
            shape.case,
            created.get("LifeCycleState")
        );
        expectations.push((shape.case, access_point_id, expected));
    }

    // Every description reads back from state, member for member.
    settle().await;
    for (case, access_point_id, expected) in &expectations {
        let read = efs
            .describe_access_points(&format!("AccessPointId={access_point_id}"))
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: DescribeAccessPoints expected 200, got {} {:?}",
            read.status, read.body
        );
        let described = access_points(&read);
        assert_eq!(
            described.len(),
            1,
            "case {case:?}: expected exactly the one access point, got {described:?}"
        );
        assert_members(
            &format!("case {case:?}: DescribeAccessPoints"),
            &described[0],
            expected,
        );
    }

    // All of them belong to the one file system, in creation order.
    let listed = efs
        .describe_access_points(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        ids_of(&listed),
        expectations
            .iter()
            .map(|(_, id, _)| id.clone())
            .collect::<Vec<String>>(),
        "DescribeAccessPoints(FileSystemId): expected every created access point in \
         creation order, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn access_point_moves_from_creating_to_available_on_the_clock_alone() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("transition").await;
    let created = efs
        .create_access_point(json!({
            "ClientToken": "transition-1",
            "FileSystemId": file_system_id,
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateAccessPoint: expected 200, got {} {:?}",
        created.status, created.body
    );
    let access_point_id = created.string("AccessPointId");
    let selector = format!("AccessPointId={access_point_id}");

    // Read straight away: the transition is not due yet, so it still reports `creating`.
    let immediate = efs.describe_access_points(&selector).await;
    assert_eq!(
        access_points(&immediate)[0]["LifeCycleState"],
        json!("creating"),
        "DescribeAccessPoints before the creating span: expected creating, got {:?}",
        immediate.body
    );

    // The clock owes `available`; after the span, and on every later read, that is what
    // the access point reports.
    settle().await;
    for read_number in 1..=3 {
        let settled = efs.describe_access_points(&selector).await;
        assert_eq!(
            access_points(&settled)[0]["LifeCycleState"],
            json!("available"),
            "read {read_number} after the creating span: expected available, got {:?}",
            settled.body
        );
    }
}

#[tokio::test]
async fn a_reused_client_token_never_mints_a_second_access_point() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("token-reuse").await;
    let other_file_system_id = efs.available_file_system("token-reuse-other").await;
    let first = efs
        .create_access_point(json!({
            "ClientToken": "reused",
            "FileSystemId": file_system_id,
            "PosixUser": { "Uid": 1, "Gid": 1 },
            "RootDirectory": { "Path": "/first" },
        }))
        .await;
    assert_eq!(
        first.status, 200,
        "the first CreateAccessPoint: expected 200, got {} {:?}",
        first.status, first.body
    );
    let access_point_id = first.string("AccessPointId");

    // The rule holds whatever the rest of the request says: identical parameters, every
    // single changed parameter, and a different file system alike.
    let repeats = [
        (
            "identical parameters",
            json!({
                "ClientToken": "reused",
                "FileSystemId": file_system_id,
                "PosixUser": { "Uid": 1, "Gid": 1 },
                "RootDirectory": { "Path": "/first" },
            }),
        ),
        (
            "a different POSIX identity",
            json!({
                "ClientToken": "reused",
                "FileSystemId": file_system_id,
                "PosixUser": { "Uid": 2, "Gid": 2 },
                "RootDirectory": { "Path": "/first" },
            }),
        ),
        (
            "a different root directory",
            json!({
                "ClientToken": "reused",
                "FileSystemId": file_system_id,
                "PosixUser": { "Uid": 1, "Gid": 1 },
                "RootDirectory": { "Path": "/second" },
            }),
        ),
        (
            "added tags",
            json!({
                "ClientToken": "reused",
                "FileSystemId": file_system_id,
                "Tags": [{ "Key": "Name", "Value": "renamed" }],
            }),
        ),
        (
            "a different file system",
            json!({
                "ClientToken": "reused",
                "FileSystemId": other_file_system_id,
            }),
        ),
    ];
    for (case, body) in repeats {
        let repeat = efs.create_access_point(body).await;
        assert_eq!(
            repeat.status, 409,
            "reuse with {case}: expected 409, got {} {:?}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.error_type.as_deref(),
            Some("AccessPointAlreadyExists"),
            "reuse with {case}: unexpected error type, got {:?}",
            repeat.error_type
        );
        assert_eq!(
            repeat.string("AccessPointId"),
            access_point_id,
            "reuse with {case}: the error must name the existing id, got {:?}",
            repeat.get("AccessPointId")
        );
        // Whatever the rest of the request said, the refusal carries the error's required
        // members only: AWS sends no `Message` member on this error class.
        crate::error_message_construction::assert_no_message_member(
            &format!("reuse with {case}"),
            &repeat,
        );
    }

    // Not one of those refusals changed state.
    for (label, id) in [
        ("the first file system", &file_system_id),
        ("the other file system", &other_file_system_id),
    ] {
        let listed = efs
            .describe_access_points(&format!("FileSystemId={id}"))
            .await;
        let expected = if id == &file_system_id {
            vec![access_point_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids_of(&listed),
            expected,
            "after every refused reuse, {label} must hold {expected:?}, got {:?}",
            listed.body
        );
    }

    // Not one of them touched the MEMBERS of the access point the token names either: a
    // refused retry that carried other parameters updates nothing.
    let described = efs
        .describe_access_points(&format!("AccessPointId={access_point_id}"))
        .await;
    assert_members(
        "after every refused reuse",
        &access_points(&described)[0],
        &json!({
            "RootDirectory": { "Path": "/first" },
            "PosixUser": { "Uid": 1, "Gid": 1 },
            "FileSystemId": file_system_id,
        }),
    );

    // A fresh token on the same file system mints a distinct access point.
    let distinct = efs
        .create_access_point(json!({
            "ClientToken": "not-reused",
            "FileSystemId": file_system_id,
        }))
        .await;
    assert_eq!(
        distinct.status, 200,
        "a fresh ClientToken: expected 200, got {} {:?}",
        distinct.status, distinct.body
    );
    assert_ne!(
        distinct.string("AccessPointId"),
        access_point_id,
        "a fresh ClientToken must mint a distinct id, got {:?}",
        distinct.get("AccessPointId")
    );
}

#[tokio::test]
async fn create_access_point_refuses_a_file_system_it_cannot_serve() {
    let efs = Emulator::new();

    // Every form a `FileSystemId` may take names the same absent file system, so each is
    // the same 404.
    let absent = [
        ("a short bare id", "fs-deadbeef".to_string()),
        ("a long bare id", "fs-00112233445566778899aabb".to_string()),
        (
            "an ARN",
            "arn:aws:elasticfilesystem:us-east-1:000000000000:file-system/fs-deadbeef"
                .to_string(),
        ),
    ];
    for (index, (case, file_system_id)) in absent.into_iter().enumerate() {
        let refused = efs
            .create_access_point(json!({
                "ClientToken": format!("absent-{index}"),
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            refused.status, 404,
            "{case}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("FileSystemNotFound"),
            "{case}: unexpected error type, got {:?}",
            refused.error_type
        );
    }

    // "Returned if the file system's lifecycle state is not available": a file system
    // still inside its creating span cannot take an access point yet.
    let created = efs
        .create_file_system(json!({ "CreationToken": "still-creating" }))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateFileSystem: expected 201, got {} {:?}",
        created.status, created.body
    );
    let file_system_id = created.string("FileSystemId");
    let too_early = efs
        .create_access_point(json!({
            "ClientToken": "too-early",
            "FileSystemId": file_system_id,
        }))
        .await;
    assert_eq!(
        too_early.status, 409,
        "CreateAccessPoint on a creating file system: expected 409, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "CreateAccessPoint on a creating file system: unexpected error type, got {:?}",
        too_early.error_type
    );

    // A refused create persisted nothing, and its ClientToken was not consumed: once the
    // file system is available the very same call succeeds.
    settle().await;
    let retried = efs
        .create_access_point(json!({
            "ClientToken": "too-early",
            "FileSystemId": file_system_id,
        }))
        .await;
    assert_eq!(
        retried.status, 200,
        "the same call once the file system is available: expected 200, got {} {:?}",
        retried.status, retried.body
    );
    assert_eq!(
        ids_of(
            &efs.describe_access_points(&format!("FileSystemId={file_system_id}"))
                .await
        ),
        vec![retried.string("AccessPointId")],
        "exactly the one access point the successful call minted must be stored"
    );
}

#[tokio::test]
async fn create_access_point_refuses_a_malformed_request() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("malformed").await;

    // "When specifying a RootDirectory, you must provide the Path"; "each key must be
    // unique".
    let malformed = [
        (
            "a RootDirectory without a Path",
            json!({
                "ClientToken": "malformed-1",
                "FileSystemId": file_system_id,
                "RootDirectory": {},
            }),
        ),
        (
            "a RootDirectory carrying only CreationInfo",
            json!({
                "ClientToken": "malformed-2",
                "FileSystemId": file_system_id,
                "RootDirectory": {
                    "CreationInfo": { "OwnerUid": 0, "OwnerGid": 0, "Permissions": "755" },
                },
            }),
        ),
        (
            "a repeated tag key",
            json!({
                "ClientToken": "malformed-3",
                "FileSystemId": file_system_id,
                "Tags": [
                    { "Key": "env", "Value": "one" },
                    { "Key": "env", "Value": "two" },
                ],
            }),
        ),
    ];
    for (case, body) in malformed {
        let refused = efs.create_access_point(body).await;
        assert_eq!(
            refused.status, 400,
            "{case}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("BadRequest"),
            "{case}: unexpected error type, got {:?}",
            refused.error_type
        );
    }
    assert!(
        ids_of(
            &efs.describe_access_points(&format!("FileSystemId={file_system_id}"))
                .await
        )
        .is_empty(),
        "a refused CreateAccessPoint must persist nothing"
    );
}

#[tokio::test]
async fn access_point_tags_are_accepted_up_to_the_published_quota_and_refused_past_it() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("tag-quota").await;
    // Published AWS limit: 50 tags per resource. The bound itself and one past it.
    for (case, count, expected_status) in [
        ("one tag", 1usize, 200u16),
        ("the published cap of 50 tags", 50, 200),
        ("one tag past the cap", 51, 400),
    ] {
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
            .collect();
        let reply = efs
            .create_access_point(json!({
                "ClientToken": format!("tags-{count}"),
                "FileSystemId": file_system_id,
                "Tags": tags,
            }))
            .await;
        assert_eq!(
            reply.status, expected_status,
            "{case}: expected {expected_status}, got {} {:?}",
            reply.status, reply.body
        );
        if expected_status == 200 {
            assert_eq!(
                reply.get("Tags").as_array().map(Vec::len),
                Some(count),
                "{case}: every tag must be reported back, got {:?}",
                reply.get("Tags")
            );
        }
    }
}

#[tokio::test]
async fn an_access_point_arn_names_the_region_the_request_was_signed_for() {
    // The ARN's Region segment comes from the request's SigV4 credential scope, so the
    // rule is checked in several Regions rather than only the default one.
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        let efs = Emulator::in_region(region);
        let file_system_id = efs.available_file_system("arn").await;
        let created = efs
            .create_access_point(json!({
                "ClientToken": "arn-1",
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "region {region}: expected 200, got {} {:?}",
            created.status, created.body
        );
        let access_point_id = created.string("AccessPointId");
        assert_eq!(
            created.string("AccessPointArn"),
            format!(
                "arn:aws:elasticfilesystem:{region}:000000000000:access-point/{access_point_id}"
            ),
            "region {region}: unexpected AccessPointArn, got {:?}",
            created.get("AccessPointArn")
        );
    }
}

#[tokio::test]
async fn describe_access_points_selects_by_either_id_and_refuses_both() {
    let efs = Emulator::new();
    let first_file_system = efs.available_file_system("select-1").await;
    let second_file_system = efs.available_file_system("select-2").await;

    let mut first_ids = Vec::new();
    for index in 0..3 {
        let created = efs
            .create_access_point(json!({
                "ClientToken": format!("select-first-{index}"),
                "FileSystemId": first_file_system,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "creating access point {index}: expected 200, got {} {:?}",
            created.status, created.body
        );
        first_ids.push(created.string("AccessPointId"));
    }
    let second_created = efs
        .create_access_point(json!({
            "ClientToken": "select-second",
            "FileSystemId": second_file_system,
        }))
        .await;
    assert_eq!(
        second_created.status, 200,
        "creating the other file system's access point: expected 200, got {} {:?}",
        second_created.status, second_created.body
    );
    let second_id = second_created.string("AccessPointId");

    // A FileSystemId selects that file system's access points only; an AccessPointId
    // selects exactly one; no selector at all reports every stored access point.
    let mut all_ids = first_ids.clone();
    all_ids.push(second_id.clone());
    for (case, query, expected) in [
        (
            "the first file system",
            format!("FileSystemId={first_file_system}"),
            first_ids.clone(),
        ),
        (
            "the second file system",
            format!("FileSystemId={second_file_system}"),
            vec![second_id.clone()],
        ),
        (
            "one access point by id",
            format!("AccessPointId={}", first_ids[1]),
            vec![first_ids[1].clone()],
        ),
        (
            "one access point by ARN",
            format!(
                "AccessPointId=arn:aws:elasticfilesystem:us-east-1:000000000000:access-point/{}",
                first_ids[2]
            ),
            vec![first_ids[2].clone()],
        ),
        ("no selector", String::new(), all_ids.clone()),
    ] {
        let reply = efs.describe_access_points(&query).await;
        assert_eq!(
            reply.status, 200,
            "{case}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            ids_of(&reply),
            expected,
            "{case}: unexpected access points, got {:?}",
            reply.body
        );
    }

    // "You can provide either an AccessPointId or a FileSystemId in the request, but not
    // both."
    let both = efs
        .describe_access_points(&format!(
            "AccessPointId={}&FileSystemId={first_file_system}",
            first_ids[0]
        ))
        .await;
    assert_eq!(
        both.status, 400,
        "both selectors: expected 400, got {} {:?}",
        both.status, both.body
    );
    assert_eq!(
        both.error_type.as_deref(),
        Some("BadRequest"),
        "both selectors: unexpected error type, got {:?}",
        both.error_type
    );

    // An id nothing is stored for is a 404 for each kind of selector, never an empty
    // list.
    for (case, query, expected_error) in [
        (
            "an unknown access point",
            "AccessPointId=fsap-deadbeef".to_string(),
            "AccessPointNotFound",
        ),
        (
            "an unknown file system",
            "FileSystemId=fs-deadbeef".to_string(),
            "FileSystemNotFound",
        ),
    ] {
        let reply = efs.describe_access_points(&query).await;
        assert_eq!(
            reply.status, 404,
            "{case}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(expected_error),
            "{case}: unexpected error type, got {:?}",
            reply.error_type
        );
    }
}

#[tokio::test]
async fn describe_access_points_pages_consistently_from_its_next_token() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("paging").await;
    let mut ids = Vec::new();
    for index in 0..5 {
        let created = efs
            .create_access_point(json!({
                "ClientToken": format!("page-{index}"),
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "creating access point {index}: expected 200, got {} {:?}",
            created.status, created.body
        );
        ids.push(created.string("AccessPointId"));
    }

    // Whatever the page size, walking the pages yields every access point exactly once,
    // in creation order: the token is where the next page resumes.
    for page_size in [1usize, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        for request_number in 0..=ids.len() {
            let mut query = format!("FileSystemId={file_system_id}&MaxResults={page_size}");
            if let Some(token) = &next_token {
                query.push_str(&format!("&NextToken={token}"));
            }
            let reply = efs.describe_access_points(&query).await;
            assert_eq!(
                reply.status, 200,
                "page size {page_size}, request {request_number}: expected 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = ids_of(&reply);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "page size {page_size}, request {request_number}: expected 1..={page_size} \
                 descriptions, got {page:?}"
            );
            seen.extend(page);
            next_token = reply.get("NextToken").as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            next_token, None,
            "page size {page_size}: the walk must end without a NextToken, got {next_token:?}"
        );
        assert_eq!(
            seen, ids,
            "page size {page_size}: expected every access point once in creation order"
        );
    }

    // A token that names nothing in the result set is a 400, not a silent first page.
    let stale = efs
        .describe_access_points(&format!(
            "FileSystemId={file_system_id}&NextToken=fsap-deadbeef"
        ))
        .await;
    assert_eq!(
        stale.status, 400,
        "a NextToken outside the result set: expected 400, got {} {:?}",
        stale.status, stale.body
    );
    assert_eq!(
        stale.error_type.as_deref(),
        Some("BadRequest"),
        "a NextToken outside the result set: unexpected error type, got {:?}",
        stale.error_type
    );
}
