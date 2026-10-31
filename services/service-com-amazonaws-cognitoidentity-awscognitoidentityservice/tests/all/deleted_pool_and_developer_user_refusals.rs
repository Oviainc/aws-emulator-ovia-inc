// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH resource an identity-scoped refusal names, and for the two
//! answers a developer-identity call gives for a developer user the pool has not registered.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * an identity is an identity OF a pool, so every operation that resolves an identity
//!   resolves the pool it belongs to too — the record of an identity outlives the delete of
//!   its pool — and names in its `ResourceNotFoundException` the resource that is actually
//!   missing: the POOL once the pool has gone, the IDENTITY while the pool stands and the
//!   identity does not;
//! * removing a developer-identity mapping the pool does not hold is the state the call asks
//!   for already holding, so `UnlinkDeveloperIdentity` is SERVED (HTTP 200, empty body) and
//!   writes nothing — while a mapping held against another identity is still the modelled
//!   `ResourceConflictException`;
//! * a merge naming a developer user identifier the pool has registered against no identity
//!   reports that USER missing, with the identifier the request carried; two users reached
//!   through one public provider as two different users cannot become one user at all, and
//!   that refusal carries one fixed sentence naming neither user nor the provider.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::developer_identity_tokens::pool_with_developer_provider;
use crate::identity_pool_flow::{error_code, Emulator};
use crate::merge_developer_identities::user;

/// The pool's `DeveloperProviderName` throughout: "the 'domain' by which Cognito will refer
/// to your users", modelled `len 1..128, pattern ^[\w._-]+$`.
const DEVELOPER_PROVIDER: &str = "login.example.app";

/// A public provider whose logins an identity of a pool with no stated trust is reached
/// through, as `GetId$Logins` publishes its name.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// A second public provider, so the shared-provider rule is driven on more than one.
const OTHER_PUBLIC_PROVIDER: &str = "accounts.google.com";

/// A well-formed `IdentityId` that nothing ever minted. `IdentityId` is modelled
/// `len 1..55, pattern ^[\w-]+:[0-9a-f-]+$`.
const ABSENT_IDENTITY_ID: &str = "us-east-1:99999999-9999-9999-9999-999999999999";

/// The message Amazon Cognito Identity answers a pair of users that cannot become one user
/// with (`crate::state::CANNOT_MERGE_IDENTITIES_MESSAGE`) — asserted here against the
/// literal so the wire-visible wording is pinned by the test, not by the code under test.
const CANNOT_MERGE_MESSAGE: &str = "Cannot merge these identities.";

/// The two published bounds of `DeveloperUserIdentifier` (modelled `len 1..1024`,
/// `smithy_query(query="shape",shape="DeveloperUserIdentifier")`), as identifiers no call
/// of this suite ever registers.
const SHORTEST_IDENTIFIER: &str = "x";

/// One `Logins` entry a row carries: the provider name and the value presented under it.
type Login<'a> = (&'a str, &'a str);

/// One row of the shared-public-provider table: the case name and the federated logins each
/// of the two users is also reached through.
type SharedProviderCase<'a> = (&'a str, Vec<Login<'a>>, Vec<Login<'a>>);

/// Amazon Cognito Identity's not-found wording for each resource: fixed text around the one
/// variable part, the value the request or the stored record carried.
fn identity_not_found(identity_id: &str) -> String {
    format!("Identity '{identity_id}' not found.")
}

fn identity_pool_not_found(identity_pool_id: &str) -> String {
    format!("IdentityPool '{identity_pool_id}' not found.")
}

fn developer_user_not_found(identifier: &str) -> String {
    format!("User '{identifier}' not found.")
}

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// Assert one call answers HTTP 400 `ResourceNotFoundException` naming `expected`.
async fn assert_not_found(emulator: &Emulator, case: &str, operation: &str, request: &Value, expected: &str) {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException from {operation}, got {body}"
    );
    assert_eq!(
        error_message(case, &body),
        expected,
        "case {case:?}: expected {expected:?} from {operation}, got {body}"
    );
}

/// The four requests that reach an identity by its id alone, for `identity_id`: the
/// operations whose `IdentityId` is the only resource the request names
/// (`smithy_query(query="digest")`).
fn identity_scoped_requests(identity_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeIdentity", json!({"IdentityId": identity_id})),
        (
            "GetCredentialsForIdentity",
            json!({"IdentityId": identity_id}),
        ),
        ("GetOpenIdToken", json!({"IdentityId": identity_id})),
        (
            "UnlinkIdentity",
            json!({
                "IdentityId": identity_id,
                "Logins": {PUBLIC_PROVIDER: "an-access-token"},
                "LoginsToRemove": [PUBLIC_PROVIDER],
            }),
        ),
    ]
}

