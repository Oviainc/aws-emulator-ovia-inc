// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByCachePolicyId`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under a cache policy exactly when one of
//! its STORED cache behaviours — the default one or any ordered one — attaches that policy
//! through `CachePolicyId`, each matching distribution listed ONCE and in id order; the page
//! contract is the one every CloudFront distribution listing states (`MaxItems` is the page
//! size and is echoed, `Marker` resumes after the id the previous page ended at,
//! `NextMarker` and `IsTruncated` arrive exactly when matching distributions remain,
//! `Quantity` states the length of `Items`); a policy with nothing attached lists an empty
//! `Items` element rather than the whole store;
//! and an id the store holds no cache policy for is `NoSuchCachePolicy` (HTTP 404).
//!
//! The rows below span what that rule generalises over: WHICH behaviour attaches the policy
//! (the default one, one ordered one, a later ordered one among several, both at once),
//! how many behaviours a distribution carries, how many distributions share a policy (so a
//! page can cut inside a matching set), a distribution attaching NO cache policy at all,
//! and a policy no distribution attaches. The wire harness (the router, the request helpers
//! and the element scanner) is the one [`crate::distribution_lifecycle`] already builds, and
//! the cache policy create is [`crate::cache_policy_lifecycle`]'s.
//!
//! The same attachment rule is CloudFront's DELETE gate for a cache policy — "you cannot
//! delete a cache policy if it's attached to a cache behavior" — so the delete is asserted
//! here too, over the same attachments.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CACHE_POLICY_IN_USE_MESSAGE, MAX_ITEMS, NO_SUCH_CACHE_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{create_policy, get_policy, Policy};
use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id,
    list_distributions, update_distribution, Answer, Node, API_VERSION,
};

/// One distribution under test: which of the created cache policies its default cache
/// behaviour attaches, and which one each of its ordered cache behaviours attaches. An
/// index is into the policies [`create_policies`] creates, in order; `None` is a behaviour
/// that attaches no cache policy at all.
///
/// `pub(crate)` with [`config_xml`] so a sibling suite that needs a distribution attaching a
/// given cache policy builds the same document rather than growing a copy of it.
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

/// How many cache policies the suite creates. The last one is attached by nothing, so an
/// empty listing is covered, and index 2 is only ever attached by an ordered behaviour.
const POLICY_COUNT: usize = 4;

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
        case: "no behaviour attaches a cache policy at all",
        default_policy: None,
        ordered: &[None],
    },
];

/// `GET /2020-05-31/distributionsByCachePolicyId/{CachePolicyId}` plus the query the caller
/// pages with — the route the model binds this operation to.
async fn list_by_policy(router: &MultiServiceRouter, policy_id: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distributionsByCachePolicyId/{policy_id}{query}"),
        None,
        None,
    )
    .await
}

/// The distribution ids of a `DistributionIdList` answer, in document order. Each item
/// travels in the `DistributionId` element the model's `xmlName` gives
/// `DistributionIdListSummary`'s member.
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

/// `DELETE /2020-05-31/cache-policy/{Id}` under the version token `if_match`.
///
/// `pub(crate)` so a sibling suite that deletes a cache policy goes over the same wire path
/// rather than growing a copy of this.
pub(crate) async fn delete_policy(
    router: &MultiServiceRouter,
    policy_id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/cache-policy/{policy_id}"),
        None,
        if_match,
    )
    .await
}

