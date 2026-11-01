// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the engine VOCABULARY `DescribeDBEngineVersions` resolves its
//! `Engine` member against, and for the rows each engine of it publishes.
//!
//! The member resolves an engine family rather than merely narrowing the rows (a value
//! that names no engine this front door publishes is refused — the refusal and its wording
//! are pinned in `refusal_wording_and_input_guards.rs`), so what decides whether a request
//! is served is which names count as engines. That vocabulary is the UNION of the engine
//! value lists this model's own members publish: a value list on one member is only as new
//! as the release that wrote it, so an engine published on a newer member is an engine
//! here too, and its versions are reference data this report carries.
//!
//! The rules are asserted over TABLES rather than one request: the engine table below is
//! that union, written from the model's two lists so a stale vocabulary fails here; the
//! empty-report rule is asserted on every PUBLISHED engine whose versions are account-minted
//! rather than catalogue data, and on the `engine` FILTER, which is a predicate and not a
//! resolver; and the near-miss table spans the ways a name can look like an engine of a
//! family this service publishes without being one.

use std::collections::BTreeSet;

use crate::db_engine_versions::{all_versions_of, versions, CUSTOM_ENGINES};
use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, text};

/// The operation under test.
const ACTION: &str = "DescribeDBEngineVersions";

/// The Query API code this front door answers a value its member does not accept with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// Every engine name this model's own `Engine` value lists publish, which is what an
/// `Engine` has to be resolved against.
///
/// Source: the union of
/// `smithy_query(query="doc",shape="CreateDBInstanceMessage",member="Engine")` and
/// `smithy_query(query="doc",shape="CreateDBCapacityReservationRequest",member="Engine")`
/// — the second list is the newer one and is the only one that names the second Developer
/// Edition engine (`sqlserver-dev-se`), beside the first (`sqlserver-dev-ee`). Capacity is
/// reserved for DB instances of an engine, so an engine that list carries is an engine
/// this front door publishes. `docdb` and `neptune` are left to their own suite
/// (`docdb_query_surface.rs`): they are published by the two sibling control planes' models
/// rather than by this one's members.
const MODEL_ENGINE_VOCABULARY: &[&str] = &[
    "aurora-mysql",
    "aurora-postgresql",
    "custom-oracle-ee",
    "custom-oracle-ee-cdb",
    "custom-oracle-se2",
    "custom-oracle-se2-cdb",
    "custom-sqlserver-dev",
    "custom-sqlserver-ee",
    "custom-sqlserver-se",
    "custom-sqlserver-web",
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
    "sqlserver-dev-ee",
    "sqlserver-dev-se",
    "sqlserver-ee",
    "sqlserver-ex",
    "sqlserver-se",
    "sqlserver-web",
];

/// The engines of that vocabulary whose versions are NOT catalogue data: RDS Custom for
/// Oracle, whose versions exist only once `CreateCustomDBEngineVersion` has stored one.
/// A request for one of them is a request for a published engine, so it is served — with
/// the empty list that is the truthful answer until a create has stored a version.
///
/// The list itself is the one this test target already states once
/// (`crate::db_engine_versions::CUSTOM_ENGINES`).
fn versions_are_account_minted(engine: &str) -> bool {
    CUSTOM_ENGINES.contains(&engine)
}

/// `(case, value)` rows that LOOK like an engine of a family this service publishes but
/// name no engine of the published vocabulary.
const NEAR_MISS_ENGINES: &[(&str, &str)] = &[
    ("an edition prefix that is not an engine", "sqlserver-dev"),
    ("an engine with its release glued on", "sqlserver-dev-se-17.0"),
    ("an engine with a typo in its edition", "sqlserver-dev-sse"),
    ("an edition RDS does not offer", "sqlserver-dev-web"),
    ("the product family alone", "sqlserver"),
    ("an engine name reversed", "es-ved-revreslqs"),
];

