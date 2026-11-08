// SPDX-License-Identifier: CC0-1.0

//! The Amazon Aurora and RDS databases Compute Optimizer has analysed FOR AN ACCOUNT.
//!
//! Compute Optimizer "generates recommendations for Amazon Aurora and RDS databases that
//! meet a specific set of requirements" from the utilization it observes on a DB instance
//! the account OWNS, so the databases it can report are the account's own inventory and
//! nothing else: a database no DB instance of the account corresponds to has no finding to
//! report, and an account that holds no DB instance at all reads the empty list. The
//! inventory is Amazon RDS's own store — the service that owns a DB instance — and the
//! utilization is the `AWS/RDS` series published for that instance, exactly as the Amazon
//! ECS analysis of this service reads the ECS inventory and the `AWS/ECS` series
//! ([`super::ecs_analysis`]).
//!
//! Everything a reader reports about a database is computed here from those two inputs and
//! from the preferences in force for it: its finding, the DB instance class the analysis
//! would move it to, what each class costs per month and the utilization observed on it.
//! Nothing is written: this is the reporting side of an analysis no request to this API
//! creates.
//!
//! [`analysed_databases`] is the ONE selection every database surface of this service reads
//! — the recommendation reader, the projection of its recommended option, the summary that
//! counts it and the export of the same rows — so no two of them can disagree about which
//! databases the account has an analysis for.

use super::*;

use std::collections::BTreeSet;

use crate::ops::types::R_D_S_D_B_METRIC_NAME_VALUES;
use ::service_com_amazonaws_rds_amazonrdsv19 as rds;

/// The CloudWatch namespace Amazon RDS publishes a DB instance's utilization in, and the
/// dimension that identifies ONE instance's series in it.
///
/// Source: the Amazon RDS metrics reference ("Monitoring Amazon RDS metrics with Amazon
/// CloudWatch"), which publishes the instance-level metrics below in the `AWS/RDS` namespace
/// under the `DBInstanceIdentifier` dimension.
pub const RDS_METRIC_NAMESPACE: &str = "AWS/RDS";
/// See [`RDS_METRIC_NAMESPACE`].
pub const RDS_DB_INSTANCE_DIMENSION: &str = "DBInstanceIdentifier";

/// The `AWS/RDS` series this analysis reads, each paired with the `RDSDBMetricName` the
/// recommendation reports it as ([`R_D_S_D_B_METRIC_NAME_VALUES`]).
///
/// Left column: the metric name Amazon RDS publishes. Right column: the member of this API's
/// own metric vocabulary that measures the same thing. A series the account published nothing
/// of in the look-back window is reported on no member at all, rather than as a zero nobody
/// observed.
///
/// `Memory` is NOT among them: Amazon RDS publishes `FreeableMemory` in bytes, and the
/// percentage this API's `Memory` member carries is that figure against the memory of the DB
/// instance class — a capacity no catalog of this emulator publishes — so the analysis records
/// nothing for it rather than inventing one.
pub const ANALYSED_SERIES: &[(&str, &str)] = &[
    ("CPUUtilization", RDS_METRIC_CPU),
    ("DatabaseConnections", "DatabaseConnections"),
    ("NetworkReceiveThroughput", "NetworkReceiveThroughput"),
    ("NetworkTransmitThroughput", "NetworkTransmitThroughput"),
    ("ReadIOPS", "EBSVolumeReadIOPS"),
    ("WriteIOPS", "EBSVolumeWriteIOPS"),
    ("ReadThroughput", "EBSVolumeReadThroughput"),
    ("WriteThroughput", "EBSVolumeWriteThroughput"),
];

/// The `AWS/RDS` series the database's storage-space utilization is DERIVED from: Amazon RDS
/// publishes the storage left ("`FreeStorageSpace` — the amount of available storage space",
/// in bytes), and the `EBSVolumeStorageSpaceUtilization` this API reports is the share of the
/// instance's allocated storage in USE — so the two are read against the allocated storage
/// the RDS inventory holds for the instance ([`storage_space_utilization`]).
const RDS_FREE_STORAGE_SPACE_METRIC: &str = "FreeStorageSpace";
/// See [`RDS_FREE_STORAGE_SPACE_METRIC`].
const METRIC_STORAGE_SPACE_UTILIZATION: &str = "EBSVolumeStorageSpaceUtilization";

