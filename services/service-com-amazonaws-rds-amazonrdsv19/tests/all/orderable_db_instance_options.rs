// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeOrderableDBInstanceOptions`, the report a caller reads
//! to choose what to create a DB instance as.
//!
//! Every call goes over the wire through the same router the production emulator mounts
//! (`crate::db_instances::emulator`), as a real awsQuery form POST.
//!
//! The rules are stated over TABLES and against the service's OWN other report: the
//! engines are rows (including ones no issue cited), the options offered for an engine are
//! cross-checked against the versions `DescribeDBEngineVersions` reports as available for
//! it, every documented filter is a row, and each published `MaxRecords` bound is checked
//! with one value past it.
//!
//! An option is one (engine version, DB instance class, license model, STORAGE TYPE)
//! combination, so the storage tables below span the storage types a class is offered
//! with and the members each type carries, over classes and engines the issue that
//! prompted them did not cite.

use std::collections::{BTreeMap, BTreeSet};

use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::versions;
use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts, REGION,
};

/// The operation under test.
const ACTION: &str = "DescribeOrderableDBInstanceOptions";

/// The engines this suite spans: one per family `DescribeDBEngineVersions` reports, so
/// every branch of the per-engine rules (Aurora versus provisioned storage, the license
/// models, the storage bounds) is a row.
const ENGINES: [&str; 7] = [
    "aurora-mysql",
    "aurora-postgresql",
    "db2-se",
    "mariadb",
    "mysql",
    "oracle-se2",
    "sqlserver-ee",
];

/// One reported `<OrderableDBInstanceOption>`, as the members this suite asserts on.
#[derive(Debug, Clone)]
struct OrderableOption {
    engine: String,
    engine_version: String,
    db_instance_class: String,
    license_model: String,
    storage_type: String,
    xml: String,
}

/// Every `<OrderableDBInstanceOption>` in a response body, in reported order.
fn options(xml: &str) -> Vec<OrderableOption> {
    subtrees(xml, "OrderableDBInstanceOption")
        .into_iter()
        .map(|entry| OrderableOption {
            engine: text(&entry, "Engine"),
            engine_version: text(&entry, "EngineVersion"),
            db_instance_class: text(&entry, "DBInstanceClass"),
            license_model: text(&entry, "LicenseModel"),
            storage_type: text(&entry, "StorageType"),
            xml: entry,
        })
        .collect()
}

/// `DescribeOrderableDBInstanceOptions` with `params`, asserted to answer 200.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&str, String)],
) -> (Vec<OrderableOption>, Option<String>) {
    let body = expect_ok(case, call(router, ACTION, params).await);
    (options(&body), optional_text(&body, "Marker"))
}

/// Every option a request carrying `params` reports, paged to exhaustion so the
/// assertions see the whole report rather than its first page.
async fn all_options(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&'static str, String)],
) -> Vec<OrderableOption> {
    let mut collected: Vec<OrderableOption> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut params: Vec<(&str, String)> = params.to_vec();
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let (page, next) = describe(router, case, &params).await;
        collected.extend(page);
        match next {
            Some(next) => marker = Some(next),
            None => break,
        }
        assert!(
            collected.len() < 100_000,
            "case {case:?}: paging did not terminate"
        );
    }
    collected
}

/// Every option offered for `engine`.
async fn all_options_for(router: &MultiServiceRouter, engine: &str) -> Vec<OrderableOption> {
    all_options(
        router,
        &format!("options for {engine}"),
        &[("Engine", engine.to_string())],
    )
    .await
}

/// The engine versions `DescribeDBEngineVersions` reports as AVAILABLE for `engine`.
async fn available_versions(router: &MultiServiceRouter, engine: &str) -> BTreeSet<String> {
    let body = expect_ok(
        &format!("available versions of {engine}"),
        call(
            router,
            "DescribeDBEngineVersions",
            &[("Engine", engine.to_string())],
        )
        .await,
    );
    versions(&body)
        .into_iter()
        .map(|version| version.engine_version)
        .collect()
}

