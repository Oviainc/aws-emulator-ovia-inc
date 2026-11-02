// SPDX-License-Identifier: CC0-1.0

//! The provenance rule EVERY paginated listing of this service answers by, driven
//! end-to-end through every one of them.
//!
//! The rule under test: a `NextToken` is a resume position one listing handed out, so a
//! listing resumes only from a token it issued itself. A token it cannot read, a token
//! another listing of this service issued (including the same operation over another
//! resource), and a token naming a record the listing does not hold are each refused with
//! 400 `ValidationException` and the one sentence this service states for the class
//! (`state::FOREIGN_NEXT_TOKEN_MESSAGE`) — never answered with a page, and never with the
//! empty page that would tell a client holding a stale token that the resource it is
//! paging has no records left.
//!
//! The rows span the class the rule generalises over rather than one recorded request:
//! every paginated operation of this service (`ListApprovalTeams`,
//! `ListIdentitySources`, `ListSessions`, `ListResourcePolicies`, `ListPolicies`,
//! `ListPolicyVersions`), the two of them that page over one resource's records driven
//! twice over DIFFERENT resources, crossed with the forms a token that was never handed
//! out takes — a value in no token grammar, the decimal position a client would guess,
//! a position of nothing and one past every record, this listing's own form for a record
//! it does not hold, and the token every OTHER listing in the same store handed out.
//!
//! Every request goes over the wire the real client uses, through the `MultiServiceRouter`
//! the harness in `approval_team_lifecycle.rs` mounts, with `MaxResults` and `NextToken`
//! bound the way each operation binds them.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, message, record_baseline_sessions, start_baseline};
use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// `MaxResults` is modelled `range(min: 1, max: 20)`: the narrowest page a client may ask
/// for, so a fixture of several records always hands out a token on its first page.
pub(crate) const NARROWEST_PAGE: i64 = 1;

/// `MaxResults`' modelled maximum, which is also its modelled `default`: wide enough that
/// one page reports every record each fixture below holds.
pub(crate) const WIDEST_PAGE: i64 = 20;

/// How many records each per-resource fixture holds, so every listing hands out a token
/// and has a record left after the one its first page reports.
const RECORDS: usize = 3;

/// Which operation a row drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    ApprovalTeams,
    IdentitySources,
    Sessions,
    ResourcePolicies,
    Policies,
    PolicyVersions,
}

/// One listing CALL: the operation, and the resource it pages over where it pages over
/// one.
pub(crate) struct Listing {
    pub(crate) kind: Kind,
    /// The resource whose records are paged: the team for `ListSessions`, the resource
    /// for `ListResourcePolicies`, the policy for `ListPolicyVersions`. Empty for the
    /// listings that name no resource.
    pub(crate) scope_arn: String,
}

impl Listing {
    /// The operation and resource this listing is, for an assertion message.
    pub(crate) fn label(&self) -> String {
        match self.kind {
            Kind::ApprovalTeams => "ListApprovalTeams".to_string(),
            Kind::IdentitySources => "ListIdentitySources".to_string(),
            Kind::Sessions => format!("ListSessions of {}", self.scope_arn),
            Kind::ResourcePolicies => format!("ListResourcePolicies of {}", self.scope_arn),
            Kind::Policies => "ListPolicies".to_string(),
            Kind::PolicyVersions => format!("ListPolicyVersions of {}", self.scope_arn),
        }
    }

