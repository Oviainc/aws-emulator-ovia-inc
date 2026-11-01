// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's pagination cursor, driven over the real wire path:
//! every call goes through the router's `dispatch`, so the request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be.
//!
//! The rule under test, asserted over a SET of inputs rather than one request:
//!
//! * a `NextToken` is honoured only when it is, byte for byte, a token a previous page of
//!   the SAME list handed out; any other spelling — including a re-spelling of a genuine
//!   token that still decodes to the same list and offset — is a `ValidationException`
//!   (HTTP 400) rather than a page;
//! * and every token this service hands out is one such canonical spelling, so paging with
//!   the token a page gives still walks the whole list exactly once.
//!
//! The rows span the list the cursor belongs to (an unfiltered list, the same operation's
//! filtered list, and a list of another operation whose identity itself contains the `:`
//! the envelope separates on) and the way the fabricated spelling differs from the minted
//! one (the sign and the padding of the offset, and the case of the envelope's hex
//! digits).

use serde_json::{json, Value};

// ONE wire harness per service: these come from the sibling suite rather than a second
// copy of the same dispatch plumbing.
use crate::identity_provider_subscriptions::{
    as_str, call, harness, provider_union, register, start_subscription, Harness, Reply, PRODUCTS,
};

/// The sentence a refused cursor is reported with, spelled out here rather than read from
/// the production constant so a change to it fails a test instead of silently changing
/// what this suite accepts.
const INVALID_TOKEN_MESSAGE: &str = "NextToken is not a valid pagination token";

/// One paginated list to exercise: the operation's route, the request that selects the
/// list, and the output member that carries a page.
struct List {
    case: &'static str,
    path: &'static str,
    body: Value,
    member: &'static str,
}

impl List {
    fn request(&self, max_results: Option<i32>, next_token: Option<&str>) -> Value {
        let mut body = self.body.clone();
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token {
            body["NextToken"] = json!(token);
        }
        body
    }

    async fn page(&self, h: &Harness, max_results: Option<i32>, next_token: Option<&str>) -> Reply {
        call(h, self.path, &self.request(max_results, next_token)).await
    }