#[tokio::test]
async fn an_engines_options_are_its_available_versions_crossed_with_its_classes() {
    let router = emulator();

    for engine in ENGINES {
        let offered = all_options_for(&router, engine).await;
        assert!(
            !offered.is_empty(),
            "case {engine:?}: an engine this service reports versions for must have \
             orderable options"
        );

        // Every option is for the engine asked for, and for a version this service
        // reports as available — never a deprecated one, and never a version the engine
        // catalogue does not carry.
        let available = available_versions(&router, engine).await;
        let offered_versions: BTreeSet<String> = offered
            .iter()
            .map(|option| option.engine_version.clone())
            .collect();
        assert_eq!(
            offered_versions, available,
            "case {engine:?}: the versions options are offered for must be exactly the \
             engine's available versions"
        );
        for option in &offered {
            assert_eq!(
                option.engine, engine,
                "case {engine:?}: Engine narrows the report, got {}",
                option.xml
            );
            // A caller reads these to decide what to create, so each must be present and
            // self-consistent.
            assert!(
                !option.db_instance_class.is_empty() && !option.license_model.is_empty(),
                "case {engine:?}: an option must name a class and a license model, got {}",
                option.xml
            );
            assert_eq!(
                text(&option.xml, "Vpc"),
                "true",
                "case {engine:?}: every option this service offers is a VPC offering, \
                 got {}",
                option.xml
            );
            let zones = subtrees(&option.xml, "AvailabilityZones");
            assert_eq!(
                zones.len(),
                1,
                "case {engine:?}: the zones are reported once, got {}",
                option.xml
            );
            assert_eq!(
                texts(&zones[0], "Name"),
                vec![
                    format!("{REGION}a"),
                    format!("{REGION}b"),
                    format!("{REGION}c")
                ],
                "case {engine:?}: an option's zones are the zones of the region the \
                 request was signed for, got {}",
                option.xml
            );
            // Aurora storage is cluster-managed, so an Aurora option reports no storage
            // bounds; a provisioned engine's option reports both.
            let is_aurora = engine.starts_with("aurora");
            assert_eq!(
                optional_text(&option.xml, "MinStorageSize").is_none(),
                is_aurora,
                "case {engine:?}: storage bounds must be reported exactly for an engine \
                 whose storage is ordered, got {}",
                option.xml
            );
            if !is_aurora {
                let min: i32 = text(&option.xml, "MinStorageSize")
                    .parse()
                    .expect("MinStorageSize is an integer");
                let max: i32 = text(&option.xml, "MaxStorageSize")
                    .parse()
                    .expect("MaxStorageSize is an integer");
                assert!(
                    min > 0 && min < max,
                    "case {engine:?}: the storage bounds must be a non-empty range, got \
                     {min}..{max} in {}",
                    option.xml
                );
            }
        }

        // One option per (version, class, license model, storage type): no duplicate
        // rows, and a row for every storage type the class can be ordered with.
        let combinations: BTreeSet<(String, String, String, String)> = offered
            .iter()
            .map(|option| {
                (
                    option.engine_version.clone(),
                    option.db_instance_class.clone(),
                    option.license_model.clone(),
                    option.storage_type.clone(),
                )
            })
            .collect();
        assert_eq!(
            combinations.len(),
            offered.len(),
            "case {engine:?}: an option must be offered once per version, class, license \
             model and storage type, got {} rows for {} combinations",
            offered.len(),
            combinations.len()
        );
    }
}

#[tokio::test]
async fn an_options_zones_follow_the_region_the_request_is_signed_for() {
    let router = emulator();

    // RDS is regional, so the zones an option is offered in are the signed region's.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let case = format!("options in {region}");
        let body = expect_ok(
            &case,
            call_in(
                &router,
                region,
                ACTION,
                &[
                    ("Engine", "mysql".to_string()),
                    ("DBInstanceClass", "db.t3.micro".to_string()),
                ],
            )
            .await,
        );
        let offered = options(&body);
        assert!(
            !offered.is_empty(),
            "case {case:?}: the class is one this service offers, so it must be reported"
        );
        for option in &offered {
            let zones = subtrees(&option.xml, "AvailabilityZones");
            assert_eq!(
                texts(&zones[0], "Name"),
                vec![
                    format!("{region}a"),
                    format!("{region}b"),
                    format!("{region}c")
                ],
                "case {case:?}: got {}",
                option.xml
            );
        }
    }
}

