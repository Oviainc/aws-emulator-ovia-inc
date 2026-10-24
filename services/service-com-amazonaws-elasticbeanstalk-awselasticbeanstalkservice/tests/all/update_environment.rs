// SPDX-License-Identifier: CC0-1.0

//! `UpdateEnvironment`, driven end to end over this service's awsQuery surface.
//!
//! RULE under test: `UpdateEnvironment` applies to the ONE stored environment the
//! request names exactly the parts of its description, release, configuration and
//! platform version that the request supplies — the configuration being the named
//! template's settings (else the environment's own) with `OptionsToRemove` taken
//! out and `OptionSettings` applied over them — leaves every other part as it
//! stood, and starts the update the model describes: `Updating` ("Environment is
//! in the process of updating its configuration settings or application
//! version"), `AbortableOperationInProgress` true, health checks suspended, with
//! the return to `Ready` owed by the clock. A request that names no environment,
//! names one in no state to be updated, names a value nothing answers to, changes
//! the tier ("if you change the tier version, name, or type, Elastic Beanstalk
//! returns InvalidParameterValue error") or asks for both a release and a
//! configuration ("attempting to update both the release and configuration is not
//! allowed and Elastic Beanstalk returns an InvalidParameterCombination error") is
//! refused with this service's own client-error code and changes nothing.
//!
//! Each table row is a different point in that class — which part of the
//! configuration changes, what kind of value it carries, which identifier names
//! the environment, which refusal class the request falls in — never a renamed
//! copy of the row before it.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns and
//! the option-setting encoder and reader are `all/configuration_templates.rs`'s,
//! so no second copy of either can drift. Nothing here reaches into the store:
//! every change is read back through an implemented reader, so an update that does
//! not persist fails these tests.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::configuration_templates::{
    as_params, expected_options, option_params, published_stack, reported_options, Option4,
};
use crate::lifecycle::{Answer, Emulator, PAST_DUE, SOLUTION_STACK, TEST_REGION};

/// The application every environment below is launched under.
const APPLICATION: &str = "update-env-app";

/// Configuration-option namespaces from Elastic Beanstalk's published
/// configuration-option reference: the environment variables an application
/// reads, the Auto Scaling group, and a scheduled scaling action — the one whose
/// options carry a `ResourceName`.
const APP_ENV: &str = "aws:elasticbeanstalk:application:environment";
const ASG: &str = "aws:autoscaling:asg";
const SCHEDULED: &str = "aws:autoscaling:scheduledaction";

/// The configuration every fixture environment is launched with, so each row
/// starts from one that already carries settings for the update to change.
const INITIAL: &[Option4] = &[(APP_ENV, "STAGE", "qa", ""), (ASG, "MinSize", "2", "")];

/// A value outside ASCII — combining marks, a non-BMP character, several scripts
/// — which has to come back byte for byte.
const UNICODE_VALUE: &str = "Grüße 🌱 こんにちは — ЖØ";

/// Call `action`, failing the named case unless it answered 200 with a success
/// envelope.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "",
        "case {case:?}: expected a success envelope from {action}, got {}",
        answer.body
    );
    answer
}

/// Create [`APPLICATION`] and launch `environment` under it with `settings` and
/// `extra`, letting the launch the clock owed complete so the environment is
/// `Ready`. Answers its `EnvironmentId`.
async fn ready_environment(
    emulator: &Emulator,
    case: &str,
    environment: &str,
    settings: &[Option4],
    extra: &[(&str, &str)],
) -> String {
    let application = [("ApplicationName", APPLICATION)];
    ok(emulator, case, "CreateApplication", &application).await;
    let mut params: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), environment.to_string()),
        ("SolutionStackName".to_string(), SOLUTION_STACK.to_string()),
    ];
    params.extend(option_params(settings));
    params.extend(owned(extra));
    let created = ok(emulator, case, "CreateEnvironment", &as_params(&params)).await;
    let environment_id = created
        .expect("EnvironmentId", case)
        .to_string();
    std::thread::sleep(PAST_DUE);
    environment_id
}

