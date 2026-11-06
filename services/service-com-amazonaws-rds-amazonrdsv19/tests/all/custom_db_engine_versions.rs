// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateCustomDBEngineVersion`, the operation that mints a custom
//! DB engine version (CEV) — the one engine version that is ACCOUNT state rather than this
//! service's published reference data.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (`crate::db_instances::emulator`), each request is a
//! real awsQuery form POST carrying the SigV4 credential scope the router keys on, and
//! every assertion reads the XML the emulator answered with — so a handler that persisted
//! nothing could not pass them.
//!
//! The rules are stated over TABLES spanning what they generalise over rather than over
//! one request: the engines a CEV can be created for are every family
//! `CreateCustomDBEngineVersion$Engine` publishes (RDS Custom for Oracle, RDS Custom for
//! SQL Server and the RDS for SQL Server bring-your-own-media engines), each with the CEV
//! name form that engine documents and a different combination of the optional media
//! members; the engines that are refused are a row each; and every created CEV is read
//! back through `DescribeDBEngineVersions`, which is the report a CEV joins.

use service_com_amazonaws_rds_amazonrdsv19::state::is_published_engine;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
    wait_for_settle, REGION,
};

/// The operation under test.
const ACTION: &str = "CreateCustomDBEngineVersion";

/// The report a created CEV joins.
const DESCRIBE: &str = "DescribeDBEngineVersions";

/// A KMS key identifier of the form `CreateCustomDBEngineVersion$KMSKeyId` takes — "a
/// symmetric encryption KMS key is required for RDS Custom" — written as the 36-character
/// key id KMS issues.
///
/// `pub(crate)` so the `ModifyCustomDBEngineVersion` suite creates its CEVs with the same
/// key rather than inventing a second one.
pub(crate) const KMS_KEY_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A second key, so a CEV created from a source CEV can carry its own.
const OTHER_KMS_KEY_ID: &str = "66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A CEV manifest: "a JSON document that describes the installation .zip files stored in
/// Amazon S3", whose `MediaImportTemplateVersion` "date is in the format YYYY-MM-DD".
/// Written without spaces because the test harness posts form values unescaped.
///
/// `pub(crate)` for the same reason as [`KMS_KEY_ID`].
pub(crate) const MANIFEST: &str = "{\"MediaImportTemplateVersion\":\"2020-08-14\"}";

/// `Status` of a CEV once this service has finished creating it: the value
/// `ModifyCustomDBEngineVersion$Status` publishes first ("available — You can use this CEV
/// to create a new RDS Custom DB instance").
pub(crate) const STATUS_AVAILABLE: &str = "available";

/// `Status` of a CEV a create has just accepted: the media it names is imported after the
/// create returns, so the CEV is still being created and reaches [`STATUS_AVAILABLE`] on
/// its own.
pub(crate) const STATUS_CREATING: &str = "creating";

/// One row of the create table: a CEV of one engine family, with the members that family's
/// documentation says it is created from.
struct Case {
    /// `Engine`.
    engine: &'static str,
    /// `EngineVersion` — the CEV name, in the form that engine documents.
    engine_version: &'static str,
    /// The `MajorEngineVersion` the CEV name belongs to.
    major_engine_version: &'static str,
    /// The `DBParameterGroupFamily` that major version publishes.
    parameter_group_family: &'static str,
    /// The request members beyond `Engine` and `EngineVersion`.
    extra: &'static [(&'static str, &'static str)],
    /// The `DBEngineMediaType` this engine reports, if any: the member is "applicable to
    /// RDS Custom for SQL Server and to RDS for SQL Server engine versions", so an Oracle
    /// CEV reports none.
    media_type: Option<&'static str>,
}

