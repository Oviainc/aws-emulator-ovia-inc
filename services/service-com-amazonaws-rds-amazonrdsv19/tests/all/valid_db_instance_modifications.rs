// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeValidDBInstanceModifications`, the report a caller reads
//! to learn what it may modify a DB instance to.
//!
//! Every call goes over the wire through the same router the production emulator mounts
//! (`crate::db_instances::emulator`), as a real awsQuery form POST.
//!
//! The rule under test is that the bounds reported for an instance are the bounds of the
//! storage types and the DB instance class ITS engine and class publish — the same
//! catalogue `DescribeOrderableDBInstanceOptions` orders against. So the rule is asserted
//! two ways over TABLES of instances whose engine, class and storage differ (burstable and
//! standard classes, the three engine storage caps, a class outside the published family
//! table, the cluster engines of all three control planes this front door serves):
//! against that other report member by member, and against the published storage table
//! itself.

use std::collections::{BTreeMap, BTreeSet};

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text,
    wait_for_settle, REGION,
};

/// The operation under test.
const ACTION: &str = "DescribeValidDBInstanceModifications";

/// The canonical account id every generated emulator presents, as the ARN form of an
/// identifier carries it.
const ACCOUNT: &str = "000000000000";

/// An integer range the report carries (`Range`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedRange {
    from: i64,
    to: i64,
    step: Option<i64>,
}

/// A double range the report carries (`DoubleRange`).
#[derive(Debug, Clone, PartialEq)]
struct ReportedDoubleRange {
    from: f64,
    to: f64,
}

/// One `<ValidStorageOptions>` of the report.
#[derive(Debug, Clone)]
struct ReportedStorage {
    /// `StorageSize`, which is a LIST of ranges: empty when the type carries no size
    /// bound at all.
    storage_size: Vec<ReportedRange>,
    provisioned_iops: Option<ReportedRange>,
    iops_to_storage_ratio: Option<ReportedDoubleRange>,
    provisioned_storage_throughput: Option<ReportedRange>,
    storage_throughput_to_iops_ratio: Option<ReportedDoubleRange>,
    supports_storage_autoscaling: Option<String>,
}

/// One `<AvailableProcessorFeature>` of the report, as the three members it carries.
type ReportedFeature = (String, String, String);

/// The whole `<ValidDBInstanceModificationsMessage>`, as the members this suite asserts
/// on: the storage options keyed by their `StorageType`, the processor features and the
/// dedicated-log-volume flag.
#[derive(Debug, Clone)]
struct Reported {
    storage: BTreeMap<String, ReportedStorage>,
    valid_processor_features: Vec<ReportedFeature>,
    supports_dedicated_log_volume: Option<String>,
    additional_storage: Option<String>,
    xml: String,
}

/// Every `<Range>` the `member` list carries, in reported order, and none when the option
/// carries no such bound at all.
fn ranges_of(xml: &str, member: &str) -> Vec<ReportedRange> {
    let Some(wrapper) = subtrees(xml, member).into_iter().next() else {
        return Vec::new();
    };
    subtrees(&wrapper, "Range")
        .into_iter()
        .map(|range| ReportedRange {
            from: integer(&text(&range, "From")),
            to: integer(&text(&range, "To")),
            step: optional_text(&range, "Step").map(|step| integer(&step)),
        })
        .collect()
}

/// The one `<Range>` the `member` list carries, or `None` when the option carries no such
/// bound at all.
fn range_of(xml: &str, member: &str) -> Option<ReportedRange> {
    let ranges = ranges_of(xml, member);
    if ranges.is_empty() {
        return None;
    }
    assert_eq!(
        ranges.len(),
        1,
        "case {member:?}: expected exactly one <Range> in {xml}"
    );
    ranges.into_iter().next()
}

/// The one `<DoubleRange>` list `member` carries, or `None` when it carries none.
fn double_range_of(xml: &str, member: &str) -> Option<ReportedDoubleRange> {
    let wrapper = subtrees(xml, member).into_iter().next()?;
    let ranges: Vec<ReportedDoubleRange> = subtrees(&wrapper, "DoubleRange")
        .into_iter()
        .map(|range| ReportedDoubleRange {
            from: number(&text(&range, "From")),
            to: number(&text(&range, "To")),
        })
        .collect();
    assert_eq!(
        ranges.len(),
        1,
        "case {member:?}: expected exactly one <DoubleRange> in {wrapper}"
    );
    ranges.into_iter().next()
}

/// A reported integer, which is a whole number or the test is reading the wrong element.
fn integer(value: &str) -> i64 {
    value
        .parse()
        .unwrap_or_else(|_| panic!("expected an integer on the wire, got {value:?}"))
}

/// A reported double.
fn number(value: &str) -> f64 {
    value
        .parse()
        .unwrap_or_else(|_| panic!("expected a double on the wire, got {value:?}"))
}

/// The report in one response body.
fn reported(case: &str, body: &str) -> Reported {
    let message = subtrees(body, "ValidDBInstanceModificationsMessage")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response carries no ValidDBInstanceModificationsMessage: {body}")
        });
    let storage_list = subtrees(&message, "Storage")
        .into_iter()
        .next()
        .unwrap_or_default();
    let mut storage: BTreeMap<String, ReportedStorage> = BTreeMap::new();
    for option in subtrees(&storage_list, "ValidStorageOptions") {
        let storage_type = text(&option, "StorageType");
        let previous = storage.insert(
            storage_type.clone(),
            ReportedStorage {
                storage_size: ranges_of(&option, "StorageSize"),
                provisioned_iops: range_of(&option, "ProvisionedIops"),
                iops_to_storage_ratio: double_range_of(&option, "IopsToStorageRatio"),
                provisioned_storage_throughput: range_of(&option, "ProvisionedStorageThroughput"),
                storage_throughput_to_iops_ratio: double_range_of(
                    &option,
                    "StorageThroughputToIopsRatio",
                ),
                supports_storage_autoscaling: optional_text(&option, "SupportsStorageAutoscaling"),
            },
        );
        assert!(
            previous.is_none(),
            "case {case:?}: storage type {storage_type:?} is reported twice in {message}"
        );
    }
    let features = subtrees(&message, "ValidProcessorFeatures")
        .into_iter()
        .next()
        .unwrap_or_default();
    Reported {
        storage,
        valid_processor_features: subtrees(&features, "AvailableProcessorFeature")
            .into_iter()
            .map(|feature| {
                (
                    text(&feature, "Name"),
                    text(&feature, "DefaultValue"),
                    text(&feature, "AllowedValues"),
                )
            })
            .collect(),
        supports_dedicated_log_volume: optional_text(&message, "SupportsDedicatedLogVolume"),
        additional_storage: subtrees(&message, "AdditionalStorage").into_iter().next(),
        xml: message,
    }
}

