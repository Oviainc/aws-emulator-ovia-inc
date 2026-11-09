// SPDX-License-Identifier: CC0-1.0

//! `DescribeType` over the awsQuery wire: the extension registry read.
//!
//! The rule every case here holds the service to: **`DescribeType` reports the registry
//! record this account and Region persists for the extension the request identifies — by
//! `Arn`, or by `Type` and `TypeName` — at the version the request selects (`VersionId`, a
//! version ARN, `PublicVersionNumber`, or the record's default version), answering the
//! modelled `TypeNotFoundException` when the registry holds no such extension or version
//! and the modelled `CFNRegistryException` when the request identifies none; and each
//! member it reports is gated on the record's visibility exactly as the model documents
//! that member.**
//!
//! The table below is one row per way a request can select a record and a version, over
//! records spanning every `RegistryType` (`RESOURCE`, `MODULE`, `HOOK`) and every
//! visibility a record can have (private, public third-party, public and
//! Amazon-Web-Services-published), with a version count above one so a default version is
//! distinguishable from a named one. The recorded trace this suite was written for is ONE
//! row of the published-extension table: a read of `AWS::S3::Bucket`, one of the public
//! extensions every account's registry holds without anybody registering it.
//!
//! `RegisterType`, the registry's one WRITER still a scaffold stub, has an issue of its own,
//! so the records below are seeded straight into the store — the same device
//! `stack_lifecycle.rs` uses for records a bound's worth of wire calls could not create.
//! Every READ goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    deprecated_status, provisioning_type, public_type_arn, registry_type, type_arn,
    type_tests_status, type_version_arn, visibility, StoredExtension, StoredLoggingConfig,
    StoredRequiredActivatedType, StoredTypeVersion,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, enroll_as_a_marketplace_seller, member, router_with_engine, store, xml_text, xml_texts,
    REGION,
};

/// Another Region's registry, for the row that reads a record registered elsewhere.
pub(crate) const OTHER_REGION: &str = "us-west-2";

/// The publisher of the public third-party record (`PublisherId` is
/// `@pattern("^[0-9a-zA-Z]{12,40}$")`).
pub(crate) const PUBLISHER_ID: &str = "abcdef123456";

/// An instant to register the seeded versions at, so every timestamp a case asserts comes
/// from the record rather than from the clock.
fn registered_at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset_secs)
}

/// A version of the private resource-type record, carrying everything a private extension
/// version can carry.
fn private_resource_version(arn: &str, version_id: &str, offset_secs: u64) -> StoredTypeVersion {
    let mut version = StoredTypeVersion::new(arn, version_id, registered_at(offset_secs));
    version.description = Some(format!("Sample resource type, version {version_id}"));
    version.schema = Some(format!(
        "{{\"typeName\":\"My::Sample::Resource\",\"version\":\"{version_id}\"}}"
    ));
    version.source_url = Some("https://example.com/source".to_string());
    version.documentation_url = Some("https://example.com/docs".to_string());
    version.execution_role_arn =
        Some("arn:aws:iam::000000000000:role/cfn-sample-execution".to_string());
    version.logging_config = Some(StoredLoggingConfig {
        log_role_arn: "arn:aws:iam::000000000000:role/cfn-sample-logging".to_string(),
        log_group_name: format!("my-sample-resource-{version_id}"),
    });
    version
}

/// The private resource type, with TWO versions and the SECOND as its default: that is
/// what makes "the default extension version" distinguishable from "that specific
/// extension version".
fn private_resource() -> StoredExtension {
    let mut extension =
        StoredExtension::new(REGION, registry_type::RESOURCE, "My::Sample::Resource");
    let arn = extension.arn.clone();

    let mut first = private_resource_version(&arn, "00000001", 0);
    first.provisioning_type = Some(provisioning_type::FULLY_MUTABLE.to_string());
    first.type_tests_status = Some(type_tests_status::PASSED.to_string());
    first.type_tests_status_description = Some("All contract tests passed".to_string());

    let mut second = private_resource_version(&arn, "00000002", 60);
    second.provisioning_type = Some(provisioning_type::IMMUTABLE.to_string());
    second.deprecated_status = deprecated_status::DEPRECATED.to_string();
    second.type_tests_status = Some(type_tests_status::FAILED.to_string());
    second.type_tests_status_description = Some("One contract test failed".to_string());

    extension.default_version_id = Some("00000002".to_string());
    extension.configuration_schema = Some("{\"SampleKey\":\"sample-value\"}".to_string());
    extension.versions = vec![first, second];
    extension
}

