// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UnlinkDeveloperIdentity`: one developer-identity mapping of a
//! stored pool is FORGOTTEN, so the identifier it named resolves nothing in that pool any
//! more.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * the `(developer provider, developer user identifier)` login the pool had indexed
//!   against the named identity is dropped and nothing else is: a lookup of the unlinked
//!   identifier no longer resolves it, the identifiers the call did not name still do, and
//!   presenting the unlinked identifier again is seen as a NEW identity;
//! * the identity record survives, keeping the developer provider only while it still has
//!   an identifier registered under it, and keeping every federated login it had;
//! * a call naming a mapping this pool does not hold is a 403 `NotAuthorizedException`,
//!   one naming a mapping held against another identity is a 409
//!   `ResourceConflictException`, one naming a pool or an identity this service does not
//!   hold is a 404 `ResourceNotFoundException`, one naming an identity of ANOTHER pool is a
//!   403 `NotAuthorizedException`, and a value outside a constraint the MODEL states is
//!   refused ahead of the operation with a 400 `ValidationException` — and every refusal
//!   writes nothing;
//! * the call is therefore not idempotent in its answer but is in its effect: repeating it
//!   is refused as a mapping this pool no longer holds and leaves the pool exactly as the
//!   first call did.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// The operation under test, spelled once.
const OPERATION: &str = "UnlinkDeveloperIdentity";

/// A well-formed `IdentityId` that nothing ever minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:55555555-5555-5555-5555-555555555555";

/// The message Amazon Cognito Identity answers a request naming a developer-identity
/// mapping the pool does not hold with (`crate::state::DEVELOPER_IDENTIFIER_FORBIDDEN_MESSAGE`)
/// — asserted here against the literal so the wire-visible wording is pinned by the test,
/// not by the code under test. One fixed sentence: it names neither the identifier the
/// request carried nor any identity.
const NO_ACCESS_MESSAGE: &str = "You do not have access to this identity";

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

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

/// The `UnlinkDeveloperIdentity` request for one case. Every member is sent only when the
/// case carries it, so a case with `None` exercises that member's absence (all four are
/// modelled **required**).
fn unlink_request(
    pool_id: Option<&str>,
    identity_id: Option<&str>,
    developer_provider: Option<&str>,
    identifier: Option<&str>,
) -> Value {
    let mut request = Map::new();
    if let Some(pool_id) = pool_id {
        request.insert("IdentityPoolId".to_string(), json!(pool_id));
    }
    if let Some(identity_id) = identity_id {
        request.insert("IdentityId".to_string(), json!(identity_id));
    }
    if let Some(developer_provider) = developer_provider {
        request.insert(
            "DeveloperProviderName".to_string(),
            json!(developer_provider),
        );
    }
    if let Some(identifier) = identifier {
        request.insert("DeveloperUserIdentifier".to_string(), json!(identifier));
    }
    Value::Object(request)
}

/// Every developer user identifier `LookupDeveloperIdentity` reports for one identity, in
/// the order it reports them — the read-back path for what this operation wrote.
async fn identifiers_of(emulator: &Emulator, pool_id: &str, identity_id: &str) -> Vec<String> {
    let looked_up = emulator
        .ok(
            "LookupDeveloperIdentity",
            &json!({
                "IdentityPoolId": pool_id,
                "IdentityId": identity_id,
                "MaxResults": 60,
            }),
        )
        .await;
    looked_up["DeveloperUserIdentifierList"]
        .as_array()
        .unwrap_or_else(|| panic!("LookupDeveloperIdentity answers a list: {looked_up}"))
        .iter()
        .map(|identifier| {
            identifier
                .as_str()
                .unwrap_or_else(|| panic!("an identifier is a string: {looked_up}"))
                .to_string()
        })
        .collect()
}

/// The description `ListIdentities` reports for one identity of a pool.
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

/// `values` sorted, which is the order the store reports a pool's logins in.
fn sorted(values: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    sorted.sort();
    sorted
}