/// `DescribeValidDBInstanceModifications` for one instance, asserted to answer 200.
async fn describe(router: &MultiServiceRouter, case: &str, named: &str) -> Reported {
    let body = expect_ok(
        case,
        call(router, ACTION, &[("DBInstanceIdentifier", named.to_string())]).await,
    );
    reported(case, &body)
}

/// Create one DB instance and hand back its identifier.
///
/// An instance of one of the sibling control planes' engines exists only as a MEMBER of a
/// DB cluster — its master user is the cluster's, and this front door refuses a create that
/// names no cluster for them — so such a row's instance joins a cluster created here.
async fn create(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
    extra: &[(&str, String)],
) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("DBInstanceClass", class.to_string()),
    ];
    if matches!(engine, "docdb" | "neptune") {
        let cluster = format!("{identifier}-cluster");
        expect_ok(
            &format!("create the cluster {cluster}"),
            call(
                router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", cluster.clone()),
                    ("Engine", engine.to_string()),
                ],
            )
            .await,
        );
        params.push(("DBClusterIdentifier", cluster));
    }
    params.extend(extra.iter().cloned());
    expect_ok(
        &format!("create {identifier}"),
        call(router, "CreateDBInstance", &params).await,
    );
}

/// One row of the tables below: a DB instance to create, and what the published storage
/// table says a modification of it is bounded by.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// The engine the instance runs.
    engine: &'static str,
    /// The DB instance class it runs on.
    class: &'static str,
    /// The storage types a modification of it can choose, in sorted order.
    storage_types: &'static [&'static str],
    /// The largest `AllocatedStorage` the engine accepts, in GiB — the cap every
    /// per-instance storage type's reported range is held to, or `None` for an engine
    /// whose storage its CLUSTER manages, which reports no size range at all.
    max_storage_size: Option<i64>,
    /// Whether the class exposes configurable processor features.
    processor_features: bool,
    /// Whether the engine takes a dedicated log volume.
    dedicated_log_volume: bool,
    /// Whether `DescribeOrderableDBInstanceOptions` publishes this class for this engine:
    /// a class outside the published family table is still a class an instance can run,
    /// and is reported from its name, so there is no orderable option to cross-check it
    /// against.
    published_class: bool,
}

/// The instances this suite reports on, and the bounds the PUBLISHED storage table gives
/// each.
///
/// Source: the storage profiles and instance-class families in
/// `src/ops/describe_orderable_db_instance_options.rs`, which carry the "Amazon RDS DB
/// instance storage" section of the Amazon RDS User Guide (General Purpose SSD `gp2`/`gp3`
/// and Provisioned IOPS SSD `io1`/`io2`, with the per-engine storage caps), plus the
/// cluster engines whose storage the cluster manages (`aurora`, and the `standard` type
/// the Amazon DocumentDB and Amazon Neptune front doors publish).
///
/// The rows differ along everything the rule generalises over: burstable versus standard
/// class, all three engine storage caps (65,536 / 16,384 / 64,000 GiB), a class the
/// published family table does NOT carry, the two engines whose classes expose processor
/// features, and the three cluster control planes.
const ROWS: &[Row] = &[
    Row {
        engine: "mysql",
        class: "db.t3.micro",
        storage_types: &["gp2", "gp3", "io1", "io2"],
        max_storage_size: Some(65_536),
        processor_features: false,
        dedicated_log_volume: true,
        published_class: true,
    },
    Row {
        engine: "postgres",
        class: "db.m5.large",
        storage_types: &["gp2", "gp3", "io1", "io2"],
        max_storage_size: Some(65_536),
        processor_features: false,
        dedicated_log_volume: true,
        published_class: true,
    },
    Row {
        engine: "mariadb",
        // A class of a published family in a size that family's published row does NOT
        // carry: still a DB instance class (`CreateDBInstance` admits every value of the
        // published form), reported as a standard class with no configurable processor.
        class: "db.m5.24xlarge",
        storage_types: &["gp2", "gp3", "io1", "io2"],
        max_storage_size: Some(65_536),
        processor_features: false,
        dedicated_log_volume: true,
        published_class: false,
    },
    Row {
        engine: "oracle-se2",
        class: "db.m5.2xlarge",
        storage_types: &["gp2", "gp3", "io1", "io2"],
        max_storage_size: Some(65_536),
        processor_features: true,
        dedicated_log_volume: true,
        published_class: true,
    },
    Row {
        engine: "sqlserver-ee",
        class: "db.r5.xlarge",
        storage_types: &["gp2", "gp3", "io1", "io2"],
        // Microsoft SQL Server caps allocated storage below what the storage types reach.
        max_storage_size: Some(16_384),
        processor_features: true,
        dedicated_log_volume: true,
        published_class: true,
    },
    Row {
        engine: "sqlserver-ex",
        class: "db.t3.small",
        storage_types: &["gp2", "gp3", "io1", "io2"],
        max_storage_size: Some(16_384),
        processor_features: false,
        dedicated_log_volume: true,
        published_class: true,
    },
    Row {
        engine: "db2-se",
        class: "db.m5.large",
        storage_types: &["gp2", "gp3", "io1", "io2"],
        // Db2 caps allocated storage at its own value.
        max_storage_size: Some(64_000),
        processor_features: false,
        dedicated_log_volume: true,
        published_class: true,
    },
    Row {
        engine: "aurora-mysql",
        class: "db.r5.large",
        storage_types: &["aurora"],
        max_storage_size: None,
        processor_features: false,
        dedicated_log_volume: false,
        published_class: true,
    },
    Row {
        engine: "docdb",
        class: "db.r5.large",
        storage_types: &["standard"],
        max_storage_size: None,
        processor_features: false,
        dedicated_log_volume: false,
        published_class: true,
    },
    Row {
        engine: "neptune",
        class: "db.t3.medium",
        storage_types: &["standard"],
        max_storage_size: None,
        processor_features: false,
        dedicated_log_volume: false,
        published_class: true,
    },
];