/// A private MODULE, whose `RequiredActivatedTypes` are "the public third-party extensions
/// that must be activated in your account in order for the module itself to be activated".
fn private_module() -> StoredExtension {
    let mut extension =
        StoredExtension::new(REGION, registry_type::MODULE, "My::Sample::Thing::MODULE");
    let mut version = StoredTypeVersion::new(&extension.arn, "00000001", registered_at(120));
    version.description = Some("Sample module".to_string());
    version.required_activated_types = vec![StoredRequiredActivatedType {
        type_name_alias: Some("MyAlias::Sample::Resource".to_string()),
        original_type_name: Some("Publisher::Sample::Resource".to_string()),
        publisher_id: Some(PUBLISHER_ID.to_string()),
        supported_major_versions: vec![1, 2],
    }];
    extension.default_version_id = Some("00000001".to_string());
    extension.versions = vec![version];
    extension
}

/// A private HOOK, whose version id is not the record's first: a version is addressed by
/// the id the record carries, never by its position.
fn private_hook() -> StoredExtension {
    let mut extension = StoredExtension::new(REGION, registry_type::HOOK, "My::Sample::Hook");
    let mut version = StoredTypeVersion::new(&extension.arn, "00000003", registered_at(180));
    version.provisioning_type = Some(provisioning_type::NON_PROVISIONABLE.to_string());
    extension.default_version_id = Some("00000003".to_string());
    extension.versions = vec![version];
    extension
}

/// A public THIRD-PARTY extension activated in this account under a type name alias.
fn activated_public_extension() -> StoredExtension {
    let mut extension =
        StoredExtension::new(REGION, registry_type::RESOURCE, "MyAlias::Sample::Resource");
    let mut version = StoredTypeVersion::new(&extension.arn, "00000001", registered_at(240));
    version.description = Some("A third party's resource type".to_string());
    version.public_version_number = Some("1.4.0".to_string());
    version.provisioning_type = Some(provisioning_type::FULLY_MUTABLE.to_string());
    // Private-only members on a PUBLIC record: persisted, and withheld from the response.
    version.execution_role_arn =
        Some("arn:aws:iam::000000000000:role/cfn-public-execution".to_string());
    version.logging_config = Some(StoredLoggingConfig {
        log_role_arn: "arn:aws:iam::000000000000:role/cfn-public-logging".to_string(),
        log_group_name: "my-alias-sample-resource".to_string(),
    });
    version.type_tests_status = Some(type_tests_status::PASSED.to_string());

    extension.visibility = visibility::PUBLIC.to_string();
    extension.publisher_id = Some(PUBLISHER_ID.to_string());
    extension.original_type_name = Some("Publisher::Sample::Resource".to_string());
    // A PUBLIC extension's own ARN carries no account id, which the modelled `TypeArn`
    // pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$`
    // admits.
    extension.original_type_arn = Some(format!(
        "arn:aws:cloudformation:{REGION}::type/resource/Publisher-Sample-Resource"
    ));
    extension.latest_public_version = Some("2.1.0".to_string());
    extension.is_activated = Some(true);
    extension.auto_update = Some(true);
    extension.versions = vec![version];
    extension
}

/// A PUBLIC extension published by Amazon Web Services, which "are not assigned a
/// publisher ID" — so the members the model withholds from public THIRD-PARTY extensions
/// are reported for it.
fn aws_published_extension() -> StoredExtension {
    let mut extension =
        StoredExtension::new(REGION, registry_type::RESOURCE, "AWS::Sample::Resource");
    let mut version = StoredTypeVersion::new(&extension.arn, "00000001", registered_at(300));
    version.provisioning_type = Some(provisioning_type::FULLY_MUTABLE.to_string());
    version.public_version_number = Some("1.0.0".to_string());
    extension.visibility = visibility::PUBLIC.to_string();
    extension.latest_public_version = Some("1.0.0".to_string());
    extension.versions = vec![version];
    extension
}

