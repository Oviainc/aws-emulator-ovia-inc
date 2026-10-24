// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDBMajorEngineVersions`, the major-version report.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (`crate::db_instances::emulator`), each
//! request is a real awsQuery form POST carrying the SigV4 credential scope the router
//! keys on, and every assertion reads the XML the emulator answered with.
//!
//! The rules under test are stated over TABLES and over the service's OWN engine-version
//! report rather than over one recorded request:
//!
//! 1. The report is the distinct `(Engine, MajorEngineVersion)` pairs of the engine
//!    version catalogue `DescribeDBEngineVersions` publishes, each once, in that
//!    catalogue's order — so the expectation is READ from that report rather than
//!    hand-listed, and every engine it carries is a row.
//! 2. `Engine` and `MajorEngineVersion` narrow that report and refuse nothing: the table
//!    spans an engine alone, a major version alone, both together, a name in another
//!    case, and three values that select nothing (an engine whose versions only an
//!    account mints, an engine of another control plane, and a major version no entry
//!    carries).
//! 3. A support lifecycle is published only for the open-source engines, with Amazon RDS
//!    Extended Support only for the four that are offered it; the windows are whole UTC
//!    days, standard support opens before it closes, and Extended Support opens the day
//!    after standard support closes and runs the three years AWS offers it for.
//! 4. `MaxRecords` holds its published bounds, the `Marker` walks every entry exactly
//!    once, and a `Marker` no prior page issued is refused.
//! 5. The report is a READ: two identical calls answer identically.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::walk_every_version;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text,
};

/// The operation under test.
const ACTION: &str = "DescribeDBMajorEngineVersions";

/// The engines a support lifecycle is published for: `SupportedEngineLifecycle` "only
/// returns information for the open source engines Amazon RDS for MariaDB, Amazon RDS for
/// MySQL, Amazon RDS for PostgreSQL, Aurora MySQL, and Aurora PostgreSQL"
/// (`smithy_query(query="doc",shape="SupportedEngineLifecycle")`).
const OPEN_SOURCE_ENGINES: [&str; 5] = [
    "aurora-mysql",
    "aurora-postgresql",
    "mariadb",
    "mysql",
    "postgres",
];

/// The one open-source engine offered no Amazon RDS Extended Support:
/// `LifecycleSupportName` states that "for Amazon RDS for MariaDB, this parameter only
/// returns the value `open-source-rds-standard-support`".
const STANDARD_SUPPORT_ONLY_ENGINE: &str = "mariadb";

/// The engines this one `rds` front door serves on behalf of ANOTHER control plane, whose
/// models declare no `DescribeDBMajorEngineVersions` at all — so this report carries none
/// of their families even though the engine-version report carries their versions.
const OTHER_CONTROL_PLANE_ENGINES: [&str; 2] = ["docdb", "neptune"];

/// The two values `LifecycleSupportName` publishes
/// (`smithy_query(query="shape",shape="LifecycleSupportName")`).
const STANDARD_SUPPORT: &str = "open-source-rds-standard-support";
const EXTENDED_SUPPORT: &str = "open-source-rds-extended-support";

/// Seconds in one day, which the published calendar is written in.
const DAY: i64 = 24 * 60 * 60;

/// The days in the three years Amazon RDS Extended Support runs for past the end of
/// standard support: three calendar years are 1095 days, or 1096 when a 29 February falls
/// inside them.
const EXTENDED_SUPPORT_DAYS: std::ops::RangeInclusive<i64> = 1095..=1096;

/// One reported `<SupportedEngineLifecycle>`.
#[derive(Debug, Clone)]
struct Lifecycle {
    name: String,
    start: String,
    end: String,
}

/// One reported `<DBMajorEngineVersion>`.
#[derive(Debug, Clone)]
struct Family {
    engine: String,
    major_engine_version: String,
    lifecycles: Vec<Lifecycle>,
}

impl Family {
    /// The `(Engine, MajorEngineVersion)` pair this entry names.
    fn pair(&self) -> (String, String) {
        (self.engine.clone(), self.major_engine_version.clone())
    }
}

