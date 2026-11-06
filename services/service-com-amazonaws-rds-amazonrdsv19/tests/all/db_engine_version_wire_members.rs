// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members an engine-version entry carries BESIDE the ones the
//! shipped `DBEngineVersion` shape declared: `MinorEngineVersion`, `SupportsRDSProxy`,
//! `SupportsCluster`, `SupportsDataGroups`, and — on an Aurora entry —
//! `SupportsEnhancedBinlog` and `SupportsChangeDataCaptureStreams`.
//!
//! The rule under test is stated over a TABLE of every engine this service publishes
//! versions for, and over every version of each: an entry carries the whole member set
//! Amazon RDS reports for that engine, each value is the VERSION's own, and the set does
//! not vary with what the request asked for. The pagination half asserts the invariant a
//! `Marker` owes a caller rather than one recorded token: a cut that leaves entries
//! unserved reports a token, the token resumes the report exactly where the page stopped,
//! and the page that serves the tail reports none.
//!
//! Every call goes over the wire through the router the production emulator mounts
//! (`crate::db_instances::emulator`).

use smithy_server_runtime::MultiServiceRouter;

use crate::custom_db_engine_versions::{described, KMS_KEY_ID};
use crate::db_engine_versions::{versions, walk_report, Version};
use crate::db_instances::{call, emulator, expect_ok, optional_text, text};

/// The operation under test.
const ACTION: &str = "DescribeDBEngineVersions";

/// Every engine this service publishes catalogue versions for, with whether a DB proxy
/// serves that engine (`SupportsRDSProxy`).
///
/// Source: "Using Amazon RDS Proxy" (Amazon RDS User Guide) names the engines a proxy
/// fronts — RDS for MariaDB, RDS for Microsoft SQL Server, RDS for MySQL, RDS for
/// PostgreSQL, Aurora MySQL and Aurora PostgreSQL. Oracle and Db2 are not among them, and
/// neither is RDS Custom, whose DB instances the customer administers. The two engines
/// this front door reports for another control plane (Amazon DocumentDB and Amazon
/// Neptune) are not proxy engines either.
const ENGINES: &[(&str, bool)] = &[
    ("aurora-mysql", true),
    ("aurora-postgresql", true),
    ("custom-sqlserver-dev", false),
    ("custom-sqlserver-ee", false),
    ("custom-sqlserver-se", false),
    ("custom-sqlserver-web", false),
    ("db2-ae", false),
    ("db2-ce", false),
    ("db2-se", false),
    ("docdb", false),
    ("mariadb", true),
    ("mysql", true),
    ("neptune", false),
    ("oracle-ee", false),
    ("oracle-ee-cdb", false),
    ("oracle-se2", false),
    ("oracle-se2-cdb", false),
    ("postgres", true),
    ("sqlserver-dev-ee", true),
    ("sqlserver-ee", true),
    ("sqlserver-ex", true),
    ("sqlserver-se", true),
    ("sqlserver-web", true),
];

/// The capability members every entry carries, whatever its engine.
const EVERY_ENTRY: [&str; 3] = ["SupportsRDSProxy", "SupportsCluster", "SupportsDataGroups"];

/// The capability members only an entry of a cluster engine's Aurora family carries: both
/// answer what can be done with the log an Aurora DB cluster replicates from.
const AURORA_ENTRY: [&str; 2] = ["SupportsEnhancedBinlog", "SupportsChangeDataCaptureStreams"];

/// One boolean member of an entry, which must be reported and must be a boolean.
fn flag(case: &str, xml: &str, member: &str) -> bool {
    let value = optional_text(xml, member)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be reported, got {xml}"));
    match value.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: {member} must be a boolean, got {other:?} in {xml}"),
    }
}

/// The minor version of `engine_version` within the `major_engine_version` family: what
/// follows that family's own prefix.
fn minor_of(engine_version: &str, major_engine_version: &str) -> String {
    engine_version
        .strip_prefix(&format!("{major_engine_version}."))
        .unwrap_or(engine_version)
        .to_string()
}

