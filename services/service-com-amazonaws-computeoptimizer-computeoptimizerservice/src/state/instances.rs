// SPDX-License-Identifier: CC0-1.0

//! The Amazon EC2 instances Compute Optimizer observes: the instance types the analysis
//! compares, the utilization observed on an instance and what a reader derives from both.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

/// One Amazon EC2 instance TYPE, as the configuration an instance runs today or as one the
/// analysis found as a candidate for it. Both sides are the same shape because every figure
/// derived from them — the cost, the saving, the utilization projected onto a candidate — is
/// a comparison BETWEEN two of them, the same reason [`GroupConfiguration`] serves both
/// sides of an Auto Scaling group's analysis.
#[derive(Debug, Clone)]
pub struct InstanceConfiguration {
    /// The instance type, `<family>.<size>` — what
    /// `RecommendedOptionProjectedMetric$recommendedInstanceType` names.
    pub instance_type: String,
    /// The vCPUs this instance type provides.
    pub vcpus: f64,
    /// The memory, in GiB, this instance type provides.
    pub memory_gib: f64,
    /// The public on-demand price of one instance-hour, in [`CURRENCY_USD`].
    pub hourly_price: f64,
    /// This type's `CpuVendorArchitecture`: [`CPU_ARCHITECTURE_CURRENT`] or
    /// [`CPU_ARCHITECTURE_AWS_ARM64`].
    pub architecture: String,
}

/// Whether a `MetricName` measures, as a percentage, a resource the instance TYPE sizes — the
/// instance's vCPUs (`Cpu`) and its memory (`Memory`).
///
/// "`Cpu` — the projected percentage of allocated EC2 compute units … identifies the
/// processing power required to run an application" and "`Memory` — the percentage of memory
/// that would be in use … identifies the amount of memory required" (`ProjectedMetric$name`):
/// both are percentages of what the instance type provides, so moving to another type changes
/// what they are OF. Every other `MetricName` of that value set measures the workload's own
/// traffic — its network packets and bytes, its disk and EBS operations — which changing the
/// instance type does not resize.
///
/// They are also exactly the metrics a PROJECTION reports: "The `Cpu` and `Memory` metrics are
/// the only projected utilization metrics returned when you run the
/// `GetEC2RecommendationProjectedMetrics` action" (`RecommendedOptionProjectedMetric`).
pub fn metric_sized_by_instance_type(metric_name: &str) -> bool {
    metric_name == METRIC_CPU || metric_name == METRIC_MEMORY
}

impl InstanceConfiguration {
    /// The estimated monthly on-demand cost of running one instance of this type: the price
    /// of one instance-hour × [`HOURS_PER_MONTH`].
    pub fn monthly_cost(&self) -> f64 {
        round_hundredths(self.hourly_price * HOURS_PER_MONTH)
    }

    /// The capacity a metric named `metric_name` is measured as a percentage OF on this
    /// instance type, or `None` for a metric the instance type does not size
    /// ([`metric_sized_by_instance_type`]).
    pub fn metric_capacity(&self, metric_name: &str) -> Option<f64> {
        match metric_name {
            METRIC_CPU => Some(self.vcpus),
            METRIC_MEMORY => Some(self.memory_gib),
            _ => None,
        }
    }
}

/// One utilization figure the analysis observed on an instance: the `MetricName` measured,
/// the `MetricStatistic` it was observed under, and its value.
#[derive(Debug, Clone)]
pub struct ObservedInstanceMetric {
    /// The `MetricName` measured (`crate::ops::types::METRIC_NAME_VALUES`).
    pub name: String,
    /// The `MetricStatistic` the figure was observed under ([`METRIC_STATISTIC_MAXIMUM`] or
    /// [`METRIC_STATISTIC_AVERAGE`]), which is what a request's `stat` selects on.
    pub statistic: String,
    /// The figure itself: a percentage for a metric the instance type sizes
    /// ([`InstanceConfiguration::metric_capacity`]), and the metric's own unit otherwise.
    pub value: f64,
}

