// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByKeyGroup`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under a key group exactly when one of its
//! STORED cache behaviours — the default one or any ordered one — names that group among the
//! `TrustedKeyGroups` it trusts, each matching distribution listed ONCE and in id order; the
//! page contract is the one every CloudFront distribution listing states (`MaxItems` is the
//! page size and is echoed, `Marker` resumes after the id the previous page ended at,
//! `NextMarker` and `IsTruncated` arrive exactly when matching distributions remain,
//! `Quantity` states the length of `Items`); a key group nothing trusts lists an empty
//! `Items` element rather than the whole store; and an id the store holds no key group for is
//! `NoSuchResource` (HTTP 404).
//!
//! The rows below span what that rule generalises over: WHICH behaviour names the group (the
//! default one, one ordered one, a later ordered one among several, both at once), how many
//! groups ONE behaviour names, how many behaviours a distribution carries, how many
//! distributions share a group (so a page can cut inside a matching set), whether the
//! wrapper's `Enabled` flag is set — the model documents it as whether the listed groups
//! "have public keys that CloudFront can use to verify the signatures", so it says nothing
//! about which groups are named — a distribution naming NO key group at all, and a group no
//! distribution names.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds; the key group create and the public keys
//! one names are [`crate::key_group_lifecycle`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ITEMS;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id,
    list_distributions, update_distribution, Answer, Node, API_VERSION,
};
use crate::key_group_lifecycle::{create_group, get_group, mint_public_keys};

/// The sentence a listing scoped to ONE key group answers for a group this account does not
/// hold, as a literal rather than read back from the crate, so the WORDING is what this
/// refusal is held to: the key-group surface's own sentence, not the one an arbitrary ARN is
/// answered with.
const NO_SUCH_KEY_GROUP_MESSAGE: &str = "The specified key group does not exist.";

/// One distribution under test: which of the created key groups its default cache behaviour
/// trusts, which ones each of its ordered cache behaviours trusts, and whether the
/// `TrustedKeyGroups` wrapper it submits is `Enabled`. An index is into the key groups
/// [`create_groups`] creates, in order; an empty slice is a behaviour that submits no
/// `TrustedKeyGroups` wrapper at all.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    default_groups: &'static [usize],
    ordered: &'static [&'static [usize]],
    enabled: bool,
}

impl Row {
    /// Whether this distribution trusts the group at `index` in any of its behaviours — the
    /// rule the listing implements, computed here from the row rather than from the handler.
    fn trusts(&self, index: usize) -> bool {
        self.default_groups.contains(&index)
            || self.ordered.iter().any(|groups| groups.contains(&index))
    }
}

/// How many key groups the suite creates. The last one is named by nothing, so an empty
/// listing is covered, and index 2 is only ever named together with another group.
const GROUP_COUNT: usize = 4;

/// The distributions created for every case below.
const ROWS: &[Row] = &[
    Row {
        case: "the default behaviour trusts group 0, no ordered behaviour at all",
        default_groups: &[0],
        ordered: &[],
        enabled: true,
    },
    Row {
        case: "a second distribution whose default behaviour trusts group 0",
        default_groups: &[0],
        ordered: &[],
        enabled: true,
    },
    Row {
        case: "a third distribution whose default behaviour trusts group 0",
        default_groups: &[0],
        ordered: &[],
        enabled: true,
    },
    Row {
        case: "the default behaviour trusts none and the one ordered behaviour trusts group 1",
        default_groups: &[],
        ordered: &[&[1]],
        enabled: true,
    },
    Row {
        case: "three ordered behaviours of which the last trusts group 1",
        default_groups: &[],
        ordered: &[&[], &[], &[1]],
        enabled: true,
    },
    Row {
        case: "the default behaviour and an ordered behaviour both trust group 2",
        default_groups: &[2],
        ordered: &[&[2]],
        enabled: true,
    },
    Row {
        case: "one behaviour trusts two groups at once (1 and 2)",
        default_groups: &[1, 2],
        ordered: &[&[]],
        enabled: true,
    },
    Row {
        case: "the wrapper names group 0 with Enabled false",
        default_groups: &[0],
        ordered: &[],
        enabled: false,
    },
    Row {
        case: "no behaviour names a key group at all",
        default_groups: &[],
        ordered: &[&[]],
        enabled: true,
    },
];