// ---------------------------------------------------------------------------
// Rule: every engine the model's value lists publish is resolved and reported
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_engine_the_model_publishes_reports_its_own_versions() {
    let router = emulator();

    for engine in MODEL_ENGINE_VOCABULARY {
        let reported = all_versions_of(&router, engine).await;

        if versions_are_account_minted(engine) {
            assert!(
                reported.is_empty(),
                "case {engine:?}: this engine's versions are minted by a create, so the \
                 report must carry none as reference data, got {:?}",
                reported
                    .iter()
                    .map(|version| version.engine_version.clone())
                    .collect::<Vec<_>>()
            );
            continue;
        }

        assert!(
            !reported.is_empty(),
            "case {engine:?}: this engine's versions are published reference data, \
             expected at least one entry, got none"
        );
        for version in &reported {
            assert_eq!(
                version.engine, *engine,
                "case {engine:?}: expected only this engine's entries, got one of {:?}",
                version.engine
            );
            assert!(
                version
                    .parameter_group_family
                    .starts_with(&format!("{engine}-"))
                    || version.parameter_group_family.starts_with(engine),
                "case {engine:?} {}: expected a DBParameterGroupFamily of this engine, \
                 got {:?}",
                version.engine_version,
                version.parameter_group_family
            );
        }

        // The name is resolved case-insensitively, like every other per-engine rule this
        // front door applies, so a differently spelled request selects the same rows.
        let shouted = all_versions_of(&router, &engine.to_ascii_uppercase()).await;
        let reported_versions: Vec<&str> = reported
            .iter()
            .map(|version| version.engine_version.as_str())
            .collect();
        let shouted_versions: Vec<&str> = shouted
            .iter()
            .map(|version| version.engine_version.as_str())
            .collect();
        assert_eq!(
            shouted_versions, reported_versions,
            "case {engine:?}: expected the same entries whatever case the engine is \
             written in, got {shouted_versions:?} against {reported_versions:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: a name outside that vocabulary names no engine family
// ---------------------------------------------------------------------------

/// A value that only LOOKS like one of this service's engines is refused the way every
/// other unresolvable engine name is — the near-miss table, which the published-vocabulary
/// table above is the positive control for.
#[tokio::test]
async fn a_name_outside_the_published_vocabulary_is_refused() {
    let router = emulator();

    for (case, value) in NEAR_MISS_ENGINES {
        let labelled = format!("{case:?} ({value:?})");
        let body = expect_error(
            &labelled,
            call(&router, ACTION, &[("Engine", *value)]).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let expected = format!("Unrecognized engine name: {value}");
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {labelled}: expected {expected:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: the `engine` FILTER is a predicate, not a resolver
// ---------------------------------------------------------------------------

/// `Filters` "specifies one or more DB engine versions to describe", so its `engine` term
/// selects among the entries rather than resolving what is reported: a term no entry
/// carries is an empty success inside the ordinary envelope, whether it names a published
/// engine with no catalogue rows or no engine at all.
#[tokio::test]
async fn an_engine_filter_term_matching_no_entry_is_an_empty_success() {
    let router = emulator();

    let mut terms: Vec<(&str, &str)> = vec![
        ("a published engine whose versions are minted", CUSTOM_ENGINES[0]),
        ("the empty string", ""),
    ];
    terms.extend(NEAR_MISS_ENGINES.iter().copied());

    for (case, value) in terms {
        let labelled = format!("{case:?} ({value:?})");
        let (status, body) = call(
            &router,
            ACTION,
            &[
                ("Filters.Filter.1.Name", "engine".to_string()),
                ("Filters.Filter.1.Values.Value.1", value.to_string()),
                ("IncludeAll", "true".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {labelled}: a filter term is a predicate, so it is an empty success \
             rather than a refusal, expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            body.contains("<DescribeDBEngineVersionsResponse"),
            "case {labelled}: expected the success envelope, got {body}"
        );
        assert!(
            !body.contains("<ErrorResponse"),
            "case {labelled}: expected no error envelope, got {body}"
        );
        let reported = versions(&body);
        assert!(
            reported.is_empty(),
            "case {labelled}: expected no entry, got {:?}",
            reported
                .iter()
                .map(|version| version.engine_version.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            optional_text(&body, "Marker"),
            None,
            "case {labelled}: an empty page owes no Marker, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: the editions of one product publish the same releases
// ---------------------------------------------------------------------------

/// The two Microsoft SQL Server Developer Edition engines are editions of one product, so
/// this service publishes the same releases for them, each under that engine's OWN
/// `DBParameterGroupFamily` — the rule the Enterprise, Standard, Express and Web rows of
/// the same catalogue already follow.
#[tokio::test]
async fn the_developer_edition_engines_publish_the_same_releases() {
    let router = emulator();

    let mut per_engine: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for engine in ["sqlserver-dev-ee", "sqlserver-dev-se"] {
        let reported = all_versions_of(&router, engine).await;
        assert!(
            !reported.is_empty(),
            "case {engine:?}: expected published versions for this engine, got none"
        );
        for version in &reported {
            // A SQL Server DB parameter group family is the engine with the product
            // RELEASE appended, written `<release>.0` where the reported
            // `MajorEngineVersion` writes it `<release>.00` (`sqlserver-ee-15.0` on the
            // published family list:
            // `smithy_query(query="doc",shape="DescribeEngineDefaultParametersMessage",member="DBParameterGroupFamily")`).
            let release = version
                .major_engine_version
                .strip_suffix('0')
                .unwrap_or(&version.major_engine_version);
            let expected_family = format!("{engine}-{release}");
            assert_eq!(
                version.parameter_group_family, expected_family,
                "case {engine:?} {}: expected the family {expected_family}, got {}",
                version.engine_version, version.parameter_group_family
            );
        }
        per_engine.push((
            engine,
            reported
                .iter()
                .map(|version| version.major_engine_version.clone())
                .collect(),
        ));
    }

    let [(first, first_majors), (second, second_majors)] = per_engine.as_slice() else {
        panic!("case \"the Developer Edition pair\": expected both engines read back");
    };
    assert_eq!(
        first_majors, second_majors,
        "case \"the Developer Edition pair\": expected {first} and {second} to publish the \
         same releases, got {first_majors:?} against {second_majors:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: a report that names no engine covers every engine with versions
// ---------------------------------------------------------------------------

/// `Engine` is optional: a request that omits it is narrowed by nothing, so the report
/// covers every engine this service publishes versions for — including an engine only the
/// newer of the model's two value lists names.
#[tokio::test]
async fn a_report_naming_no_engine_covers_every_engine_with_versions() {
    let router = emulator();

    let mut covered: BTreeSet<String> = BTreeSet::new();
    let mut marker: Option<String> = None;
    loop {
        let mut params: Vec<(&str, String)> = vec![("IncludeAll", "true".to_string())];
        if let Some(token) = &marker {
            params.push(("Marker", token.clone()));
        }
        let body = expect_ok("the whole report", call(&router, ACTION, &params).await);
        covered.extend(versions(&body).into_iter().map(|version| version.engine));
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            break;
        }
    }

    for engine in MODEL_ENGINE_VOCABULARY {
        let expected = !versions_are_account_minted(engine);
        assert_eq!(
            covered.contains(*engine),
            expected,
            "case {engine:?}: expected this engine covered by a report narrowed by \
             nothing == {expected}, got the engines {covered:?}"
        );
    }
}
