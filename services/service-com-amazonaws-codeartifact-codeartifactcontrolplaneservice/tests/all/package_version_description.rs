// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the members `DescribePackageVersion` reports about a
//! stored version, and for the one token an external connection's status is spelled with.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. `displayName` is DERIVED from the coordinate a version is stored under, by its
//!    format's own display rule, so every described version reports one: the namespace is
//!    folded into the name only for the formats whose namespace is a scope (npm, Swift),
//!    written `@<scope>/<name>`, and every other format displays the bare package name.
//! 2. A modelled LIST member the service has a value for is serialized as that list, empty
//!    included — an absent list is not an empty one. `licenses` is the case here: the
//!    versions this service holds carry no license metadata, so the list is present and
//!    empty on every described version. The metadata-sourced SCALARS beside it (`summary`,
//!    `homePage`, `sourceCodeRepository`) stay absent, because absent is what "this version
//!    has no such value" means for a single value.
//! 3. An associated external connection reports its status with the token the wire uses,
//!    `AVAILABLE`, from both `AssociateExternalConnection` and `DescribeRepository`.

use serde_json::Value;

// The `PackageFormat` catalog comes from the model's own generated constant, so a format
// added to the model is covered by the table below without another edit here.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::PACKAGE_FORMAT_VALUES;
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::package_display_name;

use crate::external_connection::{create_repository as create_connection_repository, urlencode};
use crate::harness::{call, create_domain, emulator};
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish,
};

/// The `PackageFormat`s whose namespace is a SCOPE, and whose display name therefore folds
/// it in.
///
/// Source: `PackageVersionDescription$namespace` — "The namespace of an npm or Swift
/// package version is its scope" — together with the `displayName` doc's own example, "if
/// an npm package is named `ui`, is in the namespace `vue`, and has the format `npm`, then
/// the `displayName` is `@vue/ui`"
/// (`smithy_query(query="doc",shape="PackageVersionDescription",member="displayName")`).
/// Written out here rather than imported from the crate so the table is asserted against
/// the model's documentation and not against the emulator's own copy of it.
const SCOPE_FORMATS: &[&str] = &["npm", "swift"];

