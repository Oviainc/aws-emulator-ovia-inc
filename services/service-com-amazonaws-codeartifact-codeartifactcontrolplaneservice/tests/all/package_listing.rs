// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for CodeArtifact's package listing.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with (the harness is the one
//! `tests/all/domain_repository_lifecycle.rs` already owns), so what is asserted here is
//! the wire behaviour a CodeArtifact client sees — status, error code and body.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. `PutPackageOriginConfiguration` stores the requested restrictions on the package the
//!    (`format`, `namespace`, `package`) triple names, creating the package record with no
//!    versions when the repository does not hold it yet (the operation's own modelled
//!    documentation says so), and a later `DescribePackage`/`ListPackages` answers with
//!    that same record.
//! 2. `ListPackages` returns the packages its repository holds, each exactly once, and
//!    every filter the model declares is ANDed: `format`, `publish` and `upstream` match
//!    exactly, `namespace` and `packagePrefix` are prefix matches, and a package of a
//!    format that carries no namespace matches no `namespace` filter.
//! 3. A `@paginated` listing covers every MATCHING package exactly once in one stable
//!    order across any page size, and omits `nextToken` on the last page.
//! 4. Addressing a domain, repository or package that is not held is
//!    `ResourceNotFoundException` (404); a `nextToken` this listing did not mint is
//!    `ValidationException` (400).
//! 5. A package belongs to ONE repository: it is invisible in any other repository's
//!    listing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};

/// One package to put into a repository: the triple that identifies it plus the origin
/// restrictions to set on it.
#[derive(Debug, Clone, Copy)]
struct PackageCase {
    format: &'static str,
    /// `None` for a format whose versions carry no namespace component.
    namespace: Option<&'static str>,
    package: &'static str,
    publish: &'static str,
    upstream: &'static str,
}

/// A package's identity as the wire reports it: (`format`, `namespace`, `package`), with
/// an absent namespace reduced to the empty string so the triples compare.
type Identity = (String, String, String);

impl PackageCase {
    fn identity(&self) -> Identity {
        (
            self.format.to_string(),
            self.namespace.unwrap_or_default().to_string(),
            self.package.to_string(),
        )
    }

    fn label(&self) -> String {
        format!(
            "{}/{}/{}",
            self.format,
            self.namespace.unwrap_or_default(),
            self.package
        )
    }

    /// The query string that addresses this package.
    fn coordinate_query(&self) -> String {
        let mut query = format!("format={}&package={}", self.format, self.package);
        if let Some(namespace) = self.namespace {
            query.push_str(&format!("&namespace={namespace}"));
        }
        query
    }
}

/// The packages seeded into one repository.
///
/// It spans EVERY value of `PackageFormat` the model declares — `smithy_query(query =
/// "shape", shape = "PackageFormat")` lists exactly `npm`, `pypi`, `maven`, `nuget`,
/// `generic`, `ruby`, `swift`, `cargo` — and both values of `AllowPublish`/`AllowUpstream`
/// (`ALLOW`, `BLOCK`). Which formats carry a namespace follows the model's
/// `PackageSummary$namespace` doc: a Maven package's namespace is its `groupId`, an npm or
/// Swift package's is its scope, a generic package's is its namespace, and Python, NuGet,
/// Ruby and Cargo packages have none. Two entries deliberately share a package NAME under
/// different namespaces, and two share a namespace, so a filter cannot pass by accident.
const CATALOG: &[PackageCase] = &[
    PackageCase {
        format: "maven",
        namespace: Some("com.example"),
        package: "lib-core",
        publish: "ALLOW",
        upstream: "ALLOW",
    },
    PackageCase {
        format: "maven",
        namespace: Some("com.example.tools"),
        package: "lib-cli",
        publish: "ALLOW",
        upstream: "BLOCK",
    },
    PackageCase {
        format: "maven",
        namespace: Some("com.other"),
        package: "lib-core",
        publish: "BLOCK",
        upstream: "ALLOW",
    },
    PackageCase {
        format: "npm",
        namespace: Some("@scope"),
        package: "web-ui",
        publish: "BLOCK",
        upstream: "ALLOW",
    },
    PackageCase {
        format: "npm",
        namespace: Some("@scope"),
        package: "web-core",
        publish: "ALLOW",
        upstream: "BLOCK",
    },
    PackageCase {
        format: "generic",
        namespace: Some("artifacts"),
        package: "blob-bundle",
        publish: "ALLOW",
        upstream: "BLOCK",
    },
    PackageCase {
        format: "swift",
        namespace: Some("@swiftpkgs"),
        package: "net-kit",
        publish: "BLOCK",
        upstream: "BLOCK",
    },
    PackageCase {
        format: "pypi",
        namespace: None,
        package: "requests",
        publish: "BLOCK",
        upstream: "BLOCK",
    },
    PackageCase {
        format: "nuget",
        namespace: None,
        package: "Newtonsoft.Json",
        publish: "ALLOW",
        upstream: "ALLOW",
    },
    PackageCase {
        format: "ruby",
        namespace: None,
        package: "rails",
        publish: "BLOCK",
        upstream: "ALLOW",
    },
    PackageCase {
        format: "cargo",
        namespace: None,
        package: "serde",
        publish: "ALLOW",
        upstream: "ALLOW",
    },
];

