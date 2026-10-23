// SPDX-License-Identifier: CC0-1.0

//! The `UpdateApplication` response's projection of the application's resource
//! lifecycle configuration.
//!
//! The rule under test: every projection of a stored application reports that
//! record's WHOLE resource lifecycle configuration as the store holds it at the
//! moment of the call — an application always has one, because the service
//! applies its own version-lifecycle policy to an application whose caller named
//! none — so `UpdateApplication`'s `ApplicationDescription` carries the same
//! `ResourceLifecycleConfig` the read path reports, whatever the update itself
//! changed.
//!
//! `tests/all/record_projections.rs` asserts that rule over the configurations a
//! CREATE can ask for. This suite varies what the update path itself generalises
//! over: whether the update supplies a property at all, whether it clears one,
//! and whether the policy the store holds is still the creation-time one or has
//! since been replaced by `UpdateApplicationResourceLifecycle`. Its reader for
//! the six reported settings is that suite's [`ReportedLifecycle`], so both
//! suites read one set of element paths.
//!
//! Driven over the awsQuery HTTP surface through the shared [`Emulator`]
//! harness: a projection that drops a member fails here even though the store
//! holds it.

use crate::harness::Emulator;
use crate::record_projections::{ok, ReportedLifecycle};

/// An IAM service role ARN of the form
/// `ApplicationResourceLifecycleConfig$ServiceRole` documents ("The ARN of an IAM
/// service role that Elastic Beanstalk has permission to assume"), which this
/// service requires the first time a call supplies a `VersionLifecycleConfig`.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/aws-elasticbeanstalk-service-role";

/// The `Description` every row's application is created with, so an update that
/// omits the member and one that clears it both start from a stored value.
const INITIAL_DESCRIPTION: &str = "the description creation stored";

/// One row of the table: how the application's policy got to be what it is, what
/// the update under test supplies, and what every projection must then report.
struct Case {
    /// Doubles as the application name, so each row is an independent resource.
    case: &'static str,
    /// The lifecycle members `CreateApplication` supplies (empty: none at all).
    created_with: &'static [(&'static str, &'static str)],
    /// The lifecycle members an `UpdateApplicationResourceLifecycle` call made
    /// BEFORE the update under test supplies (empty: no such call).
    relifecycled_with: &'static [(&'static str, &'static str)],
    /// The `Description` the update under test supplies (`None`: the member is
    /// omitted, so the update applies nothing).
    update_description: Option<&'static str>,
    /// Version labels created under the application first — resources the update
    /// does not take, present so the projection is asserted on an application
    /// that carries more than its own name.
    versions: &'static [&'static str],
    /// The six version-lifecycle settings every projection must report.
    expected: fn() -> ReportedLifecycle,
    /// The `ServiceRole` every projection must report (`None`: the application
    /// was never given one, so it has none).
    expected_service_role: Option<&'static str>,
}

