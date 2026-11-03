// SPDX-License-Identifier: CC0-1.0

//! What `ListServices` enumerates: the catalog's NAMESPACE, its shape, and that it
//! is big enough for paging to be real.
//!
//! The rule, in one sentence: `ListServices` enumerates the identifiers the
//! Management Console publishes a service's home under — the namespace
//! `visibleServices` keys on — and not a service's SigV4 signing name / endpoint
//! prefix, which is the namespace a client signs and routes with
//! (`smithy_query(query="doc", shape="ListServices")`: the returned identifiers are
//! the ones "you can use as values for the `visibleServices` setting", which
//! "controls only the appearance of services in the Amazon Web Services Management
//! Console").
//!
//! The two namespaces overlap — `s3`, `ec2` and `lambda` are the same string in
//! both, and are the three the model's own `@examples` output lists — so a suite
//! that only asserted "some identifiers are present" could not tell a
//! prefix-derived catalog from a console one. The table below therefore spans the
//! services where the two namespaces DIFFER, each row naming the console
//! identifier that must be in the catalog and the endpoint prefix that must not.
//!
//! What this suite does NOT assert: AWS's own catalog "var[ies] by Amazon Web
//! Services partition" (the operation doc) and grows with every launch, so its
//! exact membership on a given day is environment data rather than a rule. The
//! rows below are drawn from the namespace, never from one recorded page.

use crate::wire::{build_router, is_legal_service, list_page, whole_catalog};

/// The modelled `ServiceList` `@length` max
/// (`smithy_query(query="shape", shape="ServiceList")`): one page can carry no more
/// than this, so the catalog stays inside it to remain reportable in full.
const SERVICE_LIST_MAX_ITEMS: usize = 500;

/// The least a catalog of this namespace may hold and still be a discovery list a
/// console client can work from: AWS publishes service homes in the hundreds, and
/// the operation tells callers to "use pagination to retrieve all results", which
/// is only true of a catalog that outruns the page sizes clients ask for.
const MATERIALLY_COMPLETE: usize = 200;

/// The whole catalog, assembled by WALKING the pages rather than by asking for it
/// unsized: the rows below are about the catalog's membership and order, so they
/// must not also depend on an unsized page being able to carry every entry.
///
/// Bounded by the modelled `ServiceList` max in BOTH dimensions — entries seen and
/// pages requested — so a listing that kept minting tokens (or minted one for an
/// empty page) fails the assertion instead of walking forever.
async fn reported_catalog() -> Vec<String> {
    const PAGE_SIZE: i64 = 50;
    let max_pages = SERVICE_LIST_MAX_ITEMS / PAGE_SIZE as usize + 1;

    let router = build_router();
    let mut entries: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page_number in 1..=max_pages {
        let (page, next) = list_page(&router, Some(PAGE_SIZE), token.as_deref()).await;
        entries.extend(page);
        assert!(
            entries.len() <= SERVICE_LIST_MAX_ITEMS,
            "walking the catalog must terminate inside the modelled ServiceList max of \
             {SERVICE_LIST_MAX_ITEMS}, got {} entries by page {page_number}",
            entries.len()
        );
        match next {
            None => return entries,
            Some(next) => token = Some(next),
        }
    }
    panic!(
        "walking the catalog at {PAGE_SIZE} entries a page must end within {max_pages} pages, \
         got {} entries and another token",
        entries.len()
    );
}

/// The namespace rule, over the services whose console identifier and endpoint
/// prefix are DIFFERENT strings: the console identifier is in the catalog, the
/// endpoint prefix is not.
///
/// Rows differ along how the two namespaces diverge: a different spelling of the
/// same service (`cognito` vs `cognito-idp`, `emr` vs `elasticmapreduce`), a
/// service the console files under another service's home (`logs` and `monitoring`
/// both live under `cloudwatch`), a console identifier much longer than the prefix
/// (`directoryservice` vs `ds`), and a prefix with no console home at all
/// (`sts`, `execute-api`), where the row asserts only the absence.
#[tokio::test]
async fn the_catalog_is_the_console_identifier_namespace_not_the_endpoint_prefix_one() {
    let catalog = reported_catalog().await;

    // (case, the console identifier this service's home is published under,
    //  the SigV4 endpoint prefix that must NOT be reported as an identifier)
    let rows: [(&str, Option<&str>, &str); 9] = [
        ("Amazon Cognito", Some("cognito"), "cognito-idp"),
        ("CloudWatch Logs", Some("cloudwatch"), "logs"),
        ("CloudWatch metrics", Some("cloudwatch"), "monitoring"),
        ("Amazon EMR", Some("emr"), "elasticmapreduce"),
        ("AWS Directory Service", Some("directoryservice"), "ds"),
        ("Amazon EFS", Some("efs"), "elasticfilesystem"),
        ("Amazon Keyspaces", Some("keyspaces"), "cassandra"),
        // Signing names with no console service home of their own.
        ("AWS STS", None, "sts"),
        ("API Gateway's data plane", None, "execute-api"),
    ];

    for (case, console_identifier, endpoint_prefix) in rows {
        if let Some(identifier) = console_identifier {
            assert!(
                catalog.iter().any(|entry| entry == identifier),
                "case {case:?}: expected the console identifier {identifier:?} in the catalog, \
                 got a catalog of {} entries starting {:?}",
                catalog.len(),
                &catalog[..catalog.len().min(5)]
            );
        }
        assert!(
            !catalog.iter().any(|entry| entry == endpoint_prefix),
            "case {case:?}: expected the endpoint prefix {endpoint_prefix:?} NOT to be reported \
             as a console service identifier, got it at index {:?}",
            catalog.iter().position(|entry| entry == endpoint_prefix)
        );
    }
}

