// SPDX-License-Identifier: CC0-1.0

//! The ONE page walk every paginated list of this service serves, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the paging rule all of these lists share, now that
//! they state it in one place:
//!
//! * a page holds at most the page size the call serves, and reports the entries in the
//!   order the collection it walks is kept in;
//! * a `nextToken` arrives EXACTLY when entries remain, so it never arrives with a page
//!   that is shorter than the one the call asked for, and it names the entry the next page
//!   resumes at — following it therefore enumerates the whole collection exactly once, in
//!   one and the same order, at ANY admissible page size;
//! * the token stays within `NextToken`'s published bounds
//!   (`@length(min: 1, max: 1024)`, `pattern ^[\S]+$`);
//! * a `nextToken` a list never issued — including one ANOTHER list of this service
//!   issued, because each list's token names its own collection — is refused with
//!   `ValidationException` (HTTP 400) instead of read as a resume point, and a `maxResults`
//!   outside `@range(min: 1, max: 100)` with HTTP 400;
//! * paging changes nothing: the same walk answers the same pages afterwards.
//!
//! The rows are the LISTS themselves, one from each place this crate pages a collection,
//! and they differ along what the shared walk generalises over: whether the collection is
//! the service's own published catalog or records a caller created, what keys it in order
//! (a published id, an id this service minted, or the position in a sequence nothing
//! names), how it writes its page token (behind a per-list marker, or as the minted id
//! itself), how many entries it holds, and the summary shape it reports. Each list is then
//! walked at every page size that is interesting for its own length: the published floor,
//! sizes that do not divide it, the sizes just under, at and just past its length, the
//! published ceiling, and no `maxResults` at all.
//!
//! An entry is identified by the member that NAMES it rather than by its whole body: what
//! these cases are about is which entry lands on which page, and an experiment's reported
//! status is moved by the clock rather than by paging.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, start_experiment, token, Answer, FALLBACK_REGION,
    WAIT_DURATION,
};

/// The published bounds every `maxResults` of these lists declares
/// (`@range(min: 1, max: 100)`), spelled out here rather than imported from the crate so a
/// wrong bound in the model or in the shared page walk fails this suite.
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 100;

/// `NextToken`'s published bounds (`@length(min: 1, max: 1024)`), spelled out for the same
/// reason.
const NEXT_TOKEN_FLOOR: usize = 1;
/// See [`NEXT_TOKEN_FLOOR`].
const NEXT_TOKEN_CEILING: usize = 1024;

/// A bound on how many pages one walk may take: every page but the last carries at least
/// one entry, so a walk over a collection this suite builds ends far inside it. It keeps a
/// mis-minted token from looping forever instead of failing.
const MAX_PAGES: usize = 256;

/// How many experiment templates this suite creates — more than one page at the small page
/// sizes below, and far under the published ceiling of a page.
const TEMPLATES: usize = 5;

/// How many experiments this suite starts, spread over the templates above.
const EXPERIMENTS: usize = 4;

/// How many resources each target of the resolved-targets fixture names: `ResourceArnList`
/// is modelled `@length(min: 0, max: 5)`, so this is the most one target may name.
const ARNS_PER_TARGET: usize = 5;

/// The account id every ARN this suite builds carries (`::engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// How an entry of a list is IDENTIFIED in an assertion.
#[derive(Clone, Copy, Debug)]
enum Identity {
    /// The member that names the entry (an id, a resource type).
    Member(&'static str),
    /// The whole entry, for a list whose entries carry no name of their own.
    Whole,
}

/// One list under test: where it is served, which member its page arrives in, and how an
/// entry of it is named.
#[derive(Debug)]
struct Listing {
    /// How a failing assertion names this list.
    label: &'static str,
    /// The request URI, without the query string.
    path: String,
    /// The output member a page's entries arrive in.
    member: &'static str,
    /// How an entry of this list is identified.
    identity: Identity,
}

impl Listing {
    /// The identity of one entry, as an assertion names it.
    fn identify(&self, entry: &Value) -> String {
        match self.identity {
            Identity::Member(member) => entry[member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "{}: an entry carries no {member:?}: {entry:?}",
                        self.label
                    )
                })
                .to_string(),
            Identity::Whole => serde_json::to_string(entry).expect("an answered entry serializes"),
        }
    }
}

