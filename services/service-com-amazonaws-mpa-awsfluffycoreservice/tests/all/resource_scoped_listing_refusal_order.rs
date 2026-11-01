// SPDX-License-Identifier: CC0-1.0

//! In WHICH ORDER a paginated listing scoped to ONE named resource decides its two
//! refusals: the request's own shape, then whether that resource exists.
//!
//! The rule, over the class these tables span: a `NextToken`'s grammar is part of the
//! request's own shape and is decidable without reading a single record, so EVERY listing
//! that pages over one named resource's records refuses a token in no grammar of its own
//! with 400 `ValidationException` (`state::FOREIGN_NEXT_TOKEN_MESSAGE`) BEFORE it reports
//! the named resource missing with 404 `ResourceNotFoundException` — the same request
//! earns the same classification from every listing of the family, and from each of them
//! whether or not the resource it names exists. The half of the token rule that DOES need
//! the records — that the key was handed out — stays behind the resource: a token this
//! listing could have issued for this very resource, naming a record it does not hold,
//! leaves the 404 to answer first.
//!
//! The rows differ along the scoped LISTING (`ListSessions` over a team's sessions,
//! `ListResourcePolicies` over a resource's attachments, `ListPolicyVersions` over a
//! policy's versions — every operation of this service whose `state::Listing` carries a
//! resource ARN), along whether the named resource EXISTS, and along the FORM of the
//! token: a value in no token grammar, the bare decimals a client that counted records
//! would send, the grammar of another listing of the same store, this listing's own
//! grammar for a DIFFERENT resource, its own grammar carrying a key of a type this
//! listing's keys never take, and its own grammar for this very resource. The policy rows
//! use a catalog category and names the seeded catalog holds
//! (`state::policy_arn`), the absent ones a name no catalog entry carries.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, with `MaxResults` and `NextToken` bound the way
//! each operation binds them, so the route, the URI bindings, the error envelope and the
//! HTTP status are all exercised.

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{create_described_team, message};
use crate::approval_team_lifecycle::{error_code, expected_arn, text, Harness};
use crate::pagination_token_provenance::{assert_refused, never_a_token, Kind, Listing, WIDEST_PAGE};

/// Which of the two refusals a row expects — the whole point of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// 400 `ValidationException`: the token is in no grammar of this listing's, which is
    /// decidable without the resource.
    ForeignToken,
    /// 404 `ResourceNotFoundException`: the request's shape is readable, so what is left
    /// to answer is that the named resource does not exist.
    ResourceNotFound,
}

/// One scoped listing, with the resources the rows drive it over.
struct Scoped {
    kind: Kind,
    /// A resource of this listing's kind that the store HOLDS.
    held: String,
    /// ANOTHER resource of the same kind the store holds, so a token this same operation
    /// issued for a different resource can be sent to this one.
    other_held: String,
    /// A well-formed ARN in the caller's account that names no resource of this kind.
    absent: String,
}

impl Scoped {
    /// This listing over `scope_arn`, as the provenance suite drives a listing.
    fn listing(&self, scope_arn: &str) -> Listing {
        Listing {
            kind: self.kind,
            scope_arn: scope_arn.to_string(),
        }
    }
}

/// A key no listing of this service keys its records by: positions are unsigned integers
/// and version ids are integers, so a word is a key of neither type.
const KEY_OF_NO_LISTING_S_TYPE: &str = "not-a-key";

/// A record key well inside every modelled key type and far past every fixture's records,
/// used where a row needs a token this listing could have rendered but never handed out
/// (`PolicyVersionId` is an integer `range(min: 1)`; a position is counted from one).
const KEY_BEYOND_EVERY_FIXTURE: &str = "999999";

/// The store every table below runs against: two resources of each scoped kind, and the
/// listings over them.
async fn fixtures() -> (Harness, Vec<Scoped>) {
    let harness = Harness::new();

    // Teams are created but not waited into ACTIVE: neither scoped listing of a team
    // depends on its status, and the rows are about which refusal answers.
    let mut teams: Vec<String> = Vec::new();
    for name in [
        "refusal-order-a",
        "refusal-order-b",
        "refusal-order-c",
        "refusal-order-d",
    ] {
        let (status, created) = create_described_team(&harness, name, 1, 1, "refusal order").await;
        assert_eq!(
            status, 200,
            "case {name:?}: creating the fixture team returned {status} with {created:?}"
        );
        teams.push(text(&created, "Arn"));
    }

    // The catalog entries `Harness` records (`MPAApprovalTeamPolicy<n>`, each holding
    // versions `1..=n`), and a name it records none under.
    let held_policy = state::policy_arn("IdentityCenter", "MPAApprovalTeamPolicy2");
    let other_held_policy = state::policy_arn("IdentityCenter", "MPAApprovalTeamPolicy3");
    let absent_policy = state::policy_arn("IdentityCenter", "NeverSeeded");

    let scoped = vec![
        Scoped {
            kind: Kind::Sessions,
            held: teams[0].clone(),
            other_held: teams[1].clone(),
            absent: expected_arn("refusal-order-no-such-team"),
        },
        Scoped {
            kind: Kind::ResourcePolicies,
            held: teams[2].clone(),
            other_held: teams[3].clone(),
            absent: expected_arn("refusal-order-no-such-resource"),
        },
        Scoped {
            kind: Kind::PolicyVersions,
            held: held_policy,
            other_held: other_held_policy,
            absent: absent_policy,
        },
    ];
    (harness, scoped)
}

