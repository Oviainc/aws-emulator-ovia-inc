// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `MergeDeveloperIdentities`: two developer-authenticated users of
//! one identity pool become ONE user.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * every login the DESTINATION user was reached through — its developer user identifiers
//!   and its federated logins alike — resolves to the SOURCE user's `IdentityId`
//!   afterwards, which is the id the call answers, and that identity is linked to the union
//!   of the two users' providers;
//! * the merged-away identity keeps none of its own logins and survives as a DISABLED
//!   identity: a read of its id still describes it, a listing that hides no disabled identity
//!   still reports it and one that hides them leaves only the survivor, while the surviving
//!   identity keeps everything it had;
//! * the call's effect is idempotent — repeating it answers the same identity and changes
//!   nothing, because the two identifiers are one user by then;
//! * a call the pool cannot act on is refused with the modelled error for what is wrong with
//!   it and writes nothing: a pool this service does not hold is the modelled
//!   `ResourceNotFoundException`, a developer user
//!   identifier the pool has not registered is the same error naming that USER, a developer
//!   provider that is not the pool's own
//!   is a `NotAuthorizedException`, two users reached through one public provider are a
//!   `ResourceConflictException`, a pair whose linked
//!   logins together pass the service's documented limit is an `InvalidParameterException`,
//!   and a value outside a constraint the MODEL states is refused ahead of the operation with
//!   a `ValidationException` — every one of them HTTP 400.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// The operation under test, spelled once.
const OPERATION: &str = "MergeDeveloperIdentities";

/// The message Amazon Cognito Identity answers a request naming a developer provider that is
/// not its identity pool's own with (`crate::state::DEVELOPER_IDENTIFIER_FORBIDDEN_MESSAGE`)
/// — asserted here against the literal so the wire-visible wording is pinned by the test,
/// not by the code under test.
const NO_ACCESS_MESSAGE: &str = "You do not have access to this identity";

/// The message Amazon Cognito Identity answers a pair of users that cannot become one user
/// with (`crate::state::CANNOT_MERGE_IDENTITIES_MESSAGE`) — one fixed sentence, naming
/// neither user nor the provider they share.
const CANNOT_MERGE_MESSAGE: &str = "Cannot merge these identities.";

/// A developer user identifier no call of this test ever registers, so every pool holds it
/// against no identity.
const ABSENT_IDENTIFIER: &str = "never-registered";

/// The most logins Amazon Cognito links to one identity: "The number of linked logins is
/// limited to 20. So, the number of linked logins for the source user,
/// SourceUserIdentifier, and the destination user, DestinationUserIdentifier, together
/// should not be larger than 20"
/// (`smithy_query(query="doc",shape="MergeDeveloperIdentities")`). Spelled here as the
/// documentation states it, so the bound the test drives is not the one the code carries.
const MAX_LINKED_LOGINS: usize = 20;

/// A `Logins` entry: the provider name and the value presented under it.
type Login<'a> = (&'a str, &'a str);

/// One row of the merge table: the two users, each as the developer user identifiers it is
/// registered under and the federated logins it is also reached through.
struct MergeCase<'a> {
    case: &'a str,
    developer_provider: &'a str,
    source_identifiers: Vec<&'a str>,
    source_federated: Vec<Login<'a>>,
    destination_identifiers: Vec<&'a str>,
    destination_federated: Vec<Login<'a>>,
}

/// The `MergeDeveloperIdentities` request for one case. Every member is sent only when the
/// case carries it, so a case with `None` exercises that member's absence (all four are
/// modelled **required**).
fn merge_request(
    pool_id: Option<&str>,
    developer_provider: Option<&str>,
    source: Option<&str>,
    destination: Option<&str>,
) -> Value {
    let mut request = Map::new();
    if let Some(pool_id) = pool_id {
        request.insert("IdentityPoolId".to_string(), json!(pool_id));
    }
    if let Some(developer_provider) = developer_provider {
        request.insert(
            "DeveloperProviderName".to_string(),
            json!(developer_provider),
        );
    }
    if let Some(source) = source {
        request.insert("SourceUserIdentifier".to_string(), json!(source));
    }
    if let Some(destination) = destination {
        request.insert("DestinationUserIdentifier".to_string(), json!(destination));
    }
    Value::Object(request)
}