/// The CEVs this suite creates: one per engine FAMILY the operation serves, each with a
/// different combination of the optional media members, so no assertion below depends on
/// one engine or on one set of members being present.
///
/// The engines and the CEV name forms are the member's own published ones
/// (`smithy_query(query="doc",shape="CreateCustomDBEngineVersionMessage",member="Engine"` /
/// `member="EngineVersion"`): *major*.*minor*.*customized_string* for the SQL Server
/// engines. The major versions and parameter group families are the ones this service
/// publishes for those engines through `DescribeDBEngineVersions` — a CEV is built ON a
/// published engine version, so every row's engine is one this service publishes versions
/// of, which is what the engine-availability row of [`REFUSALS`] is the other side of.
const CASES: &[Case] = &[
    Case {
        engine: "sqlserver-se",
        engine_version: "15.00.4435.7.byom_cev2",
        major_engine_version: "15.00",
        parameter_group_family: "sqlserver-se-15.0",
        // The largest create: every media member at once, plus a description and tags.
        extra: &[
            ("KMSKeyId", KMS_KEY_ID),
            ("Manifest", MANIFEST),
            ("DatabaseInstallationFilesS3BucketName", "my-cev-media"),
            ("DatabaseInstallationFilesS3Prefix", "000000000000/cev1"),
            ("DatabaseInstallationFiles.member.1", "SQLServer2019.iso"),
            ("DatabaseInstallationFiles.member.2", "SQLServer2019-CU.exe"),
            ("Description", "standard-edition-for-my-fleet"),
            ("Tags.Tag.1.Key", "owner"),
            ("Tags.Tag.1.Value", "platform"),
        ],
        media_type: Some("Customer Provided"),
    },
    Case {
        engine: "sqlserver-dev-ee",
        engine_version: "15.00.4385.2.dev_cev2",
        major_engine_version: "15.00",
        parameter_group_family: "sqlserver-dev-ee-15.0",
        // The smallest create this engine serves: the KMS key, and nothing else — no image
        // is owed, because the image requirement is RDS Custom for SQL Server's.
        extra: &[("KMSKeyId", KMS_KEY_ID)],
        media_type: Some("Customer Provided"),
    },
    Case {
        engine: "custom-sqlserver-ee",
        engine_version: "15.00.4249.2.my_cev1",
        major_engine_version: "15.00",
        parameter_group_family: "custom-sqlserver-ee-15.0",
        // "For RDS Custom for SQL Server, an AMI ID is required to create a CEV."
        extra: &[("KMSKeyId", KMS_KEY_ID), ("ImageId", "ami-0abcdef1234567890")],
        media_type: Some("Customer Provided"),
    },
    Case {
        engine: "custom-sqlserver-dev",
        engine_version: "16.00.4165.4.dev_cev1",
        major_engine_version: "16.00",
        parameter_group_family: "custom-sqlserver-dev-16.0",
        // The other way to name the image: "specifies whether to use the latest
        // service-provided Amazon Machine Image (AMI) for the CEV".
        extra: &[
            ("KMSKeyId", KMS_KEY_ID),
            ("UseAwsProvidedLatestImage", "true"),
        ],
        media_type: Some("Customer Provided"),
    },
    Case {
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.byom_cev1",
        major_engine_version: "16.00",
        parameter_group_family: "sqlserver-ee-16.0",
        // Bring Your Own Media: installation files and no KMS key, which is "optional for
        // Amazon RDS".
        extra: &[
            ("DatabaseInstallationFilesS3BucketName", "my-cev-media"),
            ("DatabaseInstallationFiles.member.1", "SQLServer2022.iso"),
        ],
        media_type: Some("Customer Provided"),
    },
];

impl Case {
    /// This row's name in an assertion message.
    fn case(&self) -> String {
        format!("{}/{}", self.engine, self.engine_version)
    }

    /// The parameters of this row's create call, plus `extra`.
    fn params(&self, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut params = vec![
            ("Engine".to_string(), self.engine.to_string()),
            ("EngineVersion".to_string(), self.engine_version.to_string()),
        ];
        for (name, value) in self.extra.iter().chain(extra.iter()) {
            params.push(((*name).to_string(), (*value).to_string()));
        }
        params
    }

    /// What this row asked to be stored under `name`, if it named it.
    fn asked_for(&self, name: &str) -> Option<&'static str> {
        self.extra
            .iter()
            .find(|(member, _)| *member == name)
            .map(|(_, value)| *value)
    }
}