    /// The items of one page, as a client reads them off the reply.
    fn items(&self, reply: &Reply, case: &str) -> Vec<Value> {
        reply.ok(case)[self.member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no {} array in {:?}", self.member, reply.body))
            .clone()
    }
}

/// The directories, registrations and subscriptions the lists below page over.
///
/// Each `d-` directory is one the harness puts in the account's directory fleet; the `sd-`
/// one is a directory shared into the account. Two registrations share `PRODUCTS[0]`, so
/// the `Product`-filtered list has more than one page of its own.
const REGISTRATIONS: &[(&str, &str)] = &[
    ("d-0123456789", PRODUCTS[0]),
    ("d-00000000ff", PRODUCTS[0]),
    ("d-123456789a", PRODUCTS[1]),
    ("sd-abcdef0123", PRODUCTS[1]),
];

/// The user names subscribed through the first registration — three, so that list is
/// longer than a one-element page too.
const SUBSCRIBERS: &[&str] = &["alice", "bob@example.com", "carol.o-brien"];

/// A network the account really holds (the harness creates it), named by every
/// registration below: the subnets and the security group a registration's `Settings`
/// resolve against.
fn settings() -> Value {
    json!({ "Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcd" })
}

/// Register the identity providers and start the subscriptions the lists page over.
async fn seed(h: &Harness) {
    for (directory_id, product) in REGISTRATIONS {
        register(
            h,
            &json!({
                "IdentityProvider": provider_union(directory_id),
                "Product": product,
                "Settings": settings(),
            }),
        )
        .await
        .ok(&format!("registering {directory_id} for {product}"));
    }
    let (directory_id, product) = REGISTRATIONS[0];
    for username in SUBSCRIBERS {
        start_subscription(
            h,
            &json!({
                "Username": username,
                "IdentityProvider": provider_union(directory_id),
                "Product": product,
            }),
        )
        .await
        .ok(&format!("subscribing {username}"));
    }
}

/// The lists under test, each with more than one element so a one-element page is
/// truncated and carries a cursor.
fn lists() -> Vec<List> {
    let (directory_id, product) = REGISTRATIONS[0];
    vec![
        List {
            case: "the unfiltered identity-provider list",
            // `ListIdentityProviders` is bound to POST
            // /identity-provider/ListIdentityProviders (smithy_query: operation
            // ListIdentityProviders, http.uri).
            path: "/identity-provider/ListIdentityProviders",
            body: json!({}),
            member: "IdentityProviderSummaries",
        },
        List {
            case: "the identity-provider list a Product filter selected",
            path: "/identity-provider/ListIdentityProviders",
            body: json!({ "Filters": [{ "Attribute": "Product", "Value": product }] }),
            member: "IdentityProviderSummaries",
        },
        List {
            case: "the product-subscription list of one registration",
            // `ListProductSubscriptions` is bound to POST /user/ListProductSubscriptions
            // (smithy_query: operation ListProductSubscriptions, http.uri).
            path: "/user/ListProductSubscriptions",
            body: json!({ "IdentityProvider": provider_union(directory_id), "Product": product }),
            member: "ProductUserSummaries",
        },
    ]
}

/// How a fabricated token differs from the genuine one it was built from. Each is a
/// spelling that still decodes to the same list and the same offset, yet no page of any
/// list ever handed it out.
#[derive(Debug, Clone, Copy)]
enum Respell {
    /// The decimal offset with these characters in front of its digits: a spelling
    /// `usize`'s `FromStr` reads back but its `Display` never writes.
    OffsetPrefix(&'static str),
    /// Every hex digit of the envelope upper-cased.
    UpperCaseEnvelope,
    /// Only the envelope's first letter upper-cased.
    MixedCaseEnvelope,
}

/// Re-spell `genuine` the way `how` says, leaving the list it names and the position it
/// resumes at exactly as minted.
fn respell(genuine: &str, how: Respell) -> String {
    match how {
        Respell::OffsetPrefix(prefix) => {
            let decoded = ::hex::decode(genuine).expect("a minted cursor is hex");
            let decoded = String::from_utf8(decoded).expect("a minted cursor is text");
            // The offset is the tail after the LAST separator, so a list identity that
            // itself contains one (an ARN, say) is kept whole.
            let (kind, offset) = decoded
                .rsplit_once(':')
                .expect("a minted cursor names a list and an offset");
            ::hex::encode(format!("{kind}:{prefix}{offset}"))
        }
        Respell::UpperCaseEnvelope => genuine.to_uppercase(),
        Respell::MixedCaseEnvelope => {
            let mut respelled = String::with_capacity(genuine.len());
            let mut flipped = false;
            for digit in genuine.chars() {
                if !flipped && digit.is_ascii_alphabetic() {
                    respelled.extend(digit.to_uppercase());
                    flipped = true;
                } else {
                    respelled.push(digit);
                }
            }
            respelled
        }
    }
}

const RESPELLINGS: &[(&str, Respell)] = &[
    ("an offset with an explicit plus sign", Respell::OffsetPrefix("+")),
    ("an offset with one leading zero", Respell::OffsetPrefix("0")),
    (
        "an offset with nine leading zeros",
        Respell::OffsetPrefix("000000000"),
    ),
    (
        "an offset both signed and padded",
        Respell::OffsetPrefix("+0000"),
    ),
    ("an upper-cased envelope", Respell::UpperCaseEnvelope),
    ("a mixed-case envelope", Respell::MixedCaseEnvelope),
];

/// RULE: a cursor is honoured only when it is exactly a token a previous page of the same
/// list handed out — a re-spelling that decodes to the same list and offset is refused
/// with `ValidationException` (HTTP 400), and refusing it leaves the genuine token
/// working.
#[tokio::test]
async fn a_cursor_spelled_in_a_way_no_page_handed_out_is_refused() {
    let h = harness();
    seed(&h).await;

    for list in lists() {
        let first = list.page(&h, Some(1), None).await;
        let genuine = first.ok(list.case)["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a truncated page carries a NextToken", list.case))
            .to_string();
        assert_eq!(
            genuine,
            genuine.to_lowercase(),
            "case {:?}: a minted cursor is lower-case hex, got {genuine:?}",
            list.case
        );
        let expected = list.items(&list.page(&h, Some(1), Some(&genuine)).await, list.case);

        for (respelling, how) in RESPELLINGS {
            let case = format!("{} with {respelling}", list.case);
            let fabricated = respell(&genuine, *how);
            assert_ne!(
                fabricated, genuine,
                "case {case:?}: the fabricated token must differ from the minted one"
            );
            let reply = list.page(&h, Some(1), Some(&fabricated)).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected a 400 refusal, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                Some(INVALID_TOKEN_MESSAGE),
                "case {case:?}: expected the invalid-cursor sentence, got {reply:?}"
            );
        }