    /// The output member the page's records live in.
    pub(crate) fn member(&self) -> &'static str {
        match self.kind {
            Kind::ApprovalTeams => "ApprovalTeams",
            Kind::IdentitySources => "IdentitySources",
            Kind::Sessions => "Sessions",
            Kind::ResourcePolicies => "ResourcePolicies",
            Kind::Policies => "Policies",
            Kind::PolicyVersions => "PolicyVersions",
        }
    }

    /// What identifies one reported record, so two pages of the same listing can be
    /// compared without depending on a member the clock moves.
    fn entry_id(&self, entry: &Value) -> String {
        match self.kind {
            Kind::ApprovalTeams | Kind::Policies => text(entry, "Arn"),
            Kind::IdentitySources => text(entry, "IdentitySourceArn"),
            Kind::Sessions => text(entry, "SessionArn"),
            Kind::ResourcePolicies => text(entry, "PolicyArn"),
            // `PolicyVersionSummary$VersionId` is a `PolicyVersionId` (an integer).
            Kind::PolicyVersions => entry
                .get("VersionId")
                .and_then(Value::as_i64)
                .map(|id| id.to_string())
                .unwrap_or_default(),
        }
    }

    /// This listing, as the token grammar names it.
    pub(crate) fn token_listing(&self) -> state::Listing<'_> {
        match self.kind {
            Kind::ApprovalTeams => state::Listing::ApprovalTeams,
            Kind::IdentitySources => state::Listing::IdentitySources,
            Kind::Sessions => state::Listing::Sessions {
                approval_team_arn: &self.scope_arn,
            },
            Kind::ResourcePolicies => state::Listing::ResourcePolicies {
                resource_arn: &self.scope_arn,
            },
            Kind::Policies => state::Listing::Policies,
            Kind::PolicyVersions => state::Listing::PolicyVersions {
                policy_arn: &self.scope_arn,
            },
        }
    }

    /// One page, with `MaxResults` and `NextToken` bound the way this operation binds
    /// them: to the query string, except `ListSessions`, which binds them to the body.
    pub(crate) async fn page(
        &self,
        harness: &Harness,
        max_results: i64,
        token: Option<&str>,
    ) -> (u16, Value) {
        let label = encode_label(&self.scope_arn);
        let mut query = format!("MaxResults={max_results}");
        if let Some(next) = token {
            query.push_str(&format!("&NextToken={next}"));
        }
        match self.kind {
            Kind::ApprovalTeams => {
                harness
                    .call("POST", &format!("/approval-teams/?List&{query}"), None)
                    .await
            }
            Kind::IdentitySources => {
                harness
                    .call("POST", &format!("/identity-sources/?List&{query}"), None)
                    .await
            }
            Kind::Sessions => {
                let mut body = json!({ "MaxResults": max_results });
                if let Some(next) = token {
                    body["NextToken"] = json!(next);
                }
                harness
                    .call(
                        "POST",
                        &format!("/approval-teams/{label}/sessions/?List"),
                        Some(body),
                    )
                    .await
            }
            Kind::ResourcePolicies => {
                harness
                    .call(
                        "POST",
                        &format!("/resource-policies/{label}/?List&{query}"),
                        None,
                    )
                    .await
            }
            Kind::Policies => {
                harness
                    .call("POST", &format!("/policies/?List&{query}"), None)
                    .await
            }
            Kind::PolicyVersions => {
                harness
                    .call("POST", &format!("/policies/{label}/?List&{query}"), None)
                    .await
            }
        }
    }

    /// The records one page reports, by identity.
    pub(crate) fn reported(&self, page: &Value) -> Vec<String> {
        page.get(self.member())
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| self.entry_id(entry))
            .collect()
    }

    /// Tokens in THIS listing's own grammar that name a record it does not hold, so the
    /// grammar alone cannot make a token resumable.
    fn in_grammar_but_never_handed_out(&self) -> Vec<(String, String)> {
        let listing = self.token_listing();
        match self.kind {
            // A position listing counts its records from one, so 0 is before every
            // record and a position past the fixture's last names none.
            Kind::ApprovalTeams
            | Kind::IdentitySources
            | Kind::Sessions
            | Kind::ResourcePolicies => vec![
                (
                    "its own form for the position before the first record".to_string(),
                    state::position_token(listing, 0),
                ),
                (
                    "its own form for a position past every record".to_string(),
                    state::position_token(listing, 999_999),
                ),
            ],
            // The catalog listings resume from the key the catalog orders by: a policy
            // ARN, and a version id (`PolicyVersionId` is `range(min: 1)`, so 0 is
            // outside every history).
            Kind::Policies => vec![
                (
                    "its own form for an ARN the catalog does not hold".to_string(),
                    state::listing_token(listing, &state::policy_arn("IdentityCenter", "Absent")),
                ),
                (
                    "its own form for a value that is no policy ARN".to_string(),
                    state::listing_token(listing, "7"),
                ),
            ],
            Kind::PolicyVersions => vec![
                (
                    "its own form for a version id outside every history".to_string(),
                    state::listing_token(listing, "0"),
                ),
                (
                    "its own form for a version id this policy does not hold".to_string(),
                    state::listing_token(listing, "999999"),
                ),
            ],
        }
    }
}

/// Tokens no listing of this service ever hands out, whichever listing is asked.
///
/// The bare decimal rows are what a client that counted records — or that replayed a
/// token of an emulator which handed its positions out in the clear — would send.
pub(crate) fn never_a_token() -> Vec<(&'static str, String)> {
    vec![
        ("text that is no token at all", "not-a-token".to_string()),
        ("a bare position", "1".to_string()),
        ("a bare position of nothing", "0".to_string()),
        ("a bare position past every record", "999999".to_string()),
        ("a bare negative position", "-1".to_string()),
        ("a bare position with trailing text", "1x".to_string()),
        ("a bare fractional position", "1.5".to_string()),
        (
            "a bare position past what a position can hold",
            "99999999999999999999999".to_string(),
        ),
    ]
}