/// The same private resource type name, registered in ANOTHER Region's registry.
fn other_region_resource() -> StoredExtension {
    let mut extension =
        StoredExtension::new(OTHER_REGION, registry_type::RESOURCE, "Other::Region::Resource");
    let version = StoredTypeVersion::new(&extension.arn, "00000001", registered_at(360));
    extension.default_version_id = Some("00000001".to_string());
    extension.versions = vec![version];
    extension
}

/// A router over a store seeded with every record above.
///
/// This crate's ONE registry fixture: `list_types.rs` lists the same records this suite
/// describes one at a time, so the two reads are held to one set of records. The account is
/// an enrolled Amazon Web Services Marketplace seller, which is what the suites that
/// register a publisher over this fixture (`publish_type.rs`, `type_testing.rs`) require of
/// the account registering.
pub(crate) fn router_with_registry() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    enroll_as_a_marketplace_seller(&engine);
    {
        let state = store(&engine);
        state.lock().extensions = vec![
            private_resource(),
            private_module(),
            private_hook(),
            activated_public_extension(),
            aws_published_extension(),
            other_region_resource(),
        ];
    }
    (router, engine)
}

async fn describe_type(
    router: &MultiServiceRouter,
    members: &[(&str, &str)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "DescribeType", &members).await
}

/// One row of the reporting table: a request that must succeed, and the members the
/// response must carry (`Some`) or must NOT carry at all (`None`).
struct ReadCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    reported: Vec<(&'static str, Option<String>)>,
}

fn present(value: &str) -> Option<String> {
    Some(value.to_string())
}

/// The inside of `<RequiredActivatedTypes>` and the document around it.
///
/// `xml_text` answers the FIRST occurrence of a tag anywhere in the document, and
/// `RequiredActivatedType` carries `OriginalTypeName` and `PublisherId` members of its own
/// — the same names as two TOP-LEVEL members `DescribeType` reports only for a public
/// record. Splitting the document is what lets a case assert each on the right side of it.
pub(crate) fn split_required_activated_types(body: &str) -> (String, String) {
    let open = "<RequiredActivatedTypes>";
    let close = "</RequiredActivatedTypes>";
    match (body.find(open), body.find(close)) {
        (Some(start), Some(end)) if start < end => (
            body[start + open.len()..end].to_string(),
            format!("{}{}", &body[..start], &body[end + close.len()..]),
        ),
        _ => (String::new(), body.to_string()),
    }
}

