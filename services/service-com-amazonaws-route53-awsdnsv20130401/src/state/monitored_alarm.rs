// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch alarm a health check is monitoring, as a reported health check
//! carries it.
//!
//! `HealthCheck$CloudWatchAlarmConfiguration` is "information about the CloudWatch
//! alarm that Amazon Route 53 is monitoring for this health check", and the check
//! that monitors one is the alarm-reading type
//! ([`super::HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC`]): its whole input is the
//! `AlarmIdentifier` naming that alarm, which is what the evaluator in
//! [`super::health_check_status`] reads the alarm's STATE through. This module is
//! the other half of that read — the alarm's own CONFIGURATION, which is what the
//! reported health check carries: the metric the alarm watches, the statistic and
//! period it watches it over, and the comparison its evaluation makes.
//!
//! The alarm belongs to CloudWatch, so its configuration is read out of the
//! CloudWatch emulator's OWN alarm store through the shared engine — the same
//! store `PutMetricAlarm` writes and `DescribeAlarms` reads, and the same one the
//! evaluator reads an alarm's state from. Nothing is copied into this service's
//! store, so one alarm has one home and a health check cannot report settings the
//! alarm no longer has.
//!
//! An alarm this service holds no reading of carries no configuration to report,
//! which is the absent member: a check of another type, a check whose alarm the
//! account never created (or has deleted), a CloudWatch that is not mounted beside
//! this service, and an alarm whose evaluation this API cannot describe —
//! `CloudWatchAlarmConfiguration` requires the metric, statistic, period,
//! evaluation periods, threshold and comparison operator of a single-metric alarm,
//! and its `Statistic` and `ComparisonOperator` carry fewer values than
//! CloudWatch's own, so an alarm over a metric-math expression, over an extended
//! statistic, or comparing by an operator this model does not carry has no such
//! configuration to report.

use std::collections::HashMap;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use super::HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC;
use crate::ops::types::shared_types::{
    CloudWatchAlarmConfiguration, Dimension, HealthCheckConfig, COMPARISON_OPERATOR_VALUES,
    STATISTIC_VALUES,
};

/// Whether `value` is one of the values a modelled enum carries.
fn modelled(values: &[&str], value: &str) -> bool {
    values.contains(&value)
}

/// The alarm `config` is monitoring, or `None` when it monitors none.
fn monitored_alarm_name(config: &HealthCheckConfig) -> Option<&str> {
    if config.r#type != HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC {
        return None;
    }
    config
        .alarm_identifier
        .as_ref()
        .map(|identifier| identifier.name.as_str())
}

/// The alarm configuration each of `configs` reports, in the order given.
///
/// ONE CloudWatch read for the whole batch, so reporting a page of health checks
/// costs one lock of that store rather than one per health check. Call it with
/// this service's own store guard already released: the two stores are never
/// locked at once.
pub fn configurations<Eng: Send + Sync + 'static>(
    engine: &Eng,
    configs: &[HealthCheckConfig],
) -> Vec<Option<CloudWatchAlarmConfiguration>> {
    let absent = || vec![None; configs.len()];
    let names: Vec<&str> = configs.iter().filter_map(monitored_alarm_name).collect();
    if names.is_empty() {
        return absent();
    }
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return absent();
    };
    let monitored: HashMap<&str, CloudWatchAlarmConfiguration> = {
        let store = state.lock();
        names
            .into_iter()
            .filter_map(|name| {
                let stored = store.alarms.get(name)?;
                Some((name, alarm_configuration(&stored.alarm)?))
            })
            .collect()
    };
    configs
        .iter()
        .map(|config| {
            monitored_alarm_name(config).and_then(|name| monitored.get(name).cloned())
        })
        .collect()
}

/// The alarm configuration one health check reports.
pub fn configuration<Eng: Send + Sync + 'static>(
    engine: &Eng,
    config: &HealthCheckConfig,
) -> Option<CloudWatchAlarmConfiguration> {
    configurations(engine, ::std::slice::from_ref(config))
        .pop()
        .flatten()
}

/// `alarm` as the `CloudWatchAlarmConfiguration` a health check monitoring it
/// reports, or `None` when the alarm has no such configuration to report.
///
/// Every member but `Dimensions` is required, and each is the alarm's own: the
/// metric it watches (`MetricName` / `Namespace` / `Dimensions`), the `Statistic`
/// and `Period` it watches it over, and the `EvaluationPeriods`, `Threshold` and
/// `ComparisonOperator` its evaluation compares by. An alarm that carries none of
/// them is not one a single metric and a standard statistic describe, so nothing is
/// reported for it rather than a configuration filled in with values the alarm does
/// not hold.
///
/// The statistic and the comparison are held to the value lists this service's own
/// model carries ([`STATISTIC_VALUES`], [`COMPARISON_OPERATOR_VALUES`]), which are
/// SMALLER than CloudWatch's: an alarm on a percentile or on an anomaly-detection
/// band compares by something `CloudWatchAlarmConfiguration` cannot express, so it
/// is reported as no configuration rather than as a value outside the enum.
fn alarm_configuration(
    alarm: &cloudwatch::ops::types::MetricAlarm,
) -> Option<CloudWatchAlarmConfiguration> {
    let statistic = alarm.statistic.clone()?;
    let comparison_operator = alarm.comparison_operator.clone()?;
    if !modelled(STATISTIC_VALUES, &statistic)
        || !modelled(COMPARISON_OPERATOR_VALUES, &comparison_operator)
    {
        return None;
    }
    Some(CloudWatchAlarmConfiguration {
        evaluation_periods: alarm.evaluation_periods?,
        threshold: alarm.threshold?,
        comparison_operator,
        period: alarm.period?,
        metric_name: alarm.metric_name.clone()?,
        namespace: alarm.namespace.clone()?,
        statistic,
        // The dimension set the alarm's metric is identified by, in the order
        // CloudWatch holds it; an alarm on an undimensioned metric reports no
        // dimension list at all.
        dimensions: alarm
            .dimensions
            .as_ref()
            .filter(|dimensions| !dimensions.is_empty())
            .map(|dimensions| {
                dimensions
                    .iter()
                    .map(|dimension| Dimension {
                        name: dimension.name.clone(),
                        value: dimension.value.clone(),
                    })
                    .collect()
            }),
    })
}
