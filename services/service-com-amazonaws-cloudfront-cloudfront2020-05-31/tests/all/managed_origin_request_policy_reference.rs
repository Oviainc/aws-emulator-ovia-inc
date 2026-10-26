// SPDX-License-Identifier: CC0-1.0

//! Resolving an AWS-published origin request policy BY NAME out of the filtered listing and
//! REFERENCING the id it reports, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a policy family Amazon Web Services publishes is reference data
//! every account holds, so the listing filtered to the Amazon Web Services kind serves a
//! page that STATES its length and carries an `Items` element for it — never an empty page —
//! and the id a client resolves out of that page by the policy's NAME is a reference to that
//! policy wherever this service takes one: the single-policy read answers it, a cache
//! behaviour of a distribution may attach it, the stored configuration reports it back, and
//! the by-policy distribution listing finds the distribution under it, exactly as for an id
//! the account minted itself.
//!
//! The rows span what that rule generalises over: EVERY entry of the published catalogue
//! (including entries no issue cited), resolved at page sizes of one, two, the catalogue's
//! own length and the server's ceiling so the resolution has to follow `NextMarker` through
//! several pages; and, for the reference, WHICH behaviour attaches the policy (the default
//! one, one ordered one, a later ordered one, two behaviours at once), a published policy
//! beside one the account created, and a behaviour that attaches none at all.
//!
//! [`crate::managed_policy_catalogs`] asserts the catalogue's CONTENT (every published entry
//! under the identifier, name and settings Amazon publishes for it); this suite asserts what
//! a client does with it. The wire harness is the one [`crate::distribution_lifecycle`]
//! builds and the policy create is [`crate::origin_request_policy_lifecycle`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_ORIGIN_REQUEST_POLICIES, MAX_ITEMS, ORIGIN_REQUEST_POLICY_TYPE_CUSTOM,
    ORIGIN_REQUEST_POLICY_TYPE_MANAGED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id, Answer, API_VERSION,
};
use crate::origin_request_policy_lifecycle::{create_policy, get_policy, Policy};

// ---------------------------------------------------------------------------
// The two listings this suite reads
// ---------------------------------------------------------------------------