/// `xml` with every `<tag>` subtree removed.
///
/// `<Image>` carries its own `<Status>`, so reading "the one `<Status>` of this engine
/// version" is only well defined once the image is out of the way.
pub(crate) fn strip(xml: &str, tag: &str) -> String {
    let mut stripped = xml.to_string();
    for subtree in subtrees(xml, tag) {
        stripped = stripped.replace(&subtree, "");
    }
    stripped
}

/// The `<DatabaseInstallationFiles>` entries of one reported engine version.
///
/// Read out of that subtree rather than off the whole entry, because every other list the
/// entry carries names its items `<member>` too.
fn installation_files(xml: &str) -> Vec<String> {
    subtrees(xml, "DatabaseInstallationFiles")
        .iter()
        .flat_map(|subtree| texts(subtree, "member"))
        .collect()
}

/// The `<DBEngineVersion>` entries `DescribeDBEngineVersions` reports for one
/// `(Engine, EngineVersion)`, with their `<Image>` stripped.
pub(crate) async fn described(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    engine: &str,
    engine_version: &str,
    include_all: bool,
) -> Vec<String> {
    let mut params = vec![
        ("Engine".to_string(), engine.to_string()),
        ("EngineVersion".to_string(), engine_version.to_string()),
    ];
    if include_all {
        params.push(("IncludeAll".to_string(), "true".to_string()));
    }
    let body = expect_ok(case, call_in(router, region, DESCRIBE, &params).await);
    subtrees(&body, "DBEngineVersion")
        .into_iter()
        .map(|entry| strip(&entry, "Image"))
        .collect()
}

