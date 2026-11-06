// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED write leaves behind in the store, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: a request a write operation refuses changes NOTHING
//! in the store — not the stored resource, and not the store's own identifier-mint
//! counters — because an identifier the service generates is minted only for a write that
//! is actually applied. So an applied write consumes exactly one identifier per resource
//! it stores, a write that stores none consumes none, and the identifier the next applied
//! write hands out does not depend on how many refused requests preceded it.
//!
//! Those counters are store state no response reports (a minted id is unguessable
//! precisely because it is derived from them), so they are read through the one shared
//! engine the harness keeps — the way the sibling suites read a retention bound.
//!
//! The rows differ along what the rule generalises over: which write is refused
//! (`AddNotificationChannels`, `PutPermission`, `RemovePermission`), which refusal it is
//! (not found, invalid parameter, conflict, quota), how many identifiers the request would
//! otherwise have had the store mint (0, 1, 2) and, on the applied side, how many of a
//! request's channels carry an id the caller chose.

use crate::notification_channels::{
    add, channel_body, create_group, get, is_channel_id, reported_ids,
};
use crate::permission_writes::{put_permission, remove_permission, string_member};
use crate::profiling_group_lifecycle::{Answer, Harness};

/// Caller-chosen `channelId`s, each matching the model's `ChannelId` pattern
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`, so the boundary
/// validator admits them and the handler decides what they mean.
const NAMED_ID: &str = "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";
const SECOND_NAMED_ID: &str = "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d";

/// The only value the modelled `ActionGroup` enum declares
/// (`smithy_query(query="shape",shape="ActionGroup")`).
const ACTION_GROUP: &str = "agentPermissions";

/// A well-formed `RevisionId` this service never minted, for the rows that name a
/// revision the store does not hold.
const FOREIGN_REVISION: &str = "00000000-0000-4000-8000-000000000000";

/// A uri that is not an ARN at all, which `ChannelUri` — an `@arnReference` documented as
/// "Currently we only support SNS channels and thus required to be an ARN"
/// (`smithy_query(query="shape",shape="ChannelUri")`) — cannot carry.
const NOT_AN_ARN: &str = "https://example.invalid/topic";

/// An Amazon SNS topic ARN, the form a channel uri must take.
fn topic(name: &str) -> String {
    format!(
        "arn:aws:sns:us-east-1:{}:{name}",
        ::engine::EMULATOR_ACCOUNT_ID
    )
}

/// One principal to grant to. Source: the `principals` doc, "A list ARNs for the roles
/// and users you want to grant access to the profiling group"
/// (`smithy_query(query="operation",operation="PutPermission")`).
fn principal() -> String {
    format!(
        "arn:aws:iam::{}:role/AgentRole",
        ::engine::EMULATOR_ACCOUNT_ID
    )
}

/// The store's own identifier-mint counters, `(notification-channel ids, policy
/// revisions)` — state no response reports, so the only way to see what a refused write
/// did to it is to read the one shared engine behind the harness. Shared with the
/// sibling suites that assert which writes mint a revision at all.
pub(crate) fn mint_counts(harness: &Harness) -> (u64, u64) {
    let state = harness
        .engine()
        .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
    let guard = state.lock();
    (guard.channel_ids_minted, guard.revisions_minted)
}

/// One request this service refuses: which write it is and what it carries.
enum Refused {
    /// `AddNotificationChannels` on a group, with these channel bodies.
    AddChannels {
        group: &'static str,
        channels: Vec<serde_json::Value>,
    },
    /// `PutPermission` on a group, naming this revision (`None` names none).
    PutPermission {
        group: &'static str,
        revision: Option<String>,
    },
    /// `RemovePermission` on a group, naming this revision.
    RemovePermission {
        group: &'static str,
        revision: String,
    },
}

async fn send(harness: &Harness, refused: &Refused) -> Answer {
    match refused {
        Refused::AddChannels { group, channels } => add(harness, group, channels).await,
        Refused::PutPermission { group, revision } => {
            put_permission(
                harness,
                group,
                ACTION_GROUP,
                &[principal()],
                revision.as_deref(),
            )
            .await
        }
        Refused::RemovePermission { group, revision } => {
            remove_permission(harness, group, ACTION_GROUP, revision).await
        }
    }
}

/// The channel ids every named group holds right now, so a row can assert a refused write
/// left each of them exactly as it was.
async fn configurations(
    harness: &Harness,
    case: &str,
    groups: &[&'static str],
) -> Vec<(&'static str, Vec<String>)> {
    let mut held = Vec::with_capacity(groups.len());
    for group in groups {
        held.push((*group, reported_ids(case, &get(harness, group).await)));
    }
    held
}

#[tokio::test]
async fn a_refused_write_mints_no_identifier() {
    use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_NOTIFICATION_CHANNELS_PER_GROUP;

    let harness = Harness::new();

    // The stored shapes the rows below are refused against: a group holding one
    // caller-named channel, a group holding the two channels the model's `Channels` list
    // allows (both ids minted), a group holding a resource-based policy, and a group
    // holding neither.
    let one_stored = "mint-one-stored";
    let full = "mint-full";
    let policied = "mint-policied";
    let bare = "mint-bare";
    let groups: [&'static str; 4] = [one_stored, full, policied, bare];
    for group in groups {
        create_group(&harness, group).await;
    }

    let stored = add(
        &harness,
        one_stored,
        &[channel_body(Some(NAMED_ID), &topic("stored"))],
    )
    .await;
    assert_eq!(
        stored.status,
        200,
        "setup: the first channel of {one_stored} must be stored, got {} / {}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );
    let filled: Vec<serde_json::Value> = (0..MAX_NOTIFICATION_CHANNELS_PER_GROUP)
        .map(|index| channel_body(None, &topic(&format!("full-{index}"))))
        .collect();
    let filling = add(&harness, full, &filled).await;
    assert_eq!(
        filling.status,
        200,
        "setup: {full} must accept the {MAX_NOTIFICATION_CHANNELS_PER_GROUP} channels its \
         quota allows, got {} / {}",
        filling.status,
        String::from_utf8_lossy(&filling.body)
    );
    let created = put_permission(&harness, policied, ACTION_GROUP, &[principal()], None).await;
    assert_eq!(
        created.status,
        200,
        "setup: the first policy write on {policied} must succeed, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let policy_revision = string_member(&created, "revisionId");

    // Each row: the refusal, the request that draws it, and the status it carries. The
    // channel rows deliberately include channels whose id the request does NOT name, so a
    // handler that minted before deciding would move the channel-id counter.
    let rows: Vec<(&str, Refused, u16)> = vec![
        (
            "a group the store does not hold, one channel to mint",
            Refused::AddChannels {
                group: "mint-no-such-group",
                channels: vec![channel_body(None, &topic("absent"))],
            },
            404,
        ),
        (
            "a channel uri that is not an SNS topic ARN",
            Refused::AddChannels {
                group: bare,
                channels: vec![channel_body(None, NOT_AN_ARN)],
            },
            400,
        ),
        (
            "a second channel to mint behind an unusable uri",
            Refused::AddChannels {
                group: bare,
                channels: vec![
                    channel_body(None, &topic("first-of-two")),
                    channel_body(None, NOT_AN_ARN),
                ],
            },
            400,
        ),
        (
            "an id the configuration already holds, beside one to mint",
            Refused::AddChannels {
                group: one_stored,
                channels: vec![
                    channel_body(Some(NAMED_ID), &topic("again")),
                    channel_body(None, &topic("alongside")),
                ],
            },
            409,
        ),
        (
            "the same caller-named id twice in one request",
            Refused::AddChannels {
                group: bare,
                channels: vec![
                    channel_body(Some(SECOND_NAMED_ID), &topic("twice-a")),
                    channel_body(Some(SECOND_NAMED_ID), &topic("twice-b")),
                ],
            },
            409,
        ),
        (
            "one channel past a full group's quota",
            Refused::AddChannels {
                group: full,
                channels: vec![channel_body(None, &topic("past-quota"))],
            },
            402,
        ),
        (
            "two channels past the quota of a group holding one",
            Refused::AddChannels {
                group: one_stored,
                channels: vec![
                    channel_body(None, &topic("past-quota-a")),
                    channel_body(None, &topic("past-quota-b")),
                ],
            },
            402,
        ),
        (
            "a policy write naming a revision the store does not hold",
            Refused::PutPermission {
                group: policied,
                revision: Some(FOREIGN_REVISION.to_string()),
            },
            409,
        ),
        (
            "a policy write that names no revision for a group that has one",
            Refused::PutPermission {
                group: policied,
                revision: None,
            },
            409,
        ),
        (
            "a first policy write that names a revision",
            Refused::PutPermission {
                group: bare,
                revision: Some(FOREIGN_REVISION.to_string()),
            },
            409,
        ),
        (
            "a policy removal naming a stale revision",
            Refused::RemovePermission {
                group: policied,
                revision: FOREIGN_REVISION.to_string(),
            },
            409,
        ),
    ];

    for (case, refused, expected) in rows {
        let before_counts = mint_counts(&harness);
        let before_configurations = configurations(&harness, case, &groups).await;
        let before_revision = harness.stored_policy_revision(policied);

        let answer = send(&harness, &refused).await;
        assert_eq!(
            answer.status,
            expected,
            "case {case}: expected {expected}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        let after_counts = mint_counts(&harness);
        assert_eq!(
            after_counts, before_counts,
            "case {case}: a refused write must mint no identifier; the store's \
             (channelIds, revisions) counters moved from {before_counts:?} to {after_counts:?}"
        );
        let after_configurations = configurations(&harness, case, &groups).await;
        assert_eq!(
            after_configurations, before_configurations,
            "case {case}: a refused write must leave every stored configuration unchanged, \
             got {after_configurations:?}"
        );
        let after_revision = harness.stored_policy_revision(policied);
        assert_eq!(
            after_revision, before_revision,
            "case {case}: a refused write must leave the stored policy revision unchanged, \
             got {after_revision:?}"
        );
        assert_eq!(
            after_revision,
            Some(policy_revision.clone()),
            "case {case}: {policied} must still carry the revision its one applied write \
             minted, got {after_revision:?}"
        );
    }

    // The counter every refusal above left alone is still the one the next APPLIED write
    // draws from: one channel the request names no id for consumes exactly one id, however
    // many refused requests came before it.
    let before_counts = mint_counts(&harness);
    let applied = add(
        &harness,
        bare,
        &[channel_body(None, &topic("after-the-refusals"))],
    )
    .await;
    assert_eq!(
        applied.status,
        200,
        "case after the refusals: the add must succeed, got {} / {}",
        applied.status,
        String::from_utf8_lossy(&applied.body)
    );
    let after_counts = mint_counts(&harness);
    assert_eq!(
        after_counts,
        (before_counts.0 + 1, before_counts.1),
        "case after the refusals: an applied add of one unnamed channel must mint exactly \
         one channel id and no revision, got {after_counts:?} from {before_counts:?}"
    );
    let reported = reported_ids("after the refusals", &applied);
    assert_eq!(
        reported.len(),
        1,
        "case after the refusals: {bare} must hold exactly the channel just added, got \
         {reported:?}"
    );
    assert!(
        is_channel_id(&reported[0]),
        "case after the refusals: the stored channel must carry a minted ChannelId, got {}",
        reported[0]
    );
}

#[tokio::test]
async fn an_applied_add_mints_one_id_per_channel_the_request_does_not_name() {
    let harness = Harness::new();
    // Each row: the ids the request names for its channels (`None` leaves one to the
    // service) and how many ids the store must therefore mint.
    let rows: [(&str, &'static str, Vec<Option<&str>>, u64); 4] = [
        (
            "one caller-named channel",
            "applied-named",
            vec![Some(NAMED_ID)],
            0,
        ),
        (
            "one channel the service names",
            "applied-minted",
            vec![None],
            1,
        ),
        (
            "two channels the service names",
            "applied-both-minted",
            vec![None, None],
            2,
        ),
        (
            "one caller-named channel and one the service names",
            "applied-mixed",
            vec![Some(NAMED_ID), None],
            1,
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, group, named, mints) in rows {
        create_group(&harness, group).await;
        let channels: Vec<serde_json::Value> = named
            .iter()
            .enumerate()
            .map(|(index, id)| channel_body(*id, &topic(&format!("{group}-{index}"))))
            .collect();

        let before_counts = mint_counts(&harness);
        let answer = add(&harness, group, &channels).await;
        assert_eq!(
            answer.status,
            200,
            "case {case}: the add must succeed, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let after_counts = mint_counts(&harness);
        assert_eq!(
            after_counts,
            (before_counts.0 + mints, before_counts.1),
            "case {case}: the add must mint exactly {mints} channel id(s) and no revision, \
             got {after_counts:?} from {before_counts:?}"
        );

        // Read back through `GetNotificationConfiguration`: what the add reported is what
        // the store holds, the caller's ids kept and every minted one modelled-shaped.
        let stored = reported_ids(case, &get(&harness, group).await);
        assert_eq!(
            stored,
            reported_ids(case, &answer),
            "case {case}: a read must report the configuration the add reported"
        );
        assert_eq!(
            stored.len(),
            named.len(),
            "case {case}: every requested channel must be stored, got {stored:?}"
        );
        for (id, requested) in stored.iter().zip(&named) {
            match requested {
                Some(requested) => assert_eq!(
                    id, requested,
                    "case {case}: a channel must keep the id the request named, got {id}"
                ),
                None => {
                    assert!(
                        is_channel_id(id),
                        "case {case}: a channel the request named no id for must carry a \
                         minted ChannelId, got {id}"
                    );
                    minted.push(id.clone());
                }
            }
        }
    }

    let total = minted.len();
    minted.sort_unstable();
    minted.dedup();
    assert_eq!(
        minted.len(),
        total,
        "case minted ids: minting only for an applied write must still hand out distinct \
         ids, got {minted:?}"
    );
}

#[tokio::test]
async fn an_applied_policy_write_mints_one_revision_and_a_refused_one_mints_none() {
    let harness = Harness::new();
    let group = "mint-policy-sequence";
    create_group(&harness, group).await;

    // The first write: no revision to name, and one revision minted for it.
    let before_counts = mint_counts(&harness);
    let first = put_permission(&harness, group, ACTION_GROUP, &[principal()], None).await;
    assert_eq!(
        first.status,
        200,
        "case first write: expected 200, got {} / {}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    let first_revision = string_member(&first, "revisionId");
    let after_first = mint_counts(&harness);
    assert_eq!(
        after_first,
        (before_counts.0, before_counts.1 + 1),
        "case first write: an applied policy write must mint exactly one revision and no \
         channel id, got {after_first:?} from {before_counts:?}"
    );

    // A write against a revision the policy has moved past stores nothing, so it mints
    // nothing.
    let stale = put_permission(
        &harness,
        group,
        ACTION_GROUP,
        &[principal()],
        Some(FOREIGN_REVISION),
    )
    .await;
    assert_eq!(
        stale.status,
        409,
        "case stale write: expected 409, got {} / {}",
        stale.status,
        String::from_utf8_lossy(&stale.body)
    );
    assert_eq!(
        mint_counts(&harness),
        after_first,
        "case stale write: a refused policy write must mint no revision, got {:?}",
        mint_counts(&harness)
    );

    // The write that names the stored revision is applied, and mints exactly one more.
    let second = put_permission(
        &harness,
        group,
        ACTION_GROUP,
        &[principal()],
        Some(&first_revision),
    )
    .await;
    assert_eq!(
        second.status,
        200,
        "case second write: expected 200, got {} / {}",
        second.status,
        String::from_utf8_lossy(&second.body)
    );
    let second_revision = string_member(&second, "revisionId");
    assert_ne!(
        second_revision, first_revision,
        "case second write: an applied write must carry a fresh revision, got \
         {second_revision}"
    );
    let after_second = mint_counts(&harness);
    assert_eq!(
        after_second,
        (after_first.0, after_first.1 + 1),
        "case second write: an applied policy write must mint exactly one revision, got \
         {after_second:?} from {after_first:?}"
    );

    // Removing the policy's LAST statement DELETES the policy — a policy exists only
    // while it holds a statement — so there is no policy left to be at a revision: the
    // removal mints none, reports neither member, and the group stores none afterwards.
    let removed = remove_permission(&harness, group, ACTION_GROUP, &second_revision).await;
    assert_eq!(
        removed.status,
        200,
        "case emptying removal: expected 200, got {} / {}",
        removed.status,
        String::from_utf8_lossy(&removed.body)
    );
    assert_eq!(
        removed.json(),
        serde_json::json!({}),
        "case emptying removal: the removal deleted the policy, so it must report neither \
         policy nor revisionId, got {}",
        String::from_utf8_lossy(&removed.body)
    );
    let after_removal = mint_counts(&harness);
    assert_eq!(
        after_removal, after_second,
        "case emptying removal: a removal that deleted the policy must mint no revision, \
         got {after_removal:?} from {after_second:?}"
    );
    assert_eq!(
        harness.stored_policy_revision(group),
        None,
        "case emptying removal: the removal took the last statement, so the group must \
         store no policy, got {:?}",
        harness.stored_policy_revision(group)
    );

    // ...and the counter that removal left alone is still the one the next APPLIED write
    // draws from: the group is policy-less, so that write is a first grant again.
    let regranted = put_permission(&harness, group, ACTION_GROUP, &[principal()], None).await;
    assert_eq!(
        regranted.status,
        200,
        "case re-grant: expected 200, got {} / {}",
        regranted.status,
        String::from_utf8_lossy(&regranted.body)
    );
    let regrant_revision = string_member(&regranted, "revisionId");
    assert_ne!(
        regrant_revision, second_revision,
        "case re-grant: an applied write must carry a fresh revision, got {regrant_revision}"
    );
    let after_regrant = mint_counts(&harness);
    assert_eq!(
        after_regrant,
        (after_removal.0, after_removal.1 + 1),
        "case re-grant: an applied policy write must mint exactly one revision, got \
         {after_regrant:?} from {after_removal:?}"
    );
}
