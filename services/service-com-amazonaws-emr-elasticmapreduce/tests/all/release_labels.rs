// SPDX-License-Identifier: CC0-1.0

//! `ListReleaseLabels` over the wire: the release catalog, its filters and its pages.
//!
//! THE RULE these cases assert: `ListReleaseLabels` reports the release labels this
//! service publishes, newest release first in NUMERIC version order, narrowed by the
//! `Prefix` and `Application` halves of `Filters`, and cut into pages of at most
//! `MaxResults` (default 100) joined by a `NextToken` that is only a resume point for
//! the filters it was minted with.
//!
//! `DescribeReleaseLabel`, which reports ONE release of the same catalog, has its own
//! suite in `release_label_catalog.rs`; the application filter is cross-checked against
//! it below, so the two operations are held to one catalog.
//!
//! Every case drives the real HTTP path through the router's `dispatch` (the shared
//! [`Emr`] harness), so these are the bytes an SDK client sends. The assertions are
//! stated against the listing the emulator itself reports — a filtered page is checked
//! against the unfiltered listing rather than against a copy of the catalog — so they
//! pin the RULE and not one recorded answer.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_refused, at, minimal_request, Emr};

/// The largest page a caller may ask for, and the page size when it asks for none:
/// "Defines the maximum number of release labels to return in a single response. The
/// default is 100." with `MaxResultsNumber` ranged `1..100`
/// (`smithy_query(query="operation", operation="ListReleaseLabels")`).
const MAX_RESULTS_CEILING: i64 = 100;

/// One `Prefix` filter and whether the catalog can match it at all.
#[derive(Debug)]
struct PrefixCase {
    prefix: &'static str,
    matches_something: bool,
}

/// `Prefix` filters spanning the shape of the member — a whole series, a partial
/// minor, the bare `emr-` stem, the empty prefix every label starts with, and
/// prefixes outside the published releases. "Optional release label version prefix
/// filter. For example, `emr-5`." (`ReleaseLabelFilter$Prefix` doc.)
const PREFIX_FILTERS: &[PrefixCase] = &[
    PrefixCase { prefix: "", matches_something: true },
    PrefixCase { prefix: "emr", matches_something: true },
    PrefixCase { prefix: "emr-", matches_something: true },
    PrefixCase { prefix: "emr-5", matches_something: true },
    PrefixCase { prefix: "emr-6", matches_something: true },
    PrefixCase { prefix: "emr-7", matches_something: true },
    PrefixCase { prefix: "emr-6.1", matches_something: true },
    PrefixCase { prefix: "emr-5.3", matches_something: true },
    // No 4.x release is in the catalog, and neither is a label of another form.
    PrefixCase { prefix: "emr-4", matches_something: false },
    PrefixCase { prefix: "emr-99", matches_something: false },
    PrefixCase { prefix: "EMR-6", matches_something: false },
    PrefixCase { prefix: "not-a-release", matches_something: false },
];

/// Page sizes: both published bounds of `MaxResultsNumber` (1 and 100) and sizes in
/// between that do and do not divide the catalog evenly.
const PAGE_SIZES: &[i64] = &[1, 2, 3, 5, 7, MAX_RESULTS_CEILING];

/// The labels one `ListReleaseLabels` call reports, and the token it answered with.
async fn page(emr: &Emr, case: &str, request: Value) -> (Vec<String>, Option<String>) {
    let out = emr.ok("ListReleaseLabels", request).await;
    let listed = at(&out, &["ReleaseLabels"]).as_array().cloned().unwrap_or_else(|| {
        panic!("case {case:?}: ListReleaseLabels must answer a ReleaseLabels list, got {out}")
    });
    let labels = listed
        .iter()
        .map(|label| {
            label
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported release label is a string, got {out}")
                })
                .to_string()
        })
        .collect();
    let token = at(&out, &["NextToken"]).as_str().map(str::to_string);
    (labels, token)
}

/// Every label a request reports, following its tokens to the end of the listing.
/// Answers the labels and, for the caller to assert on, one entry per page served.
async fn walk(emr: &Emr, case: &str, request: Value) -> (Vec<String>, Vec<Vec<String>>) {
    let mut labels = Vec::new();
    let mut pages = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = request.clone();
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (served, next) = page(emr, case, request).await;
        labels.extend(served.iter().cloned());
        pages.push(served);
        assert!(
            pages.len() <= 1_000,
            "case {case:?}: the listing must terminate, got {} pages",
            pages.len()
        );
        match next {
            Some(next) => token = Some(next),
            None => return (labels, pages),
        }
    }
}

