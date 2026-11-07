// SPDX-License-Identifier: CC0-1.0

//! The CONFIGURATION one stack operation runs under: its `DeploymentConfig` (the mode, and
//! whether a failure is rolled back) and its `RollbackConfiguration` (the triggers
//! CloudFormation monitors while it runs, and for how long).
//!
//! Split out of [`super`] so the store module stays within the length a change to this
//! repository may touch. One concern, held in one place: every operation that takes either
//! member — `CreateStack`, `UpdateStack`, `CreateChangeSet`, `ExecuteChangeSet` — validates
//! it, stores it and reports it through the functions here, so no two of them can come to
//! read the same member differently. What the triggers stated here then DO while an
//! operation is in flight is [`super::rollback_triggers`]. Nothing about the behaviour
//! changes with the move.

use super::*;

/// The deployment mode a stack operation runs in when the request names none.
///
/// `DeploymentConfig$Mode`: "STANDARD - Use the standard deployment behavior, ensuring
/// resources are ready to serve traffic before completing the operation. This is the
/// default. You do not need to specify this value explicitly." The operation therefore ran
/// in this mode whether or not the request spelled it, which is why the member is reported
/// on every stack and change set rather than only on the ones that named it.
pub const DEPLOYMENT_CONFIG_MODE_STANDARD: &str = "STANDARD";

/// The deployment configuration in force on a stack or a change set, as the modelled
/// member.
///
/// Unlike [`deployment_config_view`], which answers only what a request supplied, this is
/// what a READ of the resource reports: CloudFormation carries the member on every stack
/// and change set, with the default mode for one that named no mode.
pub fn deployment_config_in_force(
    config: Option<&StoredDeploymentConfig>,
) -> Option<crate::ops::types::DeploymentConfig> {
    Some(crate::ops::types::DeploymentConfig {
        mode: config
            .and_then(|config| config.mode.clone())
            .or_else(|| Some(DEPLOYMENT_CONFIG_MODE_STANDARD.to_string())),
        // `DeploymentConfig$DisableRollback`: a request that stated the flag is reported as
        // having stated it; one that did not is reported with the value the MODE it runs in
        // puts in force ([`mode_disables_rollback`]).
        disable_rollback: config
            .and_then(|config| config.disable_rollback)
            .or_else(|| Some(mode_disables_rollback(stored_deployment_mode(config)))),
    })
}

/// The rollback configuration in force on a change set, as the modelled member.
///
/// CloudFormation carries the member on every change set it describes, empty for one whose
/// request named no rollback triggers.
pub fn rollback_configuration_in_force(
    configuration: Option<&StoredRollbackConfiguration>,
) -> Option<crate::ops::types::RollbackConfiguration> {
    Some(rollback_configuration_view(configuration).unwrap_or(
        crate::ops::types::RollbackConfiguration {
            rollback_triggers: None,
            monitoring_time_in_minutes: None,
        },
    ))
}

/// The resource types a rollback trigger may name, which `RollbackTrigger$Type` states as
/// a closed pair: "The resource type of the rollback trigger. Specify either
/// `AWS::CloudWatch::Alarm` or `AWS::CloudWatch::CompositeAlarm` resource types." The
/// model gives the member a plain string target rather than an enum, so the handler is
/// what holds a request to the pair.
pub const ROLLBACK_TRIGGER_TYPES: [&str; 2] =
    ["AWS::CloudWatch::Alarm", "AWS::CloudWatch::CompositeAlarm"];

/// CloudFormation's refusal for a rollback trigger naming any other resource type, which
/// carries no variable part.
pub const ROLLBACK_TRIGGER_TYPE_NOT_SUPPORTED: &str = "Rollback Trigger Type not supported";

/// How many rollback triggers one `RollbackConfiguration` may name.
///
/// The model fixes it: `RollbackTriggers` is `length: min 0, max 5`
/// (`smithy_query(query="shape", shape="RollbackTriggers")`), which is also the quota AWS
/// publishes for the rollback triggers of one stack, and the framework refuses a request
/// past it from the constraint itself. It is stated here because the monitoring the triggers
/// cost is read against it ([`rollback_triggers`]): the set CloudFormation reads during an
/// operation is bounded by the model rather than by what a request asked for.
pub const MAX_ROLLBACK_TRIGGERS: usize = 5;