#[tokio::test]
async fn every_engine_family_persists_the_custom_version_it_was_created_with() {
    let router = emulator();

    for row in CASES {
        let case = row.case();
        let created = expect_ok(&case, call(&router, ACTION, &row.params(&[])).await);
        let scalars = strip(&created, "Image");

        // The identity of the record is what the request named.
        assert_eq!(
            text(&scalars, "Engine"),
            row.engine,
            "case {case:?}: Engine must be the engine the request named, got {scalars}"
        );
        assert_eq!(
            text(&scalars, "EngineVersion"),
            row.engine_version,
            "case {case:?}: EngineVersion must be the CEV name the request named, got {scalars}"
        );
        // The members the CEV name decides: its major version and the parameter group
        // family that major version publishes.
        assert_eq!(
            text(&scalars, "MajorEngineVersion"),
            row.major_engine_version,
            "case {case:?}: MajorEngineVersion must be the major version of the CEV name, \
             got {scalars}"
        );
        assert_eq!(
            text(&scalars, "DBParameterGroupFamily"),
            row.parameter_group_family,
            "case {case:?}: DBParameterGroupFamily must be the family of that major \
             version, got {scalars}"
        );
        // A created CEV is not usable yet: the installation media is imported after the
        // create is accepted, so the create answers with the CEV still being created.
        assert_eq!(
            text(&scalars, "Status"),
            STATUS_CREATING,
            "case {case:?}: a created CEV must report the status it is still being created \
             in, got {scalars}"
        );
        // Its ARN names the engine and the CEV name, so a caller can hand it back as a
        // source; `CreateTime` and the engine description are reported for every entry.
        let arn = text(&scalars, "DBEngineVersionArn");
        let expected_prefix = format!("cev:{}/{}/", row.engine, row.engine_version);
        assert!(
            arn.starts_with("arn:aws:rds:") && arn.contains(&expected_prefix),
            "case {case:?}: DBEngineVersionArn must be an RDS CEV ARN carrying \
             {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            !text(&scalars, "CreateTime").is_empty(),
            "case {case:?}: CreateTime must be reported, got {scalars}"
        );
        assert!(
            !text(&scalars, "DBEngineDescription").is_empty(),
            "case {case:?}: DBEngineDescription must describe the engine, got {scalars}"
        );
        // The media members are reported exactly as the request supplied them, and a
        // member the request left out is not invented.
        for member in [
            "KMSKeyId",
            "DatabaseInstallationFilesS3BucketName",
            "DatabaseInstallationFilesS3Prefix",
        ] {
            assert_eq!(
                optional_text(&scalars, member).as_deref(),
                row.asked_for(member),
                "case {case:?}: {member} must report what the request named, got {scalars}"
            );
        }
        assert_eq!(
            optional_text(&scalars, "CustomDBEngineVersionManifest").as_deref(),
            row.asked_for("Manifest"),
            "case {case:?}: the stored manifest must be the one the request supplied, got \
             {scalars}"
        );
        let expected_files: Vec<String> = ["DatabaseInstallationFiles.member.1", "DatabaseInstallationFiles.member.2"]
            .iter()
            .filter_map(|member| row.asked_for(member))
            .map(str::to_string)
            .collect();
        let reported_files = installation_files(&scalars);
        assert_eq!(
            reported_files, expected_files,
            "case {case:?}: DatabaseInstallationFiles must report the files the request \
             listed, got {scalars}"
        );
        // `Description` is "an optional description of your CEV", which is what the
        // version is described by; a CEV created without one is still described.
        let description = text(&scalars, "DBEngineVersionDescription");
        match row.asked_for("Description") {
            Some(asked) => assert_eq!(
                description, asked,
                "case {case:?}: DBEngineVersionDescription must be the description the \
                 request gave, got {description:?}"
            ),
            None => assert!(
                !description.is_empty(),
                "case {case:?}: DBEngineVersionDescription must not be empty, got {scalars}"
            ),
        }
        // `Image` is reported for a CEV built on an AMI the request named, and for no
        // other: nothing invents an image id.
        let image = subtrees(&created, "Image");
        match row.asked_for("ImageId") {
            Some(image_id) => {
                assert_eq!(
                    image.len(),
                    1,
                    "case {case:?}: a CEV created with an ImageId must report one Image, \
                     got {created}"
                );
                assert_eq!(
                    text(&image[0], "ImageId"),
                    image_id,
                    "case {case:?}: Image must carry the AMI the request named, got {:?}",
                    image[0]
                );
            }
            None => assert!(
                image.is_empty(),
                "case {case:?}: a CEV the request named no ImageId for must report no \
                 Image, got {:?}",
                image
            ),
        }
        assert_eq!(
            optional_text(&scalars, "DBEngineMediaType").as_deref(),
            row.media_type,
            "case {case:?}: DBEngineMediaType is applicable to the SQL Server engines \
             only, got {scalars}"
        );
        // Tags a create carried are the version's own tag list.
        if let Some(key) = row.asked_for("Tags.Tag.1.Key") {
            assert_eq!(
                text(&scalars, "Key"),
                key,
                "case {case:?}: TagList must carry the tag the create named, got {scalars}"
            );
        }
    }

    // Read every created CEV back: the record is what the report reads, so the core
    // attributes have to come back unchanged — and a CEV joins the report that lists the
    // engine versions a caller can use, without being asked for past them. Each one has
    // reached `available` on its own by now, which is the status it is usable in.
    wait_for_settle();
    for row in CASES {
        let case = row.case();
        let entries = described(
            &router,
            REGION,
            &case,
            row.engine,
            row.engine_version,
            false,
        )
        .await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the created CEV must be reported exactly once, got {entries:?}"
        );
        let entry = &entries[0];
        assert_eq!(
            text(entry, "EngineVersion"),
            row.engine_version,
            "case {case:?}: the report must name the CEV that was created, got {entry}"
        );
        assert_eq!(
            text(entry, "Status"),
            STATUS_AVAILABLE,
            "case {case:?}: the reported status must be the stored one, which the CEV \
             reached on its own, got {entry}"
        );
        assert_eq!(
            text(entry, "DBParameterGroupFamily"),
            row.parameter_group_family,
            "case {case:?}: the reported family must be the stored one, got {entry}"
        );
        assert_eq!(
            optional_text(entry, "CustomDBEngineVersionManifest").as_deref(),
            row.asked_for("Manifest"),
            "case {case:?}: the reported manifest must be the stored one, got {entry}"
        );
        assert_eq!(
            optional_text(entry, "KMSKeyId").as_deref(),
            row.asked_for("KMSKeyId"),
            "case {case:?}: the reported KMS key must be the stored one, got {entry}"
        );
    }
}

