// SPDX-License-Identifier: CC0-1.0

//! `CreateStorageLocation` CREATES the account's regional Elastic Beanstalk storage
//! bucket in the emulator's object storage, so the name it reports is a bucket that is
//! immediately usable.
//!
//! The rule: an operation that reports a resource in another service's data plane
//! ("Creates a bucket in Amazon S3 to store application versions, logs, and other
//! files used by Elastic Beanstalk environments") stands that resource up THERE before
//! reporting it — so the very next request against the reported bucket succeeds, rather
//! than answering `NoSuchBucket`. The one bucket per account per region is created if it
//! is not there and reported if it is ("if the storage location already exists,
//! `CreateStorageLocation` still returns the bucket name but does not create a new
//! bucket"), which makes the call idempotent.
//!
//! Every suite here mounts the S3 service beside Elastic Beanstalk on ONE engine
//! ([`Emulator::with_object_storage`]) — what the assembled emulator does — and drives
//! object storage over its own front door, so what is asserted is what a client's own
//! `PutObject` / `GetObject` would see. The rows span what the rule generalises over:
//! which region the call is signed for (including `us-east-1`, the one region an Amazon
//! S3 location constraint cannot name), the shape of the key uploaded into the bucket,
//! and how many times the call is repeated.

use crate::harness::Emulator;
use crate::lifecycle::account;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// One region a storage location is created in: the region the call is signed for, the
/// `LocationConstraint` object storage must then report for the bucket, and a key whose
/// SHAPE differs from the other rows'.
///
/// The constraint column is Amazon S3's own rule, not a per-row choice: "Buckets in
/// Region us-east-1 have a LocationConstraint of null"
/// (`GetBucketLocationOutput$LocationConstraint`), and every other region code IS the
/// constraint — which is why `us-east-1` is not one of `BucketLocationConstraint`'s
/// values. Every other region below is one of them
/// (`smithy_query(query="shape",shape="BucketLocationConstraint")` on the S3 model).
struct Row {
    case: &'static str,
    region: &'static str,
    location_constraint: &'static str,
    key: &'static str,
}

/// The regions the storage location is driven in. Rows differ along the region the call
/// is signed for — the value the bucket's name and placement are derived from — and the
/// shape of the key a client then uploads.
const ROWS: &[Row] = &[
    Row {
        case: "the region the suite signs with",
        region: "us-west-2",
        location_constraint: "us-west-2",
        key: "app.zip",
    },
    Row {
        case: "the region no location constraint can name",
        region: "us-east-1",
        location_constraint: "",
        key: "bundles/app-1.2.3.zip",
    },
    Row {
        case: "a European region, with a deeply nested key",
        region: "eu-central-1",
        location_constraint: "eu-central-1",
        key: "builds/2026/03/app-1.2.3.war",
    },
    Row {
        case: "an Asia Pacific region the issue never cited",
        region: "ap-northeast-1",
        location_constraint: "ap-northeast-1",
        key: "app.jar",
    },
];

/// `CreateStorageLocation` in `region`, failing the named case unless it answers 200
/// with the account's regional bucket name.
async fn create_storage_location(emulator: &Emulator, region: &str, case: &str) -> String {
    let answer = emulator
        .call_in_region(region, "CreateStorageLocation", &[])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from CreateStorageLocation, got {} and body {}",
        answer.status, answer.body
    );
    // The documented form of the account's storage location, written out here rather
    // than taken from the service's own helper so the name is asserted against the
    // published shape: `elasticbeanstalk-<region>-<account-id>`.
    let expected = format!("elasticbeanstalk-{region}-{}", account());
    let reported = answer.value("S3Bucket").unwrap_or_default().to_string();
    assert_eq!(
        reported, expected,
        "case {case:?}: unexpected S3Bucket in {}",
        answer.body
    );
    reported
}

/// The bucket a reported storage location names EXISTS in object storage: a client's
/// `PutObject` into it succeeds, the object reads back, the bucket is placed in the
/// call's own region, and it is one of the account's own buckets.
#[tokio::test]
async fn a_reported_storage_location_is_a_bucket_object_storage_holds() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();

        let bucket = create_storage_location(&emulator, row.region, case).await;

        // The documented first step of every Elastic Beanstalk deployment: upload the
        // source bundle to the storage location. It goes over object storage's own
        // front door, signed for the same region the storage location was created in.
        let body = b"PK\x03\x04 a source bundle";
        let stored = emulator
            .s3_in_region(row.region, "PUT", &format!("/{bucket}/{}", row.key), body)
            .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: expected 200 from PutObject into s3://{bucket}/{}, got {} and \
             body {}",
            row.key, stored.status, stored.body
        );

        // The round trip: the bytes object storage hands back are the ones written.
        let read = emulator
            .s3_in_region(row.region, "GET", &format!("/{bucket}/{}", row.key), b"")
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetObject of s3://{bucket}/{}, got {} and body {}",
            row.key, read.status, read.body
        );
        assert_eq!(
            read.body,
            String::from_utf8_lossy(body),
            "case {case:?}: GetObject must hand back the uploaded bytes"
        );

        // One storage location per account per REGION: the bucket object storage holds
        // is placed in the region the call was signed for.
        let location = emulator
            .s3_in_region(row.region, "GET", &format!("/{bucket}?location"), b"")
            .await;
        assert_eq!(
            location.status, 200,
            "case {case:?}: expected 200 from GetBucketLocation of {bucket}, got {} and body {}",
            location.status, location.body
        );
        assert_eq!(
            location.value("LocationConstraint").unwrap_or_default(),
            row.location_constraint,
            "case {case:?}: unexpected location constraint for {bucket} in {}",
            location.body
        );

        // It is the ACCOUNT's bucket, the way the real storage location is: Amazon S3
        // lists it among the buckets the account owns.
        let listed = emulator.s3_in_region(row.region, "GET", "/", b"").await;
        assert!(
            listed.values_at("Buckets/Bucket/Name").contains(&&*bucket),
            "case {case:?}: {bucket} must be one of the account's buckets, got {}",
            listed.body
        );
    }
}

