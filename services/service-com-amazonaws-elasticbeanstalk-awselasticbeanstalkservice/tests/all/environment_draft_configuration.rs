// SPDX-License-Identifier: CC0-1.0

//! The DRAFT configuration of a running environment and
//! `DeleteEnvironmentConfiguration`, driven end to end over this service's
//! awsQuery surface.
//!
//! RULE under test: an update that changes a running environment's configuration
//! creates a DRAFT configuration set, so `DescribeConfigurationSettings` for
//! that environment reports TWO setting descriptions with different
//! `DeploymentStatus` values — the `deployed` one the environment is running and
//! the `pending` one being deployed (`UpdateEnvironment`: "when updating the
//! configuration settings to a new template or individual settings, a draft
//! configuration is created and DescribeConfigurationSettings for this
//! environment returns two setting descriptions with different DeploymentStatus
//! values"; `DescribeConfigurationSettings`: "it is possible to receive two sets
//! of setting descriptions. One is the deployed configuration set, and the other
//! is a draft configuration of an environment that is either in the process of
//! deployment or that failed to deploy"). The draft becomes the deployed
//! configuration when the clock completes the deployment; a deployment cancelled
//! by `AbortEnvironmentUpdate` leaves its draft `failed` ("failed: This is a
//! draft configuration that failed to deploy"), and either way the draft
//! "remains in existence until it is deleted with"
//! `DeleteEnvironmentConfiguration` — which deletes the draft and nothing else,
//! leaving the deployed configuration exactly as it was.
//!
//! Each table row is a different point in that class — which part of the
//! configuration the update changes, how many options it carries, whether the
//! change comes from a template or from the request, whether it changes the
//! configuration at all — never a renamed copy of the row before it.
//!
//! The HTTP harness (`Emulator`, `Answer`) is `all/lifecycle.rs`'s and the
//! option-setting encoder and reader are `all/configuration_templates.rs`'s, so
//! no second copy of either can drift. Every assertion reads the service back
//! over HTTP through an implemented reader; the one precondition written into
//! the store directly is an in-flight update with NOTHING owed by the clock, for
//! the abort rows — the same seam, and for the same reason, as
//! `all/abort_environment_update.rs`: an abort driven after a real update would
//! race that update's own completion.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::configuration_templates::{
    as_params, expected_options, option_params, published_stack, reported_options, Option4,
};
use crate::lifecycle::{Answer, Emulator, PAST_DUE, SOLUTION_STACK};

/// The application every environment below is launched under.
const APPLICATION: &str = "draft-config-app";

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

/// `params` as owned pairs, for a request built out of several pieces.
fn owned(params: &[(&str, &str)]) -> Vec<(String, String)> {
    params
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// Create [`APPLICATION`] and launch `environment` under it with [`INITIAL`],
/// letting the launch the clock owed complete so the environment is `Ready`.
async fn ready_environment(emulator: &Emulator, case: &str, environment: &str) {
    ok(
        emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let mut params: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), environment.to_string()),
        ("SolutionStackName".to_string(), SOLUTION_STACK.to_string()),
    ];
    params.extend(option_params(INITIAL));
    ok(emulator, case, "CreateEnvironment", &as_params(&params)).await;
    std::thread::sleep(PAST_DUE);
}

/// One configuration-settings description, as `DescribeConfigurationSettings`
/// reports it: its `DeploymentStatus`, the platform version it belongs to, and
/// its option settings — read description by description, so a description's
/// status and its options cannot be re-aligned across descriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedSet {
    deployment_status: String,
    solution_stack_name: String,
    options: Vec<(String, String, String, String)>,
}