#[tokio::test]
async fn a_second_create_of_one_cev_name_is_refused_and_changes_nothing() {
    let router = emulator();

    for row in CASES {
        let case = row.case();
        let created = expect_ok(&case, call(&router, ACTION, &row.params(&[])).await);
        let arn = text(&strip(&created, "Image"), "DBEngineVersionArn");

        // The core attributes of a CEV are fixed when it is created, so a second create of
        // the same name is refused rather than rewriting the record — whatever media it
        // carries.
        let again = row.params(&[
            ("Description", "a-different-description"),
            ("DatabaseInstallationFilesS3BucketName", "other-cev-media"),
        ]);
        expect_error(
            &format!("{case}: created twice"),
            call(&router, ACTION, &again).await,
            400,
            "CustomDBEngineVersionAlreadyExistsFault",
        );

        let entries = described(&router, REGION, &case, row.engine, row.engine_version, true).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the refused create must not have stored a second CEV, got \
             {entries:?}"
        );
        let entry = &entries[0];
        assert_eq!(
            text(entry, "DBEngineVersionArn"),
            arn,
            "case {case:?}: the stored CEV must be the one the first create minted, got \
             {entry}"
        );
        assert_eq!(
            optional_text(entry, "DatabaseInstallationFilesS3BucketName").as_deref(),
            row.asked_for("DatabaseInstallationFilesS3BucketName"),
            "case {case:?}: the refused create must not have changed the stored media, got \
             {entry}"
        );
    }
}

/// One refusal: a request this operation cannot serve, and the status and wire error code
/// it answers with.
struct Refusal {
    case: &'static str,
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// The requests this operation refuses, one row per rule.
///
/// The engines are the ones no CEV can be created for: an RDS engine whose versions AWS
/// itself provides, a value that is no engine at all, and an engine the member DOES publish
/// but this service publishes no engine version of — a CEV is built on a published build,
/// so an engine with none is refused before any other member of the request is read. The
/// parameter combinations are the two that member documentation calls out ("if you specify
/// `UseAwsProvidedLatestImage`, you can't also specify `ImageId`"; "you can't specify a
/// different JSON manifest when you specify `SourceCustomDbEngineVersionIdentifier`"), plus
/// the image prerequisite ("for RDS Custom for SQL Server, an AMI ID is required to create a
/// CEV"), whose three alternative members are a combination too. `InvalidParameterValue` and
/// `InvalidParameterCombination` are the Query API's own codes for a bad value and a bad
/// pair; the others are this operation's modelled faults.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "an RDS engine whose versions AWS provides",
        params: &[("Engine", "postgres"), ("EngineVersion", "16.4.my_cev1")],
        status: 400,
        code: "InvalidParameterValue",
    },
    Refusal {
        case: "an Aurora engine",
        params: &[
            ("Engine", "aurora-mysql"),
            ("EngineVersion", "8.0.my_cev1"),
        ],
        status: 400,
        code: "InvalidParameterValue",
    },
    Refusal {
        case: "the RDS for Oracle engine rather than its RDS Custom engine",
        params: &[("Engine", "oracle-ee"), ("EngineVersion", "19.my_cev1")],
        status: 400,
        code: "InvalidParameterValue",
    },
    Refusal {
        case: "a value that is no engine at all",
        params: &[("Engine", "custom-oracle"), ("EngineVersion", "19.my_cev1")],
        status: 400,
        code: "InvalidParameterValue",
    },
    Refusal {
        case: "an engine this service publishes no engine version of",
        params: &[
            ("Engine", "custom-oracle-ee"),
            ("EngineVersion", "19.no_build_cev"),
            ("KMSKeyId", KMS_KEY_ID),
        ],
        status: 400,
        code: "InvalidParameterValue",
    },
    Refusal {
        case: "RDS Custom for SQL Server with no AMI",
        params: &[
            ("Engine", "custom-sqlserver-web"),
            ("EngineVersion", "15.00.4249.2.no_ami_cev"),
            ("KMSKeyId", KMS_KEY_ID),
        ],
        status: 400,
        code: "InvalidParameterCombination",
    },
    Refusal {
        case: "a KMS key identifier of no KMS form",
        params: &[
            ("Engine", "sqlserver-ee"),
            ("EngineVersion", "16.00.4175.1.bad_key_cev"),
            ("KMSKeyId", "not-a-kms-key"),
        ],
        status: 400,
        code: "KMSKeyNotAccessibleFault",
    },
    Refusal {
        case: "a named AMI and the service-provided latest one at once",
        params: &[
            ("Engine", "custom-sqlserver-se"),
            ("EngineVersion", "15.00.4249.2.two_images_cev"),
            ("KMSKeyId", KMS_KEY_ID),
            ("ImageId", "ami-0abcdef1234567890"),
            ("UseAwsProvidedLatestImage", "true"),
        ],
        status: 400,
        code: "InvalidParameterCombination",
    },
    Refusal {
        case: "a manifest beside a source CEV",
        params: &[
            ("Engine", "sqlserver-ee"),
            ("EngineVersion", "16.00.4175.1.manifest_and_source_cev"),
            ("KMSKeyId", KMS_KEY_ID),
            ("Manifest", MANIFEST),
            (
                "SourceCustomDbEngineVersionIdentifier",
                "arn:aws:rds:us-west-2:000000000000:cev:sqlserver-ee/16.00.4175.1.src/0",
            ),
        ],
        status: 400,
        code: "InvalidParameterCombination",
    },
    Refusal {
        case: "a source CEV nothing stored",
        params: &[
            ("Engine", "sqlserver-ee"),
            ("EngineVersion", "16.00.4175.1.no_source_cev"),
            ("KMSKeyId", KMS_KEY_ID),
            (
                "SourceCustomDbEngineVersionIdentifier",
                "arn:aws:rds:us-west-2:000000000000:cev:sqlserver-ee/16.00.4175.1.missing/0",
            ),
        ],
        status: 404,
        code: "CustomDBEngineVersionNotFoundFault",
    },
    Refusal {
        case: "a version this service already publishes for that engine",
        params: &[
            ("Engine", "custom-sqlserver-ee"),
            ("EngineVersion", "16.00.4165.4.v1"),
            ("KMSKeyId", KMS_KEY_ID),
            ("ImageId", "ami-0abcdef1234567890"),
        ],
        status: 400,
        code: "CustomDBEngineVersionAlreadyExistsFault",
    },
];

