// SPDX-License-Identifier: CC0-1.0

//! The INSTANCE-TYPE CATALOG a request that places an instance is held to, driven over the
//! ec2Query wire the service speaks through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: **a request that names an instance type this service does not publish
//! is refused `InvalidParameterValue` naming the offending value, rather than placing an
//! instance of a type no AWS Region offers.** The catalog is the modelled `InstanceType`
//! enum, which [`ec2::state::instance_type_is_modelled`] answers against, and the refusal's
//! sentence is AWS's wording for a value outside a member's own set — `Invalid value '<the
//! value>' for InstanceType.`
//!
//! The rows of the first table differ along what the rule generalises over: whether the value
//! is a published type at all, which generation and size form it takes when it is (a current
//! generation, a previous one, a bare-metal size), and HOW it misses the catalog when it is
//! not (a family that does not exist, a size the catalog does not carry for a family that
//! does, a family stated with no size, a size stated with no family, a published type in the
//! wrong case). Every row first asserts what the service's OWN catalog says about the value,
//! so the table is read against that catalog rather than asserting a copy of it.
//!
//! The second table is the same rule at the OTHER calls that name the instance type an
//! instance will be placed as — a Spot Instance request, a Spot Fleet pool and a launch
//! template — so the catalog is one catalog rather than one per operation.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// An AMI id of the form EC2 issues: a launch names one directly, and the rows below do not
/// care which image they launch from.
const AMI: &str = "ami-0123456789abcdef0";

/// EC2's error code for a member value outside the set that member admits.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// One request's form parameters, as the harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// An AMI of the published catalog a fresh account can already see, read out of the service's
/// own state rather than written here: the calls below that resolve the image against the
/// catalog cannot be given an invented id.
fn catalog_image() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// The sentence AWS words this class of refusal with, for the value a request named.
fn refusal_message(instance_type: &str) -> String {
    format!("Invalid value '{instance_type}' for InstanceType.")
}

/// One launch of a single instance of `instance_type`.
async fn launch(router: &MultiServiceRouter, instance_type: &str) -> (u16, String) {
    call(
        router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("InstanceType", instance_type),
        ],
    )
    .await
}

/// A launch names an instance type the service publishes or it names nothing it could place:
/// a published type launches and the instance reports it, and a value outside the catalog is
/// refused `InvalidParameterValue` naming that value.
#[tokio::test]
async fn a_launch_is_held_to_the_published_instance_type_catalog() {
    // (case, the InstanceType the launch names, whether the catalog publishes it)
    let rows: &[(&str, &str, bool)] = &[
        ("a current-generation type", "t3.micro", true),
        ("a previous-generation type", "m1.small", true),
        ("a bare-metal size", "c5.metal", true),
        ("a family that does not exist", "sim2real.invalid-type", false),
        ("a size the family does not offer", "t3.enormous", false),
        ("a family stated with no size", "m5", false),
        ("a size stated with no family", "large", false),
        ("a published type in upper case", "T3.MICRO", false),
    ];

    let router = emulator();
    for (case, instance_type, published) in rows {
        // The row is read against the service's own catalog first, so a table that drifted
        // from the model fails here rather than asserting the wrong half of the rule.
        let modelled = ec2::state::instance_type_is_modelled(instance_type);
        assert_eq!(
            modelled, *published,
            "case {case:?}: expected the catalog to publish {instance_type:?} = {published}, \
             got {modelled}"
        );
        let (status, body) = launch(&router, instance_type).await;
        if *published {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from a launch of {instance_type:?}, got \
                 {status} with body {body}"
            );
            let instance_id = element(&body, "instanceId")
                .unwrap_or_else(|| panic!("case {case:?}: no <instanceId> in {body}"));
            let (described_status, described) = call(
                &router,
                REGION,
                "DescribeInstances",
                &[p("InstanceId.1", &instance_id)],
            )
            .await;
            assert_eq!(
                described_status, 200,
                "case {case:?}: expected HTTP 200 from DescribeInstances, got \
                 {described_status} with body {described}"
            );
            let reported = element(&described, "instanceType");
            assert_eq!(
                reported.as_deref(),
                Some(*instance_type),
                "case {case:?}: expected the launched instance to report \
                 {instance_type:?}, got {reported:?} in {described}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a launch of {instance_type:?}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got {code:?} \
             in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            refusal_message(instance_type),
            "case {case:?}: expected the message {:?}, got {message:?}",
            refusal_message(instance_type)
        );
        // Nothing was placed for a refused launch: the account holds no instance of that
        // type, which is what makes the refusal a refusal rather than a report.
        let (_, described) = call(&router, REGION, "DescribeInstances", &[]).await;
        assert!(
            !described.contains(instance_type),
            "case {case:?}: expected no instance of {instance_type:?} to exist, got \
             {described}"
        );
    }
}

/// Every other call that names the instance type an instance will be PLACED as is held to the
/// same catalog: a Spot Instance request's launch specification, one pool of a Spot Fleet
/// request, and the launch template data every launch made from a template carries.
#[tokio::test]
async fn every_call_that_places_an_instance_is_held_to_the_same_catalog() {
    /// A value outside the catalog, whichever call names it.
    const UNPUBLISHED: &str = "sim2real.invalid-type";
    /// A value inside it.
    const PUBLISHED: &str = "t3.micro";

    /// One row of the table: what the case is, the action called, and the parameters that name
    /// an instance type for it.
    type Row = (&'static str, &'static str, fn(&str) -> Params);

    // (case, the action, the parameters naming `<type>` as the instance type)
    let rows: &[Row] = &[
        ("a Spot Instance request", "RequestSpotInstances", |named| {
            vec![
                p("LaunchSpecification.ImageId", catalog_image()),
                p("LaunchSpecification.InstanceType", named),
            ]
        }),
        ("one pool of a Spot Fleet request", "RequestSpotFleet", |named| {
            vec![
                p(
                    "SpotFleetRequestConfig.IamFleetRole",
                    "arn:aws:iam::111122223333:role/aws-ec2-spot-fleet-tagging-role",
                ),
                p("SpotFleetRequestConfig.TargetCapacity", "1"),
                p(
                    "SpotFleetRequestConfig.LaunchSpecifications.1.ImageId",
                    catalog_image(),
                ),
                p(
                    "SpotFleetRequestConfig.LaunchSpecifications.1.InstanceType",
                    named,
                ),
            ]
        }),
        ("the data of a launch template", "CreateLaunchTemplate", |named| {
            vec![
                p("LaunchTemplateName", format!("template-for-{named}")),
                p("LaunchTemplateData.ImageId", AMI),
                p("LaunchTemplateData.InstanceType", named),
            ]
        }),
    ];

    let router = emulator();
    for (case, action, parameters) in rows {
        let (status, body) = call(&router, REGION, action, &parameters(UNPUBLISHED)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {action} naming {UNPUBLISHED:?}, got \
             {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got {code:?} \
             in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            refusal_message(UNPUBLISHED),
            "case {case:?}: expected the message {:?}, got {message:?}",
            refusal_message(UNPUBLISHED)
        );
        // The same call naming a PUBLISHED type is served, so the refusal above is about the
        // instance type and not about anything else the request carried.
        let (status, body) = call(&router, REGION, action, &parameters(PUBLISHED)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action} naming {PUBLISHED:?}, got {status} \
             with body {body}"
        );
    }
}
