// SPDX-License-Identifier: CC0-1.0

//! The one pagination rule every listing of this service that pages over a STORE KEY
//! answers by, driven end-to-end through each listing that answers by it.
//!
//! The rule under test: such a listing reports the entries its map holds ordered after
//! the key its `NextToken` names, at most `MaxResults` of them in the map's own order,
//! and hands back its token for the KEY of the last entry it reported exactly while
//! entries remain — so a client paging with any `MaxResults` sees each entry once, a page
//! followed by a token is a full page, the last page carries no token, the key of the
//! last entry (which no page hands out) is still resumable and answers an empty final
//! page, and a `NextToken` naming a key the map does not hold is refused 400
//! `ValidationException` with the one sentence this service states for that class
//! (`state::FOREIGN_NEXT_TOKEN_MESSAGE`).
//!
//! `ListPolicies` (over the catalog's policy ARNs) and `ListPolicyVersions` (over one
//! policy's version ids) both answer by that rule, so every table below drives both over
//! the same rows: the two listings are not asserted separately, they are asserted to
//! agree — which is what the one cut they share (`state::page_by_key_token` over
//! `state::cut_page`, the cut the record listings are cut by too) has to deliver.
//!
//! The rows span the class the rule generalises over — how many entries the map holds
//! (one, a few, as many as one page holds, one past it, several pages' worth) crossed
//! with the width asked for (each published bound of `MaxResults`, widths that divide the
//! count exactly and ones that do not, and the member left out so its modelled default
//! applies) — rather than one recorded request. The version ids are deliberately NOT
//! contiguous, so a token that named a count rather than the stored key would be caught.
//!
//! Every request goes over the wire the real client uses, through the `MultiServiceRouter`
//! the harness in `approval_team_lifecycle.rs` mounts, with `MaxResults` and `NextToken`
//! bound to the query string the way both operations bind them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::message;
use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness};

/// `MaxResults` is modelled `range(min: 1, max: 20)` with `default: 20` on both catalog
/// listings — the minimum width a request may ask for.
const MAX_RESULTS_MIN: i64 = 1;

/// `MaxResults`' modelled maximum, which is also its modelled `default` and therefore the
/// width a request that leaves the member out is answered with.
const MAX_RESULTS_MAX: i64 = 20;

/// The most entries any row below stores, so a paging walk that does not terminate is
/// caught rather than looping.
const MOST_ENTRIES: usize = 45;

/// A fixed instant the fixtures date their versions from, so nothing below depends on
/// when the test ran.
fn base_instant() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// Which key-token listing a row drives.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// `ListPolicies`, over the policy catalog in ARN order.
    Policies,
    /// `ListPolicyVersions`, over ONE catalog policy's versions in version-id order.
    PolicyVersions,
}

/// Every listing the rule covers. A row is driven through both of them.
const KINDS: [Kind; 2] = [Kind::Policies, Kind::PolicyVersions];

/// One listing, bound to the resource it pages over.
struct Listing {
    kind: Kind,
    /// The policy whose versions are listed; empty for the listing that names no policy.
    policy_arn: String,
}

