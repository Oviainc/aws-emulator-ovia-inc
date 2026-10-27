// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-autoscaling-autoscaling-2011-01-01 service.
//!
//! The Auto Scaling control plane keeps two named collections — launch
//! configurations and Auto Scaling groups — plus the scaling activities the
//! capacity changes of those groups produce. Every handler in `src/ops/` reads and
//! writes THIS struct through [`service_state`]; nothing in this crate holds a
//! second copy of a resource.
//!
//! # The clock this service owes
//!
//! Two changes are owed by the clock rather than by a later request, so they are
//! stored as a DUE INSTANT and materialised by [`AutoScalingState::materialize_due`]
//! (called by every handler, including the reporting ones, which may only
//! materialise what the clock already made due):
//!
//! * an instance Amazon EC2 Auto Scaling just launched is reported `Pending` and
//!   becomes `InService`; one it is scaling in is reported `Terminating` and then
//!   leaves the group ([`LIFECYCLE_TRANSITION`]);
//! * a capacity change that asked to honour the group's cooldown waits for that
//!   cooldown to elapse before it is applied (`HonorCooldown`).
//!
//! # The instances a group holds
//!
//! An instance of a group is an Amazon EC2 instance OF THE ACCOUNT: a capacity change
//! launches it in the account's own Amazon EC2 store and terminates it there when the
//! group scales in ([`ec2_instances`]), so every id a group reports is one Amazon EC2
//! resolves. This struct holds the group's own view of it — its lifecycle state, its
//! health and the zone it was placed in — and nothing that Amazon EC2's record owns.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AvailabilityZoneDistribution, AvailabilityZoneImpairmentPolicy, BlockDeviceMapping,
    CapacityReservationSpecification, InstanceLifecyclePolicy, InstanceMaintenancePolicy,
    InstanceMetadataOptions, LaunchTemplateSpecification, MixedInstancesPolicy, Operator, Tag,
    TrafficSourceIdentifier,
};

/// The Availability Zones of the subnets a request's `VPCZoneIdentifier` names, read
/// from the account's own Amazon EC2 store.
pub mod ec2_subnets;

/// The Amazon EC2 instances a capacity change launches and terminates, in the
/// account's own Amazon EC2 store: an instance id a group reports names an instance of
/// the account, not one of this service's own invention.
pub mod ec2_instances;

/// How long a lifecycle transition this service owes takes to come due.
///
/// Amazon EC2 Auto Scaling reports a freshly launched instance as `Pending` and an
/// instance being scaled in as `Terminating` before either settles, so a caller
/// that reads the group straight after a capacity change must still see the
/// transient state. The real wait depends on the instance; this emulator picks a
/// short, fixed span (chosen, not documented) and never sleeps for it.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// `DefaultCooldown` when the request omits it.
///
/// From the model's own documentation for `CreateAutoScalingGroup$DefaultCooldown`:
/// "Default: 300 seconds".
pub const DEFAULT_COOLDOWN_SECONDS: i32 = 300;

/// `HealthCheckGracePeriod` when the request omits it.
///
/// From the model's own documentation for
/// `CreateAutoScalingGroup$HealthCheckGracePeriod`: "Default: 0 seconds".
pub const DEFAULT_HEALTH_CHECK_GRACE_PERIOD: i32 = 0;

/// `HealthCheckType` when the request omits it.
///
/// From the model's own documentation for `CreateAutoScalingGroup$HealthCheckType`:
/// "EC2 is the default health check and cannot be disabled."
pub const DEFAULT_HEALTH_CHECK_TYPE: &str = "EC2";

/// `TerminationPolicies` when the request omits it: the `Default` policy, which
/// the model documents as a valid `TerminationPolicies` value and which the
/// service applies to a group that configures none.
pub const DEFAULT_TERMINATION_POLICY: &str = "Default";

// The three `LifecycleState` values this slice moves an instance through, spelled
// exactly as the model's `com.amazonaws.autoscaling#LifecycleState` enum does
// (smithy_query(query="shape",shape="LifecycleState")).
/// An instance that has been launched and is not yet in service.
pub const LIFECYCLE_PENDING: &str = "Pending";
/// An instance that is in service.
pub const LIFECYCLE_IN_SERVICE: &str = "InService";
/// An instance that is being scaled in.
pub const LIFECYCLE_TERMINATING: &str = "Terminating";

/// The `HealthStatus` of an instance the service has no failed health check for.
pub const HEALTH_STATUS_HEALTHY: &str = "Healthy";

// The `ScalingActivityStatusCode` values an activity this slice records moves
// through, from the model's `com.amazonaws.autoscaling#ScalingActivityStatusCode`.
/// A scaling activity that has started and not yet finished.
pub const ACTIVITY_IN_PROGRESS: &str = "InProgress";
/// A scaling activity that finished.
pub const ACTIVITY_SUCCESSFUL: &str = "Successful";
/// A scaling activity that did not place the instance it was started for.
pub const ACTIVITY_FAILED: &str = "Failed";

/// `Activity$AutoScalingGroupState` for a group that still exists.
pub const GROUP_STATE_IN_SERVICE: &str = "InService";
/// `Activity$AutoScalingGroupState` for a group that has been deleted.
pub const GROUP_STATE_DELETED: &str = "Deleted";

