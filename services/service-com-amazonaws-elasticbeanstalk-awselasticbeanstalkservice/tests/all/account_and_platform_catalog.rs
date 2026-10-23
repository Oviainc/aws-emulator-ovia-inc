// SPDX-License-Identifier: CC0-1.0

//! The account-wide Elastic Beanstalk operation that takes no input at all:
//! `DescribeAccountAttributes`, driven end to end over the service's awsQuery HTTP
//! surface.
//!
//! (The other two no-input operations have their own suites: `ListAvailableSolutionStacks`
//! in `tests/all/solution_stacks.rs`, which covers the platform catalog this file once
//! did, and `CreateStorageLocation` in `tests/all/record_projections.rs`, which owns the
//! per-region bucket and its quota.)
//!
//! RULE under test: `DescribeAccountAttributes` answers 200 with the five per-resource
//! quotas of the calling account, each the bound this service holds the account to,
//! unchanged by what the account already holds.
//!
//! The emulator, the awsQuery request encoder and the XML element reader are the
//! ones `tests/all/lifecycle.rs` owns, so a request is built and read exactly one
//! way across this crate.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::Emulator;

/// Regions the suite signs with: the service's own fallback region and three
/// others, so every region-derived value is asserted against more than one region.
const REGIONS: &[&str] = &["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"];

// ---------------------------------------------------------------------------
// The account's resource quotas
// ---------------------------------------------------------------------------

/// Every quota the operation reports: the member path and the bound this service
/// holds the account to.
///
/// Four of the five are bounds a create in this crate refuses a create past
/// (`CreateApplication`, `CreateApplicationVersion`, `CreateEnvironment`,
/// `CreatePlatformVersion`); `ConfigurationTemplateQuota` is the published
/// allowance of a create that is still scaffolded, so it is reported but not yet
/// enforced — unlike `ApplicationQuota`, which the last test in this file ties to
/// the refusal itself.
fn expected_quotas() -> Vec<(&'static str, usize)> {
    vec![
        ("ApplicationQuota", eb::state::MAX_APPLICATIONS),
        (
            "ApplicationVersionQuota",
            eb::state::MAX_APPLICATION_VERSIONS,
        ),
        ("EnvironmentQuota", eb::state::MAX_ENVIRONMENTS),
        (
            "ConfigurationTemplateQuota",
            eb::state::MAX_CONFIGURATION_TEMPLATES,
        ),
        ("CustomPlatformQuota", eb::state::MAX_PLATFORM_VERSIONS),
    ]
}

/// RULE: all five modelled quotas are reported, each the bound this service
/// enforces, in every region and whatever the account already holds. Rows: the five
/// quota members × four regions × an empty account and an account holding
/// resources.
#[tokio::test]
async fn the_account_attributes_report_every_resource_quota() {
    let emulator = Emulator::new();

    for populated in [false, true] {
        if populated {
            // A quota is a bound, not a count: creating resources must not move it.
            let created = emulator
                .call("CreateApplication", &[("ApplicationName", "quota-app")])
                .await;
            assert_eq!(
                created.status, 200,
                "the fixture CreateApplication must succeed, got {} and body {}",
                created.status, created.body
            );
        }

        for region in REGIONS {
            let case = format!("{region} populated={populated}");
            let answer = emulator
                .call_in_region(region, "DescribeAccountAttributes", &[])
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from DescribeAccountAttributes, got {} and body {}",
                answer.status, answer.body
            );

            for (member, bound) in expected_quotas() {
                let path = format!("ResourceQuotas/{member}/Maximum");
                let reported = answer.expect(&path, &case);
                assert_eq!(
                    reported,
                    bound.to_string(),
                    "case {case:?}: expected {member} {bound}, got {reported:?}"
                );
            }
        }
    }
}

/// RULE, tied to behaviour: the reported `ApplicationQuota` is the bound the
/// service REFUSES a create past, not a number chosen for the response — so an
/// account filled to the reported maximum accepts no further application.
#[tokio::test]
async fn the_reported_application_quota_is_the_bound_the_service_enforces() {
    let emulator = Emulator::new();
    let case = "application quota";

    let answer = emulator.call("DescribeAccountAttributes", &[]).await;
    let reported = answer
        .expect("ResourceQuotas/ApplicationQuota/Maximum", case)
        .parse::<usize>()
        .expect("a numeric Maximum");

    for index in 0..reported {
        let name = format!("app-{index:03}");
        let created = emulator
            .call("CreateApplication", &[("ApplicationName", name.as_str())])
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: application {index} of {reported} must be accepted, got {} and body {}",
            created.status, created.body
        );
    }

    let refused = emulator
        .call("CreateApplication", &[("ApplicationName", "one-too-many")])
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected 400 past the reported quota, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TooManyApplicationsException",
        "case {case:?}: expected the modelled awsQuery code, got {}",
        refused.body
    );
}
