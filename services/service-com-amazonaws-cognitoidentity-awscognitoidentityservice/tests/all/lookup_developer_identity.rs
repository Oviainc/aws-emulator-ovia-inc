// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `LookupDeveloperIdentity`: the developer-identity mappings of a
//! stored pool, reported from what `GetOpenIdTokenForDeveloperIdentity` wrote.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * a lookup resolves the OTHER side of a pool's developer-identity mapping — an
//!   identifier answers the identity it is registered under, an identity answers every
//!   developer user identifier linked to it — and a request carrying both is answered with
//!   that same pair once the mapping verifies it;
//! * a page carries at most `MaxResults` identifiers in a stable order and resumes where
//!   the previous page ended, with no token on the last page;
//! * a pair the mapping does not verify is a 409 `ResourceConflictException`, a resource
//!   this service does not hold is a 404 `ResourceNotFoundException`, an identifier the
//!   pool holds no mapping for and an identity of ANOTHER pool are both a 403
//!   `NotAuthorizedException`, and a request naming neither side of the mapping is a 400
//!   `InvalidParameterException`, while a value outside a constraint the MODEL states is
//!   refused ahead of the operation with a 400 `ValidationException`;
//! * the lookup is a REPORT: repeating it answers the same thing and leaves the pool's
//!   identities exactly as they were.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// The operation under test, spelled once.
const OPERATION: &str = "LookupDeveloperIdentity";

/// A well-formed `IdentityId` that nothing ever minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:33333333-3333-3333-3333-333333333333";

/// Register `identifier` as a developer login of `pool_id` through the operation that
/// writes the mapping, and return the `IdentityId` it resolved to. `identity_id` names an
/// existing identity to link the identifier to, as that operation's own `IdentityId`
/// member does.
async fn register(
    emulator: &Emulator,
    pool_id: &str,
    developer_provider: &str,
    identifier: &str,
    identity_id: Option<&str>,
) -> String {
    let minted = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &token_request(
                pool_id,
                &[(developer_provider, identifier)],
                identity_id,
                None,
            ),
        )
        .await;
    minted["IdentityId"]
        .as_str()
        .expect("GetOpenIdTokenForDeveloperIdentity returns an IdentityId")
        .to_string()
}

/// The `LookupDeveloperIdentity` request for one case: every optional member is sent only
/// when the case carries it, so a case with `None` exercises that member's absence.
fn lookup_request(
    pool_id: &str,
    identity_id: Option<&str>,
    identifier: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Value {
    let mut request = Map::new();
    request.insert("IdentityPoolId".to_string(), json!(pool_id));
    if let Some(identity_id) = identity_id {
        request.insert("IdentityId".to_string(), json!(identity_id));
    }
    if let Some(identifier) = identifier {
        request.insert("DeveloperUserIdentifier".to_string(), json!(identifier));
    }
    if let Some(max_results) = max_results {
        request.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        request.insert("NextToken".to_string(), json!(next_token));
    }
    Value::Object(request)
}

/// The `DeveloperUserIdentifierList` of one answer.
fn listed_identifiers(case: &str, body: &Value) -> Vec<String> {
    body["DeveloperUserIdentifierList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: {OPERATION} answers a DeveloperUserIdentifierList: {body}")
        })
        .iter()
        .map(|identifier| {
            identifier
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a developer user identifier is a string: {body}")
                })
                .to_string()
        })
        .collect()
}

/// The `IdentityId` of one answer.
fn answered_identity(case: &str, body: &Value) -> String {
    body["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {OPERATION} answers an IdentityId: {body}"))
        .to_string()
}

/// Every identity of a pool, as `ListIdentities` reports them — the state a report must
/// leave untouched.
async fn listed_identities(emulator: &Emulator, pool_id: &str) -> Value {
    let listed = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": pool_id, "MaxResults": 60}),
        )
        .await;
    listed["Identities"].clone()
}

