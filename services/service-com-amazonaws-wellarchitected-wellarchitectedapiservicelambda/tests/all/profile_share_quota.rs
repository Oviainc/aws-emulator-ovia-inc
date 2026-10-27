// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the QUOTA branch of `CreateProfileShare`: the branch
//! `super::profile_shares` leaves untested, as `super::lens_shares` and
//! `super::template_shares` each test for their own resource kind.
//!
//! The rule pinned here, over a table of inputs rather than one request: shares of a
//! Well-Architected Tool profile are metered PER PROFILE, so a create is served while the
//! profile holds fewer live shares than the ceiling and is refused with the modelled
//! `402 ServiceQuotaExceededException` — naming the profile the quota meters and carrying
//! the `@required` `QuotaCode` / `ServiceCode`
//! (`smithy_query(query="shape", shape="ServiceQuotaExceededException")`) — once it holds
//! the ceiling, whatever kind of principal the share was asked for, and a refused create
//! records nothing: no share, and not even its `@idempotencyToken`.
//!
//! The rows differ along HOW FULL the profile is (empty, two below the ceiling, the last
//! share the quota admits, and the first it refuses) and along the kind of principal the
//! share is asked for, because neither the profile's identity nor the principal's spelling
//! is what the quota meters. The ceiling itself is read from
//! `state::MAX_SHARES_PER_PROFILE` rather than spelled as a number, so the table follows
//! the quota the service enforces.
//!
//! Each profile is filled to its row's level through `super::profile_shares`'s
//! `record_profile_share`, for the reason `Client::store` gives: reaching a ceiling of a
//! thousand over HTTP would take a thousand signed requests. Every create, revoke and read
//! the test asserts about still crosses the HTTP boundary through the real
//! `MultiServiceRouter`, so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run as they do in production.

use serde_json::Value;

