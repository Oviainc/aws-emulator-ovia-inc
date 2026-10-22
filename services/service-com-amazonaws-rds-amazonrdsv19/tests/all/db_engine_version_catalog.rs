// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT the `DescribeDBEngineVersions` catalogue publishes: which
//! engines carry versions at all, the published form of an engine version, and the
//! members an entry reports for the engine family it belongs to.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (`crate::db_instances::emulator`), each
//! request is a real awsQuery form POST carrying the SigV4 credential scope the router
//! keys on, and every assertion reads the XML the emulator answered with.
//!
//! The rules under test, each asserted over a TABLE rather than over one recorded
//! request:
//!
//! 1. An engine whose engine versions AWS itself publishes is catalogue data: the report
//!    carries at least one version for every engine the model's own `Engine` value lists
//!    name, RDS Custom for SQL Server included, and carries none only for an engine whose
//!    versions exist solely as records `CreateCustomDBEngineVersion` mints from
//!    customer-supplied installation media (RDS Custom for Oracle). The table is the
//!    whole published engine list, so engines no issue cited are rows too.
//! 2. An engine version is published in the form its engine family uses — the Db2
//!    special build and revision, the Oracle release update, the SQL Server build with
//!    its CEV revision, the Aurora MySQL compatibility version, and the
//!    `<community version>-rds.YYYYMMDD` build of a major version in RDS Extended
//!    Support.
//! 3. An entry reports the character sets, NCHAR character sets and time zones
//!    `CreateDBInstance` takes for ITS engine, under the flags that ask for them — and an
//!    RDS Custom engine version lists none of them, because "for RDS Custom, the default
//!    is not to list supported character sets. If you enable this parameter, RDS Custom
//!    returns no results" (`ListSupportedCharacterSets`, and the same sentence on
//!    `ListSupportedTimezones`), while still naming its `DefaultCharacterSet`.
//! 4. A version this report publishes as available is one `CreateDBInstance` provisions:
//!    the catalogue is read back through the instance it creates.

use service_com_amazonaws_rds_amazonrdsv19::state::DB_INSTANCE_ENGINES;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::{all_versions_of, versions, Version, CUSTOM_ENGINES};
use crate::db_instances::{call, emulator, expect_ok, subtrees, text, texts};
use crate::engine_version_families::{dated_build, is_build_date};

/// The operation under test.
const ACTION: &str = "DescribeDBEngineVersions";

/// A DB instance class every engine in this suite can be ordered in.
const A_CLASS: &str = "db.m5.large";

/// The `.`-separated segments of a version string, which is what every published form
/// below is written over.
fn segments(version: &str) -> Vec<&str> {
    version.split('.').collect()
}

