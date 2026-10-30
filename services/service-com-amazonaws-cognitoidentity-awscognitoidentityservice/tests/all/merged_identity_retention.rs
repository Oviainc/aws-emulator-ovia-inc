// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a merge LEAVES BEHIND: the identity it merged away.
//!
//! RULE: a call that takes an identity's logins away does not take the identity's RECORD
//! away. `MergeDeveloperIdentities` moves every login of the merged-away user onto the
//! surviving identity and leaves the merged-away one STORED as a disabled identity of its
//! pool — still described by a read of its id, still reported by a listing that hides no
//! disabled identity, hidden from one that hides them, keeping the instant it was minted and
//! stamped with the instant the merge changed it, and reached by no developer user of the
//! pool any more (so resolving which developer user it is is refused as access the caller
//! does not have) — while removing the record is
//! `DeleteIdentities`' own effect, after which a read of that id is the modelled
//! `ResourceNotFoundException`
//! `ResourceNotFoundException`.
//!
//! Each case asserts that rule over a TABLE of inputs rather than one recorded call: the rows
//! differ in how many developer user identifiers each user carries, whether the merged-away
//! user is ALSO reached through federated logins, the spelling and size of the identifiers
//! and of the pool's own developer provider name, and (second case) how many users are merged
//! into one, one merge after another.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds. The developer users are registered with
//! `merge_developer_identities`' own helper, so both suites drive one registration path.

use serde_json::{json, Value};

use crate::describe_identity::describe;
use crate::developer_identity_tokens::pool_with_developer_provider;
use crate::identity_pool_flow::{error_code, Emulator};
use crate::merge_developer_identities::user;

/// The one element the `Logins` list of a DISABLED identity carries — an identity every login
/// has been taken off. Spelled out here so the wire-visible projection is pinned by the test
/// rather than by the code under test.
const DISABLED: &str = "DISABLED";

/// How this service words the refusal of a request about a developer identity the caller may
/// not reach — asserted against the literal, for the same reason.
const NO_ACCESS_MESSAGE: &str = "You do not have access to this identity";

/// A `Logins` entry: the provider name and the value presented under it.
type Login<'a> = (&'a str, &'a str);

/// One row of the retention table: the two developer users of one pool, each as the developer
/// user identifiers it is registered under, with the federated logins the merged-away one is
/// also reached through.
struct Row<'a> {
    case: &'a str,
    developer_provider: &'a str,
    survivor_identifiers: Vec<&'a str>,
    merged_away_identifiers: Vec<&'a str>,
    merged_away_federated: Vec<Login<'a>>,
}

/// `DescribeIdentity` for one identity, insisting it is served.
async fn described(emulator: &Emulator, case: &str, identity_id: &str) -> Value {
    let (status, body) = describe(emulator, identity_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {identity_id} to be described with HTTP 200, got {status} with \
         {body}"
    );
    body
}

/// The `Logins` list a description carries.
fn logins_of(case: &str, description: &Value) -> Vec<String> {
    description["Logins"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a description carries a Logins list: {description}"))
        .iter()
        .map(|login| {
            login
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a login is a string: {description}"))
                .to_string()
        })
        .collect()
}

/// The instant a reported member carries, as the seconds since the epoch an `awsJson` body
/// writes an instant as.
fn instant(case: &str, description: &Value, member: &str) -> f64 {
    description[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {member} is an instant: {description}"))
}

/// Every `IdentityId` a listing of the pool reports, with `HideDisabled` sent as given — the
/// member that "allows you to hide disabled identities"
/// (`smithy_query(query="operation",operation="ListIdentities")`).
async fn listed(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    hide_disabled: bool,
) -> Vec<String> {
    let page = emulator
        .ok(
            "ListIdentities",
            &json!({
                "IdentityPoolId": pool_id,
                "MaxResults": 60,
                "HideDisabled": hide_disabled,
            }),
        )
        .await;
    page["Identities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListIdentities answers an Identities list: {page}"))
        .iter()
        .map(|description| {
            description["IdentityId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an identity id is a string: {page}"))
                .to_string()
        })
        .collect()
}

/// Merge the developer user `merged_away_identifier` into `survivor_identifier` and answer the
/// `IdentityId` the call reports — which is the surviving identity.
async fn merge(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    developer_provider: &str,
    survivor_identifier: &str,
    merged_away_identifier: &str,
) -> String {
    let merged = emulator
        .ok(
            "MergeDeveloperIdentities",
            &json!({
                "IdentityPoolId": pool_id,
                "DeveloperProviderName": developer_provider,
                "SourceUserIdentifier": survivor_identifier,
                "DestinationUserIdentifier": merged_away_identifier,
            }),
        )
        .await;
    merged["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the merge answers an IdentityId: {merged}"))
        .to_string()
}