/// `params` as the owned pairs the request builders here collect.
fn owned(params: &[(&str, &str)]) -> Vec<(String, String)> {
    params
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// The awsQuery parameters that send `options` as the request's
/// `OptionsToRemove` list: `(Namespace, OptionName, ResourceName)`, the three
/// members `OptionSpecification` names one option by, with an empty
/// `ResourceName` left off the request.
fn remove_params(options: &[(&str, &str, &str)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (namespace, option_name, resource_name)) in options.iter().enumerate() {
        let at = index + 1;
        params.push((format!("OptionsToRemove.member.{at}.Namespace"), (*namespace).to_string()));
        params.push((format!("OptionsToRemove.member.{at}.OptionName"), (*option_name).to_string()));
        if !resource_name.is_empty() {
            params.push((
                format!("OptionsToRemove.member.{at}.ResourceName"),
                (*resource_name).to_string(),
            ));
        }
    }
    params
}

/// The option settings `DescribeConfigurationSettings` reports as deployed to
/// `environment` — the configuration set "associated with a running environment".
async fn deployed_options(
    emulator: &Emulator,
    case: &str,
    environment: &str,
) -> Vec<(String, String, String, String)> {
    let described = ok(
        emulator,
        case,
        "DescribeConfigurationSettings",
        &[("ApplicationName", APPLICATION), ("EnvironmentName", environment)],
    )
    .await;
    reported_options(&described)
}

/// The `names` members `DescribeEnvironments` reports for `environment`.
async fn described(emulator: &Emulator, case: &str, environment: &str, names: &[&str]) -> Vec<String> {
    let answer = ok(
        emulator,
        case,
        "DescribeEnvironments",
        &[("EnvironmentNames.member.1", environment)],
    )
    .await;
    names
        .iter()
        .map(|name| {
            answer
                .value(&format!("Environments/member/{name}"))
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the configuration table: the options the update sets, the options
/// it removes, and the configuration the environment must then report.
struct ConfigurationCase {
    case: &'static str,
    environment: &'static str,
    set: &'static [Option4],
    remove: &'static [(&'static str, &'static str, &'static str)],
    expected: &'static [Option4],
}

#[tokio::test]
async fn an_update_deploys_exactly_the_configuration_change_the_request_names() {
    // Rows differ along what the rule generalises over: whether the update adds
    // an option, replaces one, empties one, removes one, removes one that is not
    // there, does both to one option, distinguishes two options by `ResourceName`
    // alone, carries several at once or carries none — and what KIND of value it
    // carries.
    let rows = &[
        ConfigurationCase {
            case: "add an option the configuration does not carry",
            environment: "env-add",
            set: &[(APP_ENV, "FEATURE", "on", "")],
            remove: &[],
            expected: &[
                (APP_ENV, "STAGE", "qa", ""),
                (ASG, "MinSize", "2", ""),
                (APP_ENV, "FEATURE", "on", ""),
            ],
        },
        ConfigurationCase {
            case: "replace the value of an option it does carry",
            environment: "env-replace",
            set: &[(ASG, "MinSize", "3", "")],
            remove: &[],
            expected: &[(APP_ENV, "STAGE", "qa", ""), (ASG, "MinSize", "3", "")],
        },
        ConfigurationCase {
            case: "a value outside ASCII",
            environment: "env-unicode",
            set: &[(APP_ENV, "GREETING", UNICODE_VALUE, "")],
            remove: &[],
            expected: &[
                (APP_ENV, "STAGE", "qa", ""),
                (ASG, "MinSize", "2", ""),
                (APP_ENV, "GREETING", UNICODE_VALUE, ""),
            ],
        },
        ConfigurationCase {
            // A value the request sends empty is a value the caller set, not an
            // option it left alone.
            case: "an empty value",
            environment: "env-empty",
            set: &[(APP_ENV, "STAGE", "", "")],
            remove: &[],
            expected: &[(APP_ENV, "STAGE", "", ""), (ASG, "MinSize", "2", "")],
        },
        ConfigurationCase {
            case: "remove an option the configuration carries",
            environment: "env-remove",
            set: &[],
            remove: &[(ASG, "MinSize", "")],
            expected: &[(APP_ENV, "STAGE", "qa", "")],
        },
        ConfigurationCase {
            case: "remove an option the configuration does not carry",
            environment: "env-remove-absent",
            set: &[],
            remove: &[(ASG, "MaxSize", "")],
            expected: INITIAL,
        },
        ConfigurationCase {
            // Removed and set in one request: the value the request sets is what
            // the configuration ends up with.
            case: "remove and set the same option",
            environment: "env-remove-and-set",
            set: &[(ASG, "MinSize", "5", "")],
            remove: &[(ASG, "MinSize", "")],
            expected: &[(APP_ENV, "STAGE", "qa", ""), (ASG, "MinSize", "5", "")],
        },
        ConfigurationCase {
            // `ResourceName` is part of what identifies an option, so a setting
            // carrying one is a DIFFERENT option from the same namespace and name
            // without it.
            case: "an option distinguished by ResourceName alone",
            environment: "env-resource-name",
            set: &[(SCHEDULED, "MinSize", "4", "peak-hours")],
            remove: &[],
            expected: &[
                (APP_ENV, "STAGE", "qa", ""),
                (ASG, "MinSize", "2", ""),
                (SCHEDULED, "MinSize", "4", "peak-hours"),
            ],
        },
        ConfigurationCase {
            case: "several options at once, with a removal",
            environment: "env-several",
            set: &[
                (ASG, "MinSize", "4", ""),
                (ASG, "MaxSize", "6", ""),
                (APP_ENV, "REGION_TAG", "west", ""),
            ],
            remove: &[(APP_ENV, "STAGE", "")],
            expected: &[
                (ASG, "MinSize", "4", ""),
                (ASG, "MaxSize", "6", ""),
                (APP_ENV, "REGION_TAG", "west", ""),
            ],
        },
        ConfigurationCase {
            case: "no configuration change at all",
            environment: "env-no-change",
            set: &[],
            remove: &[],
            expected: INITIAL,
        },
    ];

    for ConfigurationCase { case, environment, set, remove, expected } in rows {
        // One emulator per row, so a row starts from INITIAL and not from
        // whatever the row before it left.
        let emulator = Emulator::new();
        ready_environment(&emulator, case, environment, INITIAL, &[]).await;
        assert_eq!(
            deployed_options(&emulator, case, environment).await,
            expected_options(INITIAL),
            "case {case:?}: the fixture must start from the launched configuration"
        );

        let mut params = owned(&[("EnvironmentName", environment)]);
        params.extend(option_params(set));
        params.extend(remove_params(remove));
        let updated = ok(&emulator, case, "UpdateEnvironment", &as_params(&params)).await;
        assert_eq!(
            updated.value("Status"),
            Some(eb::state::STATUS_UPDATING),
            "case {case:?}: expected the update to be in progress, got body {}",
            updated.body
        );

        // The deployed configuration is what a later reader sees — twice, so a
        // read cannot change what the next one reports.
        for read in 1..=2 {
            assert_eq!(
                deployed_options(&emulator, case, environment).await,
                expected_options(expected),
                "case {case:?}: unexpected deployed configuration on read {read}"
            );
        }
    }
}

/// One row of the release/platform table: the request members beyond the
/// environment's name, the `(member, value)` pairs that must then be reported,
/// and the configuration the environment must carry.
struct DeploymentCase {
    case: &'static str,
    environment: &'static str,
    params: Vec<(String, String)>,
    expected: Vec<(&'static str, String)>,
    expected_settings: Vec<Option4>,
}

#[tokio::test]
async fn an_update_deploys_the_description_release_template_and_platform_it_names() {
    // The platform versions and the template come from the service's OWN
    // published catalogue, so no row names a platform version it does not
    // publish.
    let launched = SOLUTION_STACK.clone();
    let moved_to = published_stack(2);
    let moved_arn = moved_to.platform_arn(TEST_REGION);
    let template_stack = published_stack(1);
    let template_settings: Vec<Option4> =
        vec![(ASG, "MinSize", "7", ""), (APP_ENV, "FROM_TEMPLATE", "yes", "")];

    let rows = vec![
        DeploymentCase {
            case: "update the description",
            environment: "env-described",
            params: owned(&[("Description", "second release")]),
            expected: vec![
                ("Description", "second release".to_string()),
                ("SolutionStackName", launched.clone()),
            ],
            expected_settings: INITIAL.to_vec(),
        },
        DeploymentCase {
            // "Description, len 0..200": an empty description is one the caller
            // set.
            case: "empty the description",
            environment: "env-undescribed",
            params: owned(&[("Description", "")]),
            expected: vec![("Description", String::new())],
            expected_settings: INITIAL.to_vec(),
        },
        DeploymentCase {
            case: "deploy an application version",
            environment: "env-released",
            params: owned(&[("VersionLabel", "v2")]),
            expected: vec![("VersionLabel", "v2".to_string()), ("SolutionStackName", launched)],
            expected_settings: INITIAL.to_vec(),
        },
        DeploymentCase {
            // "This specifies the platform version that the environment will run
            // after the environment is updated" — and an environment reports its
            // platform version BOTH ways, so the ARN follows the name.
            case: "move to another published platform version",
            environment: "env-restacked",
            params: owned(&[("SolutionStackName", moved_to.name().as_str())]),
            expected: vec![
                ("SolutionStackName", moved_to.name()),
                ("PlatformArn", moved_arn.clone()),
            ],
            expected_settings: INITIAL.to_vec(),
        },
        DeploymentCase {
            // The same move named by the OTHER member.
            case: "move by PlatformArn",
            environment: "env-replatformed",
            params: owned(&[("PlatformArn", moved_arn.as_str())]),
            expected: vec![("SolutionStackName", moved_to.name()), ("PlatformArn", moved_arn)],
            expected_settings: INITIAL.to_vec(),
        },
        DeploymentCase {
            // "Elastic Beanstalk deploys this configuration template to the
            // environment": the template's settings and platform version become
            // the environment's. `TemplateName` keeps naming "the configuration
            // template used to ORIGINALLY launch this environment", which this
            // environment had none of.
            case: "deploy a configuration template",
            environment: "env-templated",
            params: owned(&[("TemplateName", "deployed-template")]),
            expected: vec![
                ("SolutionStackName", template_stack.name()),
                ("TemplateName", String::new()),
            ],
            expected_settings: template_settings.clone(),
        },
        DeploymentCase {
            // A template deployed WITH overrides: the request's settings are
            // applied over the template's.
            case: "deploy a configuration template with an override",
            environment: "env-templated-over",
            params: {
                let mut params = owned(&[("TemplateName", "deployed-template")]);
                params.extend(option_params(&[(ASG, "MinSize", "9", "")]));
                params
            },
            expected: vec![("SolutionStackName", template_stack.name())],
            expected_settings: vec![
                (ASG, "MinSize", "9", ""),
                (APP_ENV, "FROM_TEMPLATE", "yes", ""),
            ],
        },
    ];

    for DeploymentCase { case, environment, params, expected, expected_settings } in &rows {
        let emulator = Emulator::new();
        ready_environment(&emulator, case, environment, INITIAL, &[("Description", "first release")])
            .await;
        // The release and the template a row may deploy, created over HTTP under
        // the same application.
        ok(
            &emulator,
            case,
            "CreateApplicationVersion",
            &[("ApplicationName", APPLICATION), ("VersionLabel", "v2")],
        )
        .await;
        let mut template_params = owned(&[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "deployed-template"),
            ("SolutionStackName", template_stack.name().as_str()),
        ]);
        template_params.extend(option_params(&template_settings));
        ok(&emulator, case, "CreateConfigurationTemplate", &as_params(&template_params)).await;

        let mut request = owned(&[("EnvironmentName", environment)]);
        request.extend(params.iter().cloned());
        let updated = ok(&emulator, case, "UpdateEnvironment", &as_params(&request)).await;
        for (member, value) in expected {
            assert_eq!(
                updated.value(member).unwrap_or_default(),
                value.as_str(),
                "case {case:?}: expected {member} {value:?} in the answer, got body {}",
                updated.body
            );
        }
        // Read back: the answer above is the record, not a projection of the
        // request.
        let members: Vec<&str> = expected.iter().map(|(member, _)| *member).collect();
        let values: Vec<String> = expected.iter().map(|(_, value)| value.clone()).collect();
        assert_eq!(
            described(&emulator, case, environment, &members).await,
            values,
            "case {case:?}: the update must be what a later read reports"
        );
        assert_eq!(
            deployed_options(&emulator, case, environment).await,
            expected_options(expected_settings),
            "case {case:?}: unexpected deployed configuration"
        );
        // What the request did not name is what it was.
        assert_eq!(
            described(&emulator, case, environment, &["ApplicationName"]).await,
            vec![APPLICATION.to_string()],
            "case {case:?}: an update must not move the environment to another application"
        );
    }
}

/// The four members the update lifecycle is read on.
const LIFECYCLE: &[&str] = &["Status", "AbortableOperationInProgress", "Health", "HealthStatus"];

/// Those four members for a `Ready`, idle environment.
fn ready_state() -> Vec<String> {
    vec![
        eb::state::STATUS_READY.to_string(),
        "false".to_string(),
        eb::state::HEALTH_GREEN.to_string(),
        eb::state::HEALTH_STATUS_OK.to_string(),
    ]
}

#[tokio::test]
async fn an_update_starts_the_update_the_clock_then_completes() {
    let case = "update lifecycle";
    let emulator = Emulator::new();
    ready_environment(&emulator, case, "env-lifecycle", INITIAL, &[]).await;
    assert_eq!(
        described(&emulator, case, "env-lifecycle", LIFECYCLE).await,
        ready_state(),
        "case {case:?}: the fixture must be Ready before the update"
    );

    let mut params = owned(&[("EnvironmentName", "env-lifecycle")]);
    params.extend(option_params(&[(ASG, "MinSize", "3", "")]));
    ok(&emulator, case, "UpdateEnvironment", &as_params(&params)).await;

    // The update is in flight: "Updating … in the process of updating its
    // configuration settings", cancellable, and health checks suspended ("health
    // checks are suspended during an UpdateEnvironment … request"). Read twice: a
    // read does not move the environment on by itself.
    let in_flight = vec![
        eb::state::STATUS_UPDATING.to_string(),
        "true".to_string(),
        eb::state::HEALTH_GREY.to_string(),
        eb::state::HEALTH_STATUS_PENDING.to_string(),
    ];
    for read in 1..=2 {
        assert_eq!(
            described(&emulator, case, "env-lifecycle", LIFECYCLE).await,
            in_flight,
            "case {case:?}: unexpected state while the update is in flight on read {read}"
        );
    }

    // An environment already updating is in no state to take a second update.
    let refused = emulator
        .call("UpdateEnvironment", &[("EnvironmentName", "env-lifecycle")])
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 updating an updating environment, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got body {}",
        refused.body
    );

    // The return to `Ready` is what the clock owes, and nothing is cancellable
    // once it is due.
    std::thread::sleep(PAST_DUE);
    assert_eq!(
        described(&emulator, case, "env-lifecycle", LIFECYCLE).await,
        ready_state(),
        "case {case:?}: the update the clock owed must have completed"
    );

    // Both halves of the activity were recorded, under ONE request id.
    let events = ok(
        &emulator,
        case,
        "DescribeEvents",
        &[("EnvironmentName", "env-lifecycle")],
    )
    .await;
    let recorded: Vec<(String, String)> = events
        .elements_at("Events/member")
        .iter()
        .map(|event| {
            (
                event.value("Message").unwrap_or_default().to_string(),
                event.value("RequestId").unwrap_or_default().to_string(),
            )
        })
        .filter(|(message, _)| message.starts_with("updateEnvironment"))
        .collect();
    let messages: Vec<&String> = recorded.iter().map(|(message, _)| message).collect();
    for message in ["updateEnvironment is starting.", "updateEnvironment completed successfully."] {
        assert!(
            messages.iter().any(|recorded| *recorded == message),
            "case {case:?}: expected the event {message:?} in {messages:?}"
        );
    }
    assert_eq!(
        recorded.len(),
        2,
        "case {case:?}: expected both update events, got {recorded:?}"
    );
    assert_eq!(
        recorded.first().map(|(_, request)| request),
        recorded.last().map(|(_, request)| request),
        "case {case:?}: both events belong to the request that started the update, got {recorded:?}"
    );
}

#[tokio::test]
async fn an_update_follows_the_configuration_without_reprovisioning_the_environment() {
    let case = "resources follow the configuration";
    let emulator = Emulator::new();
    ready_environment(&emulator, case, "env-resources", INITIAL, &[]).await;
    let inventory = |answer: &Answer, path: &str| -> Vec<String> {
        answer.values_at(path).into_iter().map(str::to_string).collect()
    };
    let group_path = "EnvironmentResources/AutoScalingGroups/member/Name";
    let instance_path = "EnvironmentResources/Instances/member/Id";
    let read = [("EnvironmentName", "env-resources")];

    let launched = ok(&emulator, case, "DescribeEnvironmentResources", &read).await;
    let group = inventory(&launched, group_path);
    let instances = inventory(&launched, instance_path);
    assert_eq!(
        instances.len(),
        2,
        "case {case:?}: the launched environment runs its Auto Scaling minimum of 2, got {instances:?}"
    );

    // An update that raises the Auto Scaling minimum grows the group's
    // membership; the group keeps the name it was provisioned under, because an
    // update is not the reprovision `RebuildEnvironment` performs.
    let mut params = owned(&[("EnvironmentName", "env-resources")]);
    params.extend(option_params(&[(ASG, "MinSize", "5", "")]));
    ok(&emulator, case, "UpdateEnvironment", &as_params(&params)).await;

    let after = ok(&emulator, case, "DescribeEnvironmentResources", &read).await;
    let kept = inventory(&after, instance_path);
    assert_eq!(
        kept.len(),
        5,
        "case {case:?}: expected the 5 instances the configuration now asks for, got body {}",
        after.body
    );
    assert_eq!(
        inventory(&after, group_path),
        group,
        "case {case:?}: an update must not recreate the Auto Scaling group"
    );
    for instance in &instances {
        assert!(
            kept.contains(instance),
            "case {case:?}: instance {instance:?} must still be in the group, got {kept:?}"
        );
    }
}

/// One row per request the operation must refuse, with the code it refuses
/// with. Every refusal here is an HTTP 400 client error — the loop asserts that
/// — so only the code varies.
struct RefusalCase {
    case: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
}

/// One refusal row.
fn refusal(case: &'static str, params: Vec<(String, String)>, code: &'static str) -> RefusalCase {
    RefusalCase { case, params, code }
}

#[tokio::test]
async fn bad_update_requests_are_refused_with_the_services_own_codes() {
    let case = "refusals";
    let emulator = Emulator::new();
    let environment_id = ready_environment(&emulator, case, "env-refused", INITIAL, &[]).await;
    ok(
        &emulator,
        case,
        "CreateApplicationVersion",
        &[("ApplicationName", APPLICATION), ("VersionLabel", "v2")],
    )
    .await;
    ok(&emulator, case, "CreateApplication", &[("ApplicationName", "other-app")]).await;
    // A launching environment: created here and not waited out, so its launch is
    // still in progress when the update arrives.
    ok(
        &emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "env-launching"),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ],
    )
    .await;

    // One option setting past the bound a configuration is held to.
    let mut over_bound = owned(&[("EnvironmentName", "env-refused")]);
    for at in 1..=eb::state::MAX_CONFIGURATION_OPTION_SETTINGS + 1 {
        over_bound.push((format!("OptionSettings.member.{at}.Namespace"), APP_ENV.to_string()));
        over_bound.push((format!("OptionSettings.member.{at}.OptionName"), format!("OPTION_{at}")));
        over_bound.push((format!("OptionSettings.member.{at}.Value"), at.to_string()));
    }

    // Rows span the refusal classes: no identifier, an identifier that names
    // nothing (either member), an application that is not the environment's, an
    // environment in no state to be updated, a release / template / platform
    // version that names nothing, each of the three tier members changed ("if you
    // change the tier version, name, or type, Elastic Beanstalk returns
    // InvalidParameterValue error"), both a release and a configuration, and
    // values past a modelled bound — which the MODEL refuses, so they carry the
    // request-validation code rather than this service's own.
    let rows = vec![
        refusal(
            "neither EnvironmentId nor EnvironmentName",
            owned(&[("Description", "nothing named")]),
            "MissingRequiredParameter",
        ),
        refusal(
            "only a GroupName, which names an environment through a manifest",
            owned(&[("GroupName", "dev")]),
            "MissingRequiredParameter",
        ),
        refusal(
            "an EnvironmentName that names nothing",
            owned(&[("EnvironmentName", "ghost-env")]),
            "InvalidParameterValue",
        ),
        refusal(
            "an EnvironmentId that names nothing",
            owned(&[("EnvironmentId", "e-00000000")]),
            "InvalidParameterValue",
        ),
        refusal(
            "an ApplicationName that is not the environment's",
            owned(&[("EnvironmentName", "env-refused"), ("ApplicationName", "other-app")]),
            "InvalidParameterValue",
        ),
        refusal(
            "a Launching environment",
            owned(&[("EnvironmentName", "env-launching")]),
            "InvalidParameterValue",
        ),
        refusal(
            "a VersionLabel the application has no version for",
            owned(&[("EnvironmentName", "env-refused"), ("VersionLabel", "v404")]),
            "InvalidParameterValue",
        ),
        refusal(
            "a TemplateName the application has no template for",
            owned(&[("EnvironmentName", "env-refused"), ("TemplateName", "no-such-template")]),
            "InvalidParameterValue",
        ),
        refusal(
            "a PlatformArn nothing answers to",
            vec![
                ("EnvironmentName".to_string(), "env-refused".to_string()),
                (
                    "PlatformArn".to_string(),
                    format!("arn:aws:elasticbeanstalk:{TEST_REGION}::platform/Nothing/0.0.1"),
                ),
            ],
            "InvalidParameterValue",
        ),
        refusal(
            "a tier Name change",
            owned(&[("EnvironmentName", "env-refused"), ("Tier.Name", eb::state::TIER_NAME_WORKER)]),
            "InvalidParameterValue",
        ),
        refusal(
            // Named by the id rather than the name, so the refusal is not keyed
            // to one identifier.
            "a tier Type change",
            vec![
                ("EnvironmentId".to_string(), environment_id),
                ("Tier.Type".to_string(), "SQS/HTTP".to_string()),
            ],
            "InvalidParameterValue",
        ),
        refusal(
            "a tier Version change",
            owned(&[("EnvironmentName", "env-refused"), ("Tier.Version", "2.0")]),
            "InvalidParameterValue",
        ),
        refusal(
            "both a release and a configuration",
            owned(&[
                ("EnvironmentName", "env-refused"),
                ("VersionLabel", "v2"),
                ("TemplateName", "deployed-template"),
            ]),
            "InvalidParameterCombination",
        ),
        refusal(
            "one option setting past the bound a configuration is held to",
            over_bound,
            "InvalidParameterValue",
        ),
        refusal(
            // The modelled maximum length for `EnvironmentName` is 40, so 41
            // characters is one past the published bound.
            "an EnvironmentName one over the modelled maximum of 40",
            vec![("EnvironmentName".to_string(), "e".repeat(41))],
            "ValidationError",
        ),
        refusal(
            "a Description one over the modelled maximum of 200",
            vec![
                ("EnvironmentName".to_string(), "env-refused".to_string()),
                ("Description".to_string(), "d".repeat(201)),
            ],
            "ValidationError",
        ),
    ];

    for RefusalCase { case, params, code } in &rows {
        let refused = emulator.call("UpdateEnvironment", &as_params(params)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected error code {code}, got body {}",
            refused.body
        );
        assert!(
            !refused.error_message().is_empty(),
            "case {case:?}: a refusal must say what to fix, got body {}",
            refused.body
        );
    }

    // A refused update changed nothing.
    assert_eq!(
        described(&emulator, case, "env-refused", LIFECYCLE).await,
        ready_state(),
        "case {case:?}: a refusal must not move the environment"
    );
    assert_eq!(
        deployed_options(&emulator, case, "env-refused").await,
        expected_options(INITIAL),
        "case {case:?}: a refusal must not change the deployed configuration"
    );
}

