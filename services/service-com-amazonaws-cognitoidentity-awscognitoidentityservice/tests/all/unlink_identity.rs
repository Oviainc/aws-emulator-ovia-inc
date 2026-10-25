// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UnlinkIdentity`: the links between a stored identity and the
//! login providers a call names are FORGOTTEN, so the pool no longer resolves those
//! logins to that identity.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * a provider the call names is unlinked from the identity — every login value the pool
//!   had indexed for it against that identity is dropped, so a later `GetId` presenting
//!   one mints a NEW identity ("unlinked logins will be considered new identities next
//!   time they are seen"), while a provider the call does not name still resolves to the
//!   same identity;
//! * the identity record survives, carrying exactly the providers that were left; removing
//!   its last one leaves it stored and reachable through no login at all, which is what
//!   makes it inaccessible;
//! * a call that removes no link changes nothing a later caller sees, so unlinking is
//!   idempotent;
//! * a call the store cannot act on is refused with the modelled error for what is wrong
//!   with it — an identity this service does not hold is a 404, a login already linked to
//!   another identity is a 409, a `Logins` entry the pool does not accept is a 403 — and a
//!   value outside a constraint the MODEL states is refused ahead of the operation with a
//!   400 `ValidationException`.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{error_code, Emulator};

/// The operation under test, spelled once.
const OPERATION: &str = "UnlinkIdentity";

/// A well-formed `IdentityId` that no `GetId` ever minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:44444444-4444-4444-4444-444444444444";

/// Provider names of federated logins. The first five are the ones `GetId$Logins`
/// publishes for this service ("Facebook: graph.facebook.com … Google:
/// accounts.google.com Amazon: www.amazon.com Twitter: api.twitter.com Digits:
/// www.digits.com", `smithy_query(query="doc",shape="GetIdInput",member="Logins")`); the
/// rest are OpenID Connect issuers, which the modelled `IdentityProviderName` — any string
/// of `length 1..128` — equally admits.
const PROVIDERS: [&str; 10] = [
    "graph.facebook.com",
    "accounts.google.com",
    "www.amazon.com",
    "api.twitter.com",
    "www.digits.com",
    "oidc.example.com",
    "auth.example.net",
    "idp.example.org/tenant-1",
    "idp.example.org/tenant-2",
    "login.example.co.uk",
];

/// One `Logins` entry as a case row carries it: the provider name and the token presented
/// for it.
type Login = (&'static str, String);

/// One row of the unlink table: the case name, the logins the identity is linked through,
/// and the provider names the call removes.
type UnlinkCase = (&'static str, Vec<Login>, Vec<&'static str>);

/// A `Logins` map as JSON: provider name -> provider token.
fn logins_map(entries: &[(&str, String)]) -> Value {
    let mut map = Map::new();
    for (provider, token) in entries {
        map.insert((*provider).to_string(), json!(token));
    }
    Value::Object(map)
}

/// The `UnlinkIdentity` request for one case. Every member is sent only when the case
/// carries it, so a case with `None` exercises that member's absence.
fn unlink_request(
    identity_id: Option<&str>,
    logins: Option<Value>,
    logins_to_remove: Option<Value>,
) -> Value {
    let mut request = Map::new();
    if let Some(identity_id) = identity_id {
        request.insert("IdentityId".to_string(), json!(identity_id));
    }
    if let Some(logins) = logins {
        request.insert("Logins".to_string(), logins);
    }
    if let Some(logins_to_remove) = logins_to_remove {
        request.insert("LoginsToRemove".to_string(), logins_to_remove);
    }
    Value::Object(request)
}

/// The description `ListIdentities` reports for one identity of a pool — the read-back path
/// for what this operation wrote.
async fn description(emulator: &Emulator, pool_id: &str, identity_id: &str) -> Value {
    let listed = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": pool_id, "MaxResults": 60}),
        )
        .await;
    listed["Identities"]
        .as_array()
        .unwrap_or_else(|| panic!("ListIdentities answers an Identities list: {listed}"))
        .iter()
        .find(|described| described["IdentityId"] == json!(identity_id))
        .unwrap_or_else(|| panic!("ListIdentities still reports {identity_id}: {listed}"))
        .clone()
}