/// The token rows one scoped listing is driven over for the resource `scope_arn`, each
/// with the refusal the rule says it earns.
///
/// The grammar is this service's own (`state::listing_token` / `state::position_token`),
/// so the rows that name another listing or another resource are rendered the way that
/// listing would render them rather than spelled out here.
fn token_rows(scoped: &Scoped, scope_arn: &str) -> Vec<(String, Option<String>, Expected)> {
    let this = scoped.listing(scope_arn);
    let other = scoped.listing(&scoped.other_held);
    let mut rows: Vec<(String, Option<String>, Expected)> = never_a_token()
        .into_iter()
        .map(|(case, token)| (case.to_string(), Some(token), Expected::ForeignToken))
        .collect();
    rows.push((
        "the grammar of a listing of this service that names no resource".to_string(),
        Some(state::position_token(state::Listing::ApprovalTeams, 1)),
        Expected::ForeignToken,
    ));
    rows.push((
        "the grammar of a listing of this service over a catalog key".to_string(),
        Some(state::listing_token(
            state::Listing::Policies,
            &state::policy_arn("IdentityCenter", "MPAApprovalTeamPolicy1"),
        )),
        Expected::ForeignToken,
    ));
    rows.push((
        "this listing's own grammar for ANOTHER resource".to_string(),
        Some(state::listing_token(
            other.token_listing(),
            KEY_BEYOND_EVERY_FIXTURE,
        )),
        Expected::ForeignToken,
    ));
    rows.push((
        "this listing's own grammar carrying a key of no listing's key type".to_string(),
        Some(state::listing_token(
            this.token_listing(),
            KEY_OF_NO_LISTING_S_TYPE,
        )),
        Expected::ForeignToken,
    ));
    // The grammar half of the rule is satisfied here, so what the request is left asking
    // is about the resource itself: the 404 answers, exactly as it does with no token.
    rows.push((
        "this listing's own grammar for this very resource".to_string(),
        Some(state::listing_token(
            this.token_listing(),
            KEY_BEYOND_EVERY_FIXTURE,
        )),
        Expected::ResourceNotFound,
    ));
    rows.push((
        "no token at all".to_string(),
        None,
        Expected::ResourceNotFound,
    ));
    rows
}

/// The refusal a request naming a resource this store does not hold earns: 404
/// `ResourceNotFoundException` with the one sentence this service states for the class.
fn assert_not_found(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the resource refusal, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: wrong error code in {body:?}"
    );
    assert_eq!(
        message(body),
        state::RESOURCE_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the one resource-not-found sentence, got {body:?}"
    );
}

/// For a resource the store does NOT hold, every scoped listing of this service answers
/// the token's grammar first: a token in no grammar of its own is refused 400
/// `ValidationException`, and only a request whose shape it can read is answered 404.
#[tokio::test]
async fn a_foreign_next_token_is_refused_before_the_named_resource_is_resolved() {
    let (harness, scoped) = fixtures().await;

    for listing in &scoped {
        let absent = listing.listing(&listing.absent);
        for (row, token, expected) in token_rows(listing, &listing.absent) {
            let case = format!("{} with {row}", absent.label());
            let (status, body) = absent.page(&harness, WIDEST_PAGE, token.as_deref()).await;
            match expected {
                Expected::ForeignToken => assert_refused(&case, status, &body),
                Expected::ResourceNotFound => assert_not_found(&case, status, &body),
            }
        }
    }
}

/// The classification does not depend on the resource: the SAME token rows sent to a
/// resource the store HOLDS earn the same 400 `ValidationException`, and the listing
/// still pages the resource's records when the request carries no token — so reading the
/// grammar first neither hides the 404 nor replaces the page.
#[tokio::test]
async fn the_same_token_earns_the_same_refusal_for_a_resource_the_store_holds() {
    let (harness, scoped) = fixtures().await;

    for listing in &scoped {
        let held = listing.listing(&listing.held);
        for (row, token, expected) in token_rows(listing, &listing.held) {
            // The last two rows are the resource rows: against a resource that EXISTS a
            // readable token is decided against its records (the provenance suite's
            // table) and no token at all is a page, so this table drives the
            // grammar rows here.
            if expected != Expected::ForeignToken {
                continue;
            }
            let case = format!("{} with {row}", held.label());
            let (status, body) = held.page(&harness, WIDEST_PAGE, token.as_deref()).await;
            assert_refused(&case, status, &body);
        }

        let (status, page) = held.page(&harness, WIDEST_PAGE, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the untokened listing of a stored resource returned {status} with \
             {page:?}",
            held.label()
        );
        assert!(
            page.get(held.member()).is_some(),
            "case {:?}: the page reports no {} member: {page:?}",
            held.label(),
            held.member()
        );
    }
}
