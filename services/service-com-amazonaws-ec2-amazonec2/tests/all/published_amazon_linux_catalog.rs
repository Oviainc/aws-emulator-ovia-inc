// SPDX-License-Identifier: CC0-1.0

//! Resolving a launch AMI out of the published catalog the documented way, over the wire the
//! service speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with
//! XML; README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: EVERY VARIANT AMAZON PUBLISHES UNDER THE `amazon` OWNER ALIAS HAS A
//! CURRENT BUILD IN THE CATALOG — one that is `available` and not deprecated — so a
//! `DescribeImages` narrowed to `Owners=amazon`, that variant's published name pattern and
//! `state=available` reports at least one image, each carrying the `ImageId` and the
//! `CreationDate` a caller sorts on, and the newest match launches. A variant whose only
//! builds are deprecated is a catalog that cannot be launched from: the operation models no
//! errors, so an empty set is the only way such a request can fail, and it fails silently.
//!
//! The rows span what that generalises over: the Amazon Linux 2 standard variant in both
//! architectures Amazon publishes it for, its kernel-pinned and minimal variants, both
//! Amazon Linux 2023 variants, and the Windows Server base image — each by the name pattern
//! a client writes rather than by an AMI id, which is per-Region and per-release. The second
//! test takes the rows from the catalog itself, so a variant added later is covered without
//! touching the table.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, elements, filters, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for; the image catalog is per account.
const REGION: &str = "us-east-1";

/// The owner alias Amazon's own published images carry (`Image$ImageOwnerAlias`), which is
/// also what a caller asks for in `Owners`.
const AMAZON: &str = "amazon";

/// One published variant to resolve: the name pattern a client matches on, and the
/// architecture that pattern names.
///
/// The patterns are the published name forms of the variants Amazon publishes a parameter
/// for under `/aws/service/ami-amazon-linux-latest/` and `/aws/service/ami-windows-latest/`
/// (Amazon EC2 User Guide, "Use a Systems Manager parameter instead of an AMI ID", and the
/// Amazon Linux 2023 User Guide's "Find an AMI" name forms
/// `amzn2-ami[-kernel-<kernel version>][-minimal]-hvm-<version>-<architecture>-<root volume>`
/// and `al2023-ami[-minimal]-<version>-kernel-<kernel version>-<architecture>`) — not the
/// ids, names or dates of any one release, which are per-Region, per-release values.
const PUBLISHED_VARIANTS: &[(&str, &str)] = &[
    ("amzn2-ami-hvm-*-x86_64-gp2", "x86_64"),
    ("amzn2-ami-hvm-*-arm64-gp2", "arm64"),
    ("amzn2-ami-kernel-5.10-hvm-*-x86_64-gp2", "x86_64"),
    ("amzn2-ami-minimal-hvm-*-x86_64-ebs", "x86_64"),
    ("al2023-ami-2023.*-kernel-6.1-x86_64", "x86_64"),
    ("al2023-ami-minimal-*-kernel-6.1-arm64", "arm64"),
    ("Windows_Server-2022-*", "x86_64"),
];

/// One image row of a response: the members this suite reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Image {
    image_id: String,
    name: String,
    state: String,
    architecture: String,
    owner_alias: String,
    public: String,
    creation_date: String,
    deprecation_time: Option<String>,
}

/// Every image a `DescribeImages` body reports.
fn images(case: &str, body: &str) -> Vec<Image> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "imageId").is_some())
        .map(|node| Image {
            image_id: child_text(node, "imageId").unwrap_or_default(),
            name: child_text(node, "name").unwrap_or_default(),
            state: child_text(node, "imageState").unwrap_or_default(),
            architecture: child_text(node, "architecture").unwrap_or_default(),
            owner_alias: child_text(node, "imageOwnerAlias").unwrap_or_default(),
            public: child_text(node, "isPublic").unwrap_or_default(),
            creation_date: child_text(node, "creationDate").unwrap_or_default(),
            deprecation_time: child_text(node, "deprecationTime"),
        })
        .collect()
}

