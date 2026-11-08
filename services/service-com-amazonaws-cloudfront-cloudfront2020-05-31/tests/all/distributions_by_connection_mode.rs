// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByConnectionMode`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under the connection mode its STORED
//! configuration describes — `tenant-only` when the configuration describes a
//! multi-tenant distribution (`ConnectionMode` names it, or a `TenantConfig`, which
//! "only supports multi-tenant distributions", is present), `direct` otherwise, a
//! configuration naming no tenancy member included — so the two listings PARTITION the
//! store: every distribution `ListDistributions` reports appears in exactly one of them,
//! each summary is the one the store holds, and each listing pages over its own matching
//! set with `Marker` and `MaxItems` exactly as the unfiltered listing does.
//!
//! The rows below span every arrangement of the two tenancy members, several
//! distributions per mode so a page can cut inside either set, and every admissible page
//! size. The wire harness (the router, the request helpers and the element scanner) is
//! the one [`crate::distribution_lifecycle`] already builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ITEMS;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, is_distribution_id, list_distributions, Answer, Node,
    API_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

/// `ConnectionMode`'s values, from the model's enum
/// (`smithy_query(query="shape",shape="ConnectionMode")`): exactly these two.
const CONNECTION_MODES: [&str; 2] = ["direct", "tenant-only"];

/// A `TenantConfig` element. `ParameterDefinitions` is a list whose member carries no
/// `xmlName` (`smithy_query(query="shape",shape="ParameterDefinitions")`), so each entry
/// travels in the restXml default `<member>` element.
const TENANT_CONFIG: &str = "<TenantConfig><ParameterDefinitions>\
     <member><Name>tenantName</Name><Definition><StringSchema>\
     <Required>true</Required></StringSchema></Definition></member>\
     </ParameterDefinitions></TenantConfig>";

/// One row: a configuration, and the connection mode it is therefore listed under.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// What the row adds to the `DistributionConfig` beyond the common members.
    extra: &'static str,
    /// Which of [`CONNECTION_MODES`] this configuration describes.
    mode: &'static str,
}

/// Every arrangement of the two members that decide a distribution's tenancy, with more
/// than one row per mode so a page boundary can fall inside either set.
const ROWS: &[Row] = &[
    Row {
        case: "no-tenancy-member-at-all",
        extra: "",
        mode: "direct",
    },
    Row {
        case: "connection-mode-direct",
        extra: "<ConnectionMode>direct</ConnectionMode>",
        mode: "direct",
    },
    Row {
        case: "connection-mode-direct-second",
        extra: "<ConnectionMode>direct</ConnectionMode>",
        mode: "direct",
    },
    Row {
        case: "connection-mode-tenant-only",
        extra: "<ConnectionMode>tenant-only</ConnectionMode>",
        mode: "tenant-only",
    },
    Row {
        case: "tenant-config-alone",
        extra: TENANT_CONFIG,
        mode: "tenant-only",
    },
    Row {
        case: "tenant-config-and-connection-mode",
        extra: concat!(
            "<ConnectionMode>tenant-only</ConnectionMode>",
            "<TenantConfig><ParameterDefinitions>\
             <member><Name>region</Name><Definition><StringSchema>\
             <Required>false</Required></StringSchema></Definition></member>\
             </ParameterDefinitions></TenantConfig>"
        ),
        mode: "tenant-only",
    },
];

/// The `DistributionConfig` a row submits: one origin and a default cache behaviour, plus
/// whatever tenancy members the row adds.
fn config_xml(caller_reference: &str, config_extra: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment>\
         <Enabled>true</Enabled>{config_extra}\
         </DistributionConfig>"
    )
}

/// One `ListDistributionsByConnectionMode` call: `GET
/// /2020-05-31/distributionsByConnectionMode/{ConnectionMode}` plus the query the caller
/// pages with.
async fn list_by_mode(router: &MultiServiceRouter, mode: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distributionsByConnectionMode/{mode}{query}"),
        None,
        None,
    )
    .await
}

/// The `DistributionSummary` elements of a `DistributionList` answer, in document order.
fn summaries(list: &Node) -> Vec<&Node> {
    list.child("Items")
        .map(|items| items.children_named("DistributionSummary"))
        .unwrap_or_default()
}