/// RULE: a lookup resolves the other side of the pool's developer-identity mapping — the
/// identifier answers the identity it is registered under, the identity answers that
/// identifier, and a request carrying both is answered with the same pair.
#[tokio::test]
async fn a_lookup_resolves_the_other_side_of_the_developer_identity_mapping() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "lookup", Some(developer_provider)).await;

    // The rows differ along what the rule generalises over: the SPELLING and the SIZE of
    // the developer user identifier, whose only modelled constraint is `length 1..1024`
    // with no pattern (`smithy_query(query="operation",operation="LookupDeveloperIdentity")`),
    // so every row below is a value the member admits — including each published bound.
    let at_max_length = "x".repeat(1024);
    let cases: Vec<(&str, &str)> = vec![
        ("at the minimum modelled length", "u"),
        ("a typical backend user id", "user-1234"),
        ("an email-like identifier", "person@example.com"),
        ("spaces and punctuation", "Jane Doe #7 (eu)"),
        ("non-ASCII", "ユーザー-42"),
        ("at the maximum modelled length", at_max_length.as_str()),
    ];

    for (case, identifier) in &cases {
        let identity_id = register(&emulator, &pool_id, developer_provider, identifier, None).await;

        // The identifier alone: "the other value will be searched in the database and
        // returned as a part of the response".
        let by_identifier = emulator
            .ok(
                OPERATION,
                &lookup_request(&pool_id, None, Some(identifier), None, None),
            )
            .await;
        assert_eq!(
            answered_identity(case, &by_identifier),
            identity_id,
            "case {case:?}: expected the identity {identity_id} the identifier is registered \
             under, got {by_identifier}"
        );
        assert_eq!(
            listed_identifiers(case, &by_identifier),
            vec![(*identifier).to_string()],
            "case {case:?}: expected the requested identifier back, got {by_identifier}"
        );
        assert!(
            by_identifier.get("NextToken").is_none(),
            "case {case:?}: a single-identifier answer is the last page, got {by_identifier}"
        );

        // The identity alone: the identifiers linked to it.
        let by_identity = emulator
            .ok(
                OPERATION,
                &lookup_request(&pool_id, Some(&identity_id), None, None, None),
            )
            .await;
        assert_eq!(
            answered_identity(case, &by_identity),
            identity_id,
            "case {case:?}: expected the requested identity echoed, got {by_identity}"
        );
        assert_eq!(
            listed_identifiers(case, &by_identity),
            vec![(*identifier).to_string()],
            "case {case:?}: expected the identifier linked to {identity_id}, got {by_identity}"
        );

        // Both: "If the values are verified against the database, the response returns
        // both values and is the same as the request."
        let by_both = emulator
            .ok(
                OPERATION,
                &lookup_request(&pool_id, Some(&identity_id), Some(identifier), None, None),
            )
            .await;
        assert_eq!(
            answered_identity(case, &by_both),
            identity_id,
            "case {case:?}: expected the request's own identity, got {by_both}"
        );
        assert_eq!(
            listed_identifiers(case, &by_both),
            vec![(*identifier).to_string()],
            "case {case:?}: expected the request's own identifier, got {by_both}"
        );
    }
}