/// One `ListPackages` filter combination.
#[derive(Debug, Default, Clone, Copy)]
struct Filter {
    format: Option<&'static str>,
    namespace: Option<&'static str>,
    package_prefix: Option<&'static str>,
    publish: Option<&'static str>,
    upstream: Option<&'static str>,
}

impl Filter {
    /// The filter as query parameters, in the member names the model binds
    /// (`@httpQuery`).
    fn query(&self) -> String {
        let mut query = String::new();
        for (name, value) in [
            ("format", self.format),
            ("namespace", self.namespace),
            ("package-prefix", self.package_prefix),
            ("publish", self.publish),
            ("upstream", self.upstream),
        ] {
            if let Some(value) = value {
                query.push_str(&format!("&{name}={value}"));
            }
        }
        query
    }

    /// The RULE the service filters by, written out here independently of the handler:
    /// `format`, `publish` and `upstream` match exactly, `namespace` and `packagePrefix`
    /// are prefix matches, a package with no namespace matches no `namespace` filter, and
    /// every supplied filter must hold.
    fn matches(&self, case: &PackageCase) -> bool {
        self.format.is_none_or(|format| format == case.format)
            && self.namespace.is_none_or(|namespace| {
                case.namespace
                    .is_some_and(|stored| stored.starts_with(namespace))
            })
            && self
                .package_prefix
                .is_none_or(|prefix| case.package.starts_with(prefix))
            && self.publish.is_none_or(|publish| publish == case.publish)
            && self.upstream.is_none_or(|upstream| upstream == case.upstream)
    }

    /// The identities this filter selects out of `catalog`, as a sorted set.
    fn expected(&self, catalog: &[PackageCase]) -> Vec<Identity> {
        let mut expected: Vec<Identity> = catalog
            .iter()
            .filter(|case| self.matches(case))
            .map(PackageCase::identity)
            .collect();
        expected.sort();
        expected
    }
}

/// `PutPackageOriginConfiguration` for one case.
async fn put_origin(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    case: &PackageCase,
    publish: &str,
    upstream: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/package?domain={domain}&repository={repository}&{}",
            case.coordinate_query()
        ),
        Some(json!({ "restrictions": { "publish": publish, "upstream": upstream } })),
    )
    .await
}

/// `DescribePackage` for one coordinate.
async fn describe_package(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    coordinate: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/package?domain={domain}&repository={repository}&{coordinate}"),
        None,
    )
    .await
}

/// `ListPackages` with an arbitrary query string.
async fn list_packages(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/packages?{query}"),
        Some(json!({})),
    )
    .await
}

/// The identity triple one `PackageSummary` entry reports.
fn identities_of(entry: &Value) -> Identity {
    (
        field(entry, "format"),
        entry
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        field(entry, "package"),
    )
}

/// The identities in a `ListPackages` answer, in the order returned.
fn identities(answer: &Answer) -> Vec<Identity> {
    answer
        .body
        .get("packages")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().map(identities_of).collect())
        .unwrap_or_default()
}

/// One string member of a response entry, or a marker naming the member that is missing.
fn field(entry: &Value, member: &str) -> String {
    entry
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
        .to_string()
}