/// Whether `segment` is a decimal number.
fn is_number(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether every segment is a number.
fn all_numbers(segments: &[&str]) -> bool {
    !segments.is_empty() && segments.iter().copied().all(is_number)
}

/// Whether `segment` is `tag` followed by a number (`r1`, `v1`, `sb00080714`).
fn is_tagged_number(segment: &str, tag: &str) -> bool {
    segment.strip_prefix(tag).is_some_and(is_number)
}

/// Whether `segment` is `tag` followed by a `YYYY-MM` stamp (`ru-2024-10`).
fn is_tagged_month(segment: &str, tag: &str) -> bool {
    segment.strip_prefix(tag).is_some_and(|stamp| {
        stamp
            .split_once('-')
            .is_some_and(|(year, month)| year.len() == 4 && is_number(year) && is_number(month))
    })
}

/// Whether `version` is a community release of `parts` numeric components, optionally
/// carrying the `-rds.YYYYMMDD` stamp of an Amazon RDS Extended Support build (the halves
/// `engine_version_families` reads).
fn is_community_release(version: &str, parts: usize) -> bool {
    let (base, date) = match dated_build(version) {
        Some((base, date)) => (base, Some(date)),
        None => (version, None),
    };
    if date.is_some_and(|date| !is_build_date(date)) {
        return false;
    }
    let base = segments(base);
    base.len() == parts && all_numbers(&base)
}

/// A Db2 engine version: the Db2 `V.R.M.F` version with the special build it carries and
/// its RDS revision (`11.5.9.0.sb00000000.r1`, `12.1.4.0.sb00080714.r1`).
fn is_db2_form(version: &str) -> bool {
    let parts = segments(version);
    parts.len() == 6
        && all_numbers(&parts[..4])
        && is_tagged_number(parts[4], "sb")
        && is_tagged_number(parts[5], "r")
}

/// A Microsoft SQL Server engine version, an RDS Custom CEV included: the SQL Server
/// build with its `.vN` revision (`14.00.3465.1.v1`, `15.00.4073.23.v1`).
fn is_sqlserver_form(version: &str) -> bool {
    let parts = segments(version);
    (5..=6).contains(&parts.len())
        && all_numbers(&parts[..parts.len() - 1])
        && is_tagged_number(parts[parts.len() - 1], "v")
}

/// An Oracle engine version: the release, then the release update and revision it carries
/// (`19.0.0.0.ru-2024-10.rur-2024-10.r1`).
fn is_oracle_form(version: &str) -> bool {
    let parts = segments(version);
    parts.len() == 7
        && all_numbers(&parts[..4])
        && is_tagged_month(parts[4], "ru-")
        && is_tagged_month(parts[5], "rur-")
        && is_tagged_number(parts[6], "r")
}

/// An Aurora MySQL engine version: the MySQL version it is compatible with, then the
/// Aurora version itself (`5.7.mysql_aurora.2.11.1`).
fn is_aurora_mysql_form(version: &str) -> bool {
    let parts = segments(version);
    parts.len() == 6
        && all_numbers(&parts[..2])
        && parts[2] == "mysql_aurora"
        && all_numbers(&parts[3..])
}

/// An Aurora PostgreSQL engine version: the PostgreSQL version, with the Limitless
/// Database suffix on the version that runs it (`16.6`, `16.4-limitless`).
fn is_aurora_postgresql_form(version: &str) -> bool {
    is_community_release(version.strip_suffix("-limitless").unwrap_or(version), 2)
}

/// A DocumentDB engine version (`5.0.0`).
fn is_docdb_form(version: &str) -> bool {
    is_community_release(version, 3)
}

/// A Neptune engine version (`1.3.2.0`).
fn is_neptune_form(version: &str) -> bool {
    is_community_release(version, 4)
}

/// A three-part community release: MySQL and MariaDB (`8.4.4`, `5.7.44-rds.20250213`).
fn is_three_part_release(version: &str) -> bool {
    is_community_release(version, 3)
}

/// A two-part community release: PostgreSQL (`17.2`, `11.22-rds.20241121`).
fn is_two_part_release(version: &str) -> bool {
    is_community_release(version, 2)
}

/// One engine family's published version form: the engine-name prefix of the family, the
/// predicate a version of that family satisfies, and a version that is NOT in the form,
/// so each predicate is asserted to REFUSE as well as to accept.
struct Form {
    prefix: &'static str,
    matches: fn(&str) -> bool,
    counter_example: &'static str,
}

/// One row of the table below, so it reads as a table.
const fn form(
    prefix: &'static str,
    matches: fn(&str) -> bool,
    counter_example: &'static str,
) -> Form {
    Form {
        prefix,
        matches,
        counter_example,
    }
}

/// The published form of an engine version, by the engine-name prefix of its family: the
/// row whose prefix the engine starts with is the form that engine's versions must
/// satisfy, and an engine matching NO row is a report defect — the form of a published
/// engine version is a thing a client parses. The prefixes are disjoint under
/// `starts_with` (an engine name begins with its OWN family's prefix: `aurora-mysql` does
/// not start with `mysql`, nor `custom-sqlserver-ee` with `sqlserver-`), and the test
/// below asserts that, so the table's order carries no meaning. The counter-examples are
/// a Db2 version without the special build RDS publishes it with, a CEV without its
/// revision, an Oracle release without its release update, an Aurora MySQL version
/// written as the bare Aurora version, and a release whose build stamp is not `YYYYMMDD`.
const VERSION_FORMS: [Form; 11] = [
    form("db2-", is_db2_form, "11.5.9.0"),
    form("custom-sqlserver-", is_sqlserver_form, "15.00.4073.23"),
    form("sqlserver-", is_sqlserver_form, "15.00.4385"),
    form("oracle-", is_oracle_form, "19.0.0.0"),
    form("aurora-mysql", is_aurora_mysql_form, "2.11.1"),
    form("aurora-postgresql", is_aurora_postgresql_form, "16.4.1"),
    form("docdb", is_docdb_form, "5.0"),
    form("neptune", is_neptune_form, "1.3.2"),
    form("mariadb", is_three_part_release, "10.11"),
    form("mysql", is_three_part_release, "5.7.44-rds.2025.02.13"),
    form("postgres", is_two_part_release, "11.22-rds."),
];

/// Every entry of one engine with both character-set and time-zone lists asked for.
async fn versions_with_every_list(router: &MultiServiceRouter, engine: &str) -> Vec<Version> {
    let body = expect_ok(
        engine,
        call(
            router,
            ACTION,
            &[
                ("Engine", engine.to_string()),
                ("IncludeAll", "true".to_string()),
                ("ListSupportedCharacterSets", "true".to_string()),
                ("ListSupportedTimezones", "true".to_string()),
            ],
        )
        .await,
    );
    versions(&body)
}

#[tokio::test]
async fn every_engine_aws_publishes_versions_for_carries_a_catalogue() {
    let router = emulator();

    for engine in DB_INSTANCE_ENGINES {
        let reported = all_versions_of(&router, engine).await;
        let catalogue_data = !CUSTOM_ENGINES.contains(&engine);
        assert_eq!(
            !reported.is_empty(),
            catalogue_data,
            "case {engine:?}: an engine whose versions AWS publishes must carry a \
             catalogue and an engine whose versions only CreateCustomDBEngineVersion \
             mints must carry none, got {:?}",
            reported
                .iter()
                .map(|version| version.engine_version.clone())
                .collect::<Vec<_>>()
        );
        if !catalogue_data {
            continue;
        }
        // A catalogue, not one synthesized entry: several versions, and each entry's own
        // members agree with each other.
        assert!(
            reported.len() > 1,
            "case {engine:?}: the report must carry the engine's versions, got only {:?}",
            reported
                .iter()
                .map(|version| version.engine_version.clone())
                .collect::<Vec<_>>()
        );
        for version in &reported {
            assert_eq!(
                version.engine, engine,
                "case {engine:?}: Engine narrows the report to that engine, got {:?} in {}",
                version.engine, version.xml
            );
            assert!(
                version.parameter_group_family.starts_with(engine),
                "case {engine:?}: DBParameterGroupFamily {:?} must be a family of this \
                 engine, got {}",
                version.parameter_group_family,
                version.scalars
            );
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
        }
    }
}

#[tokio::test]
async fn every_reported_version_is_published_in_its_engine_familys_form() {
    let router = emulator();

    // The whole report, so an engine this suite's tables do not name is still covered:
    // the sibling control planes' engines (`docdb`, `neptune`) are reported here too.
    let body = expect_ok(
        "every version",
        call(&router, ACTION, &[("IncludeAll", "true".to_string())]).await,
    );
    let reported = versions(&body);
    assert!(
        !reported.is_empty(),
        "case \"every version\": the report must carry the catalogue, got nothing"
    );

    let mut covered: Vec<&str> = Vec::new();
    for version in &reported {
        let case = format!("{} {}", version.engine, version.engine_version);
        // Exactly one row describes the engine's family: the prefixes are disjoint, so no
        // reordering of the table can change which form an engine is held to.
        let matching: Vec<&Form> = VERSION_FORMS
            .iter()
            .filter(|form| version.engine.starts_with(form.prefix))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: exactly one published version form must describe this \
             engine, got {:?}",
            matching.iter().map(|form| form.prefix).collect::<Vec<_>>()
        );
        let form = matching[0];
        assert!(
            (form.matches)(&version.engine_version),
            "case {case:?}: expected a version in the published form of the {:?} family, \
             got {:?}",
            form.prefix,
            version.engine_version
        );
        if !covered.contains(&form.prefix) {
            covered.push(form.prefix);
        }
    }

    // Every family the table describes is one the report actually carries, so no row is
    // dead, and every form refuses a version that is not in its family's form — an
    // assertion that accepts anything would prove nothing.
    for form in &VERSION_FORMS {
        assert!(
            covered.contains(&form.prefix),
            "case {:?}: the table describes a family the report does not carry, reported \
             families {covered:?}",
            form.prefix
        );
        assert!(
            !(form.matches)(form.counter_example),
            "case {:?}: the form must refuse {:?}, which is not how this family's \
             versions are published",
            form.prefix,
            form.counter_example
        );
    }
}