/// The most instances this emulator will hold for one Auto Scaling group, and so
/// the largest `MinSize`, `MaxSize` or `DesiredCapacity` any operation accepts.
///
/// A BOUND ON THIS PROCESS, not a value AWS documents. Every unit of capacity is
/// a stored instance plus a stored scaling activity, so a single well-formed
/// request carrying a capacity in the billions would exhaust this process's
/// memory and wedge every other caller: the bounds the operations check against
/// each other (`MinSize <= MaxSize`, `DesiredCapacity` inside them) are all
/// relative and cannot stop that. Amazon EC2 Auto Scaling bounds the same growth
/// with an adjustable account quota and answers `LimitExceededFault` past it;
/// this ceiling plays that role, at a size an in-memory emulator can hold.
pub const MAX_GROUP_CAPACITY: i32 = 1_000;

/// The most Auto Scaling groups this emulator will hold, for the same reason as
/// [`MAX_GROUP_CAPACITY`]: a bound on this process, chosen at the order of the
/// service's own adjustable per-Region quota, past which `CreateAutoScalingGroup`
/// answers its modelled `LimitExceededFault`.
pub const MAX_GROUPS: usize = 500;

/// The launch-configuration limit this service REPORTS for the account, through
/// `DescribeAccountLimits`'s `MaxNumberOfLaunchConfigurations`.
///
/// Same order as the service's own adjustable per-Region quota, like
/// [`MAX_GROUPS`]. Nothing enforces it: `CreateLaunchConfiguration` is refused
/// with `UnsupportedOperation`, so the reported count stays 0 and no caller can
/// approach the limit.
pub const MAX_LAUNCH_CONFIGURATIONS: usize = 200;

/// The most scaling activities this service retains; appending past it drops the
/// oldest.
///
/// Also a bound on this process: activities are appended by every capacity change
/// and never removed by one, so an unbounded log would grow for the life of the
/// process. Amazon EC2 Auto Scaling itself only reports activities from the last
/// six weeks, so a retention bound is the shape of the real behaviour too.
pub const MAX_RETAINED_ACTIVITIES: usize = 10_000;

/// A stored launch configuration — the `LaunchConfiguration` resource.
///
/// NOTHING in this crate constructs one: `CreateLaunchConfiguration` is refused
/// with `UnsupportedOperation`, so [`ComAmazonawsAutoscalingAutoscaling20110101State::launch_configurations`]
/// stays empty. This is the shape its readers project from
/// (`DescribeLaunchConfigurations`, a group's launch source), which is why they
/// report what the store holds instead of assuming it is empty.
#[derive(Debug, Clone)]
pub struct StoredLaunchConfiguration {
    pub name: String,
    pub arn: String,
    pub image_id: String,
    pub instance_type: String,
    pub key_name: Option<String>,
    pub security_groups: Vec<String>,
    pub classic_link_vpc_id: Option<String>,
    pub classic_link_vpc_security_groups: Vec<String>,
    pub user_data: Option<String>,
    pub kernel_id: Option<String>,
    pub ramdisk_id: Option<String>,
    pub spot_price: Option<String>,
    pub iam_instance_profile: Option<String>,
    pub block_device_mappings: Vec<BlockDeviceMapping>,
    pub metadata_options: Option<InstanceMetadataOptions>,
    /// `InstanceMonitoring$Enabled`. The model documents the default as `true`
    /// (detailed monitoring).
    pub instance_monitoring_enabled: bool,
    pub ebs_optimized: bool,
    pub associate_public_ip: Option<bool>,
    pub placement_tenancy: Option<String>,
    pub created_time: SystemTime,
}

/// One instance attached to a group.
///
/// [`StoredInstance::instance_id`] is the id of an Amazon EC2 instance OF THE ACCOUNT,
/// launched through [`ec2_instances`] by the capacity change that placed it, so every
/// id a group reports resolves in the account's own Amazon EC2 state.
#[derive(Debug, Clone)]
pub struct StoredInstance {
    pub instance_id: String,
    pub availability_zone: String,
    pub lifecycle_state: String,
    pub health_status: String,
    pub launch_configuration_name: Option<String>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub instance_type: Option<String>,
    pub image_id: Option<String>,
    pub protected_from_scale_in: bool,
    /// When this instance's transient lifecycle state comes due, if it is in one.
    pub transition_due: Option<SystemTime>,
}

/// A capacity change deferred until the group's cooldown has elapsed, because the
/// request asked for the cooldown to be honoured.
#[derive(Debug, Clone)]
pub struct PendingCapacity {
    pub desired_capacity: i32,
    pub due: SystemTime,
    pub cause: String,
}

/// A stored Auto Scaling group — the `AutoScalingGroup` resource, as created and
/// updated.
#[derive(Debug, Clone)]
pub struct StoredGroup {
    pub name: String,
    pub arn: String,
    /// The Region the group was created in, which is the Region the instances it
    /// launches run in.
    pub region: String,
    pub launch_configuration_name: Option<String>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub mixed_instances_policy: Option<MixedInstancesPolicy>,
    pub min_size: i32,
    pub max_size: i32,
    pub desired_capacity: i32,
    pub default_cooldown: i32,
    pub availability_zones: Vec<String>,
    pub availability_zone_ids: Option<Vec<String>>,
    pub load_balancer_names: Option<Vec<String>>,
    pub target_group_arns: Option<Vec<String>>,
    pub health_check_type: String,
    pub health_check_grace_period: Option<i32>,
    pub placement_group: Option<String>,
    pub vpc_zone_identifier: Option<String>,
    pub termination_policies: Vec<String>,
    pub new_instances_protected_from_scale_in: bool,
    pub service_linked_role_arn: Option<String>,
    pub max_instance_lifetime: Option<i32>,
    pub capacity_rebalance: Option<bool>,
    pub context: Option<String>,
    pub desired_capacity_type: Option<String>,
    pub default_instance_warmup: Option<i32>,
    pub traffic_sources: Option<Vec<TrafficSourceIdentifier>>,
    pub instance_maintenance_policy: Option<InstanceMaintenancePolicy>,
    pub availability_zone_distribution: Option<AvailabilityZoneDistribution>,
    pub availability_zone_impairment_policy: Option<AvailabilityZoneImpairmentPolicy>,
    pub capacity_reservation_specification: Option<CapacityReservationSpecification>,
    pub instance_lifecycle_policy: Option<InstanceLifecyclePolicy>,
    pub operator: Option<Operator>,
    pub deletion_protection: Option<String>,
    pub tags: Vec<Tag>,
    pub status: Option<String>,
    pub created_time: SystemTime,
    pub instances: Vec<StoredInstance>,
    /// End of the group's cooldown after its last applied capacity change.
    pub cooldown_until: Option<SystemTime>,
    /// A capacity change waiting for [`Self::cooldown_until`].
    pub pending_capacity: Option<PendingCapacity>,
}

