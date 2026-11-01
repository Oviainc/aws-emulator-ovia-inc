// SPDX-License-Identifier: CC0-1.0

//! Where Amazon GameLift Servers itself holds what a caller uploads: the service-owned S3
//! location `CreateBuild` and `RequestUploadCredentials` hand a build's files, and the one a
//! Realtime script's zip file is stored at.
//!
//! The rule under test is one rule for both resource families: a service-owned upload
//! location names the service's own bucket for the RESOURCE FAMILY in the Region the request
//! was served in, and its key is the account that owns the resource and the resource's own
//! id — nothing the caller supplies, and no file name appended. The rows below differ along
//! what that rule generalises over: the family (builds, scripts), the Region the call is
//! served in, and the operation that reports the location.
//!
//! The harness (`GameLift`, `call_in`/`ok_in`) is the one `hosting_loop` already drives the
//! production dispatch path with; the zip-file helpers are `scripts`'.

use serde_json::{json, Value};

use super::hosting_loop::{GameLift, REGION};
use super::scripts::{zip_bytes, zip_file_member};

/// The Regions the rows below are served in. The emulator serves a request in the Region of
/// its SigV4 credential scope, so these are what the rule's Region slot is driven with, and
/// they differ in the parts an AWS Region code varies in: its geography prefix, how many
/// segments it has, its length and its trailing digit. [`REGION`] is the one the rest of
/// this target uses; the others are not cited by any other suite.
const REGIONS: [&str; 4] = [REGION, "us-east-1", "eu-central-1", "ap-southeast-4"];

/// The one bucket family a build's service-owned upload location names, and the one a
/// script's does: the Amazon GameLift Servers-managed build and script buckets, named for
/// the Region (`state::BUILD_UPLOAD_FAMILY`, `state::SCRIPT_UPLOAD_FAMILY`).
fn service_owned_bucket(family: &str, region: &str) -> String {
    format!("prod-gamescale-{family}-{region}")
}

/// The service-owned location one resource's files are held at: the family's bucket for the
/// Region, keyed by the account that owns the resource and the resource that holds the
/// files. The location carries no `RoleArn` (that member lets the service reach ANOTHER
/// account's bucket) and no `ObjectVersion` (the caller's own bucket versioning), so the
/// whole member is compared rather than two of its parts.
fn service_owned_location(family: &str, region: &str, resource_id: &str) -> Value {
    json!({
        "Bucket": service_owned_bucket(family, region),
        "Key": format!("{}/{resource_id}", engine::emulator_account_id()),
    })
}

/// The `BuildId` of a created build.
fn build_id_of(created: &Value) -> String {
    created["Build"]["BuildId"]
        .as_str()
        .expect("CreateBuild must report a BuildId")
        .to_string()
}

/// A build created without a storage location of its own, in `region`.
async fn build_in(gamelift: &GameLift, region: &str, name: &str) -> Value {
    gamelift
        .ok_in(
            region,
            "CreateBuild",
            json!({"Name": name, "OperatingSystem": "AMAZON_LINUX_2023"}),
        )
        .await
}

/// A build created without a storage location of its own is handed the service's build
/// bucket for the Region the create was served in, keyed by the caller's account and the
/// build's own id — and `RequestUploadCredentials` re-issues credentials for that SAME
/// location, whichever Region either call is served in.
#[tokio::test]
async fn a_build_upload_location_names_the_regions_build_bucket_and_the_accounts_build() {
    let gamelift = GameLift::new();
    for region in REGIONS {
        let created = build_in(&gamelift, region, "upload-location").await;
        let build_id = build_id_of(&created);
        let expected = service_owned_location("builds", region, &build_id);
        assert_eq!(
            created["StorageLocation"], expected,
            "case {region}: the create must hand back the Region's build bucket keyed by the \
             account and the build, got {}",
            created["StorageLocation"]
        );

        // "To refresh these credentials, call RequestUploadCredentials", which "returns the
        // S3 storage location associated with the build ID": the same location, re-derived
        // from the stored build rather than from this call's own Region.
        let refreshed = gamelift
            .ok_in(
                region,
                "RequestUploadCredentials",
                json!({"BuildId": &build_id}),
            )
            .await;
        assert_eq!(
            refreshed["StorageLocation"], expected,
            "case {region}: refreshed credentials must come with the build's own location, got {}",
            refreshed["StorageLocation"]
        );
        // The upload the location and credentials are for is the caller's next step, so both
        // calls have to hand back credentials to sign it with.
        for (call, response) in [("CreateBuild", &created), ("RequestUploadCredentials", &refreshed)]
        {
            for member in ["AccessKeyId", "SecretAccessKey", "SessionToken"] {
                let value = response["UploadCredentials"][member]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    !value.is_empty(),
                    "case {region}: {call} must hand back a {member} to sign the upload with, got \
                     {}",
                    response["UploadCredentials"]
                );
            }
        }
    }
}