/// `POST /approval-teams` — one team, with `policies` attached and one approver.
async fn create_team(harness: &Harness, name: &str, policies: &[String]) -> String {
    let body = json!({
        "Name": name,
        "Description": "pagination-token provenance",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn": format!(
                "arn:aws:mpa:{REGION}:{}:identity-source/ids-{:012}",
                engine::emulator_account_id(),
                1
            ),
        }],
        "Policies": policies
            .iter()
            .map(|arn| json!({ "PolicyArn": arn }))
            .collect::<Vec<_>>(),
    });
    let (status, created) = harness.create(&body).await;
    assert_eq!(
        status, 200,
        "creating {name:?} returned {status} with {created:?}"
    );
    text(&created, "Arn")
}

/// `POST /identity-sources` — one identity source. The instance ARN satisfies
/// `IdcInstanceArn`'s modelled pattern `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`,
/// whose instance id is 16 characters wide.
async fn create_identity_source(harness: &Harness, n: usize) -> String {
    let body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": format!("arn:aws:sso:::instance/ssoins-tokens{n:010}"),
                "Region": REGION,
            }
        }
    });
    let (status, created) = harness.call("POST", "/identity-sources", Some(body)).await;
    assert_eq!(
        status, 200,
        "creating identity source {n} returned {status} with {created:?}"
    );
    text(&created, "IdentitySourceArn")
}

/// The ARNs of the catalog policies holding at least `least` versions, in catalog order —
/// read off the store, since no modelled operation mints a policy version.
fn policies_with_versions(harness: &Harness, least: usize) -> Vec<String> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .policies
        .values()
        .filter(|policy| policy.versions.len() >= least)
        .map(|policy| policy.arn.clone())
        .collect()
}

/// A store holding, for every paginated listing of this service, a fixture of several
/// records — and the listing calls that page over them.
pub(crate) async fn fixtures() -> (Harness, Vec<Listing>) {
    let harness = Harness::new();

    // Two ACTIVE teams, each with its own sessions: the same operation over two
    // resources, which is what tells a per-operation token from a per-resource one.
    let mut session_teams: Vec<String> = Vec::new();
    for name in ["token-sessions-a", "token-sessions-b"] {
        let team_arn = active_team(&harness, name, 1, 1).await;
        // The first session is started through the operation; the rest are recorded
        // through the shared state, since the operation records at most one outstanding
        // baseline session per approver — deliberately more outstanding sessions on one
        // team than the API can mint, because this fixture needs a team whose listing
        // hands out a token.
        let (status, started) = start_baseline(&harness, &team_arn, None).await;
        assert_eq!(
            status, 200,
            "starting the first session of {name} returned {status} with {started:?}"
        );
        record_baseline_sessions(&harness, &team_arn, RECORDS - 1);
        session_teams.push(team_arn);
    }

    // Two teams with attached policies, each attaching DIFFERENT catalog entries, which
    // the resource-policy listing pages over. The harness records
    // `MPAApprovalTeamPolicy1..10`, so the two rows draw on disjoint entries.
    let mut policy_resources: Vec<String> = Vec::new();
    for (index, name) in ["token-attachments-a", "token-attachments-b"]
        .into_iter()
        .enumerate()
    {
        let attachments: Vec<String> = (1..=RECORDS)
            .map(|n| {
                let policy = format!("MPAApprovalTeamPolicy{}", index * RECORDS + n);
                format!("{}/$DEFAULT", state::policy_arn("IdentityCenter", &policy))
            })
            .collect();
        policy_resources.push(create_team(&harness, name, &attachments).await);
    }

    for n in 1..=RECORDS {
        create_identity_source(&harness, n).await;
    }

    // Two policies whose version history is long enough to page: the same operation over
    // two resources again.
    let versioned = policies_with_versions(&harness, RECORDS);
    assert!(
        versioned.len() >= 2,
        "the fixture needs two policies holding {RECORDS} versions, got {versioned:?}"
    );

    let mut listings = vec![
        Listing {
            kind: Kind::ApprovalTeams,
            scope_arn: String::new(),
        },
        Listing {
            kind: Kind::IdentitySources,
            scope_arn: String::new(),
        },
        Listing {
            kind: Kind::Policies,
            scope_arn: String::new(),
        },
    ];
    listings.extend(session_teams.into_iter().map(|team_arn| Listing {
        kind: Kind::Sessions,
        scope_arn: team_arn,
    }));
    listings.extend(policy_resources.into_iter().map(|resource_arn| Listing {
        kind: Kind::ResourcePolicies,
        scope_arn: resource_arn,
    }));
    listings.extend(versioned.into_iter().take(2).map(|policy_arn| Listing {
        kind: Kind::PolicyVersions,
        scope_arn: policy_arn,
    }));
    (harness, listings)
}

