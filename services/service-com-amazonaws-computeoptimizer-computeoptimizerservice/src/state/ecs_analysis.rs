// SPDX-License-Identifier: CC0-1.0

//! The Amazon ECS service analysis the ECS readers of this service report: the ECS
//! inventory and the published utilization it reads, and the Fargate sizing it performs.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

/// The CloudWatch namespace Amazon ECS publishes a service's utilization in, and the two
/// series an ECS service recommendation is computed from — the metrics the model's own
/// `ECSServiceRecommendation$findingReasonCodes` doc names ("analyzing the
/// `CPUUtilization` metric of the current service during the look-back period", and the
/// same for `MemoryUtilization`).
pub const ECS_METRIC_NAMESPACE: &str = "AWS/ECS";
/// See [`ECS_METRIC_NAMESPACE`].
pub const ECS_CPU_UTILIZATION_METRIC: &str = "CPUUtilization";
/// See [`ECS_METRIC_NAMESPACE`].
pub const ECS_MEMORY_UTILIZATION_METRIC: &str = "MemoryUtilization";

/// The dimensions that identify ONE service's series in that namespace.
pub(super) const ECS_CLUSTER_DIMENSION: &str = "ClusterName";
/// See [`ECS_CLUSTER_DIMENSION`].
pub(super) const ECS_SERVICE_DIMENSION: &str = "ServiceName";

/// The ARN namespace Amazon ECS resources carry, and the resource KIND an ECS service's
/// ARN names (`service/cluster-name/service-name`).
pub(super) const ECS_ARN_NAMESPACE: &str = "ecs";
/// See [`ECS_ARN_NAMESPACE`].
pub(super) const ECS_SERVICE_RESOURCE_KIND: &str = "service";

/// The `ECSServiceLaunchType` values (`E_C_S_SERVICE_LAUNCH_TYPE_VALUES`). Compute
/// Optimizer "generates recommendations for Amazon ECS services on Fargate", so only
/// [`ECS_SERVICE_LAUNCH_TYPE_FARGATE`] is an analysable service.
pub const ECS_SERVICE_LAUNCH_TYPE_FARGATE: &str = "Fargate";
/// See [`ECS_SERVICE_LAUNCH_TYPE_FARGATE`].
pub const ECS_SERVICE_LAUNCH_TYPE_EC2: &str = "EC2";

/// The ECS `LaunchType` enum values (the ECS model's `LAUNCH_TYPE_VALUES`) this analysis
/// reads, and the ECS capacity providers that run a task on Fargate. ECS spells them in
/// upper case; Compute Optimizer spells its own `ECSServiceLaunchType` in mixed case.
pub(super) const ECS_LAUNCH_TYPE_FARGATE: &str = "FARGATE";
/// See [`ECS_LAUNCH_TYPE_FARGATE`].
pub(super) const ECS_LAUNCH_TYPE_EC2: &str = "EC2";
/// See [`ECS_LAUNCH_TYPE_FARGATE`].
pub(super) const ECS_FARGATE_CAPACITY_PROVIDERS: [&str; 2] = ["FARGATE", "FARGATE_SPOT"];