impl StoredGroup {
    /// The instances that count towards the group's capacity: everything that is
    /// not on its way out.
    pub fn active_instances(&self) -> impl Iterator<Item = &StoredInstance> {
        self.instances
            .iter()
            .filter(|i| i.lifecycle_state != LIFECYCLE_TERMINATING)
    }

    /// How many instances count towards the group's capacity.
    pub fn active_instance_count(&self) -> i32 {
        i32::try_from(self.active_instances().count()).unwrap_or(i32::MAX)
    }

    /// Whether the group is inside the cooldown that followed its last capacity
    /// change.
    pub fn in_cooldown(&self, now: SystemTime) -> bool {
        self.cooldown_until.is_some_and(|until| now < until)
    }

    /// How many more instances this group can be given before it reaches
    /// [`MAX_GROUP_CAPACITY`].
    ///
    /// Counts the instances still `Terminating`: they remain stored until the
    /// clock retires them, so they occupy the ceiling too. Without that, a caller
    /// alternating the capacity faster than the transition comes due could stack
    /// a fresh ceiling's worth of instances on top of the ones still leaving.
    pub fn launch_headroom(&self) -> i32 {
        MAX_GROUP_CAPACITY.saturating_sub(i32::try_from(self.instances.len()).unwrap_or(i32::MAX))
    }

    /// How many instances this group must launch to reach `desired_capacity`.
    pub fn launches_needed(&self, desired_capacity: i32) -> i32 {
        (desired_capacity - self.active_instance_count()).max(0)
    }
}

/// A stored scaling activity — the `Activity` resource a capacity change produces.
#[derive(Debug, Clone)]
pub struct StoredActivity {
    pub activity_id: String,
    pub auto_scaling_group_name: String,
    pub auto_scaling_group_arn: Option<String>,
    pub description: Option<String>,
    pub cause: String,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_code: String,
    pub progress: i32,
    pub details: Option<String>,
    pub auto_scaling_group_state: Option<String>,
    /// When this activity finishes, if it has not finished yet.
    pub completes_at: Option<SystemTime>,
}

/// Per-service state for com-amazonaws-autoscaling-autoscaling-2011-01-01. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAutoscalingAutoscaling20110101State {
    /// Launch configurations, keyed by `LaunchConfigurationName`.
    ///
    /// Empty for the life of the process: `CreateLaunchConfiguration` is refused
    /// with `UnsupportedOperation` because the account this emulator models cannot
    /// create a launch configuration at all. The collection stays here because
    /// `DescribeLaunchConfigurations`, `DeleteLaunchConfiguration` and a group's
    /// `LaunchConfigurationName` all answer FROM it rather than from a constant,
    /// so each of them reports what the store holds.
    pub launch_configurations: BTreeMap<String, StoredLaunchConfiguration>,
    /// Auto Scaling groups, keyed by `AutoScalingGroupName`.
    pub groups: BTreeMap<String, StoredGroup>,
    /// Every scaling activity this service has recorded, oldest first.
    pub activities: Vec<StoredActivity>,
    /// Source of activity ids.
    pub next_activity_seq: u64,
    /// Source of the opaque id segment of a resource ARN.
    pub next_resource_seq: u64,
}

/// A shorter name for this service's state, for use inside the crate.
pub type AutoScalingState = ComAmazonawsAutoscalingAutoscaling20110101State;

/// Render `seq` in the 8-4-4-4-12 shape AWS resource ids use, so an id this
/// emulator mints has the form a client expects while staying deterministic
/// within a process (version nibble 4, variant nibble 8, as RFC 4122 lays them out).
fn uuid_shaped(seq: u64) -> String {
    format!("{:08x}-0000-4000-8000-{:012x}", seq & 0xffff_ffff, seq)
}

impl AutoScalingState {
    /// Mint the next activity id.
    pub fn mint_activity_id(&mut self) -> String {
        self.next_activity_seq += 1;
        uuid_shaped(self.next_activity_seq)
    }

    /// Mint the opaque id segment an Auto Scaling ARN carries.
    pub fn mint_resource_id(&mut self) -> String {
        self.next_resource_seq += 1;
        uuid_shaped(self.next_resource_seq)
    }

