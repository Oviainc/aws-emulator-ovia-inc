// SPDX-License-Identifier: CC0-1.0

//! The `DeploymentStatus` of a configuration set, driven end to end over this
//! service's awsQuery surface.
//!
//! RULE under test: every configuration-settings description this service
//! reports carries a `DeploymentStatus` drawn from the modelled
//! `ConfigurationDeploymentStatus` value set, together with the instants the set
//! was created and last changed — the configuration set a stored resource
//! currently holds reports `deployed` ("this is the configuration that is
//! currently deployed"), while a draft that has not reached its environment
//! reports its own `pending` or `failed` status. So a client can tell a settled
//! configuration set from a draft without special-casing an absent member,
//! whichever kind of configuration set it described: a saved configuration
//! template, which IS the configuration set it holds, or the configuration set
//! of a running environment.
//!
//! Rows differ along what the rule generalises over: which kind of configuration
//! set is described, which source the configuration came from, how many option
//! settings it carries, how many descriptions one answer holds, and which
//! operation answered — never a renamed copy of the row before it.
//!
//! The HTTP harness (`Emulator`, `Answer`) is `all/lifecycle.rs`'s, the
//! option-setting encoder is `all/configuration_templates.rs`'s, and the one
//! precondition written into the store directly — an in-flight update with
//! NOTHING owed by the clock — is `all/environment_draft_configuration.rs`'s, so
//! no second copy of any of them can drift.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::configuration_templates::{as_params, created, option_params, published_stack, Option4};
use crate::environment_draft_configuration::hold_where_it_is;
use crate::lifecycle::{instant, Answer, Emulator, PAST_DUE, TEST_REGION};

/// The application every configuration set below belongs to.
const APPLICATION: &str = "deployment-status-app";

/// Configuration-option namespaces from Elastic Beanstalk's published
/// configuration-option reference.
const APP_ENV: &str = "aws:elasticbeanstalk:application:environment";
const ASG: &str = "aws:autoscaling:asg";

/// The configuration the fixture environments are launched with.
const INITIAL: &[Option4] = &[(APP_ENV, "STAGE", "qa", ""), (ASG, "MinSize", "2", "")];

/// Assert one configuration-settings description carries a `DeploymentStatus`
/// the model declares, equal to `expected`, beside the instants the set was
/// created and last changed.
fn assert_reported_status(description: &Answer, case: &str, expected: &str) {
    let reported = description.value("DeploymentStatus").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a DeploymentStatus, got {}",
            description.body
        )
    });
    // The modelled value set, read from the generated enum rather than written
    // out again here, so a status this service invents fails.
    assert!(
        eb::ops::types::CONFIGURATION_DEPLOYMENT_STATUS_VALUES.contains(&reported),
        "case {case:?}: expected one of {:?}, got {reported:?} in {}",
        eb::ops::types::CONFIGURATION_DEPLOYMENT_STATUS_VALUES,
        description.body
    );
    assert_eq!(
        reported, expected,
        "case {case:?}: expected DeploymentStatus {expected:?}, got {reported:?} in {}",
        description.body
    );
    // "DateCreated: The date (in UTC time) when this configuration set was
    // created" / "DateUpdated: … last modified": a described configuration set
    // reports both, and it was not modified before it was created.
    let created_at = description.value("DateCreated").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a DateCreated, got {}",
            description.body
        )
    });
    let updated_at = description.value("DateUpdated").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a DateUpdated, got {}",
            description.body
        )
    });
    assert!(
        instant(created_at, case) <= instant(updated_at, case),
        "case {case:?}: expected DateUpdated at or after DateCreated, got {created_at:?} \
         and {updated_at:?}"
    );
}

/// One row of the template table: the request members beyond the application and
/// the template name, and the option settings the create sends.
struct TemplateSetCase {
    case: &'static str,
    template_name: &'static str,
    params: Vec<(String, String)>,
    settings: Vec<Option4>,
}

