// SPDX-License-Identifier: CC0-1.0

//! The wire form of a FLOATING-POINT member of this front door's responses: a decimal
//! carrying a fractional part.
//!
//! AWS renders a Smithy `double` in an XML response as a decimal — a whole value reads
//! `0.0`, `16.0`, `256.0` — while an INTEGER member of the same document carries no
//! fractional part at all. The rule is the protocol's, so it is asserted here over every
//! double this front door puts on the wire rather than over the one member an issue
//! happened to cite: the engine-version report's Aurora Serverless v2 limits, a DB
//! cluster's own Serverless v2 capacity range (named by the create and read back), and
//! the IOPS ratios an orderable DB instance option reports.
//!
//! Each assertion is made BOTH ways: a double carries a fractional part, and an integer
//! member of the same answer does not — otherwise a rewrite that turned every number into
//! a decimal would pass.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::all_versions_of;
use crate::db_instances::{call, emulator, expect_ok, subtrees, text, texts};

/// Whether `value` is the decimal form a floating-point member carries: at least one digit
/// on each side of exactly one decimal point, after an optional sign.
fn carries_a_fractional_part(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    match digits.split_once('.') {
        Some((whole, fraction)) => {
            !whole.is_empty()
                && whole.bytes().all(|byte| byte.is_ascii_digit())
                && !fraction.is_empty()
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        }
        None => false,
    }
}

/// Assert that every `<member>` of `xml` is a decimal with a fractional part, and hand
/// back the values read — so a caller can also require that there WERE any.
fn assert_decimal_members(case: &str, xml: &str, member: &str) -> Vec<String> {
    let values = texts(xml, member);
    for value in &values {
        assert!(
            carries_a_fractional_part(value),
            "case {case:?}: <{member}> is a double, which carries a fractional part on the \
             wire, got {value:?} in {xml}"
        );
    }
    values
}

/// Assert that every `<member>` of `xml` is an integer — the other half of the rule: only
/// a floating-point member grows a fractional part.
fn assert_integer_members(case: &str, xml: &str, member: &str) {
    for value in texts(xml, member) {
        assert!(
            !carries_a_fractional_part(value.as_str())
                && value
                    .strip_prefix('-')
                    .unwrap_or(&value)
                    .bytes()
                    .all(|byte| byte.is_ascii_digit()),
            "case {case:?}: <{member}> is an integer member, so it carries no fractional \
             part, got {value:?} in {xml}"
        );
    }
}

/// `CreateDBCluster` for one row of the capacity table, asserted to answer 200.
async fn create_cluster(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    min_capacity: &str,
    max_capacity: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier.to_string()),
                ("Engine", "aurora-mysql".to_string()),
                (
                    "ServerlessV2ScalingConfiguration.MinCapacity",
                    min_capacity.to_string(),
                ),
                (
                    "ServerlessV2ScalingConfiguration.MaxCapacity",
                    max_capacity.to_string(),
                ),
            ],
        )
        .await,
    )
}

/// Every Aurora Serverless v2 capacity limit the engine-version report publishes is a
/// decimal, whichever engine and version carries it — including the whole-valued ones
/// (`0`, `128`, `256` ACUs), which are exactly the values Rust's own rendering would
/// report without a fractional part.
#[tokio::test]
async fn the_engine_version_report_states_every_serverless_v2_limit_as_a_decimal() {
    let router = emulator();

    // Every engine the report publishes Aurora Serverless v2 limits for — the rows differ
    // along the engine and, within it, along every version the catalogue holds, so the
    // table is the report's own rather than a list written out here.
    let mut limits_seen = 0usize;
    for engine in ["aurora-mysql", "aurora-postgresql", "mysql", "postgres"] {
        for version in all_versions_of(&router, engine).await {
            let case = format!("{engine} {}", version.engine_version);
            for limits in subtrees(&version.xml, "ServerlessV2FeaturesSupport") {
                assert_decimal_members(&case, &limits, "MinCapacity");
                assert_decimal_members(&case, &limits, "MaxCapacity");
                limits_seen += 1;
            }
        }
    }
    assert!(
        limits_seen > 0,
        "expected the report to publish Aurora Serverless v2 limits for at least one \
         version, got none"
    );

    // The three Aurora MySQL 3 versions that disagree about both bounds, and one Aurora
    // PostgreSQL version: the capacity a version reports is its own, and each is reported
    // as a decimal. (case, engine, engine version, MinCapacity, MaxCapacity)
    let rows: [(&str, &str, &str, &str, &str); 4] = [
        (
            "before the raised maximum",
            "aurora-mysql",
            "8.0.mysql_aurora.3.05.2",
            "0.5",
            "128.0",
        ),
        (
            "at the raised maximum",
            "aurora-mysql",
            "8.0.mysql_aurora.3.07.1",
            "0.5",
            "256.0",
        ),
        (
            "scales to zero",
            "aurora-mysql",
            "8.0.mysql_aurora.3.08.0",
            "0.0",
            "256.0",
        ),
        (
            "Aurora PostgreSQL",
            "aurora-postgresql",
            "16.6",
            "0.0",
            "256.0",
        ),
    ];
    for (case, engine, engine_version, min, max) in rows {
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeDBEngineVersions",
                &[
                    ("Engine", engine.to_string()),
                    ("EngineVersion", engine_version.to_string()),
                ],
            )
            .await,
        );
        let limits = subtrees(&body, "ServerlessV2FeaturesSupport")
            .first()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the version's Serverless v2 limits, got {body}")
            });
        assert_eq!(
            (
                text(&limits, "MinCapacity").as_str(),
                text(&limits, "MaxCapacity").as_str()
            ),
            (min, max),
            "case {case:?}: expected the published limits as decimals, got {limits}"
        );
    }
}

