// SPDX-License-Identifier: CC0-1.0

//! The custom platform versions an account holds, as `ListPlatformVersions`
//! selects them and as the quota refuses one more.
//!
//! RULE 1 (the owner alias): `PlatformOwner` is "the Amazon Web Services account
//! ID of the person who created the platform version", and a filter term on it
//! may spell the CALLING account either as that id or by the alias `self` (the
//! spelling the Elastic Beanstalk documentation lists custom platform versions
//! with) — so both select exactly the custom versions this account created in the
//! Region the request was signed for, and neither selects the versions the
//! service itself publishes.
//!
//! RULE 2 (the quota refusal): a create past the published custom-platform quota
//! is refused with the modelled `TooManyPlatformsException` (HTTP 400) and that
//! error carries NO message member at all.
//!
//! The rows span what rule 1 generalises over: the spelling of the alias
//! (lower case, mixed case, upper case), the account id itself, the owner the
//! published catalogue reports, an account that is not this one, and the operator
//! the term applies (`=` and `!=`). Everything is driven over HTTP through the
//! shared harness, so what is asserted is what a client sees.

use crate::lifecycle::{account, Emulator, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// Another Region, so a term asking for this account's versions is asserted to
/// answer only the ones that live where the request was signed for.
const OTHER_REGION: &str = "eu-west-1";

/// The custom platform versions this suite creates in [`TEST_REGION`], as
/// `(PlatformName, PlatformVersion)`.
const OWN_VERSIONS: &[(&str, &str)] = &[
    ("owner-filter-platform", "1.0.0"),
    ("owner-filter-platform", "1.1.0"),
    ("another-owner-platform", "2.0.0"),
];

/// Create the custom platform version `(name, version)` in `region` and answer the
/// `PlatformArn` the create reported.
async fn create(emulator: &Emulator, case: &str, region: &str, name: &str, version: &str) -> String {
    let created = emulator
        .call_in_region(
            region,
            "CreatePlatformVersion",
            &[
                ("PlatformName", name),
                ("PlatformVersion", version),
                ("PlatformDefinitionBundle.S3Bucket", "platform-bundles"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );
    created
        .expect("PlatformSummary/PlatformArn", case)
        .to_string()
}

/// Every `PlatformArn` `ListPlatformVersions` answers in `region` for one
/// `PlatformOwner` term.
async fn listed_arns(
    emulator: &Emulator,
    case: &str,
    region: &str,
    operator: &str,
    value: &str,
) -> Vec<String> {
    let listed = emulator
        .call_in_region(
            region,
            "ListPlatformVersions",
            &[
                ("Filters.member.1.Type", "PlatformOwner"),
                ("Filters.member.1.Operator", operator),
                ("Filters.member.1.Values.member.1", value),
            ],
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListPlatformVersions, got {} and body {}",
        listed.status, listed.body
    );
    listed
        .values_at("PlatformSummaryList/member/PlatformArn")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// One `PlatformOwner` term under test: the value it carries and whether the
/// versions this account created must be the ones it selects.
struct OwnerCase {
    case: &'static str,
    value: String,
    /// Whether a `=` term on this value selects this account's own versions.
    selects_own: bool,
}

#[tokio::test]
async fn the_platform_owner_filter_selects_this_accounts_own_versions() {
    let emulator = Emulator::new();

    let mut own: Vec<String> = Vec::new();
    for (name, version) in OWN_VERSIONS {
        own.push(create(&emulator, "setup", TEST_REGION, name, version).await);
    }
    own.sort();
    // A version of this account's in ANOTHER Region: a platform version is a
    // regional resource, so a term asking for this account's versions here must
    // not answer it.
    let elsewhere = create(
        &emulator,
        "setup",
        OTHER_REGION,
        "elsewhere-platform",
        "1.0.0",
    )
    .await;

    let cases = [
        OwnerCase {
            case: "the alias in the documented spelling",
            value: "self".to_string(),
            selects_own: true,
        },
        OwnerCase {
            case: "the alias in mixed case",
            value: "Self".to_string(),
            selects_own: true,
        },
        OwnerCase {
            case: "the alias in upper case",
            value: "SELF".to_string(),
            selects_own: true,
        },
        OwnerCase {
            case: "this account's own id",
            value: account().to_string(),
            selects_own: true,
        },
        OwnerCase {
            case: "the owner the published catalogue reports",
            value: eb::state::MANAGED_PLATFORM_OWNER.to_string(),
            selects_own: false,
        },
        OwnerCase {
            case: "an account that is not this one",
            value: "123456789012".to_string(),
            selects_own: false,
        },
    ];

    for OwnerCase {
        case,
        value,
        selects_own,
    } in &cases
    {
        let selected = listed_arns(&emulator, case, TEST_REGION, "=", value).await;
        if *selects_own {
            let mut sorted = selected.clone();
            sorted.sort();
            assert_eq!(
                sorted, own,
                "case {case:?}: expected this account's own versions {own:?}, got {sorted:?}"
            );
            assert!(
                !selected.contains(&elsewhere),
                "case {case:?}: a version created in {OTHER_REGION} must not be listed in \
                 {TEST_REGION}, got {selected:?}"
            );
        } else {
            for arn in &own {
                assert!(
                    !selected.contains(arn),
                    "case {case:?}: expected none of this account's own versions, got {selected:?}"
                );
            }
        }

        // The `!=` term is the negation of the same test over the same attribute,
        // so what one selects the other leaves out.
        let excluded = listed_arns(&emulator, case, TEST_REGION, "!=", value).await;
        for arn in &selected {
            assert!(
                !excluded.contains(arn),
                "case {case:?}: {arn} was selected by both the = and the != term, got {excluded:?}"
            );
        }
        if *selects_own {
            for arn in &own {
                assert!(
                    !excluded.contains(arn),
                    "case {case:?}: expected the != term to leave this account's versions out, \
                     got {excluded:?}"
                );
            }
        }
    }

    // The alias names the CALLING account in the Region the request was signed
    // for too, so the version created elsewhere is what it selects there.
    let case = "the alias in another Region";
    let selected = listed_arns(&emulator, case, OTHER_REGION, "=", "self").await;
    assert_eq!(
        selected,
        vec![elsewhere.clone()],
        "case {case:?}: expected only {elsewhere}, got {selected:?}"
    );
}

/// The ARNs `ListPlatformVersions` answers for this account's own versions must
/// be the very ARNs the creates reported — the round trip the alias is used for.
#[tokio::test]
async fn a_created_version_is_listable_and_taggable_under_the_owner_alias() {
    let emulator = Emulator::new();
    let case = "a created version under the alias";
    let arn = create(&emulator, case, TEST_REGION, "round-trip-platform", "1.2.3").await;

    let listed = listed_arns(&emulator, case, TEST_REGION, "=", "self").await;
    assert_eq!(
        listed,
        vec![arn.clone()],
        "case {case:?}: expected the created version to be listable, got {listed:?}"
    );

    // The very ARN the create answered with is a resource of this account, so the
    // tag read resolves it rather than refusing it.
    let tags = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn.as_str())])
        .await;
    assert_eq!(
        tags.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} and body {}",
        tags.status, tags.body
    );
    assert_eq!(
        tags.expect("ResourceArn", case),
        arn,
        "case {case:?}: the tag read must answer for the ARN it was asked about, got {}",
        tags.body
    );
}

