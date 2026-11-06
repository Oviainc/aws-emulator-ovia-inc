// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the engine-availability gate of `CreateCustomDBEngineVersion`.
//!
//! The rule under test: a custom engine version (CEV) is built ON a DB engine version of
//! its engine — that is where its major version, its DB parameter group family and its
//! capabilities come from — so a CEV can be created only for an engine this service
//! publishes at least one engine version of. An engine it publishes none of is refused with
//! the service's own fixed `Invalid DB engine` sentence, and refused BEFORE any other
//! member of the request: the same answer whatever image, manifest or KMS key the request
//! also carries.
//!
//! The table spans EVERY engine `CreateCustomDBEngineVersion$Engine` publishes, and nothing
//! in it is written out by hand: each row asks `DescribeDBEngineVersions` whether this
//! service publishes a version of that engine and reads the major version to name the CEV
//! after, so the engine-version catalog itself decides which rows are served and which are
//! refused.
//!
//! Every call goes over the wire: real awsQuery form POSTs through a `MultiServiceRouter`
//! carrying the SigV4 credential scope the router keys on, and every assertion reads the
//! XML the emulator answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::CUSTOM_ENGINE_VERSION_ENGINES;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, subtrees, text};

/// A KMS key identifier of the form `KMSKeyId` takes: the 36-character key id KMS issues.
const KMS_KEY_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The sentence Amazon RDS refuses an engine no CEV can be created for with: fixed text,
/// with nothing from the request in it.
///
/// Spelled out here so the wording is pinned by the test rather than by the code under
/// test.
const INVALID_DB_ENGINE: &str = "Invalid DB engine";

/// The `MajorEngineVersion` of the first engine version this service publishes for
/// `engine`, or `None` when it publishes none.
async fn published_major_version(router: &MultiServiceRouter, engine: &str) -> Option<String> {
    let body = expect_ok(
        engine,
        call(
            router,
            "DescribeDBEngineVersions",
            &[
                ("Engine", engine),
                ("IncludeAll", "true"),
                ("MaxRecords", "100"),
            ],
        )
        .await,
    );
    subtrees(&body, "DBEngineVersion")
        .first()
        .map(|entry| text(entry, "MajorEngineVersion"))
}

/// Whether a CEV of `engine` is created from an Amazon Machine Image: RDS Custom for SQL
/// Server, whose `ImageId` documentation states that "for RDS Custom for SQL Server, an AMI
/// ID is required to create a CEV".
fn needs_an_image(engine: &str) -> bool {
    engine.starts_with("custom-sqlserver")
}

#[tokio::test]
async fn a_cev_is_served_exactly_for_an_engine_this_service_publishes_a_build_of() {
    let router = emulator();

    for engine in CUSTOM_ENGINE_VERSION_ENGINES {
        let published = published_major_version(&router, engine).await;
        // The CEV name begins with the major engine version of the build it packages; for
        // an engine with no build, the name still carries a major version, so the refusal
        // below cannot be about the NAME's form.
        let major = published.clone().unwrap_or_else(|| "19".to_string());
        let engine_version = format!("{major}.9999.1.availability_probe");
        let case = format!("{engine} {engine_version}");

        let mut params: Vec<(&str, String)> = vec![
            ("Engine", engine.to_string()),
            ("EngineVersion", engine_version.clone()),
            ("KMSKeyId", KMS_KEY_ID.to_string()),
        ];
        if needs_an_image(engine) {
            params.push(("ImageId", "ami-0abcdef1234567890".to_string()));
        }
        let response = call(&router, "CreateCustomDBEngineVersion", &params).await;

        let Some(major) = published else {
            // No build to be created on: the engine itself is what the request is refused
            // for, and nothing is stored.
            let refused = expect_error(&case, response, 400, "InvalidParameterValue");
            assert_eq!(
                text(&refused, "Message"),
                INVALID_DB_ENGINE,
                "case {case:?}: got {refused}"
            );
            let described = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeDBEngineVersions",
                    &[
                        ("Engine", engine),
                        ("EngineVersion", engine_version.as_str()),
                        ("IncludeAll", "true"),
                    ],
                )
                .await,
            );
            assert!(
                subtrees(&described, "DBEngineVersion").is_empty(),
                "case {case:?}: a refused create must store nothing, got {described}"
            );
            // The gate fires before every other member: the identical engine with a KMS
            // key of no KMS form, no image and a manifest gets the same sentence.
            let refused = expect_error(
                &case,
                call(
                    &router,
                    "CreateCustomDBEngineVersion",
                    &[
                        ("Engine", engine),
                        ("EngineVersion", engine_version.as_str()),
                        ("KMSKeyId", "not-a-kms-key"),
                        ("Manifest", "{\"MediaImportTemplateVersion\":\"2020-08-14\"}"),
                        ("ImageId", "ami-0abcdef1234567890"),
                        ("UseAwsProvidedLatestImage", "true"),
                    ],
                )
                .await,
                400,
                "InvalidParameterValue",
            );
            assert_eq!(
                text(&refused, "Message"),
                INVALID_DB_ENGINE,
                "case {case:?}: the engine gate answers before every other member, got \
                 {refused}"
            );
            continue;
        };

        // A build exists, so the create is served and the CEV is built on that build: its
        // major version is the published one the name carries.
        let created = expect_ok(&case, response);
        assert_eq!(
            (
                text(&created, "Engine"),
                text(&created, "EngineVersion"),
                text(&created, "MajorEngineVersion"),
                text(&created, "Status"),
            ),
            (
                (*engine).to_string(),
                engine_version.clone(),
                major.clone(),
                "creating".to_string(),
            ),
            "case {case:?}: got {created}"
        );
        // And the report that lists engine versions reports it, which is what proves the
        // CEV was persisted rather than echoed.
        let described = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBEngineVersions",
                &[
                    ("Engine", engine),
                    ("EngineVersion", engine_version.as_str()),
                    ("IncludeAll", "true"),
                ],
            )
            .await,
        );
        let entries = subtrees(&described, "DBEngineVersion");
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the created CEV must be reported, got {described}"
        );
    }
}
