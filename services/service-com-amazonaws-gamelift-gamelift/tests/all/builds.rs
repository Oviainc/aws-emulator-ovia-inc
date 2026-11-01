// SPDX-License-Identifier: CC0-1.0

//! The custom game build contract, over the wire: `CreateBuild` persists a build record for
//! the game server binaries the request points at, mints its id and ARN, places it in
//! `INITIALIZED` status, records the storage location the upload path decides and hands
//! back upload credentials only for the service-owned one — and `DescribeBuild` serves that
//! one stored record back, selected by build id or by build ARN.
//!
//! The harness (`GameLift`, `gamelift_arn`, `wait_for_due_transitions`) is the one
//! `hosting_loop` already drives the production dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::{gamelift_arn, wait_for_due_transitions, GameLift};

/// Every `OperatingSystem` a build may declare, from the model's own enum
/// (`smithy_query shape=OperatingSystem`).
const OPERATING_SYSTEMS: [&str; 6] = [
    "WINDOWS_2012",
    "AMAZON_LINUX",
    "AMAZON_LINUX_2",
    "WINDOWS_2016",
    "AMAZON_LINUX_2023",
    "WINDOWS_2022",
];

/// The `ServerSdkVersion` a build created without one carries: that member's own doc — "By
/// default Amazon GameLift Servers sets this value to `4.0.2`"
/// (`smithy_query operation=CreateBuild`).
const DEFAULT_SERVER_SDK_VERSION: &str = "4.0.2";

/// The status a successful create places a build in: "if successful, this operation creates
/// a new build resource with a unique build ID and places it in `INITIALIZED` status."
const STATUS_INITIALIZED: &str = "INITIALIZED";

/// The status a build whose files the service has read reaches: `Build$Status` — "`READY`
/// -- the game build has been successfully uploaded. You can now create new fleets for this
/// build."
const STATUS_READY: &str = "READY";

/// The modelled bounds of `CreateBuildInput$Name` and `$Version`
/// (`smithy_query operation=CreateBuild`: `NonZeroAndMaxString`, len 1..1024).
const MAX_NAME_LENGTH: usize = 1024;

/// The S3 bucket the rows below that pull build files from the CALLER's own bucket name.
const CALLER_BUCKET: &str = "my-own-game-builds";

/// A caller-owned S3 storage location naming the three members the operation documents as
/// required: "the storage location must specify an Amazon S3 bucket name and key. The
/// location must also specify a role ARN that you set up to allow Amazon GameLift Servers
/// to access your Amazon S3 bucket."
fn caller_location(key: &str) -> Value {
    json!({
        "Bucket": CALLER_BUCKET,
        "Key": key,
        "RoleArn": "arn:aws:iam::123456789012:role/GameLiftBuildAccess",
    })
}

/// Create the bucket named `bucket` through S3's own API, in the Region every request in
/// this suite is signed for: a `CreateBucket` sent to a Region-specific endpoint carries
/// that Region as its location constraint.
async fn create_bucket(gamelift: &GameLift, bucket: &str) {
    let body = format!(
        "<CreateBucketConfiguration><LocationConstraint>{}</LocationConstraint>\
         </CreateBucketConfiguration>",
        super::hosting_loop::REGION
    );
    let (status, response) = gamelift
        .s3("PUT", &format!("/{bucket}"), body.as_bytes())
        .await;
    assert_eq!(
        status,
        200,
        "the bucket {bucket} must be created, got {status} with body {}",
        String::from_utf8_lossy(&response)
    );
}

/// Where the build files a create request points at live: the two options CreateBuild
/// documents.
enum Source {
    /// An Amazon GameLift Servers location the caller uploads to, which a request that
    /// names no `StorageLocation` is handed.
    ServiceOwned,
    /// An S3 object the caller owns, named by `StorageLocation`.
    CallerBucket(Value),
}

/// The `BuildId` of a created build.
fn build_id_of(build: &Value) -> String {
    build["BuildId"]
        .as_str()
        .expect("CreateBuild must report a BuildId")
        .to_string()
}

