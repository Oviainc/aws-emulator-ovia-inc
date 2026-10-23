// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 archive a create builds its resource FROM is DOWNLOADED while the
//! create is served, and a create whose archive object storage cannot hand back is
//! refused with HTTP 400 before anything is recorded.
//!
//! Two operations take such an archive — `CreateApplicationVersion$SourceBundle` ("You
//! can create an application version from a source bundle in Amazon S3") and
//! `CreatePlatformVersion$PlatformDefinitionBundle` ("The location of the platform
//! definition archive in Amazon S3") — and each refuses with the query error code its
//! recorded refusal carries (`InvalidParameterCombination` and `InvalidParameterValue`
//! respectively) and a message built from the request:
//! `Unable to download from S3 location (Bucket: <bucket>  Key: <key>). Reason: <reason>`.
//!
//! Every suite here mounts the S3 service beside Elastic Beanstalk on ONE engine
//! ([`Emulator::with_object_storage`]) — what the assembled emulator does — and stores
//! the archives it expects a create to find through the S3 front door, so the object a
//! create reads is one object storage really holds. The rows span the class the rule
//! generalises over: which create, whether the bucket exists, whether the key exists,
//! the shape of the key (flat and nested) and an archive location that names only one
//! of the two.

use crate::harness::{Answer, Emulator};
use crate::lifecycle::TEST_REGION;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// How much of the archive object storage holds before the create runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stored {
    /// The bucket exists and holds the key: the archive downloads.
    Archive,
    /// The bucket exists but does not hold the key.
    BucketOnly,
    /// Object storage holds neither.
    Nothing,
}

/// One archive a create names: where the request says it is, and how much of it
/// object storage holds.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    bucket: &'static str,
    key: &'static str,
    stored: Stored,
}

impl Row {
    /// Whether the create that names this archive is expected to succeed.
    fn downloads(&self) -> bool {
        self.stored == Stored::Archive && !self.bucket.is_empty() && !self.key.is_empty()
    }

    /// The reason the refusal carries: an archive object storage does not hold, or a
    /// location that names no object at all, is `Not Found` — the HTTP reason phrase
    /// of the 404 the download answered with.
    fn message(&self) -> String {
        format!(
            "Unable to download from S3 location (Bucket: {}  Key: {}). Reason: Not Found",
            self.bucket, self.key
        )
    }

    /// Store as much of this archive as the row says object storage holds.
    async fn seed(&self, emulator: &Emulator) {
        match self.stored {
            Stored::Archive => emulator.put_object(self.bucket, self.key, b"PK\x03\x04").await,
            // An empty key names no object, so the bucket is seeded by storing a
            // different object in it.
            Stored::BucketOnly => {
                emulator
                    .put_object(self.bucket, "another-archive.zip", b"PK\x03\x04")
                    .await;
            }
            Stored::Nothing => {}
        }
    }

    /// The archive's location as awsQuery parameters under `member`, leaving out a
    /// half the row states as absent (a location that names only a bucket, or only a
    /// key, names no object).
    fn params(&self, member: &str) -> Vec<(String, String)> {
        let mut params = Vec::new();
        if !self.bucket.is_empty() {
            params.push((format!("{member}.S3Bucket"), self.bucket.to_string()));
        }
        if !self.key.is_empty() {
            params.push((format!("{member}.S3Key"), self.key.to_string()));
        }
        params
    }
}

/// The archives the two creates are driven over. One row per thing the rule
/// generalises over: an archive that is there, the same with a nested key, a key its
/// bucket does not hold, a bucket that is not there at all, a bucket whose name is
/// legal but unknown, and a location that names only one of bucket and key.
const ROWS: &[Row] = &[
    Row {
        case: "an archive object storage holds",
        bucket: "eb-bundles",
        key: "app.zip",
        stored: Stored::Archive,
    },
    Row {
        case: "an archive under a nested key",
        bucket: "eb-bundles",
        key: "builds/2026/app-1.2.3.war",
        stored: Stored::Archive,
    },
    Row {
        case: "a key the bucket does not hold",
        bucket: "eb-bundles",
        key: "absent.zip",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a bucket object storage does not hold",
        bucket: "eb-missing-bucket",
        key: "app.zip",
        stored: Stored::Nothing,
    },
    Row {
        case: "an unknown bucket and a nested key",
        bucket: "eb-other-missing-bucket",
        key: "deep/prefix/app.zip",
        stored: Stored::Nothing,
    },
    Row {
        case: "a location that names only a bucket",
        bucket: "eb-bundles",
        key: "",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a location that names only a key",
        bucket: "",
        key: "app.zip",
        stored: Stored::Nothing,
    },
];