/// Register a developer-authenticated user of `pool_id` through the operation that writes
/// the mappings — the first identifier mints the identity, carrying the federated logins
/// beside it, and the rest register under the identity it minted — and answer its
/// `IdentityId`. `pub(crate)` so a sibling suite in this test target registers a developer
/// user the same way rather than keeping a second copy of this.
pub(crate) async fn user(
    emulator: &Emulator,
    pool_id: &str,
    developer_provider: &str,
    identifiers: &[&str],
    federated: &[Login<'_>],
) -> String {
    let mut logins: Vec<Login<'_>> = vec![(developer_provider, identifiers[0])];
    logins.extend(federated.iter().copied());
    let minted = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &token_request(pool_id, &logins, None, None),
        )
        .await;
    let identity_id = minted["IdentityId"]
        .as_str()
        .expect("GetOpenIdTokenForDeveloperIdentity returns an IdentityId")
        .to_string();
    for identifier in &identifiers[1..] {
        let linked = emulator
            .ok(
                "GetOpenIdTokenForDeveloperIdentity",
                &token_request(
                    pool_id,
                    &[(developer_provider, identifier)],
                    Some(&identity_id),
                    None,
                ),
            )
            .await;
        assert_eq!(
            linked["IdentityId"],
            json!(identity_id),
            "{identifier:?} must register under the identity the call named, got {linked}"
        );
    }
    identity_id
}

/// The `IdentityId` the call answers.
fn answered_identity(case: &str, body: &Value) -> String {
    body["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the merge answers an IdentityId: {body}"))
        .to_string()
}

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// Every developer user identifier `LookupDeveloperIdentity` reports for one identity, in
/// the order it reports them — the read-back path for the mappings this operation moved.
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
    strings(
        looked_up["DeveloperUserIdentifierList"]
            .as_array()
            .unwrap_or_else(|| panic!("LookupDeveloperIdentity answers a list: {looked_up}")),
    )
}

/// The identity ids a listing of the pool reports.
async fn listed_identities(emulator: &Emulator, pool_id: &str) -> Vec<String> {
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
        .map(|described| {
            described["IdentityId"]
                .as_str()
                .unwrap_or_else(|| panic!("an identity id is a string: {listed}"))
                .to_string()
        })
        .collect()
}

/// The provider names one identity is linked to, as a read of the identity reports them.
async fn providers_of(emulator: &Emulator, identity_id: &str) -> Vec<String> {
    let described = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": identity_id}))
        .await;
    strings(
        described["Logins"]
            .as_array()
            .unwrap_or_else(|| panic!("DescribeIdentity answers a Logins list: {described}")),
    )
}

/// The strings of a JSON array.
fn strings(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("an entry is a string: {value}"))
                .to_string()
        })
        .collect()
}

/// `values` sorted and deduplicated, which is the order the store reports a pool's logins
/// and an identity's providers in.
fn sorted(values: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    sorted.sort();
    sorted.dedup();
    sorted
}

