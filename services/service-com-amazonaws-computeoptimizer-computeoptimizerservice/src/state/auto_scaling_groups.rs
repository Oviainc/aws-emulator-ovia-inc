// SPDX-License-Identifier: CC0-1.0

//! The Auto Scaling groups Compute Optimizer has analysed, and the recommendation a reader
//! of one derives from that analysis.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

/// One Auto Scaling group Compute Optimizer has analysed: the group as it is configured,
/// the utilization it observed, and the candidate configurations the analysis found for
/// it. Every value a recommendation REPORTS is either one of these stored observations
/// or a figure computed from them, so two readers of the same row cannot disagree.
#[derive(Debug, Clone)]
pub struct AnalyzedAutoScalingGroup {
    /// The account that owns the group.
    pub account_id: String,
    /// The group's own id — the identifier segment of its ARN. A row keeps the group's
    /// REGION-FREE identity, because Compute Optimizer is regional: the ARN a reader reports
    /// names the region of the endpoint the request addressed, which [`Self::arn_in`] builds
    /// from this id and the group's name.
    pub group_id: String,
    /// `autoScalingGroupName`.
    pub name: String,
    /// The group's current configuration.
    pub current: GroupConfiguration,
    /// The utilization the analysis observed, as (`MetricName`, value) pairs at the
    /// [`METRIC_STATISTIC_MAXIMUM`] statistic — the only statistic this API reports.
    pub utilization: Vec<(String, f64)>,
    /// `currentPerformanceRisk`: "the risk of the current Auto Scaling group not meeting
    /// the performance needs of its workloads", a `CurrentPerformanceRisk` value.
    pub current_performance_risk: String,
    /// The workload types the analysis inferred for the instances in the group
    /// (`InferredWorkloadType` values), empty where it inferred none.
    pub inferred_workload_types: Vec<String>,
    /// The candidate configurations the analysis found, best first — so the reported
    /// `rank` is a candidate's position in this list ("the top recommendation option is
    /// ranked as 1").
    pub candidates: Vec<GroupConfiguration>,
    /// The Savings Plans / Reserved Instances discount rate that applies to this
    /// group's compute spend, as a fraction of the on-demand price, or `0.0` where none
    /// does. It is what separates `savingsOpportunityAfterDiscounts` from the on-demand
    /// `savingsOpportunity`.
    pub savings_plans_discount_rate: f64,
}

impl AnalyzedAutoScalingGroup {
    /// This group's identity WITHOUT the region — the resource part of its ARN.
    pub fn arn_resource(&self) -> String {
        format!(
            "autoScalingGroup:{}:autoScalingGroupName/{}",
            self.group_id, self.name
        )
    }

    /// This group's ARN as the endpoint in `region` reports it — the form
    /// `AutoScalingGroupRecommendation$autoScalingGroupArn` carries ([`resource_arn_in`]).
    pub fn arn_in(&self, region: &str) -> String {
        resource_arn_in(
            AUTO_SCALING_ARN_SERVICE,
            region,
            &self.account_id,
            &self.arn_resource(),
        )
    }

    /// The group's `Finding`.
    ///
    /// "An Auto Scaling group is considered not optimized when Compute Optimizer
    /// identifies a recommendation that can provide better performance for your
    /// workload", and "optimized when Compute Optimizer determines that the group is
    /// correctly provisioned to run your workload based on the chosen instance type" —
    /// so the group is `NotOptimized` exactly when the analysis found a candidate that
    /// is not the group's own configuration, and `Optimized` when its own configuration
    /// is the best the analysis found.
    pub fn finding(&self) -> &'static str {
        if self
            .candidates
            .iter()
            .any(|candidate| candidate.differs_from(&self.current))
        {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// The observed utilization as the modelled `UtilizationMetrics`.
    pub fn utilization_metrics(&self) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .map(|(name, value)| UtilizationMetric {
                name: Some(name.clone()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: round_hundredths(*value),
            })
            .collect()
    }

    /// The utilization the observed workload is projected to reach on `candidate`.
    ///
    /// The same work runs on whatever capacity the configuration provides, so each
    /// percentage scales with the ratio of the capacity it was observed on to the
    /// capacity it is projected onto — CPU by the group's total vCPUs, memory by the
    /// group's total memory — through the one projection [`project_percentage`] states.
    pub fn projected_utilization_metrics(
        &self,
        candidate: &GroupConfiguration,
    ) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .map(|(name, observed)| {
                let (from, to) = if name == METRIC_MEMORY {
                    (self.current.total_memory_gib(), candidate.total_memory_gib())
                } else {
                    (self.current.total_vcpus(), candidate.total_vcpus())
                };
                UtilizationMetric {
                    name: Some(name.clone()),
                    statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                    value: project_percentage(*observed, from, to),
                }
            })
            .collect()
    }

