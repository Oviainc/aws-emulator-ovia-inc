// SPDX-License-Identifier: CC0-1.0

//! What a `resolve:ssm:` `ImageId` RESOLVES TO: the public Systems Manager parameter
//! catalog `DescribeLaunchTemplateVersions` reads when the request asks for the alias to be
//! resolved, driven over the ec2Query wire the service speaks (README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! "If true, and if a Systems Manager parameter is specified for ImageId, the AMI ID is
//! displayed in the response for imageId. If false, and if a Systems Manager parameter is
//! specified for ImageId, the parameter is displayed in the response for imageId. Default:
//! false" (`DescribeLaunchTemplateVersionsRequest$ResolveAlias`). So one stored template
//! has TWO answers to one call shape, and the resolved one is an AMI id of the account's
//! visible image catalog — reached through the parameter NAME, whichever of the names the
//! catalog publishes for that image the template stored.
//!
//! The rows are the whole catalog: every public parameter name the image store resolves,
//! read out of the store rather than written here, and the names an image is published
//! under beside its `Image$PublicSsmParameterName`
//! (`ec2::state::PUBLIC_SSM_PARAMETER_ALIASES`) — which must resolve to the same AMI as the
//! name they accompany. `launch_templates` owns the rest of this operation's cover; this
//! suite owns the catalog the resolution reads.

use ec2::ops::describe_launch_template_versions::SSM_IMAGE_ALIAS_PREFIX;
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, PUBLIC_SSM_PARAMETER_ALIASES};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, indexed};

/// The Region every call here is signed for. A launch template version carries no Region
/// member, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `(key, value)` request parameter.
fn param(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Create a launch template named `name` whose `ImageId` is `image_id`, and answer its id.
async fn create_template(
    case: &str,
    router: &MultiServiceRouter,
    name: &str,
    image_id: &str,
) -> String {
    let params = vec![
        param("LaunchTemplateName", name),
        param("LaunchTemplateData.ImageId", image_id),
        param("LaunchTemplateData.InstanceType", "t3.micro"),
    ];
    let (status, body) = call(router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateLaunchTemplate, got {status} with body \
         {body}"
    );
    element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: no <launchTemplateId> in {body}"))
}

/// The `imageId` `DescribeLaunchTemplateVersions` reports for the `$Latest` version of
/// `launch_template_id`, with `ResolveAlias` as `asked` names it (`None` leaving the member
/// out of the request altogether).
async fn described_image_id(
    case: &str,
    router: &MultiServiceRouter,
    launch_template_id: &str,
    asked: Option<&str>,
) -> String {
    let mut params = vec![
        param("LaunchTemplateId", launch_template_id),
        param("LaunchTemplateVersion.1", "$Latest"),
    ];
    if let Some(value) = asked {
        params.push(param("ResolveAlias", value));
    }
    let (status, body) = call(router, "DescribeLaunchTemplateVersions", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?} with ResolveAlias={asked:?}: expected HTTP 200, got {status} with \
         body {body}"
    );
    element(&body, "imageId").unwrap_or_else(|| {
        panic!("case {case:?} with ResolveAlias={asked:?}: no <imageId> in {body}")
    })
}

/// The catalog the resolution reads: every public Systems Manager parameter name this
/// account resolves, and the id of the AMI it names. Read out of the store the handler
/// reads, so the table below is the service's own catalog rather than a copy of it.
fn catalog(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    engine
        .state::<Ec2State>()
        .lock()
        .public_ssm_parameter_image_ids(engine::emulator_account_id())
        .into_iter()
        .collect()
}

