// SPDX-License-Identifier: CC0-1.0

//! The engine default parameters of a cluster parameter group FAMILY, end to end
//! over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: that EVERY family this service publishes a default parameter group
//! for reports that family's whole engine-default catalog, in `ParameterName`
//! order, every entry carrying its published default value and `engine-default`
//! as its `Source`; that those defaults are the FAMILY's and so do not move when
//! a group of the family holds a value of its own; that a family no default group
//! exists for reports no parameters rather than another family's; the documented
//! `MaxRecords` window on each bound and one past it, the `Marker` cursor, and
//! that a read leaves the store exactly as it was.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};
use crate::cluster_parameters::{create_group, default_family, reported_parameters, Reported};
use crate::modify_cluster_parameter_group::{modify, reported};

/// Drive one `DescribeDefaultClusterParameters` request for `family`, with any
/// extra form parameters appended.
async fn describe(emulator: &Emulator, family: &str, extra: &[(&str, &str)]) -> Answer {
    let mut params = vec![("ParameterGroupFamily", family)];
    params.extend_from_slice(extra);
    emulator
        .call("DescribeDefaultClusterParameters", &params)
        .await
}

/// The `ParameterName` of every parameter a body carries, in page order.
fn reported_names(xml: &str) -> Vec<String> {
    reported_parameters(xml)
        .into_iter()
        .map(|parameter| {
            parameter
                .name
                .expect("every reported parameter carries a ParameterName")
        })
        .collect()
}

/// The published catalog's names, in catalog order.
fn catalog_names() -> Vec<String> {
    state::ENGINE_DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| parameter.name.to_string())
        .collect()
}

/// Assert one reported parameter is the catalog's entry for `name`, untouched:
/// the published default value, every published member, and `engine-default` as
/// its `Source`.
fn assert_is_the_engine_default(case: &str, parameter: &Reported) {
    let name = parameter
        .name
        .clone()
        .unwrap_or_else(|| panic!("case {case}: a parameter with no ParameterName"));
    let entry = state::ENGINE_DEFAULT_PARAMETERS
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("case {case}: {name} is not a catalog entry"));
    assert_eq!(
        (
            parameter.value.as_deref(),
            parameter.source.as_deref(),
            parameter.description.as_deref(),
            parameter.data_type.as_deref(),
            parameter.allowed_values.as_deref(),
            parameter.apply_type.as_deref(),
            parameter.is_modifiable.as_deref(),
        ),
        (
            Some(entry.default_value),
            Some(state::PARAMETER_SOURCE_ENGINE_DEFAULT),
            Some(entry.description),
            Some(entry.data_type),
            entry.allowed_values,
            Some(entry.apply_type),
            Some(if entry.is_modifiable { "true" } else { "false" }),
        ),
        "case {case}/{name}: expected the published engine default, got {parameter:?}"
    );
    assert_eq!(
        parameter.minimum_engine_version,
        state::minimum_engine_version(),
        "case {case}/{name}: expected MinimumEngineVersion {:?}, got {:?}",
        state::minimum_engine_version(),
        parameter.minimum_engine_version
    );
}

/// Every family this service publishes a default parameter group for reports
/// that family's engine default catalog, whole, in `ParameterName` order, with
/// the requested family echoed back.
///
/// The table spans what the rule generalises over: every published family, read
/// off the same catalog the handlers do (`state::published_parameter_group_families`),
/// so a family added to the service is covered without touching this suite.
#[tokio::test]
async fn every_published_family_reports_its_engine_default_catalog() {
    let emulator = Emulator::new();
    let families = state::published_parameter_group_families();
    assert!(
        families.len() > 1,
        "case the family catalogue: expected more than one published family so the \
         rule is asserted over a set, got {families:?}"
    );

    for family in families {
        let answer = describe(&emulator, &family, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {family}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("ParameterGroupFamily"),
            Some(family.clone()),
            "case {family}: expected the requested family echoed back, got {:?}",
            answer.text("ParameterGroupFamily")
        );
        assert_eq!(
            reported_names(&answer.body),
            catalog_names(),
            "case {family}: expected the whole catalog in ParameterName order, got {:?}",
            reported_names(&answer.body)
        );
        for parameter in reported_parameters(&answer.body) {
            assert_is_the_engine_default(&family, &parameter);
        }
        // Every published family carries fewer parameters than the smallest page
        // the member admits, so the whole catalog is one page and no cursor is
        // handed back.
        assert_eq!(
            answer.text("Marker"),
            None,
            "case {family}: no records remain, so the page must carry no cursor, got {:?}",
            answer.text("Marker")
        );
    }
}

