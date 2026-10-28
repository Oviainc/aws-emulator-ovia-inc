// SPDX-License-Identifier: CC0-1.0

//! `ListOpenIDConnectProviders`, end to end over the awsQuery wire, cross-read through
//! `GetOpenIDConnectProvider`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, XML members) of providers that were PERSISTED by
//! `CreateOpenIDConnectProvider`, never a handler called directly. The awsQuery harness is
//! the one `tests/all/user_lifecycle.rs` owns and the other suites in this crate share,
//! and the ARN derivation is the one `tests/all/open_id_connect_providers.rs` pins.
//!
//! The rule under test is that the listing reports EVERY OIDC provider the account holds,
//! exactly once, as the subset of attributes `OpenIDConnectProviderListEntry` declares —
//! the ARN that names it, which is the handle `GetOpenIDConnectProvider` reads the same
//! provider back by — once the signing principal's identity-based policies permit the
//! call, and that it is a read: it reports no other attribute of a provider and changes
//! nothing a later caller sees.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: how many
//! providers the account holds (from none upwards), the URL's path depth, case and length
//! (including the longest the model admits), how many client IDs and thumbprints the
//! provider carries (from none to the published bound), the tag count, and the documents
//! in force on the caller — so no single request is the specification.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::open_id_connect_providers::{client_ids, expected_arn, issuer_of, list_members, thumbprint};
use crate::role_lifecycle::{borrowed, param};
use crate::saml_provider_listing::split_element;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// How many characters a provider URL may carry: `OpenIDConnectProviderUrlType` is len
/// 1..255 (`smithy_query(query="shape", shape="OpenIDConnectProviderUrlType")`), so the
/// table holds one at the published upper bound.
const MAX_PROVIDER_URL_CHARACTERS: usize = 255;

/// "IAM lets you have up to five thumbprints for an OIDC provider"
/// (`CreateOpenIDConnectProvider$ThumbprintList`) — the bound the library states, so the
/// table holds a row at it.
const MAX_THUMBPRINTS: usize = state::MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER;

/// One provider the account is made to hold, with the members the row exercises.
struct ListingCase {
    /// `Url` — "the URL must begin with https:// … path components are allowed but query
    /// parameters are not" (`CreateOpenIDConnectProvider$Url`), len 1..255.
    url: String,
    /// `ClientIDList`, in the order the request carries it. "There is no defined format
    /// for a client ID" (`CreateOpenIDConnectProvider$ClientIDList`).
    client_ids: Vec<String>,
    /// `ThumbprintList`. Each entry is exactly 40 characters, the length `thumbprintType`
    /// declares (len 40..40).
    thumbprints: Vec<String>,
    /// The tags the create carries, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

impl ListingCase {
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![param("Url", &self.url)];
        for (index, client_id) in self.client_ids.iter().enumerate() {
            params.push(param(
                &format!("ClientIDList.member.{}", index + 1),
                client_id,
            ));
        }
        for (index, thumbprint) in self.thumbprints.iter().enumerate() {
            params.push(param(
                &format!("ThumbprintList.member.{}", index + 1),
                thumbprint,
            ));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }
}

/// The longest URL the model admits: `https://` plus a host that fills the remaining
/// characters of `OpenIDConnectProviderUrlType`.
fn longest_url() -> String {
    let suffix = ".example.com";
    let host = "u".repeat(MAX_PROVIDER_URL_CHARACTERS - "https://".len() - suffix.len());
    format!("https://{host}{suffix}")
}

/// The providers this suite's rows create, in the order they are created — which is NOT
/// the order the account enumerates them, so an assertion about the reported set cannot
/// depend on the order the creates arrived in.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        // A bare hostname, carrying nothing else at all.
        ListingCase {
            url: "https://server.example.org".to_string(),
            client_ids: Vec::new(),
            thumbprints: Vec::new(),
            tags: Vec::new(),
        },
        // One path segment, one of everything.
        ListingCase {
            url: "https://example.com/oidc".to_string(),
            client_ids: client_ids(1),
            thumbprints: vec![thumbprint(1)],
            tags: vec![("env", "dev")],
        },
        // A nested path.
        ListingCase {
            url: "https://oidc.eks.us-east-1.amazonaws.com/id/EXAMPLED539D4633E53DE1B716D3041E"
                .to_string(),
            client_ids: client_ids(3),
            thumbprints: vec![thumbprint(2), thumbprint(3)],
            tags: vec![("team", "platform"), ("env", "prod")],
        },
        // A mixed-case host and path.
        ListingCase {
            url: "https://Accounts.Example.COM/Realms/Main".to_string(),
            client_ids: vec!["one.client".to_string(), "another client".to_string()],
            thumbprints: vec![thumbprint(4)],
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        // The longest URL the model admits, at the thumbprint bound.
        ListingCase {
            url: longest_url(),
            client_ids: client_ids(1),
            thumbprints: (5..5 + MAX_THUMBPRINTS as u32).map(thumbprint).collect(),
            tags: Vec::new(),
        },
        // A single-character host — the shortest value that still names one.
        ListingCase {
            url: "https://a".to_string(),
            client_ids: Vec::new(),
            thumbprints: vec![thumbprint(11)],
            tags: vec![("only", "one")],
        },
    ]
}