/// The identifier the instance of row `index` is created under.
fn identifier(index: usize) -> String {
    format!("emu-vdbim-{index}")
}

/// Create every row's instance on one router, each with [`ALLOCATED_STORAGE`] GiB of the
/// storage type its engine defaults to where the instance carries its own storage.
async fn emulator_with_rows() -> MultiServiceRouter {
    let router = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        create(
            &router,
            &identifier(index),
            row.engine,
            row.class,
            &[("AllocatedStorage", ALLOCATED_STORAGE.to_string())],
        )
        .await;
    }
    router
}

/// The storage every row's instance is created with, in GiB: inside every per-instance
/// storage type's published range, so the reported range has to span it.
const ALLOCATED_STORAGE: i64 = 100;

#[tokio::test]
async fn the_reported_bounds_are_the_orderable_options_of_the_instances_engine_and_class() {
    let router = emulator_with_rows().await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = format!("{} on {}", row.engine, row.class);
        let report = describe(&router, &case, &identifier(index)).await;
        let orderable = orderable_storage(&router, &case, row.engine, row.class).await;
        if !row.published_class {
            // The cross-check holds only where there IS an orderable option: a class the
            // published family table does not carry is offered by no option, and the
            // table test above pins what this report says about it.
            assert!(
                orderable.is_empty(),
                "case {case:?}: this class is marked unpublished but orderable options \
                 report it: {orderable:?}"
            );
            continue;
        }

        let reported_types: BTreeSet<&String> = report.storage.keys().collect();
        let orderable_types: BTreeSet<&String> = orderable.keys().collect();
        assert_eq!(
            reported_types, orderable_types,
            "case {case:?}: the storage types a modification may choose are not the ones \
             DescribeOrderableDBInstanceOptions offers this engine and class"
        );

        for (storage_type, option) in &orderable {
            let actual = &report.storage[storage_type];
            let case = format!("{case} on {storage_type}");
            // Every size range this report cuts for the instance lies inside the TYPE's
            // published range and reaches its ceiling; where the cut falls is the
            // instance's own storage, which
            // `the_storage_ranges_are_cut_by_what_the_instance_runs_now` pins.
            match bounds(option, "MinStorageSize", "MaxStorageSize") {
                Some(published) => {
                    assert!(
                        !actual.storage_size.is_empty(),
                        "case {case:?}: the option bounds storage at {published:?} and the \
                         report carries no StorageSize range: {actual:?}"
                    );
                    for range in &actual.storage_size {
                        assert!(
                            range.from >= published.from
                                && range.to <= published.to
                                && range.step == published.step,
                            "case {case:?}: the range {range:?} is not inside the type's \
                             published {published:?}"
                        );
                    }
                    assert_eq!(
                        actual.storage_size.iter().map(|range| range.to).max(),
                        Some(published.to),
                        "case {case:?}: the reported ranges stop short of the option's \
                         MaxStorageSize {}: {actual:?}",
                        published.to
                    );
                }
                None => assert!(
                    actual.storage_size.is_empty(),
                    "case {case:?}: the option bounds no storage size, so the report must \
                     carry no range: {actual:?}"
                ),
            }
            assert_eq!(
                actual.provisioned_iops,
                bounds(option, "MinIopsPerDbInstance", "MaxIopsPerDbInstance"),
                "case {case:?}: ProvisionedIops is not the option's \
                 Min/MaxIopsPerDbInstance, got {actual:?} against {option}"
            );
            assert_eq!(
                actual.iops_to_storage_ratio,
                double_bounds(option, "MinIopsPerGib", "MaxIopsPerGib"),
                "case {case:?}: IopsToStorageRatio is not the option's Min/MaxIopsPerGib, \
                 got {actual:?} against {option}"
            );
            assert_eq!(
                actual.provisioned_storage_throughput,
                bounds(
                    option,
                    "MinStorageThroughputPerDbInstance",
                    "MaxStorageThroughputPerDbInstance"
                ),
                "case {case:?}: ProvisionedStorageThroughput is not the option's \
                 Min/MaxStorageThroughputPerDbInstance, got {actual:?} against {option}"
            );
            assert_eq!(
                actual.storage_throughput_to_iops_ratio,
                double_bounds(
                    option,
                    "MinStorageThroughputPerIops",
                    "MaxStorageThroughputPerIops"
                ),
                "case {case:?}: StorageThroughputToIopsRatio is not the option's \
                 Min/MaxStorageThroughputPerIops, got {actual:?} against {option}"
            );
            assert_eq!(
                actual.supports_storage_autoscaling,
                optional_text(option, "SupportsStorageAutoscaling"),
                "case {case:?}: SupportsStorageAutoscaling disagrees with the orderable \
                 option, got {actual:?} against {option}"
            );
        }

        // The remaining two members are the CLASS's and the ENGINE's, so they are checked
        // against any one option of that engine and class.
        let option = orderable
            .values()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: no orderable option to compare against"));
        assert_eq!(
            report.supports_dedicated_log_volume,
            optional_text(option, "SupportsDedicatedLogVolume"),
            "case {case:?}: SupportsDedicatedLogVolume disagrees with the orderable \
             option, got {:?} against {option}",
            report.supports_dedicated_log_volume
        );
        let offered_features: Vec<ReportedFeature> = subtrees(option, "AvailableProcessorFeature")
            .into_iter()
            .map(|feature| {
                (
                    text(&feature, "Name"),
                    text(&feature, "DefaultValue"),
                    text(&feature, "AllowedValues"),
                )
            })
            .collect();
        assert_eq!(
            report.valid_processor_features, offered_features,
            "case {case:?}: ValidProcessorFeatures are not the class's \
             AvailableProcessorFeatures, got {:?} against {option}",
            report.valid_processor_features
        );
    }
}

