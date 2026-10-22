// SPDX-License-Identifier: CC0-1.0

//! The setup path an Auto Scaling workflow walks before it touches autoscaling at all:
//! DISCOVER an image id, then USE it.
//!
//! `DescribeImages`' own selectors — its filters, its owner scoping, its paging — are the
//! subject of `tests/all/describe_images.rs`. What this suite asserts is the cross-operation
//! property the issue is about: every id `DescribeImages` reports is an id the operations
//! that take an `ImageId` accept, so a client that cannot hardcode an AMI can find one and
//! get on with the workflow. The rows are a TABLE over how the image was DISCOVERED —
//! unfiltered, by owner, by name pattern, by Systems Manager alias — because the rule is
//! about every reported id and not about one recorded lookup.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, filters, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image ids one `DescribeImages` call reports, asserting it succeeded.
async fn discovered(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeImages", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "imageId")
}

/// One `CreateLaunchTemplate` call naming `image_id` as the version's `ImageId`.
async fn create_launch_template(
    router: &MultiServiceRouter,
    name: &str,
    image_id: &str,
) -> (u16, String) {
    let params = vec![
        ("LaunchTemplateName".to_string(), name.to_string()),
        (
            "LaunchTemplateData.ImageId".to_string(),
            image_id.to_string(),
        ),
        (
            "LaunchTemplateData.InstanceType".to_string(),
            "t3.micro".to_string(),
        ),
    ];
    super::ec2_query::call(router, REGION, "CreateLaunchTemplate", &params).await
}

/// A fresh account can DISCOVER an image id at all — nothing in the API creates the public
/// catalog, so an account that has registered no image of its own still sees one.
#[tokio::test]
async fn a_fresh_account_can_discover_an_image_id() {
    let (router, engine) = super::ec2_query::emulator();

    let reported = discovered(&router, "a fresh account", &[]).await;
    assert!(
        !reported.is_empty(),
        "a fresh account must be able to discover an image id, got {reported:?}"
    );
    for image_id in &reported {
        assert!(
            ec2::state::image_id_is_well_formed(image_id),
            "the reported id {image_id:?} must be a well-formed AMI id"
        );
    }
    assert_eq!(
        reported.len(),
        engine
            .state::<Ec2State>()
            .lock()
            .images
            .values()
            .filter(|image| !ec2::state::image_is_deprecated(image, std::time::SystemTime::now()))
            .count(),
        "the report must be the store's current images, got {reported:?}"
    );
}

/// However a client NARROWS the lookup, the id it gets back is one `CreateLaunchTemplate`
/// accepts as its `ImageId` — and the template stores exactly that id.
#[tokio::test]
async fn an_image_id_discovered_any_way_is_accepted_as_a_launch_template_image_id() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: how the lookup was narrowed. Each is a way a client that cannot hardcode an
    // AMI finds one — the unfiltered report, the Amazon-owned catalog, a name pattern, and
    // the public Systems Manager parameter the AMI is published under.
    let lookups: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no narrowing at all", Vec::new()),
        ("Owners=[amazon]", indexed("Owner", &["amazon"])),
        (
            "a name pattern",
            filters(&[("name", &["al2023-*"]), ("state", &["available"])]),
        ),
        (
            "the architecture",
            filters(&[("architecture", &["x86_64"])]),
        ),
        (
            "the public Systems Manager parameter",
            filters(&[(
                "public-ssm-parameter-name",
                &["/aws/service/ami-amazon-linux-latest/*"],
            )]),
        ),
    ];

    for (index, (case, params)) in lookups.into_iter().enumerate() {
        let reported = discovered(&router, case, &params).await;
        let image_id = reported.first().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: expected the lookup to discover an image, got {reported:?}")
        });

        let name = format!("asg-template-{index}");
        let (status, body) = create_launch_template(&router, &name, &image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a discovered image id must be accepted as an ImageId, got \
             {status} with body {body}"
        );
        let stored = element(&body, "launchTemplateId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a launchTemplateId in {body}")
        });
        assert!(
            stored.starts_with("lt-"),
            "case {case:?}: expected a minted launch template id, got {stored:?}"
        );
    }
}
