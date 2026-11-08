// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDBEngineVersions`, the engine-version report.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (`crate::db_instances::emulator`), each
//! request is a real awsQuery form POST carrying the SigV4 credential scope the router
//! keys on, and every assertion reads the XML the emulator answered with.
//!
//! The rules under test are stated over TABLES and over the report's OWN answer rather
//! than over one recorded request: the engine list is the `Engine` member's published
//! valid values (so every engine is a row, including the ones no issue cited), the
//! default-version and upgrade-target rules are checked per engine and per major version
//! the service itself reports, and each published `MaxRecords` bound is checked together
//! with one value past it.

use std::collections::{BTreeMap, BTreeSet};

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
};

/// The operation under test.
const ACTION: &str = "DescribeDBEngineVersions";

/// Every engine `DescribeDBEngineVersionsMessage$Engine` lists under "Valid Values"
/// (`smithy_query(query="doc",shape="DescribeDBEngineVersionsMessage",member="Engine")`).
const MODELLED_ENGINES: [&str; 21] = [
    "aurora-mysql",
    "aurora-postgresql",
    "custom-oracle-ee",
    "custom-oracle-ee-cdb",
    "custom-oracle-se2",
    "custom-oracle-se2-cdb",
    "db2-ae",
    "db2-ce",
    "db2-se",
    "mariadb",
    "mysql",
    "oracle-ee",
    "oracle-ee-cdb",
    "oracle-se2",
    "oracle-se2-cdb",
    "postgres",
    "sqlserver-ee",
    "sqlserver-se",
    "sqlserver-ex",
    "sqlserver-web",
    "sqlserver-dev-ee",
];

/// The engines whose versions are CUSTOM engine versions the ACCOUNT mints: RDS Custom
/// for Oracle, whose CEVs `CreateCustomDBEngineVersion` creates from customer-supplied
/// installation media. This service therefore carries no catalogue version for them and
/// must report one only where a create has stored it — never as reference data. RDS Custom
/// for SQL Server is NOT among them: its custom engine versions are AWS-provided, so they
/// are catalogue data (see `db_engine_version_catalog.rs`).
///
/// `pub(crate)` so the catalogue suite states this engine list once.
pub(crate) const CUSTOM_ENGINES: [&str; 4] = [
    "custom-oracle-ee",
    "custom-oracle-ee-cdb",
    "custom-oracle-se2",
    "custom-oracle-se2-cdb",
];

/// The two `Status` values the `status` filter's member documents.
const STATUS_AVAILABLE: &str = "available";
const STATUS_DEPRECATED: &str = "deprecated";

/// One reported `<DBEngineVersion>`: the members this suite asserts on, plus the
/// subtree's own XML so a test can read more of it.
///
/// `pub(crate)` so the orderable-options suite can cross-check its own report against
/// the engine versions this one reports.
#[derive(Debug, Clone)]
pub(crate) struct Version {
    pub(crate) engine: String,
    pub(crate) engine_version: String,
    pub(crate) major_engine_version: String,
    pub(crate) parameter_group_family: String,
    pub(crate) status: String,
    /// The entry's XML with its `<ValidUpgradeTarget>` removed.
    pub(crate) scalars: String,
    /// The entry's whole XML, upgrade targets included.
    pub(crate) xml: String,
}

/// The entry's XML without its `<ValidUpgradeTarget>` subtree.
///
/// An upgrade target carries its OWN `<Engine>`, `<EngineVersion>` and
/// `<SupportedEngineModes>`, so a member read over the whole entry would pick up a
/// target's copy; stripping the list first is what makes "the one `<Engine>` of this
/// entry" well defined.
fn strip_upgrade_targets(xml: &str) -> String {
    let mut stripped = xml.to_string();
    for subtree in subtrees(xml, "ValidUpgradeTarget") {
        stripped = stripped.replace(&subtree, "");
    }
    stripped
}

/// Every `<DBEngineVersion>` in a response body, in the order it was reported.
pub(crate) fn versions(xml: &str) -> Vec<Version> {
    subtrees(xml, "DBEngineVersion")
        .into_iter()
        .map(|entry| {
            let scalars = strip_upgrade_targets(&entry);
            Version {
                engine: text(&scalars, "Engine"),
                engine_version: text(&scalars, "EngineVersion"),
                major_engine_version: text(&scalars, "MajorEngineVersion"),
                parameter_group_family: text(&scalars, "DBParameterGroupFamily"),
                status: text(&scalars, "Status"),
                scalars,
                xml: entry,
            }
        })
        .collect()
}

/// `DescribeDBEngineVersions` with `params`, asserted to answer 200, as the entries it
/// reported and the `Marker` it carried.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&str, String)],
) -> (Vec<Version>, Option<String>) {
    let body = expect_ok(case, call(router, ACTION, params).await);
    (versions(&body), optional_text(&body, "Marker"))
}

/// Every entry of one engine, deprecated ones included, in reported order.
///
/// `pub(crate)` so the catalogue suites read an engine's versions through the same
/// helper rather than a second copy of it.
pub(crate) async fn all_versions_of(router: &MultiServiceRouter, engine: &str) -> Vec<Version> {
    let (reported, marker) = describe(
        router,
        &format!("every version of {engine}"),
        &[
            ("Engine", engine.to_string()),
            ("IncludeAll", "true".to_string()),
        ],
    )
    .await;
    assert_eq!(
        marker, None,
        "case {engine:?}: one engine's versions must fit the default page, got a Marker"
    );
    reported
}

