// SPDX-License-Identifier: CC0-1.0

//! The one pagination rule every RECORD listing of this service answers by, driven
//! end-to-end through each listing that answers by it.
//!
//! The rule under test: a record listing reports the stored records ordered after the
//! position its `NextToken` names, at most `MaxResults` of them in the order the store
//! holds them, and hands back the position of the LAST record it reported exactly while
//! records remain — so a client paging with any `MaxResults` sees each record once, a
//! page followed by a token is a full page, the last page carries no token, and a
//! `NextToken` that is not a position this service handed out is refused 400
//! `ValidationException` with the one sentence this service states for that class
//! (`state::FOREIGN_NEXT_TOKEN_MESSAGE`).
//!
//! `ListApprovalTeams` (creation order), `ListIdentitySources` (creation order) and
//! `ListSessions` (one team's initiation order) all answer by that rule, so every table
//! below drives all three over the same rows: the three listings are not asserted
//! separately, they are asserted to agree.
//!
//! The rows span the class the rule generalises over — how many records the store holds
//! (none, one, a few, the most one page holds) and the page width asked for (each
//! published bound of `MaxResults`, a width the record count divides exactly, one it does
//! not, and the member left out so its modelled default applies) — rather than one
//! recorded request. Where a row names a published bound a comment names the modelled
//! member it comes from.
//!
//! Every request goes over the wire the real client uses, through the `MultiServiceRouter`
//! the harness in `approval_team_lifecycle.rs` mounts, with `MaxResults` and `NextToken`
//! bound the way each operation binds them: to the query string for the two
//! creation-order listings, to the body for `ListSessions`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, message, start_baseline};
use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// `MaxResults` is modelled `range(min: 1, max: 20)` with `default: 20` on every
/// paginated listing of this service — the minimum width a request may ask for.
const MAX_RESULTS_MIN: i64 = 1;

/// `MaxResults`' modelled maximum, which is also its modelled `default` and therefore the
/// width a request that leaves the member out is answered with.
const MAX_RESULTS_MAX: i64 = 20;

/// The most records a row stores: `MaxResults`' maximum, which is also
/// `state::MAX_IDENTITY_SOURCES`, the most identity sources this service holds at once.
const MOST_RECORDS: usize = 20;

/// Which record listing a row drives.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// `ListApprovalTeams`, over the stored teams in creation order.
    ApprovalTeams,
    /// `ListIdentitySources`, over the stored identity sources in creation order.
    IdentitySources,
    /// `ListSessions`, over ONE team's sessions in initiation order.
    Sessions,
}

/// Every listing the rule covers. A row is driven through all of them.
const KINDS: [Kind; 3] = [Kind::ApprovalTeams, Kind::IdentitySources, Kind::Sessions];

/// One listing, bound to the resource it pages over.
struct Listing {
    kind: Kind,
    /// The team whose sessions are listed; empty for the listings that name no team.
    team_arn: String,
}

impl Listing {
    /// The operation this listing is, for an assertion message.
    fn label(&self) -> &'static str {
        match self.kind {
            Kind::ApprovalTeams => "ListApprovalTeams",
            Kind::IdentitySources => "ListIdentitySources",
            Kind::Sessions => "ListSessions",
        }
    }

    /// The output member the page's records live in.
    fn member(&self) -> &'static str {
        match self.kind {
            Kind::ApprovalTeams => "ApprovalTeams",
            Kind::IdentitySources => "IdentitySources",
            Kind::Sessions => "Sessions",
        }
    }

    /// The member each reported entry carries its own ARN in.
    fn id_member(&self) -> &'static str {
        match self.kind {
            Kind::ApprovalTeams => "Arn",
            Kind::IdentitySources => "IdentitySourceArn",
            Kind::Sessions => "SessionArn",
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
        match self.kind {
            Kind::ApprovalTeams => {
                let uri = with_query("/approval-teams/?List", max_results, token);
                harness.call("POST", &uri, None).await
            }
            Kind::IdentitySources => {
                let uri = with_query("/identity-sources/?List", max_results, token);
                harness.call("POST", &uri, None).await
            }
            Kind::Sessions => {
                let mut body = json!({});
                if let Some(size) = max_results {
                    body["MaxResults"] = json!(size);
                }
                if let Some(next) = token {
                    body["NextToken"] = json!(next);
                }
                let uri = format!(
                    "/approval-teams/{}/sessions/?List",
                    encode_label(&self.team_arn)
                );
                harness.call("POST", &uri, Some(body)).await
            }
        }
    }

    /// The position the STORE gave the record `arn` names — what the listing's token has
    /// to be for a page ending on that record.
    fn stored_position(&self, harness: &Harness, arn: &str) -> u64 {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let guard = store.lock();
        let found = match self.kind {
            Kind::ApprovalTeams => guard.team(arn).map(|team| team.seq),
            Kind::IdentitySources => guard.identity_source(arn).map(|source| source.seq),
            Kind::Sessions => guard.session(arn).map(|session| session.seq),
        };
        found.unwrap_or_else(|| {
            panic!(
                "case {}: nothing is stored under {arn:?}",
                self.label()
            )
        })
    }

    /// The entries one page reports.
    fn entries(&self, page: &Value) -> Vec<Value> {
        page.get(self.member())
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The ARNs one page reports, in the order it reported them.
    fn reported(&self, page: &Value) -> Vec<String> {
        self.entries(page)
            .iter()
            .map(|entry| text(entry, self.id_member()))
            .collect()
    }
}