/// RULE: merging two developer-authenticated users of a pool leaves every login of the
/// destination user resolving to the source user's `IdentityId` — the id the call answers —
/// with that identity linked to the union of the two users' providers, and the
/// destination's own identity gone.
#[tokio::test]
async fn merging_two_developer_users_leaves_every_login_on_the_source_identity() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: HOW MANY developer user
    // identifiers each side carries (one, several, and a pair whose linked logins together
    // sit on the documented maximum of 20), whether each side is ALSO reached through
    // federated logins and how many, and the SPELLING and SIZE of the identifiers
    // (`DeveloperUserIdentifier` is modelled `length 1..1024`, so the rows include each
    // published bound) and of the pool's own `DeveloperProviderName` (modelled
    // `length 1..128, pattern ^[\w._-]+$`, one row sitting on its published maximum) —
    // `smithy_query(query="operation",operation="MergeDeveloperIdentities")`.
    let at_max_identifier = "x".repeat(1024);
    let at_max_provider = "p".repeat(128);
    let ten_source: Vec<String> = (0..10).map(|index| format!("s-user-{index}")).collect();
    let ten_destination: Vec<String> = (0..10).map(|index| format!("d-user-{index}")).collect();
    let cases: Vec<MergeCase<'_>> = vec![
        MergeCase {
            case: "one identifier each, no federated login",
            developer_provider: "login.example.app",
            source_identifiers: vec!["source-user"],
            source_federated: Vec::new(),
            destination_identifiers: vec!["destination-user"],
            destination_federated: Vec::new(),
        },
        MergeCase {
            case: "three identifiers on the source, one on the destination",
            developer_provider: "login.example.app",
            source_identifiers: vec!["m-user", "a-user", "z-user"],
            source_federated: Vec::new(),
            destination_identifiers: vec!["destination-user"],
            destination_federated: Vec::new(),
        },
        MergeCase {
            case: "one identifier on the source, four on the destination",
            developer_provider: "backend_1.example-app",
            source_identifiers: vec!["source-user"],
            source_federated: Vec::new(),
            destination_identifiers: vec!["d-1", "d-2", "d-3", "d-4"],
            destination_federated: Vec::new(),
        },
        MergeCase {
            case: "a federated login on the source only",
            developer_provider: "login.example.app",
            source_identifiers: vec!["source-user"],
            source_federated: vec![("graph.facebook.com", "facebook-source")],
            destination_identifiers: vec!["destination-user"],
            destination_federated: Vec::new(),
        },
        MergeCase {
            case: "federated logins on both sides, under different providers",
            developer_provider: "login.example.app",
            source_identifiers: vec!["source-user"],
            source_federated: vec![
                ("graph.facebook.com", "facebook-source"),
                ("www.amazon.com", "amazon-source"),
            ],
            destination_identifiers: vec!["destination-user"],
            destination_federated: vec![("accounts.google.com", "google-destination")],
        },
        MergeCase {
            case: "identifiers at the minimum and the maximum modelled length",
            developer_provider: "dev_provider",
            source_identifiers: vec!["s"],
            source_federated: Vec::new(),
            destination_identifiers: vec![at_max_identifier.as_str()],
            destination_federated: Vec::new(),
        },
        MergeCase {
            case: "identifiers with spaces, punctuation and non-ASCII",
            developer_provider: "dev-provider",
            source_identifiers: vec!["Jane Doe #7 (eu)", "ユーザー-42"],
            source_federated: Vec::new(),
            destination_identifiers: vec!["person@example.com"],
            destination_federated: Vec::new(),
        },
        MergeCase {
            case: "a developer provider name at the maximum modelled length",
            developer_provider: at_max_provider.as_str(),
            source_identifiers: vec!["source-user"],
            source_federated: Vec::new(),
            destination_identifiers: vec!["destination-user"],
            destination_federated: Vec::new(),
        },
        MergeCase {
            case: "linked logins together on the documented maximum",
            developer_provider: "login.example.app",
            source_identifiers: ten_source.iter().map(String::as_str).collect(),
            source_federated: Vec::new(),
            destination_identifiers: ten_destination.iter().map(String::as_str).collect(),
            destination_federated: Vec::new(),
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let name = case.case;
        // One pool per row, so the rows cannot interfere through a shared login index.
        let pool_id = pool_with_developer_provider(
            &emulator,
            &format!("merge-dev-{row}"),
            Some(case.developer_provider),
        )
        .await;
        let source = user(
            &emulator,
            &pool_id,
            case.developer_provider,
            &case.source_identifiers,
            &case.source_federated,
        )
        .await;
        let destination = user(
            &emulator,
            &pool_id,
            case.developer_provider,
            &case.destination_identifiers,
            &case.destination_federated,
        )
        .await;
        assert_ne!(
            source, destination,
            "case {name:?}: two developer users registered on their own are two identities"
        );

        let merged = emulator
            .ok(
                OPERATION,
                &merge_request(
                    Some(&pool_id),
                    Some(case.developer_provider),
                    Some(case.source_identifiers[0]),
                    Some(case.destination_identifiers[0]),
                ),
            )
            .await;
        // The identity the two users keep is the SOURCE user's, which is the one the merge's
        // recorded response answers and the one it resolves both identifiers to afterwards.
        let (survivor, merged_away) = (&source, &destination);
        assert_eq!(
            answered_identity(name, &merged),
            *survivor,
            "case {name:?}: expected the source user's identity {survivor}, got {merged}"
        );

        // Every developer user identifier of EITHER user now resolves to the surviving
        // identity, which is registered under all of them.
        let every_identifier: Vec<&str> = case
            .source_identifiers
            .iter()
            .chain(case.destination_identifiers.iter())
            .copied()
            .collect();
        for identifier in &every_identifier {
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
                json!(survivor),
                "case {name:?}: {identifier:?} must resolve to {survivor}, got {looked_up}"
            );
        }
        assert_eq!(
            identifiers_of(&emulator, &pool_id, survivor).await,
            sorted(&every_identifier),
            "case {name:?}: expected the surviving identity registered under every identifier of \
             both users"
        );

        // A federated login of either user resolves to the surviving identity too: the whole
        // login set of the merged-away user moved.
        for (provider, login) in case
            .source_federated
            .iter()
            .chain(case.destination_federated.iter())
        {
            let resolved = emulator
                .identity(&pool_id, Some(json!({ *provider: login })))
                .await;
            assert_eq!(
                resolved, *survivor,
                "case {name:?}: the login of {provider:?} must resolve to {survivor}, got \
                 {resolved}"
            );
        }

        // The surviving identity is linked to the union of the two users' providers.
        let mut providers: Vec<&str> = vec![case.developer_provider];
        providers.extend(
            case.source_federated
                .iter()
                .chain(case.destination_federated.iter())
                .map(|(provider, _)| *provider),
        );
        assert_eq!(
            providers_of(&emulator, survivor).await,
            sorted(&providers),
            "case {name:?}: expected the surviving identity linked to {providers:?}"
        );

        // The two users are one user, and the merged-away identity is still a record of the
        // pool with no login of its own: the listing reports both identities in the order
        // they were minted, and a read of the merged-away one still describes it.
        assert_eq!(
            listed_identities(&emulator, &pool_id).await,
            vec![survivor.clone(), merged_away.clone()],
            "case {name:?}: expected {survivor} and the merged-away {merged_away} in the pool"
        );
        let (status, body) = emulator
            .call("DescribeIdentity", &json!({"IdentityId": merged_away}))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the merged-away identity {merged_away} is still stored, got {status} \
             with {body}"
        );
        assert_eq!(
            body["IdentityId"],
            json!(merged_away),
            "case {name:?}: expected the description of {merged_away}, got {body}"
        );

        // The EFFECT is idempotent: the two identifiers are one user now, so repeating the
        // request answers the same identity and changes nothing.
        let repeated = emulator
            .ok(
                OPERATION,
                &merge_request(
                    Some(&pool_id),
                    Some(case.developer_provider),
                    Some(case.source_identifiers[0]),
                    Some(case.destination_identifiers[0]),
                ),
            )
            .await;
        assert_eq!(
            answered_identity(name, &repeated),
            *survivor,
            "case {name:?}: the repeated merge answers {survivor}, got {repeated}"
        );
        assert_eq!(
            identifiers_of(&emulator, &pool_id, survivor).await,
            sorted(&every_identifier),
            "case {name:?}: the repeated merge registers nothing further"
        );
        assert_eq!(
            listed_identities(&emulator, &pool_id).await,
            vec![survivor.clone(), merged_away.clone()],
            "case {name:?}: the repeated merge changes no identity of the pool"
        );
    }
}