// The flag-gating of `SupportedCharacterSets`/`SupportedTimezones`, including the RDS
// Custom rule that an RDS Custom engine version lists neither while still naming its
// `DefaultCharacterSet`, is asserted over ONE table in the sibling suite
// (`db_engine_versions::character_sets_and_timezones_are_reported_only_when_the_request_asks`),
// which this change extended with the four `custom-sqlserver-*` rows and the "lists them"
// column rather than restating the rows here.

#[tokio::test]
async fn the_nchar_character_sets_create_takes_are_reported_for_oracle_only() {
    let router = emulator();

    // `CreateDBInstance$NcharCharacterSetName` is "the name of the NCHAR character set
    // for the Oracle DB instance" and "doesn't apply to RDS Custom DB instances", so the
    // Oracle engines are the rows that report the list and every other engine — the RDS
    // Custom for SQL Server rows included — reports none.
    let rows: [(&str, bool); 8] = [
        ("oracle-ee", true),
        ("oracle-ee-cdb", true),
        ("oracle-se2", true),
        ("oracle-se2-cdb", true),
        ("sqlserver-se", false),
        ("custom-sqlserver-ee", false),
        ("mariadb", false),
        ("postgres", false),
    ];

    for (engine, reports_nchar) in rows {
        for version in versions_with_every_list(&router, engine).await {
            let case = format!("{engine} {}", version.engine_version);
            let reported = subtrees(&version.scalars, "SupportedNcharCharacterSets");
            assert_eq!(
                !reported.is_empty(),
                reports_nchar,
                "case {case:?}: expected SupportedNcharCharacterSets \
                 reported={reports_nchar}, got {}",
                version.scalars
            );
            if let Some(list) = reported.first() {
                let names = texts(list, "CharacterSetName");
                assert!(
                    !names.is_empty(),
                    "case {case:?}: a reported NCHAR character set list must name its \
                     sets, got {list}"
                );
                for name in &names {
                    assert!(
                        !name.trim().is_empty(),
                        "case {case:?}: an NCHAR character set name must not be empty, \
                         got {list}"
                    );
                }
            }
        }
        // The list is gated on the same flag as the character sets it accompanies.
        for version in all_versions_of(&router, engine).await {
            assert!(
                subtrees(&version.scalars, "SupportedNcharCharacterSets").is_empty(),
                "case {engine:?} {}: SupportedNcharCharacterSets is reported only when \
                 ListSupportedCharacterSets asks for it, got {}",
                version.engine_version,
                version.scalars
            );
        }
    }
}