        // A refused cursor is not a state change: the genuine one still reaches the same
        // page it did before.
        let again = list.items(&list.page(&h, Some(1), Some(&genuine)).await, list.case);
        assert_eq!(
            again, expected,
            "case {:?}: the minted cursor must still reach its page, got {again:?}",
            list.case
        );
    }
}

/// RULE: every cursor this service hands out is a canonical spelling of a position in the
/// list that minted it — lower-case hex over a bare decimal offset — so paging with the
/// token a page gives walks the whole list exactly once.
#[tokio::test]
async fn every_cursor_the_service_hands_out_is_canonical_and_walks_the_list() {
    let h = harness();
    seed(&h).await;

    for list in lists() {
        let all = list.items(&list.page(&h, None, None).await, list.case);
        assert!(
            all.len() > 1,
            "case {:?}: this list needs more than one element to page over, got {all:?}",
            list.case
        );

        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let reply = list.page(&h, Some(1), token.as_deref()).await;
            seen.extend(list.items(&reply, list.case));
            token = reply.ok(list.case)["NextToken"]
                .as_str()
                .map(str::to_string);
            let Some(cursor) = token.as_deref() else {
                break;
            };
            let decoded = ::hex::decode(cursor)
                .unwrap_or_else(|_| panic!("case {:?}: {cursor:?} is not hex", list.case));
            let decoded = String::from_utf8(decoded)
                .unwrap_or_else(|_| panic!("case {:?}: {cursor:?} is not text", list.case));
            let (_, offset) = decoded.rsplit_once(':').unwrap_or_else(|| {
                panic!(
                    "case {:?}: {decoded:?} names no list and offset",
                    list.case
                )
            });
            let parsed: usize = offset
                .parse()
                .unwrap_or_else(|_| panic!("case {:?}: {offset:?} is no offset", list.case));
            assert_eq!(
                offset,
                parsed.to_string(),
                "case {:?}: an offset is spelled bare, got {offset:?}",
                list.case
            );
            assert_eq!(
                cursor,
                cursor.to_lowercase(),
                "case {:?}: an envelope is lower-case hex, got {cursor:?}",
                list.case
            );
            assert!(
                seen.len() < all.len(),
                "case {:?}: a cursor is handed out only while elements remain, got {cursor:?}",
                list.case
            );
        }
        assert_eq!(
            seen, all,
            "case {:?}: paging must walk every element exactly once, in list order",
            list.case
        );
    }
}

/// RULE: the refusal is about the spelling of the cursor, not about the operation that
/// reads it — a list reports its own identities unchanged once the cursor is the minted
/// one.
#[tokio::test]
async fn the_lists_under_test_report_what_was_registered() {
    let h = harness();
    seed(&h).await;

    let providers = lists().remove(0);
    let listed = providers.items(&providers.page(&h, None, None).await, providers.case);
    let products: Vec<&str> = listed
        .iter()
        .map(|summary| as_str(summary, "Product", providers.case))
        .collect();
    let registered: Vec<&str> = REGISTRATIONS.iter().map(|(_, product)| *product).collect();
    assert_eq!(
        products, registered,
        "every registration must be reported in registration order, got {listed:?}"
    );
}