/// RULE: unlinking a developer user identifier drops exactly that mapping of the pool — the
/// identifier resolves nothing afterwards and is seen as a new identity next time it is
/// presented, while every identifier the call did not name still resolves to the same
/// identity, which keeps the developer provider only while one is left.
#[tokio::test]
async fn an_unlinked_developer_identifier_is_forgotten_and_seen_as_new_next_time() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: HOW MANY identifiers the
    // identity is registered under and how many of them one call unlinks (one, several,
    // every one), the SPELLING and SIZE of the identifier (`DeveloperUserIdentifier` is
    // modelled `length 1..1024` with no pattern, so the rows include each published bound),
    // and the spelling and size of the pool's own `DeveloperProviderName` (modelled
    // `length 1..128, pattern ^[\w._-]+$`), one row sitting on its published maximum
    // (`smithy_query(query="operation",operation="UnlinkDeveloperIdentity")`).
    let at_max_identifier = "x".repeat(1024);
    let at_max_provider = "p".repeat(128);
    // (case, the pool's DeveloperProviderName, every identifier registered, the ones unlinked)
    let cases: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        (
            "one identifier, the only one unlinked",
            "login.example.app",
            vec!["user-1"],
            vec!["user-1"],
        ),
        (
            "two identifiers, one unlinked",
            "login.example.app",
            vec!["user-1", "user-2"],
            vec!["user-2"],
        ),
        (
            "five identifiers, two unlinked",
            "backend_1.example-app",
            vec!["m-user", "a-user", "z-user", "c-user", "b-user"],
            vec!["a-user", "z-user"],
        ),
        (
            "three identifiers, every one unlinked",
            "backend.example.app",
            vec!["user-1", "user-2", "user-3"],
            vec!["user-1", "user-2", "user-3"],
        ),
        (
            "an identifier at the minimum modelled length",
            "dev_provider",
            vec!["u", "user-2"],
            vec!["u"],
        ),
        (
            "an identifier at the maximum modelled length",
            "dev-provider",
            vec![at_max_identifier.as_str(), "user-2"],
            vec![at_max_identifier.as_str()],
        ),
        (
            "identifiers with spaces, punctuation and non-ASCII",
            "login.example.app",
            vec!["Jane Doe #7 (eu)", "person@example.com", "ユーザー-42"],
            vec!["Jane Doe #7 (eu)", "ユーザー-42"],
        ),
        (
            "a developer provider name at the maximum modelled length",
            at_max_provider.as_str(),
            vec!["user-1", "user-2"],
            vec!["user-1"],
        ),
    ];

    for (row, (case, developer_provider, registered, unlinked)) in cases.iter().enumerate() {
        // One pool per row, so the rows cannot interfere through a shared login index.
        let pool_id =
            pool_with_developer_provider(&emulator, &format!("unlink-dev-{row}"), Some(developer_provider))
                .await;
        let identity_id = register(&emulator, &pool_id, developer_provider, registered[0], None).await;
        for identifier in &registered[1..] {
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
                "case {case:?}: {identifier:?} must register under the identity the call named, \
                 got {linked}"
            );
        }
        assert_eq!(
            identifiers_of(&emulator, &pool_id, &identity_id).await,
            sorted(registered),
            "case {case:?}: expected every identifier registered before the unlink"
        );

        for identifier in unlinked {
            emulator
                .ok(
                    OPERATION,
                    &unlink_request(
                        Some(&pool_id),
                        Some(&identity_id),
                        Some(developer_provider),
                        Some(identifier),
                    ),
                )
                .await;
        }

        let kept: Vec<&str> = registered
            .iter()
            .filter(|identifier| !unlinked.contains(identifier))
            .copied()
            .collect();
        assert_eq!(
            identifiers_of(&emulator, &pool_id, &identity_id).await,
            sorted(&kept),
            "case {case:?}: expected only {kept:?} left registered after unlinking {unlinked:?}"
        );

        // The identity is still stored, carrying the developer provider only while an
        // identifier of it is left.
        let described = description(&emulator, &pool_id, &identity_id).await;
        let expected_providers = if kept.is_empty() {
            Vec::new()
        } else {
            vec![(*developer_provider).to_string()]
        };
        assert_eq!(
            linked_providers(case, &described),
            expected_providers,
            "case {case:?}: expected the identity linked to {expected_providers:?}, got {described}"
        );

        // An unlinked identifier resolves nothing: a lookup of it is refused, and presenting
        // it again is seen as a NEW identity.
        for identifier in unlinked {
            let (status, body) = emulator
                .call(
                    "LookupDeveloperIdentity",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "DeveloperUserIdentifier": identifier,
                    }),
                )
                .await;
            assert_eq!(
                status, 403,
                "case {case:?}: the unlinked identifier {identifier:?} resolves nothing, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_message(case, &body),
                NO_ACCESS_MESSAGE,
                "case {case:?}: expected the fixed no-access message, got {body}"
            );
            let minted = register(&emulator, &pool_id, developer_provider, identifier, None).await;
            assert_ne!(
                minted, identity_id,
                "case {case:?}: the unlinked identifier {identifier:?} must be seen as a new \
                 identity, got {minted}"
            );
        }

        // An identifier the call never named still resolves to the identity it was
        // registered under.
        for identifier in &kept {
            let looked_up = emulator
                .ok(
                    "LookupDeveloperIdentity",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "DeveloperUserIdentifier": identifier,
                    }),
                )
                .await;
            assert_eq!(
                looked_up["IdentityId"],
                json!(identity_id),
                "case {case:?}: the kept identifier {identifier:?} still resolves to \
                 {identity_id}, got {looked_up}"
            );
        }
    }
}