/// What makes a row of the refusal table a request the service cannot serve.
enum Broken {
    /// The pool named is one this service does not hold.
    AbsentPool,
    /// The developer provider named is not the pool's own.
    OtherDeveloperProvider,
    /// The source user identifier was never registered.
    AbsentSourceIdentifier,
    /// The destination user identifier was never registered.
    AbsentDestinationIdentifier,
    /// Both users are reached through one public provider, as two different users.
    SharedPublicProvider,
    /// The two users' linked logins together pass the documented limit.
    PastTheLinkedLoginLimit,
    /// A member the model makes required is left out, or carries a value outside a
    /// constraint the model states.
    OutsideTheModel(Value),
}

/// RULE: a merge the pool cannot act on is refused with the modelled error for what is wrong
/// with it, and writes nothing — the two users stay two users, each still answering its own
/// identifier.
#[tokio::test]
async fn a_merge_the_pool_cannot_act_on_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";

    // `DeveloperProviderName` is modelled `length 1..128, pattern ^[\w._-]+$` and
    // `DeveloperUserIdentifier` `length 1..1024`, so these sit one past each published
    // maximum, and one outside the provider pattern.
    let past_max_provider = "p".repeat(129);
    let past_max_identifier = "x".repeat(1025);

    // (case, what is wrong with the request, expected status, expected error code)
    let cases: Vec<(&str, Broken, u16, &str)> = vec![
        (
            "a pool this service does not hold",
            Broken::AbsentPool,
            400,
            "ResourceNotFoundException",
        ),
        (
            "a developer provider name that is not this pool's",
            Broken::OtherDeveloperProvider,
            400,
            "NotAuthorizedException",
        ),
        (
            "a source identifier that was never registered",
            Broken::AbsentSourceIdentifier,
            400,
            "ResourceNotFoundException",
        ),
        (
            "a destination identifier that was never registered",
            Broken::AbsentDestinationIdentifier,
            400,
            "ResourceNotFoundException",
        ),
        (
            "two users reached through one public provider",
            Broken::SharedPublicProvider,
            400,
            "ResourceConflictException",
        ),
        (
            "linked logins together one past the documented maximum",
            Broken::PastTheLinkedLoginLimit,
            400,
            "InvalidParameterException",
        ),
        (
            "an absent IdentityPoolId",
            Broken::OutsideTheModel(json!({"IdentityPoolId": null})),
            400,
            "ValidationException",
        ),
        (
            "an absent DeveloperProviderName",
            Broken::OutsideTheModel(json!({"DeveloperProviderName": null})),
            400,
            "ValidationException",
        ),
        (
            "an absent SourceUserIdentifier",
            Broken::OutsideTheModel(json!({"SourceUserIdentifier": null})),
            400,
            "ValidationException",
        ),
        (
            "an absent DestinationUserIdentifier",
            Broken::OutsideTheModel(json!({"DestinationUserIdentifier": null})),
            400,
            "ValidationException",
        ),
        (
            "an identity pool id outside the modelled pattern",
            Broken::OutsideTheModel(json!({"IdentityPoolId": "not-a-pool"})),
            400,
            "ValidationException",
        ),
        (
            "a developer provider name outside the modelled pattern",
            Broken::OutsideTheModel(json!({"DeveloperProviderName": "spaces are not allowed"})),
            400,
            "ValidationException",
        ),
        (
            "a developer provider name one character past the modelled maximum length",
            Broken::OutsideTheModel(json!({"DeveloperProviderName": past_max_provider})),
            400,
            "ValidationException",
        ),
        (
            "an empty source identifier",
            Broken::OutsideTheModel(json!({"SourceUserIdentifier": ""})),
            400,
            "ValidationException",
        ),
        (
            "a destination identifier one character past the modelled maximum length",
            Broken::OutsideTheModel(json!({"DestinationUserIdentifier": past_max_identifier})),
            400,
            "ValidationException",
        ),
    ];

    for (row, (case, broken, expected_status, expected_code)) in cases.iter().enumerate() {
        // One pool per row, each holding the two users the row's request names, so a refusal
        // is read against a pool nothing else touched.
        let pool_id = pool_with_developer_provider(
            &emulator,
            &format!("merge-refusal-{row}"),
            Some(developer_provider),
        )
        .await;
        let shared_provider = matches!(broken, Broken::SharedPublicProvider);
        let source_federated: Vec<Login<'_>> = if shared_provider {
            vec![("graph.facebook.com", "facebook-source")]
        } else {
            Vec::new()
        };
        let destination_federated: Vec<Login<'_>> = if shared_provider {
            vec![("graph.facebook.com", "facebook-destination")]
        } else {
            Vec::new()
        };
        // The limit row needs the two users to carry one linked login more than the limit
        // allows together; every other row needs one identifier each.
        let source_identifiers: Vec<String> = match broken {
            Broken::PastTheLinkedLoginLimit => (0..=MAX_LINKED_LOGINS / 2)
                .map(|index| format!("source-user-{index}"))
                .collect(),
            _ => vec!["source-user".to_string()],
        };
        let destination_identifiers: Vec<String> = match broken {
            Broken::PastTheLinkedLoginLimit => (0..MAX_LINKED_LOGINS / 2)
                .map(|index| format!("destination-user-{index}"))
                .collect(),
            _ => vec!["destination-user".to_string()],
        };
        let source_named: Vec<&str> = source_identifiers.iter().map(String::as_str).collect();
        let destination_named: Vec<&str> = destination_identifiers
            .iter()
            .map(String::as_str)
            .collect();
        let source = user(
            &emulator,
            &pool_id,
            developer_provider,
            &source_named,
            &source_federated,
        )
        .await;
        let destination = user(
            &emulator,
            &pool_id,
            developer_provider,
            &destination_named,
            &destination_federated,
        )
        .await;

        let mut request = merge_request(
            Some(&pool_id),
            Some(developer_provider),
            Some(source_named[0]),
            Some(destination_named[0]),
        );
        match broken {
            Broken::AbsentPool => request["IdentityPoolId"] = json!(ABSENT_POOL_ID),
            Broken::OtherDeveloperProvider => {
                request["DeveloperProviderName"] = json!("another.example.app");
            }
            Broken::AbsentSourceIdentifier => {
                request["SourceUserIdentifier"] = json!(ABSENT_IDENTIFIER);
            }
            Broken::AbsentDestinationIdentifier => {
                request["DestinationUserIdentifier"] = json!(ABSENT_IDENTIFIER);
            }
            Broken::SharedPublicProvider | Broken::PastTheLinkedLoginLimit => {}
            Broken::OutsideTheModel(override_members) => {
                for (member, value) in override_members
                    .as_object()
                    .expect("the overrides are an object")
                {
                    match value {
                        // A member the row sends as null is one it LEAVES OUT, which is what
                        // exercises the absence of a member the model makes required.
                        Value::Null => {
                            request
                                .as_object_mut()
                                .expect("the request is an object")
                                .remove(member);
                        }
                        value => request[member] = value.clone(),
                    }
                }
            }
        }

        let (status, body) = emulator.call(OPERATION, &request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {body}"
        );
        if *expected_code == "NotAuthorizedException" {
            assert_eq!(
                error_message(case, &body),
                NO_ACCESS_MESSAGE,
                "case {case:?}: expected the fixed no-access message, got {body}"
            );
        }
        // A developer user identifier the pool has registered against no identity names a
        // USER this service does not hold, and the refusal names that user with the
        // identifier the request carried; two users of one public provider cannot become
        // one user at all, and that refusal carries AWS's fixed sentence with no part of the
        // request or the store in it.
        match broken {
            Broken::AbsentSourceIdentifier | Broken::AbsentDestinationIdentifier => {
                assert_eq!(
                    error_message(case, &body),
                    format!("User '{ABSENT_IDENTIFIER}' not found."),
                    "case {case:?}: expected the missing developer user to be named, got {body}"
                );
            }
            Broken::SharedPublicProvider => {
                assert_eq!(
                    error_message(case, &body),
                    CANNOT_MERGE_MESSAGE,
                    "case {case:?}: expected the fixed cannot-merge message, got {body}"
                );
            }
            _ => {}
        }

        // The refusal wrote nothing: the two users are still two users, each registered
        // under exactly the identifiers it was.
        for (identity, identifiers) in [
            (&source, &source_named),
            (&destination, &destination_named),
        ] {
            assert_eq!(
                identifiers_of(&emulator, &pool_id, identity).await,
                sorted(identifiers),
                "case {case:?}: a refused merge moves no mapping, so {identity} still answers \
                 {identifiers:?}"
            );
        }
        let mut both = listed_identities(&emulator, &pool_id).await;
        both.sort();
        let mut expected = vec![source.clone(), destination.clone()];
        expected.sort();
        assert_eq!(
            both, expected,
            "case {case:?}: a refused merge removes no identity"
        );
    }
}
