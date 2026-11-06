// SPDX-License-Identifier: CC0-1.0

//! What the enhanced-health reports derive about an environment and its
//! instances: the health attributes a request asks for, the health colours and
//! statuses, and the per-instance samples.
//!
//! Split out of [`super`], which holds the environment records these are
//! derived from.

use super::*;

/// The attribute value that asks a health report for every attribute it can
/// carry: "To retrieve all attributes, set to `All`."
pub const HEALTH_ATTRIBUTE_ALL: &str = "All";

/// Which attributes of a health report the request asked for.
///
/// "Specify the response elements you wish to receive. To retrieve all
/// attributes, set to `All`." Both health reports select their response members
/// this way, off their own attribute enum (`EnvironmentHealthAttribute`,
/// `InstancesHealthAttribute`), so the selection is read the same way for both;
/// the boundary validator has already refused a value outside the modelled set.
#[derive(Debug, Clone, Copy)]
pub struct RequestedHealthAttributes<'a> {
    pub(super) names: &'a [String],
}

impl<'a> RequestedHealthAttributes<'a> {
    /// The selection a request carrying `attribute_names` made. A request that
    /// named none selects nothing beyond the members the report always carries.
    pub fn of(attribute_names: Option<&'a [String]>) -> Self {
        Self {
            names: attribute_names.unwrap_or(&[]),
        }
    }

    /// Whether `attribute` was asked for, by name or through `All`.
    pub fn wants(&self, attribute: &str) -> bool {
        self.names
            .iter()
            .any(|name| name == attribute || name == HEALTH_ATTRIBUTE_ALL)
    }
}

/// The window `ApplicationMetrics` covers, in seconds: "The amount of time that
/// the metrics cover (usually 10 seconds)."
pub const METRICS_DURATION_SECONDS: i32 = 10;

/// The request metrics over the last [`METRICS_DURATION_SECONDS`], for the
/// environment as a whole and for one of its instances alike. This emulator
/// serves no application traffic, so the window holds no request: the counts are
/// zero and there are no latency percentiles to report over zero requests.
pub fn application_metrics() -> ApplicationMetrics {
    ApplicationMetrics {
        duration: Some(METRICS_DURATION_SECONDS),
        request_count: 0,
        status_codes: Some(StatusCodes {
            status2xx: Some(0),
            status3xx: Some(0),
            status4xx: Some(0),
            status5xx: Some(0),
        }),
        latency: None,
    }
}

/// The per-instance health statuses, which are also the
/// `InstanceHealthSummary` buckets an instance reporting one is counted in.
///
/// `Ok` and `Pending` are spelled exactly as the environment-level health
/// status is ([`HEALTH_STATUS_OK`], [`HEALTH_STATUS_PENDING`]), so they are
/// named off it rather than written out a second way.
pub const INSTANCE_HEALTH_NO_DATA: &str = "NoData";
/// See [`INSTANCE_HEALTH_NO_DATA`].
pub const INSTANCE_HEALTH_UNKNOWN: &str = "Unknown";
/// See [`INSTANCE_HEALTH_NO_DATA`]. "Grey. An operation is in progress on an
/// instance within the command timeout."
pub const INSTANCE_HEALTH_PENDING: &str = HEALTH_STATUS_PENDING;
/// See [`INSTANCE_HEALTH_NO_DATA`]. "Green. An instance is passing health checks
/// and the health agent is not reporting any problems."
pub const INSTANCE_HEALTH_OK: &str = HEALTH_STATUS_OK;
/// See [`INSTANCE_HEALTH_NO_DATA`]. "Green. An operation is in progress on an
/// instance."
pub const INSTANCE_HEALTH_INFO: &str = "Info";
/// See [`INSTANCE_HEALTH_NO_DATA`]. "Yellow. The health agent is reporting a
/// moderate number of request failures or other issues."
pub const INSTANCE_HEALTH_WARNING: &str = "Warning";
/// See [`INSTANCE_HEALTH_NO_DATA`]. "Red. The health agent is reporting a high
/// number of request failures or other issues."
pub const INSTANCE_HEALTH_DEGRADED: &str = "Degraded";
/// See [`INSTANCE_HEALTH_NO_DATA`]. "Red. The health agent is reporting a very
/// high number of request failures or other issues."
pub const INSTANCE_HEALTH_SEVERE: &str = "Severe";