    /// Append a scaling activity that finishes at `completes_at`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_activity(
        &mut self,
        group_name: &str,
        group_arn: &str,
        description: String,
        cause: String,
        group_state: &str,
        start_time: SystemTime,
        completes_at: Option<SystemTime>,
    ) {
        let finished = completes_at.is_none();
        let status_code = if finished {
            ACTIVITY_SUCCESSFUL
        } else {
            ACTIVITY_IN_PROGRESS
        };
        self.push_activity(StoredActivity {
            activity_id: String::new(),
            auto_scaling_group_name: group_name.to_string(),
            auto_scaling_group_arn: Some(group_arn.to_string()),
            description: Some(description),
            cause,
            start_time,
            end_time: if finished { Some(start_time) } else { None },
            status_code: status_code.to_string(),
            progress: if finished { 100 } else { 50 },
            details: None,
            auto_scaling_group_state: Some(group_state.to_string()),
            completes_at,
        });
    }

    /// Append a scaling activity that FAILED: the capacity change asked for an
    /// instance Amazon EC2 did not place, so the group reports no instance for it and
    /// the activity it recorded says so instead of claiming a launch that never
    /// happened.
    pub fn record_failed_activity(
        &mut self,
        group_name: &str,
        group_arn: &str,
        description: String,
        cause: String,
        start_time: SystemTime,
    ) {
        self.push_activity(StoredActivity {
            activity_id: String::new(),
            auto_scaling_group_name: group_name.to_string(),
            auto_scaling_group_arn: Some(group_arn.to_string()),
            description: Some(description),
            cause,
            start_time,
            // Nothing is owed on a launch that failed: it ended when it failed.
            end_time: Some(start_time),
            status_code: ACTIVITY_FAILED.to_string(),
            progress: 100,
            details: None,
            auto_scaling_group_state: Some(GROUP_STATE_IN_SERVICE.to_string()),
            completes_at: None,
        });
    }

    /// Store one activity under a freshly minted id, keeping the log bounded.
    ///
    /// The ONE writer of [`Self::activities`], so the id every activity carries comes
    /// from one minter and the retention bound is applied in one place however the
    /// activity ended: `activity_id` on the way in is ignored.
    fn push_activity(&mut self, activity: StoredActivity) {
        let activity_id = self.mint_activity_id();
        self.activities.push(StoredActivity {
            activity_id,
            ..activity
        });
        // Keep the activity log bounded: the oldest activity is the first to go.
        if self.activities.len() > MAX_RETAINED_ACTIVITIES {
            let excess = self.activities.len() - MAX_RETAINED_ACTIVITIES;
            self.activities.drain(..excess);
        }
    }

    /// Materialise everything the clock has already made due at `now`: deferred
    /// capacity changes whose cooldown has elapsed, instance lifecycle
    /// transitions, and activity completions.
    ///
    /// Every handler calls this first — including the reporting ones, which may
    /// write exactly this much and nothing else, because each of these changes was
    /// owed at `now` whether or not anybody called.
    ///
    /// `engine` is the shared engine the handler holds: a deferred capacity change
    /// launches and terminates the account's own Amazon EC2 instances when it comes
    /// due, exactly as the change a request applies straight away does.
    pub fn materialize_due<Eng: Send + Sync + 'static>(&mut self, engine: &Eng, now: SystemTime) {
        let due_changes: Vec<(String, i32, String, SystemTime)> = self
            .groups
            .values()
            .filter_map(|group| {
                group
                    .pending_capacity
                    .as_ref()
                    .filter(|pending| pending.due <= now)
                    .map(|pending| {
                        (
                            group.name.clone(),
                            // The group's bounds may have been narrowed while the
                            // change waited, and a group never holds a capacity
                            // outside its own bounds, so the deferred value is
                            // brought inside the ones in force when it is applied.
                            pending
                                .desired_capacity
                                .clamp(group.min_size, group.max_size),
                            pending.cause.clone(),
                            pending.due,
                        )
                    })
            })
            .collect();
        // Applied AT the instant it came due, not at the instant somebody looked:
        // the instances it launches and the cooldown it starts are anchored there,
        // so the passes below retire whatever that instant has itself made due and
        // the state a caller sees does not depend on when they read.
        for (name, desired_capacity, cause, due) in due_changes {
            self.apply_capacity(engine, &name, desired_capacity, &cause, due);
        }

        for group in self.groups.values_mut() {
            for instance in &mut group.instances {
                if instance.transition_due.is_some_and(|due| due <= now) {
                    instance.transition_due = None;
                    if instance.lifecycle_state == LIFECYCLE_PENDING {
                        instance.lifecycle_state = LIFECYCLE_IN_SERVICE.to_string();
                    }
                }
            }
            // An instance whose `Terminating` span has elapsed has left the group.
            group
                .instances
                .retain(|i| i.lifecycle_state != LIFECYCLE_TERMINATING || i.transition_due.is_some());
        }

        for activity in &mut self.activities {
            if activity.completes_at.is_some_and(|at| at <= now) {
                activity.end_time = activity.completes_at;
                activity.completes_at = None;
                activity.status_code = ACTIVITY_SUCCESSFUL.to_string();
                activity.progress = 100;
            }
        }
    }

    /// Set a group's desired capacity and move its instances to match: launch what
    /// is missing (`Pending`, in service once [`LIFECYCLE_TRANSITION`] is due) and
    /// scale in what is surplus (`Terminating`, gone once it is due). Each launch
    /// and each termination records its own scaling activity, and the group enters
    /// its cooldown.
    ///
    /// The instances are the account's OWN Amazon EC2 instances: the launch goes
    /// through [`ec2_instances::launch`], which mints them in the account's Amazon EC2
    /// store from the group's launch source, and the scale-in terminates those same
    /// instances there — so every id the group reports resolves in Amazon EC2. A unit
    /// of capacity Amazon EC2 could not place records a failed activity instead of an
    /// instance, since there is no instance to report.
    ///
    /// Does nothing when `group_name` names no group. The capacity is clamped to
    /// [`MAX_GROUP_CAPACITY`] here as well as refused by the handlers, so the
    /// allocation below is bounded however it is reached.
    pub fn apply_capacity<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        desired_capacity: i32,
        cause: &str,
        now: SystemTime,
    ) {
        let desired_capacity = desired_capacity.clamp(0, MAX_GROUP_CAPACITY);
        let Some(group) = self.groups.get(group_name) else {
            return;
        };
        let arn = group.arn.clone();
        let region = group.region.clone();
        let zones = group.availability_zones.clone();
        let vpc_zone_identifier = group.vpc_zone_identifier.clone();
        let launch_configuration_name = group.launch_configuration_name.clone();
        let launch_template = group.launch_template.clone();
        let mixed_instances_policy = group.mixed_instances_policy.clone();
        let tags = group.tags.clone();
        let protected = group.new_instances_protected_from_scale_in;
        let cooldown = group.default_cooldown.max(0);
        let previous = group.active_instance_count();
        let placed = group.instances.len();
        let headroom = group.launch_headroom();
        let launch_configuration = launch_configuration_name
            .as_ref()
            .and_then(|name| self.launch_configurations.get(name))
            .cloned();

        let to_launch = if zones.is_empty() {
            // Without a zone to place an instance in, nothing can be launched; the
            // handlers refuse such a request before reaching here.
            0
        } else {
            // Bounded by the ceiling AND by what the group can still hold, so
            // neither one request nor a run of them can grow the stored instances
            // past MAX_GROUP_CAPACITY. The handlers refuse a change that does not
            // fit; this is the floor under them.
            (desired_capacity - previous)
                .clamp(0, MAX_GROUP_CAPACITY)
                .min(headroom)
        };
        let to_terminate = (previous - desired_capacity).max(0);
        let due = now + LIFECYCLE_TRANSITION;

        let wanted = usize::try_from(to_launch).unwrap_or(0);
        let launched = ec2_instances::launch(
            engine,
            &ec2_instances::LaunchRequest {
                region: &region,
                group_name,
                count: wanted,
                placed,
                zones: &zones,
                vpc_zone_identifier: vpc_zone_identifier.as_deref(),
                launch_configuration: launch_configuration.as_ref(),
                launch_template: launch_template.as_ref(),
                mixed_instances_policy: mixed_instances_policy.as_ref(),
                tags: &tags,
            },
        );
        // A unit of capacity Amazon EC2 did not place is a launch that failed, so the
        // group is given no instance for it: an id it reported would resolve nowhere.
        let unplaced = wanted.saturating_sub(launched.len());
        let launched: Vec<StoredInstance> = launched
            .into_iter()
            .map(|instance| StoredInstance {
                instance_id: instance.instance_id,
                availability_zone: instance.availability_zone,
                lifecycle_state: LIFECYCLE_PENDING.to_string(),
                health_status: HEALTH_STATUS_HEALTHY.to_string(),
                launch_configuration_name: launch_configuration_name.clone(),
                launch_template: launch_template.clone(),
                // What Amazon EC2 launched it as, so the two services report one
                // instance the same way.
                instance_type: instance.instance_type,
                image_id: instance.image_id,
                protected_from_scale_in: protected,
                transition_due: Some(due),
            })
            .collect();

        let group = self
            .groups
            .get_mut(group_name)
            .expect("the group was present a moment ago");
        group.desired_capacity = desired_capacity;
        group.pending_capacity = None;
        group.cooldown_until = Some(now + Duration::from_secs(u64::from(cooldown.unsigned_abs())));
        let launched_ids: Vec<String> = launched
            .iter()
            .map(|instance| instance.instance_id.clone())
            .collect();
        group.instances.extend(launched);

        // Scale in the most recently launched instances first. The group's
        // `TerminationPolicies` are persisted and reported as configured, but the
        // `Default` policy's own inputs (Availability Zone rebalancing, billing
        // hour, launch template age) are not modelled here, so selection is by
        // launch order. An instance protected from scale-in is never chosen: that
        // is what the protection means.
        let mut terminated_ids = Vec::new();
        let mut remaining = to_terminate;
        for instance in group.instances.iter_mut().rev() {
            if remaining == 0 {
                break;
            }
            if instance.lifecycle_state == LIFECYCLE_TERMINATING || instance.protected_from_scale_in
            {
                continue;
            }
            instance.lifecycle_state = LIFECYCLE_TERMINATING.to_string();
            instance.transition_due = Some(due);
            terminated_ids.push(instance.instance_id.clone());
            remaining -= 1;
        }

        // The instances leaving the group are terminated where they live: the same
        // Amazon EC2 instances the launch minted, so the account sees them shut down
        // rather than vanish from one service's view alone.
        ec2_instances::terminate(engine, &terminated_ids);

        for instance_id in launched_ids {
            self.record_activity(
                group_name,
                &arn,
                format!("Launching a new EC2 instance: {instance_id}"),
                cause.to_string(),
                GROUP_STATE_IN_SERVICE,
                now,
                Some(due),
            );
        }
        for instance_id in terminated_ids {
            self.record_activity(
                group_name,
                &arn,
                format!("Terminating EC2 instance: {instance_id}"),
                cause.to_string(),
                GROUP_STATE_IN_SERVICE,
                now,
                Some(due),
            );
        }
        for _ in 0..unplaced {
            self.record_failed_activity(
                group_name,
                &arn,
                "Launching a new EC2 instance failed: the account could not place it"
                    .to_string(),
                cause.to_string(),
                now,
            );
        }
    }

    /// The cause an Auto Scaling capacity change records: what asked for it, and
    /// the capacities it moved between.
    pub fn capacity_change_cause(reason: &str, from: i32, to: i32) -> String {
        format!("{reason}: changing the desired capacity from {from} to {to}.")
    }
}

