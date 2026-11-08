// SPDX-License-Identifier: CC0-1.0

//! The AWS Lambda function versions Compute Optimizer observes, and the memory-size
//! recommendation a reader derives from that observation.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

/// The function version Lambda gives the unpublished version of a function, and the one
/// Compute Optimizer analyses for an unqualified ARN: "if you specify an unqualified ARN
/// without a function version suffix, Compute Optimizer will return recommendations for
/// the latest ($LATEST) version of the function"
/// (`GetLambdaFunctionRecommendations$functionArns`).
pub const LATEST_FUNCTION_VERSION: &str = "$LATEST";

/// The `LambdaFunctionRecommendationFilterName` values
/// ([`LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES`]), named so the filter rules below
/// read as the model's own vocabulary.
pub const LAMBDA_FILTER_FINDING: &str = "Finding";
/// See [`LAMBDA_FILTER_FINDING`].
pub const LAMBDA_FILTER_FINDING_REASON_CODE: &str = "FindingReasonCode";

/// The `LambdaFunctionRecommendationFinding` value outside the service-wide `Finding`
/// vocabulary: "Compute Optimizer was unable to generate a recommendation for the
/// function". Its other two values are [`FINDING_OPTIMIZED`] and [`FINDING_NOT_OPTIMIZED`].
pub const LAMBDA_FINDING_UNAVAILABLE: &str = "Unavailable";

/// The `LambdaFunctionRecommendationFindingReasonCode` values of the `Unavailable` finding:
/// `InsufficientData` — "the function does not have sufficient metric data for Compute
/// Optimizer to generate a recommendation" — and `Inconclusive` — "the function does not
/// qualify for a recommendation because Compute Optimizer cannot generate a recommendation
/// with a high degree of confidence". The two codes of the `NotOptimized` finding are
/// [`METRIC_REASON_MEMORY_OVERPROVISIONED`] and [`METRIC_REASON_MEMORY_UNDERPROVISIONED`].
pub const LAMBDA_REASON_INSUFFICIENT_DATA: &str = "InsufficientData";
/// See [`LAMBDA_REASON_INSUFFICIENT_DATA`].
pub const LAMBDA_REASON_INCONCLUSIVE: &str = "Inconclusive";

/// The `LambdaFunctionMetricName` values
/// (`crate::ops::types::LAMBDA_FUNCTION_METRIC_NAME_VALUES`): the duration a function's
/// code spends processing an event, and the memory it uses doing so.
pub const LAMBDA_METRIC_DURATION: &str = "Duration";
/// See [`LAMBDA_METRIC_DURATION`].
pub const LAMBDA_METRIC_MEMORY: &str = "Memory";

/// The statistic value beside [`METRIC_STATISTIC_MAXIMUM`] that this API's metric-statistic
/// vocabularies declare (`crate::ops::types::METRIC_STATISTIC_VALUES` and
/// `crate::ops::types::LAMBDA_FUNCTION_METRIC_STATISTIC_VALUES` are both `Average` |
/// `Maximum`): the other statistic an analysis records an observation under, and so the other
/// one a reader of projected metrics can ask for.
pub const METRIC_STATISTIC_AVERAGE: &str = "Average";

/// The `LambdaFunctionMemoryMetricStatistic` values
/// (`crate::ops::types::LAMBDA_FUNCTION_MEMORY_METRIC_STATISTIC_VALUES`): the range a
/// recommendation option projects the function's duration into, and the value expected
/// inside it.
pub const PROJECTED_STATISTIC_LOWER_BOUND: &str = "LowerBound";
/// See [`PROJECTED_STATISTIC_LOWER_BOUND`].
pub const PROJECTED_STATISTIC_UPPER_BOUND: &str = "UpperBound";
/// See [`PROJECTED_STATISTIC_LOWER_BOUND`].
pub const PROJECTED_STATISTIC_EXPECTED: &str = "Expected";

/// The memory a Lambda function can be configured with, from the AWS Lambda quotas: a
/// function's memory is "128 MB to 10,240 MB, in 1-MB increments", so no memory size this
/// service recommends falls outside that range or carries a fraction of a MB.
pub const LAMBDA_MIN_MEMORY_MB: i32 = 128;
/// See [`LAMBDA_MIN_MEMORY_MB`].
pub const LAMBDA_MAX_MEMORY_MB: i32 = 10_240;

/// How many invocations the analysed period has to hold for Compute Optimizer to reach a
/// finding for a function, from the "Supported resources and requirements" page the
/// `InsufficientData` reason code links: a Lambda function qualifies for a recommendation
/// once it has been invoked at least 50 times over the look-back period. A function below
/// that is reported `Unavailable` / `InsufficientData` rather than sized from the little
/// that was observed.
pub const LAMBDA_MIN_ANALYSED_INVOCATIONS: i64 = 50;

