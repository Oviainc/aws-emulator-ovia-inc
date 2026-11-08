// SPDX-License-Identifier: CC0-1.0

//! The Amazon EBS volumes Compute Optimizer observes, and the recommendation every reader
//! of one derives from that observation.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

/// The Amazon EBS volume inventory this emulator observes, generated once when the store
/// is created.
///
/// The emulator watches no real workload, so the volumes and their metric samples are a
/// fixed observation rather than something derived from traffic. Everything a
/// recommendation reports is nevertheless computed from what is stored here — the
/// analysed peaks come from these samples, the recommended configuration from those
/// peaks, and every cost from [`volume_monthly_cost`] — so a reader cannot report a
/// figure the observation does not support.
///
/// The set spans the cases a volume recommendation distinguishes: a previous-generation
/// volume whose demand fits its capacity (cheaper on gp3), a gp3 volume whose demand
/// outruns its provisioned IOPS (needs more performance), a heavily over-provisioned
/// Provisioned IOPS volume whose peak demand arrived outside the default look-back
/// period, and a gp3 volume already provisioned for its workload.
pub(super) fn observed_volumes() -> Vec<ObservedVolume> {
    let account_id = ::engine::emulator_account_id().to_string();

    vec![
        // gp2, 500 GiB: 3 IOPS per GiB of baseline (1,500) with a 3,000 IOPS burst and
        // 250 MiB/s of throughput. Its peak fits that comfortably, so the gp3 option
        // carries the same workload for less money.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60001".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some("gp2".to_string()),
                volume_size: 500,
                volume_baseline_iops: 1_500,
                volume_burst_iops: 3_000,
                volume_baseline_throughput: 250,
                volume_burst_throughput: 250,
                root_volume: Some(true),
            },
            tags: vec![("Owner".to_string(), "TeamA".to_string())],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 0.0,
                    read_ops_per_second: 300.0,
                    write_ops_per_second: 400.0,
                    read_bytes_per_second: 20.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 25.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 3.0,
                    read_ops_per_second: 450.0,
                    write_ops_per_second: 450.0,
                    read_bytes_per_second: 30.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 30.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 40.0,
                    read_ops_per_second: 200.0,
                    write_ops_per_second: 150.0,
                    read_bytes_per_second: 10.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 8.0 * BYTES_PER_MIB,
                },
            ],
        },
        // gp3, 100 GiB at its included performance, driven past it: the option has to
        // provision more IOPS and more throughput, which costs MORE than the volume
        // costs today — a recommendation is not always a saving.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60002".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some(VOLUME_TYPE_GP3.to_string()),
                volume_size: 100,
                volume_baseline_iops: GP3_INCLUDED_IOPS,
                volume_burst_iops: 0,
                volume_baseline_throughput: GP3_INCLUDED_THROUGHPUT_MIB,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: vec![("Owner".to_string(), "TeamB".to_string())],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 0.0,
                    read_ops_per_second: 2_400.0,
                    write_ops_per_second: 2_400.0,
                    read_bytes_per_second: 60.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 60.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 10.0,
                    read_ops_per_second: 1_000.0,
                    write_ops_per_second: 900.0,
                    read_bytes_per_second: 40.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 30.0 * BYTES_PER_MIB,
                },
            ],
        },
        // io1, 200 GiB with 8,000 provisioned IOPS: far more than the workload asks for,
        // and its heaviest day falls OUTSIDE the default look-back period, so a longer
        // look-back preference changes the option this volume gets.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60003".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some("io1".to_string()),
                volume_size: 200,
                volume_baseline_iops: 8_000,
                volume_burst_iops: 0,
                volume_baseline_throughput: 500,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: vec![
                ("Owner".to_string(), "TeamB".to_string()),
                ("Environment".to_string(), "production".to_string()),
            ],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 1.0,
                    read_ops_per_second: 600.0,
                    write_ops_per_second: 600.0,
                    read_bytes_per_second: 30.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 20.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 50.0,
                    read_ops_per_second: 1_500.0,
                    write_ops_per_second: 1_500.0,
                    read_bytes_per_second: 100.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 60.0 * BYTES_PER_MIB,
                },
            ],
        },
        // gp3, 50 GiB, already provisioned for its workload — and observed for only a
        // week, so the period analysed is that week however long a look-back period the
        // account prefers.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60004".to_string(),
            account_id,
            configuration: VolumeConfiguration {
                volume_type: Some(VOLUME_TYPE_GP3.to_string()),
                volume_size: 50,
                volume_baseline_iops: GP3_INCLUDED_IOPS,
                volume_burst_iops: 0,
                volume_baseline_throughput: GP3_INCLUDED_THROUGHPUT_MIB,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: Vec::new(),
            days_observed: 7.0,
            samples: vec![VolumeSample {
                days_ago: 0.0,
                read_ops_per_second: 700.0,
                write_ops_per_second: 800.0,
                read_bytes_per_second: 20.0 * BYTES_PER_MIB,
                write_bytes_per_second: 20.0 * BYTES_PER_MIB,
            }],
        },
    ]
}

