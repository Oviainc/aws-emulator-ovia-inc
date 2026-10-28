// SPDX-License-Identifier: CC0-1.0

//! `CreatePlatformVersion` end to end over this service's awsQuery surface: the custom
//! platform version it records is read back by `DescribePlatformVersion`, and the
//! `Tags` the request applied to it are the tags the two tag operations report and
//! mutate for that version's `PlatformArn`.
//!
//! The rule under test: a create that takes `Tags` stores them ON the resource it
//! records, and the tag operations resolve an Elastic Beanstalk `ResourceArn` to that
//! stored record — so every taggable resource a create minted answers its own tags,
//! and an ARN of a taggable type naming no stored record is still
//! `ResourceNotFoundException`.
//!
//! The rows span what the rule generalises over: how many tags the create applies
//! (none, one, several, the quota the service publishes), the shape of a key and a
//! value (plain, XML metacharacters, the modelled length bounds) and the Region the
//! create was signed for, which the version's ARN then carries. Everything is driven
//! over HTTP through the shared harness, so a tag that is not persisted — or one
//! answered from anywhere other than the store — fails these tests. The tag encoder,
//! the tag reader and the "these are the resource's tags" assertion are the ones
//! `resource_tags.rs` already owns, so this suite states no second copy of them.
//!
//! The tags are the version's ALONE: an environment launched on it inherits none of
//! them.

use crate::lifecycle::{account, Emulator, PAST_DUE, TEST_REGION};
use crate::resource_tags::{as_params, assert_tags, tag, tag_params};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// Another Region, so the Region a version's ARN carries is asserted against
/// something that is not the one the harness signs with by default.
const OTHER_REGION: &str = "eu-west-1";

/// One create under test: the platform version to record, the Region the call is
/// signed for and the tags it applies.
struct Row {
    case: &'static str,
    platform_name: &'static str,
    platform_version: &'static str,
    region: &'static str,
    tags: Vec<(String, String)>,
}

/// The awsQuery parameters of one create: the required bundle location plus the tag
/// pairs the `Tags` list serializes as, built by the one encoder
/// [`crate::resource_tags`] owns.
fn create_params(row: &Row) -> Vec<(String, String)> {
    let mut params = vec![
        ("PlatformName".to_string(), row.platform_name.to_string()),
        (
            "PlatformVersion".to_string(),
            row.platform_version.to_string(),
        ),
        (
            "PlatformDefinitionBundle.S3Bucket".to_string(),
            "platform-bundles".to_string(),
        ),
        (
            "PlatformDefinitionBundle.S3Key".to_string(),
            "platform.zip".to_string(),
        ),
    ];
    params.extend(tag_params(&row.tags));
    params
}

/// The creates under test. The tag counts are none, one and several; the key/value
/// shapes are plain text, XML metacharacters and the model's own length bounds
/// (`TagKey` is length 1..128 and `TagValue` 1..256 —
/// `smithy_query(query="shape", shape="Tag")`); the last row is created in another
/// Region, whose ARN the version then carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a version created with no tags",
            platform_name: "untagged-platform",
            platform_version: "1.0.0",
            region: TEST_REGION,
            tags: Vec::new(),
        },
        Row {
            case: "a version created with one tag",
            platform_name: "one-tag-platform",
            platform_version: "1.0.1",
            region: TEST_REGION,
            tags: vec![tag("owner", "platform-team")],
        },
        Row {
            case: "a version created with several tags",
            platform_name: "many-tag-platform",
            platform_version: "2.11.0",
            region: TEST_REGION,
            tags: vec![
                tag("owner", "platform-team"),
                tag("cost-center", "4711"),
                tag("stage", "beta"),
            ],
        },
        Row {
            case: "a tag whose key and value carry XML metacharacters",
            platform_name: "escaped-tag-platform",
            platform_version: "1.0.2",
            region: TEST_REGION,
            tags: vec![tag("a&b<c>", "\"d\" & 'e' <f>")],
        },
        Row {
            case: "a tag at the modelled key and value length bounds",
            platform_name: "long-tag-platform",
            platform_version: "10.0.0",
            region: TEST_REGION,
            tags: vec![
                tag(&"k".repeat(128), &"v".repeat(256)),
                tag("k", "v"),
            ],
        },
        Row {
            case: "a version created in another Region",
            platform_name: "regional-platform",
            platform_version: "3.0.0",
            region: OTHER_REGION,
            tags: vec![tag("region", OTHER_REGION)],
        },
    ]
}

