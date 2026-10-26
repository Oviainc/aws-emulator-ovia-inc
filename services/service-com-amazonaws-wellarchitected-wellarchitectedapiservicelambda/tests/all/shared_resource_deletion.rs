// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a share of a Well-Architected Tool resource leaves
//! behind when the SHARED RESOURCE is deleted.
//!
//! The rule pinned here, over a table that spans every resource kind this service shares
//! and deletes rather than one operation: a share grants access TO a resource and the
//! invitation it raises is the offer of that access, so neither can outlive the resource —
//! deleting a shared resource drops every share recorded against it, the create token that
//! resolved into each of those shares, and the invitations those shares raised, while a
//! resource the delete did not name keeps all three. The consequences the rows assert:
//!
//! 1. The store holds no share of a resource that is gone, so a repeated
//!    create / share / delete cycle accumulates neither share records nor remembered
//!    create tokens — what the per-resource share quota meters stays 1:1 with live grants.
//! 2. `ListShareInvitations` stops reporting the invitation a dropped share raised, and
//!    `UpdateShareInvitation` answers that invitation id with the lookup's 404; the
//!    invitation of a share of another resource is untouched.
//! 3. `ListWorkloadShares` / `ListProfileShares` / `ListTemplateShares` still report every
//!    share of the resource the delete did not name.
//!
//! The rows differ along the resource kind (`WORKLOAD`, `PROFILE` and `TEMPLATE` — the
//! three kinds whose delete is implemented; a custom lens is shared the same way but
//! `DeleteLens` is still a stub with its own issue), along how many shares the deleted
//! resource held, and along the kind of principal each share was granted to.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the resource builders, the per-kind share requests and the
//! share seeds are the ones `super::workload_harness`, `super::profile_shares`,
//! `super::template_shares` and `super::share_invitations` already spell. The principals the
//! rows share with are `super::profile_shares::OTHER_PRINCIPALS`: `SharedWith` is "The Amazon
//! Web Services account ID, organization ID, or organizational unit (OU) ID with which the
//! workload, lens, profile, or review template is shared", so one catalog spans every kind's
//! rows. The store is read only for the records the HTTP surface of a DELETED resource can
//! no longer reach (its shares answer a 404 once the resource is gone), exactly as
//! `super::profile_share_deletion` and `super::template_share_deletion` do.

use serde_json::{json, Value};

use super::profile_shares::{self, OTHER_PRINCIPALS};
use super::share_invitations::{invitation_ids, invitations};
use super::template_shares;
use super::workload_harness::{
    created_id, created_profile, member, minimal_create, review_template, Answer, Client,
};

/// The `ShareStatus` a share waits on its invitation in, and the only status
/// `ListShareInvitations` reports an invitation for — `PENDING`
/// (`smithy_query(query="shape", shape="ShareStatus")`). Spelled as the literal the model
/// publishes rather than read back out of the code under test.
const PENDING: &str = "PENDING";

/// The `PermissionType` a workload share grants; both published members are pinned by
/// `super::workload_share_permissions`, so the rows here hold it fixed and vary the things
/// this rule generalises over instead.
const WORKLOAD_PERMISSION: &str = "READONLY";