/// One Amazon EC2 instance Compute Optimizer has analysed: the instance type it runs, the
/// utilization observed on it, and the instance types the analysis found as candidates for
/// it. Every value a reader REPORTS about the instance is one of these stored observations
/// or a figure computed from them, so two readers of the same instance cannot disagree.
#[derive(Debug, Clone)]
pub struct ObservedInstance {
    /// The EC2 instance id, which is this inventory's key. The ARN a reader reports is built
    /// from it and the region the request was signed for ([`Self::instance_arn`]), because
    /// Compute Optimizer is a REGIONAL service.
    pub instance_id: String,
    /// The account that owns the instance.
    pub account_id: String,
    /// The instance type the instance runs today.
    pub current: InstanceConfiguration,
    /// The candidate instance types the analysis found, best first — so a reported `rank` is
    /// a candidate's position in this list ("the top recommendation option is ranked as 1").
    /// Empty for an instance the analysis would leave as it is.
    pub candidates: Vec<InstanceConfiguration>,
    /// The utilization the analysis observed over its look-back period, one figure per
    /// metric and statistic.
    pub utilization: Vec<ObservedInstanceMetric>,
    /// The `FindingReasonCode`s behind the instance's finding; empty where it has none.
    pub finding_reason_codes: Vec<String>,
    /// `currentPerformanceRisk`: the `CurrentPerformanceRisk` the instance's current
    /// configuration runs.
    pub current_performance_risk: String,
    /// The `InferredWorkloadType`s the analysis inferred for the instance; empty where it
    /// inferred none.
    pub inferred_workload_types: Vec<String>,
    /// `instanceState`: "the state of the instance when the recommendation was generated",
    /// one of the model's `InstanceState` values
    /// (`crate::ops::types::INSTANCE_STATE_VALUES`).
    pub instance_state: String,
    /// The instance's tags, as key/value pairs, in the order Amazon EC2 reports them. The
    /// `Name` tag is what names the instance ([`Self::instance_name`]).
    pub tags: Vec<(String, String)>,
}

/// The `InstanceState` an instance under analysis is in
/// (`crate::ops::types::INSTANCE_STATE_VALUES`): Compute Optimizer analyses the workload an
/// instance is RUNNING, so that is the state the observations below were taken in.
pub const INSTANCE_STATE_RUNNING: &str = "running";

/// The tag key Amazon EC2 names a resource with, and so the tag an instance's `instanceName`
/// is taken from: "the name of the current instance" (`InstanceRecommendation$instanceName`)
/// is the value of its `Name` tag.
pub const NAME_TAG_KEY: &str = "Name";

/// The `PlatformDifference` value a recommendation option of another CPU architecture carries
/// (`crate::ops::types::PLATFORM_DIFFERENCE_VALUES`).
pub const PLATFORM_DIFFERENCE_ARCHITECTURE: &str = "Architecture";

/// The `ExternalMetricStatusCode` for an account that prefers no external metrics provider
/// (`crate::ops::types::EXTERNAL_METRIC_STATUS_CODE_VALUES`).
pub const EXTERNAL_METRIC_STATUS_NO_METRIC_SET: &str = "NO_EXTERNAL_METRIC_SET";