/// The statistic a free-space observation is read under to report the PEAK utilization: the
/// least storage left in the window is the most storage in use. An `RDSDBMetricStatistic`
/// value (`crate::ops::types::R_D_S_D_B_METRIC_STATISTIC_VALUES`), as the statistic every
/// other series is folded under ([`METRIC_STATISTIC_MAXIMUM`]) is.
const METRIC_STATISTIC_MINIMUM: &str = "Minimum";

/// Bytes in one GiB, the unit `DBInstance$AllocatedStorage` states a DB instance's storage in
/// ("the amount of storage in gibibytes (GiB)") and the unit `FreeStorageSpace` is published
/// in (bytes).
const BYTES_PER_GIB: f64 = 1_073_741_824.0;

/// The ARN namespace an Amazon Aurora or RDS resource carries, and the resource KIND a DB
/// instance's ARN names: "arn:aws:rds:{region}:{accountId}:db:{resourceName}"
/// (`GetRDSDatabaseRecommendations$resourceArns`).
pub const RDS_ARN_NAMESPACE: &str = "rds";
/// See [`RDS_ARN_NAMESPACE`].
pub const RDS_DB_RESOURCE_KIND: &str = "db";

/// The prefix every DB instance class carries: a class is written `db.<family>.<size>`
/// (`DBInstance$DBInstanceClass`).
const DB_INSTANCE_CLASS_PREFIX: &str = "db.";

/// The vCPUs one `xlarge` instance size provides, the scale
/// [`super::instance_size_capacity`]'s multiples are of: AWS's instance-size naming gives an
/// `xlarge` 4 vCPUs and each `<N>xlarge` N times that.
pub const VCPUS_PER_XLARGE: f64 = 4.0;

/// The public on-demand price of one DB instance vCPU-hour, in [`CURRENCY_USD`].
///
/// This emulator watches no price list, so a database is priced per vCPU of its DB instance
/// class, the way the Amazon ECS analysis of this service prices a task per vCPU and per GiB:
/// the figure is the published Amazon RDS On-Demand price of a `db.m5.large` single-AZ
/// PostgreSQL instance in `us-east-1` ($0.178 per hour) over that class's 2 vCPUs. Every
/// monthly cost and saving this analysis reports is computed from it
/// ([`rds_instance_monthly_cost`]), which is what makes the savings a recommendation states
/// agree with the two classes it compares.
pub const RDS_PRICE_PER_VCPU_HOUR: f64 = 0.089;

/// What a Multi-AZ deployment multiplies the instance cost by: "in a Multi-AZ DB instance
/// deployment, Amazon RDS automatically provisions and maintains a synchronous standby
/// replica in a different Availability Zone" (`DBInstance$MultiAZ`), so such a deployment runs
/// two instances of the class and is charged for both.
const MULTI_AZ_INSTANCE_FACTOR: f64 = 2.0;

/// The `RDSInstanceFindingReasonCode` values
/// (`crate::ops::types::R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES`) this analysis reaches: the
/// reason the DB instance class it recommends is smaller, and the reason it is larger.
pub const RDS_REASON_CPU_OVERPROVISIONED: &str = "CPUOverprovisioned";
/// See [`RDS_REASON_CPU_OVERPROVISIONED`].
pub const RDS_REASON_CPU_UNDERPROVISIONED: &str = "CPUUnderprovisioned";