#[tokio::test]
async fn a_request_outside_the_rules_is_refused_and_stores_nothing() {
    let router = emulator();

    for refusal in REFUSALS {
        let case = refusal.case;
        expect_error(
            case,
            call(&router, ACTION, refusal.params).await,
            refusal.status,
            refusal.code,
        );

        // A refused create stores nothing: the engine version it named must not be
        // reported, whatever status it would have had.
        let engine = refusal
            .params
            .iter()
            .find(|(name, _)| *name == "Engine")
            .map(|(_, value)| *value)
            .expect("every refusal row names an engine");
        let engine_version = refusal
            .params
            .iter()
            .find(|(name, _)| *name == "EngineVersion")
            .map(|(_, value)| *value)
            .expect("every refusal row names an engine version");
        // A refused create stores nothing: the engine version it named must not be
        // reported, whatever status it would have had. A row whose `Engine` names no
        // published engine cannot be asked of that report at all — the report RESOLVES
        // that member and refuses a value that is no engine — so the create's own refusal
        // above is the whole of that row.
        if !is_published_engine(engine) {
            continue;
        }
        let entries = described(&router, REGION, case, engine, engine_version, true).await;
        let published = engine_version == "16.00.4165.4.v1";
        assert_eq!(
            entries.len(),
            usize::from(published),
            "case {case:?}: a refused create must leave the report as it was, got {entries:?}"
        );
    }
}

