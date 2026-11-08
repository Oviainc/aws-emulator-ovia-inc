// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByResponseHeadersPolicyId`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under a response headers policy exactly
//! when one of its STORED cache behaviours — the default one or any ordered one — attaches
//! that policy through `ResponseHeadersPolicyId`, each matching distribution listed ONCE and
//! in id order; the page contract is the one every CloudFront distribution listing states
//! (`MaxItems` is the page size and is echoed, `Marker` resumes after the id the previous
//! page ended at, `NextMarker` and `IsTruncated` arrive exactly when matching distributions
//! remain, `Quantity` states the length of `Items`); a policy with nothing attached lists an
//! empty `Items` element rather than the whole store; and an id no policy holds — neither one
//! this account created nor one AWS published — is `NoSuchResponseHeadersPolicy` (HTTP 404).
//!
//! The rows below span what that rule generalises over: WHICH behaviour attaches the policy
//! (the default one, one ordered one, a later ordered one among several, both at once), how
//! many behaviours a distribution carries, how many distributions share a policy (so a page
//! can cut inside a matching set), a distribution attaching NO response headers policy at
//! all, and a policy no distribution attaches. The created policies also differ in their own
//! configuration (which headers each adds or removes, and whether it carries a comment),
//! since the listing selects on the ATTACHMENT and not on what the policy sets, and a policy
//! AWS published is resolved beside the account's own ones.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds, and the policy create is
//! [`crate::response_headers_policy_lifecycle`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_RESPONSE_HEADERS_POLICIES, MAX_ITEMS, NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id, list_distributions,
    update_distribution, Answer, Node, API_VERSION,
};
use crate::response_headers_policy_lifecycle::{create_policy, Policy};

/// One distribution under test: which of the created response headers policies its default
/// cache behaviour attaches, and which one each of its ordered cache behaviours attaches. An
/// index is into the policies [`create_policies`] creates, in order; `None` is a behaviour
/// that attaches no response headers policy at all.
///
/// `pub(crate)` with [`config_xml`] so a sibling suite that needs a distribution attaching a
/// given response headers policy builds the same document rather than growing a copy of it.
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
    fn attaches(&self, index: usize) -> bool {
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
        case: "no behaviour attaches a response headers policy at all",
        default_policy: None,
        ordered: &[None],
    },
];

/// The policies the suite creates, in creation order. The last one is attached by nothing, so
/// an empty listing is covered, and each carries a DIFFERENT configuration so that the
/// listing is shown to select on the attachment rather than on what the policy sets.
fn policy_table() -> Vec<Policy> {
    vec![
        Policy {
            case: "a policy that sets and removes nothing",
            name: "by-response-headers-policy-0".to_string(),
            ..Policy::default()
        },
        Policy {
            case: "a policy that adds one custom header",
            name: "by-response-headers-policy-1".to_string(),
            comment: Some("adds one custom header".to_string()),
            custom_headers: Some(vec![(
                "x-emulator".to_string(),
                "on".to_string(),
                true,
            )]),
            ..Policy::default()
        },
        Policy {
            case: "a policy that removes two headers and adds none",
            name: "by-response-headers-policy-2".to_string(),
            remove_headers: Some(vec!["Server".to_string(), "X-Powered-By".to_string()]),
            ..Policy::default()
        },
        Policy {
            case: "a policy nothing attaches, adding two custom headers",
            name: "by-response-headers-policy-3".to_string(),
            comment: Some("attached by no distribution".to_string()),
            custom_headers: Some(vec![
                ("x-one".to_string(), "1".to_string(), false),
                ("x-two".to_string(), "2".to_string(), true),
            ]),
            ..Policy::default()
        },
    ]
}

/// `GET /2020-05-31/distributionsByResponseHeadersPolicyId/{ResponseHeadersPolicyId}` plus the
/// query the caller pages with — the route the model binds this operation to.
///
/// `pub(crate)` so a sibling suite drives this listing's id resolution through the same route.
pub(crate) async fn list_by_policy(
    router: &MultiServiceRouter,
    policy_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distributionsByResponseHeadersPolicyId/{policy_id}{query}"),
        None,
        None,
    )
    .await
}