#[tokio::test]
async fn each_documented_filter_narrows_the_options() {
    let router = emulator();

    let all = all_options_for(&router, "mysql").await;
    assert!(!all.is_empty(), "there must be options to filter");
    let a_version = all[0].engine_version.clone();
    let a_class = all[0].db_instance_class.clone();
    let a_license = all[0].license_model.clone();

    // One row per documented filter member, each with a value taken from the report
    // itself and the member of an option it must agree with.
    type Row = (&'static str, String, fn(&OrderableOption) -> String);
    let rows: [Row; 3] = [
        ("EngineVersion", a_version, |option| {
            option.engine_version.clone()
        }),
        ("DBInstanceClass", a_class, |option| {
            option.db_instance_class.clone()
        }),
        ("LicenseModel", a_license, |option| {
            option.license_model.clone()
        }),
    ];
    for (member, value, member_of) in rows {
        let case = format!("{member}={value}");
        let offered = all_options(
            &router,
            &case,
            &[("Engine", "mysql".to_string()), (member, value.clone())],
        )
        .await;
        let expected = all
            .iter()
            .filter(|option| member_of(option) == value)
            .count();
        assert_eq!(
            offered.len(),
            expected,
            "case {case:?}: expected the {expected} matching options of the full report"
        );
        for option in &offered {
            assert_eq!(
                member_of(option),
                value,
                "case {case:?}: every option must match the filter, got {}",
                option.xml
            );
        }
    }

    // A value that names nothing selects nothing — the operation models no error for it.
    for (case, member, value) in [
        ("a version this engine does not have", "EngineVersion", "0.0.0"),
        ("a class this service does not offer", "DBInstanceClass", "db.nonexistent"),
        ("a license model this engine is not offered under", "LicenseModel", "no-license"),
    ] {
        let (offered, marker) = describe(
            &router,
            case,
            &[("Engine", "mysql".to_string()), (member, value.to_string())],
        )
        .await;
        assert!(
            offered.is_empty() && marker.is_none(),
            "case {case:?}: expected an empty report, got {} options",
            offered.len()
        );
    }

    // "Specifies whether to show only VPC or non-VPC offerings": every offering here is
    // a VPC offering, so only-non-VPC selects none.
    for (case, vpc, expect_any) in [
        ("Vpc=true", "true", true),
        ("Vpc=false", "false", false),
    ] {
        let (offered, _) = describe(
            &router,
            case,
            &[("Engine", "mysql".to_string()), ("Vpc", vpc.to_string())],
        )
        .await;
        assert_eq!(
            !offered.is_empty(),
            expect_any,
            "case {case:?}: got {} options",
            offered.len()
        );
    }

    // `AvailabilityZoneGroup` asks for Local Zone options, which this service offers
    // none of.
    let (offered, _) = describe(
        &router,
        "AvailabilityZoneGroup",
        &[
            ("Engine", "mysql".to_string()),
            ("AvailabilityZoneGroup", "us-west-2-lax-1".to_string()),
        ],
    )
    .await;
    assert!(
        offered.is_empty(),
        "case \"AvailabilityZoneGroup\": this service offers no Local Zone options, got \
         {} ",
        offered.len()
    );

    // "Filters: This parameter isn't currently supported" — refused rather than ignored.
    expect_error(
        "a filter this operation does not support",
        call(
            &router,
            ACTION,
            &[
                ("Engine", "mysql".to_string()),
                ("Filters.Filter.1.Name", "engine".to_string()),
                ("Filters.Filter.1.Values.Value.1", "mysql".to_string()),
            ],
        )
        .await,
        400,
        "ValidationException",
    );
}

#[tokio::test]
async fn max_records_is_bounded_and_the_marker_resumes_the_report() {
    let router = emulator();

    // THIS operation's own bound: "Default: 100 Constraints: Minimum 20, maximum 1000" —
    // a different maximum from the other describe operations, so both published bounds
    // and one value past each are rows.
    for (case, value, expected) in [
        ("one below the published minimum", "19", 400),
        ("the published minimum", "20", 200),
        ("the published maximum", "1000", 200),
        ("one above the published maximum", "1001", 400),
    ] {
        let response = call(
            &router,
            ACTION,
            &[
                ("Engine", "mysql".to_string()),
                ("MaxRecords", value.to_string()),
            ],
        )
        .await;
        if expected == 200 {
            expect_ok(case, response);
        } else {
            expect_error(case, response, 400, "ValidationException");
        }
    }

    /// One option, as the paging walk identifies it: an option is one (version, class,
    /// license model, storage type) combination.
    type Row = (String, String, String, String);
    fn row(option: OrderableOption) -> Row {
        (
            option.engine_version,
            option.db_instance_class,
            option.license_model,
            option.storage_type,
        )
    }

    let expected: Vec<Row> = all_options_for(&router, "mysql")
        .await
        .into_iter()
        .map(row)
        .collect();

    let mut seen: Vec<Row> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    // A page holds at most 20 options, so the whole report takes at most this many
    // pages; one more than that means paging is not advancing.
    let page_budget = expected.len() / 20 + 2;
    loop {
        let mut params: Vec<(&str, String)> = vec![
            ("Engine", "mysql".to_string()),
            ("MaxRecords", "20".to_string()),
        ];
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let (page, next) = describe(&router, "paged options", &params).await;
        assert!(
            !page.is_empty() && page.len() <= 20,
            "case \"paged options\": a page must hold between one and MaxRecords \
             options, got {}",
            page.len()
        );
        seen.extend(page.into_iter().map(row));
        pages += 1;
        assert!(
            pages <= page_budget,
            "case \"paged options\": paging did not terminate after {pages} pages, \
             expected at most {page_budget}"
        );
        match next {
            Some(next) => marker = Some(next),
            None => break,
        }
    }
    assert_eq!(
        seen, expected,
        "case \"paged options\": paging must visit every option exactly once, in the \
         report's own order"
    );
    assert!(
        pages > 1,
        "case \"paged options\": a page size of 20 must take more than one page for \
         {} options",
        expected.len()
    );
}

/// What an option must report ABOUT its own storage type.
///
/// Source: the "Amazon RDS DB instance storage" tables in the Amazon RDS User Guide —
/// General Purpose SSD (`gp2`, `gp3`) and Provisioned IOPS SSD (`io1`, `io2`) — plus
/// Aurora, whose storage the cluster manages and which is therefore not ordered with the
/// instance at all.
struct StorageRule {
    /// `StorageType`.
    storage_type: &'static str,
    /// `MinStorageSize`, in GiB, or `None` when storage is not ordered per instance and
    /// the option reports no bounds.
    min_storage_size: Option<i32>,
    /// `SupportsIops`, and `MaxIopsPerGib` — the published IOPS-to-GiB ratio ceiling —
    /// when the type takes provisioned IOPS.
    max_iops_per_gib: Option<f64>,
    /// `SupportsStorageThroughput`, and `MinStorageThroughputPerDbInstance` in MiB/s when
    /// the type takes provisioned throughput.
    min_storage_throughput: Option<i32>,
}

/// The published storage types, one row each.
const STORAGE_RULES: &[StorageRule] = &[
    StorageRule {
        storage_type: "gp2",
        min_storage_size: Some(20),
        max_iops_per_gib: None,
        min_storage_throughput: None,
    },
    StorageRule {
        storage_type: "gp3",
        min_storage_size: Some(20),
        max_iops_per_gib: Some(500.0),
        min_storage_throughput: Some(125),
    },
    StorageRule {
        storage_type: "io1",
        min_storage_size: Some(100),
        max_iops_per_gib: Some(50.0),
        min_storage_throughput: None,
    },
    StorageRule {
        storage_type: "io2",
        min_storage_size: Some(100),
        max_iops_per_gib: Some(1000.0),
        min_storage_throughput: None,
    },
    StorageRule {
        storage_type: "aurora",
        min_storage_size: None,
        max_iops_per_gib: None,
        min_storage_throughput: None,
    },
    // The DocumentDB and Neptune models' `CreateDBCluster$StorageType`: "Valid values for
    // storage type - standard | iopt1 … Default value is standard" — carried by the
    // cluster, so the instance option reports no bounds either.
    StorageRule {
        storage_type: "standard",
        min_storage_size: None,
        max_iops_per_gib: None,
        min_storage_throughput: None,
    },
];

/// The rule for `storage_type`.
fn storage_rule(storage_type: &str) -> &'static StorageRule {
    STORAGE_RULES
        .iter()
        .find(|rule| rule.storage_type == storage_type)
        .unwrap_or_else(|| panic!("the report named a storage type RDS does not publish: {storage_type}"))
}