/// The images `DescribeImages` reports for `params`, which must succeed.
async fn described(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<Image> {
    let (status, body) = call(router, REGION, "DescribeImages", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImages, got {status} with body {body}"
    );
    images(case, &body)
}

/// The request a client resolving an AMI by name sends: the `amazon` owner, a name pattern
/// and the `available` state.
fn resolve_by_name(pattern: &str) -> Vec<Param> {
    let mut params = indexed("Owner", &[AMAZON]);
    params.extend(filters(&[
        ("name", &[pattern]),
        ("state", &["available"]),
    ]));
    params
}

/// Launch `image_id` and report the `imageId` the instance is described with — the proof
/// that the id the catalog resolved to is an AMI this account can actually run.
async fn launch_and_read_back(case: &str, router: &MultiServiceRouter, image_id: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "RunInstances",
        &[
            param("ImageId", image_id),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances with the resolved AMI must succeed, got {status} with \
         body {body}"
    );
    let instance_id = elements(&body, "instanceId")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must mint an instance: {body}"));

    let (described_status, described_body) = call(
        router,
        REGION,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id.as_str()]),
    )
    .await;
    assert_eq!(
        described_status, 200,
        "case {case:?}: DescribeInstances must succeed, got {described_status} with body \
         {described_body}"
    );
    elements(&described_body, "imageId")
        .first()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the instance must report the AMI it ran: {described_body}")
        })
}

/// The name pattern a client writes for the variant `name` is a build of: the published
/// name with every version token — the ones carrying a `.`, which is where a release
/// number lives — replaced by EC2's `*` wildcard, so one pattern matches every build of
/// that variant whatever release it comes from.
fn variant_pattern(name: &str) -> String {
    name.split('-')
        .map(|token| {
            if token.contains('.') {
                "*"
            } else {
                token
            }
        })
        .collect::<Vec<&str>>()
        .join("-")
}

/// Every published variant resolves to at least one Amazon-owned, available, undeprecated
/// image with an id and a creation date, and the newest match launches.
#[tokio::test]
async fn every_published_variant_resolves_to_a_launchable_available_image() {
    let (router, _engine) = super::ec2_query::emulator();

    for (pattern, architecture) in PUBLISHED_VARIANTS {
        let case = (*pattern).to_string();
        let reported = described(&case, &router, &resolve_by_name(pattern)).await;
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected at least one Amazon-published available image, got an \
             empty set"
        );

        for image in &reported {
            assert!(
                image.image_id.starts_with("ami-"),
                "case {case:?}: every reported image carries an AMI id, got {image:?}"
            );
            assert_eq!(
                image.state, "available",
                "case {case:?}: the state filter must hold, got {image:?}"
            );
            assert_eq!(
                image.owner_alias, AMAZON,
                "case {case:?}: the owner filter must hold, got {image:?}"
            );
            assert_eq!(
                image.public, "true",
                "case {case:?}: a published image is public, got {image:?}"
            );
            assert_eq!(
                &image.architecture, architecture,
                "case {case:?}: the pattern names that architecture, got {image:?}"
            );
            assert_eq!(
                image.deprecation_time, None,
                "case {case:?}: a request that does not ask for deprecated images gets \
                 none, got {image:?}"
            );
            assert!(
                as_timestamp(&image.creation_date).is_some(),
                "case {case:?}: the creation date a caller sorts on must be an instant, got \
                 {image:?}"
            );
        }

        // The documented way to pick one: the newest `CreationDate` of the matches.
        let newest = reported
            .iter()
            .max_by(|left, right| left.creation_date.cmp(&right.creation_date))
            .expect("a non-empty set has a newest image");
        let ran = launch_and_read_back(&case, &router, &newest.image_id).await;
        assert_eq!(
            ran, newest.image_id,
            "case {case:?}: the instance must report the AMI it was launched from, got \
             {ran:?}"
        );
    }
}

/// The same rule read off the catalog itself rather than off the table above: every build
/// the store publishes as current is reported by a read of its own variant's name pattern,
/// so no published variant is left with deprecated builds only.
#[tokio::test]
async fn every_current_catalog_build_is_reported_under_its_own_name_pattern() {
    let (router, _engine) = super::ec2_query::emulator();

    let current: Vec<ec2::state::ImageSpec> = ec2::state::published_image_specs()
        .into_iter()
        .filter(|spec| spec.deprecation_time.is_none())
        .collect();
    assert!(
        !current.is_empty(),
        "the catalog must publish at least one current build to launch from"
    );

    for spec in &current {
        let case = spec.name.clone();
        let pattern = variant_pattern(&spec.name);
        let reported = described(&case, &router, &resolve_by_name(&pattern)).await;
        assert!(
            reported.iter().any(|image| image.name == spec.name),
            "case {case:?}: the pattern {pattern:?} its own published name gives must \
             report it as an available image, got {reported:?}"
        );
    }
}