/// The `originConfiguration` an entry reports.
fn origin_configuration(entry: &Value) -> Value {
    entry
        .get("originConfiguration")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `originConfiguration` a case's restrictions must render as.
fn expected_origin_configuration(publish: &str, upstream: &str) -> Value {
    json!({ "restrictions": { "publish": publish, "upstream": upstream } })
}

/// Create `domain`/`repository` and put every case's origin configuration into it.
async fn seed(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    catalog: &[PackageCase],
) {
    let created = create_domain(router, domain, None).await;
    assert_eq!(
        created.status, 200,
        "case {domain:?}: expected CreateDomain 200, got {created:?}"
    );
    let created = create_repository(router, domain, repository, None, &[]).await;
    assert_eq!(
        created.status, 200,
        "case {repository:?}: expected CreateRepository 200, got {created:?}"
    );
    for case in catalog {
        let answer = put_origin(
            router,
            domain,
            repository,
            case,
            case.publish,
            case.upstream,
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected PutPackageOriginConfiguration 200, got {answer:?}",
            case.label()
        );
        assert_eq!(
            origin_configuration(&answer.body),
            expected_origin_configuration(case.publish, case.upstream),
            "case {:?}: the put must answer with the restrictions it stored, got {:?}",
            case.label(),
            answer.body
        );
    }
}

/// Walk a listing to exhaustion at `size` results per page, returning the identities in
/// the order they were returned and how many pages it took.
async fn collect_pages(
    router: &MultiServiceRouter,
    base_query: &str,
    size: Option<usize>,
) -> (Vec<Identity>, usize) {
    let mut collected: Vec<Identity> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = base_query.to_string();
        if let Some(size) = size {
            query.push_str(&format!("&max-results={size}"));
        }
        if let Some(token) = &token {
            query.push_str(&format!("&next-token={token}"));
        }
        let answer = list_packages(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case page size {size:?}: expected ListPackages 200, got {answer:?}"
        );
        let page = identities(&answer);
        if let Some(size) = size {
            assert!(
                page.len() <= size,
                "case page size {size:?}: a page must hold at most {size} results, got {:?}",
                page.len()
            );
        }
        collected.extend(page);
        pages += 1;
        token = answer.body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return (collected, pages);
        }
        assert!(
            pages <= CATALOG.len() + 2,
            "case page size {size:?}: pagination did not terminate, collected {collected:?}"
        );
    }
}

/// RULE 1 and RULE 2: a put stores the package, and a later single-package read and the
/// full listing both answer with that stored record.
#[tokio::test]
async fn put_origin_configuration_creates_a_package_that_reads_back() {
    let router = emulator();
    seed(&router, "pkg-domain", "pkg-repo", CATALOG).await;

    for case in CATALOG {
        let described =
            describe_package(&router, "pkg-domain", "pkg-repo", &case.coordinate_query()).await;
        assert_eq!(
            described.status,
            200,
            "case {:?}: expected DescribePackage 200, got {described:?}",
            case.label()
        );
        let package = described.body.get("package").cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                field(&package, "format"),
                package
                    .get("namespace")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                field(&package, "name"),
            ),
            case.identity(),
            "case {:?}: DescribePackage must answer with the stored triple, got {package:?}",
            case.label()
        );
        assert_eq!(
            origin_configuration(&package),
            expected_origin_configuration(case.publish, case.upstream),
            "case {:?}: DescribePackage must report the stored restrictions, got {package:?}",
            case.label()
        );
    }

    // The unfiltered listing holds every seeded package exactly once, each carrying the
    // restrictions that were stored for it.
    let listed = list_packages(&router, "domain=pkg-domain&repository=pkg-repo").await;
    assert_eq!(
        listed.status, 200,
        "expected ListPackages 200, got {listed:?}"
    );
    let mut found = identities(&listed);
    found.sort();
    let mut expected: Vec<Identity> = CATALOG.iter().map(PackageCase::identity).collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "the listing must hold every stored package exactly once, got {:?}",
        listed.body
    );
    let entries = listed.body["packages"]
        .as_array()
        .expect("packages must be a list")
        .clone();
    for case in CATALOG {
        let entry = entries
            .iter()
            .find(|entry| identities_of(entry) == case.identity())
            .unwrap_or_else(|| panic!("case {:?}: missing from the listing", case.label()));
        assert_eq!(
            origin_configuration(entry),
            expected_origin_configuration(case.publish, case.upstream),
            "case {:?}: the listing must report the stored restrictions, got {entry:?}",
            case.label()
        );
    }
}

