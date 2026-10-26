// SPDX-License-Identifier: CC0-1.0

//! An environment's MANAGED ACTIONS — `DescribeEnvironmentManagedActions`
//! ("lists an environment's upcoming and in-progress managed actions") and the
//! `ApplyEnvironmentManagedAction` that runs one of them — driven end to end
//! over the service's awsQuery HTTP surface.
//!
//! RULE under test: a managed action is maintenance the SERVICE has scheduled
//! for an environment in the environment's own maintenance window, so an
//! environment's managed actions are exactly the maintenance its managed-update
//! configuration asks for and there is something to do — a platform update while
//! it runs a release its platform branch has superseded and its update level
//! admits the newer one, and an instance replacement while weekly replacement is
//! asked for — each scheduled in the first window that opens once it is due, each
//! reported under one stable id and window, and reported until the service has
//! carried it out. Applying one runs it immediately: it reports `Running`, the
//! environment carries the change it made once it finishes, and the action is
//! then in the environment's managed-action history and in no other state that
//! can be applied again.
//!
//! Nothing here reaches into the store: every precondition is created over the
//! wire and every assertion is made over what the wire answered, so a handler
//! that fabricated an action, kept none, reported one it had already carried out
//! or changed an answer a later caller depends on fails these tests.
//!
//! Each test is a TABLE over the class the rule generalises over: which
//! managed-update options the configuration carries, which platform branch and
//! which of its releases the environment runs, which update level is allowed,
//! which maintenance window is stated, and which identifier names the
//! environment.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{instant, Answer, Emulator, PAST_DUE, TEST_REGION};

/// The managed-update configuration options this suite sets, from Elastic
/// Beanstalk's published configuration-option reference — the same namespaces
/// and option names `DescribeConfigurationOptions` publishes, which
/// [`every_option_this_suite_sets_is_published_by_the_service`] checks this list
/// against so it cannot drift from the service's own catalog.
const MANAGED_ACTIONS: &str = "aws:elasticbeanstalk:managedactions";
/// See [`MANAGED_ACTIONS`].
const PLATFORM_UPDATE: &str = "aws:elasticbeanstalk:managedactions:platformupdate";
/// See [`MANAGED_ACTIONS`].
const MANAGED_ACTIONS_ENABLED: &str = "ManagedActionsEnabled";
/// See [`MANAGED_ACTIONS`].
const PREFERRED_START_TIME: &str = "PreferredStartTime";
/// See [`MANAGED_ACTIONS`].
const INSTANCE_REFRESH_ENABLED: &str = "InstanceRefreshEnabled";
/// See [`MANAGED_ACTIONS`].
const UPDATE_LEVEL: &str = "UpdateLevel";

/// The two `ActionType` values a managed action of this service has, from the
/// modelled set (`eb::ops::types::ACTION_TYPE_VALUES`).
const PLATFORM_UPDATE_ACTION: &str = "PlatformUpdate";
/// See [`PLATFORM_UPDATE_ACTION`].
const INSTANCE_REFRESH_ACTION: &str = "InstanceRefresh";

/// The `ActionStatus` an action that has not started carries, and the one it
/// carries while it runs, from the modelled set
/// (`eb::ops::types::ACTION_STATUS_VALUES`).
const SCHEDULED: &str = "Scheduled";
/// See [`SCHEDULED`].
const RUNNING: &str = "Running";

/// The message `ManagedActionInvalidStateException` carries: the service's own
/// sentence for an action that cannot be applied, which names the states it
/// cannot be applied out of rather than the one it is in.
const INVALID_STATE_MESSAGE: &str =
    "Managed action is scheduled to execute imminently, is going to be retried, or is already \
     running";

/// Seconds in a day and in a week, for the schedule this suite expects. A
/// maintenance window is stated in UTC, where a day is exactly this long.
const DAY: i64 = 24 * 60 * 60;
/// See [`DAY`].
const WEEK: i64 = 7 * DAY;

/// A maintenance window this suite states, as `PreferredStartTime` states one
/// (`Tue:09:00`).
const WINDOW: &str = "Tue:09:00";