/// The create records the version with the tags it was given, and both the describe
/// and the tag read answer from that one stored record.
#[tokio::test]
async fn created_platform_version_tags_are_reported_for_that_version() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let params = create_params(&row);
        let created = emulator
            .call_in_region(row.region, "CreatePlatformVersion", &as_params(&params))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
            created.status, created.body
        );

        // The ARN the create answered with is the one the account's own resource is
        // named by, in the Region the call was signed for.
        let arn = eb::state::platform_version_arn(
            row.region,
            row.platform_name,
            row.platform_version,
        );
        assert_eq!(
            created.expect("PlatformSummary/PlatformArn", case),
            arn,
            "case {case:?}: unexpected PlatformArn in {}",
            created.body
        );
        assert_eq!(
            created.expect("PlatformSummary/PlatformOwner", case),
            account(),
            "case {case:?}: a custom version is owned by this account, got {}",
            created.body
        );

        // The round trip: the version the create recorded is the one
        // DescribePlatformVersion reports.
        let described = emulator
            .call_in_region(row.region, "DescribePlatformVersion", &[("PlatformArn", &arn)])
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribePlatformVersion, got {} and body {}",
            described.status, described.body
        );
        assert_eq!(
            described.expect("PlatformDescription/PlatformVersion", case),
            row.platform_version,
            "case {case:?}: the version must survive the round trip, got {}",
            described.body
        );

        // And the tags the create applied are the tags that version answers.
        assert_tags(&emulator, case, &arn, &row.tags).await;
    }

    // Each version answers its OWN tags: one created in this emulator does not
    // report another's, and the same name and version under the other Region's ARN
    // names no resource at all.
    let case = "one version's tags are not another's";
    let first = eb::state::platform_version_arn(TEST_REGION, "one-tag-platform", "1.0.1");
    let second = eb::state::platform_version_arn(TEST_REGION, "untagged-platform", "1.0.0");
    assert_tags(&emulator, case, &first, &[tag("owner", "platform-team")]).await;
    assert_tags(&emulator, case, &second, &[]).await;
}

/// The quota the service publishes is how many tags one resource can hold, so a create
/// that applies exactly that many stores every one of them.
#[tokio::test]
async fn a_create_at_the_published_tag_quota_stores_every_tag() {
    let emulator = Emulator::new();
    let case = "tags at the published per-resource quota";

    // The bound is the service's own (`crate::state::MAX_TAGS_PER_RESOURCE`, what
    // UpdateTagsForResource refuses a tag set past), read from the constant so this
    // case cannot drift from it.
    let tags: Vec<(String, String)> = (0..eb::state::MAX_TAGS_PER_RESOURCE)
        .map(|index| tag(&format!("key-{index}"), &format!("value-{index}")))
        .collect();
    let row = Row {
        case,
        platform_name: "quota-tagged-platform",
        platform_version: "1.0.0",
        region: TEST_REGION,
        tags,
    };
    let params = create_params(&row);
    let created = emulator
        .call_in_region(row.region, "CreatePlatformVersion", &as_params(&params))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );

    let arn = eb::state::platform_version_arn(row.region, row.platform_name, row.platform_version);
    assert_tags(&emulator, case, &arn, &row.tags).await;
}

/// `UpdateTagsForResource` writes through to the stored version: a key it adds, a key
/// it overwrites and a key it removes are all visible to the next read, and the
/// version itself is otherwise untouched.
#[tokio::test]
async fn a_created_platform_versions_tags_can_be_updated() {
    let emulator = Emulator::new();
    let case = "update the tags of a created platform version";

    let row = Row {
        case,
        platform_name: "mutable-tag-platform",
        platform_version: "1.2.3",
        region: TEST_REGION,
        tags: vec![tag("owner", "platform-team"), tag("stage", "beta")],
    };
    let params = create_params(&row);
    let created = emulator
        .call_in_region(row.region, "CreatePlatformVersion", &as_params(&params))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );
    let arn = eb::state::platform_version_arn(row.region, row.platform_name, row.platform_version);

    let updated = emulator
        .call(
            "UpdateTagsForResource",
            &[
                ("ResourceArn", arn.as_str()),
                ("TagsToAdd.member.1.Key", "stage"),
                ("TagsToAdd.member.1.Value", "supported"),
                ("TagsToAdd.member.2.Key", "cost-center"),
                ("TagsToAdd.member.2.Value", "4711"),
                ("TagsToRemove.member.1", "owner"),
            ],
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: expected 200 from UpdateTagsForResource, got {} and body {}",
        updated.status, updated.body
    );
    assert_tags(
        &emulator,
        case,
        &arn,
        &[tag("stage", "supported"), tag("cost-center", "4711")],
    )
    .await;

    // The version is still the resource it was: the build the clock owed completes
    // on its own schedule, and tagging neither replaced the record nor held it back.
    std::thread::sleep(PAST_DUE);
    let described = emulator
        .call("DescribePlatformVersion", &[("PlatformArn", arn.as_str())])
        .await;
    assert_eq!(
        described.expect("PlatformDescription/PlatformStatus", case),
        "Ready",
        "case {case:?}: the build the clock owed must have completed, got {}",
        described.body
    );
    assert_eq!(
        described.expect("PlatformDescription/PlatformName", case),
        row.platform_name,
        "case {case:?}: the stored version must be unchanged by the tag write, got {}",
        described.body
    );
}