#[tokio::test]
async fn an_available_version_of_any_engine_is_one_create_provisions() {
    // The engines whose catalogue this change extends, plus one it does not touch: EVERY
    // available version each one publishes is read out of the report and then
    // provisioned, so the catalogue is proved to be what the create path resolves
    // against. One store per engine: the instances of one engine would otherwise count
    // against the next engine's region quota. An RDS Custom row carries a
    // `CustomIamInstanceProfile`, which `CreateDBInstance` documents as "required for
    // RDS Custom DB instances", so the create is the one AWS models for those engines.
    for engine in [
        "custom-sqlserver-ee",
        "custom-sqlserver-dev",
        "db2-ce",
        "mysql",
        "postgres",
        "sqlserver-ex",
    ] {
        let router = emulator();
        let available: Vec<Version> = {
            let body = expect_ok(
                engine,
                call(&router, ACTION, &[("Engine", engine.to_string())]).await,
            );
            versions(&body)
        };
        assert!(
            !available.is_empty(),
            "case {engine:?}: the default report must carry an available version to \
             create with"
        );

        for (index, version) in available.iter().enumerate() {
            let case = format!("{engine} {}", version.engine_version);
            let identifier = format!("emu-cat-{engine}-{index}");
            let mut params = vec![
                ("DBInstanceIdentifier", identifier.clone()),
                ("Engine", engine.to_string()),
                ("DBInstanceClass", A_CLASS.to_string()),
                ("EngineVersion", version.engine_version.clone()),
            ];
            if engine.starts_with("custom-") {
                params.push((
                    "CustomIamInstanceProfile",
                    "AWSRDSCustomSQLServerInstanceProfile".to_string(),
                ));
            }
            let created = expect_ok(&case, call(&router, "CreateDBInstance", &params).await);
            // The create answers with the version it provisioned and with the default
            // DB parameter group of that version's family, both read from the catalogue.
            assert_eq!(
                text(&created, "EngineVersion"),
                version.engine_version,
                "case {case:?}: the instance must run the version the catalogue \
                 publishes, got {created}"
            );
            assert_eq!(
                text(&created, "DBParameterGroupName"),
                format!("default.{}", version.parameter_group_family),
                "case {case:?}: the default DB parameter group is named for the \
                 version's family, got {created}"
            );

            // …and reading the instance back reports the same version.
            let read_back = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", identifier.clone())],
                )
                .await,
            );
            assert_eq!(
                text(&read_back, "EngineVersion"),
                version.engine_version,
                "case {case:?}: the stored version must read back unchanged, got {read_back}"
            );
        }
    }
}