/// The `ExternalMetricStatus` a recommendation reports: "an object that describes Compute
/// Optimizer's integration status with your external metrics provider".
///
/// WHICH provider the analysis should take metrics from is the account's external-metrics
/// preference (`ExternalMetricsPreference$source`, one of
/// `crate::ops::types::EXTERNAL_METRICS_SOURCE_VALUES`), so an account that prefers none has
/// no integration at all: [`EXTERNAL_METRIC_STATUS_NO_METRIC_SET`]. This emulator derives a
/// recommendation from the utilization it observes itself and ingests no series from a
/// third-party provider, so once a provider IS preferred the status is that provider's
/// "insufficient metrics" code — the condition the service reports while a preferred provider
/// has published nothing the analysis can use. A source outside the model's own value set
/// names no provider to integrate with, so it reports no integration either.
pub fn external_metric_status(source: Option<&str>) -> ExternalMetricStatus {
    /// Each `ExternalMetricsSource` the model declares
    /// (`crate::ops::types::EXTERNAL_METRICS_SOURCE_VALUES`) paired with the
    /// `ExternalMetricStatusCode` that provider's absent metrics are reported under
    /// (`crate::ops::types::EXTERNAL_METRIC_STATUS_CODE_VALUES`) — one row per provider the
    /// model names, so neither value set can drift out of step with the other unnoticed.
    const INSUFFICIENT_METRICS: [(&str, &str); 4] = [
        ("Datadog", "INSUFFICIENT_DATADOG_METRICS"),
        ("Dynatrace", "INSUFFICIENT_DYNATRACE_METRICS"),
        ("Instana", "INSUFFICIENT_INSTANA_METRICS"),
        ("NewRelic", "INSUFFICIENT_NEWRELIC_METRICS"),
    ];
    debug_assert!(
        INSUFFICIENT_METRICS
            .iter()
            .all(|(provider, code)| EXTERNAL_METRICS_SOURCE_VALUES.contains(provider)
                && EXTERNAL_METRIC_STATUS_CODE_VALUES.contains(code)),
        "the catalog may only pair modelled ExternalMetricsSource and \
         ExternalMetricStatusCode values"
    );
    let no_metric_set = || ExternalMetricStatus {
        status_code: Some(EXTERNAL_METRIC_STATUS_NO_METRIC_SET.to_string()),
        status_reason: None,
    };
    let Some(source) = source.filter(|source| !source.is_empty()) else {
        return no_metric_set();
    };
    let code = INSUFFICIENT_METRICS
        .iter()
        .find(|(provider, _)| *provider == source)
        .map(|(_, code)| *code);
    let Some(code) = code else {
        return no_metric_set();
    };
    ExternalMetricStatus {
        status_code: Some(code.to_string()),
        status_reason: Some(format!(
            "Compute Optimizer has received no utilization metrics from {source}."
        )),
    }
}

/// The `MigrationEffort` of moving a workload onto a candidate of `candidate_architecture`,
/// given whether the analysis inferred a workload type for it.
///
/// `VeryLow` "if both the current and recommended instance types are of the same CPU
/// architecture", `Low` "if Amazon EMR is the inferred workload type and an Amazon Web
/// Services Graviton instance type is recommended", and `Medium` "if a workload type couldn't
/// be inferred but an Amazon Web Services Graviton instance type is recommended"
/// (`InstanceRecommendationOption$migrationEffort`).
///
/// The model names Amazon EMR as the example of an inferred workload and states no rule for
/// any other, so this generalises its `Low` case to ANY workload type the analysis inferred:
/// what the two documented Graviton cases differ on is whether a workload type was inferred at
/// all, and naming one of the nine `InferredWorkloadType` values would make the effort of a
/// Kafka or a Redis workload depend on a value the rule says nothing about.
///
/// The ONE rule in this service, shared by every analysis that ranks a candidate instance
/// type, so two recommendations of the same move cannot report different effort.
pub fn migration_effort(candidate_architecture: &str, inferred_workload_type: bool) -> &'static str
{
    if candidate_architecture != CPU_ARCHITECTURE_AWS_ARM64 {
        "VeryLow"
    } else if inferred_workload_type {
        "Low"
    } else {
        "Medium"
    }
}

/// An Amazon EC2 instance ARN — `arn:<partition>:ec2:<region>:<account>:instance/<id>` — split
/// into the parts the readers need, or `None` for anything that is not one.
/// `GetEC2InstanceRecommendations$instanceArns` takes "the Amazon Resource Name (ARN) of the
/// instances for which to return recommendations", so anything else is not an instance ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedInstanceArn<'a> {
    /// The ARN's account-id field: the account that owns the instance.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the instance is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The EC2 instance id, which is the inventory's key.
    pub instance_id: &'a str,
}