use super::profile_share_deletion::delete_share;
use super::profile_shares::{
    create_share, created_share_id, list_shares, record_profile_share, stored_share_count,
    summaries, NEW_SHARE_STATUS, OTHER_PRINCIPALS,
};
use super::workload_harness::{created_profile, member, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// `ServiceQuotaExceededException$ResourceType` for a share of a profile: the share is a
/// resource of its own — it has its own `ShareId` — so the refusal names it by what it is,
/// exactly as the refusal of a workload share names `WORKLOAD_SHARE`. The member is a
/// free-form string (`smithy_query(query="shape", shape="ExceptionResourceType")`).
/// Spelled as a literal rather than read back out of the code under test.
const PROFILE_SHARE_RESOURCE_TYPE: &str = "PROFILE_SHARE";

/// `ServiceQuotaExceededException$QuotaCode` for the shares of one profile. Both
/// `QuotaCode` and `ServiceCode` are `@required`, the model publishes no numeric Service
/// Quotas code for this quota, so it is named by what it meters and the service by its
/// signing name — the `arnNamespace` / `sigv4Name` of this service
/// (`smithy_query(query="service_info")`).
const SHARES_PER_PROFILE_QUOTA_CODE: &str = "SharesPerProfile";
/// See [`SHARES_PER_PROFILE_QUOTA_CODE`].
const SERVICE_CODE: &str = "wellarchitected";

/// Fill `profile_arn` with `count` shares, each held by a different principal, straight
/// through the shared state (see the module doc for why).
fn seed_shares(client: &Client, profile_arn: &str, count: usize) {
    for index in 0..count {
        // Twelve digits: a principal of the modelled minimum length, and distinct per
        // share, because one principal holds at most one share of one profile.
        record_profile_share(
            client,
            profile_arn,
            &format!("{index:012}"),
            NEW_SHARE_STATUS,
        );
    }
}

/// Whether the store remembers `token` as a `CreateProfileShare` idempotency token — read
/// to prove a REFUSED create recorded nothing, not even its token.
fn token_is_remembered(client: &Client, token: &str) -> bool {
    let state_handle = client.store();
    let store = state_handle.lock();
    store.create_profile_share_tokens.get(token).is_some()
}

/// Assert `answer` is the modelled quota refusal for `profile_arn`, and that the create it
/// refused recorded neither a share for `shared_with` nor `token`.
async fn assert_quota_refusal(
    client: &Client,
    case: &str,
    answer: &Answer,
    profile_arn: &str,
    shared_with: &str,
    token: &str,
) {
    assert_eq!(
        answer.status, 402,
        "case {case:?}: expected HTTP 402, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        profile_arn,
        "case {case:?}: the refusal must name the profile the quota meters, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        PROFILE_SHARE_RESOURCE_TYPE,
        "case {case:?}: expected ResourceType {PROFILE_SHARE_RESOURCE_TYPE}, got {answer:?}"
    );
    assert_eq!(
        member(answer, "QuotaCode"),
        SHARES_PER_PROFILE_QUOTA_CODE,
        "case {case:?}: the refusal must name the quota that was met, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ServiceCode"),
        SERVICE_CODE,
        "case {case:?}: the refusal must name this service, got {answer:?}"
    );
    // The message states WHICH profile met WHICH limit, both taken from the request and
    // from the quota the service enforces rather than written out as one literal.
    let message = member(answer, "Message");
    assert!(
        message.contains(profile_arn),
        "case {case:?}: the message must name the profile, got {message:?}"
    );
    assert!(
        message.contains(&state::MAX_SHARES_PER_PROFILE.to_string()),
        "case {case:?}: the message must state the limit that was met, got {message:?}"
    );

    assert_eq!(
        stored_share_count(client, profile_arn),
        state::MAX_SHARES_PER_PROFILE,
        "case {case:?}: a refused create must record nothing, so the profile still holds \
         exactly {} shares",
        state::MAX_SHARES_PER_PROFILE
    );
    let for_principal = summaries(
        &list_shares(client, profile_arn, &format!("SharedWithPrefix={shared_with}")).await,
        case,
    );
    assert!(
        for_principal.is_empty(),
        "case {case:?}: the refused principal must hold no share, got {for_principal:?}"
    );
    assert!(
        !token_is_remembered(client, token),
        "case {case:?}: a refused create must not remember its ClientRequestToken {token:?}"
    );
}

/// The share the quota admits is served and read back; the one past the ceiling is the
/// modelled 402. The rows differ along how full the profile already is.
#[tokio::test]
async fn a_profile_share_is_served_below_the_per_profile_ceiling_and_refused_at_it() {
    let ceiling = state::MAX_SHARES_PER_PROFILE;
    // (case, shares the profile already holds, whether the next create is served)
    let rows: [(&str, usize, bool); 4] = [
        ("an-empty-profile", 0, true),
        ("two-below-the-ceiling", ceiling - 2, true),
        ("one-below-the-ceiling", ceiling - 1, true),
        ("at-the-ceiling", ceiling, false),
    ];

    for (row, (case, held, served)) in rows.into_iter().enumerate() {
        // One emulator per row: the quota is per profile, and a fresh store is what makes
        // the row's fill level the only thing that differs.
        let client = Client::new();
        let (profile_arn, _) = created_profile(&client, &format!("share-quota-host-{row}")).await;
        seed_shares(&client, &profile_arn, held);
        assert_eq!(
            stored_share_count(&client, &profile_arn),
            held,
            "case {case:?}: the profile must be seeded to {held} shares"
        );

        // The row's principal walks the kinds `SharedWith` publishes (account id,
        // organization id, OU id), because the quota meters the profile's shares and not
        // how the principal is spelled.
        let (principal_case, shared_with) = OTHER_PRINCIPALS[row % OTHER_PRINCIPALS.len()];
        let case = format!("{case}/{principal_case}");
        let token = format!("tok-quota-{row}");
        let answer = create_share(&client, &profile_arn, shared_with, &token).await;

        if served {
            let share_id = created_share_id(&answer, &case);
            let listed =
                list_shares(&client, &profile_arn, &format!("SharedWithPrefix={shared_with}")).await;
            let reported = summaries(&listed, &case);
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: the share the quota admits must read back exactly once, got \
                 {listed:?}"
            );
            assert_eq!(
                reported[0].get("ShareId").and_then(Value::as_str),
                Some(share_id.as_str()),
                "case {case:?}: the share must read back under the id it was minted, got \
                 {listed:?}"
            );
            assert_eq!(
                reported[0].get("Status").and_then(Value::as_str),
                Some(NEW_SHARE_STATUS),
                "case {case:?}: a new share waits on its invitation in {NEW_SHARE_STATUS}, got \
                 {listed:?}"
            );
            assert_eq!(
                stored_share_count(&client, &profile_arn),
                held + 1,
                "case {case:?}: the served create must have recorded exactly one share"
            );
        } else {
            assert_quota_refusal(
                &client,
                &case,
                &answer,
                &profile_arn,
                shared_with,
                &token,
            )
            .await;
        }
    }
}