/// The display name rule, computed here from the documentation above, independently of the
/// implementation under test.
fn expected_display_name(format: &str, namespace: Option<&str>, package: &str) -> String {
    match namespace {
        Some(scope) if !scope.is_empty() && SCOPE_FORMATS.contains(&format) => {
            format!("@{}/{package}", scope.strip_prefix('@').unwrap_or(scope))
        }
        _ => package.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the display rule, over every modelled format and every shape of namespace.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn display_name_follows_the_formats_own_display_rule() {
    // The documented example first, verbatim from `PackageVersionDescription$displayName`:
    // npm `ui` in namespace `vue` displays as `@vue/ui`.
    assert_eq!(
        package_display_name("npm", Some("vue"), "ui"),
        "@vue/ui",
        "the model's own displayName example must hold, got {:?}",
        package_display_name("npm", Some("vue"), "ui")
    );

    // The catalog is the model's whole `PackageFormat` enum crossed with the shapes a
    // namespace comes in: absent (the formats that carry none), a bare scope, a scope the
    // caller already wrote with its `@`, a dotted Maven-style groupId, and empty.
    let namespaces = [
        ("absent", None),
        ("bare scope", Some("vue")),
        ("scope written with its @", Some("@vue")),
        ("dotted groupId", Some("com.example.tools")),
        ("empty", Some("")),
    ];
    assert!(
        PACKAGE_FORMAT_VALUES.len() >= 8,
        "expected the modelled PackageFormat catalog, got {PACKAGE_FORMAT_VALUES:?}"
    );
    for format in PACKAGE_FORMAT_VALUES {
        for (label, namespace) in &namespaces {
            let actual = package_display_name(format, *namespace, "ui");
            let expected = expected_display_name(format, *namespace, "ui");
            assert_eq!(
                actual, expected,
                "case format {format:?}, namespace {label:?}: expected displayName {expected:?}, got {actual:?}"
            );
            // Whatever the format, the display name is non-empty and inside the modelled
            // `String255` bound the member targets (length 1..255).
            assert!(
                !actual.is_empty() && actual.len() <= 255,
                "case format {format:?}, namespace {label:?}: expected a String255 value, got {actual:?}"
            );
        }
    }

    // The published boundary of `PackageName`/`String255` (length 1..255): the longest
    // name a caller can store still displays whole, and the shortest one does too.
    for length in [1usize, 254, 255] {
        let name = "n".repeat(length);
        let actual = package_display_name("generic", Some("team-core"), &name);
        assert_eq!(
            (actual.as_str(), actual.len()),
            (name.as_str(), length),
            "case name of {length} characters: a non-scope format displays the bare name, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 1 and 2 on the wire: what a described version actually reports.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn described_version_reports_its_display_name_and_an_empty_licenses_list() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rules generalise over: the namespace, the shape of the
    // package name, the asset size, and whether the version is still open for assets.
    let cases = vec![
        (
            "default namespace",
            Publish::new("alpha", "1.0.0", "payload.bin", b"one".to_vec()),
        ),
        (
            "another namespace",
            Publish::new("beta", "2.0.0-rc.1", "notes.txt", b"two".to_vec())
                .namespace("team-a"),
        ),
        (
            "dotted name, dotted namespace",
            Publish::new("org.example.tool", "3.1.4", "tool.jar", binary_content(512, 9))
                .namespace("com.example.tools"),
        ),
        (
            "name that already carries an @",
            Publish::new("@scoped-looking", "0.0.1", "payload.bin", b"three".to_vec()),
        ),
        (
            "unfinished version",
            Publish::new("open", "9.9.9", "part-0.bin", binary_content(64, 3)).unfinished(true),
        ),
    ];

    for (label, case) in &cases {
        let published = publish(&router, case).await;
        assert_eq!(
            (published.status, published.code()),
            (200, String::new()),
            "case {label:?}: publish setup must succeed, got {published:?}"
        );

        let described = describe(&router, case).await;
        assert_eq!(
            (described.status, described.code()),
            (200, String::new()),
            "case {label:?}: expected a 200 describe, got {described:?}"
        );
        let description = &described.body["packageVersion"];

        // Rule 1: a generic package's namespace is a separate coordinate, not part of how
        // the package is written, so the display name is the package name itself.
        let expected = expected_display_name(
            &case.format,
            case.namespace.as_deref(),
            &case.package,
        );
        assert_eq!(
            description["displayName"].as_str(),
            Some(expected.as_str()),
            "case {label:?}: expected displayName {expected:?}, got {described:?}"
        );

        // Rule 2: `licenses` is PRESENT and empty — not missing, and not null.
        let licenses = description.get("licenses");
        assert_eq!(
            licenses.and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {label:?}: expected licenses to be present and empty, got {licenses:?}"
        );

        // Rule 2, the other half: the metadata-sourced scalars have no source here, so
        // they are absent rather than invented.
        for member in ["summary", "homePage", "sourceCodeRepository"] {
            assert!(
                description.get(member).is_none(),
                "case {label:?}: {member} has no source for a generic version and must stay absent, got {described:?}"
            );
        }

        // Both members are projections of the stored record, so a second read repeats them
        // exactly: nothing here is minted per call.
        let again = describe(&router, case).await;
        assert_eq!(
            again.body["packageVersion"], described.body["packageVersion"],
            "case {label:?}: describing a version twice must answer identically, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the external-connection status token, on both operations that report it.
// ---------------------------------------------------------------------------

/// The one external-connection status, as the WIRE spells it: the live responses of
/// `AssociateExternalConnection` and `DescribeRepository` return `AVAILABLE`, where the
/// on-disk model's `ExternalConnectionStatus` lists `Available` and is stale for this
/// member. `status` is an enum a client COMPARES, so the token it compares against is the
/// contract.
const AVAILABLE: &str = "AVAILABLE";

#[tokio::test]
async fn external_connection_status_is_reported_with_the_wire_token() {
    let router = emulator();
    let domain = "ec-status-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );

    // Rows differ in the registry associated — one per package format family the catalog
    // spans — because the status is a property of the association, not of the registry.
    for (index, connection) in ["public:npmjs", "public:maven-central", "public:crates-io"]
        .iter()
        .enumerate()
    {
        let repository = format!("ec-status-repo-{index}");
        let created = create_connection_repository(&router, domain, &repository).await;
        assert_eq!(
            created.status, 200,
            "case {connection:?}: repository setup must succeed, got {created:?}"
        );

        let associated = call(
            &router,
            "POST",
            &format!(
                "/v1/repository/external-connection?domain={domain}&repository={repository}&external-connection={}",
                urlencode(connection)
            ),
            None,
        )
        .await;
        assert_eq!(
            associated.status, 200,
            "case {connection:?}: expected HTTP 200, got {associated:?}"
        );

        let described = call(
            &router,
            "GET",
            &format!("/v1/repository?domain={domain}&repository={repository}"),
            None,
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {connection:?}: read-back must succeed, got {described:?}"
        );

        for (operation, answer) in [
            ("AssociateExternalConnection", &associated),
            ("DescribeRepository", &described),
        ] {
            let status =
                answer.body["repository"]["externalConnections"][0]["status"].as_str();
            assert_eq!(
                status,
                Some(AVAILABLE),
                "case {connection:?}, {operation}: expected status {AVAILABLE:?}, got {answer:?}"
            );
        }
    }
}