/// The values a `HealthCheckType` may name, from the model's own documentation
/// for `CreateAutoScalingGroup$HealthCheckType`: "A comma-separated value string
/// of one or more health check types. The valid values are EC2, EBS, ELB, and
/// VPC_LATTICE."
pub const HEALTH_CHECK_TYPES: &[&str] = &["EC2", "EBS", "ELB", "VPC_LATTICE"];

/// Whether `health_check_type` is a comma-separated list of [`HEALTH_CHECK_TYPES`].
pub fn is_valid_health_check_type(health_check_type: &str) -> bool {
    !health_check_type.is_empty()
        && health_check_type
            .split(',')
            .all(|part| HEALTH_CHECK_TYPES.contains(&part.trim()))
}

/// The policies a `TerminationPolicies` entry may name, from the model's own
/// documentation for `CreateAutoScalingGroup$TerminationPolicies`: "Valid values:
/// Default | AllocationStrategy | ClosestToNextInstanceHour | NewestInstance |
/// OldestInstance | OldestLaunchConfiguration | OldestLaunchTemplate |
/// arn:aws:lambda:region:account-id:function:my-function:my-alias".
pub const TERMINATION_POLICIES: &[&str] = &[
    "Default",
    "AllocationStrategy",
    "ClosestToNextInstanceHour",
    "NewestInstance",
    "OldestInstance",
    "OldestLaunchConfiguration",
    "OldestLaunchTemplate",
];