/// The epoch second the weekly window `preferred_start_time` next opens at, at
/// or after `not_before`.
///
/// This suite's OWN statement of the schedule, so it does not read the
/// service's: the weekday names are the ones the option's published form allows,
/// 1970-01-01 was a Thursday — the fourth day of a week that starts on Monday —
/// and the window is stepped a week at a time until it is not in the past.
fn window_start(preferred_start_time: &str, not_before: i64) -> i64 {
    let mut components = preferred_start_time.split(':');
    let day = components.next().expect("a window names a weekday");
    let hour: i64 = components
        .next()
        .and_then(|hour| hour.parse().ok())
        .expect("a window names an hour");
    let minute: i64 = components
        .next()
        .and_then(|minute| minute.parse().ok())
        .expect("a window names a minute");
    let weekday = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        .iter()
        .position(|name| *name == day)
        .expect("a weekday the option allows");
    let weekday = i64::try_from(weekday).expect("a weekday index fits");
    let days = not_before.div_euclid(DAY);
    let into_the_week = (days + 3).rem_euclid(7);
    let mut opened = (days - into_the_week + weekday) * DAY + hour * 60 * 60 + minute * 60;
    while opened < not_before {
        opened += WEEK;
    }
    opened
}

/// Whether `stated` states a maintenance window at all: the published form is
/// one of the weekday names, a two-digit hour inside a day and a two-digit
/// minute inside an hour, separated by colons. This suite's own reading of that
/// form, so it does not read the service's.
fn states_a_window(stated: &str) -> bool {
    let fields: Vec<&str> = stated.split(':').collect();
    let two_digits = |field: &str, highest: u8| {
        field.len() == 2
            && field.bytes().all(|digit| digit.is_ascii_digit())
            && field.parse::<u8>().is_ok_and(|value| value <= highest)
    };
    match fields.as_slice() {
        [day, hour, minute] => {
            ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].contains(day)
                && two_digits(hour, 23)
                && two_digits(minute, 59)
        }
        _ => false,
    }
}

/// The server clock as this suite reads it, in whole epoch seconds.
fn now() -> i64 {
    i64::try_from(
        ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .expect("a clock after the epoch")
            .as_secs(),
    )
    .expect("an epoch second fits")
}

/// One platform branch of the service's own catalog whose current version
/// superseded a release — the state a managed platform update starts from — and
/// the two releases that makes available to an environment.
struct Branch {
    branch_name: &'static str,
    /// The release the environment is launched on, by version and by ARN.
    from_version: String,
    from_arn: String,
    /// The release the branch publishes above it, by version, by solution stack
    /// name and by ARN — what a platform update moves the environment to.
    to_version: String,
    to_name: String,
    to_arn: String,
}

/// The first `count` branches of the catalog that publish a superseded release,
/// read off the service's own catalog rather than written out here, so the rules
/// below are asserted over real catalog rows.
fn branches(count: usize) -> Vec<Branch> {
    let branches: Vec<Branch> = eb::state::SOLUTION_STACKS
        .iter()
        .filter_map(|stack| {
            let from_version = stack.superseded_version()?;
            Some(Branch {
                branch_name: stack.branch_name,
                from_arn: eb::state::platform_arn_of(
                    TEST_REGION,
                    stack.branch_name,
                    &from_version,
                ),
                from_version,
                to_version: stack.platform_version.to_string(),
                to_name: stack.name(),
                to_arn: stack.platform_arn(TEST_REGION),
            })
        })
        .take(count)
        .collect();
    assert_eq!(
        branches.len(),
        count,
        "case \"catalog spread\": expected {count} branches publishing a superseded release"
    );
    branches
}

/// The awsQuery parameters for the configuration `settings`, each a
/// `(Namespace, OptionName, Value)` triple, as the `OptionSettings` list carries
/// them.
fn option_params(settings: &[(&str, &str, &str)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (position, (namespace, option_name, value)) in settings.iter().enumerate() {
        let at = position + 1;
        params.push((
            format!("OptionSettings.member.{at}.Namespace"),
            (*namespace).to_string(),
        ));
        params.push((
            format!("OptionSettings.member.{at}.OptionName"),
            (*option_name).to_string(),
        ));
        params.push((
            format!("OptionSettings.member.{at}.Value"),
            (*value).to_string(),
        ));
    }
    params
}

/// `params` as the borrowed pairs the harness sends.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Create the application `application`.
async fn application(emulator: &Emulator, application: &str) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", application)])
        .await;
    assert_eq!(
        created.status, 200,
        "fixture {application:?}: CreateApplication answered {} with body {}",
        created.status, created.body
    );
}