/// The analysed Amazon Aurora and RDS databases of one account at the endpoint of `region`,
/// in the order a reader reports and pages them: the rows the findings catalog holds for the
/// account, then one row per DB instance of its Amazon RDS inventory the catalog holds none
/// for, in DB instance identifier order.
///
/// A DB instance becomes a row only once the analysis has something to compute from, which is
/// the "specific set of requirements" Compute Optimizer documents for a database: the account
/// is enrolled and the analysis its enrollment started is due
/// ([`Enrollment::analysis_visible`], the gate every reader of this service applies), the
/// instance is one this endpoint's region holds and its engine is RUNNING (its Amazon RDS
/// status is `available`), its DB instance class has a capacity this service can size against
/// ([`rds_class_vcpus`]), and the account has published utilization for it in the look-back
/// window. A DB instance that fails any of those is absent from the answer — the state of
/// every database of a freshly enrolled account, which Compute Optimizer has accumulated no
/// metric history for yet — rather than reported with an invented finding.
///
/// Reads only: the Amazon RDS inventory and the CloudWatch series are another service's state,
/// and this never writes to either, nor to this service's own store.
pub fn analysed_databases<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ComAmazonawsComputeoptimizerComputeoptimizerserviceState,
    region: &str,
    account_id: &str,
    now: f64,
) -> Vec<AnalysedResource> {
    // The gate every findings reader applies, ahead of any inventory read: an account that
    // never opted in, one that opted out and one that has just opted in hold no analysis at
    // all, whatever their inventory holds.
    if !store.enrollment.analysis_visible() {
        return Vec::new();
    }
    // The rows the findings catalog holds for the account: the analysis output this service
    // has recorded, which a database the inventory no longer holds is still reported from.
    let mut rows: Vec<AnalysedResource> = store
        .visible_analysed_resources()
        .filter(|resource| resource.resource_type == SOURCE_TYPE_RDS_DB_INSTANCE)
        .filter(|resource| resource.account_id == account_id)
        .cloned()
        .collect();
    // The inventory is the ONE account this emulator presents ([`is_foreign_account`]): a
    // request asking about any other account was refused by the reader that took it, so no
    // inventory is swept on its behalf here either.
    if account_id != ::engine::emulator_account_id() {
        return rows;
    }
    // An emulator that does not hold the Amazon RDS inventory, or the metric store the
    // analysis reads, has no database to analyse — the same answer as an account that owns
    // none — rather than a finding computed from an input nothing provided.
    let (Some(inventory), Some(observations)) = (
        rds::state::service_state(engine),
        cloudwatch::state::service_state(engine),
    ) else {
        return rows;
    };
    let inventory = inventory.lock();
    let observations = observations.lock();
    // A database the catalog already holds a recorded analysis for is reported from that row
    // and not analysed a second time, so one database is one element of the answer.
    let recorded: BTreeSet<String> = rows
        .iter()
        .map(|resource| resource.arn_resource.clone())
        .collect();
    // One region's ordered range, never the whole inventory, so the work under the lock does
    // not grow with what other regions hold: Compute Optimizer is regional, and the databases
    // this endpoint analyses are its own region's.
    for (key, record) in inventory
        .db_instances
        .range((region.to_string(), String::new())..)
    {
        if key.0 != region {
            break;
        }
        let arn_resource = format!("{RDS_DB_RESOURCE_KIND}:{}", record.identifier);
        if recorded.contains(&arn_resource) {
            continue;
        }
        if let Some(analysed) =
            analysed_database(store, &observations, record, arn_resource, region, now)
        {
            rows.push(analysed);
        }
    }
    rows
}