/// Every configuration-settings description the service reports for
/// `environment`, in document order.
async fn configuration_sets(emulator: &Emulator, case: &str, environment: &str) -> Vec<ReportedSet> {
    let described = ok(
        emulator,
        case,
        "DescribeConfigurationSettings",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", environment),
        ],
    )
    .await;
    described
        .elements_at("ConfigurationSettings/member")
        .iter()
        .map(|member| {
            // Every description is a configuration set OF THIS ENVIRONMENT, so
            // each one names it and names no template.
            assert_eq!(
                member.value("EnvironmentName"),
                Some(environment),
                "case {case:?}: a description must name the environment, got {}",
                member.body
            );
            assert_eq!(
                member.value("TemplateName"),
                None,
                "case {case:?}: an environment's configuration set is no template, got {}",
                member.body
            );
            ReportedSet {
                deployment_status: member
                    .value("DeploymentStatus")
                    .unwrap_or_default()
                    .to_string(),
                solution_stack_name: member
                    .value("SolutionStackName")
                    .unwrap_or_default()
                    .to_string(),
                options: reported_options(member),
            }
        })
        .collect()
}

/// The one description reporting "the configuration that is currently deployed
/// to the associated running environment".
async fn deployed(emulator: &Emulator, case: &str, environment: &str) -> ReportedSet {
    let reported = configuration_sets(emulator, case, environment).await;
    let deployed: Vec<&ReportedSet> = reported
        .iter()
        .filter(|set| set.deployment_status == eb::state::DEPLOYMENT_STATUS_DEPLOYED)
        .collect();
    assert_eq!(
        deployed.len(),
        1,
        "case {case:?}: a running environment reports exactly one deployed \
         configuration set, got {reported:?}"
    );
    deployed[0].clone()
}

/// The DRAFT description `environment` carries, or `None` when it carries none.
async fn draft(emulator: &Emulator, case: &str, environment: &str) -> Option<ReportedSet> {
    let reported = configuration_sets(emulator, case, environment).await;
    let drafts: Vec<ReportedSet> = reported
        .iter()
        .filter(|set| set.deployment_status != eb::state::DEPLOYMENT_STATUS_DEPLOYED)
        .cloned()
        .collect();
    assert!(
        drafts.len() <= 1,
        "case {case:?}: an environment carries at most one draft configuration, got {drafts:?}"
    );
    drafts.into_iter().next()
}

/// Delete `environment`'s draft configuration over HTTP.
async fn delete_draft(emulator: &Emulator, case: &str, environment: &str) -> Answer {
    ok(
        emulator,
        case,
        "DeleteEnvironmentConfiguration",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", environment),
        ],
    )
    .await
}

/// One row of the draft table: the update's request members beyond the
/// environment's name, and the configuration the draft it creates must hold —
/// `None` for an update that deploys no configuration change at all.
struct DraftCase {
    case: &'static str,
    environment: &'static str,
    /// The option settings the update sends.
    set: &'static [Option4],
    /// The configuration template the update deploys, with the settings that
    /// template holds, for a row that changes the configuration that way.
    template: Option<(&'static str, &'static [Option4])>,
    /// The configuration the draft must hold, or `None` for no draft.
    expected_draft: Option<&'static [Option4]>,
}

/// The settings the template rows below create their template with.
const TEMPLATE_SETTINGS: &[Option4] = &[(ASG, "MinSize", "7", ""), (APP_ENV, "TIER", "gold", "")];