/// RULE: an identity-scoped call resolves the identity and then the pool that identity
/// belongs to, so one made after the pool was deleted reports the POOL as the missing
/// resource — whatever the identity was reached through — while one naming an identity the
/// service does not hold reports the identity.
#[tokio::test]
async fn an_identity_whose_pool_was_deleted_reports_the_missing_pool() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: HOW the identity was minted, so
    // the resource named is decided by the pool being gone and not by the identity's logins.
    // (case, the `Logins` the identity was minted with)
    let cases: Vec<(&str, Option<Value>)> = vec![
        ("a guest identity minted with no login at all", None),
        (
            "an identity minted with a public-provider login",
            Some(json!({PUBLIC_PROVIDER: "a-facebook-access-token"})),
        ),
        (
            "an identity minted with two public-provider logins",
            Some(json!({
                PUBLIC_PROVIDER: "a-facebook-access-token",
                OTHER_PUBLIC_PROVIDER: "a-google-id",
            })),
        ),
    ];

    for (row, (case, logins)) in cases.into_iter().enumerate() {
        let pool_id = emulator.pool(&format!("doomed-{row}"), true).await;
        let identity_id = emulator.identity(&pool_id, logins).await;

        // The identity is readable while its pool stands: the rows below change only that
        // the pool goes.
        let described = emulator
            .ok("DescribeIdentity", &json!({"IdentityId": &identity_id}))
            .await;
        assert_eq!(
            described["IdentityId"],
            json!(identity_id),
            "case {case:?}: the identity must be readable before the delete, got {described}"
        );

        emulator
            .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &pool_id}))
            .await;

        for (operation, request) in identity_scoped_requests(&identity_id) {
            assert_not_found(
                &emulator,
                case,
                operation,
                &request,
                &identity_pool_not_found(&pool_id),
            )
            .await;
        }

        // The contrast: with the pool gone too, an identity id this service holds no record
        // of is still the identity that is reported missing — the rule names the resource
        // that is actually absent, it does not always name the pool.
        for (operation, request) in identity_scoped_requests(ABSENT_IDENTITY_ID) {
            assert_not_found(
                &emulator,
                case,
                operation,
                &request,
                &identity_not_found(ABSENT_IDENTITY_ID),
            )
            .await;
        }
    }
}

