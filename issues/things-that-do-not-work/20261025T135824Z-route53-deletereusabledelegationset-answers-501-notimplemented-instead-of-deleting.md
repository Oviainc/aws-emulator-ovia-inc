Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.delegation-set-in-use-blocks-delete
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.delegation-set-in-use-blocks-delete|run#9|AssertionError|assert code == "DelegationSetInUse", code
Issue-Commit: c7b32a251ff44cc6c28a05b1a335573ec2c90dc2

# route53 DeleteReusableDelegationSet answers 501 NotImplemented instead of deleting the set or refusing with DelegationSetInUse

> _Note: this issue was identified against the code base at commit `c7b32a251ff44cc6c28a05b1a335573ec2c90dc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteReusableDelegationSet

## Description
Cause 1 of 1: `DeleteReusableDelegationSet` is an unimplemented scaffold stub, so the whole reusable-delegation-set deletion path — both its success and its in-use refusal — is absent. No filed issue covers this operation: the only related issue, 20261024T114159Z-route53-createreusabledelegationset-stub.md, is already marked fixed and named `CreateReusableDelegationSet` (which does now answer: the check successfully creates the set and the hosted zone before failing), so this is a distinct, net-new gap on the delete side.

AWS: Smithy model `com.amazonaws.route53#DeleteReusableDelegationSet` — DELETE /2013-04-01/delegationset/{Id}, success HTTP 200 with an empty `DeleteReusableDelegationSetResponse`; modeled errors `DelegationSetInUse` (HTTP 400), `DelegationSetNotReusable` (HTTP 400), `NoSuchDelegationSet` (HTTP 400), `InvalidInput` (HTTP 400). The operation doc states: "Deletes a reusable delegation set. You can delete a reusable delegation set only if it isn't associated with any hosted zones." and `DelegationSetInUse` is documented as "The specified delegation contains associated hosted zones which must be deleted before the reusable delegation set can be deleted." Recorded behavior for the three states the check exercises:
  * delegation set still referenced by a hosted zone (the zone was created with `DelegationSetId` set) -> HTTP 400, `Error.Code` = `DelegationSetInUse`, `Error.Message` = the fixed text `The specified delegation set contains associated hosted zones.` (no variable part).
  * after the referencing hosted zone is deleted, the identical call succeeds (HTTP 200) and the set is gone.
  * a `GetReusableDelegationSet`/`DeleteReusableDelegationSet` on the now-deleted id -> HTTP 400, `Error.Code` = `NoSuchDelegationSet`, and the message is just the bare delegation set id — i.e. the message is entirely a variable part taken from the request (`Id` with the `/delegationset/` prefix stripped), with no fixed text around it.
The blocked delete must also be a no-op: the set is still returned by `GetReusableDelegationSet` after the refusal.

Emulator: every `DeleteReusableDelegationSet` call answers HTTP 501 with `Error.Code` = `NotImplemented` and message `operation DeleteReusableDelegationSet is scaffolded but not implemented in this emulator`. The replay fails at `assert code == "DelegationSetInUse"` with `AssertionError: NotImplemented` — the in-use refusal never happens, and the later success path and `NoSuchDelegationSet` assertions are never reached.

Why: a client that provisions hosted zones against a reusable delegation set can never tear it down — the delete is permanently refused with a 501 that is neither modeled nor retryable, so cleanup code aborts instead of succeeding. Worse, clients that rely on `DelegationSetInUse` to detect "zones still attached" see an unmodeled error code their error handling does not match, so ordering logic (delete zones, then delete the set) cannot be driven at all.

Fix: implement the handler `handle` body of `DeleteReusableDelegationSet` — look the delegation set up in the route53 service state, refuse with the modeled `DelegationSetInUse` (400) when any hosted zone still references it leaving state untouched, refuse with the modeled `NoSuchDelegationSet` (400, bare-id message) when the id is unknown, otherwise remove it and return the empty 200 output; then drop the SCAFFOLDED marker.

Traces: failing check route53.delegation-set-in-use-blocks-delete (create reusable delegation set -> create hosted zone with that DelegationSetId -> DeleteReusableDelegationSet).

## Evidence
Expected (real AWS):
  DeleteReusableDelegationSet while a hosted zone references the set
    -> HTTP 400, Error.Code = DelegationSetInUse,
       Error.Message = "The specified delegation set contains associated hosted zones." (fixed text, no variable parts)
    -> set still present: GetReusableDelegationSet returns it (refusal is a no-op)
  After the referencing hosted zone is deleted, the same DeleteReusableDelegationSet call -> HTTP 200, empty body
  GetReusableDelegationSet / DeleteReusableDelegationSet on the deleted id
    -> HTTP 400, Error.Code = NoSuchDelegationSet, Error.Message = the bare delegation set id (variable part from the request Id, no surrounding fixed text)

Actual (emulator):
  DeleteReusableDelegationSet -> HTTP 501, Error.Code = NotImplemented,
    Error.Message = "operation DeleteReusableDelegationSet is scaffolded but not implemented in this emulator"

Replay output:
  #### [mismatch] route53.delegation-set-in-use-blocks-delete
  File "check_route53_delegation_set_in_use_blocks_delete.py", line 106, in run
    assert code == "DelegationSetInUse", code
  AssertionError: NotImplemented

Static confirmation in the checkout: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/delete_reusable_delegation_set.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4) and its handler returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteReusableDelegationSet"))` (line 158), while the modeled error enum already declares DelegationSetInUse / DelegationSetNotReusable / InvalidInput / NoSuchDelegationSet (lines 88-93).

## Steps to Reproduce
1. route53 CreateReusableDelegationSet with a fresh CallerReference; keep the returned DelegationSet.Id.
2. route53 CreateHostedZone with a unique public zone name, a fresh CallerReference and DelegationSetId set to that id.
3. Call route53 DeleteReusableDelegationSet with that Id.
   Expected: HTTP 400 DelegationSetInUse, message "The specified delegation set contains associated hosted zones."; the set is still returned by GetReusableDelegationSet.
   Actual: HTTP 501 NotImplemented, "operation DeleteReusableDelegationSet is scaffolded but not implemented in this emulator".
4. DeleteHostedZone for the zone, then repeat step 3.
   Expected: HTTP 200 and the set is deleted; a following GetReusableDelegationSet/DeleteReusableDelegationSet returns 400 NoSuchDelegationSet whose message is the bare delegation set id.
   Actual: still HTTP 501 NotImplemented.

Replay with: run_behavioral_suite, check id route53.delegation-set-in-use-blocks-delete.