/// Create [`POLICY_COUNT`] cache policies, returning the id of each in creation order.
async fn create_policies(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..POLICY_COUNT {
        let policy = Policy {
            case: "a cache policy a distribution can attach",
            name: format!("by-cache-policy-{index}"),
            min_ttl: index as i64,
            ..Policy::default()
        };
        let answer = create_policy(router, &policy.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case policy {index}: expected HTTP 201 from CreateCachePolicy, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty(),
            "case policy {index}: expected a minted policy id, got {}",
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
        Some(index) => format!("<CachePolicyId>{}</CachePolicyId>", policies[index]),
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

/// Each policy lists exactly the distributions whose stored behaviours attach it, once
/// each and in id order — and every id it lists is a distribution the store holds whose
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
            // and find the policy among its behaviours' CachePolicyId elements.
            let fetched = get_distribution(&router, id).await;
            assert_eq!(
                fetched.status, 200,
                "case policy {index}: expected HTTP 200 from GetDistribution for {id}, got {} \
                 with {}",
                fetched.status, fetched.raw
            );
            assert!(
                fetched
                    .raw
                    .contains(&format!("<CachePolicyId>{policy_id}</CachePolicyId>")),
                "case policy {index}: expected the stored configuration of {id} to attach the \
                 policy, got {}",
                fetched.raw
            );
        }
    }
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
    // The element is present and empty rather than absent, so every page's ids are read
    // the same way.
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

/// An id the store holds no cache policy for is `NoSuchCachePolicy`, whatever shape the id
/// takes — never an empty page, which would claim the policy exists.
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
        ("a policy name rather than an id", "by-cache-policy-0".to_string()),
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
            "NoSuchCachePolicy",
            "case {case}: expected NoSuchCachePolicy for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_CACHE_POLICY_MESSAGE,
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

/// The attachment rule is also the DELETE gate: a cache policy a stored cache behaviour
/// attaches cannot be deleted, and once the distribution no longer attaches it the delete
/// lands and the policy is gone from every read.
///
/// The rows cover the attachment the listing itself covers: the default cache behaviour and
/// an ordered one, each with the policy attached and then removed.
#[tokio::test]
async fn an_attached_policy_cannot_be_deleted_and_a_detached_one_can() {
    let attachments: &[(&str, Row, Row)] = &[
        (
            "the default cache behaviour attaches the policy",
            Row {
                case: "attached at the default behaviour",
                default_policy: Some(0),
                ordered: &[],
            },
            Row {
                case: "detached at the default behaviour",
                default_policy: None,
                ordered: &[],
            },
        ),
        (
            "an ordered cache behaviour attaches the policy",
            Row {
                case: "attached at an ordered behaviour",
                default_policy: None,
                ordered: &[None, Some(0)],
            },
            Row {
                case: "detached at every ordered behaviour",
                default_policy: None,
                ordered: &[None, None],
            },
        ),
    ];

    for (case, attached, detached) in attachments {
        let router = emulator();
        let created_policy = create_policy(
            &router,
            &Policy {
                case: "the policy under the delete gate",
                name: "delete-gate".to_string(),
                min_ttl: 1,
                ..Policy::default()
            }
            .xml(),
        )
        .await;
        assert_eq!(
            created_policy.status, 201,
            "case {case}: expected HTTP 201 from CreateCachePolicy, got {} with {}",
            created_policy.status, created_policy.raw
        );
        let policy_id = created_policy
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let policy_etag = created_policy.header("etag").to_string();
        let policies = vec![policy_id.clone()];

        let created = create_distribution(&router, &config_xml("ref-delete-gate", attached, &policies)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
            created.status, created.raw
        );
        let distribution_id = created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let distribution_etag = created.header("etag").to_string();

        // The listing and the delete gate read the SAME attachment: the policy lists this
        // distribution, so the delete is refused.
        let listed = list_by_policy(&router, &policy_id, "").await;
        assert_eq!(
            listed_ids(listed.root()),
            vec![distribution_id.clone()],
            "case {case}: expected the policy to list the distribution that attaches it, got {}",
            listed.raw
        );

        let refused = delete_policy(&router, &policy_id, Some(&policy_etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 while attached, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "CachePolicyInUse",
            "case {case}: expected CachePolicyInUse while attached, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            CACHE_POLICY_IN_USE_MESSAGE,
            "case {case}: expected the modelled in-use sentence, got {}",
            refused.raw
        );
        // The refusal left the policy where it was.
        assert_eq!(
            get_policy(&router, &policy_id).await.status,
            200,
            "case {case}: expected the refused delete to leave the policy readable"
        );

        // A version that is not the policy's, and no version at all, are refused on their
        // own terms rather than as an attachment.
        for (description, if_match, status, code) in [
            (
                "no If-Match at all",
                None,
                400,
                "InvalidIfMatchVersion",
            ),
            (
                "an If-Match that is no version token",
                Some("not-a-version"),
                400,
                "InvalidIfMatchVersion",
            ),
            (
                "a version token that is not the policy's",
                Some(distribution_etag.as_str()),
                412,
                "PreconditionFailed",
            ),
        ] {
            let answer = delete_policy(&router, &policy_id, if_match).await;
            assert_eq!(
                answer.status, status,
                "case {case} with {description}: expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case} with {description}: expected {code}, got {}",
                answer.raw
            );
        }

        // Remove the policy from every cache behaviour, then the delete lands.
        let updated = update_distribution(
            &router,
            &distribution_id,
            &config_xml("ref-delete-gate", detached, &policies),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            updated.status, updated.raw
        );
        let listed = list_by_policy(&router, &policy_id, "").await;
        assert!(
            listed_ids(listed.root()).is_empty(),
            "case {case}: expected the detached policy to list nothing, got {}",
            listed.raw
        );

        let deleted = delete_policy(&router, &policy_id, Some(&policy_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 once detached, got {} with {}",
            deleted.status, deleted.raw
        );

        // Gone means gone: the id names no policy, for the reader and for this listing
        // alike.
        let fetched = get_policy(&router, &policy_id).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected HTTP 404 from GetCachePolicy after the delete, got {} with {}",
            fetched.status, fetched.raw
        );
        let listed = list_by_policy(&router, &policy_id, "").await;
        assert_eq!(
            listed.status, 404,
            "case {case}: expected HTTP 404 from the listing after the delete, got {} with {}",
            listed.status, listed.raw
        );
        assert_eq!(
            listed.error_code(),
            "NoSuchCachePolicy",
            "case {case}: expected NoSuchCachePolicy after the delete, got {}",
            listed.raw
        );
        // The distribution itself is untouched by the policy's delete.
        assert_eq!(
            get_distribution(&router, &distribution_id).await.status,
            200,
            "case {case}: expected the distribution to outlive the policy"
        );
    }
}

/// Listing is a READ: calling it never changes what a later caller sees.
#[tokio::test]
async fn listing_changes_nothing() {
    let router = emulator();
    let (policies, _) = seed(&router).await;

    for (index, policy_id) in policies.iter().enumerate() {
        let first = list_by_policy(&router, policy_id, "").await;
        let before = listed_ids_of_distributions(&router).await;
        let second = list_by_policy(&router, policy_id, "").await;
        let after = listed_ids_of_distributions(&router).await;
        assert_eq!(
            listed_ids(first.root()),
            listed_ids(second.root()),
            "case policy {index}: expected a repeated listing to report the same ids"
        );
        assert_eq!(
            before, after,
            "case policy {index}: expected ListDistributions to be unchanged by the filtered \
             listing"
        );
    }
}