/// The ARN a build with `build_id` is assigned:
/// `arn:aws:gamelift:<region>:<account>:build/<build id>` — the model's
/// `^arn:.*:build\/build-\S+$` with "the resource ID matches the `BuildId` value"
/// (`smithy_query shape=Build` member `BuildArn`).
fn build_arn(build_id: &str) -> String {
    gamelift_arn(&format!("build/{build_id}"))
}

/// `CreateBuild` stores the build the request configures and reports the stored record;
/// `DescribeBuild` serves the same record back, member for member, selected by id or by
/// ARN.
///
/// The rows differ along what the rule generalises over: which of the two documented upload
/// paths holds the build files, each documented operating system, which optional members
/// the request sets, an explicit server SDK version against the documented default, the
/// modelled name and version bounds, and how many tags the request assigns.
#[tokio::test]
async fn a_created_build_is_reported_as_stored_by_create_and_by_describe() {
    struct Case {
        label: &'static str,
        /// The request, without its `StorageLocation`.
        request: Value,
        source: Source,
        /// The members the created build must report beyond its id, ARN and creation time.
        expected: Value,
    }

    let cases = [
        Case {
            label: "an upload to the service-owned location, with a name and a version",
            request: json!({
                "Name": "space-shooter",
                "Version": "1.0.0",
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
            source: Source::ServiceOwned,
            expected: json!({
                "Name": "space-shooter",
                "Version": "1.0.0",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": DEFAULT_SERVER_SDK_VERSION,
            }),
        },
        Case {
            label: "an upload carrying no name and no version at all",
            request: json!({"OperatingSystem": OPERATING_SYSTEMS[0]}),
            source: Source::ServiceOwned,
            expected: json!({
                "OperatingSystem": OPERATING_SYSTEMS[0],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": DEFAULT_SERVER_SDK_VERSION,
            }),
        },
        Case {
            label: "an upload on the oldest Linux the model lists, with an explicit SDK version",
            request: json!({
                "Name": "linux-legacy",
                "OperatingSystem": OPERATING_SYSTEMS[1],
                "ServerSdkVersion": "5.1.2",
            }),
            source: Source::ServiceOwned,
            expected: json!({
                "Name": "linux-legacy",
                "OperatingSystem": OPERATING_SYSTEMS[1],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": "5.1.2",
            }),
        },
        Case {
            label: "an upload on Amazon Linux 2 with one tag",
            request: json!({
                "Name": "tagged-once",
                "OperatingSystem": OPERATING_SYSTEMS[2],
                "Tags": [{"Key": "owner", "Value": "liveops"}],
            }),
            source: Source::ServiceOwned,
            // `Build` carries no `Tags` member — tags are read back through
            // `ListTagsForResource` — so a tagged build reports what an untagged one does.
            expected: json!({
                "Name": "tagged-once",
                "OperatingSystem": OPERATING_SYSTEMS[2],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": DEFAULT_SERVER_SDK_VERSION,
            }),
        },
        Case {
            label: "an upload on Windows Server 2016 with three tags",
            request: json!({
                "Name": "tagged-thrice",
                "Version": "2026.02",
                "OperatingSystem": OPERATING_SYSTEMS[3],
                "Tags": [
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "env", "Value": "prod"},
                    {"Key": "cost-centre", "Value": "games"},
                ],
            }),
            source: Source::ServiceOwned,
            expected: json!({
                "Name": "tagged-thrice",
                "Version": "2026.02",
                "OperatingSystem": OPERATING_SYSTEMS[3],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": DEFAULT_SERVER_SDK_VERSION,
            }),
        },
        Case {
            label: "a name and a version at the modelled maximum length",
            request: json!({
                "Name": "n".repeat(MAX_NAME_LENGTH),
                "Version": "v".repeat(MAX_NAME_LENGTH),
                "OperatingSystem": OPERATING_SYSTEMS[5],
            }),
            source: Source::ServiceOwned,
            expected: json!({
                "Name": "n".repeat(MAX_NAME_LENGTH),
                "Version": "v".repeat(MAX_NAME_LENGTH),
                "OperatingSystem": OPERATING_SYSTEMS[5],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": DEFAULT_SERVER_SDK_VERSION,
            }),
        },
        Case {
            label: "a name and a version at the modelled minimum length",
            request: json!({
                "Name": "b",
                "Version": "1",
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
            source: Source::ServiceOwned,
            expected: json!({
                "Name": "b",
                "Version": "1",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": DEFAULT_SERVER_SDK_VERSION,
            }),
        },
        Case {
            label: "build files already in the caller's own S3 bucket",
            request: json!({
                "Name": "from-s3",
                "Version": "3.2.1",
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
            source: Source::CallerBucket(caller_location("builds/server.zip")),
            expected: json!({
                "Name": "from-s3",
                "Version": "3.2.1",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "Status": STATUS_INITIALIZED,
                // "If a StorageLocation is specified, the size of your file can be found in
                // your Amazon S3 bucket. Amazon GameLift Servers will report a SizeOnDisk
                // of 0."
                "SizeOnDisk": 0,
                "ServerSdkVersion": DEFAULT_SERVER_SDK_VERSION,
            }),
        },
        Case {
            label: "build files in a versioned S3 object, on an explicit SDK version",
            request: json!({
                "Name": "from-s3-versioned",
                "OperatingSystem": OPERATING_SYSTEMS[2],
                "ServerSdkVersion": "4.0.2",
            }),
            source: Source::CallerBucket(json!({
                "Bucket": CALLER_BUCKET,
                "Key": "builds/pinned.zip",
                "RoleArn": "arn:aws:iam::123456789012:role/GameLiftBuildAccess",
                "ObjectVersion": "3HL4kqtJlcpXroDTDmjVBH40Nrjfkd",
            })),
            expected: json!({
                "Name": "from-s3-versioned",
                "OperatingSystem": OPERATING_SYSTEMS[2],
                "Status": STATUS_INITIALIZED,
                "SizeOnDisk": 0,
                "ServerSdkVersion": "4.0.2",
            }),
        },
    ];

    let gamelift = GameLift::new();
    let mut minted_ids: Vec<String> = Vec::new();
    let mut minted_keys: Vec<String> = Vec::new();
    for case in &cases {
        let label = case.label;
        let mut request = case.request.clone();
        if let Source::CallerBucket(location) = &case.source {
            request["StorageLocation"] = location.clone();
        }

        let created = gamelift.ok("CreateBuild", request).await;
        let build = created["Build"].clone();
        let build_id = build_id_of(&build);
        assert!(
            build_id.starts_with("build-") && build_id.len() > "build-".len(),
            "case {label:?}: the minted BuildId must match the modelled ^build-\\S+$, got {build_id:?}"
        );
        assert!(
            !minted_ids.contains(&build_id),
            "case {label:?}: a build ID is unique, got the repeated {build_id:?}"
        );
        minted_ids.push(build_id.clone());

        // The storage location the upload path decides: the request's own location when the
        // files are in the caller's bucket, a service-owned one to upload to otherwise —
        // reported both on the build's own record and at the top level of the response.
        let location = created["StorageLocation"].clone();
        match &case.source {
            Source::CallerBucket(requested) => assert_eq!(
                &location, requested,
                "case {label:?}: a build whose files are in the caller's bucket must report the \
                 location the request provided, got {location}"
            ),
            Source::ServiceOwned => {
                let bucket = location["Bucket"].as_str().unwrap_or_default();
                let key = location["Key"].as_str().unwrap_or_default();
                assert!(
                    !bucket.is_empty() && bucket != CALLER_BUCKET,
                    "case {label:?}: a build with no storage location of its own must be handed a \
                     service-owned bucket to upload to, got {location}"
                );
                assert!(
                    key.contains(&build_id),
                    "case {label:?}: the service-owned storage location must name the build it \
                     holds, got {location}"
                );
            }
        }

        // "UploadCredentials ... is returned only when the operation is called without a
        // storage location", and the three parts of a set of Amazon Web Services security
        // credentials are "an access key ID, a secret access key, and a session token".
        let credentials = created["UploadCredentials"].clone();
        match &case.source {
            Source::CallerBucket(_) => assert_eq!(
                credentials,
                Value::Null,
                "case {label:?}: a build created WITH a storage location must be handed no upload \
                 credentials, got {credentials}"
            ),
            Source::ServiceOwned => {
                let mut parts: Vec<&str> = Vec::new();
                for member in ["AccessKeyId", "SecretAccessKey", "SessionToken"] {
                    let value = credentials[member].as_str().unwrap_or_default();
                    assert!(
                        !value.is_empty(),
                        "case {label:?}: the upload credentials must carry a {member}, got \
                         {credentials}"
                    );
                    parts.push(value);
                }
                let access_key_id = parts[0].to_string();
                assert!(
                    !minted_keys.contains(&access_key_id),
                    "case {label:?}: each build's upload credentials are its own, got the \
                     repeated access key {access_key_id:?}"
                );
                minted_keys.push(access_key_id);
            }
        }

        let mut expected = case.expected.clone();
        expected["BuildId"] = json!(&build_id);
        expected["BuildArn"] = json!(build_arn(&build_id));
        expected["CreationTime"] = build["CreationTime"].clone();
        assert!(
            build["CreationTime"].is_number(),
            "case {label:?}: a created build must carry a CreationTime, got {build}"
        );
        assert_eq!(
            build, expected,
            "case {label:?}: CreateBuild must report the stored build, got {build}"
        );

        // The describe path serves the SAME stored record, member for member — selected by
        // the id and by the ARN the create minted, which name the same build.
        for selector in [build_id.clone(), build_arn(&build_id)] {
            let described = gamelift
                .ok("DescribeBuild", json!({"BuildId": &selector}))
                .await;
            assert_eq!(
                described["Build"], expected,
                "case {label:?}: a describe naming {selector:?} must report the created record, got {}",
                described["Build"]
            );
        }
    }

    // Reading a build changed nothing: the same describe, run again, reports exactly what it
    // reported before.
    let first = gamelift
        .ok("DescribeBuild", json!({"BuildId": &minted_ids[0]}))
        .await;
    let second = gamelift
        .ok("DescribeBuild", json!({"BuildId": &minted_ids[0]}))
        .await;
    assert_eq!(
        first, second,
        "describing a build must not change what a later describe reports, got {second}"
    );
}

/// What the clock owes a build's status follows from whether its files are THERE: a build
/// whose files the service finds at the location the request named reaches `READY`, while
/// one whose location holds nothing — the service-owned location nobody has uploaded to
/// yet, or a caller's own key with no object at it — stays `INITIALIZED` however long a
/// caller waits.
#[tokio::test]
async fn a_build_stays_initialized_until_its_files_are_uploaded() {
    struct Case {
        label: &'static str,
        source: Source,
        /// The key in [`CALLER_BUCKET`] to upload build files to BEFORE the create, i.e.
        /// the case where the files the location names are already there.
        uploaded: Option<&'static str>,
        /// The status the build is in once every change the clock owes has come due.
        settled_status: &'static str,
    }

    let cases = [
        Case {
            label: "no storage location, so nothing has been uploaded yet",
            source: Source::ServiceOwned,
            uploaded: None,
            settled_status: STATUS_INITIALIZED,
        },
        Case {
            label: "build files already in the caller's own bucket",
            source: Source::CallerBucket(caller_location("builds/server.zip")),
            uploaded: Some("builds/server.zip"),
            settled_status: STATUS_READY,
        },
        Case {
            label: "a caller's own location that holds no build files",
            source: Source::CallerBucket(caller_location("builds/absent.zip")),
            uploaded: None,
            settled_status: STATUS_INITIALIZED,
        },
    ];

    let gamelift = GameLift::new();
    // The caller's own bucket, in the one object store this emulator holds, so the rows
    // below differ in whether the KEY they name holds an object rather than in whether the
    // bucket exists.
    create_bucket(&gamelift, CALLER_BUCKET).await;

    let mut created: Vec<(&str, String, &str)> = Vec::new();
    for case in &cases {
        let label = case.label;
        if let Some(key) = case.uploaded {
            let (status, response) = gamelift
                .s3("PUT", &format!("/{CALLER_BUCKET}/{key}"), b"PK\x03\x04build")
                .await;
            assert_eq!(
                status,
                200,
                "case {label:?}: the build files must upload to {CALLER_BUCKET}/{key}, got \
                 {status} with body {}",
                String::from_utf8_lossy(&response)
            );
        }

        let mut request = json!({"Name": "status-walk", "OperatingSystem": OPERATING_SYSTEMS[4]});
        if let Source::CallerBucket(location) = &case.source {
            request["StorageLocation"] = location.clone();
        }

        let build = gamelift.ok("CreateBuild", request).await["Build"].clone();
        // Whichever path the files take, the create itself "places it in INITIALIZED
        // status", so a caller reading straight after the call sees that status.
        assert_eq!(
            build["Status"],
            json!(STATUS_INITIALIZED),
            "case {label:?}: a created build must be reported INITIALIZED, got {build}"
        );
        created.push((label, build_id_of(&build), case.settled_status));
    }

    wait_for_due_transitions();
    for (label, build_id, settled_status) in &created {
        for attempt in 1..=2 {
            let described = gamelift
                .ok("DescribeBuild", json!({"BuildId": build_id}))
                .await;
            assert_eq!(
                described["Build"]["Status"],
                json!(settled_status),
                "case {label:?}, read {attempt}: the settled build must report {settled_status}, \
                 got {}",
                described["Build"]
            );
        }
    }
}

/// "Build names do not need to be unique": two builds created under one name are two
/// resources, each with its own id, its own storage location and its own upload credentials.
#[tokio::test]
async fn build_names_need_not_be_unique() {
    let gamelift = GameLift::new();
    let mut created: Vec<Value> = Vec::new();
    for operating_system in [OPERATING_SYSTEMS[2], OPERATING_SYSTEMS[5]] {
        let response = gamelift
            .ok(
                "CreateBuild",
                json!({"Name": "shared", "OperatingSystem": operating_system}),
            )
            .await;
        created.push(response);
    }

    let (first, second) = (created[0]["Build"].clone(), created[1]["Build"].clone());
    assert_ne!(
        build_id_of(&first),
        build_id_of(&second),
        "two builds sharing a name must be distinct resources, got {first} and {second}"
    );
    assert_ne!(
        created[0]["StorageLocation"], created[1]["StorageLocation"],
        "each build must be handed its own storage location, got {}",
        created[0]["StorageLocation"]
    );
    assert_ne!(
        created[0]["UploadCredentials"]["AccessKeyId"],
        created[1]["UploadCredentials"]["AccessKeyId"],
        "each build must be handed its own upload credentials"
    );
    for (operating_system, response) in [OPERATING_SYSTEMS[2], OPERATING_SYSTEMS[5]]
        .iter()
        .zip(created.iter())
    {
        let build = response["Build"].clone();
        assert_eq!(
            build["OperatingSystem"],
            json!(operating_system),
            "case {operating_system}: each build must report its own operating system, got {build}"
        );
        let described = gamelift
            .ok("DescribeBuild", json!({"BuildId": build_id_of(&build)}))
            .await;
        assert_eq!(
            described["Build"], build,
            "case {operating_system}: each build sharing the name must read back as itself, got {}",
            described["Build"]
        );
    }
}

/// A create that names no operating system, names an incomplete storage location, or carries
/// a value outside a modelled constraint is refused with the modelled client error — and
/// stores nothing.
#[tokio::test]
async fn a_create_outside_the_documented_contract_is_refused() {
    struct Case {
        label: &'static str,
        request: Value,
        code: &'static str,
    }

    let cases = [
        Case {
            label: "no operating system, which has no default value",
            request: json!({"Name": "no-os"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "no operating system, with build files in the caller's bucket",
            request: json!({
                "Name": "no-os-from-s3",
                "StorageLocation": caller_location("builds/server.zip"),
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a storage location naming no bucket",
            request: json!({
                "Name": "no-bucket",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "StorageLocation": {
                    "Key": "builds/server.zip",
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftBuildAccess",
                },
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a storage location naming no key",
            request: json!({
                "Name": "no-key",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "StorageLocation": {
                    "Bucket": CALLER_BUCKET,
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftBuildAccess",
                },
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a storage location naming no role",
            request: json!({
                "Name": "no-role",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "StorageLocation": {"Bucket": CALLER_BUCKET, "Key": "builds/server.zip"},
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "an operating system outside the modelled enum",
            request: json!({"Name": "plan-9", "OperatingSystem": "PLAN_9"}),
            code: "ValidationException",
        },
        Case {
            label: "a server SDK version outside the modelled pattern",
            request: json!({
                "Name": "sdk-latest",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "ServerSdkVersion": "latest",
            }),
            code: "ValidationException",
        },
        Case {
            label: "a name one past the modelled maximum length",
            request: json!({
                "Name": "n".repeat(MAX_NAME_LENGTH + 1),
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
            code: "ValidationException",
        },
        Case {
            label: "a version one past the modelled maximum length",
            request: json!({
                "Name": "long-version",
                "Version": "v".repeat(MAX_NAME_LENGTH + 1),
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
            code: "ValidationException",
        },
        Case {
            label: "a tag set that assigns one key twice",
            request: json!({
                "Name": "repeated-tag-key",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "Tags": [
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "owner", "Value": "platform"},
                ],
            }),
            code: "TaggingFailedException",
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        let label = case.label;
        let (status, code) = gamelift.err("CreateBuild", case.request.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, case.code),
            "case {label:?}: the request must be refused with {}, got {status} {code}",
            case.code
        );
    }

    // Every refused create stored nothing: the next accepted create mints the FIRST build of
    // the store, and it is the only one a describe can find.
    let created = gamelift
        .ok(
            "CreateBuild",
            json!({
                "Name": "the-first-stored-build",
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
        )
        .await;
    let described = gamelift
        .ok(
            "DescribeBuild",
            json!({"BuildId": build_id_of(&created["Build"])}),
        )
        .await;
    assert_eq!(
        described["Build"]["Name"],
        json!("the-first-stored-build"),
        "the one stored build must be the one the accepted create made, got {}",
        described["Build"]
    );
}

/// The account's build quota bounds the stored collection: once it is reached, the next
/// create answers the modelled `ConflictException` and stores nothing.
#[tokio::test]
async fn the_build_quota_is_the_bound_on_the_stored_collection() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_BUILDS_PER_ACCOUNT;
    let gamelift = GameLift::new();
    let mut last_id = String::new();
    for index in 0..quota {
        let created = gamelift
            .ok(
                "CreateBuild",
                json!({
                    "Name": format!("quota-{index}"),
                    "OperatingSystem": OPERATING_SYSTEMS[index % OPERATING_SYSTEMS.len()],
                }),
            )
            .await;
        last_id = build_id_of(&created["Build"]);
    }

    let (status, code) = gamelift
        .err(
            "CreateBuild",
            json!({
                "Name": "one-build-too-many",
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ConflictException"),
        "a create past the quota must be refused, got {status} {code}"
    );

    // The refused create stored nothing, and the build that filled the quota is intact.
    let described = gamelift
        .ok("DescribeBuild", json!({"BuildId": &last_id}))
        .await;
    assert_eq!(
        described["Build"]["Name"],
        json!(format!("quota-{}", quota - 1)),
        "the build that filled the quota must be unchanged, got {}",
        described["Build"]
    );
}

/// The location a build with no storage location of its own is handed REACHES something:
/// the build files upload to it, and read back byte for byte from the object store this
/// emulator holds — "use the credentials to manually upload your build files to the
/// specified Amazon S3 location".
#[tokio::test]
async fn the_build_upload_location_serves_the_files_uploaded_to_it() {
    let gamelift = GameLift::new();

    // Several builds, so the rule holds for each one's own key in the shared bucket rather
    // than for one object alone.
    let mut uploads: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (index, operating_system) in OPERATING_SYSTEMS.iter().enumerate() {
        let created = gamelift
            .ok(
                "CreateBuild",
                json!({"Name": format!("uploaded-{index}"), "OperatingSystem": operating_system}),
            )
            .await;
        let location = created["StorageLocation"].clone();
        let bucket = location["Bucket"]
            .as_str()
            .expect("the create must report a bucket to upload to")
            .to_string();
        let key = location["Key"]
            .as_str()
            .expect("the create must report a key to upload to")
            .to_string();

        // Deterministic bytes standing in for a zipped game server build, opening with the
        // local-file signature a zip archive carries (`PK\x03\x04`).
        let mut body: Vec<u8> = b"PK\x03\x04".to_vec();
        body.extend((0..=index as u8).cycle().take(64 + index));

        let (status, response) = gamelift
            .s3("PUT", &format!("/{bucket}/{key}"), &body)
            .await;
        assert_eq!(
            status,
            200,
            "case {operating_system}: the upload to {bucket}/{key} must be served, got {status} \
             with body {}",
            String::from_utf8_lossy(&response)
        );
        uploads.push((bucket, key, body));
    }

    for (bucket, key, body) in &uploads {
        let (status, served) = gamelift.s3("GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            (status, served.as_ref()),
            (200, body.as_slice()),
            "the files uploaded to {bucket}/{key} must read back as uploaded, got {status} with \
             {} bytes",
            served.len()
        );
    }
}

/// The service-owned upload location is NOT adopted from whatever holds its name: a bucket
/// of that name in the caller's own hands is not this service's to vend an upload into, so
/// the create is refused instead and stores no build.
#[tokio::test]
async fn an_upload_bucket_that_is_not_the_services_is_not_adopted() {
    let gamelift = GameLift::new();
    // The name the service-owned upload location carries for the Region every request in
    // this suite is served in (`state::service_owned_build_bucket`), created here through
    // S3's own API so it belongs to the caller rather than to this service.
    let taken = format!(
        "prod-gamescale-builds-{}",
        super::hosting_loop::REGION
    );
    create_bucket(&gamelift, &taken).await;

    let (status, code) = gamelift
        .err(
            "CreateBuild",
            json!({"Name": "no-location-to-vend", "OperatingSystem": OPERATING_SYSTEMS[4]}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (500, "InternalServiceException"),
        "a create that cannot be handed an upload location of this service's must be refused, \
         got {status} {code}"
    );

    // A create that names its own storage location needs no upload location at all, so it
    // is unaffected.
    let created = gamelift
        .ok(
            "CreateBuild",
            json!({
                "Name": "own-location",
                "OperatingSystem": OPERATING_SYSTEMS[4],
                "StorageLocation": caller_location("builds/server.zip"),
            }),
        )
        .await;
    assert_eq!(
        created["Build"]["Name"],
        json!("own-location"),
        "a create naming its own location must still be served, got {}",
        created["Build"]
    );
}

/// `RequestUploadCredentials` mints a FRESH set of credentials for a stored build and
/// reports them with "the S3 storage location associated with the build ID" — the same
/// location the build was created with — and changes nothing else about the build.
#[tokio::test]
async fn fresh_upload_credentials_are_issued_for_a_stored_build() {
    struct Case {
        label: &'static str,
        source: Source,
    }

    let cases = [
        Case {
            label: "a build uploading to the service-owned location",
            source: Source::ServiceOwned,
        },
        Case {
            label: "a build whose files are in the caller's own bucket",
            source: Source::CallerBucket(caller_location("builds/server.zip")),
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        let label = case.label;
        let mut request = json!({"Name": "credential-walk", "OperatingSystem": OPERATING_SYSTEMS[4]});
        if let Source::CallerBucket(location) = &case.source {
            request["StorageLocation"] = location.clone();
        }
        let created = gamelift.ok("CreateBuild", request).await;
        let build = created["Build"].clone();
        let build_id = build_id_of(&build);

        // Each call hands back a set of its own, for either selector the member accepts.
        let mut issued: Vec<String> = Vec::new();
        for selector in [build_id.clone(), build_arn(&build_id)] {
            let fresh = gamelift
                .ok("RequestUploadCredentials", json!({"BuildId": &selector}))
                .await;
            assert_eq!(
                fresh["StorageLocation"], created["StorageLocation"],
                "case {label:?}, selector {selector:?}: the credentials must come with the \
                 build's own storage location, got {}",
                fresh["StorageLocation"]
            );
            for member in ["AccessKeyId", "SecretAccessKey", "SessionToken"] {
                let value = fresh["UploadCredentials"][member]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    !value.is_empty(),
                    "case {label:?}, selector {selector:?}: the fresh credentials must carry a \
                     {member}, got {}",
                    fresh["UploadCredentials"]
                );
            }
            let access_key_id = fresh["UploadCredentials"]["AccessKeyId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !issued.contains(&access_key_id),
                "case {label:?}, selector {selector:?}: each call must issue a fresh set, got the \
                 repeated access key {access_key_id:?}"
            );
            assert_ne!(
                json!(&access_key_id),
                created["UploadCredentials"]["AccessKeyId"],
                "case {label:?}, selector {selector:?}: a fresh set must not repeat the one the \
                 create handed back"
            );
            issued.push(access_key_id);
        }

        // The build itself is unchanged: the credentials are the only thing this call makes.
        let described = gamelift
            .ok("DescribeBuild", json!({"BuildId": &build_id}))
            .await;
        assert_eq!(
            described["Build"], build,
            "case {label:?}: issuing credentials must not change the build, got {}",
            described["Build"]
        );
    }
}

/// `RequestUploadCredentials` names ONE build, so an identifier that names no stored build
/// is the modelled `NotFoundException`.
#[tokio::test]
async fn requesting_upload_credentials_for_a_build_that_is_not_stored_is_refused() {
    let gamelift = GameLift::new();
    for (label, selector) in [
        ("an id no build carries", json!("build-does-not-exist")),
        (
            "an ARN no build carries",
            json!(build_arn("build-does-not-exist")),
        ),
    ] {
        let (status, code) = gamelift
            .err("RequestUploadCredentials", json!({"BuildId": selector}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an identifier naming no stored build must be refused, got {status} \
             {code}"
        );
    }
}

/// `DescribeBuild` names ONE resource, so an identifier that names no stored build is the
/// modelled `NotFoundException` rather than an empty success.
#[tokio::test]
async fn describing_a_build_that_is_not_stored_is_refused() {
    let gamelift = GameLift::new();
    let created = gamelift
        .ok(
            "CreateBuild",
            json!({
                "Name": "the-only-build",
                "OperatingSystem": OPERATING_SYSTEMS[4],
            }),
        )
        .await;
    let build = created["Build"].clone();
    let stored_id = build_id_of(&build);

    let cases = [
        ("an id no build carries", json!("build-does-not-exist")),
        (
            "an ARN no build carries",
            json!(build_arn("build-does-not-exist")),
        ),
        (
            "an ARN in another account for a stored id",
            json!(format!(
                "arn:aws:gamelift:us-west-2:999999999999:build/{stored_id}"
            )),
        ),
    ];
    for (label, selector) in &cases {
        let (status, code) = gamelift
            .err("DescribeBuild", json!({"BuildId": selector}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an identifier naming no stored build must be refused, got {status} \
             {code}"
        );
    }

    // The stored build is untouched by those refusals.
    let described = gamelift
        .ok("DescribeBuild", json!({"BuildId": &stored_id}))
        .await;
    assert_eq!(
        described["Build"], build,
        "the stored build must still read back as created, got {}",
        described["Build"]
    );
}