/// The defaults are the FAMILY's: a value a group of that family holds is
/// reported by that group and changes nothing this operation reports.
///
/// The table spans how many values a group holds and whether they are static or
/// dynamic parameters. Each row writes through the real
/// `ModifyClusterParameterGroup` path and reads the group back through
/// `DescribeClusterParameters`, so the row proves the held value IS observable
/// before asserting the family's defaults did not move.
#[tokio::test]
async fn a_value_a_group_holds_does_not_move_the_familys_defaults() {
    let cases: [(&str, &[(&str, &str)]); 4] = [
        ("no held value", &[]),
        ("one dynamic parameter", &[("max_concurrency_scaling_clusters", "4")]),
        ("one static parameter", &[("require_ssl", "true")]),
        (
            "a static and a dynamic parameter",
            &[
                ("statement_timeout", "60000"),
                ("search_path", "$user, public, staging"),
            ],
        ),
    ];

    for (case, held) in cases {
        // One emulator per row: the rows differ in what the group holds, not in
        // what a previous row left behind.
        let emulator = Emulator::new();
        let family = default_family();
        create_group(&emulator, "holds-values").await;
        if !held.is_empty() {
            let answer = modify(&emulator, "holds-values", held).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the modify to be served with 200, got {} with body {}",
                answer.status, answer.body
            );
        }

        // The round trip: the group itself reports every held value, sourced to
        // the caller.
        let group_report = reported(&emulator, "holds-values").await;
        for (name, value) in held {
            let found = group_report
                .iter()
                .find(|(reported_name, _, _)| reported_name == name);
            assert_eq!(
                found,
                Some(&(
                    (*name).to_string(),
                    (*value).to_string(),
                    state::PARAMETER_SOURCE_USER.to_string()
                )),
                "case {case}/{name}: expected the group to report the held value \
                 {value:?} sourced {:?}, got {found:?}",
                state::PARAMETER_SOURCE_USER
            );
        }

        // And the family's defaults are exactly what they were.
        let answer = describe(&emulator, &family, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_names(&answer.body),
            catalog_names(),
            "case {case}: expected the whole catalog in ParameterName order, got {:?}",
            reported_names(&answer.body)
        );
        for parameter in reported_parameters(&answer.body) {
            assert_is_the_engine_default(case, &parameter);
        }
    }
}

/// A family this service publishes no default parameter group for reports no
/// parameters — never another family's — and is reported rather than refused,
/// the operation modelling no error at all.
#[tokio::test]
async fn a_family_with_no_default_group_reports_no_parameters() {
    let emulator = Emulator::new();

    for (case, family) in [
        ("a version this service does not offer", "redshift-0.9"),
        ("a version no Redshift offers", "redshift-9.9"),
        ("a published family in the wrong case", "Redshift-1.0"),
        ("a prefix of a published family", "redshift-1"),
        ("another engine's family", "postgres15"),
        ("the default group's name, not the family", "default.redshift-1.0"),
    ] {
        let answer = describe(&emulator, family, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 for {family:?}, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("ParameterGroupFamily"),
            Some(family.to_string()),
            "case {case}: expected the requested family echoed back, got {:?}",
            answer.text("ParameterGroupFamily")
        );
        assert_eq!(
            reported_names(&answer.body),
            Vec::<String>::new(),
            "case {case}: expected no parameters for {family:?}, got {:?}",
            reported_names(&answer.body)
        );
    }
}