impl Listing {
    /// The operation this listing is, for an assertion message.
    fn label(&self) -> &'static str {
        match self.kind {
            Kind::Policies => "ListPolicies",
            Kind::PolicyVersions => "ListPolicyVersions",
        }
    }

    /// The output member the page's entries live in.
    fn member(&self) -> &'static str {
        match self.kind {
            Kind::Policies => "Policies",
            Kind::PolicyVersions => "PolicyVersions",
        }
    }

    /// Which listing this is for the token grammar: a token carries the listing it was
    /// issued by — the operation, and the resource it pages over — so only that listing
    /// can resume from it.
    fn token_listing(&self) -> state::Listing<'_> {
        match self.kind {
            Kind::Policies => state::Listing::Policies,
            Kind::PolicyVersions => state::Listing::PolicyVersions {
                policy_arn: &self.policy_arn,
            },
        }
    }

    /// One page, with `MaxResults` and `NextToken` bound the way this operation binds
    /// them.
    async fn page(
        &self,
        harness: &Harness,
        max_results: Option<i64>,
        token: Option<&str>,
    ) -> (u16, Value) {
        let base = match self.kind {
            Kind::Policies => "/policies/?List".to_string(),
            Kind::PolicyVersions => {
                format!("/policies/{}/?List", encode_label(&self.policy_arn))
            }
        };
        let mut uri = base;
        if let Some(width) = max_results {
            uri.push_str(&format!("&MaxResults={width}"));
        }
        if let Some(next) = token {
            // A token carries a policy ARN's `:` and `/`, both legal in a query value,
            // so a token the service issued reaches the handler as it was issued.
            uri.push_str(&format!("&NextToken={next}"));
        }
        harness.call("POST", &uri, None).await
    }

    /// The ordering KEY of each entry one page reports, in the order it reported them:
    /// the policy's ARN for the catalog, the version id for a version history — which is
    /// exactly what each listing's map is keyed by.
    fn reported_keys(&self, page: &Value) -> Vec<String> {
        page.get(self.member())
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| match self.kind {
                Kind::Policies => text(entry, "Arn"),
                // `PolicyVersionSummary$VersionId` is a `PolicyVersionId` (an integer).
                Kind::PolicyVersions => entry
                    .get("VersionId")
                    .and_then(Value::as_i64)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Every key the STORE holds for this listing, in the order the map holds them.
    fn stored_keys(&self, harness: &Harness) -> Vec<String> {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let guard = store.lock();
        match self.kind {
            Kind::Policies => guard.policies.keys().cloned().collect(),
            Kind::PolicyVersions => guard
                .policy(&self.policy_arn)
                .expect("a stored policy")
                .versions
                .keys()
                .map(i32::to_string)
                .collect(),
        }
    }
}

/// Version ids for a history of `count` versions: NON-contiguous and not starting at the
/// lowest id a policy may carry, so a token built from a count rather than from the
/// stored key cannot pass. Every id is at least `state::FIRST_POLICY_VERSION`, from
/// `PolicyVersionId`'s modelled `range(min: 1)`.
fn version_ids(count: usize) -> Vec<i32> {
    (0..i32::try_from(count).expect("a small version count"))
        .map(|n| state::FIRST_POLICY_VERSION + n * 3)
        .collect()
}

/// A catalog policy named for row `n`, holding `versions` versions.
///
/// The `PolicyType` alternates so no table below tests a single value of it, and each
/// version is dated from [`base_instant`] so no reported timestamp depends on when the
/// test ran.
fn catalog_policy(n: usize, versions: usize) -> state::CatalogPolicy {
    let name = format!("PagedPolicy{n:03}");
    let ids = version_ids(versions);
    let policy_type = if n.is_multiple_of(2) {
        state::PolicyType::AwsRam
    } else {
        state::PolicyType::AwsManaged
    };
    let mut policy = state::CatalogPolicy {
        arn: state::policy_arn("IdentityCenter", &name),
        name,
        policy_type,
        default_version: ids.first().copied().unwrap_or(state::FIRST_POLICY_VERSION),
        versions: BTreeMap::new(),
    };
    for &version_id in &ids {
        let created = base_instant() + Duration::from_secs(u64::from(version_id.unsigned_abs()));
        policy.insert_version(state::CatalogPolicyVersion {
            version_id,
            status: state::PolicyStatus::Attachable,
            creation_time: created,
            last_updated_time: created + Duration::from_secs(30),
        });
    }
    policy
}

/// A store whose listing of kind `kind` pages over exactly `count` entries, the listing
/// that pages over them, and the keys in the order the listing has to report them.
///
/// The catalog is AWS's — no modelled operation mints or removes a policy or one of its
/// versions — so a table that needs a catalog of a given shape writes it directly, the
/// way `policy_catalog.rs` grows one and `approval_team_lifecycle.rs` records the
/// attachable entries its tables need.
fn fixture(kind: Kind, count: usize) -> (Harness, Listing, Vec<String>) {
    // Reading a catalog is not an organization-only operation, so no management account
    // is recorded.
    let harness = Harness::without_organization();
    let listing = {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let mut guard = store.lock();
        guard.policies.clear();
        match kind {
            Kind::Policies => {
                for n in 1..=count {
                    guard.insert_policy(catalog_policy(n, 1));
                }
                Listing {
                    kind,
                    policy_arn: String::new(),
                }
            }
            Kind::PolicyVersions => {
                let policy = catalog_policy(1, count);
                let policy_arn = policy.arn.clone();
                guard.insert_policy(policy);
                Listing { kind, policy_arn }
            }
        }
    };
    let stored = listing.stored_keys(&harness);
    (harness, listing, stored)
}

/// How many entries a row stores, and the widths asked for against that many.
fn page_rows() -> Vec<(usize, Vec<Option<i64>>)> {
    vec![
        // One entry, asked for one at a time and with the member left out.
        (1, vec![Some(MAX_RESULTS_MIN), None]),
        // A few entries: the minimum width, widths that divide the count exactly and
        // ones that do not, the maximum, and the member left out.
        (
            5,
            vec![
                Some(MAX_RESULTS_MIN),
                Some(2),
                Some(5),
                Some(MAX_RESULTS_MAX),
                None,
            ],
        ),
        // As many entries as one page holds: the width that reports them all with
        // nothing left over, and one that does not divide the count.
        (MAX_RESULTS_MAX as usize, vec![Some(MAX_RESULTS_MAX), Some(7)]),
        // One past a page, so the widest page a client may ask for still leaves one.
        (
            MAX_RESULTS_MAX as usize + 1,
            vec![Some(MAX_RESULTS_MAX), None],
        ),
        // Several pages' worth, at widths that do and do not divide the count.
        (MOST_ENTRIES, vec![Some(MAX_RESULTS_MIN), Some(5), Some(7)]),
    ]
}

/// Page through `listing` with `max_results`, asserting the rule at every page, and
/// return the keys the pages reported in order.
async fn walk(
    harness: &Harness,
    listing: &Listing,
    max_results: Option<i64>,
    case: &str,
) -> Vec<String> {
    let width = usize::try_from(max_results.unwrap_or(MAX_RESULTS_MAX)).expect("a page width");
    let mut reported: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, page) = listing.page(harness, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page returned {status} with {page:?}"
        );
        let on_this_page = listing.reported_keys(&page);
        assert!(
            on_this_page.len() <= width,
            "case {case:?}: a page of at most {width} was asked for, got {} in {page:?}",
            on_this_page.len()
        );
        reported.extend(on_this_page.iter().cloned());
        pages += 1;
        assert!(
            pages <= MOST_ENTRIES + 1,
            "case {case:?}: the pages never ran out, which is a paging loop"
        );

        match page.get("NextToken").and_then(Value::as_str) {
            None => {
                assert!(
                    page.get("NextToken").is_none_or(Value::is_null),
                    "case {case:?}: the last page must carry no NextToken: {page:?}"
                );
                break;
            }
            Some(next) => {
                // A page followed by a token is FULL: a short page with a token would
                // make a client's page count depend on something other than `MaxResults`.
                assert_eq!(
                    on_this_page.len(),
                    width,
                    "case {case:?}: a page followed by a NextToken must be {width} wide: {page:?}"
                );
                // The token IS this listing's token for the KEY of the last entry the
                // page reported, which is what makes the next page resume after it.
                let last = on_this_page
                    .last()
                    .expect("a full page reports at least one entry");
                assert_eq!(
                    next,
                    state::listing_token(listing.token_listing(), last),
                    "case {case:?}: the token must name the stored key {last:?}, got {next:?}"
                );
                token = Some(next.to_string());
            }
        }
    }
    reported
}