/// `Err(message)` where the configuration names a rollback trigger whose `Type` is not one
/// of [`ROLLBACK_TRIGGER_TYPES`].
///
/// A trigger CloudFormation monitors is an alarm — "If any of the alarms you specify goes
/// to ALARM state during the stack operation [...] CloudFormation rolls back the entire
/// stack operation" — so a type nothing can alarm on is a request the service cannot
/// honour, and it is refused rather than stored as monitoring that could never fire.
/// Shared by every operation that takes a `RollbackConfiguration` (`CreateStack`,
/// `UpdateStack` and `CreateChangeSet`), which all set the configuration a later rollback
/// would be driven by and so all hold it to the same rule.
///
/// The COUNT is not checked here: the list's own length constraint is the model's, and the
/// framework refuses a request past it before a handler is reached
/// ([`MAX_ROLLBACK_TRIGGERS`]).
pub fn validate_rollback_configuration(
    configuration: Option<&crate::ops::types::RollbackConfiguration>,
) -> Result<(), String> {
    let unsupported = configuration
        .and_then(|configuration| configuration.rollback_triggers.as_deref())
        .unwrap_or_default()
        .iter()
        .any(|trigger| !ROLLBACK_TRIGGER_TYPES.contains(&trigger.r#type.as_str()));
    if unsupported {
        return Err(ROLLBACK_TRIGGER_TYPE_NOT_SUPPORTED.to_string());
    }
    Ok(())
}

/// The request's `RollbackConfiguration` as stored configuration, with the trigger list held
/// to the model's own ceiling ([`MAX_ROLLBACK_TRIGGERS`]).
///
/// The clamp is unreachable through the wire — the list's own length constraint is the
/// model's, and the framework refuses a request past it before a handler is called — so it
/// drops nothing a caller asked for. It is here so the bound on the monitoring work holds for
/// whatever record this store ends up with ([`rollback_triggers`] reads one call per trigger),
/// rather than resting on a check made somewhere else.
pub fn stored_rollback_configuration(
    configuration: Option<&crate::ops::types::RollbackConfiguration>,
) -> Option<StoredRollbackConfiguration> {
    configuration.map(|configuration| StoredRollbackConfiguration {
        triggers: configuration
            .rollback_triggers
            .iter()
            .flatten()
            .take(MAX_ROLLBACK_TRIGGERS)
            .map(|trigger| StoredRollbackTrigger {
                arn: trigger.arn.clone(),
                trigger_type: trigger.r#type.clone(),
            })
            .collect(),
        monitoring_time_in_minutes: configuration.monitoring_time_in_minutes,
    })
}

/// Stored rollback configuration as the modelled member.
///
/// CloudFormation reports `RollbackConfiguration` on every stack, as an empty element for
/// a stack that has none, so this is always present; the triggers inside it appear only
/// when the stack has some.
pub fn rollback_configuration_view(
    configuration: Option<&StoredRollbackConfiguration>,
) -> Option<crate::ops::types::RollbackConfiguration> {
    Some(crate::ops::types::RollbackConfiguration {
        rollback_triggers: configuration.and_then(|configuration| {
            non_empty(
                configuration
                    .triggers
                    .iter()
                    .map(|trigger| crate::ops::types::RollbackTrigger {
                        arn: trigger.arn.clone(),
                        r#type: trigger.trigger_type.clone(),
                    })
                    .collect(),
            )
        }),
        monitoring_time_in_minutes: configuration
            .and_then(|configuration| configuration.monitoring_time_in_minutes),
    })
}

/// The request's `DeploymentConfig` as stored configuration.
pub fn stored_deployment_config(
    config: Option<&crate::ops::types::DeploymentConfig>,
) -> Option<StoredDeploymentConfig> {
    config.map(|config| StoredDeploymentConfig {
        mode: config.mode.clone(),
        disable_rollback: config.disable_rollback,
    })
}

/// Whether the request itself asks for rollback to be DISABLED, or `None` where it asks
/// for neither — the operation's own default then stands
/// ([`mode_disables_rollback`] is that default where nothing else decides it).
///
/// TWO members of a stack operation say this one thing, in the same words: the operation's
/// own `DisableRollback` ("Preserve the state of previously provisioned resources when an
/// operation fails") and `DeploymentConfig$DisableRollback` ("Specifies whether to disable
/// rollback of the stack if the stack operation fails"). So a request that states it in
/// either place has said it, and the ONE reading of that pair lives here rather than per
/// operation: a request stating it in the deployment configuration alone must not be read
/// as a request that asked for nothing.
pub fn rollback_disabled(
    disable_rollback: Option<bool>,
    deployment_config: Option<&crate::ops::types::DeploymentConfig>,
) -> Option<bool> {
    disable_rollback.or_else(|| deployment_config.and_then(|config| config.disable_rollback))
}

/// [`rollback_disabled`] for the deployment configuration a change set was CREATED with,
/// which the execution of that change set runs under.
pub fn stored_rollback_disabled(config: Option<&StoredDeploymentConfig>) -> Option<bool> {
    config.and_then(|config| config.disable_rollback)
}

/// Whether the MODE an operation runs in disables its rollback by itself — the default for
/// an operation whose request states the flag nowhere and names no failure action.
///
/// A failed [`DEPLOYMENT_MODE_EXPRESS`] operation keeps what it provisioned: the recorded
/// behaviour of such an operation is the stack left in its failed status with the resources
/// that were created still there, while the same template deployed in
/// [`DEPLOYMENT_MODE_STANDARD`], and in `EXPRESS` with `DisableRollback` stated false, is
/// rolled back — so the member's documented default ("Default: `false`") is the default of
/// the standard mode. It is the LAST thing consulted: a flag or a failure action the request
/// states is what that request asked for, whichever mode it runs in.
pub fn mode_disables_rollback(mode: Option<&str>) -> bool {
    mode == Some(DEPLOYMENT_MODE_EXPRESS)
}

/// The `Mode` one stored deployment configuration names, for [`mode_disables_rollback`].
pub fn stored_deployment_mode(config: Option<&StoredDeploymentConfig>) -> Option<&str> {
    config.and_then(|config| config.mode.as_deref())
}

/// Stored deployment configuration as the modelled member, with the documented defaults
/// filled in.
///
/// CloudFormation reports `DeploymentConfig` on every stack — it is the configuration the
/// stack's operations run under, which every stack has. A stack whose operations never
/// stated one runs under the defaults: [`DEPLOYMENT_MODE_STANDARD`] ("This is the default.
/// You do not need to specify this value explicitly") and, for `DisableRollback`, the
/// rollback setting in force on the stack — the same flag `Stack$DisableRollback` reports.
pub fn deployment_config_view(
    config: Option<&StoredDeploymentConfig>,
    disable_rollback: bool,
) -> Option<crate::ops::types::DeploymentConfig> {
    Some(crate::ops::types::DeploymentConfig {
        mode: Some(
            config
                .and_then(|config| config.mode.clone())
                .unwrap_or_else(|| DEPLOYMENT_MODE_STANDARD.to_string()),
        ),
        disable_rollback: Some(
            config
                .and_then(|config| config.disable_rollback)
                .unwrap_or(disable_rollback),
        ),
    })
}