/// `(From, To)` of the orderable option's `min`/`max` members as a [`ReportedRange`] with
/// the whole-unit step the report carries, or `None` when the option carries neither.
fn bounds(option: &str, min: &str, max: &str) -> Option<ReportedRange> {
    let from = optional_text(option, min).map(|value| integer(&value))?;
    let to = optional_text(option, max).map(|value| integer(&value))?;
    Some(ReportedRange {
        from,
        to,
        step: Some(1),
    })
}

/// The same for a ratio, which has no step.
fn double_bounds(option: &str, min: &str, max: &str) -> Option<ReportedDoubleRange> {
    let from = optional_text(option, min).map(|value| number(&value))?;
    let to = optional_text(option, max).map(|value| number(&value))?;
    Some(ReportedDoubleRange { from, to })
}

/// Every `<OrderableDBInstanceOption>` subtree `DescribeOrderableDBInstanceOptions`
/// reports for `engine` on `class`, keyed by its `StorageType`.
///
/// An option is one (engine version, class, license model, storage type) combination, and
/// the storage members are the TYPE's, so the first option of each type carries them.
async fn orderable_storage(
    router: &MultiServiceRouter,
    case: &str,
    engine: &str,
    class: &str,
) -> BTreeMap<String, String> {
    let mut options: BTreeMap<String, String> = BTreeMap::new();
    let mut marker: Option<String> = None;
    loop {
        let mut params: Vec<(&str, String)> = vec![
            ("Engine", engine.to_string()),
            ("DBInstanceClass", class.to_string()),
        ];
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let body = expect_ok(
            case,
            call(router, "DescribeOrderableDBInstanceOptions", &params).await,
        );
        for option in subtrees(&body, "OrderableDBInstanceOption") {
            let storage_type = text(&option, "StorageType");
            options.entry(storage_type).or_insert(option);
        }
        match optional_text(&body, "Marker") {
            Some(next) => marker = Some(next),
            None => break,
        }
    }
    options
}

#[tokio::test]
async fn every_reported_bound_is_well_formed_for_the_instances_engine_and_class() {
    let router = emulator_with_rows().await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = format!("{} on {}", row.engine, row.class);
        let report = describe(&router, &case, &identifier(index)).await;

        let reported_types: Vec<&str> = report.storage.keys().map(String::as_str).collect();
        assert_eq!(
            reported_types, row.storage_types,
            "case {case:?}: expected the published storage types {:?}, got {reported_types:?}",
            row.storage_types
        );
        assert_eq!(
            report.supports_dedicated_log_volume.as_deref(),
            Some(if row.dedicated_log_volume {
                "true"
            } else {
                "false"
            }),
            "case {case:?}: expected SupportsDedicatedLogVolume {}, got {:?}",
            row.dedicated_log_volume,
            report.supports_dedicated_log_volume
        );
        assert_eq!(
            !report.valid_processor_features.is_empty(),
            row.processor_features,
            "case {case:?}: expected configurable processor features {}, got {:?}",
            row.processor_features,
            report.valid_processor_features
        );
        for (name, default_value, allowed_values) in &report.valid_processor_features {
            assert!(
                // `AvailableProcessorFeature$Name`: "Valid names are coreCount and
                // threadsPerCore."
                matches!(name.as_str(), "coreCount" | "threadsPerCore"),
                "case {case:?}: {name:?} is not a published processor feature name"
            );
            assert!(
                allowed_values
                    .split(',')
                    .any(|allowed| allowed == default_value),
                "case {case:?}: feature {name:?} defaults to {default_value:?}, which is \
                 not among its allowed values {allowed_values:?}"
            );
        }
        // `AdditionalStorage` describes storage volumes this service models for no
        // instance, so no row claims any.
        assert_eq!(
            report.additional_storage, None,
            "case {case:?}: expected no AdditionalStorage, got {:?}",
            report.additional_storage
        );

        for (storage_type, option) in &report.storage {
            let case = format!("{case} on {storage_type}");
            for range in &option.storage_size {
                assert!(
                    range.from <= range.to,
                    "case {case:?}: StorageSize runs backwards: {range:?}"
                );
                assert_eq!(
                    range.step,
                    Some(1),
                    "case {case:?}: storage is allocated in whole GiB, so a StorageSize \
                     range's Step is 1, got {range:?}"
                );
            }
            for (member, range) in [
                ("ProvisionedIops", &option.provisioned_iops),
                (
                    "ProvisionedStorageThroughput",
                    &option.provisioned_storage_throughput,
                ),
            ] {
                let Some(range) = range else { continue };
                assert!(
                    range.from <= range.to,
                    "case {case:?}: {member} runs backwards: {range:?}"
                );
                assert_eq!(
                    range.step,
                    Some(1),
                    "case {case:?}: {member} is provisioned in whole units, so its Step is \
                     1, got {range:?}"
                );
            }
            for (member, range) in [
                ("IopsToStorageRatio", &option.iops_to_storage_ratio),
                (
                    "StorageThroughputToIopsRatio",
                    &option.storage_throughput_to_iops_ratio,
                ),
            ] {
                let Some(range) = range else { continue };
                assert!(
                    range.from <= range.to,
                    "case {case:?}: {member} runs backwards: {range:?}"
                );
            }
            // A ratio is reported exactly where the bound it is a ratio of is.
            assert_eq!(
                option.iops_to_storage_ratio.is_some(),
                option.provisioned_iops.is_some(),
                "case {case:?}: IopsToStorageRatio and ProvisionedIops disagree about \
                 whether this type takes provisioned IOPS: {option:?}"
            );
            assert_eq!(
                option.storage_throughput_to_iops_ratio.is_some(),
                option.provisioned_storage_throughput.is_some(),
                "case {case:?}: StorageThroughputToIopsRatio and \
                 ProvisionedStorageThroughput disagree about whether this type takes \
                 provisioned throughput: {option:?}"
            );
            // Autoscaling raises `AllocatedStorage`, so it is offered exactly where the
            // instance has a storage size of its own to raise.
            assert_eq!(
                option.supports_storage_autoscaling.as_deref(),
                Some(if option.storage_size.is_empty() {
                    "false"
                } else {
                    "true"
                }),
                "case {case:?}: SupportsStorageAutoscaling disagrees with StorageSize: \
                 {option:?}"
            );
            match row.max_storage_size {
                Some(cap) => {
                    assert_eq!(
                        option.storage_size.iter().map(|range| range.to).max(),
                        Some(cap),
                        "case {case:?}: expected the ranges to reach the engine's storage \
                         cap {cap} GiB, got {:?}",
                        option.storage_size
                    );
                    // Keeping the allocation the instance already has is one valid
                    // modification, and growing it is another — unless this type does not
                    // take that allocation at all, which is the case for a type whose own
                    // minimum is above it.
                    let takes_current = option
                        .storage_size
                        .iter()
                        .any(|range| range.from <= ALLOCATED_STORAGE);
                    if takes_current {
                        assert!(
                            option.storage_size.iter().any(|range| range.from
                                <= ALLOCATED_STORAGE
                                && ALLOCATED_STORAGE <= range.to),
                            "case {case:?}: no range lets the instance keep the \
                             {ALLOCATED_STORAGE} GiB it runs: {:?}",
                            option.storage_size
                        );
                    }
                    assert!(
                        option
                            .storage_size
                            .iter()
                            .any(|range| range.to > ALLOCATED_STORAGE),
                        "case {case:?}: no range reaches above the {ALLOCATED_STORAGE} GiB \
                         the instance runs, so it reports no room to grow: {:?}",
                        option.storage_size
                    );
                }
                None => assert!(
                    option.storage_size.is_empty(),
                    "case {case:?}: this engine's storage is managed by its cluster, so \
                     the report carries no size range: {:?}",
                    option.storage_size
                ),
            }
        }
    }
}