/// Launch the environment `name` of `application` on the platform version
/// `platform_arn` with the configuration `settings`, and wait out the launch the
/// clock owes so it has reached `Ready`. Answers the launch's own response, which
/// carries the environment's id and the instant it was created.
async fn launch(
    emulator: &Emulator,
    application: &str,
    name: &str,
    platform_arn: &str,
    settings: &[(&str, &str, &str)],
) -> Answer {
    let mut params = vec![
        ("ApplicationName".to_string(), application.to_string()),
        ("EnvironmentName".to_string(), name.to_string()),
        ("PlatformArn".to_string(), platform_arn.to_string()),
    ];
    params.extend(option_params(settings));
    let launched = emulator
        .call("CreateEnvironment", &borrowed(&params))
        .await;
    assert_eq!(
        launched.status, 200,
        "fixture {name:?}: CreateEnvironment answered {} with body {}",
        launched.status, launched.body
    );
    std::thread::sleep(PAST_DUE);
    launched
}

/// One managed action as a response reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    action_id: String,
    action_description: String,
    action_type: String,
    status: String,
    window_start_time: String,
}

/// Every managed action `answer` reports, in the order it reported them.
fn reported(answer: &Answer) -> Vec<Reported> {
    answer
        .elements_at("ManagedActions/member")
        .into_iter()
        .map(|member| {
            let value = |name: &str| member.value(name).unwrap_or_default().to_string();
            Reported {
                action_id: value("ActionId"),
                action_description: value("ActionDescription"),
                action_type: value("ActionType"),
                status: value("Status"),
                window_start_time: value("WindowStartTime"),
            }
        })
        .collect()
}