/// RULE: the identity a merge merged away is still a stored, DISABLED identity of its pool,
/// and only `DeleteIdentities` removes the record.
#[tokio::test]
async fn a_merge_leaves_the_merged_away_identity_stored_as_a_disabled_one() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: HOW MANY developer user
    // identifiers each side carries, whether the merged-away user is also reached through
    // federated logins and how many, and the spelling and size of the identifiers
    // (`DeveloperUserIdentifier` is modelled `length 1..1024`, so one row sits on each
    // published bound) and of the pool's own `DeveloperProviderName` (modelled
    // `length 1..128, pattern ^[\w._-]+$`, one row on its published maximum) —
    // `smithy_query(query="operation",operation="MergeDeveloperIdentities")`.
    let at_max_identifier = "x".repeat(1024);
    let at_max_provider = "p".repeat(128);
    let rows: Vec<Row<'_>> = vec![
        Row {
            case: "one identifier each, no federated login",
            developer_provider: "login.example.app",
            survivor_identifiers: vec!["survivor-user"],
            merged_away_identifiers: vec!["absorbed-user"],
            merged_away_federated: Vec::new(),
        },
        Row {
            case: "three identifiers on the merged-away user",
            developer_provider: "login.example.app",
            survivor_identifiers: vec!["survivor-user"],
            merged_away_identifiers: vec!["a-user", "m-user", "z-user"],
            merged_away_federated: Vec::new(),
        },
        Row {
            case: "a federated login on the merged-away user",
            developer_provider: "backend_1.example-app",
            survivor_identifiers: vec!["survivor-user"],
            merged_away_identifiers: vec!["absorbed-user"],
            merged_away_federated: vec![("graph.facebook.com", "facebook-absorbed")],
        },
        Row {
            case: "two federated logins on the merged-away user",
            developer_provider: "login.example.app",
            survivor_identifiers: vec!["s-1", "s-2"],
            merged_away_identifiers: vec!["absorbed-user"],
            merged_away_federated: vec![
                ("accounts.google.com", "google-absorbed"),
                ("www.amazon.com", "amazon-absorbed"),
            ],
        },
        Row {
            case: "identifiers at the minimum and the maximum modelled length",
            developer_provider: "dev_provider",
            survivor_identifiers: vec!["s"],
            merged_away_identifiers: vec![at_max_identifier.as_str()],
            merged_away_federated: Vec::new(),
        },
        Row {
            case: "an identifier with spaces, punctuation and non-ASCII",
            developer_provider: at_max_provider.as_str(),
            survivor_identifiers: vec!["person@example.com"],
            merged_away_identifiers: vec!["Jane Doe #7 (eu)", "ユーザー-42"],
            merged_away_federated: Vec::new(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One pool per row, so the rows cannot interfere through a shared login index.
        let pool_id = pool_with_developer_provider(
            &emulator,
            &format!("merge-retention-{index}"),
            Some(row.developer_provider),
        )
        .await;
        let survivor = user(
            &emulator,
            &pool_id,
            row.developer_provider,
            &row.survivor_identifiers,
            &[],
        )
        .await;
        let merged_away = user(
            &emulator,
            &pool_id,
            row.developer_provider,
            &row.merged_away_identifiers,
            &row.merged_away_federated,
        )
        .await;
        let before = described(&emulator, case, &merged_away).await;
        let minted_at = instant(case, &before, "CreationDate");
        let changed_at = instant(case, &before, "LastModifiedDate");

        let answered = merge(
            &emulator,
            case,
            &pool_id,
            row.developer_provider,
            row.survivor_identifiers[0],
            row.merged_away_identifiers[0],
        )
        .await;
        assert_eq!(
            answered, survivor,
            "case {case:?}: expected the merge to answer the surviving identity {survivor}, got \
             {answered}"
        );

        // The merged-away identity is STILL stored: a read of its id is served, and it is the
        // record that id was minted for — the same creation instant, now stamped with the
        // instant the merge changed it.
        let after = described(&emulator, case, &merged_away).await;
        assert_eq!(
            after["IdentityId"],
            json!(merged_away),
            "case {case:?}: expected the description of {merged_away}, got {after}"
        );
        assert_eq!(
            instant(case, &after, "CreationDate"),
            minted_at,
            "case {case:?}: the instant {merged_away} was minted does not move, got {after}"
        );
        assert!(
            instant(case, &after, "LastModifiedDate") >= changed_at,
            "case {case:?}: expected the merge to stamp {merged_away} as changed no earlier than \
             {changed_at}, got {after}"
        );

        // It carries no login of its own any more — every one of them moved onto the survivor
        // — which is what makes it a DISABLED identity, projected as the single sentinel.
        assert_eq!(
            logins_of(case, &after),
            vec![DISABLED.to_string()],
            "case {case:?}: expected {merged_away} to project the disabled sentinel, got {after}"
        );

        // A listing that hides no disabled identity reports both records, in the order they
        // were minted; one that hides them leaves the survivor alone.
        assert_eq!(
            listed(&emulator, case, &pool_id, false).await,
            vec![survivor.clone(), merged_away.clone()],
            "case {case:?}: expected both {survivor} and {merged_away} reported with \
             HideDisabled=false"
        );
        assert_eq!(
            listed(&emulator, case, &pool_id, true).await,
            vec![survivor.clone()],
            "case {case:?}: expected only {survivor} reported with HideDisabled=true"
        );

        // No developer user of the pool reaches it any more — every identifier of both users
        // resolves to the survivor — so asking which developer user it is is refused, while
        // the same question about the survivor is answered with every identifier of both.
        let (status, body) = emulator
            .call(
                "LookupDeveloperIdentity",
                &json!({"IdentityPoolId": &pool_id, "IdentityId": &merged_away}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the lookup of the merged-away {merged_away} to be refused \
             with HTTP 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException for {merged_away}, got {code:?} with \
             {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(NO_ACCESS_MESSAGE),
            "case {case:?}: expected the refusal of {merged_away} worded {NO_ACCESS_MESSAGE:?}, \
             got {body}"
        );
        let mut every_identifier: Vec<String> = row
            .survivor_identifiers
            .iter()
            .chain(row.merged_away_identifiers.iter())
            .map(|identifier| (*identifier).to_string())
            .collect();
        every_identifier.sort();
        let looked_up = emulator
            .ok(
                "LookupDeveloperIdentity",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityId": &survivor,
                    "MaxResults": 60,
                }),
            )
            .await;
        let reported: Vec<String> = looked_up["DeveloperUserIdentifierList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the lookup answers a list: {looked_up}"))
            .iter()
            .map(|identifier| {
                identifier
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an identifier is a string: {looked_up}")
                    })
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, every_identifier,
            "case {case:?}: expected {survivor} registered under every identifier of both users"
        );

        // Deleting the record IS what removes it, and that is the only thing that does: the
        // read that was served a moment ago is the modelled not-found afterwards, worded as
        // this service words a read of an identity it does not hold, while the survivor is
        // untouched.
        emulator
            .ok(
                "DeleteIdentities",
                &json!({"IdentityIdsToDelete": [&merged_away]}),
            )
            .await;
        let (status, body) = describe(&emulator, &merged_away).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted {merged_away} to be refused, got {status} with \
             {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException for the deleted {merged_away}, got \
             {code:?} with {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("Identity '{merged_away}' not found.").as_str()),
            "case {case:?}: expected the refusal to name {merged_away}, got {body}"
        );
        assert_eq!(
            listed(&emulator, case, &pool_id, false).await,
            vec![survivor.clone()],
            "case {case:?}: expected the deletion to leave only {survivor} in the pool"
        );
        let survivor_description = described(&emulator, case, &survivor).await;
        assert_eq!(
            survivor_description["IdentityId"],
            json!(survivor),
            "case {case:?}: the surviving identity is untouched by the deletion of another, got \
             {survivor_description}"
        );
    }
}