/// The distribution ids of a `DistributionIdList` answer, in document order. Each item travels
/// in the `DistributionId` element the model's `xmlName` gives `DistributionIdListSummary`'s
/// member.
fn listed_ids(list: &Node) -> Vec<String> {
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
            "case {}: expected HTTP 201 from CreateResponseHeadersPolicy, got {} with {}",
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
            "<ResponseHeadersPolicyId>{}</ResponseHeadersPolicyId>",
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
            // and find the policy among its behaviours' ResponseHeadersPolicyId elements.
            let fetched = get_distribution(&router, id).await;
            assert_eq!(
                fetched.status, 200,
                "case policy {index}: expected HTTP 200 from GetDistribution for {id}, got {} \
                 with {}",
                fetched.status, fetched.raw
            );
            assert!(
                fetched.raw.contains(&format!(
                    "<ResponseHeadersPolicyId>{policy_id}</ResponseHeadersPolicyId>"
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

/// A response headers policy AWS published is a policy a behaviour may attach, so the listing
/// resolves its id too: every published policy answers, with the distributions attaching it
/// and nothing else.
///
/// The ids come from the crate's own model-sourced catalog
/// (`state::MANAGED_RESPONSE_HEADERS_POLICIES`, the ids AWS publishes in the CloudFront
/// Developer Guide) rather than being written out here, so the catalog cannot drift. The
/// attached one is the LAST entry, which this suite's other cases never name.
#[tokio::test]
async fn a_policy_aws_published_lists_what_attaches_it() {
    let router = emulator();
    let published: Vec<String> = MANAGED_RESPONSE_HEADERS_POLICIES
        .iter()
        .map(|managed| managed.id.to_string())
        .collect();
    assert!(
        published.len() > 1,
        "case the published catalog: expected more than one published policy, got {published:?}"
    );
    let attached_index = published.len() - 1;

    let row = Row {
        case: "an ordered behaviour attaches a policy AWS published",
        default_policy: None,
        ordered: &[None, Some(0)],
    };
    // The row's indices are into the ids handed to `config_xml`, so hand it the published id
    // the case attaches.
    let created = create_distribution(
        &router,
        &config_xml(
            "ref-published",
            &row,
            std::slice::from_ref(&published[attached_index]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
        row.case, created.status, created.raw
    );
    let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();

    for (index, policy_id) in published.iter().enumerate() {
        let expected: Vec<String> = if index == attached_index {
            vec![distribution_id.clone()]
        } else {
            Vec::new()
        };
        let answer = list_by_policy(&router, policy_id, "").await;
        assert_eq!(
            answer.status, 200,
            "case published policy {policy_id}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            listed_ids(answer.root()),
            expected,
            "case published policy {policy_id}: expected the distributions that attach it, got {}",
            answer.raw
        );
    }
}

/// An id no response headers policy holds is `NoSuchResponseHeadersPolicy`, whatever shape the
/// id takes — never an empty page, which would claim the policy exists. This listing resolves
/// the id as a lookup KEY into the distributions it serves rather than addressing a policy by
/// it, so no precheck of the identifier's shape stands in front of the lookup and every such
/// id carries the ONE absent-policy sentence.
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
            "by-response-headers-policy-0".to_string(),
        ),
    ];

    let expected = NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE;
    for (case, candidate) in absent {
        let answer = list_by_policy(&router, &candidate, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResponseHeadersPolicy",
            "case {case}: expected NoSuchResponseHeadersPolicy for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?} for {candidate:?}, got {message:?}"
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

/// The listing answers from the STORED configuration at the instant it is called, not from the
/// configuration a distribution was created with: attaching the policy to a distribution that
/// carried none adds it to the page, detaching it takes it away again, and listing twice in a
/// row answers the same both times (a read changes nothing).
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

    let created =
        create_distribution(&router, &config_xml("ref-stored", &detached, &policies)).await;
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