#[tokio::test]
async fn update_application_reports_the_stored_resource_lifecycle_config() {
    let cases = [
        Case {
            // The plainest case: nothing ever named a policy, so what every
            // projection reports is the service's own default.
            case: "a-default-policy-and-a-new-description",
            created_with: &[],
            relifecycled_with: &[],
            update_description: Some("a new description"),
            versions: &[],
            expected: ReportedLifecycle::service_default,
            expected_service_role: None,
        },
        Case {
            // An update that supplies no property applies nothing — and still
            // projects the whole policy.
            case: "a-default-policy-and-an-omitted-description",
            created_with: &[],
            relifecycled_with: &[],
            update_description: None,
            versions: &["v1"],
            expected: ReportedLifecycle::service_default,
            expected_service_role: None,
        },
        Case {
            // "To clear these properties, specify an empty string": the member
            // that goes away is the description, never the policy beside it.
            case: "a-default-policy-and-a-cleared-description",
            created_with: &[],
            relifecycled_with: &[],
            update_description: Some(""),
            versions: &[],
            expected: ReportedLifecycle::service_default,
            expected_service_role: None,
        },
        Case {
            // A role with no rules: the role is reported, the rules are the
            // service defaults.
            case: "a-service-role-and-the-default-rules",
            created_with: &[("ResourceLifecycleConfig.ServiceRole", SERVICE_ROLE)],
            relifecycled_with: &[],
            update_description: Some("still the default rules"),
            versions: &[],
            expected: ReportedLifecycle::service_default,
            expected_service_role: Some(SERVICE_ROLE),
        },
        Case {
            // Every setting named at creation, none of them a default.
            case: "a-policy-named-at-creation",
            created_with: &[
                ("ResourceLifecycleConfig.ServiceRole", SERVICE_ROLE),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.MaxAgeInDays",
                    "7",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.DeleteSourceFromS3",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "3",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.DeleteSourceFromS3",
                    "true",
                ),
            ],
            relifecycled_with: &[],
            update_description: Some("a named policy"),
            versions: &["1.0.0", "1.0.1"],
            expected: || ReportedLifecycle {
                max_age_enabled: "true".to_string(),
                max_age_in_days: "7".to_string(),
                max_age_delete_source: "true".to_string(),
                max_count_enabled: "true".to_string(),
                max_count: "3".to_string(),
                max_count_delete_source: "true".to_string(),
            },
            expected_service_role: Some(SERVICE_ROLE),
        },
        Case {
            // The policy the store holds is no longer the creation-time one, so
            // the update projects the REPLACEMENT. The role arrives with that
            // call, since the application carried none.
            case: "a-policy-replaced-after-creation",
            created_with: &[],
            relifecycled_with: &[
                ("ResourceLifecycleConfig.ServiceRole", SERVICE_ROLE),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "4",
                ),
            ],
            update_description: Some("a replaced policy"),
            versions: &[],
            expected: || ReportedLifecycle {
                max_count_enabled: "true".to_string(),
                max_count: "4".to_string(),
                ..ReportedLifecycle::service_default()
            },
            expected_service_role: Some(SERVICE_ROLE),
        },
        Case {
            // The replacement names only the age rule and no role: the count
            // rule falls back to the service default and the role already
            // persisted with the application survives — which is exactly what
            // the update must then project.
            case: "a-policy-replaced-without-naming-the-role",
            created_with: &[
                ("ResourceLifecycleConfig.ServiceRole", SERVICE_ROLE),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.MaxAgeInDays",
                    "7",
                ),
            ],
            relifecycled_with: &[
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.Enabled",
                    "false",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.MaxAgeInDays",
                    "30",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.DeleteSourceFromS3",
                    "true",
                ),
            ],
            update_description: None,
            versions: &["release-1", "release-2", "release-3"],
            expected: || ReportedLifecycle {
                max_age_in_days: "30".to_string(),
                max_age_delete_source: "true".to_string(),
                ..ReportedLifecycle::service_default()
            },
            expected_service_role: Some(SERVICE_ROLE),
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", case.case),
            ("Description", INITIAL_DESCRIPTION),
        ];
        params.extend_from_slice(case.created_with);
        ok(&emulator, case.case, "CreateApplication", &params).await;

        for label in case.versions {
            ok(
                &emulator,
                case.case,
                "CreateApplicationVersion",
                &[("ApplicationName", case.case), ("VersionLabel", label)],
            )
            .await;
        }

        if !case.relifecycled_with.is_empty() {
            let mut params: Vec<(&str, &str)> = vec![("ApplicationName", case.case)];
            params.extend_from_slice(case.relifecycled_with);
            ok(
                &emulator,
                case.case,
                "UpdateApplicationResourceLifecycle",
                &params,
            )
            .await;
        }

        let expected = (case.expected)();

        let mut update_params: Vec<(&str, &str)> = vec![("ApplicationName", case.case)];
        if let Some(description) = case.update_description {
            update_params.push(("Description", description));
        }
        let updated = ok(
            &emulator,
            case.case,
            "UpdateApplication",
            &update_params,
        )
        .await;

        // The update's own answer carries the whole policy,
        assert_eq!(
            ReportedLifecycle::of(&updated, "Application/ResourceLifecycleConfig", case.case),
            expected,
            "case {:?}: unexpected ResourceLifecycleConfig from UpdateApplication, got {}",
            case.case,
            updated.body
        );
        assert_eq!(
            updated.value("Application/ResourceLifecycleConfig/ServiceRole"),
            case.expected_service_role,
            "case {:?}: unexpected ServiceRole from UpdateApplication, got {}",
            case.case,
            updated.body
        );

        // the read path reports the same policy for the same record — the update
        // response is a projection of what is stored, not a second answer,
        let described = ok(
            &emulator,
            case.case,
            "DescribeApplications",
            &[("ApplicationNames.member.1", case.case)],
        )
        .await;
        assert_eq!(
            ReportedLifecycle::of(
                &described,
                "Applications/member/ResourceLifecycleConfig",
                case.case
            ),
            expected,
            "case {:?}: unexpected ResourceLifecycleConfig from DescribeApplications, got {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.value("Applications/member/ResourceLifecycleConfig/ServiceRole"),
            case.expected_service_role,
            "case {:?}: unexpected ServiceRole from DescribeApplications, got {}",
            case.case,
            described.body
        );

        // and a further update projects it unchanged: nothing about reading or
        // re-updating an application moves its policy.
        let again = ok(
            &emulator,
            case.case,
            "UpdateApplication",
            &[("ApplicationName", case.case)],
        )
        .await;
        assert_eq!(
            ReportedLifecycle::of(&again, "Application/ResourceLifecycleConfig", case.case),
            expected,
            "case {:?}: a second update must project the same policy, got {}",
            case.case,
            again.body
        );

        // The update projects the application it was called for, with the
        // resources it does not take left alone, so the policy above is this
        // application's and not another's.
        assert_eq!(
            updated.expect("Application/ApplicationName", case.case),
            case.case,
            "case {:?}: UpdateApplication answered for another application: {}",
            case.case,
            updated.body
        );
        // Newest first, which is the order the application's `Versions` list is
        // reported in (`state::version_labels` sorts by descending creation
        // sequence); the point here is that the labels are all still there and
        // in that documented order, neither dropped nor reshuffled by an update
        // that does not take them.
        let expected_versions: Vec<&str> = case.versions.iter().rev().copied().collect();
        assert_eq!(
            updated.values_at("Versions/member"),
            expected_versions,
            "case {:?}: an update must not disturb the application's versions, got {}",
            case.case,
            updated.body
        );
    }
}
