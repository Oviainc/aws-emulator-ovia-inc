// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` names a position in the listing it was minted from, so it resumes only a
//! call that asks for that same listing.
//!
//! The rule: a listing a request narrows (by a `filter` member, by a target, by
//! `includeChildren`) hands out a token bound to the parameters that narrowed it, so a call
//! that changes or drops any of them — and a call of a DIFFERENT operation — is refused with
//! the modelled `ValidationException` (400), in the one wording every listing refuses a
//! token no page of it handed out with. The model says so on the member itself:
//! `ListEnabledControls`' `nextToken` is "the token to continue the list from a previous API
//! call with the same parameters".
//!
//! Read as a position in the listing the LATER call selected, such a token is silent data
//! loss on a READ: the page it answers starts in the middle of a collection the caller never
//! paged, every record before that position is never served to it, and nothing in the answer
//! says records were skipped.
//!
//! The rule is asserted over TABLES whose rows differ in what it generalises over:
//!
//! * which narrowed listing minted the token (`ListControlOperations`,
//!   `ListEnabledControls`, `ListEnabledBaselines`, `ListLandingZoneOperations` — every
//!   listing of this service that narrows from the request);
//! * how the resuming call differs: the narrowing member dropped, pointed at another value,
//!   or joined by a second member that narrows the same selection further;
//! * which calls are the SAME call: the same values in another order resume, because they
//!   select the same records in the same order;
//! * which operation the token is sent to, over every pair of this service's six listings;
//! * which pairs of narrowings the token's binding must keep apart, at the shared helper,
//!   including the ones that render alike under a weaker rendering (two values against one
//!   value that spells both, one member's value against another's).
//!
//! The listings, their published `maxResults` bounds, the HTTP harness and the fixture are
//! the sibling suites': `list_pagination` holds the table of listings that share the
//! pagination rule and the records they page over, `landing_zone_lifecycle` the emulator
//! every suite drives, and the spelling a token resumes in is
//! `non_canonical_page_token_spellings`.
//!
//! The filter members and their published list lengths come from
//! `smithy_query(query="shape", shape="ControlOperationFilter,EnabledControlFilter,
//! EnabledBaselineFilter,LandingZoneOperationFilter,ControlOperationStatus,
//! LandingZoneOperationType")` on this crate's `smithy-model/`: most filter lists admit a
//! single value, `EnabledBaselineFilter`'s admit five, which is what lets the reordering row
//! below send two.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state;

use crate::enabled_control_lifecycle::ou;
use crate::landing_zone_lifecycle::{call, emulator, past_the_settle_span};
use crate::list_pagination::{
    identities, list, listings, page_body, reported_token, seed, unhandled_token_refusal, Listing,
};

/// Two baselines of the catalog this service publishes (`state::BASELINE_CATALOG`, which
/// `ListBaselines` reports), named here because the `ListEnabledBaselines` row narrows by
/// them: the one the fixture enables on several targets, and one it enables on another.
const PAGED_BASELINE: &str = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
const OTHER_BASELINE: &str = "arn:aws:controltower:us-east-1::baseline/4T4HA1KMO10S6311";

/// The listing of `path`, from the sibling suite's table of the listings that share the
/// pagination rule.
fn listing(path: &str) -> Listing {
    listings()
        .into_iter()
        .find(|listing| listing.path == path)
        .unwrap_or_else(|| panic!("{path} is not one of this service's listings"))
}