// ---------------------------------------------------------------------------
// The report itself: which engines and versions it covers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_modelled_engine_reports_its_own_versions_and_nothing_else() {
    let router = emulator();

    for engine in MODELLED_ENGINES {
        let reported = all_versions_of(&router, engine).await;

        if CUSTOM_ENGINES.contains(&engine) {
            assert!(
                reported.is_empty(),
                "case {engine:?}: a custom engine version exists only once \
                 CreateCustomDBEngineVersion stores one, and this emulator stored none, so \
                 the report must be empty, got {:?}",
                reported
                    .iter()
                    .map(|version| version.engine_version.clone())
                    .collect::<Vec<_>>()
            );
            continue;
        }

        assert!(
            !reported.is_empty(),
            "case {engine:?}: the engine is a published valid value, so the report must \
             carry at least one version, got none"
        );
        for version in &reported {
            assert_eq!(
                version.engine, engine,
                "case {engine:?}: Engine={:?} narrows the report to that engine, got \
                 {:?} in {}",
                engine, version.engine, version.xml
            );
            // An entry's own members have to agree with each other: the version is
            // inside its major version, and the parameter group family is a family of
            // this engine.
            assert!(
                version
                    .engine_version
                    .starts_with(&version.major_engine_version),
                "case {engine:?}: MajorEngineVersion {:?} must be the major version of \
                 EngineVersion {:?}, got {}",
                version.major_engine_version,
                version.engine_version,
                version.scalars
            );
            assert!(
                version.parameter_group_family.starts_with(engine),
                "case {engine:?}: DBParameterGroupFamily {:?} must be a family of this \
                 engine, got {}",
                version.parameter_group_family,
                version.scalars
            );
            assert!(
                version.status == STATUS_AVAILABLE || version.status == STATUS_DEPRECATED,
                "case {engine:?}: Status must be one of the two documented values, got \
                 {:?} in {}",
                version.status,
                version.scalars
            );
            // Every reported entry is described, so a client listing versions has the
            // text AWS shows beside each one.
            for member in ["DBEngineDescription", "DBEngineVersionDescription"] {
                let described = text(&version.scalars, member);
                assert!(
                    !described.is_empty(),
                    "case {engine:?}: {member} must not be empty, got {}",
                    version.scalars
                );
            }
        }
    }
}