/// RULE: the mapping is the only thing the call forgets — an identity reached through
/// federated logins as well keeps every one of them, so those logins still resolve to it
/// after its developer user identifier is gone.
#[tokio::test]
async fn unlinking_a_developer_identifier_leaves_the_federated_logins_linked() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id =
        pool_with_developer_provider(&emulator, "federated-too", Some(developer_provider)).await;

    // The rows differ in how many federated providers the identity is also reached through.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "one federated login beside the developer one",
            vec![("graph.facebook.com", "fb-token")],
        ),
        (
            "two federated logins beside the developer one",
            vec![
                ("accounts.google.com", "google-token"),
                ("www.amazon.com", "amazon-token"),
            ],
        ),
    ];

    for (row, (case, federated)) in cases.iter().enumerate() {
        let logins: Map<String, Value> = federated
            .iter()
            .map(|(provider, token)| ((*provider).to_string(), json!(format!("{token}-{row}"))))
            .collect();
        let identity_id = emulator
            .identity(&pool_id, Some(Value::Object(logins.clone())))
            .await;
        let identifier = format!("user-{row}");
        let registered = register(
            &emulator,
            &pool_id,
            developer_provider,
            &identifier,
            Some(&identity_id),
        )
        .await;
        assert_eq!(
            registered, identity_id,
            "case {case:?}: the identifier registers under the identity the call named"
        );

        emulator
            .ok(
                OPERATION,
                &unlink_request(
                    Some(&pool_id),
                    Some(&identity_id),
                    Some(developer_provider),
                    Some(&identifier),
                ),
            )
            .await;

        let providers: Vec<&str> = federated.iter().map(|(provider, _)| *provider).collect();
        let described = description(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            linked_providers(case, &described),
            sorted(&providers),
            "case {case:?}: expected the federated providers {providers:?} left linked, got \
             {described}"
        );
        let resolved = emulator
            .identity(&pool_id, Some(Value::Object(logins)))
            .await;
        assert_eq!(
            resolved, identity_id,
            "case {case:?}: the federated logins still resolve to {identity_id}, got {resolved}"
        );
    }
}

