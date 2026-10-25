Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# A custom-lens share invitation cannot be answered: UpdateShareInvitation resolves workload shares only and 404s on a LENS invitation it just listed

_Filed: 2026-10-24T15:17:52Z_
_Operation: UpdateShareInvitation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateShareInvitation` is documented as "Update a workload **or custom lens** share invitation", and the emulator fully supports the lens-share half of that lifecycle: `CreateLensShare` records a `LensShareRecord` with its own minted `ShareInvitationId` in status `PENDING`, and `ListShareInvitations` reports that invitation to the recipient (`state::pending_share_invitations_for` explicitly extends the enumeration with `self.lens_shares`, and `ShareInvitationSummary$ShareResourceType` is reported as `LENS` with the lens's name).

But the answer path only looks at workload shares. The handler resolves the invitation through `store.workload_share_by_invitation_mut(&input.share_invitation_id, account)`, which iterates `self.workload_shares` and nothing else, so a `ShareInvitationId` that belongs to a LENS share resolves to `None`.

Expected (real service): `PATCH /shareInvitations/{ShareInvitationId}` with `ShareInvitationAction=ACCEPT` (or `REJECT`) on a lens share invitation answers HTTP 200 with the `ShareInvitation` (carrying `ShareResourceType=LENS` plus `LensAlias`/`LensArn`), moves the underlying lens share out of `PENDING` into `ACCEPTED`/`REJECTED`, and drops the invitation from `ListShareInvitations`.

Actual: HTTP 404 `ResourceNotFoundException` for the very id `ListShareInvitations` just handed the caller; the lens share stays `PENDING` forever (nothing else can move it — `DeleteLensShare` is a stub), so the lens-sharing workflow has no terminal state and the `LENS` branch of `ShareResourceType` is unreachable past the invitation listing.

## Evidence

Spec: `smithy_query(query="doc", shape="UpdateShareInvitation")` = "Update a workload or custom lens share invitation. This API operation can be called independently of any resource."; `ShareInvitation`/`ShareInvitationSummary` publish `ShareResourceType` (`WORKLOAD | LENS | PROFILE | TEMPLATE`) together with `LensAlias`/`LensArn`, and `ShareInvitationAction` publishes `ACCEPT|REJECT` mapping onto `ShareStatus` `ACCEPTED|REJECTED`.

Emulator: `src/ops/update_share_invitation.rs` lines 193-217 resolve the invitation exclusively via `crate::state::...::workload_share_by_invitation_mut`, whose body (src/state.rs, `pub fn workload_share_by_invitation_mut`) walks `self.workload_shares.values_mut()` only; `src/state.rs` `pending_share_invitations_for` (same file) deliberately also enumerates `self.lens_shares` for the listing, so the two sides of the same invitation disagree. `src/ops/create_lens_share.rs` lines 315-338 mint the lens share's `share_invitation_id` and store status `PENDING`.

Test coverage: `tests/all/share_invitation_answers.rs` covers WORKLOAD invitations only (its header says "the resource kind `WORKLOAD`" and `OTHER_KIND_MEMBERS` asserts `LensAlias`/`LensArn` are ABSENT from the answered invitation); `tests/all/lens_shares.rs` stops at asserting that the lens invitation is listed. No test answers a lens invitation, so nothing pins the current 404.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. PUT /importLens with a valid lens JSON document and note the returned LensArn
3. POST /lenses/{LensArn}/versions (CreateLensVersion, LensVersion="1.0") so the lens is publishable/shareable
4. POST /lenses/{LensArn}/shares (CreateLensShare) with SharedWith=<another 12-digit account id> and note the returned ShareId
5. GET /shareInvitations and observe the invitation with ShareResourceType=LENS; note its ShareInvitationId
6. PATCH /shareInvitations/{ShareInvitationId} with body {"ShareInvitationAction":"ACCEPT"}
7. Observe HTTP 404 ResourceNotFoundException instead of 200 with the lens ShareInvitation; GET /lenses/{LensArn}/shares (or re-list invitations) and observe the share is still PENDING