/// `<tag>` read as an integer, or `None` when the option carries none.
fn optional_int(xml: &str, tag: &str) -> Option<i32> {
    optional_text(xml, tag).map(|value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("<{tag}> must be an integer, got {value:?}"))
    })
}

/// `<tag>` read as a double, or `None` when the option carries none.
fn optional_double(xml: &str, tag: &str) -> Option<f64> {
    optional_text(xml, tag).map(|value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("<{tag}> must be a double, got {value:?}"))
    })
}

#[tokio::test]
async fn a_class_is_offered_once_per_storage_type_it_can_be_ordered_with() {
    let router = emulator();

    /// `(engine, DB instance class, the storage types the class is offered with, the
    /// engine's own largest AllocatedStorage in GiB)`.
    type Row = (
        &'static str,
        &'static str,
        &'static [&'static str],
        Option<i32>,
    );
    /// The General Purpose SSD types every class takes.
    const GENERAL_PURPOSE: &[&str] = &["gp2", "gp3"];
    /// Those plus the Provisioned IOPS SSD types a standard class takes.
    const EVERY_TYPE: &[&str] = &["gp2", "gp3", "io1", "io2"];
    // Rows differ along class family, class size, engine family and the engine's storage
    // cap; the classes are ones no issue cited as well as ones that were.
    const ROWS: &[Row] = &[
        // A burstable class takes General Purpose SSD only: Provisioned IOPS SSD starts
        // at 100 GiB and is ordered on the standard classes.
        ("mysql", "db.t4g.micro", GENERAL_PURPOSE, Some(65_536)),
        ("mysql", "db.t3.2xlarge", GENERAL_PURPOSE, Some(65_536)),
        // A standard class takes every published type.
        ("mysql", "db.m6gd.8xlarge", EVERY_TYPE, Some(65_536)),
        ("postgres", "db.x2iedn.32xlarge", EVERY_TYPE, Some(65_536)),
        ("mariadb", "db.r5d.4xlarge", EVERY_TYPE, Some(65_536)),
        // Each engine's own AllocatedStorage cap bounds every type's MaxStorageSize.
        ("sqlserver-ee", "db.r5d.8xlarge", EVERY_TYPE, Some(16_384)),
        ("db2-se", "db.c6gd.12xlarge", EVERY_TYPE, Some(64_000)),
        ("oracle-se2", "db.r5.large", EVERY_TYPE, Some(65_536)),
        // Aurora storage is managed by the cluster, so it is reported as one type with
        // no bounds at all.
        ("aurora-mysql", "db.serverless", &["aurora"], None),
        ("aurora-postgresql", "db.r6g.2xlarge", &["aurora"], None),
        // The DocumentDB and Neptune control planes this same front door serves are
        // cluster engines too, and report their own cluster-managed type.
        ("docdb", "db.r5.2xlarge", &["standard"], None),
        ("neptune", "db.serverless", &["standard"], None),
    ];

    for (engine, class, expected_types, max_storage_size) in ROWS {
        let case = format!("{engine} {class}");
        let (offered, _) = describe(
            &router,
            &case,
            &[
                ("Engine", engine.to_string()),
                ("DBInstanceClass", class.to_string()),
                ("MaxRecords", "1000".to_string()),
            ],
        )
        .await;
        assert!(
            !offered.is_empty(),
            "case {case:?}: the class is one this service offers, so it must be reported"
        );

        // One option per storage type, under every (engine version, license model) the
        // class is offered under — never one option carrying a single default type.
        let mut by_combination: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for option in &offered {
            assert_eq!(
                &option.db_instance_class, class,
                "case {case:?}: DBInstanceClass narrows the report, got {}",
                option.xml
            );
            by_combination
                .entry((option.engine_version.clone(), option.license_model.clone()))
                .or_default()
                .push(option.storage_type.clone());
        }
        for (combination, reported) in &by_combination {
            let reported: Vec<&str> = reported.iter().map(String::as_str).collect();
            assert_eq!(
                reported, *expected_types,
                "case {case:?}: {combination:?} must be offered once per storage type"
            );
        }

        for option in &offered {
            let rule = storage_rule(&option.storage_type);
            let reported_min = optional_int(&option.xml, "MinStorageSize");
            assert_eq!(
                reported_min, rule.min_storage_size,
                "case {case:?}: {} MinStorageSize, got {}",
                option.storage_type, option.xml
            );
            assert_eq!(
                optional_int(&option.xml, "MaxStorageSize"),
                rule.min_storage_size.and(*max_storage_size),
                "case {case:?}: {} MaxStorageSize is the engine's cap, got {}",
                option.storage_type,
                option.xml
            );
            // Storage autoscaling raises AllocatedStorage, so it is offered exactly where
            // storage is ordered with the instance.
            assert_eq!(
                text(&option.xml, "SupportsStorageAutoscaling"),
                reported_min.is_some().to_string(),
                "case {case:?}: {} SupportsStorageAutoscaling, got {}",
                option.storage_type,
                option.xml
            );
            // Provisioned IOPS.
            assert_eq!(
                text(&option.xml, "SupportsIops"),
                rule.max_iops_per_gib.is_some().to_string(),
                "case {case:?}: {} SupportsIops, got {}",
                option.storage_type,
                option.xml
            );
            assert_eq!(
                optional_double(&option.xml, "MaxIopsPerGib"),
                rule.max_iops_per_gib,
                "case {case:?}: {} MaxIopsPerGib, got {}",
                option.storage_type,
                option.xml
            );
            assert_eq!(
                optional_int(&option.xml, "MinIopsPerDbInstance").is_some(),
                rule.max_iops_per_gib.is_some(),
                "case {case:?}: {} reports its IOPS bounds exactly when it takes \
                 provisioned IOPS, got {}",
                option.storage_type,
                option.xml
            );
            // Provisioned throughput.
            assert_eq!(
                text(&option.xml, "SupportsStorageThroughput"),
                rule.min_storage_throughput.is_some().to_string(),
                "case {case:?}: {} SupportsStorageThroughput, got {}",
                option.storage_type,
                option.xml
            );
            assert_eq!(
                optional_int(&option.xml, "MinStorageThroughputPerDbInstance"),
                rule.min_storage_throughput,
                "case {case:?}: {} MinStorageThroughputPerDbInstance, got {}",
                option.storage_type,
                option.xml
            );
        }
    }
}