/// The provider names one identity is linked to, as its description reports them.
fn linked_providers(case: &str, described: &Value) -> Vec<String> {
    described["Logins"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: an identity description carries Logins: {described}")
        })
        .iter()
        .map(|provider| {
            provider
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a provider name is a string: {described}")
                })
                .to_string()
        })
        .collect()
}

/// `providers` sorted, which is the order an identity's `Logins` list is stored in.
fn sorted(providers: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = providers.iter().map(|name| (*name).to_string()).collect();
    sorted.sort();
    sorted
}

/// RULE: unlinking a provider from an identity drops every login the pool had indexed for
/// that provider against it, so a later `GetId` presenting one of those logins mints a NEW
/// identity, while a login of a provider the call did not name still resolves to the same
/// identity, which keeps exactly the providers that were left.
#[tokio::test]
async fn an_unlinked_login_resolves_to_a_new_identity_next_time() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: HOW MANY providers the identity
    // is linked to (one up to the modelled `LoginsMap` maximum of ten), how many of them the
    // call removes (one, several, every one), and the spelling and SIZE of the login value
    // (`IdentityProviderToken` is modelled `length 1..50000`, so two rows sit on its
    // published bounds).
    let at_max_token_length = "t".repeat(50_000);
    let ten_providers: Vec<Login> = PROVIDERS
        .iter()
        .enumerate()
        .map(|(index, provider)| (*provider, format!("token-{index}")))
        .collect();
    let cases: Vec<UnlinkCase> = vec![
        (
            "one provider, the only one removed",
            vec![("graph.facebook.com", "fb-token".to_string())],
            vec!["graph.facebook.com"],
        ),
        (
            "two providers, one removed",
            vec![
                ("graph.facebook.com", "fb-token".to_string()),
                ("accounts.google.com", "google-token".to_string()),
            ],
            vec!["accounts.google.com"],
        ),
        (
            "three providers, two removed",
            vec![
                ("graph.facebook.com", "fb-token".to_string()),
                ("accounts.google.com", "google-token".to_string()),
                ("www.amazon.com", "amazon-token".to_string()),
            ],
            vec!["graph.facebook.com", "www.amazon.com"],
        ),
        (
            "three providers, every one removed",
            vec![
                ("api.twitter.com", "twitter-token".to_string()),
                ("www.digits.com", "digits-token".to_string()),
                ("oidc.example.com", "oidc-token".to_string()),
            ],
            vec!["api.twitter.com", "www.digits.com", "oidc.example.com"],
        ),
        (
            "a login at the maximum modelled token length",
            vec![("api.twitter.com", at_max_token_length.clone())],
            vec!["api.twitter.com"],
        ),
        (
            "a login at the minimum modelled token length",
            vec![("www.amazon.com", "x".to_string())],
            vec!["www.amazon.com"],
        ),
        (
            "ten providers, the modelled LoginsMap maximum, half removed",
            ten_providers.clone(),
            PROVIDERS[..5].to_vec(),
        ),
    ];

    for (row, (case, linked, removed)) in cases.iter().enumerate() {
        // One pool per row, so the rows cannot interfere through a shared login index.
        let pool_id = emulator.pool(&format!("unlink-{row}"), true).await;
        let identity_id = emulator.identity(&pool_id, Some(logins_map(linked))).await;
        let all_providers: Vec<&str> = linked.iter().map(|(provider, _)| *provider).collect();
        let before = description(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            linked_providers(case, &before),
            sorted(&all_providers),
            "case {case:?}: expected the identity linked to {all_providers:?} before the \
             unlink, got {before}"
        );

        emulator
            .ok(
                OPERATION,
                &unlink_request(
                    Some(&identity_id),
                    Some(logins_map(linked)),
                    Some(json!(removed)),
                ),
            )
            .await;

        let kept: Vec<&str> = all_providers
            .iter()
            .filter(|provider| !removed.contains(provider))
            .copied()
            .collect();
        let after = description(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            linked_providers(case, &after),
            sorted(&kept),
            "case {case:?}: expected the identity left linked to {kept:?} after unlinking \
             {removed:?}, got {after}"
        );

        // An unlinked login is one the pool no longer resolves, so it names a NEW identity —
        // and that new identity is itself deduplicated, as every linked login is.
        for (provider, token) in linked
            .iter()
            .filter(|(provider, _)| removed.contains(provider))
        {
            let one_login = logins_map(&[(*provider, token.clone())]);
            let minted = emulator.identity(&pool_id, Some(one_login.clone())).await;
            assert_ne!(
                minted, identity_id,
                "case {case:?}: the unlinked login of {provider:?} must name a new identity, \
                 got {minted}"
            );
            let again = emulator.identity(&pool_id, Some(one_login)).await;
            assert_eq!(
                again, minted,
                "case {case:?}: the new identity of {provider:?} is itself deduplicated, got \
                 {again}"
            );
        }

        // A provider the call did not name is untouched: its login still resolves to the
        // same identity.
        for (provider, token) in linked
            .iter()
            .filter(|(provider, _)| !removed.contains(provider))
        {
            let resolved = emulator
                .identity(&pool_id, Some(logins_map(&[(*provider, token.clone())])))
                .await;
            assert_eq!(
                resolved, identity_id,
                "case {case:?}: the kept login of {provider:?} still resolves to \
                 {identity_id}, got {resolved}"
            );
        }

        let settled = description(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            linked_providers(case, &settled),
            sorted(&kept),
            "case {case:?}: reading a login back must not relink an unlinked provider, got \
             {settled}"
        );
    }
}