/// One row of the restated-tier table: the tier members the request repeats
/// unchanged, and whether it names the environment by its id.
struct TierCase {
    case: &'static str,
    environment: &'static str,
    tier: Vec<(&'static str, &'static str)>,
    by_id: bool,
}

#[tokio::test]
async fn a_tier_restated_unchanged_is_accepted_whichever_identifier_names_the_environment() {
    // What the model refuses is a tier CHANGE, so restating the tier the
    // environment already runs — member by member, and all three at once — is
    // accepted; the rows also span the identifier naming the environment.
    let rows = vec![
        TierCase {
            case: "the tier Name restated",
            environment: "env-tier-name",
            tier: vec![("Tier.Name", eb::state::TIER_NAME_WEB_SERVER)],
            by_id: false,
        },
        TierCase {
            case: "the tier Type restated",
            environment: "env-tier-type",
            tier: vec![("Tier.Type", eb::state::TIER_TYPE_STANDARD)],
            by_id: true,
        },
        TierCase {
            case: "the tier Version restated",
            environment: "env-tier-version",
            tier: vec![("Tier.Version", eb::state::DEFAULT_TIER_VERSION)],
            by_id: false,
        },
        TierCase {
            case: "the whole tier restated",
            environment: "env-tier-whole",
            tier: vec![
                ("Tier.Name", eb::state::TIER_NAME_WEB_SERVER),
                ("Tier.Type", eb::state::TIER_TYPE_STANDARD),
                ("Tier.Version", eb::state::DEFAULT_TIER_VERSION),
            ],
            by_id: true,
        },
    ];

    for TierCase { case, environment, tier, by_id } in &rows {
        let emulator = Emulator::new();
        let environment_id = ready_environment(&emulator, case, environment, INITIAL, &[]).await;
        let mut params: Vec<(&str, &str)> = if *by_id {
            vec![("EnvironmentId", environment_id.as_str())]
        } else {
            vec![("EnvironmentName", environment)]
        };
        params.extend(tier.iter().copied());
        let updated = ok(&emulator, case, "UpdateEnvironment", &params).await;
        assert_eq!(
            updated.value("Status"),
            Some(eb::state::STATUS_UPDATING),
            "case {case:?}: an unchanged tier must be accepted, got body {}",
            updated.body
        );
        assert_eq!(
            updated.value("Tier/Name").unwrap_or_default(),
            eb::state::TIER_NAME_WEB_SERVER,
            "case {case:?}: the environment keeps the tier it runs, got body {}",
            updated.body
        );
    }
}