/// One page of `listing`, at `page_size` and resuming at `next_token` when there is one.
async fn page(
    router: &MultiServiceRouter,
    listing: &Listing,
    page_size: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(page_size) = page_size {
        query.push(format!("maxResults={page_size}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    send(
        router,
        "GET",
        &format!("{}{query}", listing.path),
        None,
        FALLBACK_REGION,
    )
    .await
}

/// The `nextToken` an answer carries, if any.
fn next_token(answer: &Answer) -> Option<String> {
    answer.member("nextToken").as_str().map(str::to_string)
}

/// Walk every page of `listing` at `page_size`, returning the identity of each entry in the
/// order the pages reported them.
///
/// Asserts the shared paging rule as it goes: a page is never longer than the size the call
/// asked for, a `nextToken` only ever arrives with a full page, and a token stays within
/// `NextToken`'s published bounds and moves the walk on.
async fn walk(
    router: &MultiServiceRouter,
    listing: &Listing,
    page_size: Option<i32>,
) -> Vec<String> {
    let case = format!("{} maxResults={page_size:?}", listing.label);
    let allowed = usize::try_from(page_size.unwrap_or(MAX_RESULTS_CEILING))
        .expect("a published page size is positive");
    let mut collected: Vec<String> = Vec::new();
    let mut resume: Option<String> = None;
    for index in 0..MAX_PAGES {
        let answer = page(router, listing, page_size, resume.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} page {index}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let entries = answer
            .member(listing.member)
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?} page {index}: no {:?} list in {:?}",
                    listing.member, answer.body
                )
            })
            .clone();
        assert!(
            entries.len() <= allowed,
            "case {case:?} page {index}: a page holds at most {allowed} entries, got {}",
            entries.len()
        );
        collected.extend(entries.iter().map(|entry| listing.identify(entry)));
        match next_token(&answer) {
            None => return collected,
            Some(issued) => {
                assert_eq!(
                    entries.len(),
                    allowed,
                    "case {case:?} page {index}: a nextToken must only arrive with a full page, \
                     got {} entries",
                    entries.len()
                );
                let length = issued.chars().count();
                assert!(
                    (NEXT_TOKEN_FLOOR..=NEXT_TOKEN_CEILING).contains(&length)
                        && !issued.chars().any(char::is_whitespace),
                    "case {case:?} page {index}: expected a token of {NEXT_TOKEN_FLOOR}..=\
                     {NEXT_TOKEN_CEILING} non-whitespace characters, got {issued:?}"
                );
                assert_ne!(
                    Some(&issued),
                    resume.as_ref(),
                    "case {case:?} page {index}: the token did not move the walk on"
                );
                resume = Some(issued);
            }
        }
    }
    panic!("case {case:?}: paging did not terminate over {MAX_PAGES} pages");
}

/// The page sizes a collection of `count` entries is walked at: the published floor, sizes
/// that do not divide it, the sizes just under, at and just past its length, the published
/// ceiling, and no `maxResults` at all.
fn page_sizes(count: usize) -> Vec<Option<i32>> {
    let length = i32::try_from(count).expect("a collection this suite builds fits an i32");
    let mut sizes = vec![Some(MAX_RESULTS_FLOOR), Some(2), Some(3), Some(7)];
    sizes.extend(
        [length - 1, length, length + 1]
            .into_iter()
            .filter(|size| (MAX_RESULTS_FLOOR..=MAX_RESULTS_CEILING).contains(size))
            .map(Some),
    );
    sizes.push(Some(MAX_RESULTS_CEILING));
    sizes.push(None);
    sizes
}

/// The ARN of one resource an experiment target names, as AWS writes one.
fn instance_arn(index: usize) -> String {
    format!("arn:aws:ec2:{FALLBACK_REGION}:{ACCOUNT_ID}:instance/i-{index:08}")
}

