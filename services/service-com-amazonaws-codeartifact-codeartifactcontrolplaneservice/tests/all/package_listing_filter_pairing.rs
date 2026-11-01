// SPDX-License-Identifier: CC0-1.0

//! `ListPackages`' cross-member rule, asserted over the wire: a `namespace` filter needs the
//! `format` that gives it its meaning.
//!
//! Every case drives a real `restJson1` request through the `MultiServiceRouter` the
//! production emulator serves with, so what is pinned here is the status, the error code and
//! the `message` a CodeArtifact client reads.
//!
//! THE RULE: `namespace` filters on a component whose meaning is CHOSEN by the package
//! format — "The namespace of a Maven package version is its groupId. The namespace of an npm
//! or Swift package version is its scope. The namespace of a generic package is its
//! namespace", and Python, NuGet, Ruby and Cargo versions have no such component at all
//! (`smithy_query(query="operation",operation="ListPackages")`, member `namespace`). So a
//! listing that filters on `namespace` without naming a `format` is refused with
//! `ValidationException` (HTTP 400; that operation's errors list `ValidationException`
//! httpError 400) rather than answered, while a listing that names both, or `format` alone,
//! or neither, is a complete request and is served.
//!
//! The rows differ along what the rule generalises over: which of the two members the request
//! carries, the format named beside the namespace (one that has that component and one that
//! has none), the namespace's own text and length, and which other filters ride along —
//! `packagePrefix`, `publish`, `upstream` and `maxResults` are unaffected either way.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The format catalog comes from the model's own generated constant, so a model change moves
// this table rather than leaving a hand-copied list behind.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::PACKAGE_FORMAT_VALUES;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};

/// The domain and repository every case lists.
const DOMAIN: &str = "filter-pairing";
const REPOSITORY: &str = "Pairing.repo";

/// The sentence the refusal states. Fixed text naming the two members: the refusal is about
/// the pair, so nothing from the request is interpolated into it.
const EXPECTED_MESSAGE: &str = "PackageFormat cannot be null when PackageNamespace is not null";

/// `ListPackages` with `filters` appended to the addressed repository.
async fn list_packages(router: &MultiServiceRouter, filters: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/packages?domain={DOMAIN}&repository={REPOSITORY}{filters}"),
        Some(json!({})),
    )
    .await
}

/// An emulator holding the domain and repository, with one generic and one npm package
/// published into it through `PutPackageOriginConfiguration` — so a served listing has
/// something to report and a refused one is not passing by emptiness.
async fn given_repository() -> MultiServiceRouter {
    let router = emulator();
    assert_eq!(
        create_domain(&router, DOMAIN, None).await.status,
        200,
        "CreateDomain must succeed"
    );
    assert_eq!(
        create_repository(&router, DOMAIN, REPOSITORY, None, &[])
            .await
            .status,
        200,
        "CreateRepository must succeed"
    );
    // (format, namespace, package)
    for (format, namespace, package) in [
        ("generic", Some("sim2real"), "blob-bundle"),
        ("maven", Some("com.example"), "lib-core"),
        ("npm", Some("@scope"), "web-ui"),
        ("pypi", None, "requests"),
    ] {
        let mut uri = format!(
            "/v1/package?domain={DOMAIN}&repository={REPOSITORY}&format={format}&package={package}"
        );
        if let Some(namespace) = namespace {
            uri.push_str(&format!("&namespace={namespace}"));
        }
        let put = call(
            &router,
            "POST",
            &uri,
            Some(json!({ "restrictions": { "publish": "ALLOW", "upstream": "ALLOW" } })),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "seeding {format}/{package} must succeed, got {put:?}"
        );
    }
    router
}

/// THE RULE: a `namespace` filter with no `format` is refused; the same listing with a
/// `format` beside it is served.
#[tokio::test]
async fn a_namespace_filter_without_a_format_is_refused() {
    let router = given_repository().await;

    // (case, the namespace to filter on, the other filters riding along)
    let rows: &[(&str, &str, &str)] = &[
        ("a namespace that matches", "sim2real", ""),
        ("a one-character namespace prefix", "s", ""),
        ("a namespace no package carries", "nope", ""),
        ("an npm scope", "@scope", ""),
        (
            "the modelled maximum namespace length",
            // `PackageNamespace` is len 1..255, pattern `^[^#/\s]+$`
            // (`smithy_query(query="operation",operation="ListPackages")`).
            "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn",
            "",
        ),
        (
            "a namespace beside every other filter the listing takes",
            "sim2real",
            "&package-prefix=blob&publish=ALLOW&upstream=ALLOW&max-results=10",
        ),
    ];

    for (case, namespace, extra) in rows {
        assert!(
            (1..=255).contains(&namespace.chars().count()),
            "case {case:?}: the row must stay inside PackageNamespace's modelled length, got {}",
            namespace.chars().count()
        );
        let refused = list_packages(&router, &format!("&namespace={namespace}{extra}")).await;
        assert_eq!(
            (refused.status, refused.code().as_str(), refused.message()),
            (400, "ValidationException", EXPECTED_MESSAGE.to_string()),
            "case {case:?}: expected the modelled refusal, got {refused:?}"
        );
        assert!(
            refused.body.get("packages").is_none(),
            "case {case:?}: a refused listing must carry no packages, got {refused:?}"
        );

        // The identical listing with a format beside it is a complete request. Asserted for
        // EVERY format the model declares, including the ones whose packages carry no
        // namespace at all: the pair is what the rule is about, not which format it is.
        for format in PACKAGE_FORMAT_VALUES {
            let served =
                list_packages(&router, &format!("&namespace={namespace}&format={format}{extra}"))
                    .await;
            assert_eq!(
                served.status, 200,
                "case {case:?} with format {format:?}: expected the listing to be served, got \
                 {served:?}"
            );
            assert!(
                served.body["packages"].is_array(),
                "case {case:?} with format {format:?}: expected a listing, got {served:?}"
            );
        }
    }
}

/// THE COMPLEMENT: a listing that names no `namespace` is complete with or without a
/// `format`, so the refusal is scoped to the pair and does not make `format` required.
#[tokio::test]
async fn a_listing_without_a_namespace_needs_no_format() {
    let router = given_repository().await;

    // (case, the filters the listing carries)
    let rows: &[(&str, String)] = &[
        ("no filter at all", String::new()),
        ("format alone", "&format=generic".to_string()),
        (
            "format and a package prefix",
            "&format=maven&package-prefix=lib-".to_string(),
        ),
        (
            "the origin restrictions alone",
            "&publish=ALLOW&upstream=ALLOW".to_string(),
        ),
        (
            "a package prefix alone",
            "&package-prefix=blob".to_string(),
        ),
        ("a page size alone", "&max-results=1".to_string()),
    ];

    for (case, filters) in rows {
        let served = list_packages(&router, filters).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected the listing to be served, got {served:?}"
        );
        assert!(
            served.body["packages"].is_array(),
            "case {case:?}: expected a listing, got {served:?}"
        );
    }
}
