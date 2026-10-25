Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f3b5c182376fbd512bc0d8c0af2ad2661dfcff47

# DeleteProfile leaves the deleted profile's shares (and their create tokens) in the store, unlike DeleteWorkload

_Filed: 2026-10-25T16:59:33Z_
_Operation: DeleteProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f3b5c182376fbd512bc0d8c0af2ad2661dfcff47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteProfile` removes a profile through `ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remove_profile`, whose own doc comment says it drops "a profile and everything keyed to it" and justifies dropping the workload associations with "the same reason a share does not outlive the resource it shares in [`Self::remove_workload`]". But `remove_profile` never touches `self.profile_shares` (keyed by `ProfileArn`) nor `self.create_profile_share_tokens`, while the sibling `remove_workload` explicitly drops both of its counterparts (`self.workload_shares.remove(workload_id)` and `self.create_workload_share_tokens.retain(...)`).

Consequences inside this crate:

1. Every `ProfileShareRecord` of a deleted profile stays in `profile_shares` forever. `ListProfileShares` refuses the ARN with a 404 (`list_profile_shares.rs:214-219`), so those records are unreachable but never collected — a caller can loop create-profile / create-share / delete-profile and grow `profile_shares` and `create_profile_share_tokens` without bound, which is exactly what `CreateProfileShare`'s own quota comment claims is prevented ("a share and its create token are both 1:1 with a live grant" — after a `DeleteProfile` they are 1:1 with nothing).
2. `pending_share_invitations_for` enumerates `self.profile_shares` unconditionally, so a `PENDING` profile-share invitation of a profile that no longer exists is still reported by `ListShareInvitations` (and the `ProfileName` it reports resolves from a profile that is gone).
3. The share ids and share-invitation ids of the orphaned records keep being treated as taken by the id-minting uniqueness scans that walk `profile_shares`.

The fix is the one line the sibling already has: `remove_profile` should drop the profile's share map and the create tokens that resolved into it, exactly as `remove_workload` does.

## Evidence

Sibling that DOES clean up — `src/state.rs:5912-5931` (`pub fn remove_workload`):
```
5923   // A share shares THIS workload, so it cannot outlive it either.
5924   self.workload_shares.remove(workload_id);
5925   self.create_workload_share_tokens
5926       .retain(|token| token.workload_id != workload_id);
```

This path — `src/state.rs:5954-5977` (`pub fn remove_profile`), doc and body in full:
```
5954   /// Drop a profile and everything keyed to it: any create token that resolved to it,
...
5958   /// the ARN and the version of a profile that has to resolve for the association to
5959   /// mean anything ([`WorkloadProfileRecord`]) — the same reason a share does not
5960   /// outlive the resource it shares in [`Self::remove_workload`]. ...
5966   pub fn remove_profile(&mut self, profile_arn: &str) -> Option<ProfileRecord> {
5967       let record = self.profiles.remove(profile_arn)?;
5968       self.create_profile_tokens
5969           .retain(|token| token.profile_arn != profile_arn);
5970       for workload in self.workloads.values_mut() {
5971           workload.disassociate_profile(profile_arn);
5972       }
5975       self.upgrade_profile_version_tokens
5976           .retain(|token| token.profile_arn != profile_arn);
5977       Some(record)
5978   }
```
No `self.profile_shares.remove(profile_arn)` and no `self.create_profile_share_tokens.retain(...)`. A crate-wide search for those two fields finds only the declarations (`src/state.rs:4900`, `src/state.rs:4903`), the id-uniqueness scans (`src/state.rs:5043`, `src/state.rs:5069`), the invitation enumeration (`src/state.rs:6110`) and the read helpers (`src/state.rs:6177-6205`) — nothing removes them.

The caller: `src/ops/delete_profile.rs:217-224` — "`remove_profile` takes the record and everything keyed to it with it, exactly as `DeleteWorkload` and `DeleteReviewTemplate` remove theirs." / `if store.remove_profile(&input.profile_arn).is_none() {`.

The bound this breaks: `src/ops/create_profile_share.rs:252-255` — "Shares are metered per profile, and this operation models `ServiceQuotaExceededException` for the case. Refusing here is also what keeps the store bounded: a share and its create token are both 1:1 with a live grant."

The orphaned invitations are still enumerated: `src/state.rs:6107-6114` — "A profile share raises an invitation of its own kind, reported through the same enumeration for the same reason." / `invitations.extend(self.profile_shares.values().flat_map(|shares| shares.values()).filter(|share| share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS)`.

Test coverage: `tests/all/profile_deletion.rs` contains no occurrence of `share`/`Share` at all, and `tests/all/profile_shares.rs` never calls `DeleteProfile` (grep for `DeleteProfile` across `tests/all` matches only `profile_deletion.rs`, `workload_harness.rs:436`, `delete_idempotency.rs:68` and a comment in `workload_profile_disassociation.rs:221`), so nothing pins the current leak.

## Steps to Reproduce

1. Open services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/state.rs:5912-5931 and note that remove_workload drops self.workload_shares and self.create_workload_share_tokens
2. Open src/state.rs:5954-5978 and note that remove_profile drops the create tokens and the workload associations but never self.profile_shares or self.create_profile_share_tokens, despite the doc saying 'everything keyed to it' and citing remove_workload's share rule
3. CreateProfile -> CreateProfileShare(ProfileArn, SharedWith=<another principal>) -> DeleteProfile(ProfileArn)
4. ListProfileShares(ProfileArn) now answers 404 (src/ops/list_profile_shares.rs:214-219) while the ProfileShareRecord is still held in store.profile_shares, and pending_share_invitations_for (src/state.rs:6107-6114) still enumerates its PENDING invitation
5. Repeat the create/share/delete cycle: profile_shares and create_profile_share_tokens grow with every iteration, although no live profile backs any of the entries