#[tokio::test]
async fn a_saved_configuration_template_reports_its_configuration_set_as_deployed() {
    let emulator = Emulator::new();
    let case = "a template's configuration set";
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    // An environment and a template for the rows that base a template on
    // something that already carries a configuration.
    let environment_stack = published_stack(1);
    let mut environment_params = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), "status-source".to_string()),
        ("SolutionStackName".to_string(), environment_stack.name()),
    ];
    environment_params.extend(option_params(INITIAL));
    let environment = created(
        &emulator,
        "CreateEnvironment",
        &as_params(&environment_params),
    )
    .await;
    let environment_id = environment
        .value("EnvironmentId")
        .expect("CreateEnvironment answers with an EnvironmentId")
        .to_string();

    let base_stack = published_stack(0);
    let arn_stack = published_stack(2);
    let rows = vec![
        TemplateSetCase {
            case: "a solution stack and no option settings",
            template_name: "status-stack-only",
            params: vec![("SolutionStackName".to_string(), base_stack.name())],
            settings: Vec::new(),
        },
        TemplateSetCase {
            case: "a solution stack with two option settings",
            template_name: "status-stack-options",
            params: vec![("SolutionStackName".to_string(), base_stack.name())],
            settings: vec![(ASG, "MinSize", "4", ""), (APP_ENV, "TIER", "gold", "")],
        },
        TemplateSetCase {
            case: "a managed platform version named by its ARN",
            template_name: "status-by-arn",
            params: vec![(
                "PlatformArn".to_string(),
                arn_stack.platform_arn(TEST_REGION),
            )],
            settings: vec![(ASG, "MaxSize", "9", "")],
        },
        TemplateSetCase {
            case: "based on another saved configuration",
            template_name: "status-from-source",
            params: vec![
                (
                    "SourceConfiguration.ApplicationName".to_string(),
                    APPLICATION.to_string(),
                ),
                (
                    "SourceConfiguration.TemplateName".to_string(),
                    "status-stack-options".to_string(),
                ),
            ],
            settings: vec![(ASG, "MinSize", "6", "")],
        },
        TemplateSetCase {
            case: "based on a running environment",
            template_name: "status-from-environment",
            params: vec![("EnvironmentId".to_string(), environment_id.clone())],
            settings: Vec::new(),
        },
    ];

    for row in &rows {
        let mut params = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("TemplateName".to_string(), row.template_name.to_string()),
        ];
        params.extend(row.params.clone());
        params.extend(option_params(&row.settings));
        let created_answer = created(
            &emulator,
            "CreateConfigurationTemplate",
            &as_params(&params),
        )
        .await;
        // The same structure is this operation's output shape, so the status is
        // reported where the configuration set is first answered too.
        assert_reported_status(
            &created_answer,
            row.case,
            eb::state::DEPLOYMENT_STATUS_DEPLOYED,
        );

        // Twice, because a read must not change what the next caller sees.
        for read in 1..=2 {
            let described = created(
                &emulator,
                "DescribeConfigurationSettings",
                &[
                    ("ApplicationName", APPLICATION),
                    ("TemplateName", row.template_name),
                ],
            )
            .await;
            let descriptions = described.elements_at("ConfigurationSettings/member");
            assert_eq!(
                descriptions.len(),
                1,
                "case {:?}: a template is ONE configuration set, on read {read}, got {}",
                row.case,
                described.body
            );
            assert_reported_status(
                &descriptions[0],
                row.case,
                eb::state::DEPLOYMENT_STATUS_DEPLOYED,
            );
            // The description names the template whose set it is, and
            // "templates aren't associated with any environment".
            assert_eq!(
                descriptions[0].value("TemplateName"),
                Some(row.template_name),
                "case {:?}: the description must name its template, got {}",
                row.case,
                described.body
            );
            assert_eq!(
                descriptions[0].value("EnvironmentName"),
                None,
                "case {:?}: a template names no environment, got {}",
                row.case,
                described.body
            );
        }
    }

    // Nothing above reported a status for a configuration set that is not
    // there: a template the store does not hold is refused, not answered with
    // an empty description.
    let refused = emulator
        .call(
            "DescribeConfigurationSettings",
            &[
                ("ApplicationName", APPLICATION),
                ("TemplateName", "status-absent"),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 for a template the store does not hold, got {} \
         and body {}",
        refused.status, refused.body
    );
}

