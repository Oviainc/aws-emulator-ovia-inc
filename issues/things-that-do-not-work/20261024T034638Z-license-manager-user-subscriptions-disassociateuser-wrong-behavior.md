Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Disassociated user vanishes instantly instead of being observable in DISASSOCIATING

_Filed: 2026-10-24T03:46:38Z_
_Operation: DisassociateUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DisassociateUser` removes the association from the store synchronously (`state.remove_instance_user(&arn)`), then stamps `status = DISASSOCIATING` and a `DisassociationDate` on the detached copy it returns.

Expected: disassociation is asynchronous (the instance has to be reconfigured), which is why the call answers the non-terminal `DISASSOCIATING` rather than a terminal status. After the call, the association must remain observable in `DISASSOCIATING` — and the instance must remain in the service's instance view — until the teardown completes; a re-`AssociateUser` for the same (IdentityProvider, InstanceId, Username) while the previous association is still disassociating is a conflict, not a fresh association. The emulator already models the mirror-image `ASSOCIATING -> ASSOCIATED` transition through `schedule_transition`/`materialise_due`.

Actual: the single disassociate response is the only place `DISASSOCIATING` is ever observable. `refresh_instance_view` drops the instance from `ListInstances` in the same call when it was the last association, and a repeat `AssociateUser` immediately mints a brand-new association ARN as if nothing had been torn down. A client that polls for the disassociation to finish sees the state vanish between two reads with no transitional status.

## Evidence

Spec: `InstanceUserSummary$Status` ("The status of a user associated with an EC2 instance") plus `DisassociationDate`, and the handler's own choice of the non-terminal wording DISASSOCIATING (`src/state.rs`: "The status returned by `DisassociateUser` for the association it ended") — a status that is meaningless unless the resource is observable in it; the same reasoning is already filed and accepted for DeleteLicenseServerEndpoint/DELETING. Handler: `src/ops/disassociate_user.rs` lines 180-204; state: `src/state.rs::remove_instance_user` drops the record, the order list, the index and the tags in one step and `refresh_instance_view` immediately recomputes (or removes) the instance, while `materialise_due` only ever transitions ASSOCIATING -> ASSOCIATED. Tests: `tests/all/resource_resolution_refusals.rs` (lines 932-971) asserts the reply says DISASSOCIATING and that the same identifiers then fail closed; nothing asserts a DISASSOCIATING association, or its instance, is still reported afterwards.

## Steps to Reproduce

1. RegisterIdentityProvider for an Active Directory provider, then AssociateUser for a Username on an instance the account holds; wait until the association reports ASSOCIATED.
2. Call DisassociateUser with that InstanceUserArn — the reply carries InstanceUserSummary.Status=DISASSOCIATING and a DisassociationDate.
3. Immediately call ListInstances: the instance has already left the view (when that was its last association). Expected: the instance stays in view while the association is still DISASSOCIATING.
4. Immediately call AssociateUser again with the same Username, InstanceId and IdentityProvider: a brand-new association ARN is minted in ASSOCIATING. Expected: a conflict (or the stored association still in DISASSOCIATING) until the teardown completes.
