Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# DeleteProfile drops every holding workload's profile association without moving that workload's UpdatedAt, unlike DisassociateProfiles

_Filed: 2026-10-27T13:49:37Z_
_Operation: DeleteProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Deleting a profile drops the association every workload held it by — `crate::state::remove_profile` walks `self.workloads.values_mut()` and calls `workload.disassociate_profile(profile_arn)` — but it never moves those workloads' `updated_at`.

The crate states the opposite rule on the OTHER path that performs the exact same mutation: `DisassociateProfiles` drops the association with the same `WorkloadRecord::disassociate_profile` call and then writes `workload.updated_at = now`, with a comment spelling out why ("The profiles are a member of the workload (`Workload$Profiles`), so removing one changes the workload, and `Workload$UpdatedAt` … is what every read of the workload reports that change by"). `AssociateProfiles` writes it for the same reason, and so do `AssociateLenses`, `DisassociateLenses` and `UpgradeProfileVersion`.

So one of the two paths that removes a `Workload$Profiles` entry reports the change through `Workload$UpdatedAt` and the other silently does not: after `DeleteProfile`, `GetWorkload` / `ListWorkloads` report a workload whose `Profiles` member changed while the instant that is supposed to report that change stands exactly where it was. Nothing in `remove_profile`'s doc comment, nor in any test, states a reason for the divergence — its doc only explains WHICH keyed state goes with the profile ("the association every workload held it by, since an association reports the ARN and the version of a profile that has to resolve for the association to mean anything").

This is distinct from the already-FIXED DeleteProfile issue (`20261025T165933Z-wellarchitected-deleteprofile-wrong-behavior.md`, "DeleteProfile leaves the deleted profile's shares (and their create tokens) in the store"): that one was about shares and their ledgers being left behind, which `remove_profile` now purges; this one is about the holding workloads' `UpdatedAt` not being moved by the association drop.

## Evidence

The rule, on the sibling path — `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/disassociate_profiles.rs:221-227`:
```
        for arn in &input.profile_arns {
            workload.disassociate_profile(arn);
        }
        // The profiles are a member of the workload (`Workload$Profiles`), so removing one
        // changes the workload, and `Workload$UpdatedAt` — "the date and time recorded" —
        // is what every read of the workload reports that change by.
        workload.updated_at = now;
```

The same mutation without that write — `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/state.rs:7234-7251` (`pub fn remove_profile`):
```
        let record = self.profiles.remove(profile_arn)?;
        self.create_profile_tokens
            .retain(|token| token.profile_arn != profile_arn);
        for workload in self.workloads.values_mut() {
            workload.disassociate_profile(profile_arn);
        }
```
…and nothing in the rest of that function (through `state.rs:7251 Some(record)`) touches `workload.updated_at`.

The operation that calls it — `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/delete_profile.rs:219`: `if store.remove_profile(&input.profile_arn).is_none() {`.

For contrast, `AssociateProfiles` writes the same instant for the same stated reason: `src/ops/associate_profiles.rs:271-274` — "The profiles are a member of the workload (`Workload$Profiles`), so associating one changes the workload, and `Workload$UpdatedAt` … is what every read of the workload reports that change by" / `workload.updated_at = now;`.

Test coverage: `tests/all/profile_deletion.rs:355-419` (`deleting_an_associated_profile_drops_that_association`) pins that the association goes with the deleted profile ("case {case:?}/holder-{index}: the deleted profile's association must go with it") and asserts nothing at all about the holding workloads' `UpdatedAt`, so no test pins the current behaviour with a stated reason. `tests/all/workload_profile_disassociation.rs:374` pins the opposite side ("a refused request writes nothing, so the workload's UpdatedAt must not move"), i.e. that a successful disassociation does move it.

## Steps to Reproduce

1. CreateWorkload (minimal, framework lens) and CreateProfile, then AssociateProfiles to attach the profile to the workload
2. GET /workloads/{WorkloadId} and record `Workload.UpdatedAt` and `Workload.Profiles`
3. DELETE /profiles/{ProfileArn}?ClientRequestToken=tok (DeleteProfile)
4. GET /workloads/{WorkloadId} again: `Profiles` is gone, but `UpdatedAt` is byte-identical to the value recorded in step 2
5. Repeat the scenario but drop the association with PATCH /workloads/{WorkloadId}/disassociateProfiles instead: `UpdatedAt` moves, per src/ops/disassociate_profiles.rs:227
6. Compare the two: the same `Workload$Profiles` mutation is reported through `UpdatedAt` on one path and not the other