/// The emulator, with the records the caller-owned lists page, and every list under test.
async fn fixture() -> (MultiServiceRouter, Vec<Listing>) {
    let router = emulator();

    // The templates the template list pages, and the templates the experiments are started
    // from.
    let mut templates: Vec<String> = Vec::new();
    for index in 0..TEMPLATES {
        let case = format!("template-{index}");
        let created = create_template(
            &router,
            json!({
                "clientToken": token(&format!("page-walk-template-{index}")),
                "description": format!("the shared page walk: {case}"),
                "roleArn": "arn:aws:iam::000000000000:role/fis-page-walk",
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": {
                    "wait": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": WAIT_DURATION }
                    }
                }
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
            created.status, created.body
        );
        templates.push(
            created.member("experimentTemplate")["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
                .to_string(),
        );
    }

    // The experiments the experiment list pages, spread over the templates above so the
    // collection is not one template's.
    for index in 0..EXPERIMENTS {
        let case = format!("experiment-{index}");
        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("page-walk-experiment-{index}")),
                "experimentTemplateId": templates[index % templates.len()]
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
            started.status, started.body
        );
    }

    // One experiment whose template names resources, so its resolution is a collection the
    // position-keyed walk pages: two targets, each naming the most resources one target
    // may.
    let targeted = create_template(
        &router,
        json!({
            "clientToken": token("page-walk-resolved"),
            "description": "the shared page walk: a resolution to page",
            "roleArn": "arn:aws:iam::000000000000:role/fis-page-walk",
            "stopConditions": [{ "source": "none" }],
            "targets": {
                "firstInstances": {
                    "resourceType": "aws:ec2:instance",
                    "selectionMode": "ALL",
                    "resourceArns": (1..=ARNS_PER_TARGET)
                        .map(instance_arn)
                        .collect::<Vec<String>>()
                },
                "secondInstances": {
                    "resourceType": "aws:ec2:instance",
                    "selectionMode": "ALL",
                    "resourceArns": (11..11 + ARNS_PER_TARGET)
                        .map(instance_arn)
                        .collect::<Vec<String>>()
                }
            },
            "actions": {
                "stopFirst": {
                    "actionId": "aws:ec2:stop-instances",
                    "targets": { "Instances": "firstInstances" }
                },
                "stopSecond": {
                    "actionId": "aws:ec2:stop-instances",
                    "targets": { "Instances": "secondInstances" }
                }
            }
        }),
    )
    .await;
    assert_eq!(
        targeted.status, 200,
        "case resolved-targets: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        targeted.status, targeted.body
    );
    let targeted_id = targeted.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case resolved-targets: no template id in {:?}", targeted.body))
        .to_string();
    let started = start_experiment(
        &router,
        json!({
            "clientToken": token("page-walk-resolved-experiment"),
            "experimentTemplateId": targeted_id
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case resolved-targets: expected HTTP 200 from StartExperiment, got {} with {:?}",
        started.status, started.body
    );
    let experiment_id = started.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case resolved-targets: no experiment id in {:?}", started.body))
        .to_string();

    let listings = vec![
        Listing {
            label: "ListActions",
            path: "/actions".to_string(),
            member: "actions",
            identity: Identity::Member("id"),
        },
        Listing {
            label: "ListTargetResourceTypes",
            path: "/targetResourceTypes".to_string(),
            member: "targetResourceTypes",
            identity: Identity::Member("resourceType"),
        },
        Listing {
            label: "ListExperimentTemplates",
            path: "/experimentTemplates".to_string(),
            member: "experimentTemplates",
            identity: Identity::Member("id"),
        },
        Listing {
            label: "ListExperiments",
            path: "/experiments".to_string(),
            member: "experiments",
            identity: Identity::Member("id"),
        },
        Listing {
            label: "ListExperimentResolvedTargets",
            path: format!("/experiments/{experiment_id}/resolvedTargets"),
            member: "resolvedTargets",
            identity: Identity::Whole,
        },
    ];

    (router, listings)
}

#[tokio::test]
async fn every_list_enumerates_its_whole_collection_once_at_every_page_size() {
    let (router, listings) = fixture().await;

    for listing in &listings {
        // The collection as ONE page: every collection here is far under a page's published
        // ceiling, so this is the whole of it and the walks below must reproduce it.
        let whole = walk(&router, listing, Some(MAX_RESULTS_CEILING)).await;
        assert!(
            !whole.is_empty(),
            "case {}: expected a non-empty collection to page, got none",
            listing.label
        );
        let unique: BTreeSet<&String> = whole.iter().collect();
        assert_eq!(
            unique.len(),
            whole.len(),
            "case {}: expected each entry once, got {whole:?}",
            listing.label
        );

        for page_size in page_sizes(whole.len()) {
            let case = format!("{} maxResults={page_size:?}", listing.label);
            let walked = walk(&router, listing, page_size).await;
            assert_eq!(
                walked, whole,
                "case {case:?}: expected the pages to enumerate the collection once in one \
                 order, got {walked:?} instead of {whole:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_a_list_never_issued_is_refused_and_each_list_keeps_its_own() {
    let (router, listings) = fixture().await;

    // The token each list issues for its own first page of one entry. A list accepts its
    // own, which is what makes the refusals below about the token rather than about
    // resuming at all.
    let mut issued: Vec<(usize, String)> = Vec::new();
    for (index, listing) in listings.iter().enumerate() {
        let first = page(&router, listing, Some(MAX_RESULTS_FLOOR), None).await;
        let token = next_token(&first).unwrap_or_else(|| {
            panic!(
                "case {}: a full page of one entry carries a nextToken: {:?}",
                listing.label, first.body
            )
        });
        let resumed = page(&router, listing, Some(MAX_RESULTS_FLOOR), Some(&token)).await;
        assert_eq!(
            resumed.status, 200,
            "case {}: expected its own token to be accepted, got {} with {:?}",
            listing.label, resumed.status, resumed.body
        );
        issued.push((index, token));
    }

    for (index, listing) in listings.iter().enumerate() {
        // The rows: a plain word, a token of EVERY OTHER list of this service (each names
        // its own collection, so none of them is a resume point here), and this list's own
        // token with a character put in front of it or after it.
        let own = &issued[index].1;
        let mut rows: Vec<(String, String)> = vec![
            ("a-plain-word".to_string(), "nonsense".to_string()),
            (
                "its-own-token-with-a-leading-character".to_string(),
                format!("x{own}"),
            ),
            (
                "its-own-token-with-a-trailing-character".to_string(),
                format!("{own}x"),
            ),
        ];
        rows.extend(
            issued
                .iter()
                .filter(|(other, _)| *other != index)
                .map(|(other, token)| {
                    (
                        format!("the-token-of-{}", listings[*other].label),
                        token.clone(),
                    )
                }),
        );

        for (case, bad_token) in rows {
            let answer = page(&router, listing, None, Some(&bad_token)).await;
            assert_eq!(
                answer.status,
                400,
                "case {}/{case}: expected HTTP 400 for {bad_token:?}, got {} with {:?}",
                listing.label,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {}/{case}: expected ValidationException for {bad_token:?}, got {:?}",
                listing.label,
                answer.body
            );
        }

        // `maxResults` on each published bound, and one past each of them.
        for (case, max_results, expected) in [
            ("at-the-floor", MAX_RESULTS_FLOOR, 200),
            ("at-the-ceiling", MAX_RESULTS_CEILING, 200),
            ("below-the-floor", MAX_RESULTS_FLOOR - 1, 400),
            ("above-the-ceiling", MAX_RESULTS_CEILING + 1, 400),
        ] {
            let answer = page(&router, listing, Some(max_results), None).await;
            assert_eq!(
                answer.status, expected,
                "case {}/maxResults-{case}: expected HTTP {expected}, got {} with {:?}",
                listing.label, answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn paging_a_list_changes_nothing_a_later_caller_sees() {
    let (router, listings) = fixture().await;

    // A read must not change what a later caller sees: the same walk, before and after a
    // walk at another page size, reports the same entries in the same order.
    for listing in &listings {
        for page_size in [Some(MAX_RESULTS_FLOOR), Some(3), None] {
            let case = format!("{} maxResults={page_size:?}", listing.label);
            let before = walk(&router, listing, page_size).await;
            let _ = walk(&router, listing, Some(2)).await;
            let after = walk(&router, listing, page_size).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected paging twice to report the same entries, got {after:?} \
                 after {before:?}"
            );
        }
    }
}