/// Every way a request can select a record and a version, with what that selection must
/// report.
fn read_cases() -> Vec<ReadCase> {
    let resource_arn = type_arn(REGION, registry_type::RESOURCE, "My::Sample::Resource");
    let module_arn = type_arn(REGION, registry_type::MODULE, "My::Sample::Thing::MODULE");
    let hook_arn = type_arn(REGION, registry_type::HOOK, "My::Sample::Hook");
    let public_arn = type_arn(REGION, registry_type::RESOURCE, "MyAlias::Sample::Resource");
    let aws_arn = type_arn(REGION, registry_type::RESOURCE, "AWS::Sample::Resource");

    vec![
        ReadCase {
            case: "private resource type, no version named: the DEFAULT version",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            reported: vec![
                ("Arn", present(&type_version_arn(&resource_arn, "00000002"))),
                ("Type", present(registry_type::RESOURCE)),
                ("TypeName", present("My::Sample::Resource")),
                ("Visibility", present(visibility::PRIVATE)),
                ("DefaultVersionId", present("00000002")),
                ("IsDefaultVersion", present("true")),
                ("DeprecatedStatus", present(deprecated_status::DEPRECATED)),
                ("ProvisioningType", present(provisioning_type::IMMUTABLE)),
                (
                    "Description",
                    present("Sample resource type, version 00000002"),
                ),
                ("ConfigurationSchema", present("{\"SampleKey\":\"sample-value\"}")),
                // "To return the extension test status of a specific extension version,
                // you must specify VersionId" — this request named none.
                ("TypeTestsStatus", None),
                ("TypeTestsStatusDescription", None),
                // Public-only members, withheld from a private record.
                ("PublisherId", None),
                ("IsActivated", None),
                ("AutoUpdate", None),
                ("PublicVersionNumber", None),
                ("LatestPublicVersion", None),
                ("OriginalTypeName", None),
                ("OriginalTypeArn", None),
            ],
        },
        ReadCase {
            case: "private resource type, VersionId naming the non-default version",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
                ("VersionId", "00000001".to_string()),
            ],
            reported: vec![
                ("Arn", present(&type_version_arn(&resource_arn, "00000001"))),
                ("IsDefaultVersion", present("false")),
                ("DeprecatedStatus", present(deprecated_status::LIVE)),
                (
                    "ProvisioningType",
                    present(provisioning_type::FULLY_MUTABLE),
                ),
                (
                    "Description",
                    present("Sample resource type, version 00000001"),
                ),
                ("TypeTestsStatus", present(type_tests_status::PASSED)),
                (
                    "TypeTestsStatusDescription",
                    present("All contract tests passed"),
                ),
                (
                    "ExecutionRoleArn",
                    present("arn:aws:iam::000000000000:role/cfn-sample-execution"),
                ),
                (
                    "LogRoleArn",
                    present("arn:aws:iam::000000000000:role/cfn-sample-logging"),
                ),
                ("LogGroupName", present("my-sample-resource-00000001")),
                ("SourceUrl", present("https://example.com/source")),
                ("DocumentationUrl", present("https://example.com/docs")),
            ],
        },
        ReadCase {
            case: "private resource type, identified by the extension's ARN",
            members: vec![("Arn", resource_arn.clone())],
            reported: vec![
                ("Arn", present(&type_version_arn(&resource_arn, "00000002"))),
                ("TypeName", present("My::Sample::Resource")),
                ("IsDefaultVersion", present("true")),
            ],
        },
        ReadCase {
            case: "private resource type, identified by an extension VERSION ARN",
            members: vec![("Arn", type_version_arn(&resource_arn, "00000001"))],
            reported: vec![
                ("Arn", present(&type_version_arn(&resource_arn, "00000001"))),
                ("IsDefaultVersion", present("false")),
                ("DeprecatedStatus", present(deprecated_status::LIVE)),
                // The ARN pins the version id, which is what makes the test status
                // reportable.
                ("TypeTestsStatus", present(type_tests_status::PASSED)),
            ],
        },
        ReadCase {
            case: "private module, with the public extensions its activation requires",
            members: vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "My::Sample::Thing::MODULE".to_string()),
            ],
            reported: vec![
                ("Arn", present(&type_version_arn(&module_arn, "00000001"))),
                ("Type", present(registry_type::MODULE)),
                ("TypeName", present("My::Sample::Thing::MODULE")),
                ("Description", present("Sample module")),
                // A module carries no provisioning behaviour: that is "for resource type
                // extensions".
                ("ProvisioningType", None),
            ],
        },
        ReadCase {
            case: "private hook, whose only version is not numbered one",
            members: vec![
                ("Type", registry_type::HOOK.to_string()),
                ("TypeName", "My::Sample::Hook".to_string()),
            ],
            reported: vec![
                ("Arn", present(&type_version_arn(&hook_arn, "00000003"))),
                ("Type", present(registry_type::HOOK)),
                ("DefaultVersionId", present("00000003")),
                ("IsDefaultVersion", present("true")),
                (
                    "ProvisioningType",
                    present(provisioning_type::NON_PROVISIONABLE),
                ),
            ],
        },
        ReadCase {
            case: "public third-party extension activated under an alias",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
            ],
            reported: vec![
                ("Arn", present(&type_version_arn(&public_arn, "00000001"))),
                ("TypeName", present("MyAlias::Sample::Resource")),
                // "The scope at which the extension is visible and usable": an activation is
                // usable within the account and Region that made it, which is PRIVATE, while
                // the extension it was activated FROM stays published.
                ("Visibility", present(visibility::PRIVATE)),
                ("PublisherId", present(PUBLISHER_ID)),
                ("IsActivated", present("true")),
                ("AutoUpdate", present("true")),
                ("OriginalTypeName", present("Publisher::Sample::Resource")),
                (
                    "OriginalTypeArn",
                    present(&format!(
                        "arn:aws:cloudformation:{REGION}::type/resource/Publisher-Sample-Resource"
                    )),
                ),
                ("PublicVersionNumber", present("1.4.0")),
                // "This only applies if you specify a public extension, and you don't
                // specify a version."
                ("LatestPublicVersion", present("2.1.0")),
                // `DEPRECATED` is "the extension has been deactivated or deregistered", which
                // is a statement about the copy this account activated — so the activation
                // reports its own status, LIVE until something deactivates it.
                ("DeprecatedStatus", present(deprecated_status::LIVE)),
                // Withheld from a public third-party record, however the store holds them.
                ("DefaultVersionId", None),
                ("IsDefaultVersion", None),
                ("TypeTestsStatus", None),
                ("ExecutionRoleArn", None),
                ("LogRoleArn", None),
            ],
        },
        ReadCase {
            case: "public third-party extension, PublicVersionNumber naming the version",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
                ("PublicVersionNumber", "1.4.0".to_string()),
            ],
            reported: vec![
                ("Arn", present(&type_version_arn(&public_arn, "00000001"))),
                ("PublicVersionNumber", present("1.4.0")),
                // A version WAS named, so the latest available one is not reported.
                ("LatestPublicVersion", None),
            ],
        },
        ReadCase {
            case: "public third-party extension, PublisherId naming its publisher",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
                ("PublisherId", PUBLISHER_ID.to_string()),
            ],
            reported: vec![("PublisherId", present(PUBLISHER_ID))],
        },
        ReadCase {
            case: "public extension published by Amazon Web Services, which has no publisher id",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "AWS::Sample::Resource".to_string()),
            ],
            reported: vec![
                ("Arn", present(&type_version_arn(&aws_arn, "00000001"))),
                ("Visibility", present(visibility::PUBLIC)),
                // Not a THIRD-PARTY extension, so these two are reported.
                ("IsDefaultVersion", present("true")),
                ("DeprecatedStatus", present(deprecated_status::LIVE)),
                ("LatestPublicVersion", present("1.0.0")),
                // Still a PUBLIC extension: no default version id, no publisher.
                ("DefaultVersionId", None),
                ("PublisherId", None),
                ("IsActivated", None),
                ("AutoUpdate", None),
            ],
        },
    ]
}