/// Repeating the call creates NOTHING new: it reports the same bucket, and the object a
/// client already uploaded into it is still there — so the second call did not replace
/// the storage location with an empty one.
#[tokio::test]
async fn repeating_the_call_reports_the_same_bucket_and_keeps_its_objects() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();

        let first = create_storage_location(&emulator, row.region, case).await;
        let stored = emulator
            .s3_in_region(row.region, "PUT", &format!("/{first}/{}", row.key), b"bundle")
            .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: expected 200 from PutObject into s3://{first}/{}, got {} and body {}",
            row.key, stored.status, stored.body
        );

        // Three more calls: the storage location is idempotent however many times it is
        // asked for.
        for attempt in 2..=4 {
            let again = create_storage_location(&emulator, row.region, case).await;
            assert_eq!(
                again, first,
                "case {case:?} attempt {attempt}: the storage location must be the same bucket"
            );
            let read = emulator
                .s3_in_region(row.region, "GET", &format!("/{first}/{}", row.key), b"")
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?} attempt {attempt}: the object uploaded before the repeat must \
                 still be readable, got {} and body {}",
                read.status, read.body
            );
        }

        // And only ONE bucket was ever created for the region, however many calls asked
        // for it.
        let listed = emulator.s3_in_region(row.region, "GET", "/", b"").await;
        let holding = listed
            .values_at("Buckets/Bucket/Name")
            .into_iter()
            .filter(|name| *name == first)
            .count();
        assert_eq!(
            holding, 1,
            "case {case:?}: the account must hold exactly one {first}, got {}",
            listed.body
        );
    }
}

/// The storage location is usable as a `CreateApplicationVersion` source bundle, which
/// is what makes it a storage location at all: the create DOWNLOADS the archive it
/// names, so a bundle uploaded to the reported bucket is accepted and read back off the
/// stored version.
#[tokio::test]
async fn a_bundle_uploaded_to_the_storage_location_creates_an_application_version() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();

        let bucket = create_storage_location(&emulator, row.region, case).await;
        let stored = emulator
            .s3_in_region(
                row.region,
                "PUT",
                &format!("/{bucket}/{}", row.key),
                b"PK\x03\x04",
            )
            .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: expected 200 from PutObject into s3://{bucket}/{}, got {} and \
             body {}",
            row.key, stored.status, stored.body
        );

        let created_application = emulator
            .call_in_region(
                row.region,
                "CreateApplication",
                &[("ApplicationName", "storage-location-app")],
            )
            .await;
        assert_eq!(
            created_application.status, 200,
            "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
            created_application.status, created_application.body
        );

        let created = emulator
            .call_in_region(
                row.region,
                "CreateApplicationVersion",
                &[
                    ("ApplicationName", "storage-location-app"),
                    ("VersionLabel", "v1"),
                    ("SourceBundle.S3Bucket", &bucket),
                    ("SourceBundle.S3Key", row.key),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a bundle in the storage location must be accepted, got {} and \
             body {}",
            created.status, created.body
        );

        let described = emulator
            .call_in_region(
                row.region,
                "DescribeApplicationVersions",
                &[("ApplicationName", "storage-location-app")],
            )
            .await;
        assert_eq!(
            described.value("ApplicationVersions/member/SourceBundle/S3Bucket"),
            Some(bucket.as_str()),
            "case {case:?}: the stored version must report the storage location, got {}",
            described.body
        );
    }
}

/// An emulator assembled WITHOUT object storage still reports the storage location: the
/// S3 request such an emulator answers is the router's own `UnknownService`, which says
/// nothing about the bucket, so the call is not refused for it.
///
/// The counterpart of `source_bundle_download::an_emulator_without_object_storage_refuses_no_archive`,
/// and the reason the service's own record of the storage location stays the one
/// `MAX_STORAGE_LOCATIONS` bounds.
#[tokio::test]
async fn an_emulator_without_object_storage_still_reports_the_storage_location() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        let bucket = create_storage_location(&emulator, row.region, case).await;
        assert_eq!(
            bucket,
            eb::state::storage_bucket_name(row.region),
            "case {case:?}: the reported bucket must be the service's own derived name"
        );
    }
}