/// One DB instance of the account's Amazon RDS inventory as an analysed resource, or `None`
/// for one the analysis has nothing to compute a finding from (see [`analysed_databases`]).
///
/// `arn_resource` is the instance's REGION-FREE identity (`db:<identifier>`), which the
/// caller already holds because it is what it selects a recorded row by.
fn analysed_database(
    store: &ComAmazonawsComputeoptimizerComputeoptimizerserviceState,
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    record: &rds::state::DbInstanceRecord,
    arn_resource: String,
    region: &str,
    now: f64,
) -> Option<AnalysedResource> {
    // Compute Optimizer analyses the workload a database is RUNNING, so an instance whose
    // engine is not up — one still being created, stopped, or on its way out — is not one
    // the analysis has a workload for. The status is the Amazon RDS store's own, settled by
    // that service's clock; this only reads it.
    if record.status != rds::state::DB_INSTANCE_STATUS_AVAILABLE {
        return None;
    }
    let account_id = ::engine::emulator_account_id();
    let resource_arn = resource_arn_in(RDS_ARN_NAMESPACE, region, account_id, &arn_resource);
    // The look-back period in force for THIS database, by the one scope precedence this
    // service resolves a preference with, so the window the analysis folds is the window the
    // recommendation says it was generated over.
    let look_back_period = store
        .effective_preferences_for(&resource_arn, SOURCE_TYPE_RDS_DB_INSTANCE, account_id)
        .look_back_period;
    let oldest = now - look_back_period_in_days(&look_back_period) * SECONDS_PER_DAY;
    let utilization_metrics = observed_utilization(observations, record, oldest, now);
    // "Compute Optimizer generates recommendations for ... databases that meet a specific set
    // of requirements": the utilization of the instance over the look-back period is the
    // analysis's input, so a database the account published none for has no finding at all.
    // CPU is the series the instance class is sized against, so it is the one the analysis
    // cannot do without.
    let cpu_peak = utilization_metrics
        .iter()
        .find(|metric| metric.name.as_deref() == Some(RDS_METRIC_CPU))
        .map(|metric| metric.value)?;
    // A DB instance class whose size the capacity catalog does not record (the serverless
    // capacity class, which is sized by the engine rather than by a class) cannot be compared
    // with another class, so there is no finding to reach for it.
    let current_vcpus = rds_class_vcpus(&record.db_instance_class)?;

    // The capacity the observed peak needs, carried with this service's one recommendation
    // headroom ([`RECOMMENDATION_HEADROOM`], the documented "`PERCENT_20` for headroom"), and
    // the smallest class of the database's own family that provides it.
    let required_vcpus = current_vcpus * cpu_peak / UTILIZATION_PERCENTAGE_CEILING
        * (1.0 + RECOMMENDATION_HEADROOM);
    let recommended_class = rds_recommended_class(
        &record.engine,
        &record.db_instance_class,
        required_vcpus,
    );
    let recommended_vcpus = rds_class_vcpus(&recommended_class).unwrap_or(current_vcpus);
    // The finding states what the analysis found against the CURRENT class, and the reason
    // code says which measured resource it found it on: a class the observed peak needs more
    // of than it provides is under-provisioned — whether or not a bigger class exists to
    // recommend — one the peak leaves room under is over-provisioned, and a class the
    // analysis would keep is optimized and carries no reason code.
    let (finding, reason_code) = if required_vcpus > current_vcpus {
        (
            FINDING_UNDERPROVISIONED,
            Some(RDS_REASON_CPU_UNDERPROVISIONED),
        )
    } else if recommended_vcpus < current_vcpus {
        (FINDING_OVERPROVISIONED, Some(RDS_REASON_CPU_OVERPROVISIONED))
    } else {
        (FINDING_OPTIMIZED, None)
    };
    let current_monthly_cost =
        rds_instance_monthly_cost(&record.db_instance_class, record.multi_az).unwrap_or_default();
    let recommended_monthly_cost =
        rds_instance_monthly_cost(&recommended_class, record.multi_az)
            .unwrap_or(current_monthly_cost);

    Some(AnalysedResource {
        account_id: account_id.to_string(),
        resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
        arn_service: RDS_ARN_NAMESPACE.to_string(),
        arn_resource,
        finding: finding.to_string(),
        finding_reason_codes: reason_code
            .map(|code| vec![code.to_string()])
            .unwrap_or_default(),
        // How close the observed peak came to the capacity carrying it, on this service's one
        // risk scale ([`performance_risk_score`]), reported in the band the model's
        // `RDSCurrentInstancePerformanceRisk` names it by.
        current_performance_risk: performance_risk_band(performance_risk_score(
            cpu_peak,
            UTILIZATION_PERCENTAGE_CEILING,
        ))
        .to_string(),
        current_monthly_cost,
        recommended_monthly_cost,
        // `RDSDBRecommendation` reports no inferred workload type, so the analysis of a
        // database records none.
        inferred_workload_types: Vec::new(),
        rds_database: Some(RdsDatabaseDetail {
            engine: Some(record.engine.clone()),
            engine_version: record.engine_version.clone(),
            // The Amazon RDS inventory records no promotion tier for an instance, so the
            // analysis has none to report.
            promotion_tier: None,
            db_cluster_identifier: record.db_cluster_identifier.clone(),
            current_db_instance_class: Some(record.db_instance_class.clone()),
            // Only where the analysis would MOVE the database: a class it would keep is no
            // recommendation option.
            recommended_db_instance_class: (recommended_class != record.db_instance_class)
                .then(|| recommended_class.clone()),
            // The storage the inventory holds for the instance. A cluster member's storage is
            // the CLUSTER's, so the instance records no allocated storage and the analysis
            // reports no storage configuration for it rather than one of nothing.
            current_storage_configuration: record.allocated_storage.map(|allocated_storage| {
                DBStorageConfiguration {
                    storage_type: Some(record.storage_type.clone()),
                    allocated_storage,
                    iops: record.iops,
                    max_allocated_storage: record.max_allocated_storage,
                    storage_throughput: Some(record.storage_throughput),
                }
            }),
            // The finding this analysis reaches is about the DB INSTANCE class, so there is no
            // storage configuration it would move the database to.
            recommended_storage_configuration: None,
            utilization_metrics,
            // When the analysis was generated: the instant every reader of this service
            // reports for it.
            last_refresh_timestamp: store.enrollment.last_analysis_refresh(),
            tags: record
                .tags
                .iter()
                .map(|tag| Tag {
                    key: tag.key.clone(),
                    value: tag.value.clone(),
                })
                .collect(),
        }),
    })
}