/// `DescribeEnvironmentManagedActions` for `name`, optionally narrowed to one
/// `Status`.
async fn describe(emulator: &Emulator, name: &str, status: Option<&str>) -> Answer {
    let mut params = vec![("EnvironmentName", name)];
    if let Some(status) = status {
        params.push(("Status", status));
    }
    let answer = emulator
        .call("DescribeEnvironmentManagedActions", &params)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected 200 from DescribeEnvironmentManagedActions, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// The action types `name`'s managed actions carry, in the reported order.
async fn action_types(emulator: &Emulator, name: &str) -> Vec<String> {
    reported(&describe(emulator, name, None).await)
        .into_iter()
        .map(|action| action.action_type)
        .collect()
}

/// `8-4-4-4-12` lowercase hexadecimal, the UUID form the service issues an
/// `ActionId` in.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && value == value.to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// Which managed actions an environment has
// ---------------------------------------------------------------------------

/// RULE: an environment's managed actions are the maintenance its managed-update
/// configuration asks the service to perform and that there is something to do —
/// nothing at all for a configuration that asks for no managed updates.
#[tokio::test]
async fn an_environments_managed_actions_are_the_maintenance_its_configuration_asks_for() {
    /// One configuration, and the action types it has the service schedule, in
    /// the order their windows open.
    struct Case {
        case: &'static str,
        managed_actions_enabled: bool,
        /// Whether the environment is launched on the release its branch
        /// superseded (so a platform update is available) or on the current one.
        superseded: bool,
        instance_refresh: bool,
        expected: Vec<&'static str>,
    }
    let cases = [
        Case {
            case: "no managed updates, on a superseded release",
            managed_actions_enabled: false,
            superseded: true,
            instance_refresh: false,
            expected: vec![],
        },
        Case {
            case: "managed updates, on a superseded release",
            managed_actions_enabled: true,
            superseded: true,
            instance_refresh: false,
            expected: vec![PLATFORM_UPDATE_ACTION],
        },
        Case {
            case: "managed updates, already on the branch's current release",
            managed_actions_enabled: true,
            superseded: false,
            instance_refresh: false,
            expected: vec![],
        },
        Case {
            // The update is due in the window that opens next; the instances
            // were launched with the environment, so their weekly replacement
            // is due a week later — which is the order the two are reported in.
            case: "managed updates with weekly instance replacement, on a superseded release",
            managed_actions_enabled: true,
            superseded: true,
            instance_refresh: true,
            expected: vec![PLATFORM_UPDATE_ACTION, INSTANCE_REFRESH_ACTION],
        },
        Case {
            case: "weekly instance replacement with nothing to update",
            managed_actions_enabled: true,
            superseded: false,
            instance_refresh: true,
            expected: vec![INSTANCE_REFRESH_ACTION],
        },
        Case {
            case: "weekly instance replacement asked for without managed updates",
            managed_actions_enabled: false,
            superseded: true,
            instance_refresh: true,
            expected: vec![],
        },
    ];

    // Two branches of the catalog, so no rule here rests on one catalog row.
    for branch in branches(2) {
        let emulator = Emulator::new();
        application(&emulator, "managed-app").await;
        for (index, expectation) in cases.iter().enumerate() {
            let case = &format!("{} on {}", expectation.case, branch.branch_name);
            let name = format!("managed-env-{index}");
            let settings = vec![
                (
                    MANAGED_ACTIONS,
                    MANAGED_ACTIONS_ENABLED,
                    if expectation.managed_actions_enabled {
                        "true"
                    } else {
                        "false"
                    },
                ),
                (MANAGED_ACTIONS, PREFERRED_START_TIME, WINDOW),
                (
                    PLATFORM_UPDATE,
                    INSTANCE_REFRESH_ENABLED,
                    if expectation.instance_refresh {
                        "true"
                    } else {
                        "false"
                    },
                ),
            ];
            let arn = if expectation.superseded {
                &branch.from_arn
            } else {
                &branch.to_arn
            };
            launch(&emulator, "managed-app", &name, arn, &settings).await;

            let answer = describe(&emulator, &name, None).await;
            let actions = reported(&answer);
            let types: Vec<&str> = actions
                .iter()
                .map(|action| action.action_type.as_str())
                .collect();
            assert_eq!(
                types, expectation.expected,
                "case {case:?}: expected the managed actions {:?}, got {actions:?} in body {}",
                expectation.expected, answer.body
            );
            for action in &actions {
                assert_eq!(
                    action.status, SCHEDULED,
                    "case {case:?}: an action nobody has applied is {SCHEDULED}, got {action:?}"
                );
                assert!(
                    is_uuid_shaped(&action.action_id),
                    "case {case:?}: expected a UUID ActionId, got {action:?}"
                );
                assert!(
                    !action.action_description.is_empty(),
                    "case {case:?}: every managed action is described, got {action:?}"
                );
            }
            // The platform update names the two releases it moves between, so a
            // client can tell which update is scheduled.
            if let Some(update) = actions
                .iter()
                .find(|action| action.action_type == PLATFORM_UPDATE_ACTION)
            {
                for version in [&branch.from_version, &branch.to_version] {
                    assert!(
                        update.action_description.contains(version.as_str()),
                        "case {case:?}: the update's description names the versions it moves \
                         between, got {update:?}"
                    );
                }
            }
        }
    }
}

/// RULE: a platform update is scheduled only where the environment's
/// `UpdateLevel` admits the release it would move to — `patch` admits only a
/// release inside the version line the environment already runs, and this
/// catalog's superseded release is a MINOR step below its branch's current one.
#[tokio::test]
async fn a_platform_update_is_scheduled_only_up_to_the_configured_update_level() {
    // Rows: the option's two published values and the unset case, which is held
    // to neither level.
    let cases: [(&str, Option<&str>, bool); 3] = [
        ("no update level stated", None, true),
        ("minor updates allowed", Some("minor"), true),
        ("only patch updates allowed", Some("patch"), false),
    ];

    for branch in branches(2) {
        let emulator = Emulator::new();
        application(&emulator, "level-app").await;
        for (index, (case, level, scheduled)) in cases.iter().enumerate() {
            let case = &format!("{case} on {}", branch.branch_name);
            let name = format!("level-env-{index}");
            let mut settings = vec![
                (MANAGED_ACTIONS, MANAGED_ACTIONS_ENABLED, "true"),
                (MANAGED_ACTIONS, PREFERRED_START_TIME, WINDOW),
            ];
            if let Some(level) = level {
                settings.push((PLATFORM_UPDATE, UPDATE_LEVEL, level));
            }
            launch(
                &emulator,
                "level-app",
                &name,
                &branch.from_arn,
                &settings,
            )
            .await;

            let types = action_types(&emulator, &name).await;
            let expected: Vec<&str> = if *scheduled {
                vec![PLATFORM_UPDATE_ACTION]
            } else {
                vec![]
            };
            assert_eq!(
                types, expected,
                "case {case:?}: expected {expected:?}, got {types:?} for the update from {} to {}",
                branch.from_version, branch.to_version
            );
        }
    }
}

/// RULE: a scheduled action's `WindowStartTime` is the first opening of the
/// environment's maintenance window once the action is due — the stated
/// `PreferredStartTime` where the configuration states one, and the week of the
/// environment's own launch where it does not — and reading the action again
/// reports the same window and the same id.
#[tokio::test]
async fn a_scheduled_action_opens_in_the_configured_maintenance_window() {
    // Rows span the week, the clock and the two ends of a day, the case that
    // states no window at all, and the values that state NO window because they
    // are not the weekday-and-time form the option publishes: nothing validates
    // a configuration option's value where it is written, so each of these
    // reaches the schedule from the request and leaves the environment
    // maintained on the week of its own launch, exactly as stating none does.
    let windows = [
        Some("Mon:00:00"),
        Some("Tue:09:00"),
        Some("Wed:12:30"),
        Some("Sun:23:59"),
        None,
        Some("Mon:-9223372036854775000:00"),
        Some("Mon:-01:00"),
        Some("Mon:24:00"),
        Some("Mon:00:60"),
        Some("Mon:9:00"),
        Some("Mon:09"),
        Some("Mon:09:00:00"),
        Some("Funday:09:00"),
        Some(""),
    ];

    let branch = branches(1).pop().expect("one branch");
    let emulator = Emulator::new();
    application(&emulator, "window-app").await;
    for (index, window) in windows.iter().enumerate() {
        let case = &format!("{window:?}");
        let name = format!("window-env-{index}");
        let mut settings = vec![(MANAGED_ACTIONS, MANAGED_ACTIONS_ENABLED, "true")];
        if let Some(window) = window {
            settings.push((MANAGED_ACTIONS, PREFERRED_START_TIME, window));
        }
        let launched = launch(
            &emulator,
            "window-app",
            &name,
            &branch.from_arn,
            &settings,
        )
        .await;
        let created = instant(launched.expect("DateCreated", case), case);

        let answer = describe(&emulator, &name, None).await;
        let actions = reported(&answer);
        assert_eq!(
            actions.len(),
            1,
            "case {case:?}: expected one scheduled platform update, got {actions:?} in body {}",
            answer.body
        );
        let reported_start = instant(&actions[0].window_start_time, case) as i64;
        let expected = match window.filter(|window| states_a_window(window)) {
            // The first opening of the stated window from now: the update is
            // due as soon as the newer release exists.
            Some(window) => window_start(window, now()),
            // No window stated, or none the published form admits: the week
            // counted from the environment's launch.
            None => created as i64 + WEEK,
        };
        assert_eq!(
            reported_start, expected,
            "case {case:?}: expected the window to open at {expected}, got {} ({:?})",
            reported_start, actions[0].window_start_time
        );

        // A read reports what the one before it reported: an action's id and its
        // window are not minted per call.
        let again = reported(&describe(&emulator, &name, None).await);
        assert_eq!(
            again, actions,
            "case {case:?}: a second read must report the same action, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Applying a managed action
// ---------------------------------------------------------------------------

/// RULE: applying a scheduled managed action runs it immediately — it reports
/// `Running` under the same id and window, the environment carries the change it
/// made once it has finished, and the action is then in the environment's
/// managed-action history and reported as upcoming no longer. An action that is
/// not `Scheduled` cannot be applied, and an id naming no action of the
/// environment is refused.
#[tokio::test]
async fn applying_a_scheduled_platform_update_runs_it_and_records_it_in_the_history() {
    for branch in branches(2) {
        let case = branch.branch_name;
        let emulator = Emulator::new();
        application(&emulator, "apply-app").await;
        let name = "apply-env";
        launch(
            &emulator,
            "apply-app",
            name,
            &branch.from_arn,
            &[
                (MANAGED_ACTIONS, MANAGED_ACTIONS_ENABLED, "true"),
                (MANAGED_ACTIONS, PREFERRED_START_TIME, WINDOW),
            ],
        )
        .await;

        let scheduled = reported(&describe(&emulator, name, None).await);
        assert_eq!(
            scheduled.len(),
            1,
            "case {case:?}: expected one scheduled platform update, got {scheduled:?}"
        );
        let scheduled = scheduled[0].clone();

        // "To show only actions with a particular status, specify a status."
        let by_status = reported(&describe(&emulator, name, Some(SCHEDULED)).await);
        assert_eq!(
            by_status,
            vec![scheduled.clone()],
            "case {case:?}: the {SCHEDULED} filter must select the scheduled action, got \
             {by_status:?}"
        );
        let running_before = reported(&describe(&emulator, name, Some(RUNNING)).await);
        assert!(
            running_before.is_empty(),
            "case {case:?}: nothing is {RUNNING} before the action is applied, got \
             {running_before:?}"
        );

        let applied = emulator
            .call(
                "ApplyEnvironmentManagedAction",
                &[("EnvironmentName", name), ("ActionId", &scheduled.action_id)],
            )
            .await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: expected 200 from ApplyEnvironmentManagedAction, got {} and body {}",
            applied.status, applied.body
        );
        assert_eq!(
            applied.value("ActionId"),
            Some(scheduled.action_id.as_str()),
            "case {case:?}: the apply answers for the action it was given, got {}",
            applied.body
        );
        assert_eq!(
            applied.value("ActionType"),
            Some(PLATFORM_UPDATE_ACTION),
            "case {case:?}: unexpected ActionType in {}",
            applied.body
        );
        assert_eq!(
            applied.value("ActionDescription"),
            Some(scheduled.action_description.as_str()),
            "case {case:?}: the apply answers with the action's own description, got {}",
            applied.body
        );
        assert_eq!(
            applied.value("Status"),
            Some(SCHEDULED),
            "case {case:?}: the apply reports the action as it stood when it applied it — still \
             {SCHEDULED}, now running imminently — got {}",
            applied.body
        );

        // In progress: reported under the same id and window, now `Running`.
        let in_progress = reported(&describe(&emulator, name, None).await);
        assert_eq!(
            in_progress,
            vec![Reported {
                status: RUNNING.to_string(),
                ..scheduled.clone()
            }],
            "case {case:?}: expected the same action {RUNNING}, got {in_progress:?}"
        );
        let still_scheduled = reported(&describe(&emulator, name, Some(SCHEDULED)).await);
        assert!(
            still_scheduled.is_empty(),
            "case {case:?}: the action is no longer {SCHEDULED}, got {still_scheduled:?}"
        );

        // "A managed action can be applied only if its status is Scheduled."
        let again = emulator
            .call(
                "ApplyEnvironmentManagedAction",
                &[("EnvironmentName", name), ("ActionId", &scheduled.action_id)],
            )
            .await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected 400 for an action that is already running, got {} and body {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ManagedActionInvalidStateException",
            "case {case:?}: unexpected error code in {}",
            again.body
        );
        assert_eq!(
            again.error_message(),
            INVALID_STATE_MESSAGE,
            "case {case:?}: unexpected message in {}",
            again.body
        );

        // The work the clock owes: wait it out.
        std::thread::sleep(PAST_DUE);

        // The change the update made: the environment runs the newer release,
        // under both of that release's names.
        let environments = emulator
            .call(
                "DescribeEnvironments",
                &[("EnvironmentNames.member.1", name)],
            )
            .await;
        assert_eq!(
            environments.value("Environments/member/PlatformArn"),
            Some(branch.to_arn.as_str()),
            "case {case:?}: the update must move the environment to {}, got {}",
            branch.to_version,
            environments.body
        );
        assert_eq!(
            environments.value("Environments/member/SolutionStackName"),
            Some(branch.to_name.as_str()),
            "case {case:?}: expected the solution stack name of {}, got {}",
            branch.to_version,
            environments.body
        );

        // Carried out, so neither upcoming nor in progress any more.
        let after = reported(&describe(&emulator, name, None).await);
        assert!(
            after.is_empty(),
            "case {case:?}: an environment on its branch's current release has no managed \
             action left, got {after:?}"
        );

        // And in the history, as the record of a completed action.
        let history = emulator
            .call(
                "DescribeEnvironmentManagedActionHistory",
                &[("EnvironmentName", name)],
            )
            .await;
        let items = history.elements_at("ManagedActionHistoryItems/member");
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected one completed action in the history, got {}",
            history.body
        );
        let item = &items[0];
        assert_eq!(
            item.value("ActionId"),
            Some(scheduled.action_id.as_str()),
            "case {case:?}: the history records the action that ran, got {}",
            history.body
        );
        assert_eq!(
            item.value("Status"),
            Some("Completed"),
            "case {case:?}: an action the service carried out is Completed, got {}",
            history.body
        );
        assert_eq!(
            item.value("ActionType"),
            Some(PLATFORM_UPDATE_ACTION),
            "case {case:?}: unexpected ActionType in the history, got {}",
            history.body
        );
        assert_eq!(
            item.value("ActionDescription"),
            Some(scheduled.action_description.as_str()),
            "case {case:?}: the history keeps the action's description, got {}",
            history.body
        );
        let executed = instant(item.expect("ExecutedTime", case), case);
        let finished = instant(item.expect("FinishedTime", case), case);
        assert!(
            executed <= finished,
            "case {case:?}: an action finishes no earlier than it started, got \
             {executed} and {finished}"
        );

        // An action in a terminal state cannot be applied again either.
        let completed = emulator
            .call(
                "ApplyEnvironmentManagedAction",
                &[("EnvironmentName", name), ("ActionId", &scheduled.action_id)],
            )
            .await;
        assert_eq!(
            completed.error_code(),
            "ManagedActionInvalidStateException",
            "case {case:?}: expected the invalid-state refusal for a completed action, got {}",
            completed.body
        );
        assert_eq!(
            completed.error_message(),
            INVALID_STATE_MESSAGE,
            "case {case:?}: unexpected message for a completed action in {}",
            completed.body
        );

        // An id naming no action of this environment names nothing to apply.
        for unknown in ["1b2c3d4e-5f67-8901-2345-6789abcdef01", "action-1"] {
            let refused = emulator
                .call(
                    "ApplyEnvironmentManagedAction",
                    &[("EnvironmentName", name), ("ActionId", unknown)],
                )
                .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected 400 for {unknown:?}, got {} and body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ElasticBeanstalkServiceException",
                "case {case:?}: unexpected error code for {unknown:?} in {}",
                refused.body
            );
        }
    }
}

/// RULE: applying the weekly instance replacement replaces the environment's
/// Amazon EC2 instances — each one is a new instance, everything else the
/// environment is made of keeps its name — and the next replacement is then a
/// week away from the one that just ran, under an id of its own.
#[tokio::test]
async fn applying_the_instance_replacement_replaces_the_environments_instances() {
    let branch = branches(1).pop().expect("one branch");
    let emulator = Emulator::new();
    application(&emulator, "refresh-app").await;
    let name = "refresh-env";
    // Several instances, so the replacement is asserted over more than one.
    launch(
        &emulator,
        "refresh-app",
        name,
        &branch.to_arn,
        &[
            (MANAGED_ACTIONS, MANAGED_ACTIONS_ENABLED, "true"),
            (MANAGED_ACTIONS, PREFERRED_START_TIME, WINDOW),
            (PLATFORM_UPDATE, INSTANCE_REFRESH_ENABLED, "true"),
            ("aws:autoscaling:asg", "MinSize", "3"),
        ],
    )
    .await;
    let case = "weekly instance replacement";

    let inventory = |answer: &Answer, path: &str| -> Vec<String> {
        answer
            .values_at(path)
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    let resources = emulator
        .call("DescribeEnvironmentResources", &[("EnvironmentName", name)])
        .await;
    let before = inventory(&resources, "EnvironmentResources/Instances/member/Id");
    let group_before = inventory(&resources, "EnvironmentResources/AutoScalingGroups/member/Name");
    assert_eq!(
        before.len(),
        3,
        "case {case:?}: expected the three instances the configuration asks for, got {}",
        resources.body
    );

    let scheduled = reported(&describe(&emulator, name, None).await);
    assert_eq!(
        scheduled.len(),
        1,
        "case {case:?}: expected one scheduled instance replacement, got {scheduled:?}"
    );
    let replacement = scheduled[0].clone();
    assert_eq!(
        replacement.action_type, INSTANCE_REFRESH_ACTION,
        "case {case:?}: unexpected action type in {replacement:?}"
    );

    let applied = emulator
        .call(
            "ApplyEnvironmentManagedAction",
            &[
                ("EnvironmentName", name),
                ("ActionId", &replacement.action_id),
            ],
        )
        .await;
    assert_eq!(
        applied.status, 200,
        "case {case:?}: expected 200 from ApplyEnvironmentManagedAction, got {} and body {}",
        applied.status, applied.body
    );
    std::thread::sleep(PAST_DUE);

    let resources = emulator
        .call("DescribeEnvironmentResources", &[("EnvironmentName", name)])
        .await;
    let after = inventory(&resources, "EnvironmentResources/Instances/member/Id");
    assert_eq!(
        after.len(),
        before.len(),
        "case {case:?}: the group holds as many instances as before, got {}",
        resources.body
    );
    for instance in &after {
        assert!(
            !before.contains(instance),
            "case {case:?}: every instance is replaced, but {instance:?} is one of {before:?}"
        );
    }
    assert_eq!(
        inventory(&resources, "EnvironmentResources/AutoScalingGroups/member/Name"),
        group_before,
        "case {case:?}: a replacement keeps the Auto Scaling group it launches from, got {}",
        resources.body
    );

    // The next replacement is due a week after the instances that have just
    // been launched, so it is an action of its own rather than the one that ran.
    let next = reported(&describe(&emulator, name, None).await);
    assert_eq!(
        next.len(),
        1,
        "case {case:?}: expected the next weekly replacement to be scheduled, got {next:?}"
    );
    assert_ne!(
        next[0].action_id, replacement.action_id,
        "case {case:?}: the next replacement is an action of its own, got {next:?}"
    );
    assert_eq!(
        next[0].status, SCHEDULED,
        "case {case:?}: the next replacement has not started, got {next:?}"
    );
    assert!(
        instant(&next[0].window_start_time, case)
            >= instant(&replacement.window_start_time, case),
        "case {case:?}: the next replacement opens no earlier than the one that ran, got \
         {next:?} against {replacement:?}"
    );

    let history = emulator
        .call(
            "DescribeEnvironmentManagedActionHistory",
            &[("EnvironmentName", name)],
        )
        .await;
    assert_eq!(
        history.value("ManagedActionHistoryItems/member/ActionId"),
        Some(replacement.action_id.as_str()),
        "case {case:?}: the replacement that ran is in the history, got {}",
        history.body
    );
}

// ---------------------------------------------------------------------------
// Naming the environment, and the options this suite sets
// ---------------------------------------------------------------------------

/// RULE: the report names its environment by `EnvironmentId` or
/// `EnvironmentName` — either one resolves the same environment — a value no
/// environment answers to is refused, and naming neither is refused as a missing
/// parameter.
#[tokio::test]
async fn the_report_names_its_environment_by_either_identifier() {
    let branch = branches(1).pop().expect("one branch");
    let emulator = Emulator::new();
    application(&emulator, "named-app").await;
    let launched = launch(
        &emulator,
        "named-app",
        "named-env",
        &branch.from_arn,
        &[
            (MANAGED_ACTIONS, MANAGED_ACTIONS_ENABLED, "true"),
            (MANAGED_ACTIONS, PREFERRED_START_TIME, WINDOW),
        ],
    )
    .await;
    let environment_id = launched.expect("EnvironmentId", "named-env").to_string();

    let by_name = reported(&describe(&emulator, "named-env", None).await);
    let by_id = emulator
        .call(
            "DescribeEnvironmentManagedActions",
            &[("EnvironmentId", environment_id.as_str())],
        )
        .await;
    assert_eq!(
        reported(&by_id),
        by_name,
        "case \"by id\": both identifiers name the same environment, got {}",
        by_id.body
    );
    assert_eq!(
        by_name.len(),
        1,
        "case \"by name\": expected the scheduled update, got {by_name:?}"
    );

    /// One way of naming no environment, and the code the report refuses it
    /// with.
    struct Refusal {
        case: &'static str,
        params: Vec<(&'static str, &'static str)>,
        code: &'static str,
    }
    let cases = [
        Refusal {
            case: "no identifier at all",
            params: vec![],
            code: "MissingParameter",
        },
        Refusal {
            case: "a name nothing carries",
            params: vec![("EnvironmentName", "no-such-env")],
            code: "InvalidParameterValue",
        },
        Refusal {
            case: "an id nothing carries",
            params: vec![("EnvironmentId", "e-0000000000000")],
            code: "InvalidParameterValue",
        },
    ];
    for Refusal { case, params, code } in cases {
        let refused = emulator
            .call("DescribeEnvironmentManagedActions", &params)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected {code}, got body {}",
            refused.body
        );
    }
}

/// RULE: every configuration option this suite sets is one the service
/// PUBLISHES for the platform the environments run, so the configuration above
/// is the configuration a client would really write.
#[tokio::test]
async fn every_option_this_suite_sets_is_published_by_the_service() {
    let branch = branches(1).pop().expect("one branch");
    let emulator = Emulator::new();
    let published = emulator
        .call(
            "DescribeConfigurationOptions",
            &[("PlatformArn", branch.from_arn.as_str())],
        )
        .await;
    assert_eq!(
        published.status, 200,
        "expected 200 from DescribeConfigurationOptions, got {} and body {}",
        published.status, published.body
    );
    let options: Vec<(String, String)> = published
        .elements_at("Options/member")
        .into_iter()
        .map(|option| {
            (
                option.value("Namespace").unwrap_or_default().to_string(),
                option.value("Name").unwrap_or_default().to_string(),
            )
        })
        .collect();

    for (namespace, option_name) in [
        (MANAGED_ACTIONS, MANAGED_ACTIONS_ENABLED),
        (MANAGED_ACTIONS, PREFERRED_START_TIME),
        (PLATFORM_UPDATE, INSTANCE_REFRESH_ENABLED),
        (PLATFORM_UPDATE, UPDATE_LEVEL),
    ] {
        let case = &format!("{namespace} / {option_name}");
        assert!(
            options
                .iter()
                .any(|(published_namespace, published_name)| published_namespace == namespace
                    && published_name == option_name),
            "case {case:?}: the service must publish this option, got {options:?}"
        );
    }
}