/// RULE: an identity answers EVERY developer user identifier linked to it ("Cognito
/// supports the association of multiple developer user identifiers with an identity ID"),
/// and a page carries at most `MaxResults` of them, resuming where the previous page
/// ended, so the pages together are the whole set exactly once.
#[tokio::test]
async fn an_identity_answers_every_identifier_linked_to_it_one_page_at_a_time() {
    let emulator = Emulator::new();
    let developer_provider = "backend.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "pages", Some(developer_provider)).await;

    // Deliberately not in sorted order, so a stable page boundary cannot come from the
    // order the identifiers were written in.
    let identifiers = ["m-user", "a-user", "z-user", "c-user", "b-user"];
    let identity_id = register(&emulator, &pool_id, developer_provider, identifiers[0], None).await;
    for identifier in &identifiers[1..] {
        let linked = register(
            &emulator,
            &pool_id,
            developer_provider,
            identifier,
            Some(&identity_id),
        )
        .await;
        assert_eq!(
            linked, identity_id,
            "identifier {identifier:?} must link to the identity the call named"
        );
    }
    let mut expected: Vec<String> = identifiers.iter().map(|id| (*id).to_string()).collect();
    expected.sort();

    // One row per page size: each published bound of the modelled `range 1..60`, the sizes
    // between that split the set, one that exceeds it, and the member's absence.
    let page_sizes: Vec<(&str, Option<i64>)> = vec![
        ("no MaxResults", None),
        ("MaxResults at the modelled minimum", Some(1)),
        ("MaxResults splitting the set", Some(2)),
        ("MaxResults exactly the set size", Some(5)),
        ("MaxResults past the set size", Some(6)),
        ("MaxResults at the modelled maximum", Some(60)),
    ];

    for (case, page_size) in page_sizes {
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = emulator
                .ok(
                    OPERATION,
                    &lookup_request(
                        &pool_id,
                        Some(&identity_id),
                        None,
                        page_size,
                        next_token.as_deref(),
                    ),
                )
                .await;
            let on_page = listed_identifiers(case, &page);
            if let Some(page_size) = page_size {
                assert!(
                    i64::try_from(on_page.len()).unwrap_or(i64::MAX) <= page_size,
                    "case {case:?}: a page carries at most {page_size} identifiers, got {page}"
                );
            }
            assert_eq!(
                answered_identity(case, &page),
                identity_id,
                "case {case:?}: every page names the identity asked about, got {page}"
            );
            collected.extend(on_page);
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= identifiers.len() + 1,
                "case {case:?}: pagination must terminate, got {pages} pages for \
                 {} identifiers",
                identifiers.len()
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: the pages together are every identifier of {identity_id}, once, in \
             a stable order"
        );
    }
}

/// RULE: a request the mapping cannot answer is refused with the modelled error for what
/// is wrong with it — a pair the store does not link is a conflict, a resource this service
/// does not hold is not found, an identifier the pool holds no mapping for and an identity
/// of another pool are forbidden, and an input the operation cannot act on is a bad
/// parameter.
#[tokio::test]
async fn a_request_the_mapping_cannot_answer_is_refused() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "refusals", Some(developer_provider)).await;
    let other_pool = pool_with_developer_provider(&emulator, "other", Some(developer_provider)).await;
    let registered = register(&emulator, &pool_id, developer_provider, "user-1", None).await;
    let bare = emulator.identity(&pool_id, None).await;
    let foreign = emulator.identity(&other_pool, None).await;
    // A pagination token ANOTHER pool issued: two identifiers on one identity of that
    // pool, listed one at a time, so the first page carries a token scoped to that pool.
    let foreign_identity = register(&emulator, &other_pool, developer_provider, "a-user", None).await;
    register(
        &emulator,
        &other_pool,
        developer_provider,
        "b-user",
        Some(&foreign_identity),
    )
    .await;
    let foreign_page = emulator
        .ok(
            OPERATION,
            &lookup_request(&other_pool, Some(&foreign_identity), None, Some(1), None),
        )
        .await;
    let foreign_token = foreign_page["NextToken"]
        .as_str()
        .expect("a page of one of two identifiers carries a NextToken")
        .to_string();
    // A pool that names no developer provider at all, so it can hold no mapping.
    let no_developer_provider =
        pool_with_developer_provider(&emulator, "no-developer-provider", None).await;

    // (case, request, expected status, expected error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a pool this service does not hold",
            lookup_request(ABSENT_POOL_ID, None, Some("user-1"), None, None),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identity nothing ever minted",
            lookup_request(&pool_id, Some(ABSENT_IDENTITY_ID), None, None, None),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identifier that is not registered",
            lookup_request(&pool_id, None, Some("never-registered"), None, None),
            403,
            "NotAuthorizedException",
        ),
        (
            "an identifier of a pool that names no developer provider",
            lookup_request(&no_developer_provider, None, Some("user-1"), None, None),
            403,
            "NotAuthorizedException",
        ),
        (
            "an identity of ANOTHER pool",
            lookup_request(&pool_id, Some(&foreign), None, None, None),
            403,
            "NotAuthorizedException",
        ),
        (
            "a pair the store does not link",
            lookup_request(&pool_id, Some(&bare), Some("user-1"), None, None),
            409,
            "ResourceConflictException",
        ),
        (
            "a pair whose identifier is not registered at all",
            lookup_request(&pool_id, Some(&registered), Some("never-registered"), None, None),
            409,
            "ResourceConflictException",
        ),
        (
            "neither side of the mapping",
            lookup_request(&pool_id, None, None, None, None),
            400,
            "InvalidParameterException",
        ),
        (
            "a NextToken this pool never issued",
            lookup_request(&pool_id, Some(&registered), None, None, Some("deadbeef")),
            400,
            "InvalidParameterException",
        ),
        (
            "a NextToken ANOTHER pool issued",
            lookup_request(&pool_id, Some(&registered), None, None, Some(&foreign_token)),
            400,
            "InvalidParameterException",
        ),
        (
            "MaxResults one below the modelled minimum",
            lookup_request(&pool_id, Some(&registered), None, Some(0), None),
            400,
            "ValidationException",
        ),
        (
            "MaxResults one past the modelled maximum",
            lookup_request(&pool_id, Some(&registered), None, Some(61), None),
            400,
            "ValidationException",
        ),
        (
            "an identifier one character past the modelled maximum length",
            lookup_request(&pool_id, None, Some(&"x".repeat(1025)), None, None),
            400,
            "ValidationException",
        ),
        (
            "an empty identifier",
            lookup_request(&pool_id, None, Some(""), None, None),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, body) = emulator.call(OPERATION, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // An identity of a pool that holds no mapping for it is not a refusal: it simply has
    // no developer user identifier.
    let empty = emulator
        .ok(
            OPERATION,
            &lookup_request(&pool_id, Some(&bare), None, None, None),
        )
        .await;
    assert_eq!(
        listed_identifiers("an identity with no developer identifier", &empty),
        Vec::<String>::new(),
        "an identity with no developer identifier answers an empty list, got {empty}"
    );
}

