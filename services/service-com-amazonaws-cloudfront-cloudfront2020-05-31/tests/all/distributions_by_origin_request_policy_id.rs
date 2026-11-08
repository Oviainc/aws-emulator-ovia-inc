// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByOriginRequestPolicyId`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under an origin request policy exactly when
//! one of its STORED cache behaviours — the default one or any ordered one — attaches that
//! policy through `OriginRequestPolicyId`, each matching distribution listed ONCE and in id
//! order; the page contract is the one every CloudFront distribution listing states
//! (`MaxItems` is the page size and is echoed, `Marker` resumes after the id the previous
//! page ended at, `NextMarker` and `IsTruncated` arrive exactly when matching distributions
//! remain, `Quantity` states the length of `Items`); a policy with nothing attached lists an
//! empty `Items` element rather than the whole store; and an id the store holds no origin
//! request policy for is `NoSuchOriginRequestPolicy` (HTTP 404).
//!
//! The rows below span what that rule generalises over: WHICH behaviour attaches the policy
//! (the default one, one ordered one, a later ordered one among several, both at once), how
//! many behaviours a distribution carries, how many distributions share a policy (so a page
//! can cut inside a matching set), a distribution attaching NO origin request policy at all,
//! and a policy no distribution attaches. The created policies also differ in their own
//! configuration (which header, cookie and query-string behaviour each forwards), since the
//! listing selects on the ATTACHMENT and not on what the policy forwards.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds, and the policy create is
//! [`crate::origin_request_policy_lifecycle`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ITEMS, NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id, list_distributions,
    update_distribution, Answer, Node, API_VERSION,
};
use crate::origin_request_policy_lifecycle::{create_policy, strings, Policy};

/// One distribution under test: which of the created origin request policies its default
/// cache behaviour attaches, and which one each of its ordered cache behaviours attaches. An
/// index is into the policies [`create_policies`] creates, in order; `None` is a behaviour
/// that attaches no origin request policy at all.
///
/// `pub(crate)` with [`config_xml`] so a sibling suite that needs a distribution attaching a
/// policy builds it the same way this one does.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Row {
    pub(crate) case: &'static str,
    pub(crate) default_policy: Option<usize>,
    pub(crate) ordered: &'static [Option<usize>],
}

impl Row {
    /// Whether this distribution attaches the policy at `index` to any of its behaviours —
    /// the rule the listing implements, computed here from the row rather than from the
    /// handler.
    pub(crate) fn attaches(&self, index: usize) -> bool {
        self.default_policy == Some(index) || self.ordered.contains(&Some(index))
    }
}

/// The distributions created for every case below.
const ROWS: &[Row] = &[
    Row {
        case: "the default behaviour attaches policy 0, no ordered behaviour at all",
        default_policy: Some(0),
        ordered: &[],
    },
    Row {
        case: "a second distribution whose default behaviour attaches policy 0",
        default_policy: Some(0),
        ordered: &[],
    },
    Row {
        case: "a third distribution whose default behaviour attaches policy 0",
        default_policy: Some(0),
        ordered: &[],
    },
    Row {
        case: "the default behaviour attaches none and the one ordered behaviour attaches policy 1",
        default_policy: None,
        ordered: &[Some(1)],
    },
    Row {
        case: "three ordered behaviours of which the last attaches policy 1",
        default_policy: None,
        ordered: &[None, None, Some(1)],
    },
    Row {
        case: "the default behaviour and an ordered behaviour both attach policy 2",
        default_policy: Some(2),
        ordered: &[Some(2)],
    },
    Row {
        case: "the default behaviour attaches policy 1 and an ordered behaviour attaches policy 0",
        default_policy: Some(1),
        ordered: &[Some(0), None],
    },
    Row {
        case: "no behaviour attaches an origin request policy at all",
        default_policy: None,
        ordered: &[None],
    },
];

/// The policies the suite creates, in creation order. The last one is attached by nothing,
/// so an empty listing is covered, and each carries a DIFFERENT configuration so that the
/// listing is shown to select on the attachment rather than on what the policy forwards.
fn policy_table() -> Vec<Policy> {
    vec![
        Policy {
            case: "a policy that forwards nothing beyond what CloudFront always sends",
            name: "by-origin-request-policy-0".to_string(),
            ..Policy::default()
        },
        Policy {
            case: "a policy that whitelists one header and one cookie",
            name: "by-origin-request-policy-1".to_string(),
            comment: Some("forwards the authorization header".to_string()),
            header_behavior: "whitelist".to_string(),
            headers: Some(strings(&["authorization"])),
            cookie_behavior: "whitelist".to_string(),
            cookies: Some(strings(&["session"])),
            ..Policy::default()
        },
        Policy {
            case: "a policy that forwards every viewer header and every query string",
            name: "by-origin-request-policy-2".to_string(),
            header_behavior: "allViewer".to_string(),
            query_string_behavior: "all".to_string(),
            ..Policy::default()
        },
        Policy {
            case: "a policy nothing attaches, forwarding every cookie",
            name: "by-origin-request-policy-3".to_string(),
            comment: Some("attached by no distribution".to_string()),
            cookie_behavior: "all".to_string(),
            ..Policy::default()
        },
    ]
}

