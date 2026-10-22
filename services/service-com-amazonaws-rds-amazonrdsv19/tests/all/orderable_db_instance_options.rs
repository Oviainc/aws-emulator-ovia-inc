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

use std::collections::BTreeSet;

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

/// Every option offered for `engine`, paged to exhaustion so the assertions see the whole
/// report rather than its first page.
async fn all_options_for(router: &MultiServiceRouter, engine: &str) -> Vec<OrderableOption> {
    let mut collected: Vec<OrderableOption> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut params: Vec<(&str, String)> = vec![("Engine", engine.to_string())];
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let (page, next) = describe(router, &format!("options for {engine}"), &params).await;
        collected.extend(page);
        match next {
            Some(next) => marker = Some(next),
            None => break,
        }
        assert!(
            collected.len() < 10_000,
            "case {engine:?}: paging did not terminate"
        );
    }
    collected
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
            let zones = texts(&option.xml, "Name");
            assert_eq!(
                zones,
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

        // One option per (version, class, license model): no duplicate rows.
        let triples: BTreeSet<(String, String, String)> = offered
            .iter()
            .map(|option| {
                (
                    option.engine_version.clone(),
                    option.db_instance_class.clone(),
                    option.license_model.clone(),
                )
            })
            .collect();
        assert_eq!(
            triples.len(),
            offered.len(),
            "case {engine:?}: an option must be offered once per version, class and \
             license model, got {} rows for {} combinations",
            offered.len(),
            triples.len()
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
            assert_eq!(
                texts(&option.xml, "Name"),
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
        let (offered, _) = describe(
            &router,
            &case,
            &[
                ("Engine", "mysql".to_string()),
                (member, value.clone()),
                ("MaxRecords", "1000".to_string()),
            ],
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

    let expected: Vec<(String, String, String)> = all_options_for(&router, "mysql")
        .await
        .into_iter()
        .map(|option| {
            (
                option.engine_version,
                option.db_instance_class,
                option.license_model,
            )
        })
        .collect();

    let mut seen: Vec<(String, String, String)> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
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
        seen.extend(page.into_iter().map(|option| {
            (
                option.engine_version,
                option.db_instance_class,
                option.license_model,
            )
        }));
        pages += 1;
        assert!(
            pages < 50,
            "case \"paged options\": paging did not terminate after {pages} pages"
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