/// Both catalog listings page by the one key-token rule: each stored entry exactly once,
/// in the map's own order, in pages of at most `MaxResults`, with the token present
/// exactly while entries remain.
#[tokio::test]
async fn both_key_token_listings_page_by_the_one_cut() {
    for kind in KINDS {
        for (total, widths) in page_rows() {
            let (harness, listing, stored) = fixture(kind, total);
            assert_eq!(
                stored.len(),
                total,
                "case {}: the fixture holds {} entries, not {total}",
                listing.label(),
                stored.len()
            );
            for max_results in widths {
                let case = format!(
                    "{} over {total} entries with MaxResults={max_results:?}",
                    listing.label()
                );
                let reported = walk(&harness, &listing, max_results, &case).await;
                assert_eq!(
                    reported, stored,
                    "case {case:?}: expected every stored entry once, in the map's order, \
                     got {reported:?}"
                );
                assert_eq!(
                    reported.iter().collect::<BTreeSet<_>>().len(),
                    total,
                    "case {case:?}: an entry was reported twice in {reported:?}"
                );
            }
        }
    }
}

/// The key of the LAST entry is resumable in both catalog listings even though no page
/// hands it out, and answers the empty final page with no token — while a key the map
/// does not hold is refused 400 `ValidationException` with the one sentence this service
/// states for the class.
#[tokio::test]
async fn both_key_token_listings_resume_from_a_held_key_and_refuse_any_other() {
    for kind in KINDS {
        let (harness, listing, stored) = fixture(kind, 4);
        let last = stored.last().expect("a fixture of four entries").clone();

        // The key of the last entry: held, so resumable, and nothing is ordered after it.
        let case = format!("{} resuming after its last entry", listing.label());
        let token = state::listing_token(listing.token_listing(), &last);
        let (status, page) = listing
            .page(&harness, Some(MAX_RESULTS_MIN), Some(&token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the key of an entry the map holds must be resumable, got \
             {status} with {page:?}"
        );
        assert!(
            listing.reported_keys(&page).is_empty(),
            "case {case:?}: nothing is ordered after the last entry, got {page:?}"
        );
        assert!(
            page.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: the empty final page must carry no NextToken: {page:?}"
        );

        // Keys the map does not hold, in this listing's own token form: the grammar alone
        // cannot make a token resumable.
        let absent: Vec<(&str, String)> = vec![
            (
                "a key ordered before every entry",
                state::listing_token(listing.token_listing(), ""),
            ),
            (
                "a key ordered after every entry",
                state::listing_token(listing.token_listing(), &format!("{last}zzz")),
            ),
            (
                "a key of another type entirely",
                state::listing_token(listing.token_listing(), "not-a-key"),
            ),
            ("a token in no grammar of this service", "4".to_string()),
        ];
        for (row, token) in absent {
            let case = format!("{} refusing {row}", listing.label());
            let (status, body) = listing
                .page(&harness, Some(MAX_RESULTS_MIN), Some(&token))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a key the map does not hold must be refused, got {status} \
                 with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: wrong error code in {body:?}"
            );
            assert_eq!(
                message(&body),
                state::FOREIGN_NEXT_TOKEN_MESSAGE,
                "case {case:?}: expected the one pagination-token sentence, got {body:?}"
            );
        }

        // The control: the token this listing DID hand out is answered, and resumes
        // exactly after the entry its page reported.
        let (status, first) = listing.page(&harness, Some(MAX_RESULTS_MIN), None).await;
        assert_eq!(
            status, 200,
            "case {}: the first page returned {status} with {first:?}",
            listing.label()
        );
        let issued = text(&first, "NextToken");
        assert!(
            !issued.is_empty(),
            "case {}: the first of four pages must carry a NextToken: {first:?}",
            listing.label()
        );
        let (status, second) = listing
            .page(&harness, Some(MAX_RESULTS_MIN), Some(&issued))
            .await;
        assert_eq!(
            status, 200,
            "case {}: the second page returned {status} with {second:?}",
            listing.label()
        );
        assert_eq!(
            listing.reported_keys(&second),
            vec![stored[1].clone()],
            "case {}: the issued token must resume after the first page: {second:?}",
            listing.label()
        );
    }
}
