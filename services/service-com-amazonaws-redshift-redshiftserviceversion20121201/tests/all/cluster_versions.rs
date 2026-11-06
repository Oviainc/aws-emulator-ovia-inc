// SPDX-License-Identifier: CC0-1.0

//! The engine-version catalog `DescribeClusterVersions` reports, end to end over
//! the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the
//! production router builds, reusing the harness the cluster-lifecycle suite owns
//! rather than keeping a second copy of it. The suite asserts RULES over tables of
//! inputs: the catalog a read reports, how each filter selects out of it, the
//! documented `MaxRecords` window and `Marker` cursor, and the agreement between
//! the catalog and what `CreateCluster` applies.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator};

/// The `<ClusterVersion>` text of every entry a `DescribeClusterVersions` body
/// carries, in the order the page lists them.
///
/// `ClusterVersionList`'s member carries `xmlName: ClusterVersion`, so the list
/// wraps each entry in an element with the SAME name as the entry's own
/// `ClusterVersion` member. The two are told apart by content: a wrapper's open
/// tag is followed by another element, the member's by its text.
fn listed_versions(xml: &str) -> Vec<String> {
    const OPEN: &str = "<ClusterVersion>";
    const CLOSE: &str = "</ClusterVersion>";
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(OPEN) {
        let inner = &rest[start + OPEN.len()..];
        if inner.starts_with('<') {
            rest = inner;
            continue;
        }
        let Some(end) = inner.find(CLOSE) else { break };
        found.push(inner[..end].to_string());
        rest = &inner[end + CLOSE.len()..];
    }
    found
}

/// One row of the filter table: the case name, the request's filter parameters,
/// and the engine versions that must come back.
type FilterCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<&'static str>,
);

#[tokio::test]
async fn the_published_catalog_is_what_a_read_reports() {
    let emulator = Emulator::new();

    // The catalog is the service's published engine-version set, seeded when the
    // store is created: `CreateClusterMessage$ClusterVersion` — "Only version 1.0
    // is currently available. Example: 1.0".
    let published = state::published_cluster_versions();
    let expected: Vec<String> = published
        .iter()
        .map(|offered| offered.cluster_version.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        expected,
        vec![state::DEFAULT_CLUSTER_VERSION.to_string()],
        "case published catalog: expected exactly the one documented engine version, got {expected:?}"
    );

    // Every entry is internally consistent: its family is the family of its own
    // version, and its description is the ENGINE BUILD that version runs (pinned
    // as a rule in the `static_catalog` suite), not a restatement of the version.
    for offered in &published {
        let version = offered.cluster_version.clone().unwrap_or_default();
        assert_eq!(
            offered.cluster_parameter_group_family.as_deref(),
            Some(state::parameter_group_family(&version).as_str()),
            "case catalog entry {version}: expected the family of its own version, got {:?}",
            offered.cluster_parameter_group_family
        );
        let build = state::PUBLISHED_ENGINE_BUILDS
            .iter()
            .find(|(offered_version, _)| *offered_version == version)
            .map(|(_, build)| *build)
            .unwrap_or_else(|| panic!("case catalog entry {version}: expected a published build"));
        assert_eq!(
            offered.description.as_deref(),
            Some(state::engine_build_description(build).as_str()),
            "case catalog entry {version}: expected the engine build {build} as the description, got {:?}",
            offered.description
        );
    }

    // Answered before any cluster exists — "You can call this operation even
    // before creating any clusters".
    let answer = emulator.call("DescribeClusterVersions", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case read an empty emulator: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        listed_versions(&answer.body),
        expected,
        "case read an empty emulator: expected the published catalog, got body {}",
        answer.body
    );
    for offered in &published {
        let version = offered.cluster_version.clone().unwrap_or_default();
        assert!(
            all_text(&answer.body, "ClusterParameterGroupFamily")
                .contains(&state::parameter_group_family(&version)),
            "case read an empty emulator: expected the family of {version} on the wire, got {:?}",
            all_text(&answer.body, "ClusterParameterGroupFamily")
        );
        let description = offered.description.clone().unwrap_or_default();
        assert!(
            all_text(&answer.body, "Description").contains(&description),
            "case read an empty emulator: expected the engine build {description:?} of {version} on the wire, got {:?}",
            all_text(&answer.body, "Description")
        );
    }

    // A read leaves nothing behind: the next caller sees exactly the same answer.
    let again = emulator.call("DescribeClusterVersions", &[]).await;
    assert_eq!(
        (again.status, listed_versions(&again.body)),
        (answer.status, listed_versions(&answer.body)),
        "case read twice: expected an unchanged catalog, got {} with body {}",
        again.status,
        again.body
    );
}