#[tokio::test]
async fn a_configuration_change_becomes_the_environments_draft_until_the_clock_deploys_it() {
    // Rows differ along what the rule generalises over: how many options the
    // change carries, whether it adds, replaces or empties one, whether the
    // option is distinguished by `ResourceName` alone, whether the new
    // configuration comes from a template rather than from the request, and
    // whether the request changes the configuration at all.
    let rows = &[
        DraftCase {
            case: "one option replaced",
            environment: "draft-replace",
            set: &[(ASG, "MinSize", "4", "")],
            template: None,
            expected_draft: Some(&[(APP_ENV, "STAGE", "qa", ""), (ASG, "MinSize", "4", "")]),
        },
        DraftCase {
            case: "one option added",
            environment: "draft-add",
            set: &[(APP_ENV, "FEATURE", "on", "")],
            template: None,
            expected_draft: Some(&[
                (APP_ENV, "STAGE", "qa", ""),
                (ASG, "MinSize", "2", ""),
                (APP_ENV, "FEATURE", "on", ""),
            ]),
        },
        DraftCase {
            // A value the request sends empty is a value the caller set, so it
            // is a configuration change like any other.
            case: "one option emptied",
            environment: "draft-emptied",
            set: &[(APP_ENV, "STAGE", "", "")],
            template: None,
            expected_draft: Some(&[(APP_ENV, "STAGE", "", ""), (ASG, "MinSize", "2", "")]),
        },
        DraftCase {
            // `ResourceName` is part of what identifies an option, so a setting
            // carrying one is a DIFFERENT option from the same namespace and
            // name without it.
            case: "an option distinguished by ResourceName alone",
            environment: "draft-resource-named",
            set: &[(SCHEDULED, "MinSize", "2", "peak-hours")],
            template: None,
            expected_draft: Some(&[
                (APP_ENV, "STAGE", "qa", ""),
                (ASG, "MinSize", "2", ""),
                (SCHEDULED, "MinSize", "2", "peak-hours"),
            ]),
        },
        DraftCase {
            case: "several options at once",
            environment: "draft-several",
            set: &[
                (ASG, "MinSize", "3", ""),
                (ASG, "MaxSize", "9", ""),
                (APP_ENV, "REGION_TAG", "west", ""),
            ],
            template: None,
            expected_draft: Some(&[
                (APP_ENV, "STAGE", "qa", ""),
                (ASG, "MinSize", "3", ""),
                (ASG, "MaxSize", "9", ""),
                (APP_ENV, "REGION_TAG", "west", ""),
            ]),
        },
        DraftCase {
            // "Updating the configuration settings to a new template": the
            // template's own settings are the configuration being deployed.
            case: "a whole configuration template deployed",
            environment: "draft-templated",
            set: &[],
            template: Some(("draft-template", TEMPLATE_SETTINGS)),
            expected_draft: Some(TEMPLATE_SETTINGS),
        },
        DraftCase {
            // "Updating a running environment with ANY configuration changes
            // creates a draft configuration set" — and an update that restates
            // the configuration the environment already runs changes none.
            case: "the configuration restated unchanged",
            environment: "draft-unchanged",
            set: INITIAL,
            template: None,
            expected_draft: None,
        },
        DraftCase {
            case: "no configuration member at all",
            environment: "draft-none",
            set: &[],
            template: None,
            expected_draft: None,
        },
    ];

    for DraftCase {
        case,
        environment,
        set,
        template,
        expected_draft,
    } in rows
    {
        // One emulator per row, so a row starts from INITIAL and not from
        // whatever the row before it left.
        let emulator = Emulator::new();
        ready_environment(&emulator, case, environment).await;
        assert_eq!(
            deployed(&emulator, case, environment).await.options,
            expected_options(INITIAL),
            "case {case:?}: the fixture must start from the launched configuration"
        );
        assert_eq!(
            draft(&emulator, case, environment).await,
            None,
            "case {case:?}: a launched environment carries no draft configuration"
        );

        let mut params = owned(&[("EnvironmentName", environment)]);
        params.extend(option_params(set));
        if let Some((template_name, settings)) = template {
            let mut template_params = owned(&[
                ("ApplicationName", APPLICATION),
                ("TemplateName", template_name),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ]);
            template_params.extend(option_params(settings));
            ok(
                &emulator,
                case,
                "CreateConfigurationTemplate",
                &as_params(&template_params),
            )
            .await;
            params.push(("TemplateName".to_string(), (*template_name).to_string()));
        }
        ok(&emulator, case, "UpdateEnvironment", &as_params(&params)).await;

        // The update is in flight: the environment still runs the configuration
        // it had, and the one being deployed is its `pending` draft. Read twice,
        // so a read cannot change what the next one reports.
        for read in 1..=2 {
            assert_eq!(
                deployed(&emulator, case, environment).await.options,
                expected_options(INITIAL),
                "case {case:?}: the deployed configuration must not move while the \
                 deployment is in flight, on read {read}"
            );
            assert_eq!(
                draft(&emulator, case, environment).await.map(|set| (
                    set.deployment_status,
                    set.options
                )),
                expected_draft.map(|settings| (
                    eb::state::DEPLOYMENT_STATUS_PENDING.to_string(),
                    expected_options(settings)
                )),
                "case {case:?}: unexpected draft configuration on read {read}"
            );
        }

        // The deployment the clock owed has completed: the draft is the
        // configuration the environment runs, and it is a draft no longer.
        std::thread::sleep(PAST_DUE);
        assert_eq!(
            deployed(&emulator, case, environment).await.options,
            expected_options(expected_draft.unwrap_or(INITIAL)),
            "case {case:?}: the completed deployment must be what a later read reports"
        );
        assert_eq!(
            draft(&emulator, case, environment).await,
            None,
            "case {case:?}: a deployed configuration is no draft"
        );
    }
}