/// Each selection reports the record the registry holds, member by member.
#[tokio::test]
async fn describe_type_reports_the_selected_registry_record() {
    let (router, _engine) = router_with_registry();

    for ReadCase {
        case,
        members,
        reported,
    } in read_cases()
    {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status, body) = describe_type(&router, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        for (tag, expected) in reported {
            let actual = xml_text(&body, tag);
            assert_eq!(
                actual.as_deref(),
                expected.as_deref(),
                "case {case:?}: expected <{tag}> to be {expected:?}, got {actual:?} from \
                 {body:?}"
            );
        }
    }
}

/// A module's `RequiredActivatedTypes` report the whole stored list, including every
/// supported major version — and only there: `OriginalTypeName` and `PublisherId` are
/// members of that nested structure AND top-level members the handler withholds from a
/// private record, so each is asserted on the side of the document it belongs to.
#[tokio::test]
async fn a_module_reports_every_required_activated_type_it_stores() {
    let (router, _engine) = router_with_registry();

    let (status, body) = describe_type(
        &router,
        &[
            ("Type", registry_type::MODULE),
            ("TypeName", "My::Sample::Thing::MODULE"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"module required types\": expected HTTP 200, got {status} with {body:?}"
    );
    let (required, rest) = split_required_activated_types(&body);
    for (tag, expected) in [
        ("TypeNameAlias", "MyAlias::Sample::Resource"),
        ("OriginalTypeName", "Publisher::Sample::Resource"),
        ("PublisherId", PUBLISHER_ID),
    ] {
        let actual = xml_text(&required, tag);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case \"module required types\": expected the nested <{tag}> to be \
             {expected:?}, got {actual:?} from {required:?}"
        );
    }
    let majors = xml_texts(&required, "member");
    assert_eq!(
        majors,
        vec!["1".to_string(), "2".to_string()],
        "case \"module required types\": expected the stored SupportedMajorVersions, got \
         {majors:?} from {body:?}"
    );
    // The same member names at the TOP level belong to public records only, and this one
    // is private.
    for tag in ["OriginalTypeName", "PublisherId", "OriginalTypeArn"] {
        let actual = xml_text(&rest, tag);
        assert_eq!(
            actual, None,
            "case \"module required types\": expected no top-level <{tag}> on a private \
             record, got {actual:?} from {rest:?}"
        );
    }
}

/// A version's registration instants come from the record: two versions of one extension
/// report different ones.
#[tokio::test]
async fn each_version_reports_the_instant_it_was_registered() {
    let (router, _engine) = router_with_registry();

    let mut seen: Vec<(String, String)> = Vec::new();
    for version_id in ["00000001", "00000002"] {
        let (status, body) = describe_type(
            &router,
            &[
                ("Type", registry_type::RESOURCE),
                ("TypeName", "My::Sample::Resource"),
                ("VersionId", version_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {version_id:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let created = xml_text(&body, "TimeCreated").unwrap_or_else(|| {
            panic!("case {version_id:?}: expected a <TimeCreated>, got {body:?}")
        });
        let updated = xml_text(&body, "LastUpdated").unwrap_or_else(|| {
            panic!("case {version_id:?}: expected a <LastUpdated>, got {body:?}")
        });
        assert!(
            created.starts_with("2023-11-14T22:"),
            "case {version_id:?}: expected the seeded registration instant, got {created:?}"
        );
        assert_eq!(
            created, updated,
            "case {version_id:?}: a version registered once reports one instant, got \
             {created:?} and {updated:?}"
        );
        assert!(
            !seen.iter().any(|(_, instant)| *instant == created),
            "case {version_id:?}: expected this version's own instant, got the already \
             reported {created:?} ({seen:?})"
        );
        seen.push((version_id.to_string(), created));
    }
}

/// Every resource type this emulator implements is in the registry as the
/// Amazon-Web-Services-published public extension it is, without anybody registering it.
#[tokio::test]
async fn every_implemented_resource_type_is_an_aws_published_public_extension() {
    let (router, _engine) = router_with_registry();

    for type_name in IMPLEMENTED_RESOURCE_TYPES {
        let (status, body) = describe_type(
            &router,
            &[("Type", registry_type::RESOURCE), ("TypeName", type_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {type_name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        for (tag, expected) in [
            ("TypeName", present(type_name)),
            ("Type", present(registry_type::RESOURCE)),
            (
                "Arn",
                present(&public_type_arn(REGION, registry_type::RESOURCE, type_name)),
            ),
            ("Visibility", present(visibility::PUBLIC)),
            ("DeprecatedStatus", present(deprecated_status::LIVE)),
            (
                "ProvisioningType",
                present(provisioning_type::FULLY_MUTABLE),
            ),
            ("IsDefaultVersion", present("true")),
            // Not known for a published extension, so not reported: no publisher and no
            // private-extension members. (`Schema` IS reported — the registry derives the
            // type's resource schema — and `published_type_schema.rs` is the suite that
            // pins what it carries.)
            ("DefaultVersionId", None),
            ("PublisherId", None),
            ("IsActivated", None),
            ("ExecutionRoleArn", None),
            ("TypeTestsStatus", None),
        ] {
            let actual = xml_text(&body, tag);
            assert_eq!(
                actual.as_deref(),
                expected.as_deref(),
                "case {type_name:?}: expected <{tag}> to be {expected:?}, got {actual:?} \
                 from {body:?}"
            );
        }
        // Every registry record reports when its version entered the registry and where
        // the extension's source lives; `drift_and_resource_reads.rs` is what pins the
        // VALUES those two carry for a published extension.
        for tag in ["TimeCreated", "LastUpdated", "SourceUrl"] {
            let actual = xml_text(&body, tag);
            assert!(
                actual.is_some(),
                "case {type_name:?}: expected a <{tag}> on a published extension, got \
                 {actual:?} from {body:?}"
            );
        }
    }
}

/// A public resource type no resource provider stands behind is in the registry too, as the
/// `NON_PROVISIONABLE` record it is: "the resource type doesn't include create, read, and
/// delete handlers, and therefore can't actually be provisioned". It carries no schema and
/// no source URL either — a type with no handler package has no document defining it and no
/// provider repository to name — which is what tells it apart from a provisioned one.
#[tokio::test]
async fn a_published_resource_type_with_no_provider_is_non_provisionable() {
    let (router, _engine) = router_with_registry();

    for type_name in ::service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES {
        let (status, body) = describe_type(
            &router,
            &[("Type", registry_type::RESOURCE), ("TypeName", type_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {type_name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        for (tag, expected) in [
            ("TypeName", present(type_name)),
            ("Type", present(registry_type::RESOURCE)),
            (
                "Arn",
                present(&public_type_arn(REGION, registry_type::RESOURCE, type_name)),
            ),
            ("Visibility", present(visibility::PUBLIC)),
            ("DeprecatedStatus", present(deprecated_status::LIVE)),
            (
                "ProvisioningType",
                present(provisioning_type::NON_PROVISIONABLE),
            ),
            ("IsDefaultVersion", present("true")),
            // No handler package stands behind it, so neither the document that would
            // define it nor the repository that would publish it exists here.
            ("Schema", None),
            ("SourceUrl", None),
            // Not known for a published extension, as for every other one of them.
            ("DefaultVersionId", None),
            ("PublisherId", None),
            ("IsActivated", None),
            ("ExecutionRoleArn", None),
            ("TypeTestsStatus", None),
        ] {
            let actual = xml_text(&body, tag);
            assert_eq!(
                actual.as_deref(),
                expected.as_deref(),
                "case {type_name:?}: expected <{tag}> to be {expected:?}, got {actual:?} \
                 from {body:?}"
            );
        }
    }
}

/// A published extension is addressable by its ARN too — the account-less form a public
/// extension carries — and only in the Region the request names.
#[tokio::test]
async fn a_published_extension_is_addressable_by_its_public_arn() {
    let (router, _engine) = router_with_registry();
    let type_name = "AWS::S3::Bucket";

    let (status, body) = describe_type(
        &router,
        &[(
            "Arn",
            &public_type_arn(REGION, registry_type::RESOURCE, type_name),
        )],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"public ARN\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        xml_text(&body, "TypeName").as_deref(),
        Some(type_name),
        "case \"public ARN\": expected the catalogued type name, got {body:?}"
    );

    // The ARN of the SAME extension in another Region names that Region's registry, not
    // this one: the registry is regional, so a request served here finds nothing under it.
    let elsewhere = public_type_arn(OTHER_REGION, registry_type::RESOURCE, type_name);
    let (status, body) = describe_type(&router, &[("Arn", &elsewhere)]).await;
    assert_eq!(
        status, 404,
        "case \"another Region's public ARN\": expected HTTP 404, got {status} with {body:?}"
    );

    // An ACCOUNT-scoped ARN of the same name is not that public extension: a published
    // extension belongs to no account.
    let account_scoped = type_arn(REGION, registry_type::RESOURCE, type_name);
    let (status, body) = describe_type(&router, &[("Arn", &account_scoped)]).await;
    assert_eq!(
        status, 404,
        "case \"account-scoped ARN of a published extension\": expected HTTP 404, got \
         {status} with {body:?}"
    );
}

/// One row of the refusal table: a request that must fail, with the status and wire code
/// it must fail with.
struct RefusedCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
}

/// Every way a request can name no addressable extension or version (404
/// `TypeNotFoundException`), and every way it can identify no extension at all (400
/// `CFNRegistryException`).
fn refused_cases() -> Vec<RefusedCase> {
    let resource_arn = type_arn(REGION, registry_type::RESOURCE, "My::Sample::Resource");
    let not_found = |case: &'static str, members: Vec<(&'static str, String)>| RefusedCase {
        case,
        members,
        status: 404,
        code: "TypeNotFoundException",
    };
    let refused = |case: &'static str, members: Vec<(&'static str, String)>| RefusedCase {
        case,
        members,
        status: 400,
        code: "CFNRegistryException",
    };

    vec![
        not_found(
            "a type name no registry write has created",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "No::Such::Resource".to_string()),
            ],
        ),
        not_found(
            "an AWS-namespaced resource type this emulator does not implement",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "AWS::NotAService::NotAType".to_string()),
            ],
        ),
        not_found(
            "a published resource type asked for as a module",
            vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "AWS::S3::Bucket".to_string()),
            ],
        ),
        not_found(
            "a version a published extension does not carry",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "AWS::S3::Bucket".to_string()),
                ("VersionId", "00000099".to_string()),
            ],
        ),
        not_found(
            "a registered name, under another extension kind",
            vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
        ),
        not_found(
            "a name registered in another Region's registry",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "Other::Region::Resource".to_string()),
            ],
        ),
        not_found(
            "a VersionId the record does not carry",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
                ("VersionId", "00000099".to_string()),
            ],
        ),
        not_found(
            "a PublicVersionNumber the publisher never published",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
                ("PublicVersionNumber", "9.9.9".to_string()),
            ],
        ),
        not_found(
            "a PublisherId that published no such extension",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
                ("PublisherId", "fedcba654321".to_string()),
            ],
        ),
        not_found(
            "a PublisherId against a private extension, which has none",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
                ("PublisherId", PUBLISHER_ID.to_string()),
            ],
        ),
        not_found(
            "an ARN no record carries",
            vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource"),
            )],
        ),
        not_found(
            "the ARN of a record another Region's registry holds",
            vec![(
                "Arn",
                type_arn(OTHER_REGION, registry_type::RESOURCE, "Other::Region::Resource"),
            )],
        ),
        not_found(
            "a version ARN whose version the record does not carry",
            vec![("Arn", type_version_arn(&resource_arn, "00000099"))],
        ),
        refused("a request identifying no extension at all", vec![]),
        refused(
            "Type without TypeName",
            vec![("Type", registry_type::RESOURCE.to_string())],
        ),
        refused(
            "TypeName without Type",
            vec![("TypeName", "My::Sample::Resource".to_string())],
        ),
        refused(
            "both identification forms at once",
            vec![
                ("Arn", resource_arn.clone()),
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
        ),
        refused(
            "a VersionId disagreeing with the version ARN beside it",
            vec![
                ("Arn", type_version_arn(&resource_arn, "00000001")),
                ("VersionId", "00000002".to_string()),
            ],
        ),
        refused(
            "both version selectors at once",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
                ("VersionId", "00000001".to_string()),
                ("PublicVersionNumber", "1.4.0".to_string()),
            ],
        ),
    ]
}

