// SPDX-License-Identifier: CC0-1.0

//! `ListCachePolicies`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this listing:
//!
//! * "Gets a list of cache policies": the answer carries the cache policies AWS created,
//!   which every account has under the identifiers AWS publishes, together with the ones
//!   this account created, each summary stating which of the two created it (`Type`) and
//!   reporting the same record a read of that policy by id answers with;
//! * "You can optionally apply a filter to return only the managed policies created by
//!   Amazon Web Services, or only the custom policies created in your Amazon Web Services
//!   account": a request that names a kind is served that kind alone, and one that names
//!   none is served both;
//! * `MaxItems` cuts the page and is echoed as asked for, `Quantity` states the page's
//!   length, `NextMarker` is present exactly when policies remain and names the id the page
//!   ended at, and a request resumes AFTER the policy its `Marker` names — so walking the
//!   pages at ANY admissible page size enumerates every policy exactly once, in one order;
//! * a cache policy id names ONE policy whoever created it, so `GetCachePolicy` and
//!   `GetCachePolicyConfig` answer for a published id exactly as they do for one this
//!   account minted;
//! * a page size that names no page, and a `Marker` that is no cache policy id, are refused
//!   with `InvalidArgument` (400); a well-shaped marker naming no policy at all is
//!   `NoSuchCachePolicy` (404);
//! * a value a QUERY PARAMETER's enum does not admit describes a request this API cannot
//!   read: `MalformedXML` (400), naming the parameter and the values it admits;
//! * a listing is a READ: it answers the same records again and mints nothing.
//!
//! The table rows differ along everything these rules generalise over: the kinds asked for
//! (neither, managed, custom), the number of policies the account itself holds (none, one,
//! several), the page size (one, several, the collection size, past it and past the
//! server's own ceiling), where the resume point sits and which source the policy it names
//! came from, and — for the enum rule — the operation the parameter belongs to, and the form
//! the inadmissible value takes (a value of neither kind, the empty value, an admitted value
//! in the wrong case).
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, and the cache policy request builders are the cache
//! policy lifecycle suite's, reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::CACHE_POLICY_TYPE_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ManagedCachePolicy, CACHE_POLICY_TYPE_CUSTOM, CACHE_POLICY_TYPE_MANAGED,
    INVALID_CACHE_POLICY_MARKER_MESSAGE, MANAGED_CACHE_POLICIES, MAX_ITEMS,
    MAX_ITEMS_INVALID_MESSAGE, NO_SUCH_CACHE_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{create_policy, get_policy, get_policy_config, policy_table};