/// One Amazon EBS volume Compute Optimizer observes: its configuration, its tags, and
/// the metric samples a recommendation for it is computed from. This is the OBSERVATION;
/// everything a `VolumeRecommendation` reports is derived from it, so no two readers of
/// the same volume can disagree.
#[derive(Debug, Clone)]
pub struct ObservedVolume {
    /// The volume's EC2 volume id, which is its key in the inventory. The ARN a reader
    /// reports is built from it and the region the request was signed for
    /// ([`ObservedVolume::volume_arn`]), because Compute Optimizer is a REGIONAL service:
    /// the volumes it reports are the ones in the Region the caller addressed.
    pub volume_id: String,
    /// The account that owns the volume.
    pub account_id: String,
    /// The volume's configuration as EC2 reports it: type, size, and the baseline and
    /// burst IOPS and throughput that type and size deliver.
    pub configuration: VolumeConfiguration,
    /// The volume's tags, as key/value pairs, in the order EC2 reports them.
    pub tags: Vec<(String, String)>,
    /// How many days Compute Optimizer has been observing this volume. A look-back
    /// preference longer than this cannot be honoured, so the analysed period is the
    /// shorter of the two.
    pub days_observed: f64,
    /// The metric samples observed, most recent first.
    pub samples: Vec<VolumeSample>,
}

impl ObservedVolume {
    /// This volume's ARN in `region`: `arn:aws:ec2:<region>:<account>:volume/<id>`.
    pub fn volume_arn(&self, region: &str) -> String {
        ::engine::emulator_arn("ec2", region, &format!("volume/{}", self.volume_id))
    }

    /// The samples the look-back period in force analyses: those taken within
    /// `look_back_days` of the last refresh.
    pub(super) fn analysed_samples(&self, look_back_days: f64) -> impl Iterator<Item = &VolumeSample> {
        self.samples
            .iter()
            .filter(move |sample| sample.days_ago <= look_back_days)
    }