/// Create the application a version is created under.
async fn create_application(emulator: &Emulator, name: &str) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", name)])
        .await;
    assert_eq!(
        created.status, 200,
        "seeding application {name:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
}

/// `params` as the borrowed pairs the harness sends.
fn as_params(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The application versions `DescribeApplicationVersions` reports for `application`.
async fn describe_versions(emulator: &Emulator, application: &str) -> Answer {
    emulator
        .call(
            "DescribeApplicationVersions",
            &[("ApplicationName", application)],
        )
        .await
}

/// `CreateApplicationVersion` downloads the source bundle the request names: an
/// archive object storage holds is stored with the version, and one it cannot hand
/// back is refused 400 `InvalidParameterCombination` with no version recorded.
#[tokio::test]
async fn create_application_version_downloads_its_source_bundle() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        row.seed(&emulator).await;
        create_application(&emulator, "bundle-app").await;

        let mut params = vec![
            ("ApplicationName".to_string(), "bundle-app".to_string()),
            ("VersionLabel".to_string(), "v1".to_string()),
        ];
        params.extend(row.params("SourceBundle"));
        let created = emulator
            .call("CreateApplicationVersion", &as_params(&params))
            .await;

        if row.downloads() {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
                created.status, created.body
            );
            // The round trip: the bundle the create downloaded is the one the stored
            // version reports.
            let described = describe_versions(&emulator, "bundle-app").await;
            assert_eq!(
                described.value("ApplicationVersions/member/SourceBundle/S3Bucket"),
                Some(row.bucket),
                "case {case:?}: the stored SourceBundle must report the request's bucket, got {}",
                described.body
            );
            assert_eq!(
                described.value("ApplicationVersions/member/SourceBundle/S3Key"),
                Some(row.key),
                "case {case:?}: the stored SourceBundle must report the request's key, got {}",
                described.body
            );
            continue;
        }

        assert_eq!(
            created.status, 400,
            "case {case:?}: expected 400 from CreateApplicationVersion, got {} and body {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "InvalidParameterCombination",
            "case {case:?}: unexpected error code, got body {}",
            created.body
        );
        assert_eq!(
            created.error_message(),
            row.message(),
            "case {case:?}: the refusal names the request's bucket and key, got body {}",
            created.body
        );
        // Refused BEFORE anything is recorded: a client that reads the application
        // back sees no version it could believe is deployable.
        let described = describe_versions(&emulator, "bundle-app").await;
        assert!(
            described
                .values_at("ApplicationVersions/member/VersionLabel")
                .is_empty(),
            "case {case:?}: no application version may have been recorded, got body {}",
            described.body
        );
    }
}