/// `GET /2020-05-31/distributionsByKeyGroupId/{KeyGroupId}` plus the query the caller pages
/// with — the route the model binds this operation to.
async fn list_by_key_group(router: &MultiServiceRouter, key_group_id: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distributionsByKeyGroupId/{key_group_id}{query}"),
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

/// Create [`GROUP_COUNT`] key groups, each naming one public key `CreatePublicKey` minted,
/// and report the id of each in creation order.
async fn create_groups(router: &MultiServiceRouter) -> Vec<String> {
    let keys = mint_public_keys(router, "by-key-group", GROUP_COUNT).await;
    let mut ids = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let body = format!(
            "<KeyGroupConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>by-key-group-{index}</Name>\
             <Items><PublicKey>{key}</PublicKey></Items>\
             </KeyGroupConfig>"
        );
        let answer = create_group(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case group {index}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer
            .root()
            .find_text("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty(),
            "case group {index}: expected a minted key group id, got {}",
            answer.raw
        );
        ids.push(id);
    }
    ids
}

/// The `TrustedKeyGroups` wrapper a behaviour submits for `groups`, or nothing at all when
/// the row names no group for it. `Quantity` states the length of `Items`, which is what
/// every CloudFront list wrapper requires.
fn trusted_xml(enabled: bool, groups: &[usize], ids: &[String]) -> String {
    if groups.is_empty() {
        return String::new();
    }
    let items: String = groups
        .iter()
        .map(|index| format!("<KeyGroup>{}</KeyGroup>", ids[*index]))
        .collect();
    format!(
        "<TrustedKeyGroups><Enabled>{enabled}</Enabled><Quantity>{}</Quantity>\
         <Items>{items}</Items></TrustedKeyGroups>",
        groups.len()
    )
}

/// The `DistributionConfig` a row submits: one origin, a default cache behaviour and the
/// row's ordered cache behaviours, each trusting the key groups the row names.
fn config_xml(caller_reference: &str, row: &Row, groups: &[String]) -> String {
    let mut behaviors = format!("<Quantity>{}</Quantity>", row.ordered.len());
    if !row.ordered.is_empty() {
        behaviors.push_str("<Items>");
        for (position, trusted) in row.ordered.iter().enumerate() {
            behaviors.push_str(&format!(
                "<CacheBehavior><PathPattern>/path-{position}/*</PathPattern>\
                 <TargetOriginId>primary</TargetOriginId>\
                 <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
                 </CacheBehavior>",
                trusted_xml(row.enabled, trusted, groups)
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
         <Enabled>true</Enabled>\
         </DistributionConfig>",
        trusted_xml(row.enabled, row.default_groups, groups)
    )
}

/// Create every key group and every row, returning the key group ids and the `(row, id)`
/// pairs.
async fn seed(router: &MultiServiceRouter) -> (Vec<String>, Vec<(&'static Row, String)>) {
    let groups = create_groups(router).await;
    let mut created = Vec::new();
    for (position, row) in ROWS.iter().enumerate() {
        let body = config_xml(&format!("ref-{position}"), row, &groups);
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
    (groups, created)
}

/// The ids this suite expects under the key group at `index`, in id order.
fn expected_ids(created: &[(&'static Row, String)], index: usize) -> Vec<String> {
    let mut ids: Vec<String> = created
        .iter()
        .filter(|(row, _)| row.trusts(index))
        .map(|(_, id)| id.clone())
        .collect();
    ids.sort();
    ids
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

/// Walk every page of one key group, returning the ids in the order they were served.
async fn walk(
    router: &MultiServiceRouter,
    key_group_id: &str,
    page_size: Option<i32>,
) -> Vec<String> {
    let label = match page_size {
        None => format!("{key_group_id}/default"),
        Some(size) => format!("{key_group_id}/MaxItems={size}"),
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
        let answer = list_by_key_group(router, key_group_id, &query).await;
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

/// Each key group lists exactly the distributions whose stored behaviours trust it, once each
/// and in id order — and every id it lists is a distribution the store holds whose
/// configuration really names that group.
#[tokio::test]
async fn each_key_group_lists_the_distributions_that_trust_it() {
    let router = emulator();
    let (groups, created) = seed(&router).await;

    let all_ids = listed_ids_of_distributions(&router).await;

    for (index, key_group_id) in groups.iter().enumerate() {
        let expected = expected_ids(&created, index);
        let answer = list_by_key_group(&router, key_group_id, "").await;
        assert_eq!(
            answer.status, 200,
            "case group {index}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let ids = listed_ids(answer.root());
        assert_eq!(
            ids, expected,
            "case group {index}: expected the distributions that trust it, in id order, got \
             {ids:?}"
        );

        for id in &ids {
            assert!(
                all_ids.contains(id),
                "case group {index}: expected {id} to be a distribution ListDistributions \
                 reports, got {all_ids:?}"
            );
            // The listing answers from the stored configuration: read the distribution back
            // and find the group among its behaviours' TrustedKeyGroups entries.
            let fetched = get_distribution(&router, id).await;
            assert_eq!(
                fetched.status, 200,
                "case group {index}: expected HTTP 200 from GetDistribution for {id}, got {} \
                 with {}",
                fetched.status, fetched.raw
            );
            assert!(
                fetched
                    .raw
                    .contains(&format!("<KeyGroup>{key_group_id}</KeyGroup>")),
                "case group {index}: expected the stored configuration of {id} to name the key \
                 group, got {}",
                fetched.raw
            );
        }
    }
}

/// Walking the pages at ANY admissible page size enumerates that key group's distributions
/// exactly once, in id order.
#[tokio::test]
async fn walking_the_pages_enumerates_each_key_group_exactly_once() {
    let router = emulator();
    let (groups, created) = seed(&router).await;

    for (index, key_group_id) in groups.iter().enumerate() {
        let expected = expected_ids(&created, index);
        for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_ITEMS), Some(i32::MAX)] {
            let collected = walk(&router, key_group_id, page_size).await;
            assert_eq!(
                collected, expected,
                "case group {index} page size {page_size:?}: expected every distribution that \
                 trusts it exactly once in id order, got {collected:?}"
            );
        }
    }
}

/// A key group no distribution names lists nothing — an `Items` element carrying no id,
/// `Quantity` zero and no `NextMarker` — rather than falling back to the whole store.
#[tokio::test]
async fn a_key_group_nothing_trusts_lists_nothing() {
    let router = emulator();
    let (groups, created) = seed(&router).await;

    let untrusted = groups
        .iter()
        .enumerate()
        .find(|(index, _)| expected_ids(&created, *index).is_empty())
        .map(|(_, id)| id.clone())
        .expect("the table must cover a key group nothing trusts");

    let answer = list_by_key_group(&router, &untrusted, "").await;
    assert_eq!(
        answer.status, 200,
        "case the untrusted group: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let list = answer.root();
    assert_eq!(
        list.text_at("Quantity"),
        Some("0"),
        "case the untrusted group: expected Quantity 0, got {:?}",
        list.text_at("Quantity")
    );
    assert!(
        listed_ids(list).is_empty(),
        "case the untrusted group: expected no distribution id, got {:?}",
        listed_ids(list)
    );
    // The element is present and empty rather than absent, so every page's ids are read the
    // same way.
    assert!(
        list.child("Items").is_some(),
        "case the untrusted group: expected an empty Items element, got {}",
        answer.raw
    );
    assert_eq!(
        list.text_at("IsTruncated"),
        Some("false"),
        "case the untrusted group: expected IsTruncated false, got {:?}",
        list.text_at("IsTruncated")
    );
    assert!(
        list.text_at("NextMarker").is_none(),
        "case the untrusted group: expected no NextMarker, got {:?}",
        list.text_at("NextMarker")
    );
}

/// An id the store holds no key group for is `NoSuchResource`, whatever shape the id takes —
/// never an empty page, which would claim the group exists. The reader of the group itself
/// refuses the same ids, so the listing and the read agree about which ids name a group.
#[tokio::test]
async fn an_id_no_key_group_holds_is_refused() {
    let router = emulator();
    let (groups, _) = seed(&router).await;
    let known = groups.first().cloned().expect("a created key group");

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
        ("a token that is no UUID", "not-a-key-group-id".to_string()),
        (
            "a key group name rather than an id",
            "by-key-group-0".to_string(),
        ),
    ];

    for (case, candidate) in absent {
        let answer = list_by_key_group(&router, &candidate, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResource",
            "case {case}: expected NoSuchResource for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_KEY_GROUP_MESSAGE,
            "case {case}: expected the absent-key-group sentence, got {message:?}"
        );
        let read = get_group(&router, &candidate).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected GetKeyGroup to refuse {candidate:?} too, got {} with {}",
            read.status, read.raw
        );
    }
}

/// A page size that names no page, and a marker this service never issued, are refused as
/// invalid arguments — the same constraint the unfiltered listing states.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let (groups, _) = seed(&router).await;

    let refusals: &[(&str, &str)] = &[
        ("?MaxItems=0", "a page size of zero"),
        ("?MaxItems=-7", "a negative page size"),
        ("?MaxItems=not-a-number", "a page size that is no integer"),
        ("?Marker=not-an-id", "a marker that is no distribution id"),
    ];
    for key_group_id in &groups {
        for (query, description) in refusals {
            let answer = list_by_key_group(&router, key_group_id, query).await;
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

/// The listing follows the STORED configuration: a distribution that stops trusting a group
/// leaves that group's listing, and one that starts trusting it joins it.
#[tokio::test]
async fn an_update_moves_a_distribution_between_listings() {
    let router = emulator();
    let groups = create_groups(&router).await;

    let trusts_first = Row {
        case: "the default behaviour trusts group 0",
        default_groups: &[0],
        ordered: &[],
        enabled: true,
    };
    let trusts_second_at_an_ordered_behaviour = Row {
        case: "an ordered behaviour trusts group 1 instead",
        default_groups: &[],
        ordered: &[&[], &[1]],
        enabled: true,
    };

    let created =
        create_distribution(&router, &config_xml("ref-moves", &trusts_first, &groups)).await;
    assert_eq!(
        created.status, 201,
        "case the initial create: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    let listed = list_by_key_group(&router, &groups[0], "").await;
    assert_eq!(
        listed_ids(listed.root()),
        vec![id.clone()],
        "case {}: expected group 0 to list the distribution, got {}",
        trusts_first.case,
        listed.raw
    );
    let listed = list_by_key_group(&router, &groups[1], "").await;
    assert!(
        listed_ids(listed.root()).is_empty(),
        "case {}: expected group 1 to list nothing yet, got {}",
        trusts_first.case,
        listed.raw
    );

    let updated = update_distribution(
        &router,
        &id,
        &config_xml(
            "ref-moves",
            &trusts_second_at_an_ordered_behaviour,
            &groups,
        ),
        Some(&etag),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {}: expected HTTP 200 from UpdateDistribution, got {} with {}",
        trusts_second_at_an_ordered_behaviour.case, updated.status, updated.raw
    );

    let listed = list_by_key_group(&router, &groups[0], "").await;
    assert!(
        listed_ids(listed.root()).is_empty(),
        "case {}: expected group 0 to list nothing once detached, got {}",
        trusts_second_at_an_ordered_behaviour.case,
        listed.raw
    );
    let listed = list_by_key_group(&router, &groups[1], "").await;
    assert_eq!(
        listed_ids(listed.root()),
        vec![id.clone()],
        "case {}: expected group 1 to list the distribution, got {}",
        trusts_second_at_an_ordered_behaviour.case,
        listed.raw
    );
}

/// Listing is a READ: calling it never changes what a later caller sees.
#[tokio::test]
async fn listing_changes_nothing() {
    let router = emulator();
    let (groups, _) = seed(&router).await;

    for (index, key_group_id) in groups.iter().enumerate() {
        let group_before = get_group(&router, key_group_id).await;
        let first = list_by_key_group(&router, key_group_id, "").await;
        let before = listed_ids_of_distributions(&router).await;
        let second = list_by_key_group(&router, key_group_id, "").await;
        let after = listed_ids_of_distributions(&router).await;
        let group_after = get_group(&router, key_group_id).await;
        assert_eq!(
            listed_ids(first.root()),
            listed_ids(second.root()),
            "case group {index}: expected a repeated listing to report the same ids"
        );
        assert_eq!(
            before, after,
            "case group {index}: expected ListDistributions to be unchanged by the filtered \
             listing"
        );
        assert_eq!(
            group_before.raw, group_after.raw,
            "case group {index}: expected the key group record to be unchanged by the listing"
        );
        assert_eq!(
            group_before.header("etag"),
            group_after.header("etag"),
            "case group {index}: expected the key group version token to be unchanged by the \
             listing"
        );
    }
}