/// One row of the storage-range table: an instance's engine and current allocation, and
/// the ranges the report has to carry for each storage type.
#[derive(Debug, Clone, Copy)]
struct RangeRow {
    engine: &'static str,
    /// The storage the instance is created with, or `None` for an instance created
    /// without an `AllocatedStorage` of its own.
    allocated: Option<i64>,
    /// `(StorageType, [(From, To), ..])`, in reported order.
    expected: &'static [(&'static str, &'static [(i64, i64)])],
}

/// How each storage type's size range is CUT for instances whose engine and current
/// allocation differ.
///
/// `StorageSize` is a list because a modification has two valid shapes, and they are not
/// contiguous: keeping the allocation the instance already has (one value), and growing
/// it, which starts at the minimum increase `ModifyDBInstanceMessage$AllocatedStorage`
/// documents — "For RDS for Db2, MariaDB, RDS for MySQL, RDS for Oracle, and RDS for
/// PostgreSQL, the value supplied must be at least 10% greater than the current value.
/// Values that are not at least 10% greater than the existing value are rounded up so that
/// they are 10% greater than the current value." — and runs to the type's maximum, capped
/// by the engine's own.
///
/// Every row runs on `db.m5.large`, one standard class, so the engine and the allocation
/// are the only things that vary: the engines the minimum increase names and one it does
/// not, an allocation below a type's own minimum, an allocation at the engine's ceiling,
/// the three storage caps, and an instance with no allocation of its own.
const RANGE_ROWS: &[RangeRow] = &[
    RangeRow {
        engine: "mysql",
        allocated: Some(20),
        expected: &[
            // 20 GiB stays valid, and a tenth more is 22.
            ("gp2", &[(20, 20), (22, 65_536)]),
            ("gp3", &[(20, 20), (22, 65_536)]),
            // Converting to Provisioned IOPS SSD cannot allocate less than that type
            // takes, so its own 100 GiB minimum is where its one range starts: 20 GiB is
            // not a size this type carries at all.
            ("io1", &[(100, 65_536)]),
            ("io2", &[(100, 65_536)]),
        ],
    },
    RangeRow {
        engine: "postgres",
        allocated: Some(1_000),
        expected: &[
            ("gp2", &[(1_000, 1_000), (1_100, 65_536)]),
            ("gp3", &[(1_000, 1_000), (1_100, 65_536)]),
            ("io1", &[(1_000, 1_000), (1_100, 65_536)]),
            ("io2", &[(1_000, 1_000), (1_100, 65_536)]),
        ],
    },
    RangeRow {
        engine: "mariadb",
        // A tenth of 99 GiB is 9.9, which the documented rounding takes up to the whole
        // GiB: 109, not 108. And 99 GiB is below the Provisioned IOPS SSD minimum, so
        // those types report the one range from 109 — their own minimum raised to the
        // increase.
        allocated: Some(99),
        expected: &[
            ("gp2", &[(99, 99), (109, 65_536)]),
            ("gp3", &[(99, 99), (109, 65_536)]),
            ("io1", &[(109, 65_536)]),
            ("io2", &[(109, 65_536)]),
        ],
    },
    RangeRow {
        // An instance at the engine's ceiling: it can keep what it has and there is
        // nothing above it to grow into.
        engine: "oracle-se2",
        allocated: Some(65_536),
        expected: &[
            ("gp2", &[(65_536, 65_536)]),
            ("gp3", &[(65_536, 65_536)]),
            ("io1", &[(65_536, 65_536)]),
            ("io2", &[(65_536, 65_536)]),
        ],
    },
    RangeRow {
        // Db2 caps allocated storage at its own value, so every growth range stops there.
        engine: "db2-se",
        allocated: Some(20),
        expected: &[
            ("gp2", &[(20, 20), (22, 64_000)]),
            ("gp3", &[(20, 20), (22, 64_000)]),
            ("io1", &[(100, 64_000)]),
            ("io2", &[(100, 64_000)]),
        ],
    },
    RangeRow {
        // An engine the documented minimum increase does NOT name, capped below what the
        // storage types themselves reach: it grows by the GiB storage is allocated in.
        engine: "sqlserver-ee",
        allocated: Some(1_000),
        expected: &[
            ("gp2", &[(1_000, 1_000), (1_001, 16_384)]),
            ("gp3", &[(1_000, 1_000), (1_001, 16_384)]),
            ("io1", &[(1_000, 1_000), (1_001, 16_384)]),
            ("io2", &[(1_000, 1_000), (1_001, 16_384)]),
        ],
    },
    RangeRow {
        // An instance carrying no allocated storage of its own has nothing to keep and
        // nothing to grow from, so each type reports its own published range.
        engine: "mysql",
        allocated: None,
        expected: &[
            ("gp2", &[(20, 65_536)]),
            ("gp3", &[(20, 65_536)]),
            ("io1", &[(100, 65_536)]),
            ("io2", &[(100, 65_536)]),
        ],
    },
];