/// RULE: the lookup REPORTS — repeating it answers the same thing, and it leaves the
/// pool's identities and mappings exactly as they were, whether it succeeded or was
/// refused.
#[tokio::test]
async fn a_lookup_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "report", Some(developer_provider)).await;
    let identity_id = register(&emulator, &pool_id, developer_provider, "user-1", None).await;
    let before = listed_identities(&emulator, &pool_id).await;

    // (case, request) — each a lookup that is answered, and each repeated below.
    let lookups: Vec<(&str, Value)> = vec![
        (
            "by identifier",
            lookup_request(&pool_id, None, Some("user-1"), None, None),
        ),
        (
            "by identity",
            lookup_request(&pool_id, Some(&identity_id), None, None, None),
        ),
        (
            "by both",
            lookup_request(&pool_id, Some(&identity_id), Some("user-1"), None, None),
        ),
    ];
    for (case, request) in &lookups {
        let first = emulator.ok(OPERATION, request).await;
        let second = emulator.ok(OPERATION, request).await;
        assert_eq!(
            first, second,
            "case {case:?}: two identical lookups answer the same thing"
        );
        assert_eq!(
            answered_identity(case, &first),
            identity_id,
            "case {case:?}: expected identity {identity_id}, got {first}"
        );
    }

    // A refused lookup writes nothing either: no identity is minted for an identifier the
    // pool does not hold.
    let (status, _) = emulator
        .call(
            OPERATION,
            &lookup_request(&pool_id, None, Some("never-registered"), None, None),
        )
        .await;
    assert_eq!(
        status, 403,
        "a lookup of an unregistered identifier is refused, got {status}"
    );

    let after = listed_identities(&emulator, &pool_id).await;
    assert_eq!(
        before, after,
        "every lookup above is a report, so the pool's identities are unchanged"
    );
}