/// The prefix of the custom-policy form that documentation ends with: a Lambda
/// function ARN.
pub const TERMINATION_POLICY_LAMBDA_PREFIX: &str = "arn:aws:lambda:";

/// Whether `policy` is one of [`TERMINATION_POLICIES`] or a Lambda function ARN.
pub fn is_valid_termination_policy(policy: &str) -> bool {
    TERMINATION_POLICIES.contains(&policy) || policy.starts_with(TERMINATION_POLICY_LAMBDA_PREFIX)
}

// ---------------------------------------------------------------------------
// The reference-data catalogs
//
// Amazon EC2 Auto Scaling publishes a handful of fixed catalogs through
// zero-input describes (`DescribeScalingProcessTypes`,
// `DescribeTerminationPolicyTypes`, `DescribeAdjustmentTypes`,
// `DescribeAutoScalingNotificationTypes`, `DescribeLifecycleHookTypes`,
// `DescribeMetricCollectionTypes`). They are SERVICE data, not per-account
// state: the same answer for every caller, in every region, whatever the store
// holds — so they live here as constants and the handlers that report them take
// no lock and write nothing.
//
// Each catalog's doc comment names where its membership and ordering come
// from: normally the model member whose documentation publishes them, and
// where that documentation is behind the service, what it omits.
// ---------------------------------------------------------------------------

/// The processes `SuspendProcesses` and `ResumeProcesses` name, and the catalog
/// `DescribeScalingProcessTypes` reports.
///
/// Membership from the model's own documentation for `ProcessType$ProcessName`
/// ("One of the following processes: Launch, Terminate, AddToLoadBalancer,
/// AlarmNotification, AZRebalance, HealthCheck, InstanceRefresh,
/// ReplaceUnhealthy, ScheduledActions"). Held in ASCII order, which is the order
/// the service reports the catalog in.
pub const SCALING_PROCESS_TYPES: &[&str] = &[
    "AZRebalance",
    "AddToLoadBalancer",
    "AlarmNotification",
    "HealthCheck",
    "InstanceRefresh",
    "Launch",
    "ReplaceUnhealthy",
    "ScheduledActions",
    "Terminate",
];

/// The type name the custom termination policy is published under.
///
/// [`TERMINATION_POLICIES`] lists the policies a group may name literally and
/// [`TERMINATION_POLICY_LAMBDA_PREFIX`] accepts the ARN form beside them; the
/// catalog names that form by its TYPE rather than by any one function's ARN.
pub const TERMINATION_POLICY_TYPE_LAMBDA: &str = "Lambda";

/// The catalog `DescribeTerminationPolicyTypes` reports: every policy
/// [`TERMINATION_POLICIES`] accepts by name, plus
/// [`TERMINATION_POLICY_TYPE_LAMBDA`] for the Lambda-ARN form the same model
/// documentation ends with, in ASCII order.
pub const TERMINATION_POLICY_TYPES: &[&str] = &[
    "AllocationStrategy",
    "ClosestToNextInstanceHour",
    "Default",
    TERMINATION_POLICY_TYPE_LAMBDA,
    "NewestInstance",
    "OldestInstance",
    "OldestLaunchConfiguration",
    "OldestLaunchTemplate",
];

/// The catalog `DescribeAdjustmentTypes` reports, from the model's own
/// documentation for `AdjustmentType$AdjustmentType`: "The policy adjustment
/// type. The valid values are ChangeInCapacity, ExactCapacity, and
/// PercentChangeInCapacity." In ASCII order.
pub const ADJUSTMENT_TYPES: &[&str] = &[
    "ChangeInCapacity",
    "ExactCapacity",
    "PercentChangeInCapacity",
];

/// The catalog `DescribeLifecycleHookTypes` reports: the lifecycle transitions a
/// lifecycle hook may name, from the model's own documentation for
/// `PutLifecycleHookType$LifecycleTransition` ("To create a lifecycle hook for
/// scale-out events, specify autoscaling:EC2_INSTANCE_LAUNCHING. To create a
/// lifecycle hook for scale-in events, specify
/// autoscaling:EC2_INSTANCE_TERMINATING"). In ASCII order.
pub const LIFECYCLE_HOOK_TYPES: &[&str] = &[
    "autoscaling:EC2_INSTANCE_LAUNCHING",
    "autoscaling:EC2_INSTANCE_TERMINATING",
];