#[tokio::test]
async fn each_filter_selects_exactly_the_versions_it_names() {
    let emulator = Emulator::new();

    // The one published version and its family, from
    // `state::published_cluster_versions()` (asserted in
    // `the_published_catalog_is_what_a_read_reports`).
    let offered_version = state::DEFAULT_CLUSTER_VERSION;
    let offered_family = state::parameter_group_family(offered_version);

    let cases: Vec<FilterCase> = vec![
        ("no filter at all", vec![], vec![offered_version]),
        (
            "the offered version",
            vec![("ClusterVersion", offered_version)],
            vec![offered_version],
        ),
        (
            "a version nobody offers",
            vec![("ClusterVersion", "2.0")],
            vec![],
        ),
        (
            "a version that is only a prefix of the offered one",
            vec![("ClusterVersion", "1")],
            vec![],
        ),
        (
            "the offered family",
            vec![("ClusterParameterGroupFamily", "redshift-1.0")],
            vec![offered_version],
        ),
        (
            "a parameter group family no engine version is in",
            vec![("ClusterParameterGroupFamily", "redshift-2.0")],
            vec![],
        ),
        (
            "the offered family in another case",
            vec![("ClusterParameterGroupFamily", "Redshift-1.0")],
            vec![],
        ),
        (
            "both filters naming the same entry",
            vec![
                ("ClusterVersion", offered_version),
                ("ClusterParameterGroupFamily", "redshift-1.0"),
            ],
            vec![offered_version],
        ),
        (
            "an offered version with a family it is not in",
            vec![
                ("ClusterVersion", offered_version),
                ("ClusterParameterGroupFamily", "redshift-2.0"),
            ],
            vec![],
        ),
        (
            "an offered family with a version it does not hold",
            vec![
                ("ClusterVersion", "2.0"),
                ("ClusterParameterGroupFamily", "redshift-1.0"),
            ],
            vec![],
        ),
    ];

    assert_eq!(
        offered_family, "redshift-1.0",
        "case filter table: the rows name the family of the offered version, got {offered_family}"
    );

    for (case, params, expected) in cases {
        let answer = emulator.call("DescribeClusterVersions", &params).await;
        // A filter that selects nothing is an empty page, never a refusal: the
        // model declares no error for this operation.
        assert_eq!(
            answer.status, 200,
            "case filter {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let listed = listed_versions(&answer.body);
        assert_eq!(
            listed, expected,
            "case filter {case}: expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_window_and_cursor_follow_the_documented_rules() {
    let emulator = Emulator::new();

    // "Default: 100. Constraints: minimum 20, maximum 100." —
    // `DescribeClusterVersionsMessage$MaxRecords`. Each published bound, and one
    // past it on either side; only the minimum is enforced, so a page size past
    // the published maximum is served.
    for (case, max_records, accepted) in [
        (
            "one below the minimum",
            (state::MAX_RECORDS_MIN - 1).to_string(),
            false,
        ),
        ("at the minimum", state::MAX_RECORDS_MIN.to_string(), true),
        ("at the maximum", state::MAX_RECORDS_MAX.to_string(), true),
        (
            "one above the maximum",
            (state::MAX_RECORDS_MAX + 1).to_string(),
            true,
        ),
        (
            "far above the maximum",
            (state::MAX_RECORDS_MAX * 10).to_string(),
            true,
        ),
    ] {
        let answer = emulator
            .call("DescribeClusterVersions", &[("MaxRecords", &max_records)])
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case MaxRecords {case} ({max_records}): expected accepted={accepted}, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
        if !accepted {
            assert_eq!(
                answer.error_code().as_deref(),
                Some("InvalidParameterValue"),
                "case MaxRecords {case} ({max_records}): expected an InvalidParameterValue, got {:?}",
                answer.error_code()
            );
        }
    }

    // The published catalog fits in one page, so the page is complete and carries
    // no cursor to continue from.
    let whole = emulator
        .call(
            "DescribeClusterVersions",
            &[("MaxRecords", &state::MAX_RECORDS_MIN.to_string())],
        )
        .await;
    assert!(
        listed_versions(&whole.body).len() <= state::MAX_RECORDS_MIN as usize,
        "case one page: expected at most {} entries, got {:?}",
        state::MAX_RECORDS_MIN,
        listed_versions(&whole.body)
    );
    assert_eq!(
        whole.text("Marker"),
        None,
        "case one page: a complete page returns no Marker, got {:?}",
        whole.text("Marker")
    );

    // A cursor nobody handed out is refused rather than served as an offset,
    // exactly as on the other paginated reads.
    for (case, cursor) in [
        ("not a number", "not-a-cursor"),
        ("an invented offset", "7"),
        ("an offset with a forged tag", "0.00000000"),
    ] {
        let bogus = emulator
            .call("DescribeClusterVersions", &[("Marker", cursor)])
            .await;
        assert_eq!(
            (bogus.status, bogus.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case marker {case} ({cursor:?}): expected a 400 refusal, got {} / {:?}",
            bogus.status,
            bogus.error_code()
        );
    }
}

#[tokio::test]
async fn a_created_cluster_runs_a_version_the_catalog_offers() {
    let emulator = Emulator::new();

    // Each row creates a cluster and names the engine version it must report:
    // the request either names a version the catalog offers or names none and
    // takes the documented default.
    for (case, identifier, requested) in [
        ("no version named", "catalog-default", None),
        (
            "the offered version named",
            "catalog-explicit",
            Some(state::DEFAULT_CLUSTER_VERSION),
        ),
    ] {
        let created = match requested {
            None => emulator.create_minimal(identifier).await,
            Some(version) => {
                emulator
                    .call(
                        "CreateCluster",
                        &[
                            ("ClusterIdentifier", identifier),
                            ("NodeType", "ra3.xlplus"),
                            ("MasterUsername", "admin"),
                            ("ClusterVersion", version),
                        ],
                    )
                    .await
            }
        };
        assert_eq!(
            created.status, 200,
            "case create with {case}: expected 200, got {} with body {}",
            created.status, created.body
        );

        // Read it back through `DescribeClusters`: the version it reports is one
        // the catalog offers, and the parameter group it joined is the default
        // group of the family new clusters join — which is a service-level
        // default, not one derived from the cluster's engine version.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
            .await;
        assert_eq!(
            described.status, 200,
            "case describe with {case}: expected 200, got {} with body {}",
            described.status, described.body
        );
        let version = described
            .text("ClusterVersion")
            .unwrap_or_else(|| panic!("case describe with {case}: expected a ClusterVersion"));
        assert_eq!(
            all_text(&described.body, "ParameterGroupName"),
            vec![state::cluster_default_parameter_group()],
            "case describe with {case}: expected the default group of the service's parameter group family, got {:?}",
            all_text(&described.body, "ParameterGroupName")
        );

        let catalog = emulator
            .call("DescribeClusterVersions", &[("ClusterVersion", &version)])
            .await;
        assert_eq!(
            listed_versions(&catalog.body),
            vec![version.clone()],
            "case catalog lookup with {case}: expected the catalog to offer {version}, got body {}",
            catalog.body
        );
        let family = state::parameter_group_family(&version);
        assert_eq!(
            all_text(&catalog.body, "ClusterParameterGroupFamily"),
            vec![family.clone()],
            "case catalog lookup with {case}: expected family {family}, got {:?}",
            all_text(&catalog.body, "ClusterParameterGroupFamily")
        );
    }
}