/// Hold `environment` where the call that was just made left it, by taking away
/// the status change that call owed the clock, so the reads that follow cannot
/// race it. See this file's module docs: an update carries its own completion
/// one transition span later, and an abort carries its own return to `Ready`.
///
/// `pub(crate)` because the `configuration_set_deployment_status` suite needs
/// the same precondition — an in-flight update with nothing owed — rather than a
/// second copy of this seam.
pub(crate) fn hold_where_it_is(emulator: &Emulator, case: &str, environment: &str) {
    let state = emulator.store();
    let mut store = state.lock();
    let environment_id = store
        .environment_name_index
        .get(environment)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the environment must exist first"));
    let stored = store
        .environments
        .get_mut(&environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the environment must exist first"));
    stored.pending_status = None;
}

/// One row of the delete table: how the environment came to carry the draft the
/// delete removes, and the `DeploymentStatus` that draft reports.
struct DeleteCase {
    case: &'static str,
    environment: &'static str,
    /// Whether the deployment that created the draft is then cancelled.
    cancel: bool,
    expected_status: &'static str,
}

#[tokio::test]
async fn deleting_an_environments_configuration_removes_the_draft_and_leaves_the_deployed_one() {
    // Rows span the two states a draft can be in when it is deleted: waiting to
    // be deployed, and left behind by a deployment that was cancelled.
    let rows = &[
        DeleteCase {
            case: "a draft waiting to be deployed",
            environment: "delete-pending",
            cancel: false,
            expected_status: eb::state::DEPLOYMENT_STATUS_PENDING,
        },
        DeleteCase {
            case: "a draft whose deployment was cancelled",
            environment: "delete-failed",
            cancel: true,
            expected_status: eb::state::DEPLOYMENT_STATUS_FAILED,
        },
    ];

    for DeleteCase {
        case,
        environment,
        cancel,
        expected_status,
    } in rows
    {
        let emulator = Emulator::new();
        ready_environment(&emulator, case, environment).await;
        let mut params = owned(&[("EnvironmentName", environment)]);
        params.extend(option_params(&[(ASG, "MinSize", "6", "")]));
        ok(&emulator, case, "UpdateEnvironment", &as_params(&params)).await;
        // Nothing is owed by the clock from here on, so the draft stays where
        // the update left it however long these reads take.
        hold_where_it_is(&emulator, case, environment);
        if *cancel {
            ok(
                &emulator,
                case,
                "AbortEnvironmentUpdate",
                &[("EnvironmentName", environment)],
            )
            .await;
            hold_where_it_is(&emulator, case, environment);
        }

        let carried = draft(&emulator, case, environment)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the update must have created a draft"));
        assert_eq!(
            carried.deployment_status, *expected_status,
            "case {case:?}: unexpected DeploymentStatus on the draft"
        );
        assert_eq!(
            carried.options,
            expected_options(&[(APP_ENV, "STAGE", "qa", ""), (ASG, "MinSize", "6", "")]),
            "case {case:?}: the draft must hold the configuration being deployed"
        );
        assert_eq!(
            carried.solution_stack_name,
            SOLUTION_STACK.to_string(),
            "case {case:?}: the draft belongs to the platform version it is deployed on"
        );
        let before = deployed(&emulator, case, environment).await;
        assert_eq!(
            before.options,
            expected_options(INITIAL),
            "case {case:?}: the environment still runs the configuration it had"
        );

        delete_draft(&emulator, case, environment).await;

        // The draft is gone and the deployed configuration is untouched —
        // twice, because a read must not change what the next one sees.
        for read in 1..=2 {
            assert_eq!(
                draft(&emulator, case, environment).await,
                None,
                "case {case:?}: the deleted draft must stay deleted, on read {read}"
            );
            assert_eq!(
                deployed(&emulator, case, environment).await,
                before,
                "case {case:?}: deleting the draft must leave the deployed configuration, \
                 on read {read}"
            );
        }

        // Deleting it again deletes nothing and still succeeds: the model
        // declares no error for this operation at all.
        delete_draft(&emulator, case, environment).await;
        assert_eq!(
            draft(&emulator, case, environment).await,
            None,
            "case {case:?}: a second delete must leave no draft"
        );

        // The deletion is in the event log, under one request id, like every
        // other call this service records the activity of.
        let events = ok(
            &emulator,
            case,
            "DescribeEvents",
            &[("EnvironmentName", environment)],
        )
        .await;
        let messages = events.values_at("Events/member/Message");
        for message in [
            "deleteEnvironmentConfiguration is starting.",
            "deleteEnvironmentConfiguration completed successfully.",
        ] {
            assert!(
                messages.contains(&message),
                "case {case:?}: expected the event {message:?} in {messages:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cancelled_deployment_keeps_its_draft_until_it_is_deleted() {
    let case = "a cancelled deployment";
    let emulator = Emulator::new();
    ready_environment(&emulator, case, "cancelled-env").await;
    let mut params = owned(&[("EnvironmentName", "cancelled-env")]);
    params.extend(option_params(&[(APP_ENV, "STAGE", "prod", "")]));
    ok(&emulator, case, "UpdateEnvironment", &as_params(&params)).await;
    hold_where_it_is(&emulator, case, "cancelled-env");

    ok(
        &emulator,
        case,
        "AbortEnvironmentUpdate",
        &[("EnvironmentName", "cancelled-env")],
    )
    .await;

    // The cancelled deployment never reaches the environment, so its draft
    // reports "failed: This is a draft configuration that failed to deploy" —
    // and the clock's completion of the cancellation does not deploy it.
    std::thread::sleep(PAST_DUE);
    let carried = draft(&emulator, case, "cancelled-env")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the cancelled deployment must leave its draft"));
    assert_eq!(
        carried.deployment_status,
        eb::state::DEPLOYMENT_STATUS_FAILED,
        "case {case:?}: a cancelled deployment's draft failed to deploy"
    );
    assert_eq!(
        carried.options,
        expected_options(&[(APP_ENV, "STAGE", "prod", ""), (ASG, "MinSize", "2", "")]),
        "case {case:?}: the draft holds the configuration that was being deployed"
    );
    assert_eq!(
        deployed(&emulator, case, "cancelled-env").await.options,
        expected_options(INITIAL),
        "case {case:?}: the environment keeps the configuration it was running"
    );

    delete_draft(&emulator, case, "cancelled-env").await;
    assert_eq!(
        draft(&emulator, case, "cancelled-env").await,
        None,
        "case {case:?}: the delete must remove the draft the cancellation left"
    );
    assert_eq!(
        deployed(&emulator, case, "cancelled-env").await.options,
        expected_options(INITIAL),
        "case {case:?}: the deployed configuration is not a draft and does not go with one"
    );
}

/// One request the delete must refuse, with the code it refuses it with.
struct RefusalRow {
    case: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
}

#[tokio::test]
async fn deleting_a_configuration_of_an_environment_nothing_answers_to_is_refused() {
    let case = "delete refusals";
    let emulator = Emulator::new();
    ready_environment(&emulator, case, "present-env").await;
    // A second application, so a row can name an environment that exists but
    // belongs elsewhere, and a terminated environment, which runs nothing.
    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", "other-draft-app")],
    )
    .await;
    ok(
        &emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "gone-env"),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ],
    )
    .await;
    std::thread::sleep(PAST_DUE);
    ok(
        &emulator,
        case,
        "TerminateEnvironment",
        &[("EnvironmentName", "gone-env")],
    )
    .await;
    std::thread::sleep(PAST_DUE);

    // Rows span the refusal classes: a name nothing answers to, a name that
    // answers under a DIFFERENT application, an environment that is no longer
    // running, a name past a modelled bound and one under it (which the MODEL
    // refuses, so those carry the request-validation code), and each required
    // member left out.
    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "an EnvironmentName that names nothing",
            params: owned(&[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", "ghost-env"),
            ]),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an environment that belongs to another application",
            params: owned(&[
                ("ApplicationName", "other-draft-app"),
                ("EnvironmentName", "present-env"),
            ]),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an environment that is no longer running",
            params: owned(&[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", "gone-env"),
            ]),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an ApplicationName that names nothing",
            params: owned(&[
                ("ApplicationName", "no-such-app"),
                ("EnvironmentName", "present-env"),
            ]),
            code: "InvalidParameterValue",
        },
        RefusalRow {
            // The modelled maximum length for `EnvironmentName` is 40.
            case: "an EnvironmentName one over the modelled maximum of 40",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), "e".repeat(41)),
            ],
            code: "ValidationError",
        },
        RefusalRow {
            // The modelled minimum length for `EnvironmentName` is 4.
            case: "an EnvironmentName one under the modelled minimum of 4",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), "env".to_string()),
            ],
            code: "ValidationError",
        },
        RefusalRow {
            case: "no EnvironmentName at all",
            params: owned(&[("ApplicationName", APPLICATION)]),
            code: "MissingRequiredParameter",
        },
        RefusalRow {
            case: "no ApplicationName at all",
            params: owned(&[("EnvironmentName", "present-env")]),
            code: "MissingRequiredParameter",
        },
    ];

    for RefusalRow { case: row, params, code } in &rows {
        let refused = emulator
            .call("DeleteEnvironmentConfiguration", &as_params(params))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {row:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {row:?}: expected error code {code}, got body {}",
            refused.body
        );
        assert!(
            refused.value("Error/Message").is_some(),
            "case {row:?}: a refusal must say what to fix, got body {}",
            refused.body
        );
    }

    // And no refusal touched the environment that does exist: it still runs the
    // configuration it was launched with.
    assert_eq!(
        deployed(&emulator, case, "present-env").await.options,
        expected_options(INITIAL),
        "case {case:?}: a refused delete must change nothing"
    );
}