#[tokio::test]
async fn a_custom_version_belongs_to_the_region_it_was_created_in() {
    let router = emulator();
    let engine = "sqlserver-se";
    let engine_version = "15.00.4435.7.regional_cev";
    let params = [
        ("Engine", engine),
        ("EngineVersion", engine_version),
        ("KMSKeyId", KMS_KEY_ID),
    ];

    expect_ok(
        "create in the signed region",
        call_in(&router, REGION, ACTION, &params).await,
    );

    // The CEV is reported to the region that created it, by the report that lists the
    // engine versions a caller can use.
    let here = described(
        &router,
        REGION,
        "the region that created it",
        engine,
        engine_version,
        false,
    )
    .await;
    assert_eq!(
        here.len(),
        1,
        "the region the CEV was created in must report it, got {here:?}"
    );

    let elsewhere = described(
        &router,
        "eu-west-1",
        "another region",
        engine,
        engine_version,
        false,
    )
    .await;
    assert!(
        elsewhere.is_empty(),
        "a CEV is unique per customer per region, so another region must report none, got \
         {elsewhere:?}"
    );

    // The same name is therefore free in another region, and creating it there leaves the
    // first record alone.
    expect_ok(
        "create the same name in another region",
        call_in(&router, "eu-west-1", ACTION, &params).await,
    );
    let elsewhere = described(
        &router,
        "eu-west-1",
        "another region after its own create",
        engine,
        engine_version,
        false,
    )
    .await;
    assert_eq!(
        elsewhere.len(),
        1,
        "the second region must report the CEV it created, got {elsewhere:?}"
    );
    assert_ne!(
        text(&elsewhere[0], "DBEngineVersionArn"),
        text(&here[0], "DBEngineVersionArn"),
        "two regions' CEVs of one name are two records, so their ARNs differ"
    );
}

#[tokio::test]
async fn a_cev_created_from_a_source_carries_that_source_s_media() {
    let router = emulator();
    let engine = "sqlserver-se";
    let source_params = [
        ("Engine", engine),
        ("EngineVersion", "15.00.4435.7.source_cev"),
        ("KMSKeyId", KMS_KEY_ID),
        ("Manifest", MANIFEST),
        ("DatabaseInstallationFilesS3BucketName", "my-cev-media"),
        ("DatabaseInstallationFiles.member.1", "SQLServer2019.iso"),
    ];
    let source = expect_ok(
        "create the source CEV",
        call(&router, ACTION, &source_params).await,
    );
    let source_arn = text(&strip(&source, "Image"), "DBEngineVersionArn");

    let derived = expect_ok(
        "create from the source CEV",
        call(
            &router,
            ACTION,
            &[
                ("Engine".to_string(), engine.to_string()),
                ("EngineVersion".to_string(), "15.00.4435.7.derived_cev".to_string()),
                ("KMSKeyId".to_string(), OTHER_KMS_KEY_ID.to_string()),
                (
                    "SourceCustomDbEngineVersionIdentifier".to_string(),
                    source_arn.clone(),
                ),
            ],
        )
        .await,
    );
    let scalars = strip(&derived, "Image");

    assert_eq!(
        text(&scalars, "EngineVersion"),
        "15.00.4435.7.derived_cev",
        "the new CEV carries its own name, got {scalars}"
    );
    assert_ne!(
        text(&scalars, "DBEngineVersionArn"),
        source_arn,
        "the new CEV is its own record, so it carries its own ARN"
    );
    // "You can't specify a different JSON manifest when you specify
    // SourceCustomDbEngineVersionIdentifier" — because the manifest and the installation
    // files come from the source.
    assert_eq!(
        optional_text(&scalars, "CustomDBEngineVersionManifest").as_deref(),
        Some(MANIFEST),
        "the derived CEV must carry the source's manifest, got {scalars}"
    );
    assert_eq!(
        optional_text(&scalars, "DatabaseInstallationFilesS3BucketName").as_deref(),
        Some("my-cev-media"),
        "the derived CEV must carry the source's installation bucket, got {scalars}"
    );
    assert_eq!(
        installation_files(&scalars),
        vec!["SQLServer2019.iso".to_string()],
        "the derived CEV must carry the source's installation files, got {scalars}"
    );
    // Its own KMS key is the one its own request named.
    assert_eq!(
        optional_text(&scalars, "KMSKeyId").as_deref(),
        Some(OTHER_KMS_KEY_ID),
        "the derived CEV must carry the key its own create named, got {scalars}"
    );
}