/// The entries of the `OpenIDConnectProviderList` a response carries, in document order,
/// read off the members `OpenIDConnectProviderListEntry` declares
/// (`smithy_query(query="shape", shape="OpenIDConnectProviderListEntry")`): the ARN alone.
///
/// A list entry is a STRUCTURE, so the shared leaf scan cannot read it: this narrows to
/// the named list and then to each entry block inside it. A response reporting an empty
/// list carries no block at all.
fn listed_arns(body: &str) -> Vec<String> {
    let Some((content, _)) = split_element(body, "OpenIDConnectProviderList") else {
        return Vec::new();
    };
    let mut arns = Vec::new();
    let mut rest = content;
    while let Some((entry, remainder)) = split_element(rest, "member") {
        arns.push(element(entry, "Arn"));
        rest = remainder;
    }
    arns
}

/// Create one fixture provider, answering the ARN the create reports.
async fn create_provider(router: &MultiServiceRouter, case: &ListingCase) -> String {
    let params = case.params();
    let (status, body) = call(router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreateOpenIDConnectProvider must succeed, got {status}: {body}",
        case.url
    );
    let arn = element(&body, "OpenIDConnectProviderArn");
    assert_eq!(
        arn,
        expected_arn(&case.url),
        "case {:?}: the ARN must be derived from the URL, got {arn:?}",
        case.url
    );
    arn
}

/// List the account's providers, asserting the call is served.
async fn list_providers(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "ListOpenIDConnectProviders", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListOpenIDConnectProviders, got {status} with \
         body {body}"
    );
    listed_arns(&body)
}

// ---------------------------------------------------------------------------
// Every held provider, exactly once
// ---------------------------------------------------------------------------

/// An account holding no OIDC provider reports an EMPTY list, not a refusal: the list is
/// the operation's whole answer, so "none held" is an answer.
#[tokio::test]
async fn an_account_holding_no_oidc_provider_reports_an_empty_list() {
    let router = router();

    for attempt in ["first", "second"] {
        let entries = list_providers(&router, attempt).await;
        assert!(
            entries.is_empty(),
            "case {attempt:?}: an account holding no provider must report no entry, got \
             {entries:?}"
        );
    }
}