/// The documented `MaxRecords` window, on each published bound and one past it,
/// and the `Marker` cursor.
///
/// Source: `smithy_query(query="doc",
/// shape="DescribeDefaultClusterParametersMessage", member="MaxRecords")` —
/// "Default: 100 Constraints: minimum 20, maximum 100."
#[tokio::test]
async fn the_defaults_are_paged_over_the_documented_window() {
    let emulator = Emulator::new();
    let family = default_family();

    for (case, max_records, accepted) in [
        ("zero", "0", false),
        ("negative", "-1", false),
        ("one below the documented minimum", "19", false),
        ("the documented minimum", "20", true),
        ("inside the window", "50", true),
        ("the documented maximum", "100", true),
        ("one above the documented maximum", "101", true),
        ("far above the documented maximum", "1000", true),
    ] {
        let answer = describe(&emulator, &family, &[("MaxRecords", max_records)]).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} to be served with 200, \
                 got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                reported_names(&answer.body),
                catalog_names(),
                "case {case}: expected the whole catalog on one page, got {:?}",
                reported_names(&answer.body)
            );
            assert_eq!(
                answer.text("Marker"),
                None,
                "case {case}: no records remain, so the page must carry no cursor, got {:?}",
                answer.text("Marker")
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {case}: expected MaxRecords={max_records} to be refused with 400 \
                 InvalidParameterValue, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // A cursor this service did not hand out is refused rather than served as a
    // mid-page offset.
    for (case, cursor) in [
        ("not a cursor at all", "bogus"),
        ("a bare offset", "0"),
        ("an offset with the wrong tag", "0.deadbeef"),
        ("an offset past the catalog", "9999.deadbeef"),
    ] {
        let answer = describe(&emulator, &family, &[("Marker", cursor)]).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case {case}: expected the cursor {cursor:?} to be refused with 400 \
             InvalidParameterValue, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
}

/// Reading the family's defaults changes nothing a later caller sees: the same
/// request answers the same thing, and the family's own default parameter group
/// still reports exactly those defaults.
#[tokio::test]
async fn reading_the_defaults_changes_nothing() {
    let emulator = Emulator::new();
    let family = default_family();

    let first = describe(&emulator, &family, &[]).await;
    let second = describe(&emulator, &family, &[]).await;

    // The bodies differ only in the per-request id the envelope carries, so the
    // comparison is over what the two reads REPORT about the family.
    let settings = |xml: &str| {
        reported_parameters(xml)
            .into_iter()
            .map(|parameter| (parameter.name, parameter.value, parameter.source))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        settings(&first.body),
        settings(&second.body),
        "case read twice: expected an identical report, got {:?} then {:?}",
        settings(&first.body),
        settings(&second.body)
    );
    assert!(
        !settings(&first.body).is_empty(),
        "case read twice: the published family must report parameters, got nothing"
    );

    // The group the defaults were read from is left exactly as it was: still a
    // group of this family, still reporting every value as the engine's.
    let group_report = reported(&emulator, &state::default_parameter_group(state::DEFAULT_CLUSTER_VERSION)).await;
    assert_eq!(
        group_report
            .iter()
            .map(|(name, value, source)| (name.clone(), value.clone(), source.clone()))
            .collect::<Vec<_>>(),
        settings(&first.body)
            .into_iter()
            .map(|(name, value, source)| (
                name.unwrap_or_default(),
                value.unwrap_or_default(),
                source.unwrap_or_default()
            ))
            .collect::<Vec<_>>(),
        "case the sibling read: expected the family's default group to report the \
         same settings, got {group_report:?}"
    );
}