/// One row: a narrowed listing, the parameters its token is minted under, the page size that
/// leaves records after the first page, the parameter sets that must refuse that token and
/// the ones that must resume it.
struct Row {
    listing: Listing,
    case: &'static str,
    narrowed_by: serde_json::Value,
    max_results: i64,
    refuses: Vec<(&'static str, serde_json::Value)>,
    resumes: Vec<(&'static str, serde_json::Value)>,
}

/// The table: one row per listing this service narrows from the request.
fn rows() -> Vec<Row> {
    vec![
        Row {
            listing: listing("/list-control-operations"),
            case: "the operation log of one target",
            narrowed_by: serde_json::json!({
                "filter": { "targetIdentifiers": [ou("fb01")] }
            }),
            max_results: 1,
            refuses: vec![
                ("the filter dropped", serde_json::json!({})),
                (
                    "the filter pointed at another target",
                    serde_json::json!({ "filter": { "targetIdentifiers": [ou("fb02")] } }),
                ),
                (
                    "a status added to the same filter",
                    serde_json::json!({
                        "filter": {
                            "targetIdentifiers": [ou("fb01")],
                            "statuses": ["SUCCEEDED"],
                        }
                    }),
                ),
            ],
            resumes: Vec::new(),
        },
        Row {
            listing: listing("/list-enabled-controls"),
            case: "the controls enabled on one target",
            narrowed_by: serde_json::json!({ "targetIdentifier": ou("fb01") }),
            max_results: 1,
            refuses: vec![
                ("the target dropped", serde_json::json!({})),
                (
                    "another target",
                    serde_json::json!({ "targetIdentifier": ou("fb02") }),
                ),
                (
                    "a status filter added to the same target",
                    serde_json::json!({
                        "targetIdentifier": ou("fb01"),
                        "filter": { "statuses": ["SUCCEEDED"] },
                    }),
                ),
                (
                    "the same target with the child enablements asked for",
                    serde_json::json!({
                        "targetIdentifier": ou("fb01"),
                        "includeChildren": true,
                    }),
                ),
            ],
            resumes: Vec::new(),
        },
        Row {
            listing: listing("/list-enabled-baselines"),
            case: "the enablements of two baselines",
            narrowed_by: serde_json::json!({
                "filter": { "baselineIdentifiers": [PAGED_BASELINE, OTHER_BASELINE] }
            }),
            // `ListEnabledBaselinesMaxResults` is modelled `range 5..100`, so the smallest
            // page this listing admits is five records.
            max_results: 5,
            refuses: vec![
                ("the filter dropped", serde_json::json!({})),
                (
                    "one of the two baselines dropped from the filter",
                    serde_json::json!({
                        "filter": { "baselineIdentifiers": [PAGED_BASELINE] }
                    }),
                ),
                (
                    "the same filter with the inherited enablements asked for",
                    serde_json::json!({
                        "filter": { "baselineIdentifiers": [PAGED_BASELINE, OTHER_BASELINE] },
                        "includeChildren": true,
                    }),
                ),
            ],
            resumes: vec![(
                "the same two baselines in the other order",
                serde_json::json!({
                    "filter": { "baselineIdentifiers": [OTHER_BASELINE, PAGED_BASELINE] }
                }),
            )],
        },
        Row {
            listing: listing("/list-landingzone-operations"),
            case: "the operation log of one operation type",
            narrowed_by: serde_json::json!({ "filter": { "types": ["UPDATE"] } }),
            max_results: 1,
            refuses: vec![
                ("the filter dropped", serde_json::json!({})),
                (
                    "the filter pointed at another operation type",
                    serde_json::json!({ "filter": { "types": ["RESET"] } }),
                ),
                (
                    "a status added to the same filter",
                    serde_json::json!({
                        "filter": { "types": ["UPDATE"], "statuses": ["SUCCEEDED"] }
                    }),
                ),
            ],
            resumes: Vec::new(),
        },
    ]
}

/// Assert `listing` refuses `token`, in the one wording every listing refuses a token no
/// page of it handed out with, reporting no records.
async fn assert_refused(
    router: &MultiServiceRouter,
    listing: Listing,
    case: &str,
    body: serde_json::Value,
    token: &str,
) {
    let refused = call(router, listing.path, body.clone()).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?} on {} ({body}): expected 400 ValidationException for {token:?}, got \
         {:?} body {:?}",
        listing.path,
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    let expected = unhandled_token_refusal(listing, token);
    assert_eq!(
        refused.string("message"),
        expected,
        "case {case:?} on {} ({body}): expected the shared refusal wording {expected:?}, got \
         {:?}",
        listing.path,
        refused.string("message")
    );
    assert!(
        refused.body.get(listing.records).is_none(),
        "case {case:?} on {} ({body}): a refusal must report no records, got {:?}",
        listing.path,
        refused.body
    );
}

/// Assert the rule over one row.
async fn the_rule_holds_over(router: &MultiServiceRouter, row: &Row) {
    let population = identities(
        row.listing,
        &list(router, row.listing, row.narrowed_by.clone()).await,
    );
    assert!(
        population.len() as i64 > row.max_results,
        "case {:?} on {}: the fixture must select more than the {} records of one page for \
         the listing to hand out a token, got {population:?}",
        row.case,
        row.listing.path,
        row.max_results
    );

    let mut first_body = row.narrowed_by.clone();
    first_body["maxResults"] = serde_json::json!(row.max_results);
    let first = list(router, row.listing, first_body.clone()).await;
    let served = identities(row.listing, &first);
    let Some(token) = reported_token(&first) else {
        panic!(
            "case {:?} on {} ({first_body}): a page of {} of {} records must report a \
             nextToken, got {:?}",
            row.case,
            row.listing.path,
            row.max_results,
            population.len(),
            first.body
        )
    };

    for (differing, parameters) in &row.refuses {
        let mut body = parameters.clone();
        body["nextToken"] = serde_json::json!(token);
        let case = format!("{} resumed with {differing}", row.case);
        assert_refused(router, row.listing, &case, body, &token).await;
    }

    let mut resuming: Vec<(&str, serde_json::Value)> = vec![(
        "the parameters it was minted under",
        row.narrowed_by.clone(),
    )];
    resuming.extend(
        row.resumes
            .iter()
            .map(|(case, parameters)| (*case, parameters.clone())),
    );
    for (same, parameters) in resuming {
        let mut body = parameters.clone();
        body["nextToken"] = serde_json::json!(token);
        let resumed = list(router, row.listing, body.clone()).await;
        let walked = [served.clone(), identities(row.listing, &resumed)].concat();
        assert_eq!(
            walked, population,
            "case {:?} on {} resumed with {same:?} ({body}): the token must continue the \
             listing it was minted from, got {walked:?}",
            row.case, row.listing.path
        );
    }
}

/// Enable `PAGED_BASELINE` on three further targets, so the two baselines the
/// `ListEnabledBaselines` row narrows by select more enablements than that listing's
/// smallest page holds. The version is one the catalog offers for that baseline, which the
/// sibling enablement fixture is written against.
async fn seed_more_baseline_enablements(router: &MultiServiceRouter) {
    for target in ["ou-1a11-1a", "ou-2b22-2b", "ou-3c33-3c"] {
        let enabled = call(
            router,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": PAGED_BASELINE,
                "baselineVersion": "4.0",
                "targetIdentifier":
                    format!("arn:aws:organizations::123456789012:ou/o-pagingorg/{target}"),
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "enable-baseline on {target}: expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
    }
    // Past the settle span, so every seeded enablement is in its terminal status and no
    // listing below races the clock.
    tokio::time::sleep(past_the_settle_span()).await;
}

/// RULE: a `nextToken` a narrowed listing handed out resumes only a call that repeats the
/// parameters it was narrowed by; a call that changed or dropped one of them is refused,
/// rather than served a page cut out of a listing it never paged.
#[tokio::test]
async fn a_token_resumes_only_the_parameters_it_was_minted_under() {
    let router = emulator();
    seed(&router).await;
    seed_more_baseline_enablements(&router).await;

    for row in rows() {
        the_rule_holds_over(&router, &row).await;
    }
}

/// The token the first page of `listing` hands out, or `None` when the fixture's records fit
/// in one page of this listing's smallest published size, so no call of it hands out a token.
async fn minted_token(router: &MultiServiceRouter, listing: Listing) -> Option<String> {
    let page_size = listing.max_results_range.0;
    let first = list(router, listing, page_body(Some(page_size), None)).await;
    reported_token(&first)
}

/// RULE (the same rule across operations): a token one listing handed out resumes no OTHER
/// listing of this service, whichever of the six minted it — which is what every listing's
/// refusal says ("was not returned by a previous <operation> call").
#[tokio::test]
async fn a_token_resumes_no_other_operations_listing() {
    let router = emulator();
    seed(&router).await;
    seed_more_baseline_enablements(&router).await;

    let mut minted = 0usize;
    for minter in listings() {
        let Some(token) = minted_token(&router, minter).await else {
            // This listing holds no more records than its smallest page, so it hands out no
            // token to carry anywhere; it is a receiver in the rows below instead.
            continue;
        };
        minted += 1;
        for receiver in listings() {
            if receiver.path == minter.path {
                continue;
            }
            let case = format!("a token {} handed out", minter.operation);
            assert_refused(
                &router,
                receiver,
                &case,
                page_body(None, Some(&token)),
                &token,
            )
            .await;
        }
    }
    assert!(
        minted >= 5,
        "the fixture must let at least five of this service's listings hand out a token, got \
         {minted}"
    );
}

/// One narrowing at the shared helper, in the three kinds of member a handler states: a
/// single-valued member, a boolean one, and a list-valued `filter` member (as the SET a
/// handler matches against).
struct Narrowing {
    target: Option<&'static str>,
    include_children: bool,
    statuses: Option<Vec<&'static str>>,
}

impl Narrowing {
    fn key(&self) -> state::ListingKey {
        let statuses: state::Filter<'_> = self
            .statuses
            .as_ref()
            .map(|values| values.iter().copied().collect());
        state::ListingKey::new()
            .value("targetIdentifier", self.target)
            .flag("includeChildren", self.include_children)
            .filter("filter.statuses", &statuses)
    }
}

/// A narrowing that sends only `statuses`.
fn statuses(values: Option<Vec<&'static str>>) -> Narrowing {
    Narrowing {
        target: None,
        include_children: false,
        statuses: values,
    }
}

/// RULE at the shared helper: the parameters a token is bound to tell two narrowings apart
/// exactly when they select different records — so no pair of narrowings a handler can state
/// shares a token, and two spellings of the SAME narrowing do.
#[test]
fn a_token_is_bound_to_the_narrowing_a_handler_states() {
    // A listing long enough to hand out a token at the page size below; the entries
    // themselves are not what the rule generalises over.
    let listing: Vec<String> = (0..10).map(|index| format!("entry-{index}")).collect();
    let page = 4i32;

    let cases: Vec<(&str, Narrowing, Narrowing, bool)> = vec![
        (
            "one value replaced by another",
            statuses(Some(vec!["SUCCEEDED"])),
            statuses(Some(vec!["FAILED"])),
            false,
        ),
        (
            "a second value added",
            statuses(Some(vec!["SUCCEEDED"])),
            statuses(Some(vec!["SUCCEEDED", "FAILED"])),
            false,
        ),
        (
            "the member dropped",
            statuses(Some(vec!["SUCCEEDED"])),
            statuses(None),
            false,
        ),
        (
            "the member sent with no values at all",
            statuses(Some(vec![])),
            statuses(None),
            false,
        ),
        (
            "two values against one value that spells both",
            statuses(Some(vec!["SUCCEEDED", "FAILED"])),
            statuses(Some(vec!["SUCCEEDED,FAILED"])),
            false,
        ),
        (
            "one member's value moved to another member",
            Narrowing {
                target: Some("SUCCEEDED"),
                include_children: false,
                statuses: None,
            },
            statuses(Some(vec!["SUCCEEDED"])),
            false,
        ),
        (
            "a single-valued member against the empty value",
            Narrowing {
                target: Some("arn:aws:organizations::123456789012:ou/o-x/ou-1"),
                include_children: false,
                statuses: None,
            },
            Narrowing {
                target: Some(""),
                include_children: false,
                statuses: None,
            },
            false,
        ),
        (
            "the boolean member flipped",
            Narrowing {
                target: None,
                include_children: true,
                statuses: None,
            },
            statuses(None),
            false,
        ),
        (
            "the same values in another order",
            statuses(Some(vec!["SUCCEEDED", "FAILED"])),
            statuses(Some(vec!["FAILED", "SUCCEEDED"])),
            true,
        ),
        (
            "one value sent twice",
            statuses(Some(vec!["SUCCEEDED", "SUCCEEDED"])),
            statuses(Some(vec!["SUCCEEDED"])),
            true,
        ),
    ];

    // Every operation of this service that narrows its listing from the request binds its
    // token through this one rendering, so each is driven over the whole table.
    for row in listings().into_iter().filter(|row| row.token_scoped_to_parameters) {
        for (case, left, right, same_listing) in &cases {
            let (left_key, right_key) = (left.key(), right.key());
            let minted = state::page(
                row.operation,
                state::TokenScope::SameParameters(&left_key),
                None,
                Some(page),
                listing.as_slice(),
            );
            let case = format!("{} with {case:?}", row.operation);
            let token = match &minted {
                Ok((_, Some(token))) => token.clone(),
                other => panic!("case {case:?}: the first page must report a token, got {other:?}"),
            };

            let resumed = state::page(
                row.operation,
                state::TokenScope::SameParameters(&right_key),
                Some(&token),
                None,
                listing.as_slice(),
            );
            if *same_listing {
                assert_eq!(
                    resumed
                        .as_ref()
                        .map(|(entries, token)| (entries.len(), token.clone())),
                    Ok((listing.len() - page as usize, None)),
                    "case {case:?}: two spellings of one narrowing must share a token, got \
                     {resumed:?}"
                );
            } else {
                let expected = unhandled_token_refusal(row, &token);
                assert_eq!(
                    resumed.as_ref().err().map(String::as_str),
                    Some(expected.as_str()),
                    "case {case:?}: a token minted under another narrowing must be refused in \
                     the shared wording, got {:?}",
                    resumed
                        .as_ref()
                        .map(|(entries, token)| (entries.len(), token.clone()))
                );
            }
        }
    }
}