/// A request that names nothing addressable is refused with the modelled error and status,
/// never with an empty success.
#[tokio::test]
async fn an_unaddressable_or_unidentified_read_is_refused() {
    let (router, _engine) = router_with_registry();

    for RefusedCase {
        case,
        members,
        status: expected_status,
        code,
    } in refused_cases()
    {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status, body) = describe_type(&router, &members).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(code),
            "case {case:?}: expected <Code>{code}</Code>, got {body:?}"
        );
    }
}

/// `DescribeType` is a report: no read of it — successful, not-found or refused — changes
/// what a later caller sees.
#[tokio::test]
async fn a_registry_read_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_registry();
    let before = format!("{:?}", store(&engine).lock().extensions);

    let mut reads: Vec<Vec<(&str, &str)>> = vec![
        vec![
            ("Type", registry_type::RESOURCE),
            ("TypeName", "My::Sample::Resource"),
        ],
        vec![
            ("Type", registry_type::RESOURCE),
            ("TypeName", "No::Such::Resource"),
        ],
        vec![],
    ];
    reads.push(vec![
        ("Type", registry_type::MODULE),
        ("TypeName", "My::Sample::Thing::MODULE"),
    ]);

    for members in &reads {
        describe_type(&router, members).await;
    }
    // Twice over, so a read that consumed or moved something would show on the second.
    for members in &reads {
        describe_type(&router, members).await;
    }

    let after = format!("{:?}", store(&engine).lock().extensions);
    assert_eq!(
        before, after,
        "case \"reads are reports\": expected the registry to be untouched, got {after:?} \
         from {before:?}"
    );
}
