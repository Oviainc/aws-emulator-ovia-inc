// SPDX-License-Identifier: CC0-1.0

//! The DEPRECATED images of the published AMI catalog: how old they are, when they were
//! deprecated, and what a name-filtered `DescribeImages` does with them — driven over the
//! wire the service speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST
//! answered with XML; README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: AN IMAGE THE CATALOG PUBLISHES AS DEPRECATED IS DEPRECATED WHENEVER
//! IT IS READ, AND OF THE AGE ITS PUBLISHED BUILD DATE GIVES IT. A published AMI family is
//! republished on a rolling schedule and AWS's own dates move with it, so the catalog's
//! dates are computed from the instant the catalog is seeded rather than frozen: the
//! `DeprecationTime` of such an image has always already passed ("the date and time to
//! deprecate the AMI", `Image$DeprecationTime`), its `CreationDate` is of the age AWS
//! publishes a build of a family whose support has ended at, and the build date its
//! published name carries is the date it reports. That is what makes the contrast a
//! deprecation audit reads — the image a `DescribeImages` leaves out and
//! `IncludeDeprecated` brings back — available however long this emulator has been running.
//!
//! The rows span what that generalises over: every deprecated image of the catalog (each a
//! different build of a different age), both answers to `IncludeDeprecated`, and a
//! name-filtered read beside an unfiltered one — with the undeprecated catalog images as
//! the contrast that the rule is about deprecation rather than about the catalog.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, filters, indexed};
use super::wire::{child, child_text, document, Param};

/// The Region every call here is signed for; the image catalog is per account.
const REGION: &str = "us-east-1";

/// The name pattern a client resolving an Amazon Linux 2 x86_64 gp2 build matches on —
/// Amazon's own published name form for that family, `amzn2-ami-hvm-<version>-<architecture>-<root volume>`.
const AL2_NAME_PATTERN: &str = "amzn2-ami-hvm-2.0.*-x86_64-gp2";

/// The seconds in a day, for reading an age off a reported `CreationDate`.
const SECONDS_PER_DAY: f64 = 24.0 * 60.0 * 60.0;

/// The window a deprecated build of a family whose standard support has ended falls in:
/// AWS publishes such images roughly a year before that support ended, so an audit that
/// asks "how long ago was this deprecated build published?" has an answer of a realistic
/// age. The bounds are generous — the rule is the ORDER of the instants, not a particular
/// age — and both are asserted, so a build cannot drift out of the window.
const YOUNGEST_DEPRECATED_BUILD_DAYS: f64 = 300.0;

/// The far end of that window.
const OLDEST_DEPRECATED_BUILD_DAYS: f64 = 540.0;

/// One image row of a response: the members this suite reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Image {
    image_id: String,
    name: String,
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
            creation_date: child_text(node, "creationDate").unwrap_or_default(),
            deprecation_time: child_text(node, "deprecationTime"),
        })
        .collect()
}