/// AWS Lambda public pricing in the `us-east-1` Region for the x86_64 architecture, as the
/// Lambda pricing page lists it: $0.0000166667 per GB-second of billed duration plus $0.20
/// per 1 million requests. Every cost and saving a function recommendation reports is
/// computed from these two prices, so its figures agree with each other.
pub const LAMBDA_PRICE_PER_GB_SECOND: f64 = 0.000_016_666_7;
/// See [`LAMBDA_PRICE_PER_GB_SECOND`].
pub const LAMBDA_PRICE_PER_REQUEST: f64 = 0.000_000_2;

/// MB in a GB: a function's memory is configured in MB and Lambda bills GB-seconds.
pub const MB_PER_GB: f64 = 1_024.0;

/// Milliseconds in a second: Lambda bills duration in GB-SECONDS and the `Duration` metric
/// is milliseconds.
pub const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

/// Days in a month, so a rate observed per day turns into the monthly figures Compute
/// Optimizer reports. Derived from [`HOURS_PER_MONTH`] rather than stated again, so one
/// month means the same length everywhere in this service.
pub fn days_per_month() -> f64 {
    HOURS_PER_MONTH / 24.0
}

/// A Lambda function ARN — `arn:<partition>:lambda:<region>:<account>:function:<name>` with
/// an optional `:<version>` suffix — split into the parts the reader needs, or `None` for
/// anything that is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedFunctionArn<'a> {
    /// The ARN's account-id field: the account that owns the function.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the function is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The function's name.
    pub function_name: &'a str,
    /// The version the ARN names, or [`LATEST_FUNCTION_VERSION`] for an UNQUALIFIED ARN:
    /// "if you specify an unqualified ARN without a function version suffix, Compute
    /// Optimizer will return recommendations for the latest ($LATEST) version of the
    /// function. If you specify a qualified ARN with a version suffix, Compute Optimizer
    /// will return recommendations for the specified function version."
    pub function_version: &'a str,
}

/// Parse a Lambda function ARN, or `None` for anything that is not one. See
/// [`ParsedFunctionArn`].
pub fn parse_function_arn(arn: &str) -> Option<ParsedFunctionArn<'_>> {
    let parsed = parse_arn(arn)?;
    if parsed.service != "lambda" {
        return None;
    }
    let mut resource = parsed.resource.splitn(3, ':');
    if resource.next() != Some("function") {
        return None;
    }
    let function_name = resource.next().filter(|name| !name.is_empty())?;
    let function_version = resource
        .next()
        .filter(|version| !version.is_empty())
        .unwrap_or(LATEST_FUNCTION_VERSION);
    Some(ParsedFunctionArn {
        account_id: parsed.account_id,
        region: parsed.region,
        function_name,
        function_version,
    })
}

/// One interval of the period Compute Optimizer analyses a function over, as CloudWatch
/// aggregates it: how often the function ran in that interval, how long an invocation took
/// (shortest, mean and longest) and how much memory it used (mean and peak).
///
/// A sample whose memory utilization was not observed records `0.0` for both memory
/// measurements — the analysis then has no memory demand to size the function from, which
/// is what [`ObservedFunction::finding`] reports `Inconclusive` for.
#[derive(Debug, Clone, Copy, Default)]
pub struct FunctionSample {
    /// How many days before the recommendation's last refresh this interval was observed,
    /// so the look-back period in force decides whether it is analysed at all.
    pub days_ago: f64,
    /// How many times the function was invoked in the interval, which is what
    /// `LambdaFunctionRecommendation$numberOfInvocations` totals.
    pub invocations: i64,
    /// The shortest invocation of the interval, in milliseconds.
    pub duration_ms_minimum: f64,
    /// The mean invocation of the interval, in milliseconds.
    pub duration_ms_average: f64,
    /// The longest invocation of the interval, in milliseconds.
    pub duration_ms_maximum: f64,
    /// The mean memory an invocation of the interval used, in MB.
    pub memory_used_mb_average: f64,
    /// The most memory an invocation of the interval used, in MB.
    pub memory_used_mb_maximum: f64,
}