/// RULE 1 again: a second put on the same triple UPDATES that package in place — it
/// neither duplicates it nor leaves the previous restrictions behind.
#[tokio::test]
async fn a_second_put_updates_the_package_in_place() {
    let router = emulator();
    seed(&router, "put-domain", "put-repo", CATALOG).await;

    for case in CATALOG {
        // Flip both restrictions: whatever the case stored, the opposite is written.
        let flipped_publish = if case.publish == "ALLOW" {
            "BLOCK"
        } else {
            "ALLOW"
        };
        let flipped_upstream = if case.upstream == "ALLOW" {
            "BLOCK"
        } else {
            "ALLOW"
        };
        let answer = put_origin(
            &router,
            "put-domain",
            "put-repo",
            case,
            flipped_publish,
            flipped_upstream,
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected the update to answer 200, got {answer:?}",
            case.label()
        );
        let described =
            describe_package(&router, "put-domain", "put-repo", &case.coordinate_query()).await;
        assert_eq!(
            origin_configuration(&described.body["package"]),
            expected_origin_configuration(flipped_publish, flipped_upstream),
            "case {:?}: the update must be observable to the next read, got {:?}",
            case.label(),
            described.body
        );
    }

    // Still one record per triple: the updates replaced values, they did not add rows.
    let listed = list_packages(&router, "domain=put-domain&repository=put-repo").await;
    let mut found = identities(&listed);
    found.sort();
    let mut expected: Vec<Identity> = CATALOG.iter().map(PackageCase::identity).collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "an update must not add a second record for the same triple, got {:?}",
        listed.body
    );
}

/// RULE 2: every filter combination selects exactly the packages the rule matches.
#[tokio::test]
async fn every_filter_selects_exactly_the_matching_packages() {
    let router = emulator();
    seed(&router, "filter-domain", "filter-repo", CATALOG).await;

    let cases: Vec<Filter> = vec![
        // No filter at all: the whole repository.
        Filter::default(),
        // `format`, exact, over several values of the enum — including ones no issue
        // cited and one that holds a single package.
        Filter {
            format: Some("maven"),
            ..Filter::default()
        },
        Filter {
            format: Some("npm"),
            ..Filter::default()
        },
        Filter {
            format: Some("nuget"),
            ..Filter::default()
        },
        Filter {
            format: Some("cargo"),
            ..Filter::default()
        },
        Filter {
            format: Some("swift"),
            ..Filter::default()
        },
        // `namespace`, a PREFIX match: the shorter prefix selects both Maven groupIds. Each
        // row names the `format` whose component the namespace is — the filter's meaning is
        // chosen by the format, so `ListPackages` requires one beside it
        // (`crate::package_listing_filter_pairing` is the suite that pins that rule).
        Filter {
            format: Some("maven"),
            namespace: Some("com.example"),
            ..Filter::default()
        },
        Filter {
            format: Some("maven"),
            namespace: Some("com.example.tools"),
            ..Filter::default()
        },
        Filter {
            format: Some("maven"),
            namespace: Some("com"),
            ..Filter::default()
        },
        Filter {
            format: Some("npm"),
            namespace: Some("@"),
            ..Filter::default()
        },
        Filter {
            format: Some("swift"),
            namespace: Some("@"),
            ..Filter::default()
        },
        Filter {
            format: Some("generic"),
            namespace: Some("artifacts"),
            ..Filter::default()
        },
        // A namespace no package carries selects nothing — and in particular does NOT
        // select the formats that have no namespace at all.
        Filter {
            format: Some("maven"),
            namespace: Some("nope"),
            ..Filter::default()
        },
        Filter {
            format: Some("pypi"),
            namespace: Some("nope"),
            ..Filter::default()
        },
        // `packagePrefix`, a prefix match on the name.
        Filter {
            package_prefix: Some("lib-"),
            ..Filter::default()
        },
        Filter {
            package_prefix: Some("web-"),
            ..Filter::default()
        },
        Filter {
            package_prefix: Some("serde"),
            ..Filter::default()
        },
        Filter {
            package_prefix: Some("zzz"),
            ..Filter::default()
        },
        // The origin restrictions, exact, each value of each.
        Filter {
            publish: Some("ALLOW"),
            ..Filter::default()
        },
        Filter {
            publish: Some("BLOCK"),
            ..Filter::default()
        },
        Filter {
            upstream: Some("ALLOW"),
            ..Filter::default()
        },
        Filter {
            upstream: Some("BLOCK"),
            ..Filter::default()
        },
        // Combinations: every supplied filter has to hold.
        Filter {
            publish: Some("ALLOW"),
            upstream: Some("BLOCK"),
            ..Filter::default()
        },
        Filter {
            format: Some("maven"),
            package_prefix: Some("lib-"),
            publish: Some("ALLOW"),
            ..Filter::default()
        },
        Filter {
            format: Some("maven"),
            namespace: Some("com.other"),
            ..Filter::default()
        },
        Filter {
            format: Some("npm"),
            namespace: Some("com.example"),
            ..Filter::default()
        },
        Filter {
            format: Some("pypi"),
            publish: Some("BLOCK"),
            upstream: Some("BLOCK"),
            ..Filter::default()
        },
    ];

    for filter in cases {
        let query = format!("domain=filter-domain&repository=filter-repo{}", filter.query());
        let answer = list_packages(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {filter:?}: expected ListPackages 200, got {answer:?}"
        );
        let mut found = identities(&answer);
        found.sort();
        assert_eq!(
            found,
            filter.expected(CATALOG),
            "case {filter:?}: the listing must hold exactly the matching packages, got {:?}",
            answer.body
        );
    }
}