/// Whether AWS offers enhanced binary log replication on `engine_version` of `engine`.
///
/// Source: "Setting up Aurora MySQL enhanced binary log" (Amazon Aurora User Guide) offers
/// it from Aurora MySQL version 3.03.1 on. No other engine has a binary log to enhance, so
/// no other engine's version claims it.
fn offers_enhanced_binlog(engine: &str, engine_version: &str) -> bool {
    if engine != "aurora-mysql" {
        return false;
    }
    let Some(aurora_version) = engine_version.split("mysql_aurora.").nth(1) else {
        return false;
    };
    let components: Vec<u64> = aurora_version
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    components.as_slice() >= [3u64, 3, 1].as_slice()
}

/// Every version of `engine`, deprecated ones included, walked page by page.
async fn all_versions_of(router: &MultiServiceRouter, engine: &str) -> Vec<Version> {
    walk_report(
        router,
        engine,
        100,
        &[
            ("Engine", engine.to_string()),
            ("IncludeAll", "true".to_string()),
        ],
    )
    .await
}

#[tokio::test]
async fn every_entry_carries_the_whole_member_set_its_engine_reports() {
    let router = emulator();

    let mut reported = 0usize;
    for (engine, serves_a_proxy) in ENGINES {
        let all = all_versions_of(&router, engine).await;
        assert!(
            !all.is_empty(),
            "case {engine:?}: this service publishes versions for every engine in the \
             table, got none"
        );
        let aurora = engine.starts_with("aurora");

        for version in &all {
            reported += 1;
            let case = format!("{engine} {}", version.engine_version);

            // `MinorEngineVersion` is this release's minor version within its
            // major-version family: what follows the family's own prefix, which is how
            // every engine this service publishes writes a build of a family.
            let minor = text(&version.scalars, "MinorEngineVersion");
            assert!(
                !minor.is_empty(),
                "case {case:?}: MinorEngineVersion must be reported, got {}",
                version.scalars
            );
            assert_eq!(
                minor,
                minor_of(&version.engine_version, &version.major_engine_version),
                "case {case:?}: MinorEngineVersion must be the part of EngineVersion that \
                 follows MajorEngineVersion {:?}, got {minor:?}",
                version.major_engine_version
            );
            assert_ne!(
                minor, version.engine_version,
                "case {case:?}: a version of the {:?} family must report a minor version \
                 narrower than its whole EngineVersion, got {minor:?}",
                version.major_engine_version
            );

            // Reading each of these is half the assertion: `flag` refuses an absent
            // member and a value that is not a boolean.
            for member in EVERY_ENTRY {
                let _ = flag(&case, &version.scalars, member);
            }
            assert_eq!(
                flag(&case, &version.scalars, "SupportsRDSProxy"),
                *serves_a_proxy,
                "case {case:?}: SupportsRDSProxy must report whether a DB proxy serves \
                 this engine, got {}",
                version.scalars
            );

            // No published source names a version offering a cluster deployment or a data
            // group through this member, so no version claims one.
            for member in ["SupportsCluster", "SupportsDataGroups"] {
                assert!(
                    !flag(&case, &version.scalars, member),
                    "case {case:?}: no engine version this service publishes claims \
                     {member}, got {}",
                    version.scalars
                );
            }

            // The replication-log members belong to the Aurora family: an Aurora entry
            // answers both, and an entry of an engine whose DB instance keeps its own
            // storage carries neither.
            for member in AURORA_ENTRY {
                let value = optional_text(&version.scalars, member);
                assert_eq!(
                    value.is_some(),
                    aurora,
                    "case {case:?}: {member} is reported for an Aurora version and for no \
                     other, got {value:?} in {}",
                    version.scalars
                );
            }
            if aurora {
                assert_eq!(
                    flag(&case, &version.scalars, "SupportsEnhancedBinlog"),
                    offers_enhanced_binlog(engine, &version.engine_version),
                    "case {case:?}: SupportsEnhancedBinlog must report whether AWS offers \
                     enhanced binary logging on this version, got {}",
                    version.scalars
                );
                assert!(
                    !flag(&case, &version.scalars, "SupportsChangeDataCaptureStreams"),
                    "case {case:?}: no engine version this service publishes claims a \
                     change-data-capture stream, got {}",
                    version.scalars
                );
            }
        }
    }
    assert!(
        reported > ENGINES.len(),
        "case \"every engine\": the report must carry several versions per engine, got \
         {reported} across {} engines",
        ENGINES.len()
    );
}