/// One resource kind this service shares AND deletes. `ShareResourceType` publishes four
/// members (`smithy_query(query="shape", shape="ShareResourceType")`); `LENS` is absent
/// here because `DeleteLens` is still a stub, so a lens cannot be deleted over the wire at
/// all.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Workload,
    Profile,
    Template,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Workload, Kind::Profile, Kind::Template];

    /// The `ShareResourceType` member for this kind — also the case label's stem.
    fn resource_type(self) -> &'static str {
        match self {
            Kind::Workload => "WORKLOAD",
            Kind::Profile => "PROFILE",
            Kind::Template => "TEMPLATE",
        }
    }

    /// Create a resource of this kind over HTTP and answer the key its shares are kept
    /// under: the `WorkloadId`, the `ProfileArn` or the `TemplateArn`.
    async fn create(self, client: &Client, name: &str) -> String {
        match self {
            Kind::Workload => created_id(
                &client.create(minimal_create(name, &format!("tok-{name}"))).await,
                name,
            ),
            Kind::Profile => created_profile(client, name).await.0,
            Kind::Template => review_template(client, name, &["wellarchitected"]).await,
        }
    }

    /// Grant one share of `key` to `shared_with` over HTTP, under the create token `token`.
    async fn grant(self, client: &Client, key: &str, shared_with: &str, token: &str) -> Answer {
        match self {
            Kind::Workload => {
                client
                    .create_share(
                        key,
                        json!({
                            "SharedWith": shared_with,
                            "PermissionType": WORKLOAD_PERMISSION,
                            "ClientRequestToken": token,
                        }),
                    )
                    .await
            }
            Kind::Profile => profile_shares::create_share(client, key, shared_with, token).await,
            Kind::Template => template_shares::create_share(client, key, shared_with, token).await,
        }
    }

    /// The `ShareId` a successful grant answered with, checked against the modelled
    /// `^[0-9a-f]{32}$` pattern. The profile and review-template checks are the ones their
    /// own suites spell; the workload one is checked here because no suite shares it.
    fn granted_share_id(self, answer: &Answer, case: &str) -> String {
        match self {
            Kind::Workload => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected CreateWorkloadShare 200, got {answer:?}"
                );
                let id = member(answer, "ShareId").to_string();
                assert!(
                    id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                    "case {case:?}: ShareId must match ^[0-9a-f]{{32}}$, got {id:?}"
                );
                id
            }
            Kind::Profile => profile_shares::created_share_id(answer, case),
            Kind::Template => template_shares::created_share_id(answer, case),
        }
    }

    /// Record a `PENDING` share of `key` addressed to the CALLING principal and answer its
    /// `(ShareId, ShareInvitationId)`.
    ///
    /// Seeded rather than granted over HTTP for the reason each kind's own seed helper
    /// gives: an invitation is offered to the principal the resource was shared WITH, and a
    /// share whose recipient is the caller has no HTTP path in a single-account emulator —
    /// so this is the only share whose invitation `ListShareInvitations` can report to this
    /// caller. The reads that follow still cross the HTTP boundary.
    fn record_share_to_caller(self, client: &Client, key: &str) -> (String, String) {
        let caller = engine::emulator_account_id();
        match self {
            Kind::Workload => {
                client.record_workload_share(key, caller, WORKLOAD_PERMISSION, PENDING)
            }
            Kind::Profile => profile_shares::record_profile_share(client, key, caller, PENDING),
            Kind::Template => template_shares::record_template_share(client, key, caller, PENDING),
        }
    }

    /// Every `ShareId` the store still holds against `key`. Read from the store because a
    /// deleted resource's shares have no HTTP reader left: the listing is scoped to the
    /// resource, which answers 404 once it is gone.
    fn stored_share_ids(self, client: &Client, key: &str) -> Vec<String> {
        let handle = client.store();
        let store = handle.lock();
        match self {
            Kind::Workload => store
                .workload_shares_of(key)
                .iter()
                .map(|share| share.share_id.clone())
                .collect(),
            Kind::Profile => store
                .profile_shares_of(key)
                .iter()
                .map(|share| share.share_id.clone())
                .collect(),
            Kind::Template => store
                .template_shares_of(key)
                .iter()
                .map(|share| share.share_id.clone())
                .collect(),
        }
    }

    /// Whether this kind's create-share ledger still resolves `token`.
    fn remembers_grant(self, client: &Client, token: &str) -> bool {
        let handle = client.store();
        let store = handle.lock();
        match self {
            Kind::Workload => store.create_workload_share_tokens.get(token).is_some(),
            Kind::Profile => store.create_profile_share_tokens.get(token).is_some(),
            Kind::Template => store.create_template_share_tokens.get(token).is_some(),
        }
    }

    /// How many shares of this kind the WHOLE store holds, and how many create tokens its
    /// create-share ledger remembers — the two things that must not accumulate over a
    /// create / share / delete cycle.
    fn share_store_size(self, client: &Client) -> (usize, usize) {
        let handle = client.store();
        let store = handle.lock();
        match self {
            Kind::Workload => (
                store.workload_shares.values().map(|of| of.len()).sum(),
                store.create_workload_share_tokens.len(),
            ),
            Kind::Profile => (
                store.profile_shares.values().map(|of| of.len()).sum(),
                store.create_profile_share_tokens.len(),
            ),
            Kind::Template => (
                store.template_shares.values().map(|of| of.len()).sum(),
                store.create_template_share_tokens.len(),
            ),
        }
    }

    /// The `ShareId`s this kind's share listing reports for `key`, over HTTP.
    async fn listed_share_ids(self, client: &Client, key: &str, case: &str) -> Vec<String> {
        let summaries = match self {
            Kind::Workload => {
                let listed = client.list_shares(key, "").await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected ListWorkloadShares 200, got {listed:?}"
                );
                listed
                    .body
                    .get("WorkloadShareSummaries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: ListWorkloadShares must report \
                             WorkloadShareSummaries, got {listed:?}"
                        )
                    })
            }
            Kind::Profile => {
                let listed = profile_shares::list_shares(client, key, "").await;
                profile_shares::summaries(&listed, case)
            }
            Kind::Template => {
                let listed = template_shares::list_shares(client, key, "").await;
                template_shares::summaries(&listed, case)
            }
        };
        summaries
            .iter()
            .map(|summary| {
                summary
                    .get("ShareId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// Delete the resource `key` under the idempotency token `token`.
    async fn delete(self, client: &Client, key: &str, token: &str) -> Answer {
        match self {
            Kind::Workload => client.delete(key, token).await,
            Kind::Profile => client.delete_profile(key, token).await,
            Kind::Template => client.delete_review_template(key, token).await,
        }
    }
}

/// The `ShareInvitationId`s the calling principal still has outstanding, over HTTP.
async fn outstanding(client: &Client, case: &str) -> Vec<String> {
    let listed = client.list_share_invitations("").await;
    invitation_ids(&invitations(&listed, case))
}

/// Consequences 2 and 3, plus the create tokens of consequence 1: deleting a shared
/// resource takes its shares, their create tokens and their invitations, and leaves another
/// resource's alone. The rows cross every deletable kind with how many shares the deleted
/// resource held — none granted over the wire, one, and several, each to a different kind of
/// principal.
#[tokio::test]
async fn deleting_a_shared_resource_drops_its_shares_tokens_and_invitations() {
    for kind in Kind::ALL {
        for granted in [0usize, 1, 3] {
            let case = format!("{}-{granted}-granted", kind.resource_type().to_lowercase());
            let client = Client::new();

            let doomed = kind.create(&client, &format!("doomed-{case}")).await;
            let mut grants: Vec<(String, String)> = Vec::new();
            for (label, principal) in OTHER_PRINCIPALS.iter().take(granted) {
                let token = format!("grant-{case}-{label}");
                let answer = kind.grant(&client, &doomed, principal, &token).await;
                let share_id = kind.granted_share_id(&answer, &format!("{case}/{label}"));
                grants.push((token, share_id));
            }
            // The one share whose invitation this caller can be offered, so the invitation
            // side of the rule is observable over HTTP.
            let (seeded_share, seeded_invitation) = kind.record_share_to_caller(&client, &doomed);

            // A second resource of the same kind, shared the same way: the delete must not
            // touch anything of ITS.
            let kept = kind.create(&client, &format!("kept-{case}")).await;
            let kept_token = format!("keep-{case}");
            let kept_answer = kind
                .grant(&client, &kept, OTHER_PRINCIPALS[0].1, &kept_token)
                .await;
            let kept_share = kind.granted_share_id(&kept_answer, &format!("{case}/kept"));
            let (kept_seeded_share, kept_invitation) = kind.record_share_to_caller(&client, &kept);

            // Before the delete: every share, token and invitation is in place.
            let mut expected: Vec<String> = grants
                .iter()
                .map(|(_, share_id)| share_id.clone())
                .collect();
            expected.push(seeded_share.clone());
            expected.sort();
            let mut held = kind.stored_share_ids(&client, &doomed);
            held.sort();
            assert_eq!(
                held, expected,
                "case {case:?}: every grant must be recorded before the delete"
            );
            for (token, share_id) in &grants {
                assert!(
                    kind.remembers_grant(&client, token),
                    "case {case:?}: the create token of share {share_id:?} must be remembered \
                     before the delete"
                );
            }
            let before = outstanding(&client, &case).await;
            assert!(
                before.contains(&seeded_invitation) && before.contains(&kept_invitation),
                "case {case:?}: both pending invitations must be outstanding before the \
                 delete, got {before:?}"
            );

            let deleted = kind.delete(&client, &doomed, &format!("del-{case}")).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected the delete of the shared resource to answer 200, \
                 got {deleted:?}"
            );

            // A share does not outlive the resource it shares, and neither does the create
            // token that resolved into it.
            assert_eq!(
                kind.stored_share_ids(&client, &doomed),
                Vec::<String>::new(),
                "case {case:?}: the deleted resource's shares must go with it"
            );
            for (token, share_id) in &grants {
                assert!(
                    !kind.remembers_grant(&client, token),
                    "case {case:?}: the create token of share {share_id:?} must go with the \
                     share it resolved to"
                );
            }

            // Nor does the invitation the share raised: it stops being reported, and the id
            // resolves to nothing for the principal it was offered to.
            let after = outstanding(&client, &case).await;
            assert!(
                !after.contains(&seeded_invitation),
                "case {case:?}: the invitation of a share of the deleted resource must stop \
                 being reported, got {after:?}"
            );
            let answered = client
                .update_share_invitation(
                    &seeded_invitation,
                    json!({ "ShareInvitationAction": "ACCEPT" }),
                )
                .await;
            assert_eq!(
                (answered.status, answered.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: answering the invitation of a share that is gone is the \
                 lookup's 404, got {answered:?}"
            );

            // The resource the delete did not name keeps its shares, its token and its
            // invitation.
            let mut kept_shares = kind.listed_share_ids(&client, &kept, &case).await;
            kept_shares.sort();
            let mut kept_expected = vec![kept_share.clone(), kept_seeded_share.clone()];
            kept_expected.sort();
            assert_eq!(
                kept_shares, kept_expected,
                "case {case:?}: a resource the delete did not name keeps every share of its own"
            );
            assert!(
                kind.remembers_grant(&client, &kept_token),
                "case {case:?}: the create token of another resource's share must survive"
            );
            assert!(
                after.contains(&kept_invitation),
                "case {case:?}: another resource's pending invitation must still be reported, \
                 got {after:?}"
            );
        }
    }
}

/// Consequence 1 as an invariant: a share and its create token are 1:1 with a LIVE grant,
/// so repeating create-resource / share / delete leaves the share store and the
/// create-share ledger exactly as empty as they started, however many times the cycle runs.
/// The rows are every deletable kind, over several cycles.
#[tokio::test]
async fn repeating_share_and_delete_accumulates_no_share_in_the_store() {
    /// Shares granted per cycle: enough that each cycle writes several records and tokens,
    /// while staying far below the per-resource share quota, which `super::profile_shares`
    /// and `super::template_shares` pin on their own.
    const GRANTS_PER_CYCLE: usize = 2;

    for kind in Kind::ALL {
        let client = Client::new();
        let (empty_shares, empty_tokens) = kind.share_store_size(&client);
        assert_eq!(
            (empty_shares, empty_tokens),
            (0, 0),
            "case {:?}: a fresh store holds no share and no create token",
            kind.resource_type()
        );

        for cycle in 0..3usize {
            let case = format!("{}-cycle-{cycle}", kind.resource_type().to_lowercase());
            let key = kind.create(&client, &format!("cycled-{case}")).await;
            for (label, principal) in OTHER_PRINCIPALS.iter().take(GRANTS_PER_CYCLE) {
                let answer = kind
                    .grant(&client, &key, principal, &format!("grant-{case}-{label}"))
                    .await;
                kind.granted_share_id(&answer, &format!("{case}/{label}"));
            }
            kind.record_share_to_caller(&client, &key);

            let (shares, tokens) = kind.share_store_size(&client);
            assert_eq!(
                (shares, tokens),
                (GRANTS_PER_CYCLE + 1, GRANTS_PER_CYCLE),
                "case {case:?}: the cycle's grants must be recorded before the delete"
            );

            let deleted = kind.delete(&client, &key, &format!("del-{case}")).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected the delete of the shared resource to answer 200, \
                 got {deleted:?}"
            );

            let (shares, tokens) = kind.share_store_size(&client);
            assert_eq!(
                (shares, tokens),
                (0, 0),
                "case {case:?}: no share and no create token may outlive the resource the \
                 cycle deleted"
            );
            let outstanding = outstanding(&client, &case).await;
            assert!(
                outstanding.is_empty(),
                "case {case:?}: the cycle leaves no invitation of a resource that is gone, \
                 got {outstanding:?}"
            );
        }
    }
}
