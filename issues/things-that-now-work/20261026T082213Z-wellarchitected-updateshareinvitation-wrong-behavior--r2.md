Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# A review-template share invitation cannot be answered: UpdateShareInvitation resolves workload/lens/profile shares only and 404s on a TEMPLATE invitation it just listed

_Filed: 2026-10-26T08:22:13Z_
_Operation: UpdateShareInvitation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateTemplateShare` is implemented and stores review-template shares in `state.template_shares`, each carrying its own minted `ShareInvitationId`, and `ListShareInvitations` reports those invitations with `ShareResourceType=TEMPLATE` (the test suite pins this). But the resolution `UpdateShareInvitation` uses, `ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::share_by_invitation_mut`, looks in only three of the four share maps — `workload_shares`, `lens_shares`, `profile_shares` — and `ShareByInvitationMut` has only `Workload`, `Lens` and `Profile` variants (and `TemplateShareRecord` has no `to_invitation()` projection at all, only `to_summary` and `to_invitation_summary`). So a TEMPLATE invitation that `ListShareInvitations` just reported resolves to `None` and `UpdateShareInvitation` answers 404 `ResourceNotFoundException` for both `ACCEPT` and `REJECT`: a review-template share is permanently stuck in `PENDING` and can never be accepted or rejected.

This contradicts the handler's own stated rule and the state helper's own stated invariant. The doc comment on `share_by_invitation_mut` still justifies the omission with a claim that is no longer true — that this store does not hold review-template shares because `CreateTemplateShare` is a stub — while `create_template_share.rs` is a fully implemented handler that inserts into `store.template_shares` and whose own comment says the share it creates "is what `ListShareInvitations` reports to the principal offered the share".

This is the same defect class as the already-FIXED lens-invitation issue (20261024T151752Z-wellarchitected-updateshareinvitation-wrong-behavior), which added the `Lens` arm; the TEMPLATE arm was left out and became reachable when `CreateTemplateShare` stopped being a stub. `DeleteTemplateShare` is a scaffold stub, so there is no other way to clear such a share either.

## Evidence

The listing side reports TEMPLATE invitations — `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/state.rs:7227-7246`:
```
// A review-template share raises an invitation of its own kind, reported through
// the same enumeration for the same reason.
invitations.extend(
    self.template_shares
        .values()
        .flat_map(|shares| shares.values())
        .filter(|share| {
            share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS
        })
```

The answering side does NOT — `src/state.rs:7426-7460` looks in `self.workload_shares`, `self.lens_shares`, then `self.profile_shares` and returns `?`/`None` after the profile pass; there is no `template_shares` arm. The enum it answers with, `src/state.rs:4191-4195`:
```
pub enum ShareByInvitationMut<'a> {
    Workload(&'a mut WorkloadShareRecord),
    Lens(&'a mut LensShareRecord),
    Profile(&'a mut ProfileShareRecord),
}
```

The invariant the helper claims, with a stale justification — `src/state.rs:7404-7409`:
```
/// across every share this store holds — exactly the shares
/// [`Self::pending_share_invitations_for`] reports invitations for, so the two sides
/// of one invitation cannot disagree about whether it exists. A kind whose shares this
/// store does not hold yet (a review-template share, whose create is a stub with its
/// own issue) joins both sides by being stored: it is then one more arm here and one
/// more map to look in, with the same two conditions.
```
That claim is contradicted in this same crate by `src/ops/create_template_share.rs:296-312`, an implemented handler: `let share_invitation_id = store.mint_share_invitation_id();` … `store.template_shares.entry(input.template_arn.clone()).or_default().insert(share_id.clone(), record);` — preceded by `create_template_share.rs:290-295`: "Creating the share is what raises the invitation the recipient has to answer … it is what `ListShareInvitations` reports to the principal offered the share."

The handler rule the code breaks — `src/ops/update_share_invitation.rs:188-190`:
```
// named by its `ShareInvitationId` alone, so it is resolved across every stored
// share whatever resource it shares — the same shares `ListShareInvitations`
// reports invitations for, so an invitation that was listed can be answered.
```
followed by `src/ops/update_share_invitation.rs:197-205`, where an unresolved invitation becomes a 404:
```
let Some(mut share) = store.share_by_invitation_mut(
    &input.share_invitation_id,
    ::engine::emulator_account_id(),
) else {
    return Err(UpdateShareInvitationErrorModeled::ResourceNotFoundException(
        crate::state::share_invitation_not_found(&input.share_invitation_id),
```

The model supports the kind: `ShareInvitation` declares `ShareResourceType` (enum `LENS | PROFILE | TEMPLATE | WORKLOAD`) and `TemplateArn` — "<p>The review template ARN.</p>" (smithy_query shape=ShareInvitation).

Test coverage proving the invitation is listed (and that nothing pins the 404 as intended): `tests/all/template_shares.rs:983-1060`, `a_template_share_raises_a_template_invitation_for_the_principal_it_was_offered_to`, asserts `list_share_invitations("ShareResourceType=TEMPLATE")` reports one invitation per shared template carrying `TemplateArn`/`TemplateName`. No test in `tests/all/share_invitations.rs` or `tests/all/share_invitation_answers.rs` answers a TEMPLATE invitation; `tests/all/share_invitations.rs:358-363` still carries the stale comment "`CreateProfileShare` and `CreateTemplateShare` are still stubs with their own issues".

## Steps to Reproduce

1. CreateReviewTemplate to get a TemplateArn
2. CreateTemplateShare on that TemplateArn with SharedWith = the caller's own principal seeded as recipient (as tests/all/template_shares.rs::record_template_share does) so the invitation is offered to the caller
3. ListShareInvitations?ShareResourceType=TEMPLATE — the invitation is reported with its ShareInvitationId, TemplateArn and TemplateName
4. PATCH /shareInvitations/{that ShareInvitationId} with ShareInvitationAction=ACCEPT
5. Observed: HTTP 404 ResourceNotFoundException (share_invitation_not_found) because share_by_invitation_mut never looks in state.template_shares; the share stays PENDING forever and ListTemplateShares keeps reporting Status=PENDING
6. Compare with the same call against a LENS or PROFILE invitation, which is resolved and moved to ACCEPTED by src/state.rs:7438-7459