/// The whole catalog in the order the service reports it: one unfiltered call, which
/// the `MaxResults` default of 100 serves in full.
async fn whole_catalog(emr: &Emr) -> Vec<String> {
    let (labels, token) = page(emr, "no filters", json!({})).await;
    assert!(
        token.is_none(),
        "case \"no filters\": the default page of 100 holds the whole catalog, \
         so no NextToken is due, got {token:?}"
    );
    assert!(
        !labels.is_empty(),
        "case \"no filters\": the Region publishes releases, got {labels:?}"
    );
    labels
}

/// The version a release label carries, as the numbers of its dotted components:
/// labels are "in the format emr-x.x.x" (`ListSupportedInstanceTypes$ReleaseLabel`
/// doc), and those components compare as numbers, not as text.
fn version_of(case: &str, label: &str) -> Vec<u32> {
    let body = label.strip_prefix("emr-").unwrap_or_else(|| {
        panic!("case {case:?}: a release label is emr-x.x.x, got {label:?}")
    });
    body.split('.')
        .map(|component| {
            component.parse::<u32>().unwrap_or_else(|_| {
                panic!("case {case:?}: every component of {label:?} is a number")
            })
        })
        .collect()
}

#[tokio::test]
async fn the_catalog_is_reported_newest_release_first() {
    let emr = Emr::new();
    let labels = whole_catalog(&emr).await;

    for label in &labels {
        let version = version_of("no filters", label);
        assert!(
            version.len() == 3,
            "case {label:?}: a release label carries three version components, got {version:?}"
        );
    }

    // Strictly descending: the latest release leads, no release is reported twice.
    for pair in labels.windows(2) {
        let (newer, older) = (&pair[0], &pair[1]);
        assert!(
            version_of("ordering", newer) > version_of("ordering", older),
            "case {newer:?} before {older:?}: the catalog is reported newest first, \
             got {labels:?}"
        );
    }

    // Numeric, not lexicographic. Each row is a pair of published releases whose text
    // order is the opposite of their version order (`emr-6.9.0` sorts after
    // `emr-6.10.0` as text), so a listing sorted as text fails here. Rows come from
    // the catalog in `src/state.rs`, whose source is the Amazon EMR Release Guide.
    for (newer, older) in [
        ("emr-6.10.0", "emr-6.9.0"),
        ("emr-6.11.0", "emr-6.9.0"),
        ("emr-5.36.2", "emr-5.30.2"),
        ("emr-7.0.0", "emr-6.15.0"),
    ] {
        let newer_at = labels.iter().position(|label| label == newer);
        let older_at = labels.iter().position(|label| label == older);
        if let (Some(newer_at), Some(older_at)) = (newer_at, older_at) {
            assert!(
                newer_at < older_at,
                "case {newer:?} vs {older:?}: the newer release is reported first, \
                 got positions {newer_at} and {older_at} in {labels:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_catalog_is_independent_of_the_store_and_a_read_changes_nothing() {
    let emr = Emr::new();
    let before = whole_catalog(&emr).await;

    // Clusters, including one naming a release label, are account state; the catalog
    // is service data and neither depends on them nor is changed by being read.
    emr.ok("RunJobFlow", minimal_request("catalog-one")).await;
    let mut named = minimal_request("catalog-two");
    named["ReleaseLabel"] = json!(before[0].clone());
    emr.ok("RunJobFlow", named).await;

    for attempt in 1..=3 {
        let again = whole_catalog(&emr).await;
        assert_eq!(
            again, before,
            "case attempt {attempt}: reading the catalog must answer the same releases, \
             got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_prefix_filter_keeps_exactly_the_labels_that_start_with_it() {
    let emr = Emr::new();
    let catalog = whole_catalog(&emr).await;

    for case in PREFIX_FILTERS {
        let label = format!("Prefix={:?}", case.prefix);
        let expected: Vec<String> = catalog
            .iter()
            .filter(|release| release.starts_with(case.prefix))
            .cloned()
            .collect();
        let (reported, token) = page(
            &emr,
            &label,
            json!({ "Filters": { "Prefix": case.prefix } }),
        )
        .await;

        assert_eq!(
            reported, expected,
            "case {label}: expected the catalog's labels with that prefix, in catalog \
             order, got {reported:?}"
        );
        assert_eq!(
            !reported.is_empty(),
            case.matches_something,
            "case {label}: expected matches_something={}, got {reported:?}",
            case.matches_something
        );
        assert!(
            token.is_none(),
            "case {label}: one page holds every match, so no NextToken is due, got {token:?}"
        );
    }
}

#[tokio::test]
async fn the_application_filter_keeps_the_releases_that_ship_it() {
    let emr = Emr::new();
    let catalog = whole_catalog(&emr).await;

    /// A filter, and whether the catalog can match it.
    struct AppCase {
        application: &'static str,
        matches_something: bool,
    }

    // Spanning the two forms the member documents — "spark@2.1.0" is name@version,
    // "spark" is the name alone — plus capitalisation and applications or versions no
    // release ships. The applications named are those the Amazon EMR Release Guide
    // lists for the releases in `src/state.rs`'s catalog.
    let cases = [
        AppCase { application: "hadoop", matches_something: true },
        AppCase { application: "spark", matches_something: true },
        AppCase { application: "HIVE", matches_something: true },
        AppCase { application: "Presto", matches_something: true },
        AppCase { application: "trino", matches_something: true },
        AppCase { application: "ganglia", matches_something: true },
        AppCase { application: "spark@3.5.1", matches_something: true },
        AppCase { application: "hadoop@2.8.5", matches_something: true },
        AppCase { application: "spark@0.0.0", matches_something: false },
        AppCase { application: "hadoop@", matches_something: false },
        AppCase { application: "not-an-application", matches_something: false },
    ];

    for case in &cases {
        let label = format!("Application={:?}", case.application);
        let (reported, token) = page(
            &emr,
            &label,
            json!({ "Filters": { "Application": case.application } }),
        )
        .await;

        assert!(
            token.is_none(),
            "case {label}: one page holds every match, so no NextToken is due, got {token:?}"
        );
        assert_eq!(
            !reported.is_empty(),
            case.matches_something,
            "case {label}: expected matches_something={}, got {reported:?}",
            case.matches_something
        );
        // Whatever it keeps, it keeps in catalog order and keeps nothing else.
        let subsequence: Vec<String> = catalog
            .iter()
            .filter(|release| reported.contains(release))
            .cloned()
            .collect();
        assert_eq!(
            reported, subsequence,
            "case {label}: a filtered listing is the catalog with releases dropped, \
             got {reported:?}"
        );

        // A version narrows: `name@version` keeps a subset of what `name` keeps.
        if let Some((name, _)) = case.application.split_once('@') {
            let (unversioned, _) = page(
                &emr,
                &format!("Application={name:?}"),
                json!({ "Filters": { "Application": name } }),
            )
            .await;
            for release in &reported {
                assert!(
                    unversioned.contains(release),
                    "case {label}: {release:?} must also match the bare {name:?}, \
                     got {unversioned:?}"
                );
            }
        }
    }

    // The name halves are compared without regard to case, so the same filter spelled
    // three ways is the same filter.
    let mut spellings = Vec::new();
    for spelling in ["hadoop", "HADOOP", "Hadoop"] {
        let (reported, _) = page(
            &emr,
            &format!("Application={spelling:?}"),
            json!({ "Filters": { "Application": spelling } }),
        )
        .await;
        spellings.push((spelling, reported));
    }
    for (spelling, reported) in &spellings {
        assert_eq!(
            reported, &spellings[0].1,
            "case Application={spelling:?}: capitalisation must not change the match, \
             got {reported:?}"
        );
        // Every Amazon EMR 5.x, 6.x and 7.x release ships Hadoop (the Release Guide's
        // application tables), so this filter drops nothing.
        assert_eq!(
            reported, &catalog,
            "case Application={spelling:?}: every catalogued release ships Hadoop, \
             got {reported:?}"
        );
    }

    // Both halves of `Filters` narrow together: the two filters applied at once keep
    // exactly the releases each keeps on its own.
    let (by_prefix, _) = page(&emr, "Prefix only", json!({ "Filters": { "Prefix": "emr-6" } })).await;
    let (by_application, _) =
        page(&emr, "Application only", json!({ "Filters": { "Application": "trino" } })).await;
    let (by_both, _) = page(
        &emr,
        "Prefix and Application",
        json!({ "Filters": { "Prefix": "emr-6", "Application": "trino" } }),
    )
    .await;
    let intersection: Vec<String> = by_prefix
        .iter()
        .filter(|release| by_application.contains(release))
        .cloned()
        .collect();
    assert_eq!(
        by_both, intersection,
        "case \"Prefix and Application\": both filters narrow together, got {by_both:?} \
         against {by_prefix:?} and {by_application:?}"
    );
    assert!(
        !by_both.is_empty(),
        "case \"Prefix and Application\": a 6.x release ships Trino, got {by_both:?}"
    );
}

#[tokio::test]
async fn the_application_filter_agrees_with_describe_release_label() {
    let emr = Emr::new();
    let catalog = whole_catalog(&emr).await;

    // What each release bundles, read back out of the service through the operation
    // that reports one release's applications. Nothing here restates the catalog: the
    // expectation for every filter below is derived from these answers.
    let mut bundles: Vec<(String, Vec<String>)> = Vec::new();
    for label in &catalog {
        let described = emr
            .ok("DescribeReleaseLabel", json!({ "ReleaseLabel": label }))
            .await;
        let applications = at(&described, &["Applications"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {label:?}: DescribeReleaseLabel must report Applications, got {described}")
            })
            .iter()
            .map(|application| {
                at(application, &["Name"])
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        bundles.push((label.clone(), applications));
    }

    // One application that every release bundles, one that arrives part way through
    // the catalog, and one Amazon EMR dropped: each must be kept by the filter for
    // exactly the releases `DescribeReleaseLabel` reports it for.
    for application in ["hadoop", "trino", "iceberg", "ganglia", "jupyterhub"] {
        let expected: Vec<String> = bundles
            .iter()
            .filter(|(_, applications)| applications.iter().any(|name| name == application))
            .map(|(label, _)| label.clone())
            .collect();
        let (reported, _) = page(
            &emr,
            &format!("Application={application:?}"),
            json!({ "Filters": { "Application": application } }),
        )
        .await;
        assert_eq!(
            reported, expected,
            "case Application={application:?}: the filter must keep exactly the releases \
             DescribeReleaseLabel reports it for, got {reported:?}"
        );
        assert!(
            !reported.is_empty() && reported.len() <= catalog.len(),
            "case Application={application:?}: expected a non-empty part of the catalog, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn max_results_cuts_the_listing_into_pages_that_rejoin_into_it() {
    let emr = Emr::new();
    let catalog = whole_catalog(&emr).await;

    for &size in PAGE_SIZES {
        let case = format!("MaxResults={size}");
        let (labels, pages) = walk(&emr, &case, json!({ "MaxResults": size })).await;

        assert_eq!(
            labels, catalog,
            "case {case}: paging through the listing must report the whole catalog in \
             order, got {labels:?}"
        );
        let expected_pages = catalog.len().div_ceil(size as usize);
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case}: expected {expected_pages} pages for {} releases, got {:?}",
            catalog.len(),
            pages.iter().map(Vec::len).collect::<Vec<_>>()
        );
        for (index, served) in pages.iter().enumerate() {
            let last = index + 1 == pages.len();
            if last {
                assert!(
                    !served.is_empty() && served.len() <= size as usize,
                    "case {case}: the last page holds 1..={size} releases, got {served:?}"
                );
            } else {
                assert_eq!(
                    served.len(),
                    size as usize,
                    "case {case}: page {index} must be full, got {served:?}"
                );
            }
        }
    }

    // A filtered listing pages the same way, over its own narrower list.
    let filtered = json!({ "Filters": { "Prefix": "emr-6" }, "MaxResults": 2 });
    let (paged, pages) = walk(&emr, "Prefix=emr-6 MaxResults=2", filtered).await;
    let (whole, _) = page(&emr, "Prefix=emr-6", json!({ "Filters": { "Prefix": "emr-6" } })).await;
    assert_eq!(
        paged, whole,
        "case \"Prefix=emr-6 MaxResults=2\": the pages rejoin into the filtered listing, \
         got {paged:?}"
    );
    assert!(
        pages.len() > 1,
        "case \"Prefix=emr-6 MaxResults=2\": more than one page is due for {} releases, \
         got {} page(s)",
        whole.len(),
        pages.len()
    );
}

#[tokio::test]
async fn max_results_outside_the_published_range_is_refused() {
    let emr = Emr::new();

    // The modelled `1..100` range, at each bound and one past it. The framework
    // refuses a modelled constraint before the handler runs, so the code it answers
    // with is the framework's; what this case pins is the 400 and that the bounds
    // themselves are accepted.
    for (case, size, accepted) in [
        ("one below the floor", 0, false),
        ("the floor", 1, true),
        ("the ceiling", MAX_RESULTS_CEILING, true),
        ("one past the ceiling", MAX_RESULTS_CEILING + 1, false),
    ] {
        let (status, payload) = emr
            .call("ListReleaseLabels", json!({ "MaxResults": size }))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} (MaxResults={size}): expected HTTP 200, got {status} with {payload}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (MaxResults={size}): expected HTTP 400, got {status} with {payload}"
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_resumes_only_the_listing_and_filters_it_was_minted_for() {
    let emr = Emr::new();
    let catalog = whole_catalog(&emr).await;
    let (six_x, _) = page(&emr, "Prefix=emr-6", json!({ "Filters": { "Prefix": "emr-6" } })).await;

    // A token minted for the `emr-6` listing, one release in.
    let (first, token) = page(
        &emr,
        "Prefix=emr-6 MaxResults=1",
        json!({ "Filters": { "Prefix": "emr-6" }, "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        first,
        vec![six_x[0].clone()],
        "case \"Prefix=emr-6 MaxResults=1\": the first page is the newest match, got {first:?}"
    );
    let token = token.expect("a token is due while matches remain");

    // Used with the filters it was minted for, it resumes right after that release.
    let (resumed, _) = page(
        &emr,
        "same filters",
        json!({ "Filters": { "Prefix": "emr-6" }, "MaxResults": 1, "NextToken": &token }),
    )
    .await;
    assert_eq!(
        resumed,
        vec![six_x[1].clone()],
        "case \"same filters\": resuming reports the next match, got {resumed:?}"
    );

    // "The ListReleaseLabels request fails if the identity … and all filtering
    // parameters are different from the original request, or if the NextToken is
    // expired or tampered with." (`ListReleaseLabelsInput$NextToken` doc.) One row per
    // way a token can fail to be a resume point for the request carrying it.
    let tampered = ::aws_smithy_types::base64::encode("emr/v1/releaseLabels/0:|0:/emr-99.9.9");
    let rows: Vec<(&str, Value)> = vec![
        (
            "another prefix",
            json!({ "Filters": { "Prefix": "emr-5" }, "NextToken": &token }),
        ),
        ("no filters", json!({ "NextToken": &token })),
        (
            "an application filter",
            json!({ "Filters": { "Application": "spark" }, "NextToken": &token }),
        ),
        (
            "a longer prefix",
            json!({ "Filters": { "Prefix": "emr-6." }, "NextToken": &token }),
        ),
        ("not a token at all", json!({ "NextToken": "emr-6.15.0" })),
        ("not even base64", json!({ "NextToken": "!!not-base64!!" })),
        ("a tampered cursor", json!({ "NextToken": tampered })),
    ];
    for (case, request) in rows {
        let (status, payload) = emr.call("ListReleaseLabels", request).await;
        assert_refused(case, status, &payload, "InvalidRequestException");
    }

    // A token for the unfiltered listing resumes the unfiltered listing.
    let (_, unfiltered_token) = page(&emr, "MaxResults=1", json!({ "MaxResults": 1 })).await;
    let unfiltered_token = unfiltered_token.expect("a token is due while releases remain");
    let (second, _) = page(
        &emr,
        "unfiltered resume",
        json!({ "MaxResults": 1, "NextToken": unfiltered_token }),
    )
    .await;
    assert_eq!(
        second,
        vec![catalog[1].clone()],
        "case \"unfiltered resume\": expected the second release of the catalog, got {second:?}"
    );
}