    /// The `MigrationEffort` of moving this group onto `candidate`, by the rule the
    /// model states: "The migration effort is `VeryLow` if both the current and
    /// recommended instance types are of the same CPU architecture", `Low` "if Amazon
    /// EMR is the inferred workload type and an Amazon Web Services Graviton instance
    /// type is recommended", and `Medium` "if a workload type couldn't be inferred but
    /// an Amazon Web Services Graviton instance type is recommended".
    ///
    /// The model names Amazon EMR as the example of an inferred workload and states no
    /// rule for any other, so the emulator generalises its `Low` case to ANY workload type
    /// the analysis inferred: what the two documented Graviton cases differ on is whether
    /// a workload type was inferred at all, and naming one of the nine
    /// `InferredWorkloadType` values would make the effort of a Kafka or a Redis group
    /// depend on a value the rule says nothing about.
    ///
    /// `infers_workload_types` is whether the account leaves the workload-type inference
    /// active: with the inference deactivated no workload type is inferred for the
    /// group, which is the `Medium` case.
    ///
    /// The rule itself is the crate's ([`migration_effort`]), shared with the Amazon EC2
    /// instance analysis, so two recommendations of the same move cannot report different
    /// effort.
    pub fn migration_effort(
        &self,
        candidate: &GroupConfiguration,
        infers_workload_types: bool,
    ) -> &'static str {
        migration_effort(
            &candidate.architecture,
            infers_workload_types && !self.inferred_workload_types.is_empty(),
        )
    }

    /// The saving `candidate` offers against the group's own configuration, on the
    /// public on-demand prices stored with the analysis: the monthly cost it removes,
    /// and that saving as a percentage of the group's current monthly cost.
    ///
    /// `discount_rate` nets both sides of the comparison down by a Savings Plans /
    /// Reserved Instances discount, which is how `savingsOpportunityAfterDiscounts` is
    /// computed; pass `0.0` for the on-demand `savingsOpportunity`.
    pub fn savings(&self, candidate: &GroupConfiguration, discount_rate: f64) -> (f64, f64) {
        let retained = 1.0 - discount_rate;
        let current = round_hundredths(self.current.monthly_cost() * retained);
        let projected = round_hundredths(candidate.monthly_cost() * retained);
        let saving = round_hundredths((current - projected).max(0.0));
        (saving, percentage_of(saving, current))
    }

    /// This group as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it: the finding and the costs the analysis reached, with the RECOMMENDED
    /// cost being the top-ranked candidate's — "the top recommendation option is ranked as
    /// 1" — or the group's own cost where the analysis found no other candidate.
    ///
    /// The projection is what keeps one group's analysis in ONE place: the summary reader
    /// and `GetAutoScalingGroupRecommendations` both answer from this row's source, so
    /// they cannot disagree about a group's finding, its performance risk or what adopting
    /// its recommendation saves. An Auto Scaling group recommendation reports no finding
    /// reason codes (`AutoScalingGroupRecommendation` declares no such member), so the
    /// row carries none.
    pub fn to_analysed_resource(&self) -> AnalysedResource {
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_AUTO_SCALING_GROUP.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for a group is the id and name its ARN
            // names it by.
            arn_service: AUTO_SCALING_ARN_SERVICE.to_string(),
            arn_resource: self.arn_resource(),
            finding: self.finding().to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: self.current_performance_risk.clone(),
            current_monthly_cost: self.current.monthly_cost(),
            recommended_monthly_cost: self
                .candidates
                .first()
                .unwrap_or(&self.current)
                .monthly_cost(),
            inferred_workload_types: self.inferred_workload_types.clone(),
            // A group is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
}