#[tokio::test]
async fn the_storage_ranges_are_cut_by_what_the_instance_runs_now() {
    let router = emulator();

    for (index, row) in RANGE_ROWS.iter().enumerate() {
        let identifier = format!("emu-vdbim-range-{index}");
        let case = format!("{} allocated {:?} GiB", row.engine, row.allocated);
        let extra: Vec<(&str, String)> = row
            .allocated
            .map(|allocated| vec![("AllocatedStorage", allocated.to_string())])
            .unwrap_or_default();
        create(&router, &identifier, row.engine, "db.m5.large", &extra).await;

        let report = describe(&router, &case, &identifier).await;
        let actual: Vec<(String, Vec<(i64, i64)>)> = report
            .storage
            .iter()
            .map(|(storage_type, option)| {
                (
                    storage_type.clone(),
                    option
                        .storage_size
                        .iter()
                        .map(|range| (range.from, range.to))
                        .collect(),
                )
            })
            .collect();
        let expected: Vec<(String, Vec<(i64, i64)>)> = row
            .expected
            .iter()
            .map(|(storage_type, ranges)| ((*storage_type).to_string(), ranges.to_vec()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the storage ranges {expected:?}, got {actual:?}"
        );
    }
}

/// One modification attempt and what the bounds this report publishes make of it.
#[derive(Debug, Clone, Copy)]
struct ModifyRow {
    case: &'static str,
    /// The engine and the storage the instance starts on.
    engine: &'static str,
    storage_type: &'static str,
    allocated: i64,
    /// The members the modification supplies.
    params: &'static [(&'static str, &'static str)],
    /// `Ok(the AllocatedStorage the instance ends up with)`, or `Err((status, wire error
    /// code))` for a modification the bounds exclude.
    outcome: Result<i64, (u16, &'static str)>,
}

/// What `ModifyDBInstance` does with values this report's bounds admit and exclude.
///
/// "You can use this information when you call `ModifyDBInstance`" is only true if the two
/// agree, so every row here is a value the report's own ranges place: inside them it is
/// accepted and stored, outside them it is refused with a modelled client error. A size
/// between the current allocation and the minimum increase lies in NEITHER reported range,
/// and the recording is what settles it: the behavioural unit this operation was filed
/// from, `rds.valid-db-instance-modifications-bound-what-modify-accepts`, modifies a 20 GiB
/// `gp3` RDS for PostgreSQL instance to 21 GiB and records real AWS rejecting that call, so
/// such a size is refused here too, whatever `ModifyDBInstanceMessage$AllocatedStorage`'s
/// prose about rounding says (see `growth_floor`).
const MODIFY_ROWS: &[ModifyRow] = &[
    ModifyRow {
        case: "a reduction is refused: storage only grows",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("AllocatedStorage", "50")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "a size past the engine's cap is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("AllocatedStorage", "65537")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        // The recorded case: real AWS rejects a growth below the minimum increase rather
        // than rounding it up.
        case: "a growth smaller than the minimum increase is refused, as recorded",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("AllocatedStorage", "105")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "the allocation the instance already has is accepted",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("AllocatedStorage", "100")],
        outcome: Ok(100),
    },
    ModifyRow {
        case: "a growth at the documented minimum increase is accepted",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("AllocatedStorage", "110")],
        outcome: Ok(110),
    },
    ModifyRow {
        case: "a growth inside the reported range is stored as asked",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("AllocatedStorage", "500")],
        outcome: Ok(500),
    },
    ModifyRow {
        case: "provisioned IOPS on a type that reports none is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("Iops", "3000")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "provisioned IOPS below the type's reported range is refused",
        engine: "postgres",
        storage_type: "gp3",
        allocated: 100,
        params: &[("Iops", "2000")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "provisioned IOPS inside the type's reported range is accepted",
        engine: "postgres",
        storage_type: "gp3",
        allocated: 100,
        params: &[("Iops", "4000")],
        outcome: Ok(100),
    },
    ModifyRow {
        case: "provisioned IOPS past the type's reported storage ratio is refused",
        engine: "postgres",
        storage_type: "io1",
        allocated: 100,
        params: &[("Iops", "100000")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "provisioned throughput on a type that reports none is refused",
        engine: "postgres",
        storage_type: "io1",
        allocated: 200,
        params: &[("StorageThroughput", "500")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "provisioned throughput inside the type's reported range is accepted",
        engine: "postgres",
        storage_type: "gp3",
        allocated: 100,
        params: &[("StorageThroughput", "500")],
        outcome: Ok(100),
    },
    ModifyRow {
        case: "a storage type the engine publishes no value for is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        // Aurora storage, which only an Amazon Aurora DB instance carries: "This setting
        // doesn't apply to Amazon Aurora DB instances. Storage is managed by the DB
        // cluster." (`CreateDBInstanceMessage$StorageType`), and it is not in the value
        // list a per-instance-storage engine publishes.
        params: &[("StorageType", "aurora")],
        outcome: Err((400, "StorageTypeNotSupported")),
    },
    ModifyRow {
        // The legacy magnetic type is in the value list `StorageType` publishes
        // ("gp2 | gp3 | io1 | io2 | standard"), so it is not an unsupported type — this
        // service orders no option for it, and it carries its own published bounds.
        case: "the legacy magnetic storage type the member publishes is accepted",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("StorageType", "standard")],
        outcome: Ok(100),
    },
    ModifyRow {
        // Magnetic storage runs to 3,072 GiB (3 TiB), so a growth past it is refused even
        // though the engine's own cap is higher.
        case: "a size past the magnetic maximum is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("StorageType", "standard"), ("AllocatedStorage", "4000")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        // A magnetic volume's performance is not configurable: it takes no provisioned
        // IOPS, which is the bound this report would carry for it.
        case: "provisioned IOPS on magnetic storage is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("StorageType", "standard"), ("Iops", "3000")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "provisioned throughput on magnetic storage is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("StorageType", "standard"), ("StorageThroughput", "500")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "a growth inside the magnetic range is accepted",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("StorageType", "standard"), ("AllocatedStorage", "1000")],
        outcome: Ok(1_000),
    },
    ModifyRow {
        case: "a size below the minimum of the type being converted to is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 20,
        params: &[("StorageType", "io1"), ("AllocatedStorage", "50")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        // An engine whose storage its CLUSTER manages reports no StorageSize at all, so
        // there is no size for a modification of the INSTANCE to set.
        case: "a size on cluster-managed storage is refused",
        engine: "docdb",
        storage_type: "standard",
        allocated: 100,
        params: &[("AllocatedStorage", "200")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        // `MaxAllocatedStorage` is the ceiling Amazon RDS scales storage to, so it cannot
        // sit below the storage the instance already has.
        case: "an autoscaling ceiling below the current allocation is refused",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("MaxAllocatedStorage", "50")],
        outcome: Err((400, "InvalidParameterCombination")),
    },
    ModifyRow {
        case: "an autoscaling ceiling above the current allocation is accepted",
        engine: "postgres",
        storage_type: "gp2",
        allocated: 100,
        params: &[("MaxAllocatedStorage", "1000")],
        outcome: Ok(100),
    },
];

#[tokio::test]
async fn the_reported_bounds_are_the_ones_modify_enforces() {
    let router = emulator();

    // Every row's instance is created first and then waited for: a modification is
    // refused on an instance that is still coming up, whatever it asks for.
    for (index, row) in MODIFY_ROWS.iter().enumerate() {
        create(
            &router,
            &format!("emu-vdbim-mod-{index}"),
            row.engine,
            "db.m5.large",
            &[
                ("StorageType", row.storage_type.to_string()),
                ("AllocatedStorage", row.allocated.to_string()),
            ],
        )
        .await;
    }
    wait_for_settle();

    for (index, row) in MODIFY_ROWS.iter().enumerate() {
        let identifier = format!("emu-vdbim-mod-{index}");
        let case = row.case;

        let mut params: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", identifier.clone()),
            ("ApplyImmediately", "true".to_string()),
        ];
        params.extend(
            row.params
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        let response = call(&router, "ModifyDBInstance", &params).await;
        match row.outcome {
            Ok(allocated) => {
                expect_ok(case, response);
                // Read the instance back: what a modification the bounds admit stores is
                // what a later caller sees.
                let body = expect_ok(
                    case,
                    call(
                        &router,
                        "DescribeDBInstances",
                        &[("DBInstanceIdentifier", identifier.clone())],
                    )
                    .await,
                );
                let instance = subtrees(&body, "DBInstance")
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("case {case:?}: no DBInstance in {body}"));
                assert_eq!(
                    text(&instance, "AllocatedStorage"),
                    allocated.to_string(),
                    "case {case:?}: expected {allocated} GiB allocated after the \
                     modification, got {instance}"
                );
            }
            Err((status, code)) => {
                expect_error(case, response, status, code);
                // A refused modification leaves the instance exactly as it was.
                let body = expect_ok(
                    case,
                    call(
                        &router,
                        "DescribeDBInstances",
                        &[("DBInstanceIdentifier", identifier.clone())],
                    )
                    .await,
                );
                let instance = subtrees(&body, "DBInstance")
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("case {case:?}: no DBInstance in {body}"));
                assert_eq!(
                    text(&instance, "AllocatedStorage"),
                    row.allocated.to_string(),
                    "case {case:?}: the refused modification changed the stored \
                     allocation: {instance}"
                );
                assert_eq!(
                    text(&instance, "StorageType"),
                    row.storage_type,
                    "case {case:?}: the refused modification changed the stored storage \
                     type: {instance}"
                );
            }
        }
    }
}