/// Every `<DBMajorEngineVersion>` in a response body, in the order it was reported.
///
/// A lifecycle window carries no `<Engine>` or `<MajorEngineVersion>` of its own, so the
/// two member reads over the whole entry are unambiguous.
fn reported(xml: &str) -> Vec<Family> {
    subtrees(xml, "DBMajorEngineVersion")
        .into_iter()
        .map(|entry| Family {
            engine: text(&entry, "Engine"),
            major_engine_version: text(&entry, "MajorEngineVersion"),
            lifecycles: subtrees(&entry, "SupportedEngineLifecycle")
                .into_iter()
                .map(|window| Lifecycle {
                    name: text(&window, "LifecycleSupportName"),
                    start: text(&window, "LifecycleSupportStartDate"),
                    end: text(&window, "LifecycleSupportEndDate"),
                })
                .collect(),
        })
        .collect()
}

/// One `DescribeDBMajorEngineVersions` call: the families it reported and the `Marker` it
/// carried.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(&str, String)],
) -> (Vec<Family>, Option<String>) {
    let body = expect_ok(case, call(router, ACTION, params).await);
    (reported(&body), optional_text(&body, "Marker"))
}

/// The whole report, walked one page of `page_size` at a time: a page the report issued a
/// `Marker` for carries entries, no page exceeds `MaxRecords`, and the walk terminates.
async fn walk(router: &MultiServiceRouter, case: &str, page_size: usize) -> Vec<Family> {
    let mut seen: Vec<Family> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params: Vec<(&str, String)> = vec![("MaxRecords", page_size.to_string())];
        if let Some(marker) = &marker {
            params.push(("Marker", marker.clone()));
        }
        let (page, next) = describe(router, case, &params).await;
        assert!(
            !page.is_empty(),
            "case {case:?}: a page the report issued a Marker for must carry entries"
        );
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page must hold at most MaxRecords={page_size} entries, got {}",
            page.len()
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

/// The `(Engine, MajorEngineVersion)` pairs the ENGINE-VERSION report publishes, deduped
/// in its own order and without the engines of another control plane: the expectation this
/// report owes, read from the service rather than hand-listed.
async fn catalogued_families(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for version in walk_every_version(router, "every engine version", 20).await {
        if OTHER_CONTROL_PLANE_ENGINES.contains(&version.engine.as_str()) {
            continue;
        }
        let pair = (version.engine.clone(), version.major_engine_version.clone());
        if !found.contains(&pair) {
            found.push(pair);
        }
    }
    found
}

/// A reported timestamp as epoch seconds, which is what the window arithmetic below is
/// stated in.
fn instant(case: &str, member: &str, value: &str) -> i64 {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| {
            panic!("case {case:?}: {member} must be an ISO-8601 instant, got {value:?}")
        })
        .secs()
}

#[tokio::test]
async fn the_report_is_the_catalogues_major_version_families() {
    let router = emulator();

    let expected = catalogued_families(&router).await;
    assert!(
        expected.len() > 20,
        "case \"every major engine version\": the catalogue must publish more families \
         than one page holds, so paging is exercised, got {}",
        expected.len()
    );

    let seen: Vec<(String, String)> = walk(&router, "every major engine version", 20)
        .await
        .iter()
        .map(Family::pair)
        .collect();
    assert_eq!(
        seen, expected,
        "case \"every major engine version\": the report must carry each \
         (Engine, MajorEngineVersion) pair of the engine-version catalogue exactly once, \
         in that catalogue's order"
    );

    // The engines of another control plane carry versions in the engine-version report and
    // no family here; asserted directly so the exclusion is not read out of `expected`
    // alone.
    for engine in OTHER_CONTROL_PLANE_ENGINES {
        assert!(
            !seen.iter().any(|(reported, _)| reported == engine),
            "case {engine:?}: an engine of another control plane must report no major \
             engine version, got {seen:?}"
        );
    }
}

/// One narrowing request: the members it carries, and whether it must select anything.
struct Narrowing {
    case: &'static str,
    engine: Option<&'static str>,
    major_engine_version: Option<&'static str>,
    /// Whether this request must select at least one family. A row that selects none says
    /// so here, which is what keeps an empty answer from passing a row meant to narrow.
    selects: bool,
}