/// One Lambda function version Compute Optimizer observes: how it is configured, its tags,
/// and the metric samples a recommendation for it is computed from. This is the
/// OBSERVATION; everything a `LambdaFunctionRecommendation` reports is derived from it, so
/// no two readers of the same function can disagree.
#[derive(Debug, Clone)]
pub struct ObservedFunction {
    /// The function's name, which with [`Self::function_version`] is its key in the
    /// inventory. The ARN a reader reports is built from them and the region the request was
    /// signed for ([`Self::function_arn`]), because Compute Optimizer is a REGIONAL service:
    /// the functions it reports are the ones in the Region the caller addressed.
    pub function_name: String,
    /// "The version number of the current function"
    /// (`LambdaFunctionRecommendation$functionVersion`) — a published version number, or
    /// [`LATEST_FUNCTION_VERSION`] for the unpublished one an unqualified ARN names.
    pub function_version: String,
    /// The account that owns the function.
    pub account_id: String,
    /// "The amount of memory, in MB, that's allocated to the current function"
    /// (`LambdaFunctionRecommendation$currentMemorySize`).
    pub configured_memory_mb: i32,
    /// How much of an invocation's duration scales with the memory the function is
    /// configured with, as a fraction of it: "Lambda allocates CPU power in proportion to
    /// the amount of memory configured", so the compute-bound part of an invocation gets
    /// faster on more memory and slower on less, while the part spent waiting on another
    /// service does not move at all. An OBSERVED property of the workload, which is what
    /// lets [`Self::projected_duration_ms`] state the duration a different memory size
    /// would run at.
    pub cpu_bound_duration_share: f64,
    /// The function's tags, as key/value pairs, in the order Lambda reports them.
    pub tags: Vec<(String, String)>,
    /// How many days Compute Optimizer has been observing this function. A look-back
    /// preference longer than this cannot be honoured, so the analysed period is the
    /// shorter of the two.
    pub days_observed: f64,
    /// The metric samples observed, most recent first.
    pub samples: Vec<FunctionSample>,
}

impl ObservedFunction {
    /// This function's identity WITHOUT the region — the resource part of its ARN, which is
    /// also its key in the inventory. A published version is a QUALIFIED ARN
    /// (`function:<name>:<version>`); the unpublished `$LATEST` version is the unqualified
    /// one (`function:<name>`), the form a request may name it by.
    pub fn arn_resource(&self) -> String {
        if self.function_version == LATEST_FUNCTION_VERSION {
            format!("function:{}", self.function_name)
        } else {
            format!("function:{}:{}", self.function_name, self.function_version)
        }
    }

    /// This function's ARN in `region`:
    /// `arn:aws:lambda:<region>:<account>:function:<name>[:<version>]`.
    pub fn function_arn(&self, region: &str) -> String {
        if self.account_id == ::engine::emulator_account_id() {
            ::engine::emulator_arn("lambda", region, &self.arn_resource())
        } else {
            format!(
                "arn:aws:lambda:{region}:{}:{}",
                self.account_id,
                self.arn_resource()
            )
        }
    }

    /// The samples the look-back period in force analyses: those observed within
    /// `look_back_days` of the last refresh.
    pub(super) fn analysed_samples(&self, look_back_days: f64) -> impl Iterator<Item = &FunctionSample> {
        self.samples
            .iter()
            .filter(move |sample| sample.days_ago <= look_back_days)
    }

    /// "The number of days for which utilization metrics were analyzed for the function":
    /// the preferred look-back period ([`look_back_period_in_days`]), or however long this
    /// function has been observed when that is shorter.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// "The number of times your function code was applied during the look-back period"
    /// (`LambdaFunctionRecommendation$numberOfInvocations`).
    pub fn invocations(&self, look_back_days: f64) -> i64 {
        self.analysed_samples(look_back_days)
            .map(|sample| sample.invocations.max(0))
            .sum()
    }