/// The same namespace, spanned by the FORM of an identifier rather than by the
/// contrast with a prefix: every row is a console service home the issue's
/// recording did not cite, and the rows differ along segment count, segment
/// content (letters, letters and digits), and size — the shortest identifier the
/// model's `@length(1, 64)` admits through the longest entry this catalog holds.
#[tokio::test]
async fn identifiers_across_the_namespace_are_reported_whatever_their_form() {
    let catalog = reported_catalog().await;

    // (case, identifier)
    let rows: [(&str, &str); 10] = [
        ("one segment, letters only", "cloudformation"),
        ("one segment at the model's minimum length", "q"),
        ("one segment ending in a digit", "cloud9"),
        ("one segment with digits inside it", "route53"),
        ("two characters, letter then digit", "m2"),
        ("letters then a version digit", "wafv2"),
        ("two hyphen-separated segments", "redshift-serverless"),
        ("two segments, the second a digit-bearing word", "workspaces-web"),
        ("three hyphen-separated segments", "network-security-manager"),
        ("one long unsegmented word", "applicationrecoverycontroller"),
    ];

    for (case, identifier) in rows {
        assert!(
            is_legal_service(identifier),
            "case {case:?}: this row must itself be a value the model admits, got {identifier:?}"
        );
        assert!(
            catalog.iter().any(|entry| entry == identifier),
            "case {case:?}: expected {identifier:?} in the catalog, got {} entries",
            catalog.len()
        );
    }
}

/// Every entry is a value the model admits as a `Service`, and the catalog is one
/// stable, duplicate-free order — the invariant an offset-bearing page token
/// depends on.
#[tokio::test]
async fn catalog_entries_are_legal_service_identifiers_in_one_stable_order() {
    let catalog = reported_catalog().await;

    for entry in &catalog {
        assert!(
            is_legal_service(entry),
            "every entry must satisfy Service @length(1, 64) and \
             @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, got {entry:?}"
        );
    }
    for pair in catalog.windows(2) {
        assert!(
            pair[0] < pair[1],
            "the catalog must be strictly increasing (sorted and duplicate-free), got \
             {:?} before {:?}",
            pair[0],
            pair[1]
        );
    }
}

/// The catalog is big enough for the operation's own advice — "use pagination to
/// retrieve all results" — to mean something: a client sizing pages the way a
/// console does walks several of them, and the whole catalog still fits the
/// modelled `ServiceList` bound.
#[tokio::test]
async fn the_catalog_spans_several_pages_at_the_page_sizes_a_client_asks_for() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;

    assert!(
        (MATERIALLY_COMPLETE..=SERVICE_LIST_MAX_ITEMS).contains(&catalog.len()),
        "expected a catalog of at least {MATERIALLY_COMPLETE} entries and no more than the \
         modelled ServiceList max of {SERVICE_LIST_MAX_ITEMS}, got {}",
        catalog.len()
    );

    for size in [10_i64, 25, 50, 100] {
        let (page, token) = list_page(&router, Some(size), None).await;
        assert_eq!(
            page.len(),
            size as usize,
            "case maxResults={size}: a full first page must carry exactly {size} entries, got {}",
            page.len()
        );
        assert!(
            token.is_some(),
            "case maxResults={size}: a catalog of {} entries must offer a second page",
            catalog.len()
        );
        assert_eq!(
            page,
            catalog[..size as usize].to_vec(),
            "case maxResults={size}: the first page must be the catalog's own first slice, got \
             {page:?}"
        );
    }
}