/// A DB cluster's own Serverless v2 capacity range: the create names it, the answer and
/// every later read report it as a decimal, and the integer members of the same answer
/// keep their integral form.
#[tokio::test]
async fn a_db_cluster_reports_its_serverless_v2_capacity_as_a_decimal() {
    let router = emulator();

    // Rows differ along the form of the capacity the create names: a whole value, a
    // half-step fractional one, the scale-to-zero minimum, and the two-digit and
    // three-digit magnitudes. (case, identifier, MinCapacity named, MaxCapacity named,
    // MinCapacity reported, MaxCapacity reported)
    let rows: [(&str, &str, &str, &str, &str, &str); 4] = [
        (
            "fractional minimum, whole maximum",
            "emu-decimal-cluster-a",
            "0.5",
            "16",
            "0.5",
            "16.0",
        ),
        (
            "whole minimum and maximum",
            "emu-decimal-cluster-b",
            "2",
            "64",
            "2.0",
            "64.0",
        ),
        (
            "scale to zero",
            "emu-decimal-cluster-c",
            "0",
            "256",
            "0.0",
            "256.0",
        ),
        (
            "both named with a fractional part",
            "emu-decimal-cluster-d",
            "1.5",
            "2.5",
            "1.5",
            "2.5",
        ),
    ];

    for (case, identifier, named_min, named_max, min, max) in rows {
        let created = create_cluster(&router, case, identifier, named_min, named_max).await;
        let read_back = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier.to_string())],
            )
            .await,
        );
        for (label, body) in [("the create's answer", &created), ("the read back", &read_back)] {
            let case = format!("{case} ({label})");
            let scaling = subtrees(body, "ServerlessV2ScalingConfiguration")
                .first()
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the cluster's Serverless v2 capacity range, \
                         got {body}"
                    )
                });
            assert_eq!(
                (
                    text(&scaling, "MinCapacity").as_str(),
                    text(&scaling, "MaxCapacity").as_str()
                ),
                (min, max),
                "case {case:?}: expected the capacity range the create named, as decimals, \
                 got {scaling}"
            );
            // The other half of the rule, on the same document: an integer member of the
            // cluster carries no fractional part.
            for member in [
                "BackupRetentionPeriod",
                "Port",
                "BacktrackWindow",
                "MonitoringInterval",
            ] {
                assert_integer_members(&case, body, member);
            }
        }
    }
}

/// The IOPS ratios an orderable DB instance option reports are doubles too, so the same
/// rule holds for an operation that reports no capacity at all — while the option's
/// integer bounds stay integral.
#[tokio::test]
async fn an_orderable_db_instance_option_states_its_ratios_as_decimals() {
    let router = emulator();

    let mut ratios_seen = 0usize;
    // Two engines whose options carry the ratio members, and one that is storage-typed
    // differently, so the rows differ along what the option publishes.
    for engine in ["mysql", "postgres", "aurora-mysql"] {
        let case = format!("orderable options of {engine}");
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &[("Engine", engine.to_string())],
            )
            .await,
        );
        let options = subtrees(&body, "OrderableDBInstanceOption");
        // The report answered at all, so the loop below is not vacuous.
        assert!(
            !options.is_empty(),
            "case {case:?}: expected at least one orderable option, got {body}"
        );
        for option in options {
            for member in [
                "MinIopsPerGib",
                "MaxIopsPerGib",
                "MinStorageThroughputPerIops",
                "MaxStorageThroughputPerIops",
            ] {
                ratios_seen += assert_decimal_members(&case, &option, member).len();
            }
            for member in [
                "MinStorageSize",
                "MaxStorageSize",
                "MinIopsPerDbInstance",
                "MaxIopsPerDbInstance",
            ] {
                assert_integer_members(&case, &option, member);
            }
        }
    }
    assert!(
        ratios_seen > 0,
        "expected at least one reported IOPS ratio across the engines above, got none"
    );
}