#[tokio::test]
async fn an_options_capabilities_follow_its_engine_and_its_class() {
    let router = emulator();

    /// One (engine, class) combination and the capabilities an option for it reports.
    struct CapabilityCase {
        engine: &'static str,
        db_instance_class: &'static str,
        /// `SupportedActivityStreamModes`, which the member's own documentation states:
        /// "Aurora PostgreSQL returns the value [sync, async]. Aurora MySQL and RDS for
        /// Oracle return [async] only. If Database Activity Streams isn't supported, the
        /// return value is an empty list."
        activity_stream_modes: &'static [&'static str],
        /// `AvailableProcessorFeatures` as the `coreCount` feature's
        /// `(DefaultValue, AllowedValues)`, or `None` for a class with no configurable
        /// processor feature: configuring cores is published for the core-licensed
        /// engines (Oracle, SQL Server) on multithreaded Intel classes only.
        core_count: Option<(&'static str, &'static str)>,
        /// `SupportsClusters` — a Multi-AZ DB cluster, offered on the MySQL and
        /// PostgreSQL engines and never on a burstable class.
        supports_clusters: bool,
        /// `SupportsDedicatedLogVolume` — a volume ordered with the instance, which
        /// Aurora's cluster-managed storage does not take.
        supports_dedicated_log_volume: bool,
    }

    const CASES: &[CapabilityCase] = &[
        CapabilityCase {
            engine: "aurora-postgresql",
            db_instance_class: "db.r5.xlarge",
            activity_stream_modes: &["sync", "async"],
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: false,
        },
        CapabilityCase {
            engine: "aurora-mysql",
            db_instance_class: "db.r5.xlarge",
            activity_stream_modes: &["async"],
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: false,
        },
        CapabilityCase {
            engine: "oracle-se2",
            db_instance_class: "db.r5.4xlarge",
            activity_stream_modes: &["async"],
            // 16 vCPUs at two threads per core: eight cores, disabled in pairs.
            core_count: Some(("8", "2,4,6,8")),
            supports_clusters: false,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "oracle-se2",
            db_instance_class: "db.r6g.4xlarge",
            activity_stream_modes: &["async"],
            // A Graviton core is not multithreaded and its count is not configurable.
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "oracle-se2",
            db_instance_class: "db.t3.large",
            activity_stream_modes: &["async"],
            // A burstable class shares its vCPUs.
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "sqlserver-se",
            db_instance_class: "db.m5.2xlarge",
            activity_stream_modes: &[],
            core_count: Some(("4", "2,4")),
            supports_clusters: false,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "mysql",
            db_instance_class: "db.m5.2xlarge",
            activity_stream_modes: &[],
            // Not a core-licensed engine: no processor feature to configure.
            core_count: None,
            supports_clusters: true,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "mysql",
            db_instance_class: "db.t3.medium",
            activity_stream_modes: &[],
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "postgres",
            db_instance_class: "db.r7g.large",
            activity_stream_modes: &[],
            core_count: None,
            supports_clusters: true,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "db2-se",
            db_instance_class: "db.m5.large",
            activity_stream_modes: &[],
            // Two vCPUs are ONE core, which leaves nothing to configure.
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: true,
        },
        CapabilityCase {
            engine: "docdb",
            db_instance_class: "db.r5.4xlarge",
            activity_stream_modes: &[],
            // Not a core-licensed engine, and its storage is cluster-managed.
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: false,
        },
        CapabilityCase {
            engine: "neptune",
            db_instance_class: "db.t3.medium",
            activity_stream_modes: &[],
            core_count: None,
            supports_clusters: false,
            supports_dedicated_log_volume: false,
        },
    ];

    for expected in CASES {
        let case = format!("{} {}", expected.engine, expected.db_instance_class);
        let (offered, _) = describe(
            &router,
            &case,
            &[
                ("Engine", expected.engine.to_string()),
                ("DBInstanceClass", expected.db_instance_class.to_string()),
                ("MaxRecords", "1000".to_string()),
            ],
        )
        .await;
        assert!(
            !offered.is_empty(),
            "case {case:?}: the class is one this service offers for this engine, so it \
             must be reported"
        );
        for option in &offered {
            let modes = subtrees(&option.xml, "SupportedActivityStreamModes");
            assert_eq!(
                modes.len(),
                1,
                "case {case:?}: the modes are reported once, got {}",
                option.xml
            );
            assert_eq!(
                texts(&modes[0], "member"),
                expected.activity_stream_modes,
                "case {case:?}: SupportedActivityStreamModes, got {}",
                option.xml
            );
            let features = subtrees(&option.xml, "AvailableProcessorFeature");
            match expected.core_count {
                None => assert!(
                    features.is_empty(),
                    "case {case:?}: a class with no configurable processor feature must \
                     report none, got {}",
                    option.xml
                ),
                Some((default_value, allowed_values)) => {
                    let reported: Vec<(String, String, String)> = features
                        .iter()
                        .map(|feature| {
                            (
                                text(feature, "Name"),
                                text(feature, "DefaultValue"),
                                text(feature, "AllowedValues"),
                            )
                        })
                        .collect();
                    assert_eq!(
                        reported,
                        vec![
                            (
                                "coreCount".to_string(),
                                default_value.to_string(),
                                allowed_values.to_string()
                            ),
                            (
                                "threadsPerCore".to_string(),
                                "2".to_string(),
                                "1,2".to_string()
                            ),
                        ],
                        "case {case:?}: AvailableProcessorFeatures, got {}",
                        option.xml
                    );
                }
            }
            assert_eq!(
                text(&option.xml, "SupportsClusters"),
                expected.supports_clusters.to_string(),
                "case {case:?}: SupportsClusters, got {}",
                option.xml
            );
            assert_eq!(
                text(&option.xml, "SupportsDedicatedLogVolume"),
                expected.supports_dedicated_log_volume.to_string(),
                "case {case:?}: SupportsDedicatedLogVolume, got {}",
                option.xml
            );
        }
    }
}