/// The listing reports every provider the account holds, exactly once, however many it
/// holds: the account is grown one provider at a time and listed after each create, so the
/// reported set is asserted at every count from one upwards.
#[tokio::test]
async fn every_held_provider_is_reported_exactly_once_at_every_count() {
    let router = router();
    let mut expected_arns: Vec<String> = Vec::new();

    for case in listing_cases() {
        expected_arns.push(create_provider(&router, &case).await);
        expected_arns.sort();

        let mut reported = list_providers(&router, &case.url).await;
        reported.sort();
        assert_eq!(
            reported, expected_arns,
            "case {:?}: the listing must report every held provider exactly once, got \
             {reported:?}",
            case.url
        );
        // `arnType` is modelled len 20..2048 — the constraint on every reported ARN.
        for arn in &reported {
            assert!(
                arn.len() >= 20 && arn.len() <= 2048,
                "case {:?}: a reported ARN must be 20..2048 characters, got {} in {arn:?}",
                case.url,
                arn.len()
            );
        }
    }
}

/// Each reported ARN is the handle the same provider reads back by: the entry's ARN
/// resolves through `GetOpenIDConnectProvider` to the record the create persisted.
#[tokio::test]
async fn each_reported_arn_resolves_through_get_open_id_connect_provider() {
    let router = router();
    let cases = listing_cases();
    for case in &cases {
        create_provider(&router, case).await;
    }

    let reported = list_providers(&router, "every provider").await;
    for case in &cases {
        let arn = expected_arn(&case.url);
        assert!(
            reported.contains(&arn),
            "case {:?}: the listing must report the held provider {arn:?}, got {reported:?}",
            case.url
        );

        let read_params = vec![param("OpenIDConnectProviderArn", &arn)];
        let (status, read) =
            call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: a reported ARN must read back, got {status}: {read}",
            case.url
        );
        let reported_url = element(&read, "Url");
        assert_eq!(
            reported_url,
            issuer_of(&case.url),
            "case {:?}: the reported ARN must resolve to this provider, got {reported_url:?}",
            case.url
        );
    }
    assert_eq!(
        reported.len(),
        cases.len(),
        "the listing must report no provider the account does not hold, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// A subset of the attributes, and nothing written
// ---------------------------------------------------------------------------

/// "IAM resource-listing operations return a subset of the available attributes for the
/// resource. For example, this operation does not return tags"
/// (`ListOpenIDConnectProviders`): the subset is what the entry shape declares — the ARN
/// alone — so no other attribute of a provider (its URL, its client IDs, its thumbprints,
/// its creation instant or its tags) appears in the response, even though every row's
/// provider carries some of them.
#[tokio::test]
async fn the_listing_reports_only_the_attributes_the_entry_shape_declares() {
    let router = router();
    let cases = listing_cases();
    for case in &cases {
        create_provider(&router, case).await;
    }

    let (status, body) = call(&router, "ListOpenIDConnectProviders", &[]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListOpenIDConnectProviders, got {status} with body {body}"
    );
    for member in ["Url", "ClientIDList", "ThumbprintList", "CreateDate", "Key", "Value"] {
        assert!(
            !body.contains(&format!("<{member}>")),
            "the listing must not report <{member}>, got {body}"
        );
    }
    // Every entry carries the member the shape DOES declare.
    let reported = listed_arns(&body);
    assert_eq!(
        reported.len(),
        cases.len(),
        "expected {} entries, got {reported:?}",
        cases.len()
    );
    assert_eq!(
        elements(&body, "Arn").len(),
        cases.len(),
        "every entry must carry exactly one <Arn>, got {body}"
    );
}

/// Listing is a READ: two listings report the same entries, a provider read after them
/// still reports the record its create wrote, and a second create for a held URL is still
/// refused — so nothing a later caller sees was changed by listing.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    let cases = listing_cases();
    for case in &cases {
        create_provider(&router, case).await;
    }

    let first = list_providers(&router, "first listing").await;
    let second = list_providers(&router, "second listing").await;
    assert_eq!(
        first, second,
        "two listings must report the same entries, got {first:?} then {second:?}"
    );

    for case in &cases {
        let read_params = vec![param("OpenIDConnectProviderArn", &expected_arn(&case.url))];
        let (status, read) =
            call(&router, "GetOpenIDConnectProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held provider must still read back, got {status}: {read}",
            case.url
        );
        assert_eq!(
            list_members(&read, "ClientIDList"),
            case.client_ids,
            "case {:?}: a listing must leave the held client IDs alone, got {read}",
            case.url
        );
        assert_eq!(
            list_members(&read, "ThumbprintList"),
            case.thumbprints,
            "case {:?}: a listing must leave the held thumbprints alone, got {read}",
            case.url
        );
        assert_eq!(
            elements(&read, "Key").len(),
            case.tags.len(),
            "case {:?}: a listing must leave the held tags alone, got {read}",
            case.url
        );

        // "You cannot register the same provider multiple times in a single Amazon Web
        // Services account": the URL is still held, so the create is still refused — a
        // listing neither released nor duplicated the registration.
        let params = case.params();
        let (status, body) = call(&router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 409,
            "case {:?}: a second create for a held URL must still be refused, got {status}: \
             {body}",
            case.url
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {:?}: expected EntityAlreadyExists, got {body}",
            case.url
        );
    }

    let third = list_providers(&router, "after the refused creates").await;
    assert_eq!(
        third, first,
        "a refused create must leave the reported entries alone, got {third:?}"
    );
}