/// At the ceiling the refusal is the same for every kind of principal, and the quota
/// meters the shares that are LIVE: revoking one makes room for exactly one more.
#[tokio::test]
async fn the_ceiling_refuses_every_principal_and_meters_only_live_shares() {
    let client = Client::new();
    let (profile_arn, _) = created_profile(&client, "share-quota-live-host").await;
    seed_shares(&client, &profile_arn, state::MAX_SHARES_PER_PROFILE);

    for (principal_case, shared_with) in OTHER_PRINCIPALS {
        let token = format!("tok-full-{principal_case}");
        let refused = create_share(&client, &profile_arn, shared_with, &token).await;
        assert_quota_refusal(
            &client,
            principal_case,
            &refused,
            &profile_arn,
            shared_with,
            &token,
        )
        .await;
    }

    // Revoke ONE of the shares the profile holds, over HTTP, and the next create fits.
    let seeded = summaries(
        &list_shares(&client, &profile_arn, "SharedWithPrefix=000000000000").await,
        "the-first-seeded-share",
    );
    let revoked_id = seeded
        .first()
        .and_then(|summary| summary.get("ShareId"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case \"the-first-seeded-share\": the seeded share must be listed, got {seeded:?}")
        })
        .to_string();
    let revoked = delete_share(&client, &profile_arn, &revoked_id, "tok-revoke-for-room").await;
    assert_eq!(
        revoked.status, 200,
        "case \"revoke-one-share\": expected DeleteProfileShare 200, got {revoked:?}"
    );
    assert_eq!(
        stored_share_count(&client, &profile_arn),
        state::MAX_SHARES_PER_PROFILE - 1,
        "case \"revoke-one-share\": the revoke must leave the profile one share below the \
         ceiling"
    );

    let (first_case, first_principal) = OTHER_PRINCIPALS[0];
    let after_the_revoke = create_share(
        &client,
        &profile_arn,
        first_principal,
        &format!("tok-after-revoke-{first_case}"),
    )
    .await;
    created_share_id(&after_the_revoke, "after-the-revoke");
    assert_eq!(
        stored_share_count(&client, &profile_arn),
        state::MAX_SHARES_PER_PROFILE,
        "case \"after-the-revoke\": the profile must be back at the ceiling"
    );

    // Room for exactly one: the share after it meets the ceiling again.
    let (second_case, second_principal) = OTHER_PRINCIPALS[1];
    let token = format!("tok-full-again-{second_case}");
    let full_again = create_share(&client, &profile_arn, second_principal, &token).await;
    assert_quota_refusal(
        &client,
        "full-again",
        &full_again,
        &profile_arn,
        second_principal,
        &token,
    )
    .await;
}