/// RULE: removing the last linked login leaves the identity STORED and reachable through no
/// login at all — "removing the last linked login will make this identity inaccessible",
/// which is an identity nothing resolves to, not a deleted one.
#[tokio::test]
async fn removing_the_last_linked_login_leaves_the_identity_stored_but_unreachable() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("last-login", true).await;

    // The rows differ in how many providers the identity was linked to: every one of them is
    // removed in a single call.
    let cases: Vec<(&str, Vec<Login>)> = vec![
        (
            "one linked provider",
            vec![("graph.facebook.com", "fb-token".to_string())],
        ),
        (
            "two linked providers",
            vec![
                ("accounts.google.com", "google-token".to_string()),
                ("www.amazon.com", "amazon-token".to_string()),
            ],
        ),
        (
            "three linked providers",
            vec![
                ("api.twitter.com", "twitter-token".to_string()),
                ("www.digits.com", "digits-token".to_string()),
                ("auth.example.net", "oidc-token".to_string()),
            ],
        ),
    ];

    for (case, linked) in &cases {
        let identity_id = emulator.identity(&pool_id, Some(logins_map(linked))).await;
        let providers: Vec<&str> = linked.iter().map(|(provider, _)| *provider).collect();

        emulator
            .ok(
                OPERATION,
                &unlink_request(
                    Some(&identity_id),
                    Some(logins_map(linked)),
                    Some(json!(providers)),
                ),
            )
            .await;

        // Still described by the pool — it is inaccessible, not deleted.
        let after = description(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            linked_providers(case, &after),
            Vec::<String>::new(),
            "case {case:?}: the identity is still stored, linked to nothing, got {after}"
        );

        for (provider, token) in linked {
            let minted = emulator
                .identity(&pool_id, Some(logins_map(&[(*provider, token.clone())])))
                .await;
            assert_ne!(
                minted, identity_id,
                "case {case:?}: no login reaches the unlinked identity any more, {provider:?} \
                 answered {minted}"
            );
        }
    }
}

