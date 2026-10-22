Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wellarchitected.share-with-own-account-rejected
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected.share-with-own-account-rejected|run#6|AssertionError|assert code == "ValidationException", (
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# CreateWorkloadShare is unimplemented: self-share is not rejected and no share is ever created

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkloadShare

## Description
1 cause, in service wellarchitected.

Cause 1 of 1: CreateWorkloadShare answers 501 NotImplemented for every request, so neither its rejection path nor its success path exists.

AWS: Smithy CreateWorkloadShare is POST /workloads/{WorkloadId}/shares, success code 200, input WorkloadId (@required, @httpLabel, @length 32..32, @pattern ^[0-9a-f]{32}$), SharedWith (@required, @length 12..2048), PermissionType (@required, enum PermissionType = CONTRIBUTOR | READONLY), ClientRequestToken (@required, @idempotencyToken, @length 1..2048); output members WorkloadId and ShareId (@pattern ^[0-9a-f]{32}$); modeled errors include ValidationException @error("client") @httpError(400) ("The user input is not valid"). Recorded behavior: sharing a workload with the CALLER'S OWN principal (the account id returned by sts:GetCallerIdentity, i.e. the workload's owning account) is refused with HTTP 400 ValidationException — the refusal is decided against the caller's identity, not against a literal account id — and the rejected attempt creates no share. The same request shape with a different principal is accepted and returns a ShareId that then appears in ListWorkloadShares. Recorded message for the self-share refusal, as an EXAMPLE of text built from the request: '[Validation] The shared with principal cannot be the same as the sharer's principal.' The 400 status and the ValidationException error code are the requirements.

Emulator: every CreateWorkloadShare call returns HTTP 501 with error code 'NotImplemented' and message 'operation CreateWorkloadShare is scaffolded but not implemented in this emulator'. The handler at services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/create_workload_share.rs still carries the SCAFFOLDED marker and its handle() body returns crate::scaffolded::Scaffolded::not_implemented("CreateWorkloadShare"). Consequently the self-share case surfaces 'NotImplemented' where ValidationException is expected, no ShareId is ever minted, and no share state is recorded.

Why: a client cannot share a workload at all against this emulator — sharing flows fail with a non-modeled 501 that no SDK retry/error-handling branch understands (ValidationException is a modeled client error; NotImplemented is not in the operation's error set). Validation-driven clients that expect a 400 for a self-share see a 5xx and may treat it as a transient server fault and retry forever, and any test that asserts a workload has no shares after a rejected attempt cannot run.

Fix: implement the CreateWorkloadShare handler (its validation, its state mutation and its serializer) in services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/create_workload_share.rs: compare SharedWith against the caller's own account/owning principal and return a 400 ValidationException (with the required Message populated) without mutating state, otherwise create the share, mint a ShareId matching ^[0-9a-f]{32}$, record it in the workload's share state so ListWorkloadShares reports it, and return {WorkloadId, ShareId}. Delete the SCAFFOLDED marker line once done. NOTE: this is related to but not covered by open issue 20261021T235127Z-wellarchitected-validationexception-envelope-and-input-validation-diverge-across-wellarchitected.md, which covers the ValidationException envelope on CreateWorkload/UpdateAnswer/CreateMilestone/CreateReviewTemplate/ListWorkloads/ListMilestones and names no share operation; and it is a different operation from the already-filed unimplemented-stub issues for ListWorkloadShares, ListAnswers, ImportLens, ListMilestones and CreateReviewTemplate.

Traces: behavioral check wellarchitected.share-with-own-account-rejected (mismatch: "sharing with the sharer's own principal failed with 'NotImplemented', expected ValidationException").

## Evidence
Behavioral suite simparity-behavioral-aws-v1, emulator booted and serving (1/12 checks passed; wellarchitected 1/12).

[mismatch] wellarchitected.share-with-own-account-rejected
  check_wellarchitected_share_with_own_account_rejected.py line 86:
  assert code == "ValidationException"
  AssertionError: sharing with the sharer's own principal failed with 'NotImplemented', expected ValidationException

Sequence exercised by the check: CreateWorkload (succeeds) -> CreateWorkloadShare(WorkloadId=<new workload>, SharedWith=<own account from sts:GetCallerIdentity>, PermissionType='READONLY', ClientRequestToken=<uuid>).
  Expected: HTTP 400, error code ValidationException (example message '[Validation] The shared with principal cannot be the same as the sharer's principal.'), and no share left on the workload.
  Actual: HTTP 501, error code 'NotImplemented', message 'operation CreateWorkloadShare is scaffolded but not implemented in this emulator'.

The same 501 blocks the check's downstream assertions, which never ran: the positive control CreateWorkloadShare with a different principal (expects 200 + a ShareId matching ^[0-9a-f]{32}$ that becomes the workload's only share) and the unknown-PermissionType rejection (expects 400 ValidationException against enum PermissionType = CONTRIBUTOR | READONLY).

Source confirmation: services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/create_workload_share.rs line 4 ('//! SCAFFOLDED, NOT IMPLEMENTED') and line 186 (Err(crate::scaffolded::Scaffolded::not_implemented("CreateWorkloadShare"))).

Status 400 and error code ValidationException are the requirements; the quoted messages are examples.

## Steps to Reproduce
1. git clone the emulator repo and boot the emulator (wellarchitected is bootstrapped).
2. Replay behavioral suite simparity-behavioral-aws-v1.
3. Re-replay check wellarchitected.share-with-own-account-rejected, or reproduce by hand:
   a. sts:GetCallerIdentity -> note Account.
   b. wellarchitected:CreateWorkload with WorkloadName/<unique>, Description, Environment=PREPRODUCTION, ReviewOwner, Lenses=['wellarchitected'], AwsRegions=[<region>], ClientRequestToken=<uuid> -> note WorkloadId.
   c. wellarchitected:CreateWorkloadShare(WorkloadId=<from b>, SharedWith=<Account from a>, PermissionType='READONLY', ClientRequestToken=<uuid>).
      Observe HTTP 501 / 'NotImplemented'; expected HTTP 400 / 'ValidationException' with a populated Message, and no share created.
   d. Repeat (c) with SharedWith='111122223333'; expected HTTP 200 with a ShareId matching ^[0-9a-f]{32}$ that then appears in ListWorkloadShares for the workload. Observe HTTP 501.