/// The per-instance health status the instances of an environment reporting
/// each health COLOUR are in.
///
/// Every `InstanceHealthSummary` bucket states the colour it belongs to, which
/// is what pairs the two: an instance of a `Green` environment "is passing
/// health checks" (`Ok`), one of a `Grey` environment has "an operation in
/// progress … within the command timeout" (`Pending` — the one grey bucket an
/// environment mid-operation is in, where `NoData` and `Unknown` are the agent
/// reporting nothing), and the `Yellow` and `Red` colours are the moderate and
/// the high failure counts (`Warning`, `Degraded`).
pub(super) const INSTANCE_HEALTH_STATUS_BY_COLOUR: &[(&str, &str)] = &[
    (HEALTH_GREEN, INSTANCE_HEALTH_OK),
    (HEALTH_GREY, INSTANCE_HEALTH_PENDING),
    (HEALTH_YELLOW, INSTANCE_HEALTH_WARNING),
    (HEALTH_RED, INSTANCE_HEALTH_DEGRADED),
];

/// The per-instance health status an environment reporting health colour
/// `health` presents its instances in. A colour outside the modelled
/// `EnvironmentHealth` set is nothing this service writes, and an instance whose
/// health is not known falls in the bucket for exactly that — "Elastic Beanstalk
/// and the health agent are reporting no data on an instance".
pub fn instance_health_status(health: &str) -> &'static str {
    INSTANCE_HEALTH_STATUS_BY_COLOUR
        .iter()
        .find(|(colour, _)| *colour == health)
        .map_or(INSTANCE_HEALTH_NO_DATA, |(_, status)| *status)
}

/// The Availability Zone letters an environment's instances are spread over, in
/// turn.
///
/// Elastic Beanstalk launches an environment's Auto Scaling group across the
/// subnets it is configured with, so a group of several instances runs in
/// several zones. HOW MANY zones a Region publishes is not something this
/// service models, so the three here are CHOSEN; the zone NAME is the
/// documented `<region><letter>` form.
pub(super) const AVAILABILITY_ZONE_LETTERS: &[&str] = &["a", "b", "c"];

/// The Availability Zone the instance at `index` of an environment in `region`
/// runs in — "the availability zone in which the instance runs". Taken from the
/// instance's place in the stored inventory, so two reads of one environment
/// report the same zone for the same instance.
pub fn availability_zone(region: &str, index: usize) -> String {
    format!(
        "{region}{}",
        AVAILABILITY_ZONE_LETTERS[index % AVAILABILITY_ZONE_LETTERS.len()]
    )
}

/// The configuration-option namespace that carries the instance types an
/// environment's Auto Scaling group may launch, and the option in it: "a
/// comma-separated list of instance types".
pub(super) const NAMESPACE_EC2_INSTANCES: &str = "aws:ec2:instances";
/// See [`NAMESPACE_EC2_INSTANCES`].
pub(super) const OPTION_INSTANCE_TYPES: &str = "InstanceTypes";
/// The launch-configuration namespace's own, single-valued form of that option,
/// which an older request names the instance type by.
pub(super) const NAMESPACE_LAUNCH_CONFIGURATION: &str = "aws:autoscaling:launchconfiguration";
/// See [`NAMESPACE_LAUNCH_CONFIGURATION`].
pub(super) const OPTION_INSTANCE_TYPE: &str = "InstanceType";

/// The instance type an environment that asked for none runs on.
///
/// The published configuration-option reference gives the default as the
/// smallest burstable type the Region and the account offer, which varies
/// between them, so this value is CHOSEN; an environment that NAMES an instance
/// type runs on the one it named.
pub const DEFAULT_INSTANCE_TYPE: &str = "t3.micro";

/// The type of the Amazon EC2 instances an environment with these configuration
/// options runs — "the instance's type".
///
/// The first of the types its Auto Scaling group may launch
/// (`aws:ec2:instances` / `InstanceTypes`, "a comma-separated list"), the single
/// type an older request named instead (`aws:autoscaling:launchconfiguration` /
/// `InstanceType`), or [`DEFAULT_INSTANCE_TYPE`] when the environment named
/// none.
pub fn environment_instance_type(settings: &[ConfigurationOptionSetting]) -> String {
    option_value(settings, NAMESPACE_EC2_INSTANCES, OPTION_INSTANCE_TYPES)
        .and_then(|types| {
            types
                .split(',')
                .map(str::trim)
                .find(|instance_type| !instance_type.is_empty())
        })
        .or_else(|| {
            option_value(
                settings,
                NAMESPACE_LAUNCH_CONFIGURATION,
                OPTION_INSTANCE_TYPE,
            )
            .filter(|instance_type| !instance_type.is_empty())
        })
        .unwrap_or(DEFAULT_INSTANCE_TYPE)
        .to_string()
}