    /// The highest of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    pub(super) fn peak(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        self.analysed_samples(look_back_days)
            .map(of)
            .fold(0.0_f64, f64::max)
    }

    /// The lowest of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    pub(super) fn trough(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        let lowest = self
            .analysed_samples(look_back_days)
            .map(of)
            .fold(f64::INFINITY, f64::min);
        if lowest.is_finite() {
            lowest.max(0.0)
        } else {
            0.0
        }
    }

    /// The mean of one sample measurement over the analysed period, each sample weighted by
    /// the invocations it covers — the mean OVER INVOCATIONS, which is what a per-invocation
    /// average means. `0.0` for a period that holds no invocation.
    pub(super) fn mean_per_invocation(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        let mut invocations = 0.0_f64;
        let mut total = 0.0_f64;
        for sample in self.analysed_samples(look_back_days) {
            let weight = sample.invocations.max(0) as f64;
            invocations += weight;
            total += weight * of(sample);
        }
        if invocations > 0.0 {
            total / invocations
        } else {
            0.0
        }
    }

    /// The memory an invocation used at its peak over the analysed period, in MB — the
    /// demand the function's configured memory has to carry.
    pub fn peak_memory_mb(&self, look_back_days: f64) -> f64 {
        self.peak(look_back_days, |sample| sample.memory_used_mb_maximum)
    }

    /// The mean duration of an invocation over the analysed period, in milliseconds.
    pub fn average_duration_ms(&self, look_back_days: f64) -> f64 {
        self.mean_per_invocation(look_back_days, |sample| sample.duration_ms_average)
    }

    /// The metrics `LambdaFunctionRecommendation$utilizationMetrics` reports: each
    /// `LambdaFunctionMetricName` at each `LambdaFunctionMetricStatistic`, in the order the
    /// model's own `ExportableLambdaFunctionField` names them (`DurationAverage`,
    /// `DurationMaximum`, `MemoryAverage`, `MemoryMaximum`). Every value is computed from
    /// the analysed samples, so the metrics a recommendation carries are the observation the
    /// rest of it was derived from.
    pub fn utilization_metrics(&self, look_back_days: f64) -> Vec<LambdaFunctionUtilizationMetric> {
        let metric = |name: &str, statistic: &str, value: f64| LambdaFunctionUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(statistic.to_string()),
            value: round_hundredths(value),
        };
        vec![
            metric(
                LAMBDA_METRIC_DURATION,
                METRIC_STATISTIC_AVERAGE,
                self.average_duration_ms(look_back_days),
            ),
            metric(
                LAMBDA_METRIC_DURATION,
                METRIC_STATISTIC_MAXIMUM,
                self.peak(look_back_days, |sample| sample.duration_ms_maximum),
            ),
            metric(
                LAMBDA_METRIC_MEMORY,
                METRIC_STATISTIC_AVERAGE,
                self.mean_per_invocation(look_back_days, |sample| sample.memory_used_mb_average),
            ),
            metric(
                LAMBDA_METRIC_MEMORY,
                METRIC_STATISTIC_MAXIMUM,
                self.peak_memory_mb(look_back_days),
            ),
        ]
    }

    /// The memory size this service recommends for the function, in MB: enough to carry the
    /// peak an invocation used over the analysed period with [`RECOMMENDATION_HEADROOM`] to
    /// spare, inside the range Lambda allows ([`LAMBDA_MIN_MEMORY_MB`]) and in whole MB.
    pub fn recommended_memory_mb(&self, look_back_days: f64) -> i32 {
        let required = self.peak_memory_mb(look_back_days) * (1.0 + RECOMMENDATION_HEADROOM);
        (required.ceil() as i32).clamp(LAMBDA_MIN_MEMORY_MB, LAMBDA_MAX_MEMORY_MB)
    }

    /// The function's `LambdaFunctionRecommendationFinding` over the analysed period.
    ///
    /// `Unavailable` while the analysis has nothing to size the function from — fewer than
    /// [`LAMBDA_MIN_ANALYSED_INVOCATIONS`] invocations, or no memory utilization observed at
    /// all. Otherwise `NotOptimized` exactly when the memory the workload asks for
    /// ([`Self::recommended_memory_mb`]) is not the memory the function is configured with,
    /// because then "its memory configuration can be sized down while still meeting the
    /// performance requirements" or "doesn't meet the performance requirements of the
    /// workload"; `Optimized` when it is — "the function is correctly provisioned to run
    /// your workload based on its current configuration and its utilization history".
    pub fn finding(&self, look_back_days: f64) -> &'static str {
        if self.invocations(look_back_days) < LAMBDA_MIN_ANALYSED_INVOCATIONS
            || self.peak_memory_mb(look_back_days) <= 0.0
        {
            return LAMBDA_FINDING_UNAVAILABLE;
        }
        if self.recommended_memory_mb(look_back_days) == self.configured_memory_mb {
            FINDING_OPTIMIZED
        } else {
            FINDING_NOT_OPTIMIZED
        }
    }

    /// "The reason for the finding classification of the function": the code of the finding
    /// [`Self::finding`] reached, and NOTHING for an optimized function — "functions that
    /// have a finding classification of `Optimized` don't have a finding reason code".
    pub fn finding_reason_codes(&self, look_back_days: f64) -> Vec<String> {
        if self.invocations(look_back_days) < LAMBDA_MIN_ANALYSED_INVOCATIONS {
            return vec![LAMBDA_REASON_INSUFFICIENT_DATA.to_string()];
        }
        if self.peak_memory_mb(look_back_days) <= 0.0 {
            return vec![LAMBDA_REASON_INCONCLUSIVE.to_string()];
        }
        match self
            .recommended_memory_mb(look_back_days)
            .cmp(&self.configured_memory_mb)
        {
            ::std::cmp::Ordering::Less => vec![METRIC_REASON_MEMORY_OVERPROVISIONED.to_string()],
            ::std::cmp::Ordering::Greater => vec![METRIC_REASON_MEMORY_UNDERPROVISIONED.to_string()],
            ::std::cmp::Ordering::Equal => Vec::new(),
        }
    }

    /// The `CurrentPerformanceRisk` the function's configured memory runs: how close the
    /// peak an invocation used came to the memory it is allowed, on the service's one risk
    /// scale ([`performance_risk_score`]).
    pub fn current_performance_risk(&self, look_back_days: f64) -> &'static str {
        performance_risk_band(performance_risk_score(
            self.peak_memory_mb(look_back_days),
            f64::from(self.configured_memory_mb.max(0)),
        ))
    }

    /// The duration an invocation that took `observed_ms` at the configured memory would
    /// take at `memory_mb`.
    ///
    /// Lambda "allocates CPU power in proportion to the amount of memory configured", so
    /// the compute-bound share of the invocation ([`Self::cpu_bound_duration_share`]) scales
    /// with the ratio of the two memory sizes and the rest of it does not move: a function
    /// that spends its time waiting on another service gets no faster on more memory.
    pub fn projected_duration_ms(&self, observed_ms: f64, memory_mb: i32) -> f64 {
        let share = self.cpu_bound_duration_share.clamp(0.0, 1.0);
        let ratio = demand_ratio(
            f64::from(self.configured_memory_mb.max(0)),
            f64::from(memory_mb.max(0)),
        );
        if !ratio.is_finite() {
            return observed_ms.max(0.0);
        }
        round_hundredths(observed_ms.max(0.0) * (share * ratio + (1.0 - share)))
    }

    /// The projected metrics a recommendation option carries: the duration range the
    /// function would run in at `memory_mb`, projected from the shortest, the mean and the
    /// longest invocation the analysed period observed. `LambdaFunctionMemoryMetricName`
    /// declares one metric — `Duration` — so each statistic is reported for it.
    pub fn projected_utilization_metrics(
        &self,
        look_back_days: f64,
        memory_mb: i32,
    ) -> Vec<LambdaFunctionMemoryProjectedMetric> {
        let metric = |statistic: &str, observed_ms: f64| LambdaFunctionMemoryProjectedMetric {
            name: Some(LAMBDA_METRIC_DURATION.to_string()),
            statistic: Some(statistic.to_string()),
            value: self.projected_duration_ms(observed_ms, memory_mb),
        };
        vec![
            metric(
                PROJECTED_STATISTIC_LOWER_BOUND,
                self.trough(look_back_days, |sample| sample.duration_ms_minimum),
            ),
            metric(
                PROJECTED_STATISTIC_EXPECTED,
                self.average_duration_ms(look_back_days),
            ),
            metric(
                PROJECTED_STATISTIC_UPPER_BOUND,
                self.peak(look_back_days, |sample| sample.duration_ms_maximum),
            ),
        ]
    }

    /// How many times the function runs in a month at the rate the analysed period observed.
    pub fn monthly_invocations(&self, look_back_days: f64) -> f64 {
        if look_back_days <= 0.0 {
            return 0.0;
        }
        self.invocations(look_back_days) as f64 / look_back_days * days_per_month()
    }

    /// What a month of this function costs in USD at `memory_mb` with a mean invocation of
    /// `duration_ms`: the GB-seconds that many invocations of that length at that memory
    /// bill, plus the per-request charge, both at [`LAMBDA_PRICE_PER_GB_SECOND`].
    pub fn monthly_cost(&self, memory_mb: i32, duration_ms: f64, look_back_days: f64) -> f64 {
        let invocations = self.monthly_invocations(look_back_days);
        let gb_seconds = invocations
            * (duration_ms.max(0.0) / MILLISECONDS_PER_SECOND)
            * (f64::from(memory_mb.max(0)) / MB_PER_GB);
        round_hundredths(
            gb_seconds * LAMBDA_PRICE_PER_GB_SECOND + invocations * LAMBDA_PRICE_PER_REQUEST,
        )
    }

    /// What the function costs per month as it is configured today.
    pub fn current_monthly_cost(&self, look_back_days: f64) -> f64 {
        self.monthly_cost(
            self.configured_memory_mb,
            self.average_duration_ms(look_back_days),
            look_back_days,
        )
    }

    /// What the function would cost per month at the recommended memory size: the same
    /// invocations at the projected duration that memory size runs them in, so a saving is
    /// never claimed from a memory cut the duration pays back.
    pub fn recommended_monthly_cost(&self, look_back_days: f64) -> f64 {
        let memory_mb = self.recommended_memory_mb(look_back_days);
        self.monthly_cost(
            memory_mb,
            self.projected_duration_ms(self.average_duration_ms(look_back_days), memory_mb),
            look_back_days,
        )
    }

    /// What adopting the recommended memory size saves per month, and that saving "as a
    /// percentage of monthly cost" (`SavingsOpportunity$savingsOpportunityPercentage`). A
    /// recommendation that costs the same or more saves NOTHING rather than a negative
    /// amount — an under-provisioned function is recommended MORE memory.
    pub fn savings(&self, look_back_days: f64) -> (f64, f64) {
        let current = self.current_monthly_cost(look_back_days);
        let saved = round_hundredths((current - self.recommended_monthly_cost(look_back_days)).max(0.0));
        (percentage_of(saved, current), saved)
    }

    /// The function recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every figure is computed here from the stored configuration and samples. A memory
    /// size recommendation option is reported exactly when the analysis reached a finding
    /// for the function: an `Unavailable` function has no memory size to move to, because
    /// "Compute Optimizer was unable to generate a recommendation" for it.
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> LambdaFunctionRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let finding = self.finding(look_back_days);
        let reason_codes = self.finding_reason_codes(look_back_days);
        let memory_mb = self.recommended_memory_mb(look_back_days);
        let (percentage, value) = self.savings(look_back_days);

        LambdaFunctionRecommendation {
            function_arn: Some(self.function_arn(region)),
            function_version: Some(self.function_version.clone()),
            account_id: Some(self.account_id.clone()),
            current_memory_size: self.configured_memory_mb,
            number_of_invocations: self.invocations(look_back_days),
            utilization_metrics: Some(self.utilization_metrics(look_back_days)),
            lookback_period_in_days: look_back_days,
            last_refresh_timestamp,
            finding: Some(finding.to_string()),
            finding_reason_codes: (!reason_codes.is_empty()).then_some(reason_codes),
            memory_size_recommendation_options: (finding != LAMBDA_FINDING_UNAVAILABLE).then(|| {
                vec![LambdaFunctionMemoryRecommendationOption {
                    // One option, so it is the first-ranked one: "rank 1 is considered the
                    // best" wherever this service ranks a recommendation option.
                    rank: 1,
                    memory_size: memory_mb,
                    projected_utilization_metrics: Some(
                        self.projected_utilization_metrics(look_back_days, memory_mb),
                    ),
                    savings_opportunity: Some(SavingsOpportunity {
                        savings_opportunity_percentage: percentage,
                        estimated_monthly_savings: Some(EstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value,
                        }),
                    }),
                    // This emulator prices a function from the public price table in
                    // [`LAMBDA_PRICE_PER_GB_SECOND`] and records no negotiated discount for
                    // Lambda, so the after-discount figures are the before-discount ones —
                    // which is what the reported [`SAVINGS_ESTIMATION_SOURCE`] says.
                    savings_opportunity_after_discounts: Some(
                        LambdaSavingsOpportunityAfterDiscounts {
                            savings_opportunity_percentage: percentage,
                            estimated_monthly_savings: Some(LambdaEstimatedMonthlySavings {
                                currency: Some(CURRENCY_USD.to_string()),
                                value,
                            }),
                        },
                    ),
                }]
            }),
            current_performance_risk: Some(self.current_performance_risk(look_back_days).to_string()),
            // `LambdaEffectiveRecommendationPreferences` carries one preference: where the
            // money figures came from ([`SAVINGS_ESTIMATION_SOURCE`]).
            effective_recommendation_preferences: Some(LambdaEffectiveRecommendationPreferences {
                savings_estimation_mode: Some(LambdaSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
            }),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect()
            }),
        }
    }

    /// This function as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it — or `None` for a function no summary can count.
    ///
    /// The projection is what keeps one function's analysis in ONE place: the summary reader
    /// and `GetLambdaFunctionRecommendations` both answer from this observation, so they
    /// cannot disagree about a function's finding, its performance risk or what adopting its
    /// recommendation saves. A function whose finding is `Unavailable` is NOT a row: the
    /// findings a summary of Lambda functions reports are the number of them "that are
    /// NotOptimized, or Optimized" and `Summary$name` cannot even carry `Unavailable`, so
    /// such a function is counted at no finding — matching the per-function reader, which
    /// withholds it unless a request asks for it by name.
    pub fn to_analysed_resource(&self, look_back_period: &str) -> Option<AnalysedResource> {
        let look_back_days = self.analysed_days(look_back_period);
        let finding = self.finding(look_back_days);
        if !findings_reported_for(SOURCE_TYPE_LAMBDA_FUNCTION)
            .iter()
            .any(|(stored, _)| *stored == finding)
        {
            return None;
        }
        Some(AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_LAMBDA_FUNCTION.to_string(),
            arn_service: "lambda".to_string(),
            arn_resource: self.arn_resource(),
            finding: finding.to_string(),
            finding_reason_codes: self.finding_reason_codes(look_back_days),
            current_performance_risk: self.current_performance_risk(look_back_days).to_string(),
            current_monthly_cost: self.current_monthly_cost(look_back_days),
            recommended_monthly_cost: self.recommended_monthly_cost(look_back_days),
            // Compute Optimizer infers workload types for EC2 instances, not for functions,
            // and a function recommendation declares no such member.
            inferred_workload_types: Vec::new(),
            // A function is not a database, so there is no database configuration to record.
            rds_database: None,
        })
    }
}