/// RULE: every merge retains its own merged-away record, so a pool whose users were merged
/// into one still holds one identity per user it ever minted — all but one of them disabled.
#[tokio::test]
async fn merging_many_users_into_one_retains_one_disabled_identity_per_merge() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";

    // The rows differ in HOW MANY developer users of the pool are merged into one, one merge
    // after another: two (one merge) up to five (four merges).
    for users in 2..=5usize {
        let case = format!("{users} users merged into one");
        let pool_id = pool_with_developer_provider(
            &emulator,
            &format!("merge-chain-{users}"),
            Some(developer_provider),
        )
        .await;

        let identifiers: Vec<String> = (0..users).map(|index| format!("user-{index}")).collect();
        let mut minted = Vec::with_capacity(users);
        for identifier in &identifiers {
            minted.push(
                user(
                    &emulator,
                    &pool_id,
                    developer_provider,
                    &[identifier.as_str()],
                    &[],
                )
                .await,
            );
        }

        // Every later user is merged into the first one, which is the identity that survives.
        for identifier in &identifiers[1..] {
            let answered = merge(
                &emulator,
                &case,
                &pool_id,
                developer_provider,
                &identifiers[0],
                identifier,
            )
            .await;
            assert_eq!(
                answered, minted[0],
                "case {case:?}: expected every merge to answer {}, got {answered}",
                minted[0]
            );
        }

        assert_eq!(
            listed(&emulator, &case, &pool_id, false).await,
            minted,
            "case {case:?}: expected one identity per user the pool ever minted"
        );
        assert_eq!(
            listed(&emulator, &case, &pool_id, true).await,
            vec![minted[0].clone()],
            "case {case:?}: expected only the surviving identity to be an enabled one"
        );
        for merged_away in &minted[1..] {
            let description = described(&emulator, &case, merged_away).await;
            assert_eq!(
                logins_of(&case, &description),
                vec![DISABLED.to_string()],
                "case {case:?}: expected {merged_away} to project the disabled sentinel, got \
                 {description}"
            );
        }
    }
}