/// One page of `ListOriginRequestPolicies`: `GET /2020-05-31/origin-request-policy` with the
/// query a client filters and pages with.
async fn list_policies(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-request-policy{query}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/distributionsByOriginRequestPolicyId/{Id}` — the listing that resolves a
/// distribution through the policy one of its behaviours attaches.
async fn list_distributions_by_policy(router: &MultiServiceRouter, policy_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distributionsByOriginRequestPolicyId/{policy_id}"),
        None,
        None,
    )
    .await
}

/// One summary of a page, as a client reads it: the kind the policy is, its id and its name.
type Summary = (String, String, String);

/// Every summary a page reports, in page order.
fn page_summaries(answer: &Answer) -> Vec<Summary> {
    answer
        .root()
        .child("Items")
        .map(|items| {
            items
                .children_named("OriginRequestPolicySummary")
                .into_iter()
                .map(|summary| {
                    (
                        summary.text_at("Type").unwrap_or_default().to_string(),
                        summary
                            .text_at("OriginRequestPolicy/Id")
                            .unwrap_or_default()
                            .to_string(),
                        summary
                            .text_at("OriginRequestPolicy/OriginRequestPolicyConfig/Name")
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Walk every page of the listing `filter` selects at `size` items a page — the catalog walk
/// a client makes to find a policy by name — asserting the page contract on the way and
/// reporting every summary served, in the order they were served.
///
/// `expected_total` is how many summaries the filter selects, which is what says whether a
/// page is allowed to carry a `NextMarker`.
async fn walk(
    router: &MultiServiceRouter,
    filter: &str,
    size: Option<i32>,
    expected_total: usize,
) -> Vec<Summary> {
    let label = format!("Type={filter:?} MaxItems={size:?}");
    let ceiling = usize::try_from(MAX_ITEMS).expect("the page ceiling fits a usize");
    let mut walked: Vec<Summary> = Vec::new();
    let mut marker: Option<String> = None;
    // At most one page per summary plus one, so a listing that never stops truncating fails
    // here rather than looping forever.
    for page in 0..=expected_total + 1 {
        let mut params: Vec<String> = Vec::new();
        if !filter.is_empty() {
            params.push(format!("Type={filter}"));
        }
        if let Some(size) = size {
            params.push(format!("MaxItems={size}"));
        }
        if let Some(marker) = &marker {
            params.push(format!("Marker={marker}"));
        }
        let query = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };

        let answer = list_policies(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {label} page {page}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        // "MaxItems: the maximum number of origin request policies requested."
        assert_eq!(
            answer.root().text_at("MaxItems"),
            Some(size.unwrap_or(MAX_ITEMS).to_string().as_str()),
            "case {label} page {page}: expected MaxItems to echo the page size, got {}",
            answer.raw
        );
        let summaries = page_summaries(&answer);
        // "Quantity: the total number of origin request policies returned in the response."
        assert_eq!(
            answer.root().text_at("Quantity"),
            Some(summaries.len().to_string().as_str()),
            "case {label} page {page}: expected Quantity to state the page's length ({}), got {}",
            summaries.len(),
            answer.raw
        );
        // A page that states a length carries the element that holds it: a client that reads
        // the items of a non-empty page finds them there.
        if !summaries.is_empty() {
            assert!(
                answer.root().child("Items").is_some(),
                "case {label} page {page}: expected an Items element beside Quantity {}, got {}",
                summaries.len(),
                answer.raw
            );
        }
        if let Some(size) = size {
            let allowed = usize::try_from(size).unwrap_or(ceiling).min(ceiling);
            assert!(
                summaries.len() <= allowed,
                "case {label} page {page}: expected at most {allowed} summaries, got {}",
                summaries.len()
            );
        }
        walked.extend(summaries.iter().cloned());

        let next = answer.root().text_at("NextMarker").map(str::to_string);
        let remaining = walked.len() < expected_total;
        assert_eq!(
            next.is_some(),
            remaining,
            "case {label} page {page}: expected NextMarker present {remaining} after {} of \
             {expected_total} summaries, got {}",
            walked.len(),
            answer.raw
        );
        match next {
            Some(value) => {
                assert_eq!(
                    Some(value.as_str()),
                    summaries.last().map(|(_, id, _)| id.as_str()),
                    "case {label} page {page}: expected NextMarker to name the id the page \
                     ended at, got {}",
                    answer.raw
                );
                marker = Some(value);
            }
            None => return walked,
        }
    }
    panic!("case {label}: the listing never stopped truncating");
}

/// The id the published policy called `name` is served under, resolved the way a client
/// resolves it: walk the listing filtered to the Amazon Web Services kind and take the id of
/// the summary carrying that name. `None` when the walk reports no such policy.
fn resolve_by_name(walked: &[Summary], name: &str) -> Option<String> {
    let matching: Vec<&Summary> = walked
        .iter()
        .filter(|(_, _, served)| served == name)
        .collect();
    assert!(
        matching.len() <= 1,
        "case {name}: expected the name to identify at most one policy, got {matching:?}"
    );
    matching.first().map(|(_, id, _)| id.clone())
}

/// The page sizes every resolution below is made at: one and two items a page (so the walk
/// has to follow `NextMarker` through several pages), the catalogue's own length, the
/// server's own ceiling (the size the recorded client asks for) and no page size at all.
fn page_sizes(total: usize) -> Vec<Option<i32>> {
    vec![
        None,
        Some(1),
        Some(2),
        Some(i32::try_from(total).unwrap_or(MAX_ITEMS)),
        Some(MAX_ITEMS),
    ]
}

// ---------------------------------------------------------------------------
// Referencing a resolved id from a distribution
// ---------------------------------------------------------------------------

/// One distribution under test: which policy of the attachable set its default cache
/// behaviour attaches, and which one each of its ordered cache behaviours attaches. An index
/// is into the attachable ids (every published policy in catalogue order, then the one the
/// account created); `None` is a behaviour that attaches no origin request policy at all.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    default_policy: Option<usize>,
    ordered: Vec<Option<usize>>,
}

impl Row {
    /// Every attachable index this distribution attaches, deduplicated.
    fn attached(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .default_policy
            .into_iter()
            .chain(self.ordered.iter().copied().flatten())
            .collect();
        indices.sort_unstable();
        indices.dedup();
        indices
    }
}

/// The distributions under test: one per published policy attaching it to the default cache
/// behaviour, then the rows that vary WHERE the attachment sits and mix a published policy
/// with the account's own.
///
/// `custom` is the index of the policy the account created, which is the last attachable one,
/// so the published entries are the indices below it. The rows that mix two published
/// policies need three of them, which the guard below states rather than indexing past the
/// catalogue if it ever holds fewer.
fn rows(custom: usize) -> Vec<Row> {
    assert!(
        custom >= 3,
        "case the attachment rows: expected at least three published policies to attach, got \
         {custom}"
    );
    let last_published = custom - 1;
    let mut rows: Vec<Row> = MANAGED_ORIGIN_REQUEST_POLICIES
        .iter()
        .enumerate()
        .map(|(index, managed)| Row {
            case: format!(
                "the published policy {} on the default cache behaviour",
                managed.name
            ),
            default_policy: Some(index),
            ordered: Vec::new(),
        })
        .collect();
    rows.push(Row {
        case: "a published policy on the one ordered cache behaviour and none on the default \
               one"
            .to_string(),
        default_policy: None,
        ordered: vec![Some(0)],
    });
    rows.push(Row {
        case: "a published policy on a later ordered cache behaviour among three".to_string(),
        default_policy: None,
        ordered: vec![None, None, Some(last_published)],
    });
    rows.push(Row {
        case: "a published policy on the default behaviour and the account's own on an ordered \
               one"
            .to_string(),
        default_policy: Some(0),
        ordered: vec![Some(custom)],
    });
    rows.push(Row {
        case: "the account's own policy on the default behaviour and a published one on a later \
               ordered behaviour"
            .to_string(),
        default_policy: Some(custom),
        ordered: vec![None, Some(1)],
    });
    rows.push(Row {
        case: "two different published policies on two behaviours".to_string(),
        default_policy: Some(1),
        ordered: vec![Some(2)],
    });
    rows.push(Row {
        case: "no behaviour attaches an origin request policy at all".to_string(),
        default_policy: None,
        ordered: vec![None],
    });
    rows
}

/// The `DistributionConfig` a row submits: one origin, a default cache behaviour and the
/// row's ordered cache behaviours, each attaching the policy the row names.
fn config_xml(caller_reference: &str, row: &Row, attachable: &[String]) -> String {
    let attachment = |index: Option<usize>| match index {
        Some(index) => format!(
            "<OriginRequestPolicyId>{}</OriginRequestPolicyId>",
            attachable[index]
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
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
         </DefaultCacheBehavior>\
         <CacheBehaviors>{behaviors}</CacheBehaviors>\
         <Comment>{caller_reference}</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>",
        attachment(row.default_policy)
    )
}

/// The configuration a `Distribution` payload carries, read as `(default attachment, one per
/// ordered behaviour)`. An attachment a behaviour does not state is `None`.
fn stored_attachments(answer: &Answer) -> (Option<String>, Vec<Option<String>>) {
    let config = answer
        .root()
        .child("DistributionConfig")
        .expect("a Distribution payload carries its DistributionConfig");
    let read = |value: Option<&str>| value.filter(|text| !text.is_empty()).map(str::to_string);
    let default = read(
        config
            .at("DefaultCacheBehavior")
            .and_then(|behavior| behavior.text_at("OriginRequestPolicyId")),
    );
    let ordered = config
        .at("CacheBehaviors/Items")
        .map(|items| {
            items
                .children_named("CacheBehavior")
                .into_iter()
                .map(|behavior| read(behavior.text_at("OriginRequestPolicyId")))
                .collect()
        })
        .unwrap_or_default();
    (default, ordered)
}

/// The distribution ids a `DistributionIdList` answer reports, in document order.
fn listed_distribution_ids(answer: &Answer) -> Vec<String> {
    answer
        .root()
        .child("Items")
        .map(|items| {
            items
                .children_named("DistributionId")
                .into_iter()
                .map(|item| item.text().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The policy this account creates in the rows below, so a published policy is exercised
/// beside one the account owns.
fn account_policy() -> Policy {
    Policy {
        case: "the policy this account created",
        name: "account-origin-request-policy".to_string(),
        comment: Some("created in this account".to_string()),
        header_behavior: "whitelist".to_string(),
        headers: Some(vec!["authorization".to_string()]),
        ..Policy::default()
    }
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// A client that walks the listing filtered to the Amazon Web Services kind finds EVERY
/// published policy by name, at every admissible page size, in an account that created
/// nothing: the first page states a length above zero and carries the `Items` element that
/// holds it, every summary states that Amazon Web Services created it, and the id each name
/// resolves to is the one the catalogue publishes.
#[tokio::test]
async fn every_published_policy_resolves_by_name_at_every_page_size() {
    let router = emulator();
    let total = MANAGED_ORIGIN_REQUEST_POLICIES.len();
    assert!(
        total > 0,
        "expected the published catalogue to hold at least one policy"
    );

    for size in page_sizes(total) {
        let walked = walk(&router, ORIGIN_REQUEST_POLICY_TYPE_MANAGED, size, total).await;
        assert_eq!(
            walked.len(),
            total,
            "case MaxItems={size:?}: expected the walk to serve every published policy exactly \
             once, got {walked:?}"
        );
        for (kind, id, name) in &walked {
            assert_eq!(
                kind, ORIGIN_REQUEST_POLICY_TYPE_MANAGED,
                "case MaxItems={size:?} ({name}): expected the summary to state the Amazon Web \
                 Services kind, got {kind:?}"
            );
            assert!(
                !id.is_empty(),
                "case MaxItems={size:?} ({name}): expected the summary to carry an id"
            );
        }

        for managed in MANAGED_ORIGIN_REQUEST_POLICIES {
            let resolved = resolve_by_name(&walked, managed.name);
            assert_eq!(
                resolved.as_deref(),
                Some(managed.id),
                "case MaxItems={size:?}: expected {} to resolve to the published id, got \
                 {resolved:?}",
                managed.name
            );
        }
    }
}

/// The id resolved out of the listing is a reference to that policy: the single-policy read
/// answers it with the name it was resolved by, and the kind filter keeps it out of the
/// account's own partition.
#[tokio::test]
async fn a_resolved_id_reads_back_as_the_policy_it_names() {
    let router = emulator();
    let total = MANAGED_ORIGIN_REQUEST_POLICIES.len();
    let walked = walk(&router, ORIGIN_REQUEST_POLICY_TYPE_MANAGED, None, total).await;

    for managed in MANAGED_ORIGIN_REQUEST_POLICIES {
        let id = resolve_by_name(&walked, managed.name)
            .unwrap_or_else(|| panic!("case {}: expected the listing to serve it", managed.name));
        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected HTTP 200 from GetOriginRequestPolicy for the resolved id, got {} \
             with {}",
            managed.name, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {}: expected the read to report the id asked for, got {}",
            managed.name,
            read.raw
        );
        assert_eq!(
            read.root()
                .text_at("OriginRequestPolicyConfig/Name"),
            Some(managed.name),
            "case {}: expected the read to report the name it was resolved by, got {}",
            managed.name,
            read.raw
        );
    }

    // The account created nothing, so its own partition is empty while the published one is
    // not: the two are different sources, not two names for the store.
    let own = walk(&router, ORIGIN_REQUEST_POLICY_TYPE_CUSTOM, None, 0).await;
    assert!(
        own.is_empty(),
        "case the account's own partition: expected no policy, got {own:?}"
    );
}

/// A resolved published id is attachable wherever this service takes an origin request
/// policy id: `CreateDistribution` accepts a cache behaviour that names it, the stored
/// configuration reports it back through `GetDistribution`, and the by-policy listing finds
/// the distribution under it — exactly as for an id the account minted.
#[tokio::test]
async fn a_resolved_id_is_attachable_to_a_cache_behaviour() {
    let router = emulator();
    let total = MANAGED_ORIGIN_REQUEST_POLICIES.len();
    let walked = walk(&router, ORIGIN_REQUEST_POLICY_TYPE_MANAGED, None, total).await;

    // The attachable set: every published policy, resolved BY NAME out of the listing, then
    // the one this account created.
    let mut attachable: Vec<String> = Vec::new();
    for managed in MANAGED_ORIGIN_REQUEST_POLICIES {
        attachable.push(
            resolve_by_name(&walked, managed.name)
                .unwrap_or_else(|| panic!("case {}: expected the listing to serve it", managed.name)),
        );
    }
    let policy = account_policy();
    let created_policy = create_policy(&router, &policy.xml()).await;
    assert_eq!(
        created_policy.status, 201,
        "case {}: expected HTTP 201 from CreateOriginRequestPolicy, got {} with {}",
        policy.case, created_policy.status, created_policy.raw
    );
    let custom = attachable.len();
    attachable.push(
        created_policy
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
    );
    assert!(
        !attachable[custom].is_empty(),
        "case {}: expected a minted policy id, got {}",
        policy.case,
        created_policy.raw
    );

    let mut expected_under: Vec<(usize, Vec<String>)> = (0..attachable.len())
        .map(|index| (index, Vec::new()))
        .collect();

    for (position, row) in rows(custom).iter().enumerate() {
        let body = config_xml(&format!("managed-ref-{position}"), row, &attachable);
        let created = create_distribution(&router, &body).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&distribution_id),
            "case {}: expected a minted distribution id, got {distribution_id:?}",
            row.case
        );

        // What the row submitted, as the stored configuration must report it.
        let expected_default = row.default_policy.map(|index| attachable[index].clone());
        let expected_ordered: Vec<Option<String>> = row
            .ordered
            .iter()
            .map(|index| index.map(|index| attachable[index].clone()))
            .collect();

        // The create's own payload and a later read answer from the same stored
        // configuration, so both are checked against what the row submitted.
        let fetched = get_distribution(&router, &distribution_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 from GetDistribution, got {} with {}",
            row.case, fetched.status, fetched.raw
        );
        for (source, answer) in [("the create", &created), ("GetDistribution", &fetched)] {
            let (default, ordered) = stored_attachments(answer);
            assert_eq!(
                default, expected_default,
                "case {} ({source}): expected the default behaviour to attach \
                 {expected_default:?}, got {default:?}",
                row.case
            );
            assert_eq!(
                ordered, expected_ordered,
                "case {} ({source}): expected the ordered behaviours to attach \
                 {expected_ordered:?}, got {ordered:?}",
                row.case
            );
        }

        for index in row.attached() {
            expected_under[index].1.push(distribution_id.clone());
        }
    }

    for (index, mut expected) in expected_under {
        expected.sort();
        let policy_id = &attachable[index];
        let answer = list_distributions_by_policy(&router, policy_id).await;
        assert_eq!(
            answer.status, 200,
            "case policy {policy_id}: expected HTTP 200 from \
             ListDistributionsByOriginRequestPolicyId, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            listed_distribution_ids(&answer),
            expected,
            "case policy {policy_id}: expected the distributions that attach it, in id order, \
             got {}",
            answer.raw
        );
    }
}