/// Check a `LambdaFunctionRecommendationFilters` list against the vocabulary the model
/// declares for it, returning the reason it is not valid.
///
/// `name` is a `LambdaFunctionRecommendationFilterName` and the values a filter carries are
/// the value set of the member that name selects on: "specify `Optimized`, `NotOptimized`,
/// or `Unavailable` if you specify the `name` parameter as `Finding`"; "specify
/// `MemoryOverprovisioned`, `MemoryUnderprovisioned`, `InsufficientData`, or `Inconclusive`
/// if you specify the `name` parameter as `FindingReasonCode`". A name or a value outside
/// those sets is refused rather than silently matching nothing — the same rule
/// [`rds_filter_rejection`] applies to the other findings readers' filters.
pub fn lambda_filter_rejection(filters: &[LambdaFunctionRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = lambda_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set a `LambdaFunctionRecommendationFilterName` selects on, or `None` for a name
/// the model does not declare.
pub(super) fn lambda_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        LAMBDA_FILTER_FINDING => Some(LAMBDA_FUNCTION_RECOMMENDATION_FINDING_VALUES),
        LAMBDA_FILTER_FINDING_REASON_CODE => {
            Some(LAMBDA_FUNCTION_RECOMMENDATION_FINDING_REASON_CODE_VALUES)
        }
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetLambdaFunctionRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for the reason codes, which are a LIST, when any reported code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`lambda_filter_rejection`] refuses the request first.
pub fn lambda_recommendation_matches_filters(
    recommendation: &LambdaFunctionRecommendation,
    filters: &[LambdaFunctionRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        match filter.name.as_deref() {
            Some(LAMBDA_FILTER_FINDING) => recommendation
                .finding
                .as_deref()
                .is_some_and(|finding| values.iter().any(|value| value == finding)),
            Some(LAMBDA_FILTER_FINDING_REASON_CODE) => {
                recommendation
                    .finding_reason_codes
                    .as_ref()
                    .is_some_and(|codes| {
                        codes
                            .iter()
                            .any(|code| values.iter().any(|value| value == code))
                    })
            }
            _ => false,
        }
    })
}