/// RULE: a call that removes no link changes nothing a later caller sees — the identity's
/// record, the providers it is linked to and the instant it was last modified are all left
/// as they were — so unlinking is idempotent.
#[tokio::test]
async fn a_call_that_removes_no_link_changes_nothing() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("no-op", true).await;
    let linked: Vec<Login> = vec![
        ("graph.facebook.com", "fb-token".to_string()),
        ("accounts.google.com", "google-token".to_string()),
    ];

    // (case, the provider names the call asks to remove) — each a request the store has
    // nothing to act on.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("an empty LoginsToRemove", vec![]),
        (
            "a provider this identity is not linked to",
            vec!["www.digits.com"],
        ),
        (
            "several providers, none of them linked",
            vec!["www.digits.com", "oidc.example.com", "api.twitter.com"],
        ),
    ];

    for (case, removed) in &cases {
        // A fresh identity per row, reached through the same two providers: the logins carry
        // the row's own tokens so each identity is its own.
        let row_logins: Vec<Login> = linked
            .iter()
            .map(|(provider, token)| (*provider, format!("{token}-{case}")))
            .collect();
        let identity_id = emulator
            .identity(&pool_id, Some(logins_map(&row_logins)))
            .await;
        let before = description(&emulator, &pool_id, &identity_id).await;

        emulator
            .ok(
                OPERATION,
                &unlink_request(
                    Some(&identity_id),
                    Some(logins_map(&row_logins)),
                    Some(json!(removed)),
                ),
            )
            .await;

        let after = description(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            before, after,
            "case {case:?}: removing {removed:?} removes no link, so the identity's whole \
             description is unchanged"
        );
        for (provider, token) in &row_logins {
            let resolved = emulator
                .identity(&pool_id, Some(logins_map(&[(*provider, token.clone())])))
                .await;
            assert_eq!(
                resolved, identity_id,
                "case {case:?}: {provider:?} still resolves to {identity_id}, got {resolved}"
            );
        }
    }

    // Repeating a call that DID remove a link removes nothing more: the second answer is a
    // success and the identity is exactly as the first call left it.
    let identity_id = emulator.identity(&pool_id, Some(logins_map(&linked))).await;
    let request = unlink_request(
        Some(&identity_id),
        Some(logins_map(&linked)),
        Some(json!(["graph.facebook.com"])),
    );
    emulator.ok(OPERATION, &request).await;
    let once = description(&emulator, &pool_id, &identity_id).await;
    emulator.ok(OPERATION, &request).await;
    let twice = description(&emulator, &pool_id, &identity_id).await;
    assert_eq!(
        once, twice,
        "unlinking the same provider twice leaves the identity as the first call left it"
    );
    assert_eq!(
        linked_providers("repeated", &twice),
        vec!["accounts.google.com".to_string()],
        "the provider the call never named is still linked, got {twice}"
    );
}

