// SPDX-License-Identifier: CC0-1.0

//! The Realtime script contract, over the wire: `CreateScript` persists a script record
//! for the zip file the request provides, mints its id and ARN, records the size and the
//! storage location the SOURCE of that zip file decides, and `DescribeScript` serves that
//! one stored record back — selected by script id or by script ARN.
//!
//! The harness (`GameLift`, `gamelift_arn`) is the one `hosting_loop` already drives the
//! production dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::{gamelift_arn, GameLift};

/// Every `NodeJsVersion` a Realtime script may declare, from the member's own doc — "The
/// valid values are `10.x | 24.x`" (`smithy_query shape=Script` member `NodeJsVersion`) —
/// which is also what the handler enforces (`state::NODE_JS_VERSIONS`).
const NODE_JS_VERSIONS: [&str; 2] = ["10.x", "24.x"];

/// The `NodeJsVersion` a script created without one carries: same doc — "By default,
/// `NodeJsVersion` is `10.x`."
const DEFAULT_NODE_JS_VERSION: &str = "10.x";

/// The modelled bounds of `CreateScriptInput$Name` and `$Version`
/// (`smithy_query operation=CreateScript`: `NonZeroAndMaxString`, len 1..1024).
const MAX_NAME_LENGTH: usize = 1024;

/// The modelled bounds of `CreateScriptInput$ZipFile` (`ZipBlob`, len 0..5000000) — "the
/// maximum size of a zip file is 5 MB".
const MAX_ZIP_BYTES: usize = 5_000_000;

/// The S3 bucket the rows below that upload from the CALLER's own bucket name.
const CALLER_BUCKET: &str = "my-own-game-scripts";

/// `len` deterministic bytes standing in for a zipped Realtime script, opening with the
/// local-file signature a zip archive carries (`PK\x03\x04`) when there is room for it.
fn zip_bytes(len: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = b"PK\x03\x04".to_vec();
    bytes.truncate(len);
    while bytes.len() < len {
        bytes.push((bytes.len() % 251) as u8);
    }
    bytes
}

/// A blob travels base64-encoded on awsJson1_1, which is what the wire serializer does
/// with it (`aws_smithy_types::base64`, the codec the runtime itself uses).
fn zip_file_member(bytes: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(bytes))
}

/// A caller-owned S3 storage location naming the three members the operation documents as
/// required: "the storage location must specify the Amazon S3 bucket name, the zip file
/// name (the "key"), and a role ARN that allows Amazon GameLift Servers to access the
/// Amazon S3 storage location".
fn caller_location(key: &str) -> Value {
    json!({
        "Bucket": CALLER_BUCKET,
        "Key": key,
        "RoleArn": "arn:aws:iam::123456789012:role/GameLiftScriptAccess",
    })
}

/// Where the zip file a create request provides comes from: the two alternatives
/// CreateScript documents.
enum Source {
    /// A local zip file of this many bytes, sent in `ZipFile`.
    LocalZip(usize),
    /// An S3 object the caller owns, named by `StorageLocation`.
    CallerBucket(Value),
}

/// The `ScriptId` of a created script.
fn script_id_of(script: &Value) -> String {
    script["ScriptId"]
        .as_str()
        .expect("CreateScript must report a ScriptId")
        .to_string()
}

/// The ARN a script with `script_id` is assigned:
/// `arn:aws:gamelift:<region>:<account>:script/<script id>` — the model's
/// `^arn:.*:script\/script-\S+$` with "the resource ID matches the `ScriptId` value"
/// (`smithy_query shape=Script` member `ScriptArn`).
fn script_arn(script_id: &str) -> String {
    gamelift_arn(&format!("script/{script_id}"))
}