#[tokio::test]
async fn a_configuration_template_is_reported_as_deployed_and_carries_no_draft() {
    // The OTHER configuration set `DescribeConfigurationSettings` reports is a
    // template: it names no environment, carries no draft beside it, and the
    // one set it reports is the configuration the saved template holds.
    let case = "a template is no environment";
    let emulator = Emulator::new();
    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let stack = published_stack(1);
    let mut params = owned(&[
        ("ApplicationName", APPLICATION),
        ("TemplateName", "saved-config"),
    ]);
    params.push(("SolutionStackName".to_string(), stack.name()));
    params.extend(option_params(&[(ASG, "MaxSize", "8", "")]));
    ok(
        &emulator,
        case,
        "CreateConfigurationTemplate",
        &as_params(&params),
    )
    .await;

    let described = ok(
        &emulator,
        case,
        "DescribeConfigurationSettings",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "saved-config"),
        ],
    )
    .await;
    let reported = described.elements_at("ConfigurationSettings/member");
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: a template is one configuration set, got {}",
        described.body
    );
    assert_eq!(
        reported[0].value("DeploymentStatus"),
        Some(eb::state::DEPLOYMENT_STATUS_DEPLOYED),
        "case {case:?}: a stored template reports its configuration set as deployed, got {}",
        described.body
    );
    assert_eq!(
        reported_options(&reported[0]),
        expected_options(&[(ASG, "MaxSize", "8", "")]),
        "case {case:?}: the template's own settings must be reported, got {}",
        described.body
    );
}