// ---------------------------------------------------------------------------
// Decided against the caller's identity-based policies
// ---------------------------------------------------------------------------

/// What a row of the authorization table expects of the signed listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the call.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// The IAM action this operation is authorized under, as a policy statement names it.
const LIST_ACTION: &str = "iam:ListOpenIDConnectProviders";

/// The resource the decision is made on and a refusal names: EVERY provider in the
/// account, which is the `oidc-provider` ARN with its name a wildcard — the request names
/// no single provider.
fn provider_collection() -> String {
    format!("arn:aws:iam::{}:oidc-provider/*", account_id())
}

/// One row of the authorization table: the documents in force on the calling principal
/// and the answer the rule requires.
struct AuthorizationCase {
    case: &'static str,
    /// `(PolicyName, PolicyDocument)` pairs, each put on the caller in this order.
    policies: Vec<(String, String)>,
    expect: Expect,
}

/// The rows: a principal with no document at all, grants that are about the request
/// (literal, differently cased, a wildcard run, scoped to the collection), grants that are
/// about another action or another resource type, and explicit denies layered over a
/// grant.
fn authorization_cases() -> Vec<AuthorizationCase> {
    let allow_everything = statement("Allow", "*", "*");
    let collection = provider_collection();
    vec![
        AuthorizationCase {
            case: "no identity-based policy at all",
            policies: Vec::new(),
            expect: Expect::NoAllow,
        },
        AuthorizationCase {
            case: "an Allow naming the action on every resource",
            policies: vec![("allow-list".to_string(), statement("Allow", LIST_ACTION, "*"))],
            expect: Expect::Served,
        },
        AuthorizationCase {
            // "IAM compares action names case-insensitively", so the spelling of the
            // action element cannot decide whether a grant applies.
            case: "an Allow spelling the action in another case",
            policies: vec![(
                "allow-mixed-case".to_string(),
                statement("Allow", "IAM:listOPENIDCONNECTproviders", "*"),
            )],
            expect: Expect::Served,
        },
        AuthorizationCase {
            case: "an Allow whose action element is a wildcard run",
            policies: vec![(
                "allow-list-star".to_string(),
                statement("Allow", "iam:List*", "*"),
            )],
            expect: Expect::Served,
        },
        AuthorizationCase {
            case: "an Allow scoped to the provider collection",
            policies: vec![(
                "allow-collection".to_string(),
                statement("Allow", LIST_ACTION, &collection),
            )],
            expect: Expect::Served,
        },
        AuthorizationCase {
            // The request addresses the whole collection, so a grant scoped to ONE held
            // provider is not about it.
            case: "an Allow scoped to one held provider",
            policies: vec![(
                "allow-one-provider".to_string(),
                statement(
                    "Allow",
                    LIST_ACTION,
                    &expected_arn(&listing_cases()[0].url),
                ),
            )],
            expect: Expect::NoAllow,
        },
        AuthorizationCase {
            case: "an Allow for a sibling listing only",
            policies: vec![(
                "allow-saml".to_string(),
                statement("Allow", "iam:ListSAMLProviders", "*"),
            )],
            expect: Expect::NoAllow,
        },
        AuthorizationCase {
            case: "an Allow scoped to another resource type",
            policies: vec![(
                "allow-users".to_string(),
                statement(
                    "Allow",
                    LIST_ACTION,
                    &format!("arn:aws:iam::{}:user/*", account_id()),
                ),
            )],
            expect: Expect::NoAllow,
        },
        AuthorizationCase {
            case: "a Deny naming the action, over a grant for everything",
            policies: vec![
                ("allow-everything".to_string(), allow_everything.clone()),
                ("deny-list".to_string(), statement("Deny", LIST_ACTION, "*")),
            ],
            expect: Expect::ExplicitDeny,
        },
        AuthorizationCase {
            case: "a Deny scoped to the provider collection, over a grant for everything",
            policies: vec![
                ("allow-everything".to_string(), allow_everything.clone()),
                (
                    "deny-collection".to_string(),
                    statement("Deny", LIST_ACTION, &collection),
                ),
            ],
            expect: Expect::ExplicitDeny,
        },
    ]
}