#[tokio::test]
async fn an_engine_with_a_managed_integration_publishes_its_feature_names() {
    let router = emulator();

    // `SupportedFeatureNames` are the `FeatureName` values `AddRoleToDBInstance` takes
    // for that engine version ("the supported features vary by DB engine and DB engine
    // version"). An engine RDS associates a ROLE with a feature for — the S3 integration
    // of Db2, Oracle and SQL Server, the `aws_s3`/`aws_lambda` features of PostgreSQL —
    // publishes at least one name; an engine that configures the same integration through
    // DB parameter group parameters instead (MySQL, MariaDB) publishes none, and so does
    // every RDS Custom engine, whose DB instances have no option group and no managed
    // integration to name.
    //
    // (engine, RDS associates a role with a feature for this engine).
    let rows: [(&str, bool); 10] = [
        ("db2-ae", true),
        ("db2-ce", true),
        ("db2-se", true),
        ("oracle-ee", true),
        ("sqlserver-se", true),
        ("postgres", true),
        ("mysql", false),
        ("mariadb", false),
        ("custom-sqlserver-ee", false),
        ("custom-sqlserver-web", false),
    ];

    for (engine, has_integration) in rows {
        let reported = all_versions_of(&router, engine).await;
        assert!(
            !reported.is_empty(),
            "case {engine:?}: the report must carry this engine's versions"
        );
        for version in &reported {
            let case = format!("{engine} {}", version.engine_version);
            let names = subtrees(&version.scalars, "SupportedFeatureNames")
                .first()
                .map(|list| texts(list, "member"))
                .unwrap_or_default();
            assert_eq!(
                !names.is_empty(),
                has_integration,
                "case {case:?}: expected SupportedFeatureNames non-empty={has_integration}, \
                 got {names:?} in {}",
                version.scalars
            );
        }
    }
}

// The RDS Extended Support rule — a family past its community end of life keeps
// publishing `<community version>-rds.YYYYMMDD` builds, which are its later entries — is
// asserted over its own table by the sibling suite
// (`engine_version_families::a_family_past_its_community_end_of_life_is_published_through_dated_builds`),
// whose `dated_build`/`is_build_date` readers the form table above reuses rather than
// restating the rule here.