/// One `DescribeImages` call that must succeed, as the images it reported.
async fn described(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<Image> {
    let (status, body) = call(router, REGION, "DescribeImages", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImages, got {status} with body {body}"
    );
    images(case, &body)
}

/// `timestamp` as the days between it and now — positive for an instant in the past.
fn days_ago(case: &str, timestamp: &str) -> f64 {
    let at = as_timestamp(timestamp)
        .unwrap_or_else(|| panic!("case {case:?}: {timestamp:?} must be an ISO 8601 instant"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    (now - at) / SECONDS_PER_DAY
}

/// Every deprecated image of the catalog is already deprecated, was published before it
/// was, is of the published age, and carries that build date in its name.
#[tokio::test]
async fn every_deprecated_catalog_image_is_deprecated_and_of_a_realistic_age() {
    let (router, _engine) = super::ec2_query::emulator();

    let deprecated: Vec<ec2::state::ImageSpec> = ec2::state::published_image_specs()
        .into_iter()
        .filter(|spec| spec.deprecation_time.is_some())
        .collect();
    assert!(
        !deprecated.is_empty(),
        "the catalog must publish at least one deprecated image for an audit to read"
    );

    // Rows: every deprecated image of the catalog, each a build of its own age.
    for spec in &deprecated {
        let case = spec.name.clone();
        let reported = described(
            &case,
            &router,
            &{
                let mut params = filters(&[("name", &[spec.name.as_str()])]);
                params.push(super::wire::param("IncludeDeprecated", "true"));
                params
            },
        )
        .await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the catalog must hold exactly that image, got {reported:?}"
        );
        let image = &reported[0];
        let deprecation_time = image
            .deprecation_time
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: a deprecated image reports when: {image:?}"));
        let deprecated_days_ago = days_ago(&case, &deprecation_time);
        assert!(
            deprecated_days_ago > 0.0,
            "case {case:?}: a deprecated image's deprecation time must have passed, \
             got {deprecation_time:?} ({deprecated_days_ago} days ago)"
        );
        let created_days_ago = days_ago(&case, &image.creation_date);
        assert!(
            created_days_ago > deprecated_days_ago,
            "case {case:?}: an image cannot be published after it was deprecated, got \
             created {created_days_ago} days ago and deprecated {deprecated_days_ago} \
             days ago"
        );
        assert!(
            (YOUNGEST_DEPRECATED_BUILD_DAYS..=OLDEST_DEPRECATED_BUILD_DAYS)
                .contains(&created_days_ago),
            "case {case:?}: expected a build published between \
             {YOUNGEST_DEPRECATED_BUILD_DAYS} and {OLDEST_DEPRECATED_BUILD_DAYS} days \
             ago, got {created_days_ago} ({:?})",
            image.creation_date
        );
        // The build date the published name carries IS the date the image reports: an
        // Amazon Linux name is `amzn2-ami-hvm-2.0.<YYYYMMDD>.<n>-…`.
        let build_date: String = image
            .creation_date
            .chars()
            .take(10)
            .filter(char::is_ascii_digit)
            .collect();
        assert!(
            image.name.contains(&build_date),
            "case {case:?}: the published name must carry the build date {build_date} it \
             reports, got {:?}",
            image.name
        );
    }

    // At least one of them is the Amazon Linux 2 x86_64 gp2 build a client resolves by
    // that family's published name pattern.
    let matching = described(
        "the AL2 gp2 pattern",
        &router,
        &{
            let mut params = filters(&[("name", &[AL2_NAME_PATTERN])]);
            params.push(super::wire::param("IncludeDeprecated", "true"));
            params.extend(indexed("Owner", &["amazon"]));
            params
        },
    )
    .await;
    assert!(
        !matching.is_empty(),
        "an audit must find the deprecated Amazon Linux 2 builds under the family's own \
         name pattern, got {matching:?}"
    );
    for spec in &deprecated {
        let name = &spec.name;
        assert!(
            matching
                .iter()
                .any(|image| &image.name == name && image.deprecation_time.is_some()),
            "case {name:?}: the family's own name pattern must report that build as one of \
             the deprecated ones, got {matching:?}"
        );
    }
}

/// The contrast an audit reads: a name-filtered read leaves the deprecated builds out, and
/// the same read with `IncludeDeprecated` brings exactly them back.
#[tokio::test]
async fn a_name_filtered_read_hides_the_deprecated_builds_unless_asked_for() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the family's own name pattern and a pattern matching every published image, so
    // the contrast is not a property of one pattern; each read twice, with and without the
    // flag.
    for pattern in [AL2_NAME_PATTERN, "*"] {
        for include in [false, true] {
            let case = format!("name={pattern} IncludeDeprecated={include}");
            let mut params = filters(&[("name", &[pattern])]);
            if include {
                params.push(super::wire::param("IncludeDeprecated", "true"));
            }
            let reported = described(&case, &router, &params).await;
            let deprecated: Vec<&Image> = reported
                .iter()
                .filter(|image| image.deprecation_time.is_some())
                .collect();
            assert_eq!(
                !deprecated.is_empty(),
                include,
                "case {case:?}: a deprecated build is reported exactly when the request \
                 asks for one, got {deprecated:?}"
            );
            if pattern == AL2_NAME_PATTERN {
                // The family's own current build is published whatever the request asks
                // about deprecated ones: the flag adds the deprecated builds beside it
                // rather than deciding whether the family is reported at all.
                assert!(
                    reported
                        .iter()
                        .any(|image| image.deprecation_time.is_none()),
                    "case {case:?}: the family's current build must be reported whatever \
                     the request asks about deprecated ones, got {reported:?}"
                );
            } else {
                assert!(
                    reported.len() > deprecated.len(),
                    "case {case:?}: the catalog also holds images that are not \
                     deprecated, got {reported:?}"
                );
            }
        }
    }
}