/// `base` (which already carries the `?List` marker) with the paging members appended.
fn with_query(base: &str, max_results: Option<i64>, token: Option<&str>) -> String {
    let mut uri = base.to_string();
    if let Some(size) = max_results {
        uri.push_str(&format!("&MaxResults={size}"));
    }
    if let Some(next) = token {
        // An ARN's `:`, `/` and `$` are all legal in a query value, so a token the table
        // writes reaches the handler as written.
        uri.push_str(&format!("&NextToken={next}"));
    }
    uri
}

/// `POST /approval-teams` — one team, with the seeded AWS-managed policy attached and one
/// approver, since what this suite asserts is the paging and not the team's own members.
async fn create_team(harness: &Harness, name: &str) -> (u16, Value) {
    let body = json!({
        "Name": name,
        "Description": "seq-token paging",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn": format!(
                "arn:aws:mpa:{REGION}:{}:identity-source/ids-{:012}",
                engine::emulator_account_id(),
                1
            ),
        }],
        "Policies": [{
            "PolicyArn": "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT",
        }],
    });
    harness.create(&body).await
}

/// `POST /identity-sources` — one identity source. The instance ARN satisfies
/// `IdcInstanceArn`'s modelled pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`, whose instance id is 16
/// characters wide.
async fn create_identity_source(harness: &Harness, n: usize) -> (u16, Value) {
    let body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": format!("arn:aws:sso:::instance/ssoins-paging{n:010}"),
                "Region": REGION,
            }
        }
    });
    harness.call("POST", "/identity-sources", Some(body)).await
}

/// A store holding `total` records of the kind `kind` pages over, and the ARNs in the
/// order the listing has to report them.
async fn fixture(kind: Kind, total: usize) -> (Harness, Listing, Vec<String>) {
    let harness = Harness::new();
    match kind {
        Kind::ApprovalTeams => {
            let mut stored = Vec::with_capacity(total);
            for n in 1..=total {
                let (status, created) = create_team(&harness, &format!("paged-team-{n:02}")).await;
                assert_eq!(
                    status, 200,
                    "creating team {n} of {total} returned {status} with {created:?}"
                );
                stored.push(text(&created, "Arn"));
            }
            let listing = Listing {
                kind,
                team_arn: String::new(),
            };
            (harness, listing, stored)
        }
        Kind::IdentitySources => {
            let mut stored = Vec::with_capacity(total);
            for n in 1..=total {
                let (status, created) = create_identity_source(&harness, n).await;
                assert_eq!(
                    status, 200,
                    "creating identity source {n} of {total} returned {status} with {created:?}"
                );
                stored.push(text(&created, "IdentitySourceArn"));
            }
            let listing = Listing {
                kind,
                team_arn: String::new(),
            };
            (harness, listing, stored)
        }
        Kind::Sessions => {
            let team_arn = active_team(&harness, "paged-session-team", 1, 1).await;
            let mut stored = Vec::with_capacity(total);
            for n in 1..=total {
                let (status, started) = start_baseline(&harness, &team_arn, None).await;
                assert_eq!(
                    status, 200,
                    "starting session {n} of {total} returned {status} with {started:?}"
                );
                stored.push(text(&started, "BaselineSessionArn"));
            }
            let listing = Listing { kind, team_arn };
            (harness, listing, stored)
        }
    }
}