/// The ids of a `DistributionList` answer, in document order.
fn listed_ids(list: &Node) -> Vec<String> {
    summaries(list)
        .into_iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Create every row, returning `(case, mode, id)` per created distribution.
async fn create_rows(router: &MultiServiceRouter) -> Vec<(&'static str, &'static str, String)> {
    let mut created = Vec::new();
    for row in ROWS {
        let body = config_xml(&format!("ref-{}", row.case), row.extra);
        let answer = create_distribution(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_distribution_id(&id),
            "case {}: expected a minted distribution id, got {id:?}",
            row.case
        );
        created.push((row.case, row.mode, id));
    }
    created
}

/// Walk every page of one mode, returning the ids in the order they were served.
async fn walk(router: &MultiServiceRouter, mode: &str, page_size: Option<i32>) -> Vec<String> {
    let label = match page_size {
        None => format!("{mode}/default"),
        Some(size) => format!("{mode}/MaxItems={size}"),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    // At most one page per distribution plus one, so a listing that never stops
    // truncating fails here rather than looping forever.
    for page in 0..=ROWS.len() + 1 {
        let query = match (page_size, marker.as_deref()) {
            (None, None) => String::new(),
            (None, Some(value)) => format!("?Marker={value}"),
            (Some(size), None) => format!("?MaxItems={size}"),
            (Some(size), Some(value)) => format!("?MaxItems={size}&Marker={value}"),
        };
        let answer = list_by_mode(router, mode, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {label} page {page}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        assert_eq!(
            list.text_at("Marker"),
            Some(marker.as_deref().unwrap_or_default()),
            "case {label} page {page}: expected the Marker the request carried, got {:?}",
            list.text_at("Marker")
        );
        // "The value you provided for the MaxItems request parameter."
        assert_eq!(
            list.text_at("MaxItems"),
            Some(page_size.unwrap_or(MAX_ITEMS).to_string().as_str()),
            "case {label} page {page}: expected MaxItems to echo the page size, got {:?}",
            list.text_at("MaxItems")
        );
        let ids = listed_ids(list);
        assert_eq!(
            list.text_at("Quantity"),
            Some(ids.len().to_string().as_str()),
            "case {label} page {page}: expected Quantity to state the items served ({}), got {:?}",
            ids.len(),
            list.text_at("Quantity")
        );
        if let Some(size) = page_size {
            assert!(
                i32::try_from(ids.len()).unwrap_or(i32::MAX) <= size.min(MAX_ITEMS),
                "case {label} page {page}: expected at most {} items, got {}",
                size.min(MAX_ITEMS),
                ids.len()
            );
        }
        collected.extend(ids.iter().cloned());

        let next_marker = list.text_at("NextMarker").map(str::to_string);
        let is_truncated = list.text_at("IsTruncated").unwrap_or_default().to_string();
        match next_marker {
            Some(next) => {
                assert_eq!(
                    is_truncated, "true",
                    "case {label} page {page}: a NextMarker means IsTruncated true, got \
                     {is_truncated:?}"
                );
                assert_eq!(
                    ids.last().map(String::as_str),
                    Some(next.as_str()),
                    "case {label} page {page}: expected NextMarker to be the id this page ended \
                     at, got {next:?}"
                );
                marker = Some(next);
            }
            None => {
                assert_eq!(
                    is_truncated, "false",
                    "case {label} page {page}: no NextMarker means IsTruncated false, got \
                     {is_truncated:?}"
                );
                return collected;
            }
        }
    }
    panic!("case {label}: the listing never stopped truncating");
}

/// Each distribution is listed under the mode its configuration describes, and the two
/// listings partition what `ListDistributions` reports.
#[tokio::test]
async fn each_distribution_is_listed_under_the_mode_its_configuration_describes() {
    let router = emulator();
    let created = create_rows(&router).await;

    for mode in CONNECTION_MODES {
        let mut expected: Vec<String> = created
            .iter()
            .filter(|(_, row_mode, _)| *row_mode == mode)
            .map(|(_, _, id)| id.clone())
            .collect();
        expected.sort();
        assert!(
            !expected.is_empty(),
            "case {mode}: the table must cover this mode"
        );

        let answer = list_by_mode(&router, mode, "").await;
        assert_eq!(
            answer.status, 200,
            "case {mode}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        let ids = listed_ids(list);
        assert_eq!(
            ids, expected,
            "case {mode}: expected the distributions of this mode in id order, got {ids:?}"
        );

        // The summaries are the stored records, not something this listing invented: each
        // one matches the summary the unfiltered listing reports for the same id.
        let unfiltered = list_distributions(&router, "").await;
        for id in &ids {
            let filtered_summary = summaries(list)
                .into_iter()
                .find(|summary| summary.text_at("Id") == Some(id.as_str()))
                .cloned()
                .unwrap_or_default();
            let plain_summary = summaries(unfiltered.root())
                .into_iter()
                .find(|summary| summary.text_at("Id") == Some(id.as_str()))
                .cloned()
                .unwrap_or_default();
            for member in ["ARN", "Status", "DomainName", "ETag", "Comment"] {
                assert_eq!(
                    filtered_summary.text_at(member),
                    plain_summary.text_at(member),
                    "case {mode} {id}: expected {member} to be the stored record's {:?}, got {:?}",
                    plain_summary.text_at(member),
                    filtered_summary.text_at(member)
                );
            }
        }
    }

    // Partition: the two listings together are exactly `ListDistributions`, with nothing
    // in both.
    let mut union: Vec<String> = Vec::new();
    for mode in CONNECTION_MODES {
        let answer = list_by_mode(&router, mode, "").await;
        union.extend(listed_ids(answer.root()));
    }
    union.sort();
    let mut all_ids = listed_ids(list_distributions(&router, "").await.root());
    all_ids.sort();
    assert_eq!(
        union, all_ids,
        "expected the two connection-mode listings to partition the store, got {union:?} against \
         {all_ids:?}"
    );
}

/// Walking the pages at ANY admissible page size enumerates that mode's distributions
/// exactly once, in id order.
#[tokio::test]
async fn walking_the_pages_enumerates_each_mode_exactly_once() {
    let router = emulator();
    let created = create_rows(&router).await;

    for mode in CONNECTION_MODES {
        let mut expected: Vec<String> = created
            .iter()
            .filter(|(_, row_mode, _)| *row_mode == mode)
            .map(|(_, _, id)| id.clone())
            .collect();
        expected.sort();

        for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_ITEMS), Some(i32::MAX)] {
            let collected = walk(&router, mode, page_size).await;
            assert_eq!(
                collected, expected,
                "case {mode} page size {page_size:?}: expected every distribution of this mode \
                 exactly once in id order, got {collected:?}"
            );
        }
    }
}

/// A mode with nothing under it lists nothing — no `Items` element, `Quantity` zero and
/// no `NextMarker` — rather than falling back to the whole store.
#[tokio::test]
async fn a_mode_with_no_distributions_lists_nothing() {
    let router = emulator();
    // Only `direct` distributions exist in this emulator.
    for case in ["only-direct-stored", "still-only-direct-stored"] {
        let body = config_xml(&format!("ref-{case}"), "<ConnectionMode>direct</ConnectionMode>");
        let answer = create_distribution(&router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
    }

    let answer = list_by_mode(&router, "tenant-only", "").await;
    assert_eq!(
        answer.status, 200,
        "case empty-mode: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let list = answer.root();
    assert_eq!(
        list.text_at("Quantity"),
        Some("0"),
        "case empty-mode: expected Quantity 0, got {:?}",
        list.text_at("Quantity")
    );
    assert!(
        list.child("Items").is_none(),
        "case empty-mode: expected no Items element at all, got {}",
        answer.raw
    );
    assert_eq!(
        list.text_at("IsTruncated"),
        Some("false"),
        "case empty-mode: expected IsTruncated false, got {:?}",
        list.text_at("IsTruncated")
    );
    assert!(
        list.text_at("NextMarker").is_none(),
        "case empty-mode: expected no NextMarker, got {:?}",
        list.text_at("NextMarker")
    );

    // And the mode that does hold them still reports both.
    let direct = list_by_mode(&router, "direct", "").await;
    assert_eq!(
        listed_ids(direct.root()).len(),
        2,
        "case empty-mode: expected the direct listing to still report both distributions, got {}",
        direct.raw
    );
}

/// A page size that names no page, and a marker this service never issued, are refused as
/// invalid arguments — the same constraint the unfiltered listing states, under either
/// mode.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    create_rows(&router).await;

    let refusals: &[(&str, &str)] = &[
        ("?MaxItems=-7", "a negative page size"),
        ("?MaxItems=not-a-number", "a page size that is no integer"),
        ("?Marker=not-an-id", "a marker that is no distribution id"),
    ];
    for mode in CONNECTION_MODES {
        for (query, description) in refusals {
            let answer = list_by_mode(&router, mode, query).await;
            assert_eq!(
                answer.status, 400,
                "case {mode} with {description}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {mode} with {description}: expected InvalidArgument, got {:?}",
                answer.error_code()
            );
        }
    }
}

/// A `ConnectionMode` outside the model's enum is refused rather than served: the listing
/// never answers with a page for a mode no distribution can have, and the refusal carries
/// the code this service answers a request whose published schema it cannot read — the value
/// here travelled in the URI (a path segment), and a request that carries no document has no
/// document the value could have been read from.
#[tokio::test]
async fn a_mode_outside_the_modelled_enum_is_not_served() {
    let router = emulator();
    create_rows(&router).await;

    for mode in ["Direct", "tenant_only", "multi-tenant"] {
        let answer = list_by_mode(&router, mode, "").await;
        assert_eq!(
            answer.status, 400,
            "case {mode:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "MalformedXML",
            "case {mode:?}: expected MalformedXML, got {:?}",
            answer.error_code()
        );
        // The refusal is built from the request: it names the member that failed and the
        // constraint it failed, which for an enum is the value set the model declares.
        let message = answer.raw.clone();
        for fragment in ["connectionMode", "direct", "tenant-only"] {
            assert!(
                message.contains(fragment),
                "case {mode:?}: expected the refusal to name {fragment:?}, got {message}"
            );
        }
    }
}

/// Listing is a READ: calling it never changes what a later caller sees, under either
/// mode.
#[tokio::test]
async fn listing_changes_nothing() {
    let router = emulator();
    create_rows(&router).await;

    for mode in CONNECTION_MODES {
        let first = list_by_mode(&router, mode, "").await;
        let before = list_distributions(&router, "").await;
        let second = list_by_mode(&router, mode, "").await;
        let after = list_distributions(&router, "").await;
        assert_eq!(
            listed_ids(first.root()),
            listed_ids(second.root()),
            "case {mode}: expected a repeated listing to report the same distributions"
        );
        assert_eq!(
            listed_ids(before.root()),
            listed_ids(after.root()),
            "case {mode}: expected ListDistributions to be unchanged by the filtered listing"
        );
    }
}