#[tokio::test]
async fn the_member_set_does_not_vary_with_what_the_request_asked_for() {
    let router = emulator();

    // One version, read four ways: narrowed by engine, narrowed by engine and version,
    // narrowed by parameter group family, and with the character-set and time-zone lists
    // asked for. The member set an entry carries is the entry's, not the request's.
    let anchor = all_versions_of(&router, "aurora-postgresql")
        .await
        .into_iter()
        .next_back()
        .expect("aurora-postgresql publishes versions");
    let narrowings: [(&str, Vec<(&str, String)>); 4] = [
        ("engine only", vec![("Engine", "aurora-postgresql".to_string())]),
        (
            "engine and version",
            vec![
                ("Engine", "aurora-postgresql".to_string()),
                ("EngineVersion", anchor.engine_version.clone()),
            ],
        ),
        (
            "parameter group family",
            vec![(
                "DBParameterGroupFamily",
                anchor.parameter_group_family.clone(),
            )],
        ),
        (
            "lists asked for",
            vec![
                ("Engine", "aurora-postgresql".to_string()),
                ("EngineVersion", anchor.engine_version.clone()),
                ("ListSupportedCharacterSets", "true".to_string()),
                ("ListSupportedTimezones", "true".to_string()),
            ],
        ),
    ];

    for (case, params) in narrowings {
        let body = expect_ok(case, call(&router, ACTION, &params).await);
        let entry = versions(&body)
            .into_iter()
            .find(|version| version.engine_version == anchor.engine_version)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the report must carry {:?}, got {body}",
                    anchor.engine_version
                )
            });
        assert_eq!(
            text(&entry.scalars, "MinorEngineVersion"),
            text(&anchor.scalars, "MinorEngineVersion"),
            "case {case:?}: MinorEngineVersion must not depend on how the request \
             narrowed the report, got {}",
            entry.scalars
        );
        for member in EVERY_ENTRY.iter().chain(AURORA_ENTRY.iter()) {
            assert_eq!(
                flag(case, &entry.scalars, member),
                flag(case, &anchor.scalars, member),
                "case {case:?}: {member} must not depend on how the request narrowed the \
                 report, got {}",
                entry.scalars
            );
        }
    }
}

/// One custom engine version a create stores, as the round-trip table names it.
struct Row {
    engine: &'static str,
    engine_version: &'static str,
    /// The major version whose family the CEV name belongs to.
    major_engine_version: &'static str,
    /// The rest of the create's parameters.
    extra: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_stored_custom_engine_version_carries_the_same_member_set() {
    let router = emulator();

    // A custom engine version is the one engine-version kind a request creates, so it is
    // the one that can be round-tripped: create it, then read it back through
    // `DescribeDBEngineVersions`. Both answers come from one projection, so both carry
    // the member set a catalogue row of the same engine carries.
    //
    // (engine, CEV name, the major version its name belongs to, the rest of the create).
    // The name forms are the ones `CreateCustomDBEngineVersion$EngineVersion` publishes,
    // and RDS Custom for SQL Server requires an AMI id.
    let rows: [Row; 2] = [
        Row {
            engine: "custom-sqlserver-se",
            engine_version: "15.00.4073.23.my_cev1",
            major_engine_version: "15.00",
            extra: &[
                ("KMSKeyId", KMS_KEY_ID),
                ("UseAwsProvidedLatestImage", "true"),
            ],
        },
        Row {
            engine: "custom-sqlserver-ee",
            engine_version: "15.00.4249.2.my_cev1",
            major_engine_version: "15.00",
            extra: &[
                ("KMSKeyId", KMS_KEY_ID),
                ("ImageId", "ami-0abcdef1234567890"),
            ],
        },
    ];

    for Row {
        engine,
        engine_version,
        major_engine_version,
        extra,
    } in rows
    {
        let case = format!("{engine} {engine_version}");
        let mut params = vec![
            ("Engine".to_string(), engine.to_string()),
            ("EngineVersion".to_string(), engine_version.to_string()),
        ];
        for (name, value) in extra {
            params.push(((*name).to_string(), (*value).to_string()));
        }
        let created = expect_ok(
            &case,
            call(&router, "CreateCustomDBEngineVersion", &params).await,
        );
        let described = described(&router, "us-west-2", &case, engine, engine_version, true).await;
        let read_back = described.first().unwrap_or_else(|| {
            panic!("case {case:?}: the created version must be reported, got {described:?}")
        });

        for projection in [&created, read_back] {
            assert_eq!(
                text(projection, "MinorEngineVersion"),
                minor_of(engine_version, major_engine_version),
                "case {case:?}: a custom engine version reports the minor version of its \
                 own name, got {projection}"
            );
            for member in EVERY_ENTRY {
                // Present and a boolean on the create's answer and on the read-back
                // alike; an RDS Custom version fronts no proxy.
                assert!(
                    !flag(&case, projection, member),
                    "case {case:?}: {member} must be reported unsupported for an RDS \
                     Custom version, got {projection}"
                );
            }
            for member in AURORA_ENTRY {
                assert_eq!(
                    optional_text(projection, member),
                    None,
                    "case {case:?}: {member} is reported for an Aurora version only, got \
                     {projection}"
                );
            }
        }
    }
}

/// One page of the whole report at `page_size`, resumed at `marker`: the entries and the
/// `Marker` the result carried.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
    marker: Option<&str>,
) -> (Vec<Version>, Option<String>) {
    let mut params = vec![
        ("MaxRecords".to_string(), page_size.to_string()),
        ("IncludeAll".to_string(), "true".to_string()),
    ];
    if let Some(marker) = marker {
        params.push(("Marker".to_string(), marker.to_string()));
    }
    let body = expect_ok(case, call(router, ACTION, &params).await);
    (versions(&body), optional_text(&body, "Marker"))
}