/// The page-walk table: how many records the store holds, and the widths asked for
/// against that many.
fn page_rows() -> Vec<(usize, Vec<Option<i64>>)> {
    vec![
        // Nothing stored: one empty page, token or no token.
        (0, vec![None, Some(MAX_RESULTS_MIN)]),
        // One record, asked for one at a time and with the member left out.
        (1, vec![Some(MAX_RESULTS_MIN), None]),
        // A few records: the minimum width, widths that divide the count exactly and
        // ones that do not, the maximum, and the member left out.
        (
            5,
            vec![
                Some(MAX_RESULTS_MIN),
                Some(2),
                Some(4),
                Some(5),
                Some(MAX_RESULTS_MAX),
                None,
            ],
        ),
        // As many records as one page holds: the width that reports them all with
        // nothing left over, and a width that does not divide the count.
        (MOST_RECORDS, vec![Some(MAX_RESULTS_MAX), Some(7)]),
    ]
}

/// Page through `listing` with `max_results`, asserting the rule at every page, and
/// return the ARNs the pages reported in order.
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
        let on_this_page = listing.reported(&page);
        assert!(
            on_this_page.len() <= width,
            "case {case:?}: a page of at most {width} was asked for, got {} in {page:?}",
            on_this_page.len()
        );
        reported.extend(on_this_page.iter().cloned());
        pages += 1;
        assert!(
            pages <= MOST_RECORDS + 1,
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
                // The token IS the stored position of the last record the page reported,
                // which is what makes the next page resume exactly after it.
                let last = on_this_page
                    .last()
                    .expect("a full page reports at least one record");
                assert_eq!(
                    next,
                    listing.stored_position(harness, last).to_string(),
                    "case {case:?}: the token must be the stored position of {last:?}, \
                     got {next:?}"
                );
                token = Some(next.to_string());
            }
        }
    }
    reported
}

/// Every record listing of this service pages by the one seq-token rule: each stored
/// record exactly once, in the store's own order, in pages of at most `MaxResults`, with
/// the token present exactly while records remain.
#[tokio::test]
async fn every_record_listing_pages_by_the_one_seq_token_rule() {
    for kind in KINDS {
        for (total, widths) in page_rows() {
            let (harness, listing, stored) = fixture(kind, total).await;
            assert_eq!(
                stored.len(),
                total,
                "case {}: the fixture holds {} records, not {total}",
                listing.label(),
                stored.len()
            );
            for max_results in widths {
                let case = format!(
                    "{} over {total} records with MaxResults={max_results:?}",
                    listing.label()
                );
                let reported = walk(&harness, &listing, max_results, &case).await;
                assert_eq!(
                    reported, stored,
                    "case {case:?}: expected every stored record once, in stored order, \
                     got {reported:?}"
                );
                assert_eq!(
                    reported.iter().collect::<BTreeSet<_>>().len(),
                    total,
                    "case {case:?}: a record was reported twice in {reported:?}"
                );
            }
        }
    }
}

/// A `NextToken` no page of this service handed out is refused by every record listing
/// the same way: 400 `ValidationException` with the one sentence this service states for
/// the class, whatever form the token takes.
#[tokio::test]
async fn every_record_listing_refuses_a_next_token_it_did_not_issue() {
    for kind in KINDS {
        let (harness, listing, stored) = fixture(kind, 3).await;
        let foreign: Vec<(&str, String)> = vec![
            ("text that is no position at all", "not-a-token".to_string()),
            ("a negative position", "-1".to_string()),
            ("a position with trailing text", "1x".to_string()),
            ("a fractional position", "1.0".to_string()),
            (
                "a position past what a position can hold",
                "99999999999999999999999".to_string(),
            ),
            ("the ARN of a stored record", stored[0].clone()),
        ];
        for (row, token) in foreign {
            let case = format!("{} refusing {row}", listing.label());
            let (status, body) = listing
                .page(&harness, Some(MAX_RESULTS_MIN), Some(&token))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a foreign NextToken must be refused, got {status} with {body:?}"
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

        // The control: a token this listing DID hand out is answered, so the refusal is
        // about a token's provenance and not about tokens.
        let (status, first) = listing.page(&harness, Some(MAX_RESULTS_MIN), None).await;
        assert_eq!(
            status, 200,
            "case {}: the first page returned {status} with {first:?}",
            listing.label()
        );
        let issued = text(&first, "NextToken");
        assert!(
            !issued.is_empty(),
            "case {}: the first of three pages must carry a NextToken: {first:?}",
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
            listing.reported(&second),
            vec![stored[1].clone()],
            "case {}: the issued token must resume after the first page: {second:?}",
            listing.label()
        );
    }
}