/// `GET /2020-05-31/distributionsByOriginRequestPolicyId/{OriginRequestPolicyId}` plus the
/// query the caller pages with — the route the model binds this operation to.
pub(crate) async fn list_by_policy(
    router: &MultiServiceRouter,
    policy_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distributionsByOriginRequestPolicyId/{policy_id}{query}"),
        None,
        None,
    )
    .await
}

/// The distribution ids of a `DistributionIdList` answer, in document order. Each item
/// travels in the `DistributionId` element the model's `xmlName` gives
/// `DistributionIdListSummary`'s member.
pub(crate) fn listed_ids(list: &Node) -> Vec<String> {
    list.child("Items")
        .map(|items| {
            items
                .children_named("DistributionId")
                .into_iter()
                .map(|item| item.text().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create every policy of [`policy_table`], returning the id of each in creation order.
async fn create_policies(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for policy in policy_table() {
        let answer = create_policy(router, &policy.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateOriginRequestPolicy, got {} with {}",
            policy.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            !id.is_empty(),
            "case {}: expected a minted policy id, got {}",
            policy.case,
            answer.raw
        );
        ids.push(id);
    }
    ids
}

/// The `DistributionConfig` a row submits: one origin, a default cache behaviour and the
/// row's ordered cache behaviours, each attaching the policy the row names.
pub(crate) fn config_xml(caller_reference: &str, row: &Row, policies: &[String]) -> String {
    let attachment = |index: Option<usize>| match index {
        Some(index) => format!(
            "<OriginRequestPolicyId>{}</OriginRequestPolicyId>",
            policies[index]
        ),
        None => String::new(),
    };
    let mut behaviors = format!("<Quantity>{}</Quantity>", row.ordered.len());
    if !row.ordered.is_empty() {
        behaviors.push_str("<Items>");
        for (position, index) in row.ordered.iter().enumerate() {
            behaviors.push_str(&format!(
                "<CacheBehavior><PathPattern>/path-{position}/*</PathPattern>\
                 <TargetOriginId>primary</TargetOriginId>\
                 <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
                 </CacheBehavior>",
                attachment(*index)
            ));
        }
        behaviors.push_str("</Items>");
    }
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
         </DefaultCacheBehavior>\
         <CacheBehaviors>{behaviors}</CacheBehaviors>\
         <Comment>{caller_reference}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled>\
         </DistributionConfig>",
        attachment(row.default_policy)
    )
}

/// Create every policy and every row, returning the policy ids and the `(row, id)` pairs.
async fn seed(router: &MultiServiceRouter) -> (Vec<String>, Vec<(&'static Row, String)>) {
    let policies = create_policies(router).await;
    let mut created = Vec::new();
    for (position, row) in ROWS.iter().enumerate() {
        let body = config_xml(&format!("ref-{position}"), row, &policies);
        let answer = create_distribution(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&id),
            "case {}: expected a minted distribution id, got {id:?}",
            row.case
        );
        created.push((row, id));
    }
    (policies, created)
}

/// The ids this suite expects under the policy at `index`, in id order.
fn expected_ids(created: &[(&'static Row, String)], index: usize) -> Vec<String> {
    let mut ids: Vec<String> = created
        .iter()
        .filter(|(row, _)| row.attaches(index))
        .map(|(_, id)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Walk every page of one policy, returning the ids in the order they were served.
async fn walk(router: &MultiServiceRouter, policy_id: &str, page_size: Option<i32>) -> Vec<String> {
    let label = match page_size {
        None => format!("{policy_id}/default"),
        Some(size) => format!("{policy_id}/MaxItems={size}"),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    // At most one page per distribution plus one, so a listing that never stops truncating
    // fails here rather than looping forever.
    for page in 0..=ROWS.len() + 1 {
        let query = match (page_size, marker.as_deref()) {
            (None, None) => String::new(),
            (None, Some(value)) => format!("?Marker={value}"),
            (Some(size), None) => format!("?MaxItems={size}"),
            (Some(size), Some(value)) => format!("?MaxItems={size}&Marker={value}"),
        };
        let answer = list_by_policy(router, policy_id, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {label} page {page}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        // "Marker: the value provided in the Marker request field."
        assert_eq!(
            list.text_at("Marker"),
            Some(marker.as_deref().unwrap_or_default()),
            "case {label} page {page}: expected the Marker the request carried, got {:?}",
            list.text_at("Marker")
        );
        // "MaxItems: the maximum number of distribution IDs requested."
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
            "case {label} page {page}: expected Quantity to state the ids served ({}), got {:?}",
            ids.len(),
            list.text_at("Quantity")
        );
        if let Some(size) = page_size {
            assert!(
                i32::try_from(ids.len()).unwrap_or(i32::MAX) <= size.min(MAX_ITEMS),
                "case {label} page {page}: expected at most {} ids, got {}",
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

/// Every distribution id in `ListDistributions`, in document order.
async fn listed_ids_of_distributions(router: &MultiServiceRouter) -> Vec<String> {
    let answer = list_distributions(router, "").await;
    assert_eq!(
        answer.status, 200,
        "case the unfiltered listing: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    answer
        .root()
        .child("Items")
        .map(|items| {
            items
                .children_named("DistributionSummary")
                .into_iter()
                .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Each policy lists exactly the distributions whose stored behaviours attach it, once each
/// and in id order — and every id it lists is a distribution the store holds whose
/// configuration really names that policy.
#[tokio::test]
async fn each_policy_lists_the_distributions_that_attach_it() {
    let router = emulator();
    let (policies, created) = seed(&router).await;

    let all_ids = listed_ids_of_distributions(&router).await;

    for (index, policy_id) in policies.iter().enumerate() {
        let expected = expected_ids(&created, index);
        let answer = list_by_policy(&router, policy_id, "").await;
        assert_eq!(
            answer.status, 200,
            "case policy {index}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let ids = listed_ids(answer.root());
        assert_eq!(
            ids, expected,
            "case policy {index}: expected the distributions that attach it, in id order, got \
             {ids:?}"
        );

        for id in &ids {
            assert!(
                all_ids.contains(id),
                "case policy {index}: expected {id} to be a distribution ListDistributions \
                 reports, got {all_ids:?}"
            );
            // The listing answers from the stored configuration: read the distribution back
            // and find the policy among its behaviours' OriginRequestPolicyId elements.
            let fetched = get_distribution(&router, id).await;
            assert_eq!(
                fetched.status, 200,
                "case policy {index}: expected HTTP 200 from GetDistribution for {id}, got {} \
                 with {}",
                fetched.status, fetched.raw
            );
            assert!(
                fetched.raw.contains(&format!(
                    "<OriginRequestPolicyId>{policy_id}</OriginRequestPolicyId>"
                )),
                "case policy {index}: expected the stored configuration of {id} to attach the \
                 policy, got {}",
                fetched.raw
            );
        }
    }
}

/// Walking the pages at ANY admissible page size enumerates that policy's distributions
/// exactly once, in id order.
#[tokio::test]
async fn walking_the_pages_enumerates_each_policy_exactly_once() {
    let router = emulator();
    let (policies, created) = seed(&router).await;

    for (index, policy_id) in policies.iter().enumerate() {
        let expected = expected_ids(&created, index);
        for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_ITEMS), Some(i32::MAX)] {
            let collected = walk(&router, policy_id, page_size).await;
            assert_eq!(
                collected, expected,
                "case policy {index} page size {page_size:?}: expected every distribution that \
                 attaches it exactly once in id order, got {collected:?}"
            );
        }
    }
}

/// A policy no distribution attaches lists nothing — an `Items` element carrying no id,
/// `Quantity` zero and no `NextMarker` — rather than falling back to the whole store.
#[tokio::test]
async fn a_policy_nothing_attaches_lists_nothing() {
    let router = emulator();
    let (policies, created) = seed(&router).await;

    let unattached = policies
        .iter()
        .enumerate()
        .find(|(index, _)| expected_ids(&created, *index).is_empty())
        .map(|(_, id)| id.clone())
        .expect("the table must cover a policy nothing attaches");

    let answer = list_by_policy(&router, &unattached, "").await;
    assert_eq!(
        answer.status, 200,
        "case the unattached policy: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let list = answer.root();
    assert_eq!(
        list.text_at("Quantity"),
        Some("0"),
        "case the unattached policy: expected Quantity 0, got {:?}",
        list.text_at("Quantity")
    );
    assert!(
        listed_ids(list).is_empty(),
        "case the unattached policy: expected no distribution id, got {:?}",
        listed_ids(list)
    );
    // The element is present and empty rather than absent, so every page's ids are read the
    // same way.
    assert!(
        list.child("Items").is_some(),
        "case the unattached policy: expected an empty Items element, got {}",
        answer.raw
    );
    assert_eq!(
        list.text_at("IsTruncated"),
        Some("false"),
        "case the unattached policy: expected IsTruncated false, got {:?}",
        list.text_at("IsTruncated")
    );
    assert!(
        list.text_at("NextMarker").is_none(),
        "case the unattached policy: expected no NextMarker, got {:?}",
        list.text_at("NextMarker")
    );
}

/// An id the store holds no origin request policy for is `NoSuchOriginRequestPolicy`,
/// whatever shape the id takes — never an empty page, which would claim the policy exists.
#[tokio::test]
async fn an_id_no_policy_holds_is_refused() {
    let router = emulator();
    let (policies, _) = seed(&router).await;
    let known = policies.first().cloned().expect("a created policy");

    let mut flipped: Vec<char> = known.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 5] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a known id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("a known id in upper case", known.to_uppercase()),
        ("a token that is no UUID", "not-a-policy-id".to_string()),
        (
            "a policy name rather than an id",
            "by-origin-request-policy-0".to_string(),
        ),
    ];

    for (case, candidate) in absent {
        let answer = list_by_policy(&router, &candidate, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchOriginRequestPolicy",
            "case {case}: expected NoSuchOriginRequestPolicy for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
            "case {case}: expected the modelled message, got {message:?}"
        );
    }
}

/// A page size that names no page, and a marker this service never issued, are refused as
/// invalid arguments — the same constraint the unfiltered listing states.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let (policies, _) = seed(&router).await;

    let refusals: &[(&str, &str)] = &[
        ("?MaxItems=-7", "a negative page size"),
        ("?MaxItems=not-a-number", "a page size that is no integer"),
        ("?Marker=not-an-id", "a marker that is no distribution id"),
    ];
    for policy_id in &policies {
        for (query, description) in refusals {
            let answer = list_by_policy(&router, policy_id, query).await;
            assert_eq!(
                answer.status, 400,
                "case {description}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {description}: expected InvalidArgument, got {:?}",
                answer.error_code()
            );
        }
    }
}

/// The listing answers from the STORED configuration at the instant it is called, not from
/// the configuration a distribution was created with: attaching the policy to a distribution
/// that carried none adds it to the page, detaching it takes it away again, and listing
/// twice in a row answers the same both times (a read changes nothing).
#[tokio::test]
async fn the_listing_follows_the_stored_configuration() {
    let router = emulator();
    let policies = create_policies(&router).await;
    let target = policies.first().cloned().expect("a created policy");

    let detached = Row {
        case: "no behaviour attaches the policy",
        default_policy: None,
        ordered: &[None, None],
    };
    let attached_default = Row {
        case: "the default behaviour attaches the policy",
        default_policy: Some(0),
        ordered: &[None, None],
    };
    let attached_ordered = Row {
        case: "a later ordered behaviour attaches the policy",
        default_policy: None,
        ordered: &[None, Some(0)],
    };

    let created = create_distribution(&router, &config_xml("ref-stored", &detached, &policies)).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
        detached.case, created.status, created.raw
    );
    let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let mut etag = created.header("etag").to_string();

    // A read is a read: the same request twice answers the same page.
    for attempt in 0..2 {
        let answer = list_by_policy(&router, &target, "").await;
        assert!(
            listed_ids(answer.root()).is_empty(),
            "case {} attempt {attempt}: expected the policy to list nothing, got {}",
            detached.case,
            answer.raw
        );
    }

    for row in [&attached_default, &attached_ordered, &detached] {
        let updated = update_distribution(
            &router,
            &distribution_id,
            &config_xml("ref-stored", row, &policies),
            Some(&etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            row.case, updated.status, updated.raw
        );
        etag = updated.header("etag").to_string();

        let expected: Vec<String> = if row.attaches(0) {
            vec![distribution_id.clone()]
        } else {
            Vec::new()
        };
        let answer = list_by_policy(&router, &target, "").await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected HTTP 200, got {} with {}",
            row.case, answer.status, answer.raw
        );
        assert_eq!(
            listed_ids(answer.root()),
            expected,
            "case {}: expected the page to follow the stored configuration, got {}",
            row.case,
            answer.raw
        );
    }
}