/// The identity of each reported entry, as two pages are compared by.
fn keys(entries: &[Version]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|version| (version.engine.clone(), version.engine_version.clone()))
        .collect()
}

#[tokio::test]
async fn a_cut_that_leaves_entries_unserved_reports_the_marker_that_resumes_it() {
    let router = emulator();

    let whole = walk_report(
        &router,
        "whole report",
        100,
        &[("IncludeAll", "true".to_string())],
    )
    .await;
    assert!(
        whole.len() > 40,
        "case \"whole report\": the catalogue must be larger than one page, got {} entries",
        whole.len()
    );

    // Each published `MaxRecords` bound and one size between them: wherever the cut
    // falls, the page is the report's own page and the token names the entry the next
    // page starts at.
    for page_size in [20usize, 37, 100] {
        let case = format!("page size {page_size}");
        let (first, marker) = page(&router, &case, page_size, None).await;
        let served = page_size.min(whole.len());
        assert_eq!(
            keys(&first),
            keys(&whole[..served]),
            "case {case:?}: the first page must be the report's own first {served} entries"
        );
        if whole.len() <= page_size {
            assert_eq!(
                marker, None,
                "case {case:?}: a page that serves the whole report reports no \
                 continuation token"
            );
            continue;
        }
        let marker = marker.unwrap_or_else(|| {
            panic!(
                "case {case:?}: a cut that leaves {} entries unserved must report a \
                 continuation token",
                whole.len() - served
            )
        });
        let (second, _) = page(&router, &case, page_size, Some(&marker)).await;
        let end = (served + page_size).min(whole.len());
        assert_eq!(
            keys(&second),
            keys(&whole[served..end]),
            "case {case:?}: the token must resume the report where the first page \
             stopped, with entries {served}..{end}"
        );
    }

    // Walking to the end: every entry exactly once, and the page that serves the tail
    // reports no token at all.
    let mut seen: Vec<Version> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let (entries, next) = page(&router, "walk to the end", 100, marker.as_deref()).await;
        seen.extend(entries);
        match next {
            Some(next) => marker = Some(next),
            None => break,
        }
        assert!(
            seen.len() <= whole.len(),
            "case \"walk to the end\": paging must terminate at {} entries, got {}",
            whole.len(),
            seen.len()
        );
    }
    assert_eq!(
        keys(&seen),
        keys(&whole),
        "case \"walk to the end\": the walked pages must be the whole report, once each"
    );
}