/// Parse an Amazon EC2 instance ARN, or `None` for anything that is not one. Built on the one
/// place this service decides what an ARN looks like ([`parse_arn`]). See
/// [`ParsedInstanceArn`].
pub fn parse_instance_arn(arn: &str) -> Option<ParsedInstanceArn<'_>> {
    let parsed = parse_arn(arn)?;
    if parsed.service != "ec2" {
        return None;
    }
    let (resource_kind, instance_id) = parsed.resource.split_once('/')?;
    if resource_kind != "instance" || instance_id.is_empty() {
        return None;
    }
    Some(ParsedInstanceArn {
        account_id: parsed.account_id,
        region: parsed.region,
        instance_id,
    })
}

impl ObservedInstance {
    /// "The name of the current instance" — the value of its `Name` tag, or `None` for an
    /// instance that carries none, which Amazon EC2 reports no name for either.
    pub fn instance_name(&self) -> Option<String> {
        self.tags
            .iter()
            .find(|(key, _)| key == NAME_TAG_KEY)
            .map(|(_, value)| value.clone())
    }

    /// The instance's tags as the modelled `Tags` list, or `None` when it carries none.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        modelled_tags(&self.tags)
    }

    /// The metrics `InstanceRecommendation$utilizationMetrics` reports: every observation the
    /// analysis recorded under [`METRIC_STATISTIC_MAXIMUM`], which is the one statistic this
    /// API reports ("The Compute Optimizer API, Command Line Interface (CLI), and SDKs return
    /// utilization metrics using only the `Maximum` statistic", `UtilizationMetric$statistic`).
    pub fn utilization_metrics(&self) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .filter(|metric| metric.statistic == METRIC_STATISTIC_MAXIMUM)
            .map(|metric| UtilizationMetric {
                name: Some(metric.name.clone()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: round_hundredths(metric.value),
            })
            .collect()
    }

    /// The metrics `InstanceRecommendationOption$projectedUtilizationMetrics` reports for
    /// `option`: "The `Cpu` and `Memory` metrics are the only projected utilization metrics
    /// returned", so the observations projected are the ones the instance TYPE sizes
    /// ([`metric_sized_by_instance_type`]), each through the one projection this service
    /// projects an instance's utilization with ([`Self::projected_metric_value`]) — so this
    /// option and the series `GetEC2RecommendationProjectedMetrics` projects for it cannot
    /// disagree.
    pub fn projected_utilization_metrics(
        &self,
        option: &InstanceConfiguration,
    ) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .filter(|metric| metric.statistic == METRIC_STATISTIC_MAXIMUM)
            .filter(|metric| metric_sized_by_instance_type(&metric.name))
            .map(|metric| UtilizationMetric {
                name: Some(metric.name.clone()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: self.projected_metric_value(&metric.name, metric.value, option),
            })
            .collect()
    }

    /// The `PerformanceRisk` an option carries: how close the utilization projected onto
    /// `option` comes to the capacity of the type it recommends, on the model's 0..4 scale
    /// ([`performance_risk_score`]) — "the performance risk of the recommended instance is
    /// calculated as the maximum performance risk score across the analyzed resource
    /// specifications", so an option is as risky as its most loaded dimension.
    pub fn option_performance_risk(&self, option: &InstanceConfiguration) -> f64 {
        let peak = self
            .projected_utilization_metrics(option)
            .iter()
            .map(|metric| metric.value)
            .fold(0.0_f64, f64::max);
        round_hundredths(performance_risk_score(peak, UTILIZATION_PERCENTAGE_CEILING))
    }

    /// What adopting `option` saves per month, and that saving "as a percentage of monthly
    /// cost" (`SavingsOpportunity$savingsOpportunityPercentage`). An option that costs the
    /// same or more saves NOTHING rather than a negative amount — an under-provisioned
    /// instance is recommended a LARGER type.
    pub fn savings(&self, option: &InstanceConfiguration) -> (f64, f64) {
        let current = self.current.monthly_cost();
        let saved = round_hundredths((current - option.monthly_cost()).max(0.0));
        (saved, percentage_of(saved, current))
    }

    /// The `PlatformDifference`s between the type the instance runs and `option`
    /// (`crate::ops::types::PLATFORM_DIFFERENCE_VALUES`): "the configuration differences
    /// between the current instance and the recommended instance type".
    ///
    /// The analysis records an instance type's CPU vendor and architecture, so the one
    /// difference it can state is `Architecture` — "the CPU architecture of the recommended
    /// instance type is different than that of the current instance" — which a candidate of
    /// another `CpuVendorArchitecture` has. The remaining values of that enum describe a
    /// hypervisor, a network or storage interface, instance-store availability or a
    /// virtualization type, none of which this analysis observes.
    pub fn platform_differences(&self, option: &InstanceConfiguration) -> Vec<String> {
        if option.architecture == self.current.architecture {
            Vec::new()
        } else {
            vec![PLATFORM_DIFFERENCE_ARCHITECTURE.to_string()]
        }
    }

    /// "The level of effort required to migrate from the current instance type to the
    /// recommended instance type" — the crate's one rule for it ([`migration_effort`]), so an
    /// instance's effort and an Auto Scaling group's mean the same thing.
    pub fn migration_effort(
        &self,
        option: &InstanceConfiguration,
        infers_workload_types: bool,
    ) -> &'static str {
        migration_effort(
            &option.architecture,
            infers_workload_types && !self.inferred_workload_types.is_empty(),
        )
    }

    /// This instance's identity WITHOUT the region — the resource part of its ARN.
    pub fn arn_resource(&self) -> String {
        format!("instance/{}", self.instance_id)
    }

    /// This instance's ARN as the endpoint in `region` reports it:
    /// `arn:aws:ec2:<region>:<account>:instance/<id>`, the form
    /// `GetEC2RecommendationProjectedMetrics$instanceArn` carries.
    pub fn instance_arn(&self, region: &str) -> String {
        resource_arn_in("ec2", region, &self.account_id, &self.arn_resource())
    }

    /// The top-ranked candidate the analysis found, or `None` for an instance it would leave
    /// as it is — which is also the configuration a cost or a saving is computed against.
    pub fn recommended_configuration(&self) -> Option<&InstanceConfiguration> {
        self.candidates.first()
    }

    /// The instance's `Finding`.
    ///
    /// An instance is `Overprovisioned` "if at least one specification of your instance, such
    /// as CPU, memory, or network, can be sized down while still meeting the performance
    /// requirements of your workload", `Underprovisioned` "if at least one specification …
    /// does not meet the performance requirements of your workload", and `Optimized` "when
    /// all specifications … meet the performance requirements" — so the finding follows the
    /// top-ranked candidate: one that GROWS a specification is one the workload needed more
    /// of, one that only SHRINKS specifications is one the instance has more of than the
    /// workload needs, and an instance with no candidate at all, or one at the same capacity
    /// (a newer generation of the same size), is optimized.
    pub fn finding(&self) -> &'static str {
        let Some(option) = self.recommended_configuration() else {
            return FINDING_OPTIMIZED;
        };
        let grows = option.vcpus > self.current.vcpus || option.memory_gib > self.current.memory_gib;
        let shrinks =
            option.vcpus < self.current.vcpus || option.memory_gib < self.current.memory_gib;
        match (grows, shrinks) {
            (true, _) => FINDING_UNDERPROVISIONED,
            (false, true) => FINDING_OVERPROVISIONED,
            (false, false) => FINDING_OPTIMIZED,
        }
    }

    /// The value a metric is projected to take once the instance runs on `option`, given the
    /// `observed` value on the type it runs today.
    ///
    /// "This represents the projected utilization of a recommendation option had you used
    /// that resource during the analyzed period" (`ProjectedMetric`): a metric measured as a
    /// percentage of a resource the instance type sizes
    /// ([`InstanceConfiguration::metric_capacity`]) is projected by the one percentage
    /// projection [`project_percentage`] states. Every other metric measures the workload
    /// itself, which changing the instance type does not resize, so it is projected
    /// unchanged.
    ///
    /// The ONE projection rule for an instance, so the series this projects over a window and
    /// the projected utilization an instance recommendation option reports cannot disagree.
    pub fn projected_metric_value(
        &self,
        metric_name: &str,
        observed: f64,
        option: &InstanceConfiguration,
    ) -> f64 {
        let capacities = self
            .current
            .metric_capacity(metric_name)
            .zip(option.metric_capacity(metric_name));
        let Some((from, to)) = capacities else {
            return observed;
        };
        project_percentage(observed, from, to)
    }

    /// This instance as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it: the finding the analysis reached, what the instance costs today and
    /// what the top-ranked candidate would cost — "the top recommendation option is ranked as
    /// 1" — or its own cost where the analysis found no candidate.
    ///
    /// The projection is what keeps one instance's analysis in ONE place: the summary reader
    /// and every reader of an instance recommendation answer from this observation, so they
    /// cannot disagree about an instance's finding, its performance risk or what adopting its
    /// recommendation saves.
    pub fn to_analysed_resource(&self) -> AnalysedResource {
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_EC2_INSTANCE.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for an instance is its EC2 instance id.
            arn_service: "ec2".to_string(),
            arn_resource: self.arn_resource(),
            finding: self.finding().to_string(),
            finding_reason_codes: self.finding_reason_codes.clone(),
            current_performance_risk: self.current_performance_risk.clone(),
            current_monthly_cost: self.current.monthly_cost(),
            recommended_monthly_cost: self
                .recommended_configuration()
                .unwrap_or(&self.current)
                .monthly_cost(),
            inferred_workload_types: self.inferred_workload_types.clone(),
            // An instance is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
}