/// RULE: IAM authorizes its own API, so the listing is decided against the signing
/// principal's identity-based policies before any provider is reported — a `Deny` about
/// the request refuses it whatever a grant says, a grant with no such `Deny` serves it,
/// and a principal with neither is refused with HTTP 403 `AccessDenied` naming the
/// principal, the action and the resource. A refused call reports no provider at all.
#[tokio::test]
async fn the_listing_is_decided_against_the_callers_identity_policies() {
    let router = router();
    // One held provider, so a row that is served has something to report and a row that
    // is refused is visibly withholding it.
    let held = create_provider(&router, &listing_cases()[0]).await;

    for (row, case) in authorization_cases().into_iter().enumerate() {
        let user_name = format!("oidc-caller{row}");
        let access_key_id = principal(&router, case.case, &user_name, &case.policies).await;
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), "ListOpenIDConnectProviders", &[]).await;
        let case_name = case.case;

        match case.expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {case_name:?}: expected the call to be served, got {status}: {body}"
                );
                assert_eq!(
                    listed_arns(&body),
                    vec![held.clone()],
                    "case {case_name:?}: a served listing must report the held provider, got \
                     {body}"
                );
            }
            Expect::ExplicitDeny | Expect::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {case_name:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case_name:?}: expected the error code AccessDenied, got {body}"
                );
                let reason = if case.expect == Expect::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    format!("because no identity-based policy allows the {LIST_ACTION} action")
                };
                assert_access_denied_message(
                    case_name,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    LIST_ACTION,
                    &provider_collection(),
                    &reason,
                );
                assert!(
                    listed_arns(&body).is_empty(),
                    "case {case_name:?}: a refused listing must report no provider, got {body}"
                );

                // A refusal is not a one-shot: the same call is refused the same way
                // again, and still reports nothing. Every part of the message is a fact
                // of the request and of the documents in force, so the second denial
                // reads identically.
                let (again_status, again) =
                    call_signed_by(&router, Some(&access_key_id), "ListOpenIDConnectProviders", &[])
                        .await;
                assert_eq!(
                    again_status, 403,
                    "case {case_name:?}: the second call must be refused too, got \
                     {again_status}: {again}"
                );
                assert_access_denied_message(
                    case_name,
                    &element(&again, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    LIST_ACTION,
                    &provider_collection(),
                    &reason,
                );
                assert_eq!(
                    element(&again, "Message"),
                    element(&body, "Message"),
                    "case {case_name:?}: the same refused call must be worded alike every \
                     time, got {again}"
                );
                assert!(
                    listed_arns(&again).is_empty(),
                    "case {case_name:?}: a refused listing must report no provider, got {again}"
                );
            }
        }
    }
}