/// The narrowing table: each member alone, both together, another case, and the values
/// that select nothing.
const NARROWINGS: &[Narrowing] = &[
    Narrowing {
        case: "every family of one engine",
        engine: Some("mysql"),
        major_engine_version: None,
        selects: true,
    },
    Narrowing {
        case: "one family of one engine",
        engine: Some("postgres"),
        major_engine_version: Some("16"),
        selects: true,
    },
    Narrowing {
        case: "an engine offered standard support only",
        engine: Some("mariadb"),
        major_engine_version: None,
        selects: true,
    },
    Narrowing {
        case: "an Oracle multi-tenant family",
        engine: Some("oracle-ee-cdb"),
        major_engine_version: Some("21"),
        selects: true,
    },
    Narrowing {
        case: "a SQL Server family, whose major version is a release",
        engine: Some("sqlserver-ee"),
        major_engine_version: Some("14.00"),
        selects: true,
    },
    Narrowing {
        case: "a Db2 edition",
        engine: Some("db2-ae"),
        major_engine_version: None,
        selects: true,
    },
    Narrowing {
        case: "an Aurora engine, whose families are cluster engines",
        engine: Some("aurora-postgresql"),
        major_engine_version: Some("15"),
        selects: true,
    },
    Narrowing {
        case: "an engine named in another case",
        engine: Some("MySQL"),
        major_engine_version: Some("8.0"),
        selects: true,
    },
    Narrowing {
        case: "a major version alone, which several engines publish",
        engine: None,
        major_engine_version: Some("19"),
        selects: true,
    },
    Narrowing {
        case: "an engine whose versions only an account mints",
        engine: Some("custom-oracle-ee"),
        major_engine_version: None,
        selects: false,
    },
    Narrowing {
        case: "an engine of another control plane",
        engine: Some("docdb"),
        major_engine_version: None,
        selects: false,
    },
    Narrowing {
        case: "a major version no entry of that engine carries",
        engine: Some("mysql"),
        major_engine_version: Some("9.9"),
        selects: false,
    },
    Narrowing {
        case: "a value that is not an engine",
        engine: Some("not-an-engine"),
        major_engine_version: None,
        selects: false,
    },
];