/// The tags a create applies land on the version and NOWHERE else: "Elastic
/// Beanstalk applies these tags only to the platform version. Environments that you
/// create using the platform version don't inherit the tags"
/// (`CreatePlatformVersion$Tags`).
#[tokio::test]
async fn an_environment_launched_on_a_tagged_version_does_not_inherit_its_tags() {
    let emulator = Emulator::new();
    let case = "an environment launched on a tagged custom platform version";

    let application = emulator
        .call("CreateApplication", &[("ApplicationName", "platform-app")])
        .await;
    assert_eq!(
        application.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        application.status, application.body
    );

    let row = Row {
        case,
        platform_name: "inherited-tag-platform",
        platform_version: "4.0.0",
        region: TEST_REGION,
        tags: vec![tag("scope", "platform-version")],
    };
    let params = create_params(&row);
    let created = emulator
        .call_in_region(row.region, "CreatePlatformVersion", &as_params(&params))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );
    let arn = eb::state::platform_version_arn(row.region, row.platform_name, row.platform_version);

    // The version is built before an environment can be launched on it, so wait
    // until the move to `Ready` the clock owes is due.
    std::thread::sleep(PAST_DUE);
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "platform-app"),
                ("EnvironmentName", "inheritance-env"),
                ("PlatformArn", arn.as_str()),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );
    let environment_arn = launched.expect("EnvironmentArn", case).to_string();

    assert_tags(&emulator, case, &environment_arn, &[]).await;
    // And the version still answers its own.
    assert_tags(&emulator, case, &arn, &row.tags).await;
}

/// A platform ARN that names no version this account created is not a taggable
/// resource: the type IS one Elastic Beanstalk tags, so what is missing is the
/// resource.
#[tokio::test]
async fn a_platform_arn_naming_no_created_version_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreatePlatformVersion",
            &[
                ("PlatformName", "known-platform"),
                ("PlatformVersion", "1.0.0"),
                ("PlatformDefinitionBundle.S3Bucket", "platform-bundles"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "seeding a platform version: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );

    // A managed version is the SERVICE's own resource — `PlatformOwner` is
    // `eb::state::MANAGED_PLATFORM_OWNER`, not this account — so this account holds
    // no tags for it either. The catalogue entry is read from the published table
    // rather than written out here.
    let managed = eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack")
        .platform_arn(TEST_REGION);
    let rows: Vec<(&str, String)> = vec![
        (
            "a platform name nothing created",
            eb::state::platform_version_arn(TEST_REGION, "ghost-platform", "1.0.0"),
        ),
        (
            "a version the created platform does not have",
            eb::state::platform_version_arn(TEST_REGION, "known-platform", "9.9.9"),
        ),
        (
            "the created version's name under another Region's ARN",
            eb::state::platform_version_arn(OTHER_REGION, "known-platform", "1.0.0"),
        ),
        (
            "a platform ARN with no version segment",
            eb::state::platform_version_arn(TEST_REGION, "known-platform", ""),
        ),
        ("a managed version this account did not create", managed),
    ];

    for (case, arn) in &rows {
        for action in ["ListTagsForResource", "UpdateTagsForResource"] {
            let mut params = vec![("ResourceArn", arn.as_str())];
            if action == "UpdateTagsForResource" {
                params.push(("TagsToAdd.member.1.Key", "owner"));
                params.push(("TagsToAdd.member.1.Value", "platform-team"));
            }
            let refused = emulator.call(action, &params).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400 from {action} for {arn:?}, got {} and body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException from {action}, got body {}",
                refused.body
            );
        }
    }

    // The positive control beside them: the version that WAS created answers 200 with
    // the tags it carries, which is none.
    assert_tags(
        &emulator,
        "the created version",
        &eb::state::platform_version_arn(TEST_REGION, "known-platform", "1.0.0"),
        &[],
    )
    .await;
}