/// `CreateScript` stores the script the request configures and reports the stored record;
/// `DescribeScript` serves the same record back, member for member, selected by id or by
/// ARN.
///
/// The rows differ along what the rule generalises over: which of the two documented
/// sources provides the zip file, the size of that zip file (including each modelled
/// bound), which optional members the request sets, each documented `NodeJsVersion` and
/// the default, and how many tags the request assigns.
#[tokio::test]
async fn a_created_script_is_reported_as_stored_by_create_and_by_describe() {
    struct Case {
        label: &'static str,
        /// The request, without its source members.
        request: Value,
        source: Source,
        /// The members the created script must report beyond its id, ARN, creation time
        /// and storage location.
        expected: Value,
    }

    let cases = [
        Case {
            label: "a local zip upload with a name and a version",
            request: json!({"Name": "realtime-core", "Version": "1.0.0"}),
            source: Source::LocalZip(512),
            expected: json!({
                "Name": "realtime-core",
                "Version": "1.0.0",
                "SizeOnDisk": 512,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a one-byte local zip upload with only a name",
            request: json!({"Name": "tiny"}),
            source: Source::LocalZip(1),
            expected: json!({
                "Name": "tiny",
                "SizeOnDisk": 1,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a zip upload at the modelled minimum size, carrying no name at all",
            request: json!({}),
            source: Source::LocalZip(0),
            expected: json!({
                "SizeOnDisk": 0,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a zip upload at the modelled maximum size",
            request: json!({"Name": "five-megabytes", "Version": "max"}),
            source: Source::LocalZip(MAX_ZIP_BYTES),
            expected: json!({
                "Name": "five-megabytes",
                "Version": "max",
                "SizeOnDisk": MAX_ZIP_BYTES,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a zip upload that picks the older documented Node.js version",
            request: json!({"Name": "node-ten", "NodeJsVersion": NODE_JS_VERSIONS[0]}),
            source: Source::LocalZip(2048),
            expected: json!({
                "Name": "node-ten",
                "SizeOnDisk": 2048,
                "NodeJsVersion": NODE_JS_VERSIONS[0],
            }),
        },
        Case {
            label: "a zip upload that picks the newer documented Node.js version",
            request: json!({"Name": "node-twentyfour", "NodeJsVersion": NODE_JS_VERSIONS[1]}),
            source: Source::LocalZip(4096),
            expected: json!({
                "Name": "node-twentyfour",
                "SizeOnDisk": 4096,
                "NodeJsVersion": NODE_JS_VERSIONS[1],
            }),
        },
        Case {
            label: "a name and a version at the modelled maximum length",
            request: json!({
                "Name": "n".repeat(MAX_NAME_LENGTH),
                "Version": "v".repeat(MAX_NAME_LENGTH),
            }),
            source: Source::LocalZip(64),
            expected: json!({
                "Name": "n".repeat(MAX_NAME_LENGTH),
                "Version": "v".repeat(MAX_NAME_LENGTH),
                "SizeOnDisk": 64,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a zip upload with one tag",
            request: json!({"Name": "tagged-once", "Tags": [{"Key": "owner", "Value": "liveops"}]}),
            source: Source::LocalZip(128),
            // `Script` carries no `Tags` member — tags are read back through
            // `ListTagsForResource` — so a tagged script reports what an untagged one does.
            expected: json!({
                "Name": "tagged-once",
                "SizeOnDisk": 128,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a zip upload with three tags",
            request: json!({
                "Name": "tagged-thrice",
                "Tags": [
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "env", "Value": "prod"},
                    {"Key": "cost-centre", "Value": "games"},
                ],
            }),
            source: Source::LocalZip(256),
            expected: json!({
                "Name": "tagged-thrice",
                "SizeOnDisk": 256,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a script pulled from the caller's own S3 bucket",
            request: json!({"Name": "from-s3", "Version": "2.1"}),
            source: Source::CallerBucket(caller_location("scripts/realtime.zip")),
            expected: json!({
                "Name": "from-s3",
                "Version": "2.1",
                // "When files are uploaded from an S3 location, this value remains at 0."
                "SizeOnDisk": 0,
                "NodeJsVersion": DEFAULT_NODE_JS_VERSION,
            }),
        },
        Case {
            label: "a script pulled from a versioned S3 object, on the newer Node.js version",
            request: json!({"Name": "from-s3-versioned", "NodeJsVersion": NODE_JS_VERSIONS[1]}),
            source: Source::CallerBucket(json!({
                "Bucket": CALLER_BUCKET,
                "Key": "scripts/pinned.zip",
                "RoleArn": "arn:aws:iam::123456789012:role/GameLiftScriptAccess",
                "ObjectVersion": "3HL4kqtJlcpXroDTDmjVBH40Nrjfkd",
            })),
            expected: json!({
                "Name": "from-s3-versioned",
                "SizeOnDisk": 0,
                "NodeJsVersion": NODE_JS_VERSIONS[1],
            }),
        },
    ];

    let gamelift = GameLift::new();
    let mut minted_ids: Vec<String> = Vec::new();
    for case in &cases {
        let label = case.label;
        let mut request = case.request.clone();
        match &case.source {
            Source::LocalZip(len) => {
                request["ZipFile"] = zip_file_member(&zip_bytes(*len));
            }
            Source::CallerBucket(location) => {
                request["StorageLocation"] = location.clone();
            }
        }

        let created = gamelift.ok("CreateScript", request).await;
        let script = created["Script"].clone();
        let script_id = script_id_of(&script);
        assert!(
            script_id.starts_with("script-") && script_id.len() > "script-".len(),
            "case {label:?}: the minted ScriptId must match the modelled ^script-\\S+$, got {script_id:?}"
        );
        assert!(
            !minted_ids.contains(&script_id),
            "case {label:?}: a script ID is unique, got the repeated {script_id:?}"
        );
        minted_ids.push(script_id.clone());

        // The storage location the SOURCE decides: the request's own location for an S3
        // upload, a service-owned one for a local zip file.
        let location = script["StorageLocation"].clone();
        match &case.source {
            Source::CallerBucket(requested) => assert_eq!(
                &location, requested,
                "case {label:?}: a script pulled from the caller's bucket must report the \
                 location the request provided, got {location}"
            ),
            Source::LocalZip(_) => {
                let bucket = location["Bucket"].as_str().unwrap_or_default();
                let key = location["Key"].as_str().unwrap_or_default();
                assert!(
                    !bucket.is_empty() && bucket != CALLER_BUCKET,
                    "case {label:?}: an uploaded zip file must be reported in a service-owned \
                     bucket, got {location}"
                );
                assert!(
                    key.contains(&script_id),
                    "case {label:?}: the service-owned storage location must name the script it \
                     holds, got {location}"
                );
            }
        }

        let mut expected = case.expected.clone();
        expected["ScriptId"] = json!(&script_id);
        expected["ScriptArn"] = json!(script_arn(&script_id));
        expected["StorageLocation"] = location;
        expected["CreationTime"] = script["CreationTime"].clone();
        assert!(
            script["CreationTime"].is_number(),
            "case {label:?}: a created script must carry a CreationTime, got {script}"
        );
        assert_eq!(
            script, expected,
            "case {label:?}: CreateScript must report the stored script, got {script}"
        );

        // The describe path serves the SAME stored record, member for member — selected by
        // the id and by the ARN the create minted, which name the same script.
        for selector in [script_id.clone(), script_arn(&script_id)] {
            let described = gamelift
                .ok("DescribeScript", json!({"ScriptId": &selector}))
                .await;
            assert_eq!(
                described["Script"], expected,
                "case {label:?}: a describe naming {selector:?} must report the created record, got {}",
                described["Script"]
            );
        }
    }

    // Reading a script changed nothing: the same describe, run again, reports exactly what
    // it reported before.
    let first = gamelift
        .ok("DescribeScript", json!({"ScriptId": &minted_ids[0]}))
        .await;
    let second = gamelift
        .ok("DescribeScript", json!({"ScriptId": &minted_ids[0]}))
        .await;
    assert_eq!(
        first, second,
        "describing a script must not change what a later describe reports, got {second}"
    );
}

/// "Script names do not need to be unique": two scripts created under one name are two
/// resources, each with its own id, its own size and its own storage location.
#[tokio::test]
async fn script_names_need_not_be_unique() {
    let gamelift = GameLift::new();
    let mut created: Vec<Value> = Vec::new();
    for len in [16_usize, 32] {
        let script = gamelift
            .ok(
                "CreateScript",
                json!({"Name": "shared", "ZipFile": zip_file_member(&zip_bytes(len))}),
            )
            .await;
        created.push(script["Script"].clone());
    }

    assert_ne!(
        script_id_of(&created[0]),
        script_id_of(&created[1]),
        "two scripts sharing a name must be distinct resources, got {} and {}",
        created[0],
        created[1]
    );
    for (len, script) in [16, 32].iter().zip(created.iter()) {
        assert_eq!(
            script["SizeOnDisk"],
            json!(len),
            "case zip length {len}: each script must report its own size, got {script}"
        );
        let described = gamelift
            .ok("DescribeScript", json!({"ScriptId": script_id_of(script)}))
            .await;
        assert_eq!(
            described["Script"], *script,
            "case zip length {len}: each script sharing the name must read back as itself, got {}",
            described["Script"]
        );
    }
}

/// A create that does not name exactly one documented source for the zip file, names an
/// incomplete storage location, or asks for a Node.js version the service does not run, is
/// refused with the modelled client error — and stores nothing.
#[tokio::test]
async fn a_create_outside_the_documented_contract_is_refused() {
    struct Case {
        label: &'static str,
        request: Value,
        code: &'static str,
    }

    let zip = zip_file_member(&zip_bytes(64));
    let cases = vec![
        Case {
            label: "both documented sources at once",
            request: json!({
                "Name": "two-sources",
                "ZipFile": zip.clone(),
                "StorageLocation": caller_location("scripts/realtime.zip"),
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "neither documented source",
            request: json!({"Name": "no-source"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a storage location naming no bucket",
            request: json!({
                "Name": "no-bucket",
                "StorageLocation": {
                    "Key": "scripts/realtime.zip",
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftScriptAccess",
                },
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a storage location naming no key",
            request: json!({
                "Name": "no-key",
                "StorageLocation": {
                    "Bucket": CALLER_BUCKET,
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftScriptAccess",
                },
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a storage location naming no role",
            request: json!({
                "Name": "no-role",
                "StorageLocation": {"Bucket": CALLER_BUCKET, "Key": "scripts/realtime.zip"},
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a Node.js version the service does not run, in the modelled form",
            request: json!({"Name": "node-twelve", "ZipFile": zip.clone(), "NodeJsVersion": "12.x"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a Node.js version one minor past a documented one",
            request: json!({"Name": "node-ten-one", "ZipFile": zip.clone(), "NodeJsVersion": "10.1"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a Node.js version outside the modelled pattern",
            request: json!({"Name": "node-latest", "ZipFile": zip.clone(), "NodeJsVersion": "latest"}),
            code: "ValidationException",
        },
        Case {
            label: "a name one past the modelled maximum length",
            request: json!({"Name": "n".repeat(MAX_NAME_LENGTH + 1), "ZipFile": zip.clone()}),
            code: "ValidationException",
        },
        Case {
            label: "a zip file one byte past the modelled maximum size",
            request: json!({
                "Name": "too-large",
                "ZipFile": zip_file_member(&zip_bytes(MAX_ZIP_BYTES + 1)),
            }),
            code: "ValidationException",
        },
        Case {
            label: "a tag set that assigns one key twice",
            request: json!({
                "Name": "repeated-tag-key",
                "ZipFile": zip.clone(),
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
        let (status, code) = gamelift.err("CreateScript", case.request.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, case.code),
            "case {label:?}: the request must be refused with {}, got {status} {code}",
            case.code
        );
    }

    // Every refused create stored nothing: the next accepted create mints the FIRST script
    // of the store, and it is the only one a describe can find.
    let created = gamelift
        .ok(
            "CreateScript",
            json!({"Name": "the-first-stored-script", "ZipFile": zip}),
        )
        .await;
    let described = gamelift
        .ok(
            "DescribeScript",
            json!({"ScriptId": script_id_of(&created["Script"])}),
        )
        .await;
    assert_eq!(
        described["Script"]["Name"],
        json!("the-first-stored-script"),
        "the one stored script must be the one the accepted create made, got {}",
        described["Script"]
    );
}

/// The account's script quota bounds the stored collection: once it is reached, the next
/// create answers the modelled `ConflictException` and stores nothing.
#[tokio::test]
async fn the_script_quota_is_the_bound_on_the_stored_collection() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_SCRIPTS_PER_ACCOUNT;
    let gamelift = GameLift::new();
    let mut last_id = String::new();
    for index in 0..quota {
        let created = gamelift
            .ok(
                "CreateScript",
                json!({
                    "Name": format!("quota-{index}"),
                    "ZipFile": zip_file_member(&zip_bytes(8)),
                }),
            )
            .await;
        last_id = script_id_of(&created["Script"]);
    }

    let (status, code) = gamelift
        .err(
            "CreateScript",
            json!({
                "Name": "one-script-too-many",
                "ZipFile": zip_file_member(&zip_bytes(8)),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ConflictException"),
        "a create past the quota must be refused, got {status} {code}"
    );

    // The refused create stored nothing, and the script that filled the quota is intact.
    let described = gamelift
        .ok("DescribeScript", json!({"ScriptId": &last_id}))
        .await;
    assert_eq!(
        described["Script"]["Name"],
        json!(format!("quota-{}", quota - 1)),
        "the script that filled the quota must be unchanged, got {}",
        described["Script"]
    );
}

/// `DescribeScript` names ONE resource, so an identifier that names no stored script is
/// the modelled `NotFoundException` rather than an empty success.
#[tokio::test]
async fn describing_a_script_that_is_not_stored_is_refused() {
    let gamelift = GameLift::new();
    let created = gamelift
        .ok(
            "CreateScript",
            json!({
                "Name": "the-only-script",
                "ZipFile": zip_file_member(&zip_bytes(96)),
            }),
        )
        .await;
    let script = created["Script"].clone();
    let stored_id = script_id_of(&script);

    let cases = vec![
        ("an id no script carries", json!("script-does-not-exist")),
        (
            "an ARN no script carries",
            json!(script_arn("script-does-not-exist")),
        ),
        (
            "an ARN in another account for a stored id",
            json!(format!(
                "arn:aws:gamelift:us-west-2:999999999999:script/{stored_id}"
            )),
        ),
    ];
    for (label, selector) in cases {
        let (status, code) = gamelift
            .err("DescribeScript", json!({"ScriptId": selector}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an identifier naming no stored script must be refused, got {status} {code}"
        );
    }

    // The stored script is untouched by those refusals.
    let described = gamelift
        .ok("DescribeScript", json!({"ScriptId": &stored_id}))
        .await;
    assert_eq!(
        described["Script"], script,
        "the stored script must still read back as created, got {}",
        described["Script"]
    );
}