/// The task-level CPU/memory combinations AWS Fargate supports, smallest first: CPU units
/// paired with every task memory (MiB) that CPU size admits.
///
/// Verbatim from the Amazon ECS model's own `RegisterTaskDefinitionRequest$memory`
/// documentation ("If using the Fargate launch type, this field is required and you must
/// use one of the following values. This determines your range of supported values for the
/// `cpu` parameter"), which lists: 512/1024/2048 MiB for 256 CPU units; 1024-4096 in 1 GB
/// steps for 512; 2048-8192 in 1 GB steps for 1024; "Between 4096 (4 GB) and 16384 (16 GB)
/// in increments of 1024 (1 GB)" for 2048; "Between 8192 (8 GB) and 30720 (30 GB) in
/// increments of 1024 (1 GB)" for 4096; "Between 16 GB and 60 GB in 4 GB increments" for
/// 8192; "Between 32GB and 120 GB in 8 GB increments" for 16384.
///
/// A recommendation option is picked from THIS list, so the size reported back is one a
/// Fargate task definition could actually be registered with.
pub(super) const FARGATE_TASK_SIZES: &[(i32, &[i32])] = &[
    (256, &[512, 1024, 2048]),
    (512, &[1024, 2048, 3072, 4096]),
    (1024, &[2048, 3072, 4096, 5120, 6144, 7168, 8192]),
    (
        2048,
        &[
            4096, 5120, 6144, 7168, 8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360, 16384,
        ],
    ),
    (
        4096,
        &[
            8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360, 16384, 17408, 18432, 19456,
            20480, 21504, 22528, 23552, 24576, 25600, 26624, 27648, 28672, 29696, 30720,
        ],
    ),
    (
        8192,
        &[
            16384, 20480, 24576, 28672, 32768, 36864, 40960, 45056, 49152, 53248, 57344, 61440,
        ],
    ),
    (
        16384,
        &[
            32768, 40960, 49152, 57344, 65536, 73728, 81920, 90112, 98304, 106496, 114688, 122880,
        ],
    ),
];

/// The Amazon ECS service one ARN names.
#[derive(Debug, Clone)]
pub struct EcsServiceRef {
    /// The ARN as given, which a reader reports back as the service's own.
    pub arn: String,
    /// The ARN's account field: the account that owns the service.
    pub account_id: String,
    /// The ARN's region field: the regional endpoint the service belongs to.
    pub region: String,
    /// The cluster that hosts the service, and the service's name — the pair that keys
    /// the ECS store and names the service's CloudWatch dimensions.
    pub cluster: String,
    /// See [`EcsServiceRef::cluster`].
    pub service: String,
}

/// The ARN of an Amazon ECS service, split into the parts the analysis reads, or `None`
/// for anything that is not one.
///
/// What an ARN looks like at all is [`parse_arn`]'s decision — "the one place this service
/// decides what an ARN looks like, so every operation that takes one accepts and refuses the
/// same strings" — and this adds what the member documents on top of it:
/// `arn:aws:ecs:region:aws_account_id:service/cluster-name/service-name`
/// (`GetECSServiceRecommendations$serviceArns`, `GetECSServiceRecommendationProjectedMetrics$serviceArn`),
/// whose account field must be an account id and whose resource part names the CLUSTER as
/// well as the service. The cluster/name split is the ECS store's own
/// (`ecs::state::cluster_and_service_from_reference`), which is what keeps these readers and
/// the service that owns the record from disagreeing on which service an ARN names.
pub fn parse_ecs_service_arn(arn: &str) -> Option<EcsServiceRef> {
    let parsed = parse_arn(arn)?;
    if parsed.service != ECS_ARN_NAMESPACE
        || parsed.resource_kind() != ECS_SERVICE_RESOURCE_KIND
        || parsed.region.is_empty()
        || !is_account_id(parsed.account_id)
    {
        return None;
    }
    let (cluster, service) = ecs::state::cluster_and_service_from_reference(arn);
    let cluster = cluster?;
    if cluster.is_empty() || service.is_empty() {
        return None;
    }
    Some(EcsServiceRef {
        arn: arn.to_string(),
        account_id: parsed.account_id.to_string(),
        region: parsed.region.to_string(),
        cluster,
        service,
    })
}

/// The persisted ECS configuration of one service: every value taken from the ECS store,
/// which is why a recommendation can report a current configuration at all.
#[derive(Debug, Clone)]
pub struct EcsServiceConfig {
    /// The service's ECS `launchType`, when it names one.
    pub launch_type: Option<String>,
    /// The capacity providers its `capacityProviderStrategy` names, when it names no
    /// launch type.
    pub capacity_providers: Vec<String>,
    /// The task definition the service runs.
    pub task_definition_arn: Option<String>,
    /// That task definition's task-level `cpu`, in CPU units.
    pub cpu_units: Option<i32>,
    /// That task definition's task-level `memory`, in MiB.
    pub memory_mib: Option<i32>,
    /// How many tasks the service keeps running (`Service$desiredCount`): the multiplier
    /// between what ONE task of the size above costs per month and what the SERVICE does.
    pub desired_tasks: i32,
    /// Its container definitions, as the modelled `ContainerConfiguration` list.
    pub containers: Vec<ContainerConfiguration>,
    /// The service's tags.
    pub tags: Vec<Tag>,
}