/// The utilization the account published for one DB instance over the window `oldest..now`,
/// as the `RDSDBUtilizationMetric`s a recommendation reports: one entry per [`ANALYSED_SERIES`]
/// row the window holds an observation of, plus the storage-space utilization derived from the
/// free space published for it ([`storage_space_utilization`]).
///
/// Every entry is reported under [`METRIC_STATISTIC_MAXIMUM`] — "the Compute Optimizer API,
/// Command Line Interface (CLI), and SDKs return utilization metrics using only the `Maximum`
/// statistic" — and the fold is CloudWatch's own ([`cloudwatch::state::BasicStatistics`]), the
/// same one `GetMetricStatistics` aggregates a period with, so the figure this analysis
/// reports is the figure CloudWatch itself would report for the window.
fn observed_utilization(
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    record: &rds::state::DbInstanceRecord,
    oldest: f64,
    now: f64,
) -> Vec<RDSDBUtilizationMetric> {
    debug_assert!(
        ANALYSED_SERIES
            .iter()
            .all(|(_, reported)| R_D_S_D_B_METRIC_NAME_VALUES.contains(reported))
            && R_D_S_D_B_METRIC_NAME_VALUES.contains(&METRIC_STORAGE_SPACE_UTILIZATION),
        "an analysed series may only be reported as a modelled RDSDBMetricName"
    );
    let mut metrics: Vec<RDSDBUtilizationMetric> = ANALYSED_SERIES
        .iter()
        .filter_map(|(published, reported)| {
            let value = observed_statistic(
                observations,
                &record.identifier,
                published,
                METRIC_STATISTIC_MAXIMUM,
                oldest,
                now,
            )?;
            Some(RDSDBUtilizationMetric {
                name: Some((*reported).to_string()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: round_hundredths(value),
            })
        })
        .collect();
    let free_space = observed_statistic(
        observations,
        &record.identifier,
        RDS_FREE_STORAGE_SPACE_METRIC,
        METRIC_STATISTIC_MINIMUM,
        oldest,
        now,
    );
    if let Some(value) = free_space
        .zip(record.allocated_storage)
        .and_then(|(free_bytes, allocated)| storage_space_utilization(free_bytes, allocated))
    {
        metrics.push(RDSDBUtilizationMetric {
            name: Some(METRIC_STORAGE_SPACE_UTILIZATION.to_string()),
            statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
            value,
        });
    }
    metrics
}

/// The share of an instance's allocated storage in use, as a percentage, given the least
/// storage left in the window: the allocated storage less what is free, over the allocated
/// storage. `None` for an instance with no allocated storage, which no share is a share OF.
fn storage_space_utilization(free_bytes: f64, allocated_storage: i32) -> Option<f64> {
    let capacity = f64::from(allocated_storage) * BYTES_PER_GIB;
    if capacity <= 0.0 {
        return None;
    }
    let used = (capacity - free_bytes.max(0.0)).max(0.0);
    Some(round_hundredths(
        (used / capacity * UTILIZATION_PERCENTAGE_CEILING).clamp(0.0, UTILIZATION_PERCENTAGE_CEILING),
    ))
}

/// One statistic of one DB instance's `AWS/RDS` series over the window `oldest..now`, or
/// `None` when the account published nothing of it in that window.
///
/// A datapoint whose aggregates are not finite describes no utilization, so it is left out of
/// the fold exactly as one outside the window is — the same rule the Amazon ECS analysis of
/// this service folds a series by.
fn observed_statistic(
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    identifier: &str,
    metric_name: &str,
    statistic: &str,
    oldest: f64,
    now: f64,
) -> Option<f64> {
    let key = cloudwatch::state::MetricKey {
        namespace: RDS_METRIC_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions: [(
            RDS_DB_INSTANCE_DIMENSION.to_string(),
            identifier.to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let data = observations.metrics.get(&key)?;
    let window = data.iter().filter(|datum| {
        datum.timestamp_secs.is_finite()
            && datum.timestamp_secs >= oldest
            && datum.timestamp_secs <= now
            && datum.minimum.is_finite()
            && datum.maximum.is_finite()
            && datum.sum.is_finite()
            && datum.sample_count.is_finite()
    });
    cloudwatch::state::BasicStatistics::of(window).named(statistic)
}

/// The vCPUs one DB instance class provides, or `None` for a class whose size the capacity
/// catalog records no multiple for ([`instance_size_capacity`]).
///
/// A DB instance class is `db.<family>.<size>` and the size names are the same doubling
/// sequence an Amazon EC2 instance type's are, so the capacity of a class is that multiple of
/// an `xlarge`'s [`VCPUS_PER_XLARGE`] vCPUs — the ONE capacity scale this service compares
/// instance sizes on, shared with the projection of a resized database's utilization.
pub fn rds_class_vcpus(db_instance_class: &str) -> Option<f64> {
    instance_size_capacity(db_instance_class).map(|capacity| capacity * VCPUS_PER_XLARGE)
}

/// What one DB instance of `db_instance_class` costs per month, in [`CURRENCY_USD`]: the
/// class's vCPUs at [`RDS_PRICE_PER_VCPU_HOUR`] for [`HOURS_PER_MONTH`], doubled for a
/// Multi-AZ deployment ([`MULTI_AZ_INSTANCE_FACTOR`]). `None` for a class with no capacity to
/// price ([`rds_class_vcpus`]).
pub fn rds_instance_monthly_cost(db_instance_class: &str, multi_az: bool) -> Option<f64> {
    let vcpus = rds_class_vcpus(db_instance_class)?;
    let instances = if multi_az {
        MULTI_AZ_INSTANCE_FACTOR
    } else {
        1.0
    };
    Some(round_hundredths(
        vcpus * RDS_PRICE_PER_VCPU_HOUR * instances * HOURS_PER_MONTH,
    ))
}

/// The DB instance class the analysis would run the database on: the SMALLEST class of its own
/// family that provides `required_vcpus`, and the family's largest where none does — past it
/// there is no bigger class to recommend, so the database stays where it is and its finding
/// still says the class does not carry its workload.
///
/// The candidates are the classes Amazon RDS publishes as orderable for the database's ENGINE
/// (`rds::ops::describe_orderable_db_instance_options::instance_classes`, "this service's one
/// published answer to what a DB instance can be"), so a class this analysis recommends is a
/// class the database could actually be modified to run on. The family is kept: a
/// recommendation that changed the family would change the processor generation and, for a
/// Graviton family, the architecture, which is a different recommendation from sizing.
/// `current` stands where the engine publishes no class of its family at all, since there is
/// then nothing to move to.
pub fn rds_recommended_class(engine: &str, current: &str, required_vcpus: f64) -> String {
    let Some(family) = rds_class_family(current) else {
        return current.to_string();
    };
    let mut candidates: Vec<(f64, String)> =
        rds::ops::describe_orderable_db_instance_options::instance_classes(engine)
            .into_iter()
            .filter(|class| rds_class_family(&class.name) == Some(family))
            .filter_map(|class| rds_class_vcpus(&class.name).map(|vcpus| (vcpus, class.name)))
            .collect();
    candidates.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(::std::cmp::Ordering::Equal)
            .then_with(|| left.1.cmp(&right.1))
    });
    let smallest_that_fits = candidates
        .iter()
        .find(|(vcpus, _)| *vcpus >= required_vcpus)
        .map(|(_, name)| name.clone());
    smallest_that_fits
        .or_else(|| candidates.last().map(|(_, name)| name.clone()))
        .unwrap_or_else(|| current.to_string())
}

/// The FAMILY part of a DB instance class — the `m5` of `db.m5.large` — or `None` for a value
/// that is not a family-and-size class (`db.serverless`, the serverless capacity class).
fn rds_class_family(db_instance_class: &str) -> Option<&str> {
    let rest = db_instance_class.strip_prefix(DB_INSTANCE_CLASS_PREFIX)?;
    let (family, size) = rest.split_once('.')?;
    (!family.is_empty() && !size.is_empty()).then_some(family)
}