use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/cache-policy`, with `query` appended verbatim (`?Type=managed&MaxItems=2`),
/// from the operation's `http` binding.
///
/// `pub(crate)` so a sibling suite that reads the collection back goes over the same wire
/// path rather than growing a copy of this.
pub(crate) async fn list_policies(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/cache-policy{query}"),
        None,
        None,
    )
    .await
}

/// A `GET` of `path` under this service's API version, for the listings whose own enum
/// parameter the last rule below is asserted over.
async fn get_path(router: &MultiServiceRouter, path: &str) -> Answer {
    call(router, "GET", &format!("/{API_VERSION}{path}"), None, None).await
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// One summary a page reports: `(Type, Id, Name)` — which of the two created the policy,
/// the id it is addressed by, and the name its configuration carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    kind: String,
    id: String,
    name: String,
}

/// Every summary a page reports, in page order.
fn page_summaries(answer: &Answer) -> Vec<Summary> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("CachePolicySummary")
                .into_iter()
                .map(|summary| Summary {
                    kind: summary.text_at("Type").unwrap_or_default().to_string(),
                    id: summary
                        .text_at("CachePolicy/Id")
                        .unwrap_or_default()
                        .to_string(),
                    name: summary
                        .text_at("CachePolicy/CachePolicyConfig/Name")
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids a page reports, in the order the page carries them.
///
/// `pub(crate)` so a sibling suite that checks WHICH policies a listing still reports uses
/// this scanner rather than growing a copy of it.
pub(crate) fn page_ids(answer: &Answer) -> Vec<String> {
    page_summaries(answer)
        .into_iter()
        .map(|summary| summary.id)
        .collect()
}

// ---------------------------------------------------------------------------
// The policies under test
// ---------------------------------------------------------------------------

/// Cache policies Amazon publishes, spelled out HERE rather than read from the crate, so a
/// catalog that drifts from what AWS publishes fails this suite.
///
/// Source: the CloudFront Developer Guide's list of managed cache policies ("Using the
/// managed cache policies"), which states each policy's name, its identifier and its TTL
/// settings. The rows are `(Id, Name, MinTTL, DefaultTTL, MaxTTL)`, and none of them is the
/// policy any issue cited.
const PUBLISHED_POLICIES: &[(&str, &str, i64, i64, i64)] = &[
    (
        "658327ea-f89d-4fab-a63d-7e88639e58f6",
        "Managed-CachingOptimized",
        1,
        86_400,
        31_536_000,
    ),
    (
        "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
        "Managed-CachingDisabled",
        0,
        0,
        0,
    ),
    (
        "b2884449-e4de-46a7-ac36-70bc7f1ddd6d",
        "Managed-CachingOptimizedForUncompressedObjects",
        1,
        86_400,
        31_536_000,
    ),
];

/// The summary every published policy is expected under: AWS created it, so its `Type` is
/// the managed one. The order is the PUBLISHED catalogue's own, which is the order a
/// listing serves it in.
fn managed_summaries() -> Vec<Summary> {
    MANAGED_CACHE_POLICIES
        .iter()
        .map(|managed: &ManagedCachePolicy| Summary {
            kind: CACHE_POLICY_TYPE_MANAGED.to_string(),
            id: managed.id.to_string(),
            name: managed.name.to_string(),
        })
        .collect()
}

/// Create every configuration the cache policy table describes and report the summary each
/// one is expected under, in id order.
async fn create_custom_policies(router: &MultiServiceRouter) -> Vec<Summary> {
    let mut summaries: Vec<Summary> = Vec::new();
    for policy in policy_table() {
        let created = create_policy(router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 from CreateCachePolicy, got {} with {}",
            policy.case, created.status, created.raw
        );
        let id = created
            .root()
            .text_at("Id")
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the create carries no Id: {}",
                    policy.case, created.raw
                )
            })
            .to_string();
        summaries.push(Summary {
            kind: CACHE_POLICY_TYPE_CUSTOM.to_string(),
            id,
            name: policy.name.clone(),
        });
    }
    summaries.sort_by(|left, right| left.id.cmp(&right.id));
    summaries
}

/// Walk the listing `query` describes at `size` items a page, and report every id the walk
/// reported, in walk order, asserting the page bookkeeping of every page on the way.
async fn walk_pages(router: &MultiServiceRouter, filter: &str, size: usize, total: usize) -> Vec<String> {
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match &marker {
            Some(value) => format!("?MaxItems={size}&Marker={value}{filter}"),
            None => format!("?MaxItems={size}{filter}"),
        };
        let page = list_policies(router, &query).await;
        assert_eq!(
            page.status, 200,
            "case {query}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page.root().text_at("MaxItems"),
            Some(size.to_string().as_str()),
            "case {query}: expected the requested MaxItems echoed, got {}",
            page.raw
        );
        let ids = page_ids(&page);
        assert!(
            ids.len() <= size.min(ceiling),
            "case {query}: a page served {} items, got {}",
            ids.len(),
            page.raw
        );
        assert_eq!(
            page.root().text_at("Quantity"),
            Some(ids.len().to_string().as_str()),
            "case {query}: expected Quantity to state the page's length, got {}",
            page.raw
        );
        if ids.is_empty() {
            assert!(
                page.root().at("Items").is_none(),
                "case {query}: a page that lists nothing carries no Items element, got {}",
                page.raw
            );
        }
        walked.extend(ids.iter().cloned());
        let truncated = walked.len() < total;
        let next = page.root().text_at("NextMarker").map(str::to_string);
        assert_eq!(
            next.is_some(),
            truncated,
            "case {query}: expected NextMarker present {truncated} after {} of {total} \
             items, got {}",
            walked.len(),
            page.raw
        );
        match next {
            Some(value) => {
                assert_eq!(
                    Some(value.as_str()),
                    ids.last().map(String::as_str),
                    "case {query}: expected NextMarker to name the id the page ended at, \
                     got {}",
                    page.raw
                );
                marker = Some(value);
            }
            None => break,
        }
    }
    walked
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An account that has created nothing still lists the cache policies AWS created: every
/// published policy, under the identity and the configuration AWS gives it, each stating
/// that AWS created it.
#[tokio::test]
async fn an_account_that_created_nothing_lists_the_published_policies() {
    let router = emulator();

    // The two kinds this service names are the two the model's enum admits, so a filter
    // built from either is a value a client may send.
    for kind in [CACHE_POLICY_TYPE_MANAGED, CACHE_POLICY_TYPE_CUSTOM] {
        assert!(
            CACHE_POLICY_TYPE_VALUES.contains(&kind),
            "case {kind}: expected a value of the model's CachePolicyType, got {CACHE_POLICY_TYPE_VALUES:?}"
        );
    }

    let listed = list_policies(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case fresh account: expected HTTP 200 from ListCachePolicies, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        page_summaries(&listed),
        managed_summaries(),
        "case fresh account: expected every published policy once, in the published order, \
         got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case fresh account: expected the default MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(managed_summaries().len().to_string().as_str()),
        "case fresh account: expected Quantity to state the page's length, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case fresh account: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );

    // Each policy Amazon publishes is there under the identity and the TTLs Amazon
    // publishes for it.
    let summaries = page_summaries(&listed);
    for (id, name, min_ttl, default_ttl, max_ttl) in PUBLISHED_POLICIES {
        let listed_summary = summaries
            .iter()
            .find(|summary| summary.id == *id)
            .unwrap_or_else(|| panic!("case {name}: the listing reports no policy {id}: {}", listed.raw));
        assert_eq!(
            listed_summary.name, *name,
            "case {name}: expected the published name for {id}, got {:?}",
            listed_summary.name
        );
        assert_eq!(
            listed_summary.kind, CACHE_POLICY_TYPE_MANAGED,
            "case {name}: expected AWS to be named as the creator, got {:?}",
            listed_summary.kind
        );

        // A published policy is READABLE by its id, and reports the same record there.
        let read = get_policy(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {name}: expected HTTP 200 from GetCachePolicy for {id}, got {} with {}",
            read.status, read.raw
        );
        let config = read
            .root()
            .at("CachePolicyConfig")
            .unwrap_or_else(|| panic!("case {name}: the get carries no configuration: {}", read.raw));
        for (member, expected) in [
            ("MinTTL", min_ttl),
            ("DefaultTTL", default_ttl),
            ("MaxTTL", max_ttl),
        ] {
            assert_eq!(
                config.text_at(member),
                Some(expected.to_string().as_str()),
                "case {name}: expected the published {member} {expected}, got {}",
                read.raw
            );
        }
        assert_eq!(
            config.text_at("Name"),
            Some(*name),
            "case {name}: expected the published name from the get, got {}",
            read.raw
        );

        // The version token a read reports has this service's token shape, is the same from
        // both reads, and is the same again on a later call: nothing changes a published
        // policy, so nothing changes its version.
        let etag = read.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {name}: expected a version token in the ETag header, got {etag:?}"
        );
        let config_read = get_policy_config(&router, id).await;
        assert_eq!(
            config_read.status, 200,
            "case {name}: expected HTTP 200 from GetCachePolicyConfig for {id}, got {} with {}",
            config_read.status, config_read.raw
        );
        assert_eq!(
            config_read.header("etag"),
            etag,
            "case {name}: expected one version token across both reads, got {:?}",
            config_read.header("etag")
        );
        assert_eq!(
            get_policy(&router, id).await.header("etag"),
            etag,
            "case {name}: expected the version token to be the same on a later read"
        );
        assert_eq!(
            get_policy(&router, id).await.root().text_at("Id"),
            Some(*id),
            "case {name}: expected a later read to answer from the same record"
        );
    }
}

/// The listing reports every policy of the kind asked for — the published ones, the
/// account's own, or both — and walking the pages at any admissible page size enumerates
/// every one of them exactly once, in one order. A listing is a read: it answers the same
/// records again.
#[tokio::test]
async fn a_listing_reports_every_policy_of_the_kind_asked_for_and_pages_through_them() {
    let router = emulator();
    let custom = create_custom_policies(&router).await;
    let managed = managed_summaries();
    // One total order over both sources: the published catalogue in its own order, then the
    // policies this account created, by id.
    let both: Vec<Summary> = managed.iter().chain(custom.iter()).cloned().collect();

    // (the case, the filter as the request carries it, the summaries it must report)
    let rows: [(&str, &str, &Vec<Summary>); 3] = [
        ("no filter at all", "", &both),
        (
            "only the policies AWS created",
            "&Type=managed",
            &managed,
        ),
        (
            "only the policies this account created",
            "&Type=custom",
            &custom,
        ),
    ];
    for (case, filter, expected) in rows {
        let query = if filter.is_empty() {
            String::new()
        } else {
            format!("?{}", filter.trim_start_matches('&'))
        };
        let listed = list_policies(&router, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            listed.status, listed.raw
        );
        assert_eq!(
            &page_summaries(&listed),
            expected,
            "case {case}: expected every policy of that kind once, in the listing order, \
             got {}",
            listed.raw
        );

        // Walking the pages at ANY admissible page size enumerates them in that same
        // order: sizes below, at and past the collection size, and one past the server's
        // own ceiling.
        let order: Vec<String> = expected.iter().map(|summary| summary.id.clone()).collect();
        let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
        for size in [
            1usize,
            2,
            3,
            order.len().saturating_sub(1).max(1),
            order.len(),
            order.len() + 1,
            ceiling + 1,
        ] {
            let walked = walk_pages(&router, filter, size, order.len()).await;
            assert_eq!(
                walked, order,
                "case {case} at page size {size}: walking the pages must enumerate every \
                 policy exactly once, in order"
            );
        }

        // A listing is a READ: the same request answers the same records again.
        let again = list_policies(&router, &query).await;
        assert_eq!(
            &page_summaries(&again),
            expected,
            "case {case}: a read must not change what a later caller sees, got {}",
            again.raw
        );
    }

    // Each summary of a policy this account created reports what a read of that policy
    // answers with: the two sides of one record.
    for summary in &custom {
        let read = get_policy(&router, &summary.id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected HTTP 200 from GetCachePolicy, got {} with {}",
            summary.name, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("CachePolicyConfig/Name"),
            Some(summary.name.as_str()),
            "case {}: expected the listed name from the get, got {}",
            summary.name,
            read.raw
        );
    }
}

/// A page size that names no page, and a `Marker` that is no cache policy id, are refused
/// with the modelled `InvalidArgument` (400) and change nothing; a well-shaped marker that
/// names no policy at all is the modelled `NoSuchCachePolicy` (404); a marker naming a
/// policy EITHER source holds is a resume point.
#[tokio::test]
async fn a_page_size_or_marker_that_names_no_resume_point_is_refused() {
    let router = emulator();
    let custom = create_custom_policies(&router).await;
    let before = page_ids(&list_policies(&router, "").await);

    // Every form a page size can be unusable in, and every form a marker can fail the id
    // shape in (no id at all, too short, too long, the wrong case, another namespace's
    // token, characters outside the id alphabet).
    let rows: [(&str, &str, &str); 10] = [
        ("a negative page size", "?MaxItems=-1", MAX_ITEMS_INVALID_MESSAGE),
        (
            "a page size that is no integer",
            "?MaxItems=not-a-number",
            MAX_ITEMS_INVALID_MESSAGE,
        ),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999",
            MAX_ITEMS_INVALID_MESSAGE,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=-1&Marker=658327ea-f89d-4fab-a63d-7e88639e58f6",
            MAX_ITEMS_INVALID_MESSAGE,
        ),
        (
            "a marker that is no id at all",
            "?Marker=nope",
            INVALID_CACHE_POLICY_MARKER_MESSAGE,
        ),
        (
            "a marker too short to be an id",
            "?Marker=658327ea-f89d-4fab-a63d",
            INVALID_CACHE_POLICY_MARKER_MESSAGE,
        ),
        (
            "a marker too long to be an id",
            "?Marker=658327ea-f89d-4fab-a63d-7e88639e58f6-7e88639e58f6",
            INVALID_CACHE_POLICY_MARKER_MESSAGE,
        ),
        (
            "a marker in the wrong case",
            "?Marker=658327EA-F89D-4FAB-A63D-7E88639E58F6",
            INVALID_CACHE_POLICY_MARKER_MESSAGE,
        ),
        (
            "a marker from another resource's namespace",
            "?Marker=E1AAAAAAAAAAAA",
            INVALID_CACHE_POLICY_MARKER_MESSAGE,
        ),
        (
            "a marker carrying characters outside the id alphabet",
            "?Marker=658327ea-f89d-4fab-a63d-7e88639e58%21",
            INVALID_CACHE_POLICY_MARKER_MESSAGE,
        ),
    ];
    for (case, query, expected_message) in rows {
        let answer = list_policies(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {query}, got {}",
            answer.raw
        );
        // What each refusal states is the CONSTRAINT on the parameter — a marker is a cache
        // policy id, a page size is an integer that bounds a page — rather than text quoted
        // out of the request.
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            expected_message,
            "case {case}: expected the constraint stated for {query}, got {}",
            answer.raw
        );
    }

    // A well-shaped id no policy holds is the modelled 404: the listing is being resumed at
    // a policy that does not exist.
    let unheld = "00000000-0000-4000-8000-000000000000";
    let answer = list_policies(&router, &format!("?Marker={unheld}")).await;
    assert_eq!(
        answer.status, 404,
        "case a marker naming no policy: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchCachePolicy",
        "case a marker naming no policy: expected NoSuchCachePolicy, got {}",
        answer.raw
    );
    assert_eq!(
        answer.root().find_text("Message").unwrap_or_default(),
        NO_SUCH_CACHE_POLICY_MESSAGE,
        "case a marker naming no policy: expected the modelled sentence, got {}",
        answer.raw
    );

    // A marker naming a policy EITHER source holds resumes after it, whichever source that
    // is: the page it names holds the policies ordered after it.
    let published = managed_summaries();
    let resume_points = [
        published
            .first()
            .map(|summary| summary.id.clone())
            .expect("the catalog is not empty"),
        custom
            .first()
            .map(|summary| summary.id.clone())
            .expect("the fixture created some policies"),
    ];
    for marker in resume_points {
        let page = list_policies(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            page.status, 200,
            "case resume at {marker}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        // The resume point names where the previous page ended, so the page it names holds
        // the policies that come AFTER it in the listing's one order.
        let position = before
            .iter()
            .position(|id| *id == marker)
            .expect("the listing holds the resume point");
        let expected: Vec<String> = before[position + 1..].to_vec();
        assert_eq!(
            page_ids(&page),
            expected,
            "case resume at {marker}: expected the policies ordered after it, got {}",
            page.raw
        );
    }

    // Every refusal above left the store exactly as it was.
    assert_eq!(
        page_ids(&list_policies(&router, "").await),
        before,
        "case refusals: a refused listing must change nothing"
    );
}

/// A value a query parameter's enum does not admit describes a request this API cannot read
/// — the URI is no document, so there is no document the value could have been read from:
/// `MalformedXML` (400), under the sentence that names the parameter and the values it
/// admits — listed in the order this API publishes that enum's values, which for
/// `CachePolicyType` is the caller-owned kind first.
///
/// The rows span the operations whose listing binds such a parameter (cache policies, origin
/// request policies and response headers policies all filter on a `Type` the model
/// constrains to `custom` and `managed`) and the forms an inadmissible value takes.
#[tokio::test]
async fn a_type_the_enum_does_not_admit_is_refused_as_a_request_that_cannot_be_read() {
    let router = emulator();

    let paths = [
        "/cache-policy",
        "/origin-request-policy",
        "/response-headers-policy",
    ];
    let values = [
        ("a value of neither kind", "not-a-policy-type"),
        ("the empty value", ""),
        ("an admitted value in the wrong case", "MANAGED"),
        ("an admitted value with trailing text", "customary"),
    ];
    for path in paths {
        for (case, value) in values {
            let query = format!("{path}?Type={value}");
            let answer = get_path(&router, &query).await;
            assert_eq!(
                answer.status, 400,
                "case {case} on {path}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "MalformedXML",
                "case {case} on {path}: expected MalformedXML, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert!(
                message.contains("'type'"),
                "case {case} on {path}: expected the refusal to name the parameter, got \
                 {message:?}"
            );
            assert!(
                message.contains(&format!(
                    "[{CACHE_POLICY_TYPE_CUSTOM}, {CACHE_POLICY_TYPE_MANAGED}]"
                )),
                "case {case} on {path}: expected the admitted values listed as the API \
                 publishes them, got {message:?}"
            );
            assert!(
                message.contains(&format!("'{value}'")),
                "case {case} on {path}: expected the refusal to show the value it refused, \
                 got {message:?}"
            );
        }
    }

    // A value a parameter's enum DOES admit is served rather than refused, under every one
    // of them: the refusal is about the value, not about the parameter being there.
    for kind in CACHE_POLICY_TYPE_VALUES {
        let answer = list_policies(&router, &format!("?Type={kind}")).await;
        assert_eq!(
            answer.status, 200,
            "case Type={kind}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
    }
}