/// `CreatePlatformVersion` downloads the platform definition bundle the request
/// names: an archive object storage holds yields a platform version that can be read
/// back, and one it cannot hand back is refused 400 `InvalidParameterValue` with no
/// platform version recorded.
#[tokio::test]
async fn create_platform_version_downloads_its_definition_bundle() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        row.seed(&emulator).await;

        let mut params = vec![
            ("PlatformName".to_string(), "bundle-platform".to_string()),
            ("PlatformVersion".to_string(), "1.0.0".to_string()),
        ];
        params.extend(row.params("PlatformDefinitionBundle"));
        let created = emulator
            .call("CreatePlatformVersion", &as_params(&params))
            .await;

        let arn = eb::state::platform_version_arn(TEST_REGION, "bundle-platform", "1.0.0");
        let described = emulator
            .call("DescribePlatformVersion", &[("PlatformArn", &arn)])
            .await;

        if row.downloads() {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
                created.status, created.body
            );
            // The round trip: the version the create recorded is the one
            // DescribePlatformVersion reports.
            assert_eq!(
                described.value("PlatformDescription/PlatformArn"),
                Some(arn.as_str()),
                "case {case:?}: the created platform version must be readable back, got body {}",
                described.body
            );
            continue;
        }

        assert_eq!(
            created.status, 400,
            "case {case:?}: expected 400 from CreatePlatformVersion, got {} and body {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "InvalidParameterValue",
            "case {case:?}: unexpected error code, got body {}",
            created.body
        );
        assert_eq!(
            created.error_message(),
            row.message(),
            "case {case:?}: the refusal names the request's bucket and key, got body {}",
            created.body
        );
        assert_eq!(
            described.status, 400,
            "case {case:?}: no platform version may have been recorded, got {} and body {}",
            described.status, described.body
        );
    }
}

/// A `CreateApplicationVersion` that names NO source bundle downloads nothing: "If
/// neither SourceBundle nor SourceBuildInformation are provided, Elastic Beanstalk
/// uses a sample application", so the create is accepted with object storage mounted
/// and answering, and the version reports the sample application's location.
#[tokio::test]
async fn a_version_with_no_source_bundle_downloads_nothing() {
    let case = "no SourceBundle at all";
    let emulator = Emulator::with_object_storage();
    create_application(&emulator, "sample-app").await;

    let created = emulator
        .call(
            "CreateApplicationVersion",
            &[("ApplicationName", "sample-app"), ("VersionLabel", "v1")],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
        created.status, created.body
    );
    let sample = eb::state::sample_application_source_bundle(TEST_REGION);
    assert_eq!(
        created.value("ApplicationVersion/SourceBundle/S3Bucket"),
        sample.s3_bucket.as_deref(),
        "case {case:?}: the sample application's bucket must be reported, got body {}",
        created.body
    );
    assert_eq!(
        created.value("ApplicationVersion/SourceBundle/S3Key"),
        sample.s3_key.as_deref(),
        "case {case:?}: the sample application's key must be reported, got body {}",
        created.body
    );
}

/// An archive that cannot be downloaded is refused before ANY resource is recorded —
/// including the application `AutoCreateApplication` would have created.
#[tokio::test]
async fn a_refused_source_bundle_creates_no_application() {
    let case = "AutoCreateApplication with an archive that is not there";
    let emulator = Emulator::with_object_storage();

    let created = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "auto-created-app"),
                ("VersionLabel", "v1"),
                ("AutoCreateApplication", "true"),
                ("SourceBundle.S3Bucket", "eb-missing-bucket"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
        )
        .await;
    assert_eq!(
        created.error_code(),
        "InvalidParameterCombination",
        "case {case:?}: unexpected error code, got body {}",
        created.body
    );

    let applications = emulator.call("DescribeApplications", &[]).await;
    assert!(
        applications
            .values_at("Applications/member/ApplicationName")
            .is_empty(),
        "case {case:?}: no application may have been created, got body {}",
        applications.body
    );
}

/// An emulator assembled WITHOUT object storage cannot say the archive is absent, so
/// it does not refuse the create for it.
///
/// The S3 request such an emulator answers is the router's own `UnknownService` — a
/// statement about the emulator, not about the caller's object — and reporting it as
/// `Not Found` would tell a client its archive is missing when nothing looked for it.
/// The create is accepted and the location the request named is stored as it stands.
#[tokio::test]
async fn an_emulator_without_object_storage_refuses_no_archive() {
    let case = "no object storage mounted";
    let emulator = Emulator::new();
    create_application(&emulator, "no-storage-app").await;

    let created = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "no-storage-app"),
                ("VersionLabel", "v1"),
                ("SourceBundle.S3Bucket", "eb-bundles"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
        created.status, created.body
    );
    assert_eq!(
        created.value("ApplicationVersion/SourceBundle/S3Key"),
        Some("app.zip"),
        "case {case:?}: the location the request named must be stored as it stands, got body {}",
        created.body
    );
}