/// RULE: `UnlinkDeveloperIdentity` is asked for a state — the identifier resolving nothing in
/// the pool — so a call naming a mapping the pool does not hold is SERVED with the empty
/// output and writes nothing, however the mapping came not to be there.
#[tokio::test]
async fn unlinking_a_developer_user_the_pool_does_not_hold_is_served() {
    let emulator = Emulator::new();
    let pool_id =
        pool_with_developer_provider(&emulator, "served-unlink", Some(DEVELOPER_PROVIDER)).await;
    // One identity keeps a mapping no row names, so every row can be read against it.
    let keeper = user(
        &emulator,
        &pool_id,
        DEVELOPER_PROVIDER,
        &["keeper", "detached-by-an-earlier-call"],
        &[],
    )
    .await;
    // A second identity whose only mapping is taken off it, so a later call reaches an
    // identity no login resolves to at all.
    let emptied = user(&emulator, &pool_id, DEVELOPER_PROVIDER, &["solo"], &[]).await;

    let unlink = |identity_id: &str, identifier: &str| {
        json!({
            "IdentityPoolId": &pool_id,
            "IdentityId": identity_id,
            "DeveloperProviderName": DEVELOPER_PROVIDER,
            "DeveloperUserIdentifier": identifier,
        })
    };

    // The two mappings these rows rely on being gone are taken off first, through this very
    // operation: that call is the one that WRITES, and the identity it changed is read back
    // here so the rows below can be shown to change nothing further.
    for (identity_id, identifier) in [
        (&keeper, "detached-by-an-earlier-call"),
        (&emptied, "solo"),
    ] {
        emulator
            .ok("UnlinkDeveloperIdentity", &unlink(identity_id, identifier))
            .await;
    }
    // The removal above really happened: the identifier it named resolves nothing now, which
    // is what makes the rows below calls about a mapping the pool no longer holds.
    let (status, body) = emulator
        .call(
            "LookupDeveloperIdentity",
            &json!({
                "IdentityPoolId": &pool_id,
                "DeveloperUserIdentifier": "detached-by-an-earlier-call",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "the detached identifier must resolve nothing after the unlink, got {status} with {body}"
    );
    let described_before = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": &keeper}))
        .await;

    // The rows differ along what the rule generalises over: WHY the pool holds no such
    // mapping, and the length of the identifier named — `DeveloperUserIdentifier` is
    // modelled `len 1..1024`, so two rows sit on its published bounds.
    let longest_identifier = "u".repeat(1024);
    // (case, the identity named, the identifier named)
    let cases: Vec<(&str, &String, &str)> = vec![
        (
            "an identifier this pool never registered",
            &keeper,
            "never-registered",
        ),
        (
            "an identifier an earlier call of this operation detached",
            &keeper,
            "detached-by-an-earlier-call",
        ),
        (
            "an identity no login resolves to any more",
            &emptied,
            "solo",
        ),
        (
            "an unregistered identifier at the shortest modelled length",
            &keeper,
            SHORTEST_IDENTIFIER,
        ),
        (
            "an unregistered identifier at the modelled maximum length",
            &keeper,
            longest_identifier.as_str(),
        ),
    ];

    for (case, identity_id, identifier) in cases {
        let (status, body) = emulator
            .call("UnlinkDeveloperIdentity", &unlink(identity_id, identifier))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for a mapping the pool does not hold, got {status} \
             with {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: expected this operation's empty output, got {body}"
        );

        // It wrote nothing: the mapping no row named still resolves to the identity it was
        // registered under.
        let looked_up = emulator
            .ok(
                "LookupDeveloperIdentity",
                &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": "keeper"}),
            )
            .await;
        assert_eq!(
            looked_up["IdentityId"],
            json!(keeper),
            "case {case:?}: the mapping the call did not name still resolves to {keeper}, got \
             {looked_up}"
        );
        // Nor did it touch the identity record: a served removal that removed nothing writes
        // nothing at all, so every member a read reports — its providers and the instant it
        // last changed — is what it was before the call.
        let described_after = emulator
            .ok("DescribeIdentity", &json!({"IdentityId": &keeper}))
            .await;
        assert_eq!(
            described_after, described_before,
            "case {case:?}: expected the identity record to be untouched \
             ({described_before}), got {described_after}"
        );
    }

    // The contrast: a mapping the pool DOES hold, against another identity, is still refused
    // — the served answer above is about there being nothing left to forget, not about every
    // identifier a request may name.
    let other = user(&emulator, &pool_id, DEVELOPER_PROVIDER, &["other-user"], &[]).await;
    assert_ne!(
        other, keeper,
        "an identifier registered on its own is its own identity"
    );
    let (status, body) = emulator
        .call("UnlinkDeveloperIdentity", &unlink(&keeper, "other-user"))
        .await;
    assert_eq!(
        status, 400,
        "a mapping held against another identity is refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ResourceConflictException"),
        "a mapping held against another identity expects ResourceConflictException, got {body}"
    );
}

/// RULE: a merge names two developer users of the pool, and an identifier the pool has
/// registered against no identity names a USER this service does not hold — reported as the
/// missing resource it is, with the identifier the request carried.
#[tokio::test]
async fn a_merge_naming_an_unregistered_developer_user_reports_that_user() {
    let emulator = Emulator::new();
    let pool_id =
        pool_with_developer_provider(&emulator, "merge-missing-user", Some(DEVELOPER_PROVIDER))
            .await;
    let source = user(&emulator, &pool_id, DEVELOPER_PROVIDER, &["source-user"], &[]).await;
    let destination = user(
        &emulator,
        &pool_id,
        DEVELOPER_PROVIDER,
        &["destination-user"],
        &[],
    )
    .await;

    // The rows differ along what the rule generalises over: WHICH side of the merge names a
    // user the pool has not registered, and the length of that identifier — two rows sit on
    // the published bounds of `DeveloperUserIdentifier` (`len 1..1024`).
    let longest_identifier = "u".repeat(1024);
    // (case, SourceUserIdentifier sent, DestinationUserIdentifier sent, the user named back)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a source identifier the pool never registered",
            "never-registered",
            "destination-user",
            "never-registered",
        ),
        (
            "a destination identifier the pool never registered",
            "source-user",
            "no-such-destination",
            "no-such-destination",
        ),
        (
            "neither identifier registered, so the source is the first to resolve",
            "unregistered-source",
            "unregistered-destination",
            "unregistered-source",
        ),
        (
            "an unregistered identifier at the shortest modelled length",
            SHORTEST_IDENTIFIER,
            "destination-user",
            SHORTEST_IDENTIFIER,
        ),
        (
            "an unregistered identifier at the modelled maximum length",
            "source-user",
            longest_identifier.as_str(),
            longest_identifier.as_str(),
        ),
    ];

    for (case, source_sent, destination_sent, named) in cases {
        let request = json!({
            "IdentityPoolId": &pool_id,
            "DeveloperProviderName": DEVELOPER_PROVIDER,
            "SourceUserIdentifier": source_sent,
            "DestinationUserIdentifier": destination_sent,
        });
        assert_not_found(
            &emulator,
            case,
            "MergeDeveloperIdentities",
            &request,
            &developer_user_not_found(named),
        )
        .await;

        // The refusal wrote nothing: the two registered users are still two users.
        for (identity_id, identifier) in [
            (&source, "source-user"),
            (&destination, "destination-user"),
        ] {
            let looked_up = emulator
                .ok(
                    "LookupDeveloperIdentity",
                    &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": identifier}),
                )
                .await;
            assert_eq!(
                looked_up["IdentityId"],
                json!(identity_id),
                "case {case:?}: a refused merge moves no mapping, so {identifier:?} still \
                 resolves to {identity_id}, got {looked_up}"
            );
        }
    }
}