/// One processor-feature modification and what `ValidProcessorFeatures` makes of it.
#[derive(Debug, Clone, Copy)]
struct FeatureRow {
    case: &'static str,
    engine: &'static str,
    class: &'static str,
    /// The `(Name, Value)` the modification supplies.
    feature: (&'static str, &'static str),
    /// Whether the class's reported features exclude it.
    refused: bool,
}

/// What `ModifyDBInstance` does with the processor features this report admits and
/// excludes.
///
/// `ValidProcessorFeatures` is "Valid processor features for your DB instance", so a
/// feature it does not carry is one the modification cannot set: configuring cores is
/// published for the core-licensed engines (RDS for Oracle, RDS for Microsoft SQL Server)
/// on the multithreaded Intel classes, under the two published names
/// (`AvailableProcessorFeature$Name`: "Valid names are `coreCount` and `threadsPerCore`")
/// and only at the values the class allows. The rows span both names, a value outside the
/// allowed list, a name outside the published two, an engine that configures none, a
/// burstable class that configures none, and a class the published family table does not
/// carry — which is bounded by nothing, because that table is an explicitly partial subset
/// of AWS's matrix.
const FEATURE_ROWS: &[FeatureRow] = &[
    FeatureRow {
        case: "a core count the class allows",
        engine: "oracle-se2",
        class: "db.m5.2xlarge",
        feature: ("coreCount", "4"),
        refused: false,
    },
    FeatureRow {
        case: "a core count the class does not allow: cores are disabled in pairs",
        engine: "oracle-se2",
        class: "db.m5.2xlarge",
        feature: ("coreCount", "3"),
        refused: true,
    },
    FeatureRow {
        case: "a thread count the class allows",
        engine: "oracle-se2",
        class: "db.m5.2xlarge",
        feature: ("threadsPerCore", "1"),
        refused: false,
    },
    FeatureRow {
        case: "a feature name RDS does not publish",
        engine: "oracle-se2",
        class: "db.m5.2xlarge",
        feature: ("turboBoost", "1"),
        refused: true,
    },
    FeatureRow {
        case: "an engine whose classes configure no processor",
        engine: "postgres",
        class: "db.m5.2xlarge",
        feature: ("coreCount", "4"),
        refused: true,
    },
    FeatureRow {
        case: "a burstable class, whose vCPUs are shared",
        engine: "sqlserver-ee",
        class: "db.t3.large",
        feature: ("coreCount", "1"),
        refused: true,
    },
    FeatureRow {
        case: "a class the published family table does not carry",
        engine: "oracle-se2",
        class: "db.m5.24xlarge",
        feature: ("coreCount", "24"),
        refused: false,
    },
];