/// The catalog `DescribeAutoScalingNotificationTypes` reports: the event types a
/// group can notify about, from the model's own documentation for
/// `NotificationConfiguration$NotificationType` ("One of the following event
/// notification types: …"), in the order that documentation publishes them.
///
/// The lifecycle transitions of [`LIFECYCLE_HOOK_TYPES`] are a SEPARATE catalog,
/// published by `DescribeLifecycleHookTypes`, even though the model carries both
/// through the one `AutoScalingNotificationTypes` list shape (it is the target of
/// this operation's output member and of that one's alike): a transition is what a
/// lifecycle hook is created for, not an event a group notifies about, and the
/// model documents each list on its own member.
pub const AUTO_SCALING_NOTIFICATION_TYPES: &[&str] = &[
    "autoscaling:EC2_INSTANCE_LAUNCH",
    "autoscaling:EC2_INSTANCE_LAUNCH_ERROR",
    "autoscaling:EC2_INSTANCE_TERMINATE",
    "autoscaling:EC2_INSTANCE_TERMINATE_ERROR",
    "autoscaling:TEST_NOTIFICATION",
];

/// The catalog `DescribeMetricCollectionTypes` reports as `Metrics`: the group
/// metrics `EnableMetricsCollection` and `DisableMetricsCollection` name.
///
/// The model's own documentation for `MetricCollectionType$Metric` ("One of the
/// following metrics: …") enumerates only the twenty metrics that predate the
/// retained-capacity series, so that prose is a LOWER BOUND on what the service
/// publishes rather than the catalog itself: the warm pool's own minimum
/// (`WarmPoolMinSize`) and the four retained-capacity series a group's instance
/// maintenance policy reports (`GroupTerminatingRetainedInstances`,
/// `GroupTerminatingRetainedCapacity`, `WarmPoolPendingRetainedCapacity`,
/// `WarmPoolTerminatingRetainedCapacity`) are enabled by name through
/// `EnableMetricsCollection` and published by this catalog without appearing in
/// it.
///
/// Grouped the way that documentation groups what it does enumerate — sizes,
/// then instance counts, then capacities, then the warm pool — with each metric
/// the prose omits beside the one it refines.
pub const METRIC_COLLECTION_TYPES: &[&str] = &[
    "GroupMinSize",
    "GroupMaxSize",
    "GroupDesiredCapacity",
    "GroupInServiceInstances",
    "GroupPendingInstances",
    "GroupStandbyInstances",
    "GroupTerminatingInstances",
    "GroupTerminatingRetainedInstances",
    "GroupTotalInstances",
    "GroupInServiceCapacity",
    "GroupPendingCapacity",
    "GroupStandbyCapacity",
    "GroupTerminatingCapacity",
    "GroupTerminatingRetainedCapacity",
    "GroupTotalCapacity",
    "WarmPoolMinSize",
    "WarmPoolDesiredCapacity",
    "WarmPoolWarmedCapacity",
    "WarmPoolPendingCapacity",
    "WarmPoolPendingRetainedCapacity",
    "WarmPoolTerminatingCapacity",
    "WarmPoolTerminatingRetainedCapacity",
    "WarmPoolTotalCapacity",
    "GroupAndWarmPoolDesiredCapacity",
    "GroupAndWarmPoolTotalCapacity",
];

/// Whether `metric` is one of [`METRIC_COLLECTION_TYPES`].
///
/// The metrics `EnableMetricsCollection` and `DisableMetricsCollection` accept
/// are the metrics this catalog publishes, so both sides check membership
/// through here and cannot drift apart. NOT YET IN FORCE on the wire: those two
/// operations are still scaffolded (each owned by its own issue), so nothing in
/// `src/ops/` validates a metric name today — this is the seam they are to use
/// when they are implemented.
pub fn is_valid_metric_collection_type(metric: &str) -> bool {
    METRIC_COLLECTION_TYPES.contains(&metric)
}

/// The catalog `DescribeMetricCollectionTypes` reports as `Granularities`, from
/// the model's own documentation for `MetricGranularityType$Granularity`: "The
/// granularity. The only valid value is 1Minute."
pub const METRIC_GRANULARITIES: &[&str] = &["1Minute"];

/// The units a `DesiredCapacityType` may name, from the model's own documentation
/// for `CreateAutoScalingGroup$DesiredCapacityType`: "Valid values: units | vcpu |
/// memory-mib".
pub const DESIRED_CAPACITY_TYPES: &[&str] = &["units", "vcpu", "memory-mib"];

/// Whether `name` satisfies the character rule the model documents for
/// `AutoScalingGroupName`: "The name can contain any ASCII character 33 to 126
/// including most punctuation characters, digits, and upper and lowercased
/// letters. You cannot use a colon (:) in the name."
pub fn is_valid_group_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| matches!(c, '!'..='~') && c != ':')
}

/// The smallest non-zero `MaxInstanceLifetime` the service accepts.
///
/// From the model's own documentation for
/// `CreateAutoScalingGroup$MaxInstanceLifetime`: "If specified, the value must be
/// either 0 or a number equal to or greater than 86,400 seconds (1 day)."
pub const MIN_MAX_INSTANCE_LIFETIME: i32 = 86_400;

/// Whether `seconds` satisfies that rule.
pub fn is_valid_max_instance_lifetime(seconds: i32) -> bool {
    seconds == 0 || seconds >= MIN_MAX_INSTANCE_LIFETIME
}