/// What the ECS store holds for one service, or `None` when it holds no ACTIVE record for
/// it. A service whose record is INACTIVE (deleted) is gone as far as a reader is
/// concerned, exactly as `DescribeServices` reports it.
pub fn ecs_service_config(
    inventory: &ecs::state::ComAmazonawsEcsAmazonec2containerservicev20141113State,
    reference: &EcsServiceRef,
) -> Option<EcsServiceConfig> {
    let record = inventory
        .services
        .get(&(reference.cluster.clone(), reference.service.clone()))
        .filter(|record| record.is_active())?;
    let definition = record
        .task_definition
        .as_ref()
        .and_then(|(family, revision)| {
            inventory.task_definitions.get(&(family.clone(), *revision))
        });
    Some(EcsServiceConfig {
        launch_type: record.launch_type.clone(),
        capacity_providers: record
            .capacity_provider_strategy
            .iter()
            .map(|item| item.capacity_provider.clone())
            .collect(),
        task_definition_arn: record.task_definition_arn.clone(),
        cpu_units: definition
            .and_then(|definition| definition.cpu.as_deref())
            .and_then(ecs_cpu_units),
        memory_mib: definition
            .and_then(|definition| definition.memory.as_deref())
            .and_then(ecs_memory_mib),
        desired_tasks: record.desired_count,
        containers: definition
            .map(|definition| {
                definition
                    .container_definitions
                    .iter()
                    .map(|container| ContainerConfiguration {
                        container_name: container.name.clone(),
                        memory_size_configuration: (container.memory.is_some()
                            || container.memory_reservation.is_some())
                        .then_some(MemorySizeConfiguration {
                            memory: container.memory,
                            memory_reservation: container.memory_reservation,
                        }),
                        // The ECS member defaults to `0`, which is "no container-level
                        // CPU reservation" rather than a reservation of nothing.
                        cpu: (container.cpu > 0).then_some(container.cpu),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        tags: record
            .tags
            .iter()
            .map(|tag| Tag {
                key: tag.key.clone(),
                value: tag.value.clone(),
            })
            .collect(),
    })
}

/// A task definition's task-level `cpu` as CPU units, or `None` for a value that is
/// neither form the ECS model documents: "It can be expressed as an integer using CPU
/// units (for example, `1024`) or as a string using vCPUs (for example, `1 vCPU` or `1
/// vcpu`) … String values are converted to an integer indicating the CPU units"
/// (`RegisterTaskDefinitionRequest$cpu`). One vCPU is 1024 CPU units.
pub fn ecs_cpu_units(value: &str) -> Option<i32> {
    let value = value.trim();
    let lowered = value.to_ascii_lowercase();
    for suffix in ["vcpus", "vcpu"] {
        if let Some(count) = lowered.strip_suffix(suffix) {
            let count: f64 = count.trim().parse().ok()?;
            return Some(ecs_rounded_units(count * 1024.0));
        }
    }
    value.parse().ok()
}

/// A task definition's task-level `memory` as MiB, or `None` for a value that is neither
/// form the ECS model documents: "It can be expressed as an integer using MiB (for example
/// ,`1024`) or as a string using GB (for example, `1GB` or `1 GB`) … String values are
/// converted to an integer indicating the MiB" (`RegisterTaskDefinitionRequest$memory`).
/// One GB is 1024 MiB.
pub fn ecs_memory_mib(value: &str) -> Option<i32> {
    let value = value.trim();
    let lowered = value.to_ascii_lowercase();
    if let Some(count) = lowered.strip_suffix("gb") {
        let count: f64 = count.trim().parse().ok()?;
        return Some(ecs_rounded_units(count * 1024.0));
    }
    value.parse().ok()
}

/// A capacity in whole units, rounded to the nearest one and bounded by the modelled
/// `i32`, so no parsed or computed size can overflow or go negative.
pub fn ecs_rounded_units(value: f64) -> i32 {
    if !value.is_finite() {
        return i32::MAX;
    }
    value.round().clamp(0.0, f64::from(i32::MAX)) as i32
}

/// A capacity rounded UP to a whole unit: the smallest size that still holds `value`.
pub(super) fn ecs_ceiled_units(value: f64) -> i32 {
    if !value.is_finite() {
        return i32::MAX;
    }
    value.ceil().clamp(0.0, f64::from(i32::MAX)) as i32
}

/// The Compute Optimizer `ECSServiceLaunchType` one service runs with, or `None` for a
/// launch type Compute Optimizer does not classify.
///
/// A service names either a `launchType` or a `capacityProviderStrategy`; the ECS
/// `FARGATE` / `FARGATE_SPOT` capacity providers are the ones that run a task on Fargate.
pub fn ecs_launch_type_of(config: &EcsServiceConfig) -> Option<&'static str> {
    if let Some(launch_type) = config.launch_type.as_deref() {
        return match launch_type {
            ECS_LAUNCH_TYPE_FARGATE => Some(ECS_SERVICE_LAUNCH_TYPE_FARGATE),
            ECS_LAUNCH_TYPE_EC2 => Some(ECS_SERVICE_LAUNCH_TYPE_EC2),
            _ => None,
        };
    }
    config
        .capacity_providers
        .iter()
        .any(|provider| ECS_FARGATE_CAPACITY_PROVIDERS.contains(&provider.as_str()))
        .then_some(ECS_SERVICE_LAUNCH_TYPE_FARGATE)
}

/// One series' observations over the look-back period, as the analysis reads them.
#[derive(Debug, Clone)]
pub struct EcsObserved {
    /// The largest observation in the window — the peak the sizing must hold.
    pub maximum: f64,
    /// The mean of the observations in the window.
    pub average: f64,
    /// The time stamp of the newest observation in the window.
    pub newest_secs: f64,
}

/// What the account published for ONE service's two analysed series over the look-back
/// window: `None` for a series it published nothing in that window, because an analysis
/// without observed utilization would be invented rather than computed.
#[derive(Debug, Clone)]
pub struct EcsObservedUtilization {
    /// The service's `CPUUtilization` observations.
    pub cpu: Option<EcsObserved>,
    /// The service's `MemoryUtilization` observations.
    pub memory: Option<EcsObserved>,
}

impl EcsObservedUtilization {
    /// The two series an analysis needs, or `None` when either was not observed.
    pub fn both(&self) -> Option<(&EcsObserved, &EcsObserved)> {
        self.cpu.as_ref().zip(self.memory.as_ref())
    }
}

/// What the account published for one service's analysed series over the `look_back_period`
/// ending at `now`.
///
/// The fold is CloudWatch's own ([`cloudwatch::state::BasicStatistics`]), the same one
/// `GetMetricStatistics` aggregates a period with, so the utilization this analysis reports
/// is the utilization CloudWatch itself would report for the window; the window's length is
/// the one [`look_back_period_in_days`] states, which is what every reader of this service
/// reports `lookbackPeriodInDays` from.
pub fn ecs_observed_utilization(
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    reference: &EcsServiceRef,
    look_back_period: &str,
    now: f64,
) -> EcsObservedUtilization {
    let oldest = now - look_back_period_in_days(look_back_period) * SECONDS_PER_DAY;
    EcsObservedUtilization {
        cpu: ecs_observed_series(
            observations,
            reference,
            ECS_CPU_UTILIZATION_METRIC,
            oldest,
            now,
        ),
        memory: ecs_observed_series(
            observations,
            reference,
            ECS_MEMORY_UTILIZATION_METRIC,
            oldest,
            now,
        ),
    }
}

/// What the account published for ONE of a service's series over the window, or `None` when
/// it published nothing in it.
pub(super) fn ecs_observed_series(
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    reference: &EcsServiceRef,
    metric_name: &str,
    oldest: f64,
    now: f64,
) -> Option<EcsObserved> {
    let key = cloudwatch::state::MetricKey {
        namespace: ECS_METRIC_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions: [
            (ECS_CLUSTER_DIMENSION.to_string(), reference.cluster.clone()),
            (ECS_SERVICE_DIMENSION.to_string(), reference.service.clone()),
        ]
        .into_iter()
        .collect(),
    };
    let data = observations.metrics.get(&key)?;
    // A datapoint whose aggregates are not finite describes no utilization, so it is left
    // out of the fold exactly as one outside the window is: a series with none left is a
    // series with nothing to analyse, not a service whose utilization folds to NaN.
    let window = data.iter().filter(|datum| {
        datum.timestamp_secs.is_finite()
            && datum.timestamp_secs >= oldest
            && datum.timestamp_secs <= now
            && datum.minimum.is_finite()
            && datum.maximum.is_finite()
            && datum.sum.is_finite()
            && datum.sample_count.is_finite()
    });
    let newest_secs = window
        .clone()
        .map(|datum| datum.timestamp_secs)
        .fold(f64::NEG_INFINITY, f64::max);
    let folded = cloudwatch::state::BasicStatistics::of(window);
    Some(EcsObserved {
        maximum: folded.named(METRIC_STATISTIC_MAXIMUM)?,
        average: folded.average()?,
        newest_secs: newest_secs.is_finite().then_some(newest_secs)?,
    })
}

/// The capacity one dimension of an ECS service needs: the absolute usage an observed peak
/// of `peak_percent` per cent of `current` stands for, carried with the crate's one
/// recommendation headroom ([`RECOMMENDATION_HEADROOM`], the service's documented
/// "`PERCENT_20` for headroom"), rounded up to a whole unit. The SAME arithmetic the EBS
/// sizing applies, so this service states one headroom rule.
pub fn ecs_required_capacity(current: i32, peak_percent: f64) -> i32 {
    let used = f64::from(current) * peak_percent.max(0.0) / 100.0;
    ecs_ceiled_units(used * (1.0 + RECOMMENDATION_HEADROOM))
}

/// The Fargate task size an ECS service's observed peaks are sized to: the smallest size
/// the catalog offers that holds what BOTH dimensions need ([`ecs_required_capacity`]).
///
/// The ONE recommendation option an ECS service recommendation carries, so the option
/// `GetECSServiceRecommendations` reports and the option
/// `GetECSServiceRecommendationProjectedMetrics` projects onto are the same task size.
pub fn ecs_recommended_task_size(
    current_cpu: i32,
    current_memory: i32,
    cpu_peak_percent: f64,
    memory_peak_percent: f64,
) -> (i32, i32) {
    ecs_smallest_fargate_size(
        ecs_required_capacity(current_cpu, cpu_peak_percent),
        ecs_required_capacity(current_memory, memory_peak_percent),
    )
}

/// The smallest Fargate task size that holds `cpu` CPU units and `memory` MiB, from
/// [`FARGATE_TASK_SIZES`]; the largest size in the catalog when nothing in it is big
/// enough, since there is no larger task to recommend.
///
/// CPU is the PRIMARY key: the catalog is walked in CPU order and the first row that can
/// hold the memory requirement wins, so the result is the smallest CPU size that fits —
/// which is the size a memory requirement forces up, exactly as the catalog's own
/// "Available `cpu` values" wording reads (a task memory is offered BY cpu size). A later
/// reader should not reorder this to minimise some combined total: the two dimensions are
/// priced and offered per CPU row, not as a sum.
pub fn ecs_smallest_fargate_size(cpu: i32, memory: i32) -> (i32, i32) {
    for (size_cpu, memories) in FARGATE_TASK_SIZES {
        if *size_cpu < cpu {
            continue;
        }
        if let Some(size_memory) = memories.iter().find(|candidate| **candidate >= memory) {
            return (*size_cpu, *size_memory);
        }
    }
    // Past the catalog's largest size there is no bigger Fargate task to recommend, so the
    // largest one IS the recommendation: the service is still analysable (its utilization
    // was observed and its finding stays `Underprovisioned`, which is exactly the
    // "performance needs are not met" the finding reports), so this is not the
    // unsupported-resource case — that answers a service the analysis has no INPUT for, not
    // one whose demand exceeds what Fargate offers.
    let (largest_cpu, largest_memories) = FARGATE_TASK_SIZES
        .last()
        .expect("the Fargate task size catalog is not empty");
    (
        *largest_cpu,
        *largest_memories
            .last()
            .expect("every Fargate CPU size admits at least one task memory"),
    )
}

/// The smallest task CPU size in [`FARGATE_TASK_SIZES`] that holds `cpu` CPU units, or the
/// catalog's largest when none does. The per-DIMENSION floor an over-provisioned reason
/// code is judged against, so a CPU requirement that fits a smaller size is reported as
/// sizeable-down even when the memory requirement forces the recommended task up.
pub fn ecs_smallest_fargate_cpu(cpu: i32) -> i32 {
    FARGATE_TASK_SIZES
        .iter()
        .map(|(size_cpu, _)| *size_cpu)
        .find(|size_cpu| *size_cpu >= cpu)
        .unwrap_or_else(|| {
            FARGATE_TASK_SIZES
                .last()
                .map_or(cpu, |(largest_cpu, _)| *largest_cpu)
        })
}

/// The smallest task memory [`FARGATE_TASK_SIZES`] offers at ANY CPU size that holds
/// `memory` MiB, or the catalog's largest when none does. The memory counterpart of
/// [`ecs_smallest_fargate_cpu`].
pub fn ecs_smallest_fargate_memory(memory: i32) -> i32 {
    let offered = FARGATE_TASK_SIZES
        .iter()
        .flat_map(|(_, memories)| memories.iter().copied());
    offered
        .clone()
        .filter(|size_memory| *size_memory >= memory)
        .min()
        .unwrap_or_else(|| offered.max().unwrap_or(memory))
}

/// The utilization `observed` per cent of `current` capacity becomes at `option` capacity:
/// the same absolute usage over the new size.
pub fn ecs_projected_metric_value(observed: f64, current: i32, option: i32) -> f64 {
    if option <= 0 {
        return 0.0;
    }
    observed * f64::from(current) / f64::from(option)
}

/// The utilization an ECS service's observed statistics become at the recommended task
/// size, as `ECSServiceRecommendationOption$projectedUtilizationMetrics` reports them:
/// each analysed metric under each statistic the analysis observed it with, projected by
/// the ONE rule [`ecs_projected_metric_value`] states.
///
/// Stated once so the projection an option carries and the series
/// `GetECSServiceRecommendationProjectedMetrics` reports for that option cannot disagree.
/// The projection is a single value computed from the observation, so both reported bounds
/// are that value — a wider interval would claim a spread the observation does not carry.
pub fn ecs_projected_utilization_metrics(
    cpu: &EcsObserved,
    memory: &EcsObserved,
    current: (i32, i32),
    option: (i32, i32),
) -> Vec<ECSServiceProjectedUtilizationMetric> {
    // The metric names are the `ECSServiceMetricName` values ("`Cpu` — The amount of CPU
    // capacity that's used in the service", and the same for `Memory`), which this service's
    // one metric-name vocabulary ([`METRIC_CPU`], [`METRIC_MEMORY`]) already spells; each is
    // projected on the sizes of the dimension it measures.
    [
        (
            METRIC_CPU,
            METRIC_STATISTIC_MAXIMUM,
            cpu.maximum,
            current.0,
            option.0,
        ),
        (
            METRIC_CPU,
            METRIC_STATISTIC_AVERAGE,
            cpu.average,
            current.0,
            option.0,
        ),
        (
            METRIC_MEMORY,
            METRIC_STATISTIC_MAXIMUM,
            memory.maximum,
            current.1,
            option.1,
        ),
        (
            METRIC_MEMORY,
            METRIC_STATISTIC_AVERAGE,
            memory.average,
            current.1,
            option.1,
        ),
    ]
    .into_iter()
    .map(|(name, statistic, observed, current, option)| {
        let value = ecs_projected_metric_value(observed, current, option);
        ECSServiceProjectedUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(statistic.to_string()),
            lower_bound_value: value,
            upper_bound_value: value,
        }
    })
    .collect()
}