/// One row of the environment table: what happened to the environment, and the
/// `DeploymentStatus` of every description the answer must then hold, in
/// document order.
struct EnvironmentSetCase {
    case: &'static str,
    environment: &'static str,
    /// The option settings an update deploys onto it, or none for a row that
    /// updates nothing.
    update: &'static [Option4],
    /// Whether that update is then cancelled.
    cancel: bool,
    expected_statuses: &'static [&'static str],
}

#[tokio::test]
async fn an_environments_configuration_sets_report_the_deployed_one_and_its_draft() {
    // Rows span the three shapes an environment's answer takes: the deployed
    // configuration alone, that one beside a draft waiting to be deployed, and
    // that one beside a draft whose deployment was cancelled.
    let rows = &[
        EnvironmentSetCase {
            case: "an environment with nothing in flight",
            environment: "status-ready",
            update: &[],
            cancel: false,
            expected_statuses: &[eb::state::DEPLOYMENT_STATUS_DEPLOYED],
        },
        EnvironmentSetCase {
            case: "an environment with an update in flight",
            environment: "status-updating",
            update: &[(ASG, "MinSize", "5", "")],
            cancel: false,
            expected_statuses: &[
                eb::state::DEPLOYMENT_STATUS_DEPLOYED,
                eb::state::DEPLOYMENT_STATUS_PENDING,
            ],
        },
        EnvironmentSetCase {
            case: "an environment whose update was cancelled",
            environment: "status-cancelled",
            update: &[(APP_ENV, "STAGE", "prod", ""), (ASG, "MinSize", "8", "")],
            cancel: true,
            expected_statuses: &[
                eb::state::DEPLOYMENT_STATUS_DEPLOYED,
                eb::state::DEPLOYMENT_STATUS_FAILED,
            ],
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        created(
            &emulator,
            "CreateApplication",
            &[("ApplicationName", APPLICATION)],
        )
        .await;
        let mut params = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("EnvironmentName".to_string(), row.environment.to_string()),
            (
                "SolutionStackName".to_string(),
                published_stack(0).name(),
            ),
        ];
        params.extend(option_params(INITIAL));
        created(&emulator, "CreateEnvironment", &as_params(&params)).await;
        // The launch the clock owed completes, so the environment is running
        // the configuration it was created with.
        std::thread::sleep(PAST_DUE);

        if !row.update.is_empty() {
            let mut update = vec![("EnvironmentName".to_string(), row.environment.to_string())];
            update.extend(option_params(row.update));
            created(&emulator, "UpdateEnvironment", &as_params(&update)).await;
            // The draft stays where the update left it however long the reads
            // below take.
            hold_where_it_is(&emulator, row.case, row.environment);
            if row.cancel {
                created(
                    &emulator,
                    "AbortEnvironmentUpdate",
                    &[("EnvironmentName", row.environment)],
                )
                .await;
                hold_where_it_is(&emulator, row.case, row.environment);
            }
        }

        let described = created(
            &emulator,
            "DescribeConfigurationSettings",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", row.environment),
            ],
        )
        .await;
        let descriptions = described.elements_at("ConfigurationSettings/member");
        assert_eq!(
            descriptions.len(),
            row.expected_statuses.len(),
            "case {:?}: expected {} configuration set(s), got {}",
            row.case,
            row.expected_statuses.len(),
            described.body
        );
        for (description, expected) in descriptions.iter().zip(row.expected_statuses) {
            assert_reported_status(description, row.case, expected);
            // Every description here is a set OF THIS ENVIRONMENT, so it names
            // the environment and no template.
            assert_eq!(
                description.value("EnvironmentName"),
                Some(row.environment),
                "case {:?}: the description must name its environment, got {}",
                row.case,
                described.body
            );
            assert_eq!(
                description.value("TemplateName"),
                None,
                "case {:?}: an environment's configuration set is no template, got {}",
                row.case,
                described.body
            );
        }
    }
}