    /// The peak of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    pub(super) fn peak(&self, look_back_days: f64, of: fn(&VolumeSample) -> f64) -> f64 {
        self.analysed_samples(look_back_days)
            .map(of)
            .fold(0.0_f64, f64::max)
    }

    /// How many analysed samples `exceeded` answers true for.
    pub(super) fn samples_exceeding(
        &self,
        look_back_days: f64,
        exceeded: impl Fn(&VolumeSample) -> bool,
    ) -> f64 {
        self.analysed_samples(look_back_days)
            .filter(|sample| exceeded(sample))
            .count() as f64
    }

    /// "The number of days for which utilization metrics were analyzed for the volume"
    /// (`VolumeRecommendation$lookBackPeriodInDays`): the preferred look-back period
    /// ([`look_back_period_in_days`]), or however long this volume has been observed when
    /// that is shorter.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// The metrics `VolumeRecommendation$utilizationMetrics` reports: every
    /// `EBSMetricName` the model declares, each the peak of the analysed samples — and,
    /// for the two "exceeded" metrics, the number of analysed samples whose demand went
    /// past the capacity the volume's configuration delivers.
    pub fn utilization_metrics(&self, look_back_days: f64) -> Vec<EBSUtilizationMetric> {
        let iops_capacity = configured_iops_capacity(&self.configuration);
        let throughput_capacity = configured_throughput_capacity(&self.configuration);
        let metric = |name: &str, value: f64| EBSUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
            value,
        };
        vec![
            metric(
                EBS_METRIC_READ_OPS_PER_SECOND,
                self.peak(look_back_days, |sample| sample.read_ops_per_second),
            ),
            metric(
                EBS_METRIC_WRITE_OPS_PER_SECOND,
                self.peak(look_back_days, |sample| sample.write_ops_per_second),
            ),
            metric(
                EBS_METRIC_READ_BYTES_PER_SECOND,
                self.peak(look_back_days, |sample| sample.read_bytes_per_second),
            ),
            metric(
                EBS_METRIC_WRITE_BYTES_PER_SECOND,
                self.peak(look_back_days, |sample| sample.write_bytes_per_second),
            ),
            metric(
                EBS_METRIC_IOPS_EXCEEDED,
                self.samples_exceeding(look_back_days, |sample| sample.iops() > iops_capacity),
            ),
            metric(
                EBS_METRIC_THROUGHPUT_EXCEEDED,
                self.samples_exceeding(look_back_days, |sample| {
                    sample.throughput_mib_per_second() > throughput_capacity
                }),
            ),
        ]
    }

    /// The configuration this service recommends for the volume: gp3 at the same size,
    /// with IOPS and throughput provisioned to carry the analysed peak plus
    /// [`RECOMMENDATION_HEADROOM`], inside gp3's documented limits. A volume whose peak
    /// already fits gp3's included performance gets gp3's included performance.
    pub fn recommended_configuration(&self, look_back_days: f64) -> VolumeConfiguration {
        let size = self.configuration.volume_size.max(0);
        let headroom = 1.0 + RECOMMENDATION_HEADROOM;

        let peak_iops = self.peak(look_back_days, VolumeSample::iops);
        let iops_ceiling = GP3_MAX_IOPS
            .min(size.saturating_mul(GP3_MAX_IOPS_PER_GIB))
            .max(GP3_INCLUDED_IOPS);
        let iops = ((peak_iops * headroom).ceil() as i32).clamp(GP3_INCLUDED_IOPS, iops_ceiling);

        let peak_throughput = self.peak(look_back_days, VolumeSample::throughput_mib_per_second);
        let throughput_ceiling = GP3_MAX_THROUGHPUT_MIB
            .min((f64::from(iops) * GP3_MAX_THROUGHPUT_MIB_PER_IOPS) as i32)
            .max(GP3_INCLUDED_THROUGHPUT_MIB);
        let throughput = ((peak_throughput * headroom).ceil() as i32)
            .clamp(GP3_INCLUDED_THROUGHPUT_MIB, throughput_ceiling);

        VolumeConfiguration {
            volume_type: Some(VOLUME_TYPE_GP3.to_string()),
            volume_size: size,
            volume_baseline_iops: iops,
            // gp3 performance is provisioned rather than bursted — a gp3 volume uses no
            // burst credits — so the option delivers its baseline and nothing above it.
            volume_burst_iops: 0,
            volume_baseline_throughput: throughput,
            volume_burst_throughput: 0,
            // A recommendation changes a volume's type and provisioned performance; the
            // volume stays attached where it was, so whether it is a root volume is a
            // property of the volume, not of the option.
            root_volume: self.configuration.root_volume,
        }
    }

    /// How close the analysed peak came to what `configuration` sustains, as a
    /// `PerformanceRisk` on the model's 0..4 scale: 0 while the peak stays under a
    /// quarter of the sustained capacity, 4 once it reaches 125% of it. "The higher the
    /// risk, the more likely the current EBS volume doesn't have sufficient capacity"
    /// (`VolumeRecommendation$currentPerformanceRisk`), so the score rises with that
    /// ratio and is measured against the BASELINE: burst capacity is not sustained.
    pub fn performance_risk(
        &self,
        configuration: &VolumeConfiguration,
        look_back_days: f64,
    ) -> f64 {
        let baseline_iops = f64::from(configuration.volume_baseline_iops.max(0));
        let baseline_throughput = f64::from(configuration.volume_baseline_throughput.max(0));
        let iops_ratio = demand_ratio(self.peak(look_back_days, VolumeSample::iops), baseline_iops);
        let throughput_ratio = demand_ratio(
            self.peak(look_back_days, VolumeSample::throughput_mib_per_second),
            baseline_throughput,
        );
        performance_risk_score_of_ratio(iops_ratio.max(throughput_ratio))
    }

    /// The volume's tags as the modelled `Tags` list, or `None` when it carries none.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        modelled_tags(&self.tags)
    }

    /// The volume recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every figure is computed here from the stored configuration and samples: the
    /// metrics are the analysed peaks, the option is the configuration those peaks ask
    /// for, each cost comes from [`volume_monthly_cost`], and the finding is
    /// `NotOptimized` exactly when the option would either carry a peak the current
    /// configuration does not sustain or cost less than it — "a volume is considered not
    /// optimized when Compute Optimizer identifies a recommendation that can provide
    /// better performance for your workload", and `Optimized` otherwise, where "Compute
    /// Optimizer might [still] recommend a new generation volume type"
    /// (`VolumeRecommendation$finding`).
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> VolumeRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let current = self.configuration.clone();
        let recommended = self.recommended_configuration(look_back_days);
        let current_risk = self.performance_risk(&current, look_back_days);
        let option_risk = self.performance_risk(&recommended, look_back_days);
        let savings = self.savings(look_back_days);

        VolumeRecommendation {
            volume_arn: Some(self.volume_arn(region)),
            account_id: Some(self.account_id.clone()),
            current_configuration: Some(current),
            finding: Some(self.finding(look_back_days).to_string()),
            utilization_metrics: Some(self.utilization_metrics(look_back_days)),
            look_back_period_in_days: look_back_days,
            volume_recommendation_options: Some(vec![VolumeRecommendationOption {
                configuration: Some(recommended),
                performance_risk: option_risk,
                // One option, so it is the first-ranked one. "Rank 1 is considered the
                // best" (`VolumeRecommendationOption$rank`).
                rank: 1,
                savings_opportunity: savings.map(|(percentage, value)| SavingsOpportunity {
                    savings_opportunity_percentage: percentage,
                    estimated_monthly_savings: Some(EstimatedMonthlySavings {
                        currency: Some(CURRENCY_USD.to_string()),
                        value,
                    }),
                }),
                // This emulator prices a volume from the public price table in
                // [`volume_monthly_cost`] and records no negotiated discount for storage,
                // so the after-discount figures are the before-discount ones — which is
                // what the reported [`SAVINGS_ESTIMATION_SOURCE`] says.
                savings_opportunity_after_discounts: savings.map(|(percentage, value)| {
                    EBSSavingsOpportunityAfterDiscounts {
                        savings_opportunity_percentage: percentage,
                        estimated_monthly_savings: Some(EBSEstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value,
                        }),
                    }
                }),
            }]),
            last_refresh_timestamp,
            current_performance_risk: Some(performance_risk_band(current_risk).to_string()),
            effective_recommendation_preferences: Some(EBSEffectiveRecommendationPreferences {
                savings_estimation_mode: Some(EBSSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
                look_back_period: Some(look_back_period.to_string()),
            }),
            tags: self.tag_list(),
        }
    }

    /// The volume's `EBSFinding` over the analysed period: `NotOptimized` exactly when the
    /// recommended configuration would either carry a peak the current one does not
    /// sustain or cost less than it, `Optimized` otherwise. `EBSFinding`'s two values are
    /// [`FINDING_NOT_OPTIMIZED`] and [`FINDING_OPTIMIZED`].
    pub fn finding(&self, look_back_days: f64) -> &'static str {
        let recommended = self.recommended_configuration(look_back_days);
        let safer = self.performance_risk(&recommended, look_back_days)
            < self.performance_risk(&self.configuration, look_back_days);
        let cheaper = match (
            volume_monthly_cost(&self.configuration),
            volume_monthly_cost(&recommended),
        ) {
            (Some(current), Some(option)) => option < current,
            _ => false,
        };
        if safer || cheaper {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// What adopting the recommended configuration saves per month and that saving as a
    /// percentage of what the volume costs today, or `None` for a volume type this
    /// emulator holds no price for ([`volume_monthly_cost`]) — whose recommendation then
    /// carries no savings figure rather than an invented one.
    pub fn savings(&self, look_back_days: f64) -> Option<(f64, f64)> {
        let current = volume_monthly_cost(&self.configuration)?;
        let option = volume_monthly_cost(&self.recommended_configuration(look_back_days))?;
        let saved = round_hundredths((current - option).max(0.0));
        Some((percentage_of(saved, current), saved))
    }

    /// This volume as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it, under the look-back period in force.
    ///
    /// The projection is what keeps one volume's analysis in ONE place: the summary reader
    /// and `GetEBSVolumeRecommendations` both answer from this observation, so they cannot
    /// disagree about a volume's finding, its performance risk or what adopting its
    /// recommendation saves. A volume recommendation reports no finding reason codes
    /// (`VolumeRecommendation` declares no such member) and infers no workload type, so
    /// the row carries neither.
    pub fn to_analysed_resource(&self, look_back_period: &str) -> AnalysedResource {
        let look_back_days = self.analysed_days(look_back_period);
        let current_monthly_cost = volume_monthly_cost(&self.configuration).unwrap_or_default();
        let recommended_monthly_cost =
            volume_monthly_cost(&self.recommended_configuration(look_back_days))
                .unwrap_or(current_monthly_cost);
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_EBS_VOLUME.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for a volume is its EC2 volume id.
            arn_service: "ec2".to_string(),
            arn_resource: format!("volume/{}", self.volume_id),
            finding: self.finding(look_back_days).to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: performance_risk_band(
                self.performance_risk(&self.configuration, look_back_days),
            )
            .to_string(),
            current_monthly_cost,
            recommended_monthly_cost,
            inferred_workload_types: Vec::new(),
            // A volume is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
}