/// The `Deployment$Status` values: "`In Progress` : The deployment is in
/// progress", "`Deployed` : The deployment succeeded".
pub const INSTANCE_DEPLOYMENT_IN_PROGRESS: &str = "In Progress";
/// See [`INSTANCE_DEPLOYMENT_IN_PROGRESS`].
pub const INSTANCE_DEPLOYMENT_DEPLOYED: &str = "Deployed";

/// "Descriptions of the data that contributed to the environment's current
/// health status", which are also the causes "that provide more information
/// about the current health status" of one of its instances: a healthy
/// environment has none, and one whose health checks are suspended or not yet
/// started reports the state that suspended them.
pub fn environment_health_causes(environment: &Environment) -> Vec<String> {
    if environment.health == HEALTH_GREEN {
        return Vec::new();
    }
    vec![format!(
        "Environment health is {}: the environment's status is {}.",
        environment.health, environment.status
    )]
}

/// The health of each Amazon EC2 instance `environment` is made of, in the order
/// its inventory holds them — one `SingleInstanceHealth` per instance, with
/// every member the `InstancesHealthAttribute` set can select from it.
///
/// An environment with no inventory at all — one the clock has terminated,
/// which "is not running" — is made of no instance, so it reports none.
pub fn environment_instance_health(environment: &Environment) -> Vec<SingleInstanceHealth> {
    let Some(resources) = environment.resources.as_ref() else {
        return Vec::new();
    };
    let health_status = instance_health_status(&environment.health);
    let instance_type = environment_instance_type(&environment.option_settings);
    let causes = environment_health_causes(environment);
    let deployment = environment.current_deployment();
    resources
        .instances
        .iter()
        .enumerate()
        .map(|(index, instance_id)| SingleInstanceHealth {
            // "The ID of the Amazon EC2 instance" — the one the environment's
            // inventory holds, which `DescribeEnvironmentResources` reports.
            instance_id: Some(instance_id.clone()),
            health_status: Some(health_status.to_string()),
            // "The color indicator that gives you information about the health
            // of the EC2 instance", which is the colour of the bucket that
            // status falls in.
            color: Some(environment.health.clone()),
            causes: Some(causes.clone()),
            // "The time at which the EC2 instance was launched": when the
            // instances this environment runs NOW were launched, which a
            // rebuild and a managed instance replacement move.
            launched_at: Some(environment.instances_replaced_at),
            application_metrics: Some(application_metrics()),
            // "Operating system metrics from the instance" — CPU utilization and
            // load average, which the enhanced-health agent reads from the
            // running operating system. No instance runs here, so there are no
            // operating-system metrics to report rather than numbers nothing
            // measured.
            system: None,
            deployment: Some(deployment.clone()),
            availability_zone: Some(availability_zone(&environment.region, index)),
            instance_type: Some(instance_type.clone()),
        })
        .collect()
}

/// `instances` counted into the `InstanceHealthSummary` buckets — "summary
/// health information for the instances in the environment".
///
/// Counted from the very list the per-instance report answers with, so the
/// summary and that report cannot disagree about how many instances are in a
/// bucket. Every bucket is reported, including the empty ones.
pub fn instance_health_summary(instances: &[SingleInstanceHealth]) -> InstanceHealthSummary {
    let count = |bucket: &str| {
        Some(
            i32::try_from(
                instances
                    .iter()
                    .filter(|instance| instance.health_status.as_deref() == Some(bucket))
                    .count(),
            )
            .unwrap_or(i32::MAX),
        )
    };
    InstanceHealthSummary {
        no_data: count(INSTANCE_HEALTH_NO_DATA),
        unknown: count(INSTANCE_HEALTH_UNKNOWN),
        pending: count(INSTANCE_HEALTH_PENDING),
        ok: count(INSTANCE_HEALTH_OK),
        info: count(INSTANCE_HEALTH_INFO),
        warning: count(INSTANCE_HEALTH_WARNING),
        degraded: count(INSTANCE_HEALTH_DEGRADED),
        severe: count(INSTANCE_HEALTH_SEVERE),
    }
}