/// Whether a request's filters ask for the functions the analysis reached no finding for.
///
/// "Functions with a finding of unavailable are not returned unless you specify the filter
/// parameter with a value of `Unavailable` in your `GetLambdaFunctionRecommendations`
/// request" (`LambdaFunctionRecommendation$finding`), so the reader withholds them from
/// every other request rather than reporting a recommendation that does not exist.
pub fn lambda_filters_ask_for_unavailable(filters: &[LambdaFunctionRecommendationFilter]) -> bool {
    filters.iter().any(|filter| {
        filter.name.as_deref() == Some(LAMBDA_FILTER_FINDING)
            && filter
                .values
                .iter()
                .flatten()
                .any(|value| value == LAMBDA_FINDING_UNAVAILABLE)
    })
}

/// The Lambda functions this emulator observes, generated once when the store is created.
///
/// The emulator watches no real workload, so the functions and their metric samples are a
/// fixed observation rather than something derived from traffic. Everything a recommendation
/// reports is nevertheless computed from what is stored here — the analysed invocations,
/// durations and memory peaks come from these samples, the recommended memory size from
/// those peaks, and every cost from [`LAMBDA_PRICE_PER_GB_SECOND`] — so a reader cannot
/// report a figure the observation does not support.
///
/// The set spans the cases a function recommendation distinguishes: an over-provisioned
/// function whose heaviest day falls OUTSIDE the default look-back period, an
/// under-provisioned PUBLISHED version (a qualified ARN), one already sized for its
/// workload, one invoked too rarely to analyse, and one whose memory utilization was never
/// observed.
pub(super) fn observed_functions() -> Vec<ObservedFunction> {
    let account_id = ::engine::emulator_account_id().to_string();

    vec![
        // 1 GB configured for a workload that peaks at 420 MB on its analysed days: the
        // recommendation sizes the memory down, and half of each invocation is spent
        // waiting on another service, so the smaller memory does not give the saving back
        // in duration. Its heaviest day (780 MB) is 40 days old, so a longer look-back
        // preference changes the memory size this function is recommended.
        ObservedFunction {
            function_name: "invoice-renderer".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 1_024,
            cpu_bound_duration_share: 0.5,
            tags: vec![("Owner".to_string(), "TeamA".to_string())],
            days_observed: 93.0,
            samples: vec![
                FunctionSample {
                    days_ago: 0.0,
                    invocations: 42_000,
                    duration_ms_minimum: 180.0,
                    duration_ms_average: 310.0,
                    duration_ms_maximum: 900.0,
                    memory_used_mb_average: 350.0,
                    memory_used_mb_maximum: 400.0,
                },
                FunctionSample {
                    days_ago: 6.0,
                    invocations: 58_000,
                    duration_ms_minimum: 200.0,
                    duration_ms_average: 328.0,
                    duration_ms_maximum: 1_100.0,
                    memory_used_mb_average: 362.0,
                    memory_used_mb_maximum: 420.0,
                },
                FunctionSample {
                    days_ago: 40.0,
                    invocations: 30_000,
                    duration_ms_minimum: 240.0,
                    duration_ms_average: 520.0,
                    duration_ms_maximum: 1_800.0,
                    memory_used_mb_average: 610.0,
                    memory_used_mb_maximum: 780.0,
                },
            ],
        },
        // A PUBLISHED version, so a qualified ARN names it: 128 MB for a workload that
        // peaks at 120 MB, which leaves no headroom — the recommendation sizes the memory
        // UP, and an under-provisioned function saves nothing by taking it.
        ObservedFunction {
            function_name: "thumbnail-resizer".to_string(),
            function_version: "3".to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 128,
            cpu_bound_duration_share: 0.8,
            tags: vec![
                ("Owner".to_string(), "TeamB".to_string()),
                ("Environment".to_string(), "production".to_string()),
            ],
            days_observed: 93.0,
            samples: vec![FunctionSample {
                days_ago: 1.0,
                invocations: 8_400,
                duration_ms_minimum: 420.0,
                duration_ms_average: 850.0,
                duration_ms_maximum: 2_400.0,
                memory_used_mb_average: 104.0,
                memory_used_mb_maximum: 120.0,
            }],
        },
        // 128 MB — Lambda's smallest — for a workload whose peak plus headroom still fits
        // inside it, so there is no other memory size to move to: optimized, and observed
        // for only a week, so the period analysed is that week however long a look-back
        // period the account prefers.
        ObservedFunction {
            function_name: "order-dispatcher".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 128,
            cpu_bound_duration_share: 0.6,
            tags: Vec::new(),
            days_observed: 7.0,
            samples: vec![FunctionSample {
                days_ago: 0.0,
                invocations: 15_000,
                duration_ms_minimum: 40.0,
                duration_ms_average: 95.0,
                duration_ms_maximum: 260.0,
                memory_used_mb_average: 88.0,
                memory_used_mb_maximum: 100.0,
            }],
        },
        // Invoked a handful of times in the analysed period — below what the service needs
        // to reach a finding — so the analysis has insufficient data for it.
        ObservedFunction {
            function_name: "log-forwarder".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id: account_id.clone(),
            configured_memory_mb: 512,
            cpu_bound_duration_share: 0.4,
            tags: Vec::new(),
            days_observed: 93.0,
            samples: vec![FunctionSample {
                days_ago: 2.0,
                invocations: 12,
                duration_ms_minimum: 60.0,
                duration_ms_average: 140.0,
                duration_ms_maximum: 300.0,
                memory_used_mb_average: 180.0,
                memory_used_mb_maximum: 240.0,
            }],
        },
        // Invoked often enough, but no memory utilization was ever observed for it, so the
        // analysis has no demand to size its memory from and cannot conclude anything.
        ObservedFunction {
            function_name: "nightly-reconciler".to_string(),
            function_version: LATEST_FUNCTION_VERSION.to_string(),
            account_id,
            configured_memory_mb: 256,
            cpu_bound_duration_share: 0.3,
            tags: vec![("Owner".to_string(), "TeamA".to_string())],
            days_observed: 93.0,
            samples: vec![FunctionSample {
                days_ago: 1.0,
                invocations: 2_000,
                duration_ms_minimum: 500.0,
                duration_ms_average: 1_200.0,
                duration_ms_maximum: 4_000.0,
                memory_used_mb_average: 0.0,
                memory_used_mb_maximum: 0.0,
            }],
        },
    ]
}