/// The first of `sizes` past [`MAX_GROUP_CAPACITY`], if any.
///
/// Every operation that sets a group's sizes checks them through this, so one
/// request cannot store bounds whose capacity this process could not hold, and
/// the refusal can name the offending value.
pub fn size_past_ceiling(sizes: &[i32]) -> Option<i32> {
    sizes.iter().copied().find(|size| *size > MAX_GROUP_CAPACITY)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAutoscalingAutoscaling20110101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAutoscalingAutoscaling20110101State>)
}

/// The region a request is signed for, which is the region the resources it
/// creates live in. Falls back to the emulator's default region when the request
/// carries no SigV4 credential scope (an unsigned call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's resources live in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The tracing target every audit record this service emits carries.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the structured audit record a state-changing operation owes: the
/// operation, the resource it changed, how the call ended, and which credential
/// was presented for it.
///
/// Only identifiers are recorded. The presented access key id names the caller;
/// the signature, the secret and the request body never appear here, and neither
/// does any other header. The subscriber stamps the time.
pub fn audit(parts: &::http::request::Parts, operation: &str, resource: &str, outcome: &str) {
    ::tracing::info!(
        target: AUDIT_TARGET,
        operation,
        resource,
        outcome,
        region = %request_region(parts),
        access_key_id = %::engine::extract_akid(&parts.headers),
        "Auto Scaling state change"
    );
}

/// Convert a stored instant to the wire timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The ARN of an Auto Scaling group, in the form the service reports:
/// `arn:aws:autoscaling:{region}:{account}:autoScalingGroup:{id}:autoScalingGroupName/{name}`.
pub fn group_arn(region: &str, resource_id: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "autoscaling",
        region,
        &format!("autoScalingGroup:{resource_id}:autoScalingGroupName/{name}"),
    )
}

/// Decode a `NextToken` into the offset it names, or `None` when the token is not
/// one this service issued.
///
/// The paginated describes in this service page over a name-ordered list, so an
/// issued token is the offset the next page starts at, behind a marker: anything
/// without the marker, and the offset 0 (the first page is reached with no token
/// at all, so no token for it is ever issued), is not a token this service gave
/// out.
pub fn decode_next_token(token: &str) -> Option<usize> {
    token
        .strip_prefix(NEXT_TOKEN_MARKER)?
        .parse::<usize>()
        .ok()
        .filter(|offset| *offset > 0)
}

/// The message a paginated read refuses an unusable `NextToken` with.
///
/// The REQUIREMENT here is that the refusal quotes the token the caller sent —
/// that is the only thing the caller can act on, since the token it is holding is
/// not one this service will resume from. The surrounding wording is an example,
/// not something the model or a recording pins down.
pub fn invalid_next_token_message(token: &str) -> String {
    format!("The token '{token}' was invalid")
}

/// Encode the offset the next page starts at as a `NextToken`.
pub fn encode_next_token(offset: usize) -> String {
    format!("{NEXT_TOKEN_MARKER}{offset}")
}

/// The marker every `NextToken` this service issues carries, so a token it never
/// issued is refused rather than read as an offset.
const NEXT_TOKEN_MARKER: &str = "next-";

/// One page of a name-ordered list, plus the `NextToken` that follows it.
///
/// `max_records` is the request's `MaxRecords`; absent or out of range it falls
/// back to [`DEFAULT_MAX_RECORDS`], and it is capped at [`MAX_MAX_RECORDS`].
pub fn page<T: Clone>(items: &[T], offset: usize, max_records: Option<i32>) -> (Vec<T>, Option<String>) {
    let limit = page_limit(max_records);
    let start = offset.min(items.len());
    let end = start.saturating_add(limit).min(items.len());
    let next = (end < items.len()).then(|| encode_next_token(end));
    (items[start..end].to_vec(), next)
}

/// How many items one page holds, from the request's `MaxRecords`: absent or
/// non-positive it falls back to [`DEFAULT_MAX_RECORDS`], and it is capped at
/// [`MAX_MAX_RECORDS`].
///
/// Separate from [`page`] because a describe that pages over its own cursor
/// rather than over an offset still bounds its page the same way.
pub fn page_limit(max_records: Option<i32>) -> usize {
    max_records
        .filter(|value| *value > 0)
        .map_or(DEFAULT_MAX_RECORDS, |value| {
            (value as usize).min(MAX_MAX_RECORDS)
        })
}

/// A request's `MaxRecords` when it is outside the range the model documents
/// ("The default value is 50 and the maximum value is 100"), for an operation to
/// refuse: `None` when the request omits it or carries an acceptable value.
pub fn max_records_out_of_range(max_records: Option<i32>) -> Option<i32> {
    max_records.filter(|value| *value < 1 || usize::try_from(*value).unwrap_or(usize::MAX) > MAX_MAX_RECORDS)
}

/// The most names a describe may filter by, which the model documents on
/// `AutoScalingGroupNames`: "By default, you can only specify up to 50 names. You
/// can optionally increase this limit using the MaxRecords property."
pub fn max_filter_names(max_records: Option<i32>) -> usize {
    max_records
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_MAX_RECORDS)
        .max(DEFAULT_MAX_RECORDS)
}

/// `MaxRecords` when the request omits it.
///
/// From the model's own documentation for `AutoScalingGroupNamesType$MaxRecords`:
/// "The default value is 50 and the maximum value is 100."
pub const DEFAULT_MAX_RECORDS: usize = 50;

/// The largest `MaxRecords` the service honours (same documentation).
pub const MAX_MAX_RECORDS: usize = 100;