/// RULE 3: paging covers every matching package exactly once in one stable order, at every
/// page size, and the modelled `maxResults` bounds are honoured.
#[tokio::test]
async fn pages_cover_every_matching_package_exactly_once() {
    let router = emulator();
    seed(&router, "page-domain", "page-repo", CATALOG).await;

    let base = "domain=page-domain&repository=page-repo";
    // The unpaged order is the reference every page size must reproduce.
    let (reference, pages) = collect_pages(&router, base, None).await;
    assert_eq!(
        pages, 1,
        "case no maxResults: {} packages fit one default page, got {pages} pages",
        CATALOG.len()
    );
    assert_eq!(
        reference.len(),
        CATALOG.len(),
        "case no maxResults: expected every package, got {reference:?}"
    );

    // 1 and 1000 are the published bounds of `ListPackagesMaxResults`
    // (smithy_query: range 1..1000); the sizes between them straddle the catalog size.
    for size in [1usize, 2, 3, 4, 5, 10, 11, 12, 1000] {
        let (collected, pages) = collect_pages(&router, base, Some(size)).await;
        assert_eq!(
            collected, reference,
            "case page size {size}: paging must reproduce the unpaged order exactly once each, \
             got {collected:?} in {pages} page(s)"
        );
        let expected_pages = CATALOG.len().div_ceil(size.min(CATALOG.len()));
        assert_eq!(
            pages, expected_pages,
            "case page size {size}: expected {expected_pages} page(s), got {pages}"
        );
    }

    // A filtered page is `maxResults` MATCHING packages, not `maxResults` scanned.
    let filter = Filter {
        format: Some("maven"),
        ..Filter::default()
    };
    let filtered_base = format!("{base}{}", filter.query());
    for size in [1usize, 2, 3] {
        let (collected, _) = collect_pages(&router, &filtered_base, Some(size)).await;
        let mut sorted = collected.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            filter.expected(CATALOG),
            "case filtered page size {size}: expected every match exactly once, got {collected:?}"
        );
    }
    let first = list_packages(&router, &format!("{filtered_base}&max-results=1")).await;
    assert_eq!(
        identities(&first).len(),
        1,
        "a filtered first page must be full of matches, got {:?}",
        first.body
    );
    assert!(
        first.body["nextToken"].as_str().is_some(),
        "a filtered page with more matches left must carry a nextToken, got {:?}",
        first.body
    );

    // One past each published bound is refused by the modelled range.
    for out_of_range in ["0", "1001"] {
        let answer = list_packages(&router, &format!("{base}&max-results={out_of_range}")).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={out_of_range:?}: outside the modelled 1..1000 range must be \
             refused, got {answer:?}"
        );
    }

    // An empty repository is an empty page with no cursor, not an error.
    let empty_repo = create_repository(&router, "page-domain", "page-empty", None, &[]).await;
    assert_eq!(
        empty_repo.status, 200,
        "expected CreateRepository 200, got {empty_repo:?}"
    );
    let empty = list_packages(&router, "domain=page-domain&repository=page-empty").await;
    assert_eq!(
        (empty.status, identities(&empty), empty.body["nextToken"].clone()),
        (200, Vec::new(), Value::Null),
        "an empty repository must list no packages and no cursor, got {empty:?}"
    );
}