/// The Amazon EC2 instance inventory this emulator observes, generated once when the store
/// is created.
///
/// The emulator watches no real workload, so WHICH instances it holds and the utilization
/// observed on them are a fixed observation rather than something derived from traffic.
/// Everything a reader reports is nevertheless computed from what is stored here — each
/// cost from the stored on-demand price, each finding from the candidate the analysis found,
/// and every projected metric from the observed one — so a reader cannot report a figure the
/// observation does not support.
///
/// The set spans the cases an instance recommendation distinguishes: an instance the
/// analysis would size DOWN (so its recommendation saves, and one of its candidates is a
/// Graviton type, which a request may ask for on its own), one it would size UP (whose
/// recommendation therefore saves nothing) and one it would leave as it is (which has no
/// candidate to project onto). They differ in whether Amazon EC2 names them — an instance is
/// named by its `Name` tag, and one carries none.
///
/// The prices and capacities are the public on-demand Linux prices of these instance types
/// in the `us-east-1` Region, as the Amazon EC2 On-Demand pricing page lists them: `m5.large`
/// $0.096 per hour with 2 vCPUs and 8 GiB, `m5.xlarge` $0.192 with 4 vCPUs and 16 GiB,
/// `m5.2xlarge` $0.384 with 8 vCPUs and 32 GiB, and the Graviton `m6g.xlarge` $0.154 with
/// 4 vCPUs and 16 GiB.
pub(super) fn observed_instances() -> Vec<ObservedInstance> {
    let account_id = ::engine::emulator_account_id().to_string();
    let configuration = |instance_type: &str,
                         vcpus: f64,
                         memory_gib: f64,
                         hourly_price: f64,
                         architecture: &str| InstanceConfiguration {
        instance_type: instance_type.to_string(),
        vcpus,
        memory_gib,
        hourly_price,
        architecture: architecture.to_string(),
    };
    let m5_large = || configuration("m5.large", 2.0, 8.0, 0.096, CPU_ARCHITECTURE_CURRENT);
    let m5_xlarge = || configuration("m5.xlarge", 4.0, 16.0, 0.192, CPU_ARCHITECTURE_CURRENT);
    let m5_2xlarge = || configuration("m5.2xlarge", 8.0, 32.0, 0.384, CPU_ARCHITECTURE_CURRENT);
    let m6g_xlarge = || configuration("m6g.xlarge", 4.0, 16.0, 0.154, CPU_ARCHITECTURE_AWS_ARM64);
    let metric = |name: &str, statistic: &str, value: f64| ObservedInstanceMetric {
        name: name.to_string(),
        statistic: statistic.to_string(),
        value,
    };

    vec![
        // Half the workload's memory and processing power is idle: the analysis would move
        // the instance to half the capacity, which costs half as much. It found a Graviton
        // candidate of the same capacity too, which a request asking for `AWS_ARM64` gets on
        // its own.
        ObservedInstance {
            instance_id: "i-0a1b2c3d4e5f60001".to_string(),
            account_id: account_id.clone(),
            current: m5_2xlarge(),
            candidates: vec![m5_xlarge(), m6g_xlarge()],
            utilization: vec![
                metric(METRIC_CPU, METRIC_STATISTIC_MAXIMUM, 22.5),
                metric(METRIC_MEMORY, METRIC_STATISTIC_MAXIMUM, 31.0),
                metric(METRIC_CPU, METRIC_STATISTIC_AVERAGE, 12.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_AVERAGE, 20.0),
                // A metric the instance type does not size: the workload's own network
                // traffic, in bytes per second.
                metric(
                    "NETWORK_IN_BYTES_PER_SECOND",
                    METRIC_STATISTIC_MAXIMUM,
                    1_250_000.0,
                ),
            ],
            finding_reason_codes: vec![METRIC_REASON_MEMORY_OVERPROVISIONED.to_string()],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["PostgreSql".to_string()],
            instance_state: INSTANCE_STATE_RUNNING.to_string(),
            tags: vec![
                (NAME_TAG_KEY.to_string(), "reporting-db".to_string()),
                ("Owner".to_string(), "TeamA".to_string()),
            ],
        },
        // The workload outgrew the instance: the analysis would move it to twice the
        // capacity, which costs MORE — a recommendation is not always a saving.
        ObservedInstance {
            instance_id: "i-0a1b2c3d4e5f60002".to_string(),
            account_id: account_id.clone(),
            current: m5_large(),
            candidates: vec![m5_xlarge()],
            utilization: vec![
                metric(METRIC_CPU, METRIC_STATISTIC_MAXIMUM, 88.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_MAXIMUM, 97.0),
                metric(METRIC_CPU, METRIC_STATISTIC_AVERAGE, 64.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_AVERAGE, 90.0),
            ],
            finding_reason_codes: vec![METRIC_REASON_MEMORY_UNDERPROVISIONED.to_string()],
            current_performance_risk: PERFORMANCE_RISK_HIGH.to_string(),
            inferred_workload_types: vec!["Redis".to_string()],
            instance_state: INSTANCE_STATE_RUNNING.to_string(),
            tags: vec![(NAME_TAG_KEY.to_string(), "session-cache".to_string())],
        },
        // Already the right size for its workload: no candidate, so nothing to project onto.
        ObservedInstance {
            instance_id: "i-0a1b2c3d4e5f60003".to_string(),
            account_id,
            current: m5_xlarge(),
            candidates: Vec::new(),
            utilization: vec![
                metric(METRIC_CPU, METRIC_STATISTIC_MAXIMUM, 55.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_MAXIMUM, 60.0),
                metric(METRIC_CPU, METRIC_STATISTIC_AVERAGE, 41.0),
                metric(METRIC_MEMORY, METRIC_STATISTIC_AVERAGE, 48.0),
            ],
            finding_reason_codes: Vec::new(),
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: Vec::new(),
            instance_state: INSTANCE_STATE_RUNNING.to_string(),
            // An instance nobody tagged: it carries no tags, and so no name either.
            tags: Vec::new(),
        },
    ]
}