/// RULE 2: the quota refusal carries the modelled code and status, and no message.
#[tokio::test]
async fn the_quota_refusal_carries_no_message_member() {
    let emulator = Emulator::new();
    let case = "one version past the published quota";

    for index in 0..eb::state::MAX_PLATFORM_VERSIONS {
        let version = format!("1.0.{index}");
        create(&emulator, case, TEST_REGION, "quota-platform", &version).await;
    }
    // The account is at its quota, which is also what `DescribeAccountAttributes`
    // publishes as the limit, so the next create is the one that is refused.
    let attributes = emulator.call("DescribeAccountAttributes", &[]).await;
    assert_eq!(
        attributes.expect("ResourceQuotas/CustomPlatformQuota/Maximum", case),
        eb::state::MAX_PLATFORM_VERSIONS.to_string(),
        "case {case:?}: the quota under test must be the published one, got {}",
        attributes.body
    );

    let refused = emulator
        .call(
            "CreatePlatformVersion",
            &[
                ("PlatformName", "quota-platform"),
                ("PlatformVersion", "2.0.0"),
                ("PlatformDefinitionBundle.S3Bucket", "platform-bundles"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TooManyPlatformsException",
        "case {case:?}: expected the modelled quota error, got body {}",
        refused.body
    );
    assert!(
        refused.value("Error/Message").is_none(),
        "case {case:?}: the quota refusal must carry no Message element, got body {}",
        refused.body
    );

    // Nothing was recorded for the refused create: the account still holds exactly
    // the versions the quota admits.
    let listed = listed_arns(&emulator, case, TEST_REGION, "=", "self").await;
    assert_eq!(
        listed.len(),
        eb::state::MAX_PLATFORM_VERSIONS,
        "case {case:?}: expected {} versions after the refusal, got {}",
        eb::state::MAX_PLATFORM_VERSIONS,
        listed.len()
    );
}