/// The refusal every token that was never handed out earns: 400 `ValidationException`
/// with the one sentence this service states for the class.
pub(crate) fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: a token this listing did not hand out must be refused, got {status} \
         with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: wrong error code in {body:?}"
    );
    assert_eq!(
        message(body),
        state::FOREIGN_NEXT_TOKEN_MESSAGE,
        "case {case:?}: expected the one pagination-token sentence, got {body:?}"
    );
}

/// Every paginated listing of this service refuses every `NextToken` it did not hand out
/// — whatever its form, including a token another listing of the same store issued — and
/// answers the one it did.
#[tokio::test]
async fn every_listing_refuses_a_next_token_it_did_not_hand_out() {
    let (harness, listings) = fixtures().await;

    // What each listing DID hand out, so the refusals below are about provenance and the
    // control at the end proves the listing still pages.
    let mut issued: Vec<String> = Vec::new();
    for listing in &listings {
        let (status, first) = listing.page(&harness, NARROWEST_PAGE, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first page returned {status} with {first:?}",
            listing.label()
        );
        let token = text(&first, "NextToken");
        assert!(
            !token.is_empty(),
            "case {:?}: a fixture of several records must hand out a token on a page of \
             {NARROWEST_PAGE}: {first:?}",
            listing.label()
        );
        issued.push(token);
    }

    for (index, listing) in listings.iter().enumerate() {
        let mut rows: Vec<(String, String)> = never_a_token()
            .into_iter()
            .map(|(case, token)| (case.to_string(), token))
            .collect();
        rows.extend(listing.in_grammar_but_never_handed_out());
        // The token every OTHER listing in this same store handed out: a token of this
        // service's, issued to someone paging something else.
        for (other_index, (other, token)) in listings.iter().zip(issued.iter()).enumerate() {
            if other_index != index {
                rows.push((
                    format!("the token {} handed out", other.label()),
                    token.clone(),
                ));
            }
        }

        for (row, token) in rows {
            let case = format!("{} refusing {row}", listing.label());
            let (status, body) = listing.page(&harness, NARROWEST_PAGE, Some(&token)).await;
            assert_refused(&case, status, &body);
        }

        // The control: the token THIS listing handed out is answered, and resumes exactly
        // after the record its first page reported.
        let (status, whole) = listing.page(&harness, WIDEST_PAGE, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the unpaged listing returned {status} with {whole:?}",
            listing.label()
        );
        let every = listing.reported(&whole);
        assert!(
            every.len() >= 2,
            "case {:?}: the fixture must hold at least two records for a token to resume \
             after one: {whole:?}",
            listing.label()
        );
        let (status, second) = listing
            .page(&harness, NARROWEST_PAGE, Some(&issued[index]))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: its own token returned {status} with {second:?}",
            listing.label()
        );
        assert_eq!(
            listing.reported(&second),
            vec![every[1].clone()],
            "case {:?}: its own token must resume after the first page: {second:?}",
            listing.label()
        );
    }
}

/// The token a listing hands out is this service's OWN value, not one the client could
/// have counted to: it is neither the record's identity nor a bare position — and a walk
/// that uses only the tokens the listing issued reports every record exactly once, in
/// the listing's own order.
#[tokio::test]
async fn an_issued_token_is_the_service_s_own_and_resumes_where_its_page_stopped() {
    let (harness, listings) = fixtures().await;

    for listing in &listings {
        let (status, whole) = listing.page(&harness, WIDEST_PAGE, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the unpaged listing returned {status} with {whole:?}",
            listing.label()
        );
        let every = listing.reported(&whole);

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page_number in 1..=every.len() + 1 {
            let case = format!("{} page {page_number}", listing.label());
            let (status, page) = listing.page(&harness, NARROWEST_PAGE, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a page returned {status} with {page:?}"
            );
            let reported = listing.reported(&page);
            walked.extend(reported.iter().cloned());
            match page.get("NextToken").and_then(Value::as_str) {
                None => break,
                Some(next) => {
                    let last = reported
                        .last()
                        .expect("a page that hands out a token reported a record");
                    // The token is the service's own composite, not the record's
                    // identity and not the bare position a client would count to.
                    assert_ne!(
                        next, last,
                        "case {case:?}: the token must not be the record's own identity: {page:?}"
                    );
                    assert!(
                        next.parse::<u64>().is_err(),
                        "case {case:?}: the token must not be a bare position, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
            }
        }
        assert_eq!(
            walked, every,
            "case {:?}: a walk on the listing's own tokens must report every record once, \
             in order, got {walked:?}",
            listing.label()
        );
    }
}