/// RULE: two developer users reached through ONE public provider as two different users
/// cannot become one user — "If the users to be merged are associated with the same public
/// provider, but as two different users, an exception will be thrown" — and the refusal is
/// one fixed sentence, naming neither user nor the provider they share.
#[tokio::test]
async fn a_merge_of_two_users_of_one_public_provider_cannot_be_served() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: WHICH public provider the two
    // users share, and how many providers each carries beside it — the refusal is the same
    // sentence either way.
    // (case, the source user's federated logins, the destination user's)
    let cases: Vec<SharedProviderCase<'_>> = vec![
        (
            "two users of one public provider",
            vec![(PUBLIC_PROVIDER, "facebook-source")],
            vec![(PUBLIC_PROVIDER, "facebook-destination")],
        ),
        (
            "two users of another public provider",
            vec![(OTHER_PUBLIC_PROVIDER, "google-source")],
            vec![(OTHER_PUBLIC_PROVIDER, "google-destination")],
        ),
        (
            "one shared provider among providers that are not shared",
            vec![
                (PUBLIC_PROVIDER, "facebook-source"),
                (OTHER_PUBLIC_PROVIDER, "google-source"),
            ],
            vec![(OTHER_PUBLIC_PROVIDER, "google-destination")],
        ),
    ];

    for (row, (case, source_federated, destination_federated)) in cases.into_iter().enumerate() {
        let pool_id = pool_with_developer_provider(
            &emulator,
            &format!("merge-conflict-{row}"),
            Some(DEVELOPER_PROVIDER),
        )
        .await;
        let source = user(
            &emulator,
            &pool_id,
            DEVELOPER_PROVIDER,
            &["source-user"],
            &source_federated,
        )
        .await;
        let destination = user(
            &emulator,
            &pool_id,
            DEVELOPER_PROVIDER,
            &["destination-user"],
            &destination_federated,
        )
        .await;
        assert_ne!(
            source, destination,
            "case {case:?}: the two users must start as two identities"
        );

        let (status, body) = emulator
            .call(
                "MergeDeveloperIdentities",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "DeveloperProviderName": DEVELOPER_PROVIDER,
                    "SourceUserIdentifier": "source-user",
                    "DestinationUserIdentifier": "destination-user",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a pair that cannot be merged, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceConflictException"),
            "case {case:?}: expected ResourceConflictException, got {body}"
        );
        assert_eq!(
            error_message(case, &body),
            CANNOT_MERGE_MESSAGE,
            "case {case:?}: expected the fixed cannot-merge sentence, got {body}"
        );

        // The refusal wrote nothing: both identities are still the pool's own two users.
        for (identity_id, identifier) in [
            (&source, "source-user"),
            (&destination, "destination-user"),
        ] {
            let looked_up = emulator
                .ok(
                    "LookupDeveloperIdentity",
                    &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": identifier}),
                )
                .await;
            assert_eq!(
                looked_up["IdentityId"],
                json!(identity_id),
                "case {case:?}: a refused merge leaves {identifier:?} resolving to \
                 {identity_id}, got {looked_up}"
            );
        }
    }
}