/// RULE: a stored `ImageId` of the form `resolve:ssm:<parameter>` is reported as the AMI id
/// the public parameter catalog resolves `<parameter>` to when `ResolveAlias` is true, and
/// as the stored parameter itself when it is false or absent — for EVERY name the catalog
/// publishes, and the resolved id is one `DescribeImages` reports as an available image.
#[tokio::test]
async fn every_published_parameter_name_resolves_to_the_ami_it_publishes() {
    let (router, engine) = super::ec2_query::emulator();
    let published = catalog(&engine);
    assert!(
        published.len() > 1,
        "the image catalog must publish more than one parameter name to span, got \
         {published:?}"
    );

    for (index, (parameter, image_id)) in published.iter().enumerate() {
        let case = format!("{parameter} -> {image_id}");
        let alias = format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}");
        let template =
            create_template(&case, &router, &format!("ssm.catalog.{index}"), &alias).await;

        // The stored form: the parameter, which is what the default and an explicit false
        // report.
        for asked in [None, Some("false")] {
            let reported = described_image_id(&case, &router, &template, asked).await;
            assert_eq!(
                reported, alias,
                "case {case:?} with ResolveAlias={asked:?}: expected the stored parameter \
                 {alias}, got {reported:?}"
            );
        }

        // The resolved form: the AMI id, which is a different answer to the same call.
        let resolved = described_image_id(&case, &router, &template, Some("true")).await;
        assert_eq!(
            &resolved, image_id,
            "case {case:?}: expected ResolveAlias=true to report {image_id}, got {resolved:?}"
        );
        assert!(
            resolved.starts_with("ami-") && resolved != alias,
            "case {case:?}: expected an AMI id differing from the stored alias, got \
             {resolved:?}"
        );

        // The catalog is the image store, so the AMI it resolved to is one the describe
        // reports — available, and published by Amazon.
        let (status, images) =
            call(&router, "DescribeImages", &indexed("ImageId", &[resolved.as_str()])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the resolved AMI must be describable, got {status} with {images}"
        );
        assert_eq!(
            element(&images, "imageId").as_deref(),
            Some(resolved.as_str()),
            "case {case:?}: expected DescribeImages to report {resolved}, got {images}"
        );
        assert_eq!(
            element(&images, "imageState").as_deref(),
            Some("available"),
            "case {case:?}: expected the resolved AMI to be available, got {images}"
        );
        assert_eq!(
            element(&images, "imageOwnerAlias").as_deref(),
            Some("amazon"),
            "case {case:?}: expected the resolved AMI to be published by Amazon, got {images}"
        );
    }
}

/// RULE: a name the catalog publishes BESIDE an image's `Image$PublicSsmParameterName`
/// resolves to that same image — two published names for one AMI are two ways of asking for
/// it, not two AMIs.
///
/// The rows are the published pairs themselves
/// (`ec2::state::PUBLIC_SSM_PARAMETER_ALIASES`), so every one of them is exercised rather
/// than the one an issue happened to cite.
#[tokio::test]
async fn a_second_published_name_resolves_to_the_same_ami_as_the_first() {
    let (router, engine) = super::ec2_query::emulator();
    let published: Vec<(String, String)> = catalog(&engine);
    let resolved_id = |name: &str| -> Option<String> {
        published
            .iter()
            .find(|(parameter, _)| parameter == name)
            .map(|(_, image_id)| image_id.clone())
    };

    assert!(
        !PUBLIC_SSM_PARAMETER_ALIASES.is_empty(),
        "the published catalog must name at least one pair to span"
    );
    for (index, (name, published_as)) in PUBLIC_SSM_PARAMETER_ALIASES.iter().enumerate() {
        let case = format!("{name} beside {published_as}");
        let expected = resolved_id(published_as).unwrap_or_else(|| {
            panic!("case {case:?}: the catalog must publish {published_as}, got {published:?}")
        });
        let through_the_second_name = resolved_id(name).unwrap_or_else(|| {
            panic!("case {case:?}: the catalog must resolve {name}, got {published:?}")
        });
        assert_eq!(
            through_the_second_name, expected,
            "case {case:?}: both names must resolve to one AMI, got {through_the_second_name:?} \
             and {expected:?}"
        );

        // And end to end: a template storing either name reports the same AMI id under
        // `ResolveAlias=true`.
        let mut reported: Vec<String> = Vec::new();
        for (which, parameter) in [("second", *name), ("first", *published_as)] {
            let alias = format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}");
            let template = create_template(
                &case,
                &router,
                &format!("ssm.pair.{index}.{which}"),
                &alias,
            )
            .await;
            reported.push(described_image_id(&case, &router, &template, Some("true")).await);
        }
        assert_eq!(
            reported,
            vec![expected.clone(), expected.clone()],
            "case {case:?}: both names must describe as {expected}, got {reported:?}"
        );
    }
}
