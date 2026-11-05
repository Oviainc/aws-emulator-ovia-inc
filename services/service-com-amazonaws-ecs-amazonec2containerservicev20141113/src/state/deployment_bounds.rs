// SPDX-License-Identifier: CC0-1.0

//! The bounds a SERVICE's deployment has to meet before the scheduler is given it.
//!
//! `DeploymentConfiguration`'s two percentages bound the same count from opposite ends and
//! `healthCheckGracePeriodSeconds` is a span of time the scheduler waits out, so each rule
//! here is about the configuration the service is LEFT with — not about the half of it one
//! request happened to carry. Spelled once for the create of a service and the update of
//! one, so a configuration one of them refuses cannot be written by the other.

use super::*;

/// `InvalidParameterException$message` for a deployment configuration whose floor and
/// ceiling leave the scheduler no room at all. It names both members and the value they
/// share, and echoes nothing else of the request.
pub const DEPLOYMENT_NO_ROOM_MESSAGE: &str =
    "Both maximumPercent and minimumHealthyPercent cannot be 100 as this will block \
     deployments.";

/// The refusal a deployment configuration earns when it leaves a rolling deployment no
/// room to replace a task, or `None` when the scheduler has room.
///
/// The two percentages bound the SAME count from opposite ends: `minimumHealthyPercent`
/// is "a lower limit on the number of your service's tasks that must remain in the
/// RUNNING state during a deployment, as a percentage of the desiredCount", and
/// `maximumPercent` "an upper limit on the number of your service's tasks that are
/// allowed in the RUNNING or PENDING state during a deployment", of the same count. At
/// 100% each, the scheduler may neither start a task above the count the service wants
/// nor stop one below it, so no task of the service could ever be replaced and the
/// deployment could never finish.
///
/// Weighed on the EFFECTIVE configuration — the one the service is left with, each member
/// defaulted ([`deployment_configuration`]) or merged over the stored one
/// ([`merged_deployment_configuration`]) — because that is the configuration the
/// scheduler would have to deploy with, whichever call supplied which half of it.
pub fn deployment_room_violation(configuration: &DeploymentConfiguration) -> Option<String> {
    let both_at_ceiling = configuration.minimum_healthy_percent == Some(MAX_MINIMUM_HEALTHY_PERCENT)
        && configuration.maximum_percent == Some(MIN_MAXIMUM_PERCENT);
    both_at_ceiling.then(|| DEPLOYMENT_NO_ROOM_MESSAGE.to_string())
}

/// The smallest `healthCheckGracePeriodSeconds` a service may name. The member is "The
/// period of time, in seconds, that the Amazon ECS service scheduler ignores unhealthy
/// ... health checks after a task has first started": a period of time the scheduler
/// waits out is never negative, and a service that wants none asks for the zero its own
/// doc gives as the default ("If you do not specify a health check grace period value,
/// the default value of 0 is used").
pub const MIN_HEALTH_CHECK_GRACE_PERIOD_SECONDS: i32 = 0;

/// `InvalidParameterException$message` for a `healthCheckGracePeriodSeconds` below the
/// zero its own doc bounds it at. Amazon ECS names the period and the bound it crossed,
/// and carries neither the value the request sent nor how this emulator read it.
pub const HEALTH_CHECK_GRACE_PERIOD_NEGATIVE_MESSAGE: &str =
    "healthCheckGracePeriod must be at least 0";

/// The refusal a requested `healthCheckGracePeriodSeconds` earns when it names no period
/// of time at all, or `None` when it names one (or names none).
///
/// A comparison against the floor above and nothing else: the value is the caller's `i32`
/// as the wire carried it, so no arithmetic is done on it and no bound can be crossed
/// reading it.
pub fn health_check_grace_period_violation(seconds: Option<i32>) -> Option<&'static str> {
    seconds
        .filter(|seconds| *seconds < MIN_HEALTH_CHECK_GRACE_PERIOD_SECONDS)
        .map(|_| HEALTH_CHECK_GRACE_PERIOD_NEGATIVE_MESSAGE)
}