/// The Auto Scaling group analysis this emulator holds, built once with the store.
///
/// A row keeps the group's REGION-FREE identity (its id and its name), because Compute
/// Optimizer is regional: every reader builds the ARN for the region the request addressed
/// ([`AnalyzedAutoScalingGroup::arn_in`]), so the same groups are reported at every regional
/// endpoint, each under that endpoint's own ARN.
///
/// The emulator observes no real workload, so WHICH groups are analysed is a fixed set
/// rather than something derived from traffic; every figure a recommendation reports is
/// nevertheless computed from the observations stored here (cost, saving, percentage,
/// projected utilization, finding, rank and migration effort all follow from these rows),
/// and a reader of a row never writes to it.
///
/// The rows span the shapes a client must handle: a single-instance-type group that is
/// over-provisioned (two candidates, one of them Graviton), a group that is correctly
/// provisioned (its own configuration is the only candidate, so no saving), and a
/// mixed-instance-type group with a Savings Plans discount against its spend.
pub(super) fn seed_auto_scaling_group_analyses() -> Vec<AnalyzedAutoScalingGroup> {
    let account_id = ::engine::emulator_account_id().to_string();

    vec![
        AnalyzedAutoScalingGroup {
            account_id: account_id.clone(),
            group_id: "6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e51".to_string(),
            name: "web-tier-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec!["m5.2xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 4,
                min_size: 2,
                max_size: 8,
                vcpus: 8.0,
                memory_gib: 32.0,
                hourly_price: 0.384,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 0.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 12.4),
                (METRIC_MEMORY.to_string(), 28.6),
            ],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["Nginx".to_string()],
            candidates: vec![
                GroupConfiguration {
                    instance_types: vec!["m5.xlarge".to_string()],
                    allocation_strategy: None,
                    desired_capacity: 4,
                    min_size: 2,
                    max_size: 8,
                    vcpus: 4.0,
                    memory_gib: 16.0,
                    hourly_price: 0.192,
                    architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                    performance_risk: 1.0,
                },
                GroupConfiguration {
                    instance_types: vec!["m6g.xlarge".to_string()],
                    allocation_strategy: None,
                    desired_capacity: 4,
                    min_size: 2,
                    max_size: 8,
                    vcpus: 4.0,
                    memory_gib: 16.0,
                    hourly_price: 0.154,
                    architecture: CPU_ARCHITECTURE_AWS_ARM64.to_string(),
                    performance_risk: 1.0,
                },
            ],
            savings_plans_discount_rate: 0.0,
        },
        AnalyzedAutoScalingGroup {
            account_id: account_id.clone(),
            group_id: "6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e52".to_string(),
            name: "batch-worker-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec!["c5.4xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 6,
                min_size: 6,
                max_size: 12,
                vcpus: 16.0,
                memory_gib: 32.0,
                hourly_price: 0.68,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 1.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 86.5),
                (METRIC_MEMORY.to_string(), 71.2),
            ],
            current_performance_risk: PERFORMANCE_RISK_LOW.to_string(),
            inferred_workload_types: Vec::new(),
            // The group's own configuration is the only candidate the analysis found,
            // which is what makes it `Optimized`: there is no saving to report.
            candidates: vec![GroupConfiguration {
                instance_types: vec!["c5.4xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 6,
                min_size: 6,
                max_size: 12,
                vcpus: 16.0,
                memory_gib: 32.0,
                hourly_price: 0.68,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 1.0,
            }],
            savings_plans_discount_rate: 0.0,
        },
        AnalyzedAutoScalingGroup {
            account_id,
            group_id: "6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e53".to_string(),
            name: "mixed-fleet-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec![
                    "m5.large".to_string(),
                    "m5a.large".to_string(),
                    "m6i.large".to_string(),
                ],
                allocation_strategy: Some("LowestPrice".to_string()),
                desired_capacity: 10,
                min_size: 4,
                max_size: 20,
                vcpus: 2.0,
                memory_gib: 8.0,
                hourly_price: 0.096,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 0.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 9.1),
                (METRIC_MEMORY.to_string(), 22.3),
            ],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["AmazonEmr".to_string()],
            candidates: vec![
                GroupConfiguration {
                    instance_types: vec!["m6g.large".to_string(), "m6gd.large".to_string()],
                    allocation_strategy: Some("LowestPrice".to_string()),
                    desired_capacity: 6,
                    min_size: 4,
                    max_size: 20,
                    vcpus: 2.0,
                    memory_gib: 8.0,
                    hourly_price: 0.077,
                    architecture: CPU_ARCHITECTURE_AWS_ARM64.to_string(),
                    performance_risk: 1.0,
                },
                GroupConfiguration {
                    instance_types: vec![
                        "m5.large".to_string(),
                        "m5a.large".to_string(),
                        "m6i.large".to_string(),
                    ],
                    allocation_strategy: Some("LowestPrice".to_string()),
                    desired_capacity: 6,
                    min_size: 4,
                    max_size: 20,
                    vcpus: 2.0,
                    memory_gib: 8.0,
                    hourly_price: 0.096,
                    architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                    performance_risk: 1.0,
                },
            ],
            savings_plans_discount_rate: 0.28,
        },
    ]
}