/// The location is one a client can actually upload the build zip to: the bucket it names is
/// in the one object store this emulator holds, so a `PutObject` of the build files at the
/// reported key is served and reads back as uploaded — in every Region a create was served
/// in.
#[tokio::test]
async fn the_reported_build_upload_location_is_one_the_client_can_upload_to() {
    let gamelift = GameLift::new();
    let mut uploaded: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (index, region) in REGIONS.iter().enumerate() {
        let created = build_in(&gamelift, region, "uploadable").await;
        let location = created["StorageLocation"].clone();
        let bucket = location["Bucket"].as_str().unwrap_or_default().to_string();
        let key = location["Key"].as_str().unwrap_or_default().to_string();
        let body = zip_bytes(64 + index);

        let (status, response) = gamelift
            .s3("PUT", &format!("/{bucket}/{key}"), &body)
            .await;
        assert_eq!(
            status,
            200,
            "case {region}: the upload to {bucket}/{key} must be served, got {status} with body {}",
            String::from_utf8_lossy(&response)
        );
        uploaded.push((bucket, key, body));
    }

    for (bucket, key, body) in &uploaded {
        let (status, served) = gamelift.s3("GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            (status, served.as_ref()),
            (200, body.as_slice()),
            "case {bucket}/{key}: the uploaded build files must read back as uploaded, got {status} \
             with {} bytes",
            served.len()
        );
    }
}

/// Nothing the caller supplies names either part of the location: two builds created in one
/// Region under one name share the Region's bucket and differ only in the build id their key
/// carries, and the same build created in another Region is held in that Region's bucket.
#[tokio::test]
async fn service_owned_locations_differ_only_in_the_region_and_the_resource() {
    let gamelift = GameLift::new();
    let mut keys: Vec<String> = Vec::new();
    for region in [REGIONS[0], REGIONS[0], REGIONS[2]] {
        let created = build_in(&gamelift, region, "shared-name").await;
        let build_id = build_id_of(&created);
        let location = created["StorageLocation"].clone();
        assert_eq!(
            location["Bucket"],
            json!(service_owned_bucket("builds", region)),
            "case {region}: every build of a Region is held in that Region's one build bucket, \
             got {location}"
        );
        let key = location["Key"].as_str().unwrap_or_default().to_string();
        assert!(
            !keys.contains(&key),
            "case {region}: two builds must never share a location, got the repeated key {key:?}"
        );
        assert_eq!(
            key,
            format!("{}/{build_id}", engine::emulator_account_id()),
            "case {region}: the key is the account and the build, with no file name appended, got \
             {key:?}"
        );
        keys.push(key);
    }
}

/// A script uploaded as a local zip file is held in the service's SCRIPT bucket for the
/// Region, keyed the same way as a build — and `DescribeScript` reports the location that
/// was stored, so a later read cannot derive a different one.
#[tokio::test]
async fn a_script_upload_location_names_the_regions_script_bucket_and_is_read_back() {
    let gamelift = GameLift::new();
    for (index, region) in REGIONS.iter().enumerate() {
        let created = gamelift
            .ok_in(
                region,
                "CreateScript",
                json!({
                    "Name": "upload-location",
                    "ZipFile": zip_file_member(&zip_bytes(32 + index)),
                }),
            )
            .await;
        let script = created["Script"].clone();
        let script_id = script["ScriptId"]
            .as_str()
            .expect("CreateScript must report a ScriptId")
            .to_string();
        let expected = service_owned_location("scripts", region, &script_id);
        assert_eq!(
            script["StorageLocation"], expected,
            "case {region}: an uploaded zip file must be reported in the Region's script bucket \
             keyed by the account and the script, got {}",
            script["StorageLocation"]
        );

        // The two families are told apart by the bucket, not by the key: the same account and
        // the same Region, a different family.
        assert_ne!(
            script["StorageLocation"]["Bucket"],
            json!(service_owned_bucket("builds", region)),
            "case {region}: a script is not held in the build bucket, got {}",
            script["StorageLocation"]
        );

        // Either selector the member accepts names the same script ("in a GameLift script
        // ARN, the resource ID matches the ScriptId value").
        let script_arn = format!(
            "arn:aws:gamelift:{region}:{}:script/{script_id}",
            engine::emulator_account_id()
        );
        for selector in [script_id.clone(), script_arn] {
            let described = gamelift
                .ok_in(region, "DescribeScript", json!({"ScriptId": &selector}))
                .await;
            assert_eq!(
                described["Script"]["StorageLocation"], expected,
                "case {region}: a describe of {selector:?} must report the stored location, got {}",
                described["Script"]["StorageLocation"]
            );
        }
    }
}