#[tokio::test]
async fn a_version_or_family_that_names_nothing_reports_nothing_rather_than_failing() {
    let router = emulator();

    // `EngineVersion` and `DBParameterGroupFamily` narrow the report; a value that names
    // no version or family therefore selects nothing, and an empty 200 — not a fault — is
    // the answer. (An `Engine` that names no ENGINE is a different case: the engine name is
    // resolved rather than matched, and is refused — see
    // `tests/all/refusal_wording_and_input_guards.rs`.)
    for (case, member, value) in [
        ("a version no engine has", "EngineVersion", "0.0.0"),
        (
            "a parameter group family that does not exist",
            "DBParameterGroupFamily",
            "nosuchfamily1",
        ),
    ] {
        let (reported, marker) = describe(
            &router,
            case,
            &[(member, value.to_string()), ("IncludeAll", "true".into())],
        )
        .await;
        assert!(
            reported.is_empty() && marker.is_none(),
            "case {case:?}: expected an empty report, got {:?}",
            reported
                .iter()
                .map(|version| version.engine_version.clone())
                .collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------------
// DefaultOnly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_only_reports_one_default_per_engine_and_per_major_version() {
    let router = emulator();

    for engine in MODELLED_ENGINES {
        let all = all_versions_of(&router, engine).await;
        if all.is_empty() {
            continue;
        }

        // "only the default version of the specified engine": one entry, and it is one
        // of the engine's own versions.
        let (default_for_engine, _) = describe(
            &router,
            &format!("the default version of {engine}"),
            &[
                ("Engine", engine.to_string()),
                ("DefaultOnly", "true".to_string()),
                ("IncludeAll", "true".to_string()),
            ],
        )
        .await;
        assert_eq!(
            default_for_engine.len(),
            1,
            "case {engine:?}: DefaultOnly must report exactly one version of the engine, \
             got {:?}",
            default_for_engine
                .iter()
                .map(|version| version.engine_version.clone())
                .collect::<Vec<_>>()
        );
        let engine_default = &default_for_engine[0];
        assert!(
            all.iter()
                .any(|version| version.engine_version == engine_default.engine_version),
            "case {engine:?}: the default version {:?} must be one the full report \
             carries",
            engine_default.engine_version
        );

        // "or the engine and major version combination": narrowed to one major version,
        // that major version's default — one entry, inside the major version asked for.
        let majors: BTreeSet<String> = all
            .iter()
            .map(|version| version.major_engine_version.clone())
            .collect();
        for major in &majors {
            let case = format!("the default version of {engine} {major}");
            let (defaults, _) = describe(
                &router,
                &case,
                &[
                    ("Engine", engine.to_string()),
                    ("EngineVersion", major.clone()),
                    ("DefaultOnly", "true".to_string()),
                    ("IncludeAll", "true".to_string()),
                ],
            )
            .await;
            assert_eq!(
                defaults.len(),
                1,
                "case {case:?}: expected exactly one default for the major version, got \
                 {:?}",
                defaults
                    .iter()
                    .map(|version| version.engine_version.clone())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                &defaults[0].major_engine_version, major,
                "case {case:?}: the default reported must be inside the major version \
                 asked for, got {}",
                defaults[0].scalars
            );
        }

        // The engine's default is also the default of its own major version, so the two
        // reports agree where they overlap.
        let case = format!("the default of {engine}'s default major version");
        let (same, _) = describe(
            &router,
            &case,
            &[
                ("Engine", engine.to_string()),
                (
                    "EngineVersion",
                    engine_default.major_engine_version.clone(),
                ),
                ("DefaultOnly", "true".to_string()),
                ("IncludeAll", "true".to_string()),
            ],
        )
        .await;
        assert_eq!(
            same.first().map(|version| version.engine_version.clone()),
            Some(engine_default.engine_version.clone()),
            "case {case:?}: an engine's default version must be its major version's \
             default too, got {:?}",
            same.iter()
                .map(|version| version.engine_version.clone())
                .collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------------
// IncludeAll and the status filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn include_all_is_what_reports_a_version_that_is_not_available() {
    let router = emulator();

    // The published catalogue is larger than the biggest page this operation serves, so
    // each of the two reports is WALKED: comparing one page of each would compare two
    // different windows onto the catalogue.
    let with_all = walk_every_version(&router, "every version", 100).await;
    let available_only = walk_report(&router, "available versions", 100, &[]).await;

    let all_keys: BTreeSet<(String, String)> = with_all
        .iter()
        .map(|version| (version.engine.clone(), version.engine_version.clone()))
        .collect();
    let available_keys: BTreeSet<(String, String)> = available_only
        .iter()
        .map(|version| (version.engine.clone(), version.engine_version.clone()))
        .collect();

    assert!(
        available_keys.is_subset(&all_keys),
        "case \"default report\": the default report must be a subset of the IncludeAll \
         report, extra entries {:?}",
        available_keys.difference(&all_keys).collect::<Vec<_>>()
    );
    for version in &available_only {
        assert_eq!(
            version.status, STATUS_AVAILABLE,
            "case \"default report\": \"the default is to list only available engine \
             versions\", got {}",
            version.scalars
        );
    }
    assert!(
        with_all
            .iter()
            .any(|version| version.status == STATUS_DEPRECATED),
        "case \"IncludeAll\": the report must carry a version that is not available, so \
         the flag is observable; statuses {:?}",
        with_all
            .iter()
            .map(|version| version.status.clone())
            .collect::<BTreeSet<_>>()
    );
    assert!(
        available_keys.len() < all_keys.len(),
        "case \"IncludeAll\": it must report MORE than the default, got {} and {}",
        all_keys.len(),
        available_keys.len()
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_documented_filter_narrows_the_report_to_what_it_names() {
    let router = emulator();

    let all = walk_every_version(&router, "every version", 100).await;
    assert!(!all.is_empty(), "the report must carry versions to filter");

    // One row per filter name the `Filters` member documents, each with a value taken
    // from the report itself plus the member of an entry it must agree with. The
    // `engine-mode` row is the one whose value is not an entry member: it matches an
    // entry's `SupportedEngineModes`, so it is checked on its own below.
    //
    // `FilterRow`'s third column reads the entry member the filter selects on, so one
    // loop can assert every row.
    type FilterRow = (&'static str, String, fn(&Version) -> String);
    let rows: [FilterRow; 4] = [
        ("engine", "postgres".to_string(), |version| {
            version.engine.clone()
        }),
        (
            "engine-version",
            all.iter()
                .find(|version| version.engine == "mysql")
                .map(|version| version.engine_version.clone())
                .expect("the report carries a mysql version"),
            |version| version.engine_version.clone(),
        ),
        (
            "db-parameter-group-family",
            all.iter()
                .find(|version| version.engine == "sqlserver-ee")
                .map(|version| version.parameter_group_family.clone())
                .expect("the report carries a sqlserver-ee version"),
            |version| version.parameter_group_family.clone(),
        ),
        ("status", STATUS_DEPRECATED.to_string(), |version| {
            version.status.clone()
        }),
    ];

    for (name, value, member_of) in rows {
        let case = format!("filter {name}={value}");
        let reported = walk_report(
            &router,
            &case,
            100,
            &[
                ("Filters.Filter.1.Name", name.to_string()),
                ("Filters.Filter.1.Values.Value.1", value.clone()),
                ("IncludeAll", "true".to_string()),
            ],
        )
        .await;
        assert!(
            !reported.is_empty(),
            "case {case:?}: the value came from the report itself, so it must select \
             something"
        );
        for version in &reported {
            assert_eq!(
                member_of(version),
                value,
                "case {case:?}: every reported entry must match the filter, got {}",
                version.scalars
            );
        }
        // …and it really narrowed: the excluded entries are gone.
        let expected: usize = all
            .iter()
            .filter(|version| member_of(version) == value)
            .count();
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected the {expected} matching entries of the full report, \
             got {}",
            reported.len()
        );
    }
}

#[tokio::test]
async fn the_engine_mode_filter_selects_by_supported_engine_mode() {
    let router = emulator();

    // "Valid DB engine modes are the following: global, multimaster, parallelquery,
    // provisioned, serverless" — every published value is a row, including the three no
    // engine here runs, which must select nothing rather than everything.
    for mode in ["global", "multimaster", "parallelquery", "provisioned", "serverless"] {
        let case = format!("engine-mode={mode}");
        // `provisioned` selects nearly the whole catalogue, which is larger than one
        // page, so the selection is walked rather than read off its first page.
        let reported = walk_report(
            &router,
            &case,
            100,
            &[
                ("Filters.Filter.1.Name", "engine-mode".to_string()),
                ("Filters.Filter.1.Values.Value.1", mode.to_string()),
                ("IncludeAll", "true".to_string()),
            ],
        )
        .await;
        for version in &reported {
            let modes = subtrees(&version.scalars, "SupportedEngineModes")
                .into_iter()
                .next()
                .unwrap_or_default();
            assert!(
                modes.contains(&format!(">{mode}<")),
                "case {case:?}: every reported entry must support that mode, got {}",
                version.scalars
            );
        }
        match mode {
            // Every engine version here runs provisioned; only Aurora also runs
            // serverless.
            "provisioned" => assert!(
                !reported.is_empty(),
                "case {case:?}: provisioned must select the engines that run it"
            ),
            "serverless" => {
                assert!(
                    !reported.is_empty(),
                    "case {case:?}: serverless must select the Aurora engines"
                );
                for version in &reported {
                    assert!(
                        version.engine.starts_with("aurora"),
                        "case {case:?}: only an Aurora engine runs serverless, got {}",
                        version.scalars
                    );
                }
            }
            _ => assert!(
                reported.is_empty(),
                "case {case:?}: no engine version here runs that mode, got {:?}",
                reported
                    .iter()
                    .map(|version| version.engine.clone())
                    .collect::<Vec<_>>()
            ),
        }
    }
}

#[tokio::test]
async fn a_filter_name_this_operation_does_not_support_is_refused() {
    let router = emulator();

    // Filter names are case-sensitive, and an unsupported one is refused rather than
    // ignored: ignoring it would report versions the caller excluded.
    for name in ["not-a-filter", "Engine", "engine-family"] {
        let case = format!("filter name {name}");
        expect_error(
            &case,
            call(
                &router,
                ACTION,
                &[
                    ("Filters.Filter.1.Name", name.to_string()),
                    ("Filters.Filter.1.Values.Value.1", "mysql".to_string()),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }
}

// ---------------------------------------------------------------------------
// MaxRecords and Marker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_records_is_bounded_and_the_marker_resumes_the_report() {
    let router = emulator();

    // "Constraints: Minimum 20, maximum 100" — each published bound, and one step past
    // each. A value outside the window is one the parameter does not accept, so it is
    // refused with `InvalidParameterValue`.
    for (case, value, expected) in [
        ("one below the published minimum", "19", 400),
        ("the published minimum", "20", 200),
        ("the published maximum", "100", 200),
        ("one above the published maximum", "101", 400),
    ] {
        let response = call(&router, ACTION, &[("MaxRecords", value.to_string())]).await;
        if expected == 200 {
            expect_ok(case, response);
        } else {
            expect_error(case, response, 400, "InvalidParameterValue");
        }
    }

    let expected: Vec<(String, String)> = walk_every_version(&router, "every version", 100)
        .await
        .iter()
        .map(|version| (version.engine.clone(), version.engine_version.clone()))
        .collect();

    // Paging at the smallest page size the operation accepts must visit every entry
    // exactly once, in the same order, and terminate — the per-page guards live in
    // `walk_every_version`.
    let seen: Vec<(String, String)> = walk_every_version(&router, "paged report", 20)
        .await
        .iter()
        .map(|version| (version.engine.clone(), version.engine_version.clone()))
        .collect();
    assert_eq!(
        seen, expected,
        "case \"paged report\": paging must visit every entry exactly once, in the \
         report's own order"
    );
    assert!(
        expected.len() > 20,
        "case \"paged report\": a page size of 20 must take more than one page, so the \
         report must carry more than 20 entries, got {}",
        expected.len()
    );
}

/// One page of the WHOLE report (`IncludeAll`, no narrowing) at `page_size`, resumed at
/// `marker`: the entries it reported and the `Marker` it carried.
///
/// `pub(crate)` because the catalogue-population suite walks the same report; the paging
/// guards every walk owes belong in one place.
pub(crate) async fn page_of_every_version(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
    marker: Option<&str>,
) -> (Vec<Version>, Option<String>) {
    page_of_report(
        router,
        case,
        page_size,
        marker,
        &[("IncludeAll", "true".to_string())],
    )
    .await
}

/// One page of the report `params` asks for, resumed at `marker`.
///
/// The published catalogue is larger than the biggest page this operation serves
/// (`MaxRecords` "maximum 100"), so a test that wants a WHOLE report — narrowed or not —
/// has to page it; this is the one page reader every such walk is built on.
async fn page_of_report(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
    marker: Option<&str>,
    params: &[(&str, String)],
) -> (Vec<Version>, Option<String>) {
    let mut paged: Vec<(&str, String)> = vec![("MaxRecords", page_size.to_string())];
    paged.extend(params.iter().cloned());
    if let Some(marker) = marker {
        paged.push(("Marker", marker.to_string()));
    }
    let (page, next) = describe(router, case, &paged).await;
    assert!(
        page.len() <= page_size,
        "case {case:?}: a page must hold at most MaxRecords={page_size} entries, got {}",
        page.len()
    );
    (page, next)
}

/// The whole report `params` asks for, walked one page of `page_size` at a time: a page
/// the report issued a `Marker` for carries entries, no page exceeds `MaxRecords`, and
/// the walk terminates.
pub(crate) async fn walk_report(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
    params: &[(&str, String)],
) -> Vec<Version> {
    let mut seen: Vec<Version> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (page, next) =
            page_of_report(router, case, page_size, marker.as_deref(), params).await;
        assert!(
            marker.is_none() || !page.is_empty(),
            "case {case:?}: a page the report issued a Marker for must carry entries"
        );
        seen.extend(page);
        pages += 1;
        assert!(
            pages < 500,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match next {
            Some(next) => marker = Some(next),
            None => break,
        }
    }
    seen
}

/// The whole report with nothing narrowed and deprecated entries included.
pub(crate) async fn walk_every_version(
    router: &MultiServiceRouter,
    case: &str,
    page_size: usize,
) -> Vec<Version> {
    walk_report(
        router,
        case,
        page_size,
        &[("IncludeAll", "true".to_string())],
    )
    .await
}

// ---------------------------------------------------------------------------
// The members a request asks for
// ---------------------------------------------------------------------------

#[tokio::test]
async fn character_sets_and_timezones_are_reported_only_when_the_request_asks() {
    let router = emulator();

    // (engine, the engine takes a CharacterSetName, the engine takes a Timezone, the
    // engine LISTS them when the request asks). `CreateDBInstance`'s `CharacterSetName`
    // is the Oracle character set and the SQL Server server collation, and its `Timezone`
    // is a SQL Server member; MySQL, MariaDB and PostgreSQL are the rows that must get
    // NEITHER even when they ask for both. The RDS Custom rows take both — they report a
    // `DefaultCharacterSet`, which no flag gates — but list neither, whatever the request
    // asks for: "For RDS Custom, the default is not to list supported character sets. If
    // you enable this parameter, RDS Custom returns no results"
    // (`ListSupportedCharacterSets`, and the same sentence on `ListSupportedTimezones`).
    let rows: [(&str, bool, bool, bool); 11] = [
        ("oracle-se2", true, false, true),
        ("oracle-ee", true, false, true),
        ("sqlserver-ee", true, true, true),
        ("sqlserver-web", true, true, true),
        ("mysql", false, false, true),
        ("mariadb", false, false, true),
        ("postgres", false, false, true),
        ("custom-sqlserver-ee", true, true, false),
        ("custom-sqlserver-se", true, true, false),
        ("custom-sqlserver-web", true, true, false),
        ("custom-sqlserver-dev", true, true, false),
    ];

    for (engine, has_character_sets, has_timezones, lists_them) in rows {
        let silent = all_versions_of(&router, engine).await;
        for version in &silent {
            assert!(
                subtrees(&version.scalars, "SupportedCharacterSets").is_empty(),
                "case {engine:?}: SupportedCharacterSets is reported only when \
                 ListSupportedCharacterSets asks for it, got {}",
                version.scalars
            );
            assert!(
                subtrees(&version.scalars, "SupportedTimezones").is_empty(),
                "case {engine:?}: SupportedTimezones is reported only when \
                 ListSupportedTimezones asks for it, got {}",
                version.scalars
            );
            // `DefaultCharacterSet` is not gated on the flag: it is the engine's own
            // default, reported for an engine that has one.
            assert_eq!(
                !subtrees(&version.scalars, "DefaultCharacterSet").is_empty(),
                has_character_sets,
                "case {engine:?}: DefaultCharacterSet must be present exactly for an \
                 engine that takes a CharacterSetName, got {}",
                version.scalars
            );
        }

        let case = format!("{engine} with both lists asked for");
        let (asked, _) = describe(
            &router,
            &case,
            &[
                ("Engine", engine.to_string()),
                ("IncludeAll", "true".to_string()),
                ("ListSupportedCharacterSets", "true".to_string()),
                ("ListSupportedTimezones", "true".to_string()),
            ],
        )
        .await;
        assert_eq!(
            asked.len(),
            silent.len(),
            "case {case:?}: asking for a member must not change WHICH versions are \
             reported"
        );
        for version in &asked {
            assert_eq!(
                !subtrees(&version.scalars, "SupportedCharacterSets").is_empty(),
                has_character_sets && lists_them,
                "case {case:?}: SupportedCharacterSets must be reported exactly for an \
                 engine that has them and lists them, got {}",
                version.scalars
            );
            assert_eq!(
                !subtrees(&version.scalars, "SupportedTimezones").is_empty(),
                has_timezones && lists_them,
                "case {case:?}: SupportedTimezones must be reported exactly for an \
                 engine that has them and lists them, got {}",
                version.scalars
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ValidUpgradeTarget
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_entrys_upgrade_targets_are_that_engines_later_versions() {
    let router = emulator();

    for engine in MODELLED_ENGINES {
        let all = all_versions_of(&router, engine).await;
        if all.is_empty() {
            continue;
        }
        // The major version each version belongs to, so a target can be classified as a
        // major or a minor upgrade without naming any version here.
        let majors: BTreeMap<String, String> = all
            .iter()
            .map(|version| {
                (
                    version.engine_version.clone(),
                    version.major_engine_version.clone(),
                )
            })
            .collect();

        for (index, version) in all.iter().enumerate() {
            let case = format!("{engine} {} upgrade targets", version.engine_version);
            let targets = subtrees(&version.xml, "UpgradeTarget");
            let reported: Vec<String> = targets
                .iter()
                .map(|target| text(target, "EngineVersion"))
                .collect();
            let later: Vec<String> = all
                .iter()
                .skip(index + 1)
                .map(|later| later.engine_version.clone())
                .collect();
            assert_eq!(
                reported, later,
                "case {case:?}: the upgrade targets must be exactly this engine's later \
                 versions, in the report's order"
            );
            for target in &targets {
                let target_version = text(target, "EngineVersion");
                let target_major = majors
                    .get(&target_version)
                    .expect("a target is one of the engine's own versions");
                assert_eq!(
                    text(target, "Engine"),
                    engine,
                    "case {case:?}: an upgrade target stays on the same engine, got \
                     {target}"
                );
                assert_eq!(
                    text(target, "IsMajorVersionUpgrade"),
                    (*target_major != version.major_engine_version).to_string(),
                    "case {case:?}: a target in another major version is a major \
                     version upgrade, got {target}"
                );
                if text(target, "AutoUpgrade") == "true" {
                    assert_eq!(
                        *target_major, version.major_engine_version,
                        "case {case:?}: an automatic upgrade never crosses a major \
                         version, got {target}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The capability members: what THIS version can do
// ---------------------------------------------------------------------------

/// The boolean capability members every reported entry carries. A client chooses an
/// engine version by gating on these, so every entry has to answer each of them rather
/// than leaving the caller to guess from the engine name.
const CAPABILITY_FLAGS: [&str; 9] = [
    "SupportsReadReplica",
    "SupportsLogExportsToCloudwatchLogs",
    "SupportsParallelQuery",
    "SupportsGlobalDatabases",
    "SupportsBabelfish",
    "SupportsLimitlessDatabase",
    "SupportsLocalWriteForwarding",
    "SupportsIntegrations",
    "SupportsCertificateRotationWithoutRestart",
];

/// The capability members that belong to Aurora's cluster engine: parallel query, global
/// databases, Babelfish, Limitless Database and local write forwarding. A version of an
/// RDS DB instance engine cannot claim one.
const AURORA_ONLY_FLAGS: [&str; 5] = [
    "SupportsParallelQuery",
    "SupportsGlobalDatabases",
    "SupportsBabelfish",
    "SupportsLimitlessDatabase",
    "SupportsLocalWriteForwarding",
];

/// The values of one list member of an entry: an awsQuery list of strings serializes its
/// entries as `<member>` elements inside the member's own element
/// (`smithy_query(query="shape",shape="CACertificateIdentifiersList")` — the list's
/// member carries no `xmlName`).
pub(crate) fn list_members(xml: &str, member: &str) -> Vec<String> {
    subtrees(xml, member)
        .first()
        .map(|subtree| texts(subtree, "member"))
        .unwrap_or_default()
}

/// One boolean member of an entry, asserted to be a boolean at all.
pub(crate) fn flag(case: &str, xml: &str, member: &str) -> bool {
    let value = optional_text(xml, member)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be reported, got {xml}"));
    match value.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: {member} must be a boolean, got {other:?} in {xml}"),
    }
}

/// One component of an engine version, so two versions compare by their NUMBERS rather
/// than as text: `10.11.9` is later than `10.5.28`, and `8.0.mysql_aurora.3.07.1` later
/// than `8.0.mysql_aurora.3.05.2`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Component {
    /// A numeric component, compared as a number.
    Number(u64),
    /// Anything else (`mysql_aurora`, `ru`, `v1`, `limitless`), compared as text and
    /// always after a number, so `16.4` precedes `16.4-limitless`.
    Text(String),
}

/// The sort key of an engine version: its `.`, `-` and `_` separated components.
pub(crate) fn version_key(version: &str) -> Vec<Component> {
    version
        .split(['.', '-', '_'])
        .map(|part| match part.parse::<u64>() {
            Ok(number) => Component::Number(number),
            Err(_) => Component::Text(part.to_string()),
        })
        .collect()
}

#[tokio::test]
async fn the_envelope_reports_the_published_per_engine_and_per_version_reference_data() {
    let router = emulator();

    // One row per (engine, version) whose published envelope data this table pins, with
    // the member values AWS publishes for it. The rows span the four kinds of member the
    // envelope carries reference data in — the engine's description prose, its exportable
    // log types, the version's feature names and its Aurora Serverless v2 limits — over
    // engines and versions that disagree about each.
    //
    // Sources, per member:
    //
    // * `ExportableLogTypes` — the log types `EnableCloudwatchLogsExports` takes for that
    //   engine (`smithy_query(query="doc",shape="CreateDBInstanceMessage",
    //   member="EnableCloudwatchLogsExports")` points at the per-engine published list).
    // * `SupportedFeatureNames` — the `FeatureName` values `AddRoleToDBInstance` /
    //   `AddRoleToDBCluster` take for that engine; an engine that associates a role with
    //   no feature reports the list EMPTY (`<SupportedFeatureNames/>`), not absent.
    // * `ServerlessV2FeaturesSupport` — `MinCapacity` is 0 for a version that supports
    //   Aurora Serverless v2 pause/resume ("if the minimum capacity is 0 ACUs, the engine
    //   version … supports the automatic pause/resume feature") and half an ACU for one
    //   that does not; `MaxCapacity` is one of the two the member documents ("might be
    //   `256` or `128`"). Both are doubles, so each is reported as a decimal.
    //
    // (case, engine, engine version, exportable log types, feature names, Serverless v2
    // limits as reported on the wire)
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        Option<(&'static str, &'static str)>,
    );
    const MYSQL_LOGS: &[&str] = &["audit", "error", "general", "slowquery"];
    const AURORA_POSTGRESQL_LOGS: &[&str] = &["iam-db-auth-error", "instance", "postgresql"];
    const POSTGRESQL_FEATURES: &[&str] = &["Lambda", "s3Export", "s3Import"];
    const AURORA_POSTGRESQL_FEATURES: &[&str] = &[
        "Bedrock",
        "Comprehend",
        "Lambda",
        "SageMaker",
        "s3Export",
        "s3Import",
    ];
    const AURORA_MYSQL_FEATURES: &[&str] =
        &["Bedrock", "Comprehend", "SageMaker", "s3Export", "s3Import"];
    let rows: Vec<Row> = vec![
        // RDS for MySQL and MariaDB associate a role with no feature at all.
        ("RDS for MySQL", "mysql", "8.0.40", MYSQL_LOGS, &[], None),
        ("RDS for MariaDB", "mariadb", "11.4.4", MYSQL_LOGS, &[], None),
        // RDS for PostgreSQL has the `aws_s3` and `aws_lambda` features, and no Aurora
        // Serverless v2 limits: it is not an Aurora engine.
        (
            "RDS for PostgreSQL",
            "postgres",
            "17.2",
            &["postgresql", "upgrade"],
            POSTGRESQL_FEATURES,
            None,
        ),
        // Aurora PostgreSQL: three exportable log types, the machine learning features
        // beside the S3 and Lambda ones, and the current Serverless v2 limits.
        (
            "Aurora PostgreSQL 16",
            "aurora-postgresql",
            "16.6",
            AURORA_POSTGRESQL_LOGS,
            AURORA_POSTGRESQL_FEATURES,
            Some(("0.0", "256.0")),
        ),
        (
            "Aurora PostgreSQL 13",
            "aurora-postgresql",
            "13.15",
            AURORA_POSTGRESQL_LOGS,
            AURORA_POSTGRESQL_FEATURES,
            Some(("0.0", "256.0")),
        ),
        // Aurora PostgreSQL 11 ran before Aurora Serverless v2 existed.
        (
            "Aurora PostgreSQL 11",
            "aurora-postgresql",
            "11.21",
            AURORA_POSTGRESQL_LOGS,
            AURORA_POSTGRESQL_FEATURES,
            None,
        ),
        // Aurora MySQL 3: the Serverless v2 limits are the VERSION's, and these three
        // versions disagree about both bounds.
        (
            "Aurora MySQL before the raised maximum",
            "aurora-mysql",
            "8.0.mysql_aurora.3.05.2",
            MYSQL_LOGS,
            AURORA_MYSQL_FEATURES,
            Some(("0.5", "128.0")),
        ),
        (
            "Aurora MySQL at the raised maximum",
            "aurora-mysql",
            "8.0.mysql_aurora.3.07.1",
            MYSQL_LOGS,
            AURORA_MYSQL_FEATURES,
            Some(("0.5", "256.0")),
        ),
        (
            "Aurora MySQL with pause/resume",
            "aurora-mysql",
            "8.0.mysql_aurora.3.08.0",
            MYSQL_LOGS,
            AURORA_MYSQL_FEATURES,
            Some(("0.0", "256.0")),
        ),
        // Aurora MySQL 2 is a cluster engine without Serverless v2.
        (
            "Aurora MySQL 2",
            "aurora-mysql",
            "5.7.mysql_aurora.2.11.6",
            MYSQL_LOGS,
            AURORA_MYSQL_FEATURES,
            None,
        ),
    ];

    for (case, engine, engine_version, log_types, features, serverless_v2) in rows {
        let (reported, _) = describe(
            &router,
            case,
            &[
                ("Engine", engine.to_string()),
                ("EngineVersion", engine_version.to_string()),
                ("IncludeAll", "true".to_string()),
            ],
        )
        .await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: one engine version is one entry, got {reported:?}"
        );
        let version = &reported[0];
        assert_eq!(
            list_members(&version.scalars, "ExportableLogTypes"),
            log_types,
            "case {case:?}: expected the published exportable log types, got {}",
            version.scalars
        );
        assert_eq!(
            list_members(&version.scalars, "SupportedFeatureNames"),
            features,
            "case {case:?}: expected the published feature names, got {}",
            version.scalars
        );
        // A list member with no entries is still REPORTED: an SDK reading it gets an
        // empty list rather than a null.
        assert!(
            version.scalars.contains("SupportedFeatureNames"),
            "case {case:?}: SupportedFeatureNames must be reported even when empty, got {}",
            version.scalars
        );
        match serverless_v2 {
            Some((min, max)) => {
                let limits = subtrees(&version.scalars, "ServerlessV2FeaturesSupport")
                    .first()
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: an Aurora Serverless v2 version reports its \
                             limits, got {}",
                            version.scalars
                        )
                    });
                assert_eq!(
                    (
                        text(&limits, "MinCapacity").as_str(),
                        text(&limits, "MaxCapacity").as_str()
                    ),
                    (min, max),
                    "case {case:?}: expected the published Serverless v2 limits, got {limits}"
                );
            }
            None => assert!(
                subtrees(&version.scalars, "ServerlessV2FeaturesSupport").is_empty(),
                "case {case:?}: a version that does not run Aurora Serverless v2 reports \
                 no limits at all, got {}",
                version.scalars
            ),
        }
    }

    // The engine's own description prose, which every version of that engine repeats, and
    // the version description built from it. (case, engine, engine version, the
    // DBEngineDescription, the DBEngineVersionDescription)
    let descriptions: [(&str, &str, &str, &str, &str); 5] = [
        (
            // Aurora PostgreSQL's version string IS the community PostgreSQL version it
            // is compatible with, which is how AWS describes it.
            "Aurora PostgreSQL",
            "aurora-postgresql",
            "16.6",
            "Aurora (PostgreSQL)",
            "Aurora PostgreSQL (Compatible with PostgreSQL 16.6)",
        ),
        (
            // An Aurora MySQL version string carries two releases — the MySQL release it
            // is compatible with and the Aurora release itself — and the description
            // states them in that order, as AWS publishes it for this engine. Both
            // major-version families are rows, because the rule is the engine's.
            "Aurora MySQL 2",
            "aurora-mysql",
            "5.7.mysql_aurora.2.11.1",
            "Aurora MySQL",
            "Aurora (MySQL 5.7) 2.11.1",
        ),
        (
            "Aurora MySQL 3",
            "aurora-mysql",
            "8.0.mysql_aurora.3.08.0",
            "Aurora MySQL",
            "Aurora (MySQL 8.0) 3.08.0",
        ),
        (
            "RDS for MySQL",
            "mysql",
            "8.0.40",
            "MySQL Community Edition",
            "MySQL 8.0.40",
        ),
        (
            "RDS for PostgreSQL",
            "postgres",
            "17.2",
            "PostgreSQL",
            "PostgreSQL 17.2",
        ),
    ];

    for (case, engine, engine_version, engine_description, version_description) in descriptions {
        let (reported, _) = describe(
            &router,
            case,
            &[
                ("Engine", engine.to_string()),
                ("EngineVersion", engine_version.to_string()),
                ("IncludeAll", "true".to_string()),
            ],
        )
        .await;
        let version = reported
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected one entry, got {reported:?}"));
        assert_eq!(
            text(&version.scalars, "DBEngineDescription"),
            engine_description,
            "case {case:?}: expected the published engine description, got {}",
            version.scalars
        );
        assert_eq!(
            text(&version.scalars, "DBEngineVersionDescription"),
            version_description,
            "case {case:?}: expected the published version description, got {}",
            version.scalars
        );
    }
}

#[tokio::test]
async fn every_entry_answers_every_capability_member_a_client_gates_on() {
    let router = emulator();

    for engine in MODELLED_ENGINES {
        for version in all_versions_of(&router, engine).await {
            let case = format!("{engine} {}", version.engine_version);
            for member in CAPABILITY_FLAGS {
                // Reading it is the assertion: `flag` refuses an absent member and a
                // value that is not a boolean.
                let _ = flag(&case, &version.scalars, member);
            }

            // "A list of the supported DB engine modes" — every version runs at least
            // one, and each is one of the five the `engine-mode` filter documents.
            let modes = list_members(&version.scalars, "SupportedEngineModes");
            assert!(
                !modes.is_empty(),
                "case {case:?}: a version must report the engine modes it runs, got {}",
                version.scalars
            );
            for mode in &modes {
                assert!(
                    ["global", "multimaster", "parallelquery", "provisioned", "serverless"]
                        .contains(&mode.as_str()),
                    "case {case:?}: {mode:?} is not one of the documented engine modes, \
                     got {}",
                    version.scalars
                );
            }

            // "A list of the supported CA certificate identifiers": a client needs one
            // to pass `CACertificateIdentifier` to CreateDBInstance, so every version
            // reports the CAs it accepts.
            let authorities = list_members(&version.scalars, "SupportedCACertificateIdentifiers");
            assert!(
                !authorities.is_empty(),
                "case {case:?}: a version must report the CA certificates it accepts, \
                 got {}",
                version.scalars
            );
            for authority in &authorities {
                assert!(
                    authority.starts_with("rds-ca-"),
                    "case {case:?}: an Amazon RDS CA identifier is named rds-ca-…, got \
                     {authority:?} in {}",
                    version.scalars
                );
            }

            // `ExportableLogTypes` and `SupportsLogExportsToCloudwatchLogs` answer the
            // same question, so they must agree.
            let log_types = list_members(&version.scalars, "ExportableLogTypes");
            assert_eq!(
                flag(&case, &version.scalars, "SupportsLogExportsToCloudwatchLogs"),
                !log_types.is_empty(),
                "case {case:?}: SupportsLogExportsToCloudwatchLogs must agree with \
                 ExportableLogTypes {log_types:?}, got {}",
                version.scalars
            );

            // Every feature name is a name: an empty list is a version with no
            // engine-integration feature, an empty NAME is a report defect.
            for feature in list_members(&version.scalars, "SupportedFeatureNames") {
                assert!(
                    !feature.trim().is_empty(),
                    "case {case:?}: a reported feature name must not be empty, got {}",
                    version.scalars
                );
            }
        }
    }
}

#[tokio::test]
async fn a_capability_is_the_versions_own_and_not_its_engines() {
    let router = emulator();

    // The capability members of one entry, as the signature two versions of one engine
    // are compared by.
    let signature = |version: &Version| -> String {
        let flags: Vec<String> = CAPABILITY_FLAGS
            .iter()
            .map(|member| {
                format!(
                    "{member}={}",
                    optional_text(&version.scalars, member).unwrap_or_default()
                )
            })
            .collect();
        format!(
            "{}|{}|{:?}",
            flags.join(","),
            list_members(&version.scalars, "SupportedEngineModes").join("+"),
            optional_text(&version.scalars, "MaxCapacity")
        )
    };

    let mut engines_whose_versions_differ = 0usize;
    let mut serverless_v1_engines: BTreeSet<String> = BTreeSet::new();

    for engine in MODELLED_ENGINES {
        let all = all_versions_of(&router, engine).await;
        if all.is_empty() {
            continue;
        }
        let is_aurora = engine.starts_with("aurora");

        for version in &all {
            let case = format!("{engine} {}", version.engine_version);

            // Aurora's cluster capabilities are Aurora's: an RDS DB instance engine
            // version reports none of them.
            if !is_aurora {
                for member in AURORA_ONLY_FLAGS {
                    assert!(
                        !flag(&case, &version.scalars, member),
                        "case {case:?}: {member} is an Aurora cluster capability, so a \
                         version of an RDS engine must report false, got {}",
                        version.scalars
                    );
                }
                assert!(
                    subtrees(&version.scalars, "ServerlessV2FeaturesSupport").is_empty(),
                    "case {case:?}: Aurora Serverless v2 limits belong to an Aurora \
                     version, got {}",
                    version.scalars
                );
            } else {
                // An Aurora reader is a member of its cluster, not an RDS read replica,
                // so an Aurora version reports no read replica support.
                assert!(
                    !flag(&case, &version.scalars, "SupportsReadReplica"),
                    "case {case:?}: an Aurora version serves cluster readers rather than \
                     read replicas, so SupportsReadReplica must be false, got {}",
                    version.scalars
                );
            }

            // "Specifies any Aurora Serverless v2 properties or limits that differ
            // between Aurora engine versions": when reported, the limits are a usable
            // capacity range. The minimum may be ZERO — "if the minimum capacity is 0
            // ACUs, the engine version … supports the automatic pause/resume feature of
            // Aurora Serverless v2" — and the maximum is one of the two the member
            // documents ("might be 256 or 128").
            if let Some(limits) = subtrees(&version.scalars, "ServerlessV2FeaturesSupport").first()
            {
                let min: f64 = text(limits, "MinCapacity")
                    .parse()
                    .expect("MinCapacity is a number");
                let max: f64 = text(limits, "MaxCapacity")
                    .parse()
                    .expect("MaxCapacity is a number");
                assert!(
                    min >= 0.0 && min < max,
                    "case {case:?}: the Serverless v2 capacity range must be usable, got \
                     {min}..{max} in {}",
                    version.scalars
                );
                let reported_max = text(limits, "MaxCapacity");
                assert!(
                    matches!(reported_max.as_str(), "256.0" | "128.0"),
                    "case {case:?}: the Serverless v2 maximum is one of the two published \
                     capacities, as the decimal a double is on the wire, got \
                     {reported_max:?} in {}",
                    version.scalars
                );
            }

            // Aurora Serverless v1 is a property of the VERSION: only an Aurora version
            // offers it at all.
            if list_members(&version.scalars, "SupportedEngineModes")
                .iter()
                .any(|mode| mode == "serverless")
            {
                assert!(
                    is_aurora,
                    "case {case:?}: only an Aurora version runs the serverless engine \
                     mode, got {}",
                    version.scalars
                );
                serverless_v1_engines.insert(engine.to_string());
                assert!(
                    all.iter().any(|other| !list_members(
                        &other.scalars,
                        "SupportedEngineModes"
                    )
                    .iter()
                    .any(|mode| mode == "serverless")),
                    "case {case:?}: the engine mode is the version's, so an engine with a \
                     Serverless v1 version must also report versions without it"
                );
            }
        }

        // Two versions of one engine may disagree about a capability — that is what
        // makes these members the VERSION's.
        let signatures: BTreeSet<String> = all.iter().map(signature).collect();
        if signatures.len() > 1 {
            engines_whose_versions_differ += 1;
        }
    }

    assert!(
        engines_whose_versions_differ >= 2,
        "the capability members are per version, so the report must carry engines whose \
         versions disagree about one, got {engines_whose_versions_differ}"
    );
    assert!(
        !serverless_v1_engines.is_empty(),
        "the serverless engine mode must be reported by the Aurora versions that run \
         Aurora Serverless v1, got none"
    );
}

#[tokio::test]
async fn an_upgrade_target_reports_the_capabilities_of_the_version_it_names() {
    let router = emulator();

    // A caller decides whether to take an upgrade by what it would be RUNNING
    // afterwards, so each target's capability members are the target version's own.
    let shared: [&str; 5] = [
        "SupportsParallelQuery",
        "SupportsGlobalDatabases",
        "SupportsBabelfish",
        "SupportsLimitlessDatabase",
        "SupportsLocalWriteForwarding",
    ];

    for engine in MODELLED_ENGINES {
        let all = all_versions_of(&router, engine).await;
        let by_version: BTreeMap<String, Version> = all
            .iter()
            .map(|version| (version.engine_version.clone(), version.clone()))
            .collect();

        for version in &all {
            for target in subtrees(&version.xml, "UpgradeTarget") {
                let target_version = text(&target, "EngineVersion");
                let case = format!("{engine} {} -> {target_version}", version.engine_version);
                let entry = by_version
                    .get(&target_version)
                    .expect("a target is one of the engine's own versions");
                for member in shared {
                    assert_eq!(
                        optional_text(&target, member),
                        optional_text(&entry.scalars, member),
                        "case {case:?}: {member} must be the TARGET version's, got \
                         {target}"
                    );
                }
                assert_eq!(
                    list_members(&target, "SupportedEngineModes"),
                    list_members(&entry.scalars, "SupportedEngineModes"),
                    "case {case:?}: SupportedEngineModes must be the TARGET version's, \
                     got {target}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_report_carries_several_versions_per_engine_in_ascending_order() {
    let router = emulator();

    let (all, _) = describe(
        &router,
        "every version",
        &[("IncludeAll", "true".to_string())],
    )
    .await;

    // The report is grouped by engine: an engine's entries are contiguous, so a client
    // reading a page sees whole engine families rather than interleaved ones.
    let mut seen_engines: Vec<String> = Vec::new();
    for version in &all {
        if seen_engines.last() != Some(&version.engine) {
            assert!(
                !seen_engines.contains(&version.engine),
                "case \"every version\": the entries of {:?} must be contiguous, got the \
                 engine again after {:?}",
                version.engine,
                seen_engines
            );
            seen_engines.push(version.engine.clone());
        }
    }

    for engine in MODELLED_ENGINES {
        let reported = all_versions_of(&router, engine).await;
        if reported.is_empty() {
            continue;
        }

        // A catalogue, not one synthesized entry: every engine carries several versions,
        // and the earliest of them can be upgraded to a later one.
        assert!(
            reported.len() > 1,
            "case {engine:?}: the report must carry the engine's versions, got only {:?}",
            reported
                .iter()
                .map(|version| version.engine_version.clone())
                .collect::<Vec<_>>()
        );
        assert!(
            !subtrees(&reported[0].xml, "UpgradeTarget").is_empty(),
            "case {engine:?}: the earliest version must have somewhere to upgrade to, \
             got {}",
            reported[0].xml
        );

        // …and they are reported in ascending version order, which is the order the
        // Marker pages through.
        for pair in reported.windows(2) {
            let (earlier, later) = (&pair[0], &pair[1]);
            assert!(
                version_key(&earlier.engine_version) < version_key(&later.engine_version),
                "case {engine:?}: the report must ascend by version, got {:?} before {:?}",
                earlier.engine_version,
                later.engine_version
            );
        }
    }
}