/// RULE: a call the store cannot act on is refused with the modelled error for what is
/// wrong with it, and writes nothing: an identity this service does not hold is not found,
/// a presented login already linked to another identity is a conflict, a `Logins` entry the
/// pool does not accept is not authorized, and a value outside a MODELLED constraint is
/// refused ahead of the operation.
#[tokio::test]
async fn a_call_the_store_cannot_act_on_is_refused() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("refusals", true).await;
    let own_login: Vec<Login> = vec![("graph.facebook.com", "fb-token".to_string())];
    let other_login: Vec<Login> = vec![("accounts.google.com", "google-token".to_string())];
    let identity_id = emulator
        .identity(&pool_id, Some(logins_map(&own_login)))
        .await;
    let other_identity = emulator
        .identity(&pool_id, Some(logins_map(&other_login)))
        .await;
    assert_ne!(
        identity_id, other_identity,
        "two different federated logins are two identities"
    );

    // A user-pool provider name carries its issuer, and this pool configures no
    // `CognitoIdentityProviders`, so no user pool is a login provider of it.
    let foreign_user_pool = logins_map(&[(
        "cognito-idp.us-east-1.amazonaws.com/us-east-1_abc123",
        "id-token".to_string(),
    )]);
    // Eleven DISTINCT entries, one past the modelled `LoginsMap` maximum of ten.
    let too_many: Value = Value::Object(
        (0..11)
            .map(|index| (format!("idp-{index}.example.com"), json!("token")))
            .collect(),
    );
    let past_max_token = logins_map(&[("graph.facebook.com", "t".repeat(50_001))]);
    let empty_token = logins_map(&[("graph.facebook.com", String::new())]);
    // `IdentityId` is modelled `length 1..55, pattern ^[\w-]+:[0-9a-f-]+$`, so these two
    // sit on and one past its published maximum, both well-formed.
    let at_max_length_identity = format!("us-east-1:{}", "a".repeat(45));
    let past_max_length_identity = format!("us-east-1:{}", "a".repeat(46));
    let one_provider = json!(["graph.facebook.com"]);
    let past_max_length_provider = json!(["x".repeat(129)]);

    // (case, request, expected status, expected error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an identity nothing ever minted",
            unlink_request(
                Some(ABSENT_IDENTITY_ID),
                Some(logins_map(&own_login)),
                Some(one_provider.clone()),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed identity id at the modelled maximum length that nothing minted",
            unlink_request(
                Some(&at_max_length_identity),
                Some(logins_map(&own_login)),
                Some(one_provider.clone()),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a login already linked to another identity",
            unlink_request(
                Some(&identity_id),
                Some(logins_map(&other_login)),
                Some(one_provider.clone()),
            ),
            409,
            "ResourceConflictException",
        ),
        (
            "a Logins entry naming a user pool this pool does not configure",
            unlink_request(
                Some(&identity_id),
                Some(foreign_user_pool),
                Some(one_provider.clone()),
            ),
            403,
            "NotAuthorizedException",
        ),
        (
            "an identity id outside the modelled pattern",
            unlink_request(
                Some("not-an-identity"),
                Some(logins_map(&own_login)),
                Some(one_provider.clone()),
            ),
            400,
            "ValidationException",
        ),
        (
            "an identity id one character past the modelled maximum length",
            unlink_request(
                Some(&past_max_length_identity),
                Some(logins_map(&own_login)),
                Some(one_provider.clone()),
            ),
            400,
            "ValidationException",
        ),
        (
            "an absent IdentityId",
            unlink_request(
                None,
                Some(logins_map(&own_login)),
                Some(one_provider.clone()),
            ),
            400,
            "ValidationException",
        ),
        (
            "an absent Logins",
            unlink_request(Some(&identity_id), None, Some(one_provider.clone())),
            400,
            "ValidationException",
        ),
        (
            "an absent LoginsToRemove",
            unlink_request(Some(&identity_id), Some(logins_map(&own_login)), None),
            400,
            "ValidationException",
        ),
        (
            "one Logins entry past the modelled maximum of ten",
            unlink_request(Some(&identity_id), Some(too_many), Some(one_provider.clone())),
            400,
            "ValidationException",
        ),
        (
            "a token one character past the modelled maximum length",
            unlink_request(
                Some(&identity_id),
                Some(past_max_token),
                Some(one_provider.clone()),
            ),
            400,
            "ValidationException",
        ),
        (
            "an empty token",
            unlink_request(
                Some(&identity_id),
                Some(empty_token),
                Some(one_provider.clone()),
            ),
            400,
            "ValidationException",
        ),
        (
            "an empty provider name in LoginsToRemove",
            unlink_request(
                Some(&identity_id),
                Some(logins_map(&own_login)),
                Some(json!([""])),
            ),
            400,
            "ValidationException",
        ),
        (
            "a provider name in LoginsToRemove past the modelled maximum length",
            unlink_request(
                Some(&identity_id),
                Some(logins_map(&own_login)),
                Some(past_max_length_provider),
            ),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in &cases {
        let (status, body) = emulator.call(OPERATION, request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Every refusal above wrote nothing: both identities are still linked to their own
    // login, and that login still resolves to them.
    for (identity, login) in [(&identity_id, &own_login), (&other_identity, &other_login)] {
        let described = description(&emulator, &pool_id, identity).await;
        let providers: Vec<&str> = login.iter().map(|(provider, _)| *provider).collect();
        assert_eq!(
            linked_providers("after the refusals", &described),
            sorted(&providers),
            "a refused unlink removes no link, got {described}"
        );
        let resolved = emulator.identity(&pool_id, Some(logins_map(login))).await;
        assert_eq!(
            resolved, *identity,
            "a refused unlink leaves the login resolving to {identity}, got {resolved}"
        );
    }
}