#[tokio::test]
async fn engine_and_major_engine_version_narrow_the_report() {
    let router = emulator();
    let whole = walk(&router, "every major engine version", 100).await;

    for row in NARROWINGS {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(engine) = row.engine {
            params.push(("Engine", engine.to_string()));
        }
        if let Some(major) = row.major_engine_version {
            params.push(("MajorEngineVersion", major.to_string()));
        }
        // The expectation is the whole report under the same rule: the entries whose
        // members match the values the request named, case-insensitively, in the report's
        // own order.
        let expected: Vec<(String, String)> = whole
            .iter()
            .filter(|family| {
                row.engine
                    .is_none_or(|engine| family.engine.eq_ignore_ascii_case(engine))
                    && row.major_engine_version.is_none_or(|major| {
                        family.major_engine_version.eq_ignore_ascii_case(major)
                    })
            })
            .map(Family::pair)
            .collect();
        assert_eq!(
            !expected.is_empty(),
            row.selects,
            "case {:?}: the table says this request selects {}, but the whole report \
             matches {expected:?}",
            row.case,
            row.selects
        );

        let (page, marker) = describe(&router, row.case, &params).await;
        let seen: Vec<(String, String)> = page.iter().map(Family::pair).collect();
        assert_eq!(
            seen, expected,
            "case {:?}: the narrowed report must be the whole report's matching entries, \
             in its order",
            row.case
        );
        assert_eq!(
            marker, None,
            "case {:?}: a narrowed report that fits in one page must carry no Marker, got \
             {marker:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_open_source_family_reports_its_support_lifecycles() {
    let router = emulator();

    for family in walk(&router, "every major engine version", 100).await {
        let case = format!("{} {}", family.engine, family.major_engine_version);
        let names: Vec<&str> = family
            .lifecycles
            .iter()
            .map(|window| window.name.as_str())
            .collect();
        let open_source = OPEN_SOURCE_ENGINES.contains(&family.engine.as_str());
        let expected_names: Vec<&str> = if !open_source {
            Vec::new()
        } else if family.engine == STANDARD_SUPPORT_ONLY_ENGINE {
            vec![STANDARD_SUPPORT]
        } else {
            vec![STANDARD_SUPPORT, EXTENDED_SUPPORT]
        };
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected the lifecycle names {expected_names:?}, got {names:?}"
        );

        for window in &family.lifecycles {
            let start = instant(&case, "LifecycleSupportStartDate", &window.start);
            let end = instant(&case, "LifecycleSupportEndDate", &window.end);
            // AWS publishes a support calendar in DAYS, so every bound of it is a whole
            // UTC day.
            for (member, value, secs) in [
                ("LifecycleSupportStartDate", &window.start, start),
                ("LifecycleSupportEndDate", &window.end, end),
            ] {
                assert_eq!(
                    secs % DAY,
                    0,
                    "case {case:?}: {member} must be a whole UTC day, got {value:?}"
                );
            }
            assert!(
                start < end,
                "case {case:?}: the {} window must open before it closes, got {:?} and {:?}",
                window.name,
                window.start,
                window.end
            );
        }

        if !open_source {
            continue;
        }
        let standard = family
            .lifecycles
            .iter()
            .find(|window| window.name == STANDARD_SUPPORT)
            .unwrap_or_else(|| panic!("case {case:?}: checked just above"));
        let standard_end = instant(&case, "LifecycleSupportEndDate", &standard.end);
        if let Some(extended) = family
            .lifecycles
            .iter()
            .find(|window| window.name == EXTENDED_SUPPORT)
        {
            let extended_start = instant(&case, "LifecycleSupportStartDate", &extended.start);
            assert_eq!(
                extended_start,
                standard_end + DAY,
                "case {case:?}: Extended Support must open the day after standard \
                 support closes, got {:?} against {:?}",
                extended.start,
                standard.end
            );
            let extended_end = instant(&case, "LifecycleSupportEndDate", &extended.end);
            let days = (extended_end - standard_end) / DAY;
            assert!(
                EXTENDED_SUPPORT_DAYS.contains(&days),
                "case {case:?}: Extended Support must run the three years past the end of \
                 standard support ({EXTENDED_SUPPORT_DAYS:?} days), got {days} between \
                 {:?} and {:?}",
                standard.end,
                extended.end
            );
        }
    }
}

#[tokio::test]
async fn max_records_is_bounded_and_the_marker_resumes_the_report() {
    let router = emulator();

    // "The maximum number of records to include in the response … Minimum 20, maximum
    // 100" — each published bound, and one step past each. The bound is a MODELLED range
    // on this operation's `MaxRecords`, so a value past it is refused at the boundary with
    // the constraint validator's own `ValidationError`, before this handler runs.
    for (case, value, status) in [
        ("one below the published minimum", "19", 400),
        ("the published minimum", "20", 200),
        ("the published maximum", "100", 200),
        ("one above the published maximum", "101", 400),
    ] {
        let response = call(&router, ACTION, &[("MaxRecords", value.to_string())]).await;
        if status == 200 {
            expect_ok(case, response);
        } else {
            let body = expect_error(case, response, status, "ValidationError");
            assert!(
                text(&body, "Message").contains("maxRecords"),
                "case {case:?}: the refusal must name the member it refused, got {body}"
            );
        }
    }

    // A `Marker` no prior page issued is refused rather than resynchronised onto a page
    // the caller cannot place.
    expect_error(
        "a Marker no prior page issued",
        call(
            &router,
            ACTION,
            &[("Marker", "not-a-marker-this-report-issued".to_string())],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // Paging at the smallest page size the operation accepts visits every entry exactly
    // once, in the same order as the unpaged report.
    let (whole, marker) = describe(&router, "the default page", &[]).await;
    assert_eq!(
        marker, None,
        "case \"the default page\": the whole report must fit in the default page size, \
         got the Marker {marker:?}"
    );
    let expected: Vec<(String, String)> = whole.iter().map(Family::pair).collect();
    let paged: Vec<(String, String)> = walk(&router, "the paged report", 20)
        .await
        .iter()
        .map(Family::pair)
        .collect();
    assert_eq!(
        paged, expected,
        "case \"the paged report\": paging must visit every entry exactly once, in the \
         report's own order"
    );
}

#[tokio::test]
async fn the_report_is_a_read_and_answers_the_same_twice() {
    let router = emulator();

    let params: Vec<(&str, String)> = vec![("Engine", "mysql".to_string())];
    let first = expect_ok("the first read", call(&router, ACTION, &params).await);
    let second = expect_ok("the second read", call(&router, ACTION, &params).await);
    assert_eq!(
        reported(&first)
            .iter()
            .map(Family::pair)
            .collect::<Vec<_>>(),
        reported(&second)
            .iter()
            .map(Family::pair)
            .collect::<Vec<_>>(),
        "case \"two reads\": a report must not change what a later caller sees"
    );
    assert_eq!(
        reported(&first)
            .iter()
            .flat_map(|family| family.lifecycles.iter().map(|window| window.end.clone()))
            .collect::<Vec<_>>(),
        reported(&second)
            .iter()
            .flat_map(|family| family.lifecycles.iter().map(|window| window.end.clone()))
            .collect::<Vec<_>>(),
        "case \"two reads\": a published support calendar must not move between reads"
    );
}