/// RULE 4 and RULE 5: what is not held is not found, a foreign cursor is refused, and a
/// package is visible only in the repository that holds it.
#[tokio::test]
async fn unknown_resources_and_foreign_cursors_are_refused() {
    let router = emulator();
    seed(&router, "scope-domain", "scope-repo", CATALOG).await;
    let other = create_repository(&router, "scope-domain", "scope-other", None, &[]).await;
    assert_eq!(
        other.status, 200,
        "expected CreateRepository 200, got {other:?}"
    );

    // RULE 5: the packages belong to `scope-repo` alone.
    let other_listing = list_packages(&router, "domain=scope-domain&repository=scope-other").await;
    assert_eq!(
        (other_listing.status, identities(&other_listing)),
        (200, Vec::new()),
        "a package must not be visible in another repository, got {other_listing:?}"
    );
    for case in CATALOG {
        let described =
            describe_package(&router, "scope-domain", "scope-other", &case.coordinate_query())
                .await;
        assert_eq!(
            (described.status, described.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {:?}: a package of another repository must not be found, got {described:?}",
            case.label()
        );
    }

    // RULE 4: a domain, a repository and a package that are not held.
    let cases: Vec<(&str, String)> = vec![
        ("unknown domain", "domain=no-domain&repository=scope-repo".to_string()),
        (
            "unknown repository",
            "domain=scope-domain&repository=no-repo".to_string(),
        ),
        (
            "domain pinned to an account that does not own it",
            "domain=scope-domain&repository=scope-repo&domain-owner=123456789012".to_string(),
        ),
    ];
    for (label, query) in cases {
        let answer = list_packages(&router, &query).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected ListPackages 404, got {answer:?}"
        );
    }

    // Pinning the owner correctly still resolves: the rule is that the owner must MATCH,
    // not that `domainOwner` must be absent.
    let pinned = list_packages(
        &router,
        &format!(
            "domain=scope-domain&repository=scope-repo&domain-owner={}",
            engine::EMULATOR_ACCOUNT_ID
        ),
    )
    .await;
    assert_eq!(
        (pinned.status, identities(&pinned).len()),
        (200, CATALOG.len()),
        "a domainOwner that matches the owner must resolve, got {pinned:?}"
    );

    // A package is addressed by its WHOLE triple: the same name under another format or
    // namespace is a different package, and that one is not held.
    let held = CATALOG[0];
    for (label, coordinate) in [
        (
            "right name, wrong format",
            format!("format=npm&package={}&namespace=com.example", held.package),
        ),
        (
            "right name, wrong namespace",
            format!("format=maven&package={}&namespace=com.nope", held.package),
        ),
        (
            "right name, no namespace",
            format!("format=maven&package={}", held.package),
        ),
        (
            "name no package carries",
            "format=maven&package=no-such-package&namespace=com.example".to_string(),
        ),
    ] {
        let described = describe_package(&router, "scope-domain", "scope-repo", &coordinate).await;
        assert_eq!(
            (described.status, described.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected DescribePackage 404, got {described:?}"
        );
    }

    // A cursor this listing did not mint names no place in its key order.
    let repo_token = list_packages(&router, "domain=scope-domain&repository=scope-repo&max-results=1")
        .await
        .body["nextToken"]
        .as_str()
        .expect("a first page of one of eleven carries a nextToken")
        .to_string();
    let foreign: Vec<(&str, String)> = vec![
        ("not a token at all", "not-a-token".to_string()),
        // Well-formed hex that decodes to UTF-8 but carries no cursor marker.
        ("unmarked hex", "616131".to_string()),
        // A cursor minted for ANOTHER repository's package listing.
        ("another repository's cursor", repo_token),
    ];
    for (label, token) in foreign {
        let answer = list_packages(
            &router,
            &format!("domain=scope-domain&repository=scope-other&next-token={token}"),
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (400, "ValidationException".to_string()),
            "case {label:?}: a foreign nextToken must be refused, got {answer:?}"
        );
    }
}