#[tokio::test]
async fn the_reported_processor_features_are_the_ones_modify_enforces() {
    let router = emulator();

    for (index, row) in FEATURE_ROWS.iter().enumerate() {
        create(
            &router,
            &format!("emu-vdbim-feat-{index}"),
            row.engine,
            row.class,
            &[("AllocatedStorage", "100".to_string())],
        )
        .await;
    }
    wait_for_settle();

    for (index, row) in FEATURE_ROWS.iter().enumerate() {
        let identifier = format!("emu-vdbim-feat-{index}");
        let case = row.case;
        let (name, value) = row.feature;
        // Where the report names features at all, it has to agree with the refusal: a
        // feature modify takes is one the report carries at that value, and one it refuses
        // is one the report does not. A class the report names NO feature for is either one
        // that configures none (refused) or one outside the published family table (bounded
        // by nothing), which the report alone cannot tell apart.
        let reported = describe(&router, case, &identifier).await;
        let carried = reported
            .valid_processor_features
            .iter()
            .any(|(feature, _, allowed)| {
                feature == name && allowed.split(',').any(|candidate| candidate == value)
            });
        if !reported.valid_processor_features.is_empty() {
            assert_eq!(
                carried, !row.refused,
                "case {case:?}: the report and the modification disagree about \
                 {name}={value}: {:?}",
                reported.valid_processor_features
            );
        }

        let response = call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", identifier.clone()),
                ("ApplyImmediately", "true".to_string()),
                ("ProcessorFeatures.ProcessorFeature.1.Name", name.to_string()),
                (
                    "ProcessorFeatures.ProcessorFeature.1.Value",
                    value.to_string(),
                ),
            ],
        )
        .await;
        if row.refused {
            expect_error(case, response, 400, "InvalidParameterCombination");
        } else {
            expect_ok(case, response);
        }
    }
}

#[tokio::test]
async fn an_instance_is_addressed_by_identifier_or_arn_in_its_own_region() {
    let router = emulator();
    create(
        &router,
        "emu-vdbim-named",
        "mysql",
        "db.m5.large",
        &[("AllocatedStorage", "100".to_string())],
    )
    .await;
    let by_identifier = describe(&router, "by identifier", "emu-vdbim-named").await;

    // "The customer identifier or the ARN of your DB instance" — both name the one
    // instance, so both report the same bounds, and an identifier is matched folded
    // because it is "stored as a lowercase string".
    for (case, named) in [
        (
            "by ARN",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-vdbim-named"),
        ),
        ("by upper-case identifier", "EMU-VDBIM-NAMED".to_string()),
    ] {
        let report = describe(&router, case, &named).await;
        assert_eq!(
            report.xml, by_identifier.xml,
            "case {case:?}: {named} reports bounds the identifier does not"
        );
    }

    // RDS is regional: neither another region's request nor another region's ARN finds it,
    // and neither does an identifier no instance carries.
    for (case, region, named) in [
        (
            "an identifier no instance carries",
            REGION,
            "emu-vdbim-absent".to_string(),
        ),
        (
            "the instance's identifier in another region",
            "eu-west-1",
            "emu-vdbim-named".to_string(),
        ),
        (
            "an ARN of another region",
            REGION,
            format!("arn:aws:rds:eu-west-1:{ACCOUNT}:db:emu-vdbim-named"),
        ),
    ] {
        expect_error(
            case,
            call_in(
                &router,
                region,
                ACTION,
                &[("DBInstanceIdentifier", named.clone())],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
    }
}

#[tokio::test]
async fn an_instance_on_its_way_out_has_no_valid_modifications() {
    let router = emulator();
    create(&router, "emu-vdbim-doomed", "mysql", "db.m5.large", &[]).await;
    // An instance that is still COMING UP does have valid modifications: its engine and
    // class already fix them.
    let creating = describe(&router, "still creating", "emu-vdbim-doomed").await;
    assert!(
        !creating.storage.is_empty(),
        "case \"still creating\": a creating instance reports no storage options: {}",
        creating.xml
    );

    expect_ok(
        "delete",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-vdbim-doomed".to_string()),
                ("SkipFinalSnapshot", "true".to_string()),
            ],
        )
        .await,
    );
    expect_error(
        "deleting",
        call(
            &router,
            ACTION,
            &[("DBInstanceIdentifier", "emu-vdbim-doomed".to_string())],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
}

#[tokio::test]
async fn the_report_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    create(
        &router,
        "emu-vdbim-read",
        "postgres",
        "db.m5.large",
        &[("AllocatedStorage", "100".to_string())],
    )
    .await;
    let before = describe_instance(&router, "before").await;

    let first = describe(&router, "first read", "emu-vdbim-read").await;
    let second = describe(&router, "second read", "emu-vdbim-read").await;
    assert_eq!(
        first.xml, second.xml,
        "case \"second read\": the report is not the same twice running"
    );

    let after = describe_instance(&router, "after").await;
    assert_eq!(
        before, after,
        "case \"after\": reporting valid modifications changed the instance"
    );
}

/// The one `<DBInstance>` `DescribeDBInstances` reports for the instance under test, with
/// the members that settle on their own normalised away: a read must leave every OTHER
/// member exactly as it was.
async fn describe_instance(router: &MultiServiceRouter, case: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-vdbim-read".to_string())],
        )
        .await,
    );
    let instance = subtrees(&body, "DBInstance")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no DBInstance in {body}"));
    // `DBInstanceStatus` moves from `creating` to `available` on its own, which is the
    // clock's doing and not this report's.
    let status = text(&instance, "DBInstanceStatus");
    instance.replace(&status, "<settled>")
}