/// RULE: a call the pool's mapping cannot act on is refused with the modelled error for
/// what is wrong with it, and writes nothing — a mapping this pool does not hold is
/// forbidden, one held against another identity is a conflict, a pool or identity this
/// service does not hold is not found, an identity of another pool is forbidden, and a
/// value outside a MODELLED constraint is refused ahead of the operation.
#[tokio::test]
async fn a_call_the_mapping_cannot_act_on_is_refused() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "refusals", Some(developer_provider)).await;
    let other_pool = pool_with_developer_provider(&emulator, "other", Some(developer_provider)).await;
    // A pool that names no developer provider at all, so it can hold no mapping.
    let no_developer_provider =
        pool_with_developer_provider(&emulator, "no-developer-provider", None).await;
    let identity_id = register(&emulator, &pool_id, developer_provider, "user-1", None).await;
    let other_identity = register(&emulator, &pool_id, developer_provider, "user-2", None).await;
    let foreign_identity = register(&emulator, &other_pool, developer_provider, "user-1", None).await;
    let bare_identity = emulator.identity(&no_developer_provider, None).await;
    assert_ne!(
        identity_id, other_identity,
        "two identifiers registered on their own are two identities"
    );

    // `DeveloperProviderName` is modelled `length 1..128, pattern ^[\w._-]+$` and
    // `DeveloperUserIdentifier` `length 1..1024`, so these sit one past each published
    // maximum, and one outside the provider pattern.
    let past_max_provider = "p".repeat(129);
    let past_max_identifier = "x".repeat(1025);
    let past_max_identity = format!("us-east-1:{}", "a".repeat(46));

    // (case, request, expected status, expected error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a pool this service does not hold",
            unlink_request(
                Some(ABSENT_POOL_ID),
                Some(&identity_id),
                Some(developer_provider),
                Some("user-1"),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identity nothing ever minted",
            unlink_request(
                Some(&pool_id),
                Some(ABSENT_IDENTITY_ID),
                Some(developer_provider),
                Some("user-1"),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identifier that was never registered",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some(developer_provider),
                Some("never-registered"),
            ),
            403,
            "NotAuthorizedException",
        ),
        (
            "a developer provider name that is not this pool's",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some("another.example.app"),
                Some("user-1"),
            ),
            403,
            "NotAuthorizedException",
        ),
        (
            "a pool that names no developer provider at all",
            unlink_request(
                Some(&no_developer_provider),
                Some(&bare_identity),
                Some(developer_provider),
                Some("user-1"),
            ),
            403,
            "NotAuthorizedException",
        ),
        (
            "an identifier registered under ANOTHER identity of the same pool",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some(developer_provider),
                Some("user-2"),
            ),
            409,
            "ResourceConflictException",
        ),
        (
            "an identity of ANOTHER pool",
            unlink_request(
                Some(&pool_id),
                Some(&foreign_identity),
                Some(developer_provider),
                Some("user-1"),
            ),
            403,
            "NotAuthorizedException",
        ),
        (
            "an absent IdentityPoolId",
            unlink_request(
                None,
                Some(&identity_id),
                Some(developer_provider),
                Some("user-1"),
            ),
            400,
            "ValidationException",
        ),
        (
            "an absent IdentityId",
            unlink_request(
                Some(&pool_id),
                None,
                Some(developer_provider),
                Some("user-1"),
            ),
            400,
            "ValidationException",
        ),
        (
            "an absent DeveloperProviderName",
            unlink_request(Some(&pool_id), Some(&identity_id), None, Some("user-1")),
            400,
            "ValidationException",
        ),
        (
            "an absent DeveloperUserIdentifier",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some(developer_provider),
                None,
            ),
            400,
            "ValidationException",
        ),
        (
            "an identity id outside the modelled pattern",
            unlink_request(
                Some(&pool_id),
                Some("not-an-identity"),
                Some(developer_provider),
                Some("user-1"),
            ),
            400,
            "ValidationException",
        ),
        (
            "an identity id one character past the modelled maximum length",
            unlink_request(
                Some(&pool_id),
                Some(&past_max_identity),
                Some(developer_provider),
                Some("user-1"),
            ),
            400,
            "ValidationException",
        ),
        (
            "a developer provider name outside the modelled pattern",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some("spaces are not allowed"),
                Some("user-1"),
            ),
            400,
            "ValidationException",
        ),
        (
            "a developer provider name one character past the modelled maximum length",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some(&past_max_provider),
                Some("user-1"),
            ),
            400,
            "ValidationException",
        ),
        (
            "an empty developer provider name",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some(""),
                Some("user-1"),
            ),
            400,
            "ValidationException",
        ),
        (
            "an identifier one character past the modelled maximum length",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some(developer_provider),
                Some(&past_max_identifier),
            ),
            400,
            "ValidationException",
        ),
        (
            "an empty identifier",
            unlink_request(
                Some(&pool_id),
                Some(&identity_id),
                Some(developer_provider),
                Some(""),
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

    // Every refusal above wrote nothing: both mappings of the pool are exactly as they
    // were, and each identity still answers its own identifier.
    for (identity, identifier) in [(&identity_id, "user-1"), (&other_identity, "user-2")] {
        assert_eq!(
            identifiers_of(&emulator, &pool_id, identity).await,
            vec![identifier.to_string()],
            "a refused unlink forgets no mapping, so {identity} still answers {identifier:?}"
        );
        let looked_up = emulator
            .ok(
                "LookupDeveloperIdentity",
                &json!({
                    "IdentityPoolId": pool_id,
                    "DeveloperUserIdentifier": identifier,
                }),
            )
            .await;
        assert_eq!(
            looked_up["IdentityId"],
            json!(identity),
            "a refused unlink leaves {identifier:?} resolving to {identity}, got {looked_up}"
        );
    }
}

/// RULE: the EFFECT of the call is idempotent — the mapping the first call forgot is gone,
/// so repeating the same request is refused as a mapping this pool no longer holds and
/// leaves the pool exactly as the first call left it.
#[tokio::test]
async fn repeating_the_call_forgets_nothing_more() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "repeat", Some(developer_provider)).await;
    let identity_id = register(&emulator, &pool_id, developer_provider, "user-1", None).await;
    register(
        &emulator,
        &pool_id,
        developer_provider,
        "user-2",
        Some(&identity_id),
    )
    .await;

    let request = unlink_request(
        Some(&pool_id),
        Some(&identity_id),
        Some(developer_provider),
        Some("user-1"),
    );
    emulator.ok(OPERATION, &request).await;
    let once = description(&emulator, &pool_id, &identity_id).await;
    let identifiers_once = identifiers_of(&emulator, &pool_id, &identity_id).await;

    let (status, body) = emulator.call(OPERATION, &request).await;
    assert_eq!(
        status, 403,
        "the mapping the first call forgot is gone, so the second is refused, got {status} with \
         {body}"
    );
    assert!(
        error_code(&body).contains("NotAuthorizedException"),
        "expected NotAuthorizedException on the repeated call, got {body}"
    );
    assert_eq!(
        error_message("the repeated call", &body),
        NO_ACCESS_MESSAGE,
        "expected the fixed no-access message on the repeated call, got {body}"
    );

    let twice = description(&emulator, &pool_id, &identity_id).await;
    assert_eq!(
        once, twice,
        "the refused repeat leaves the identity as the first call left it"
    );
    assert_eq!(
        identifiers_of(&emulator, &pool_id, &identity_id).await,
        identifiers_once,
        "the refused repeat forgets no further mapping"
    );
    assert_eq!(
        identifiers_once,
        vec!["user-2".to_string()],
        "the identifier the call never named is still registered"
    );
}
