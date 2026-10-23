Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.create-membership-duplicate-conflict cleanrooms.delete-collaboration-cascades-membership-status cleanrooms.get-membership-not-found
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms.create-membership-duplicate-conflict|run#9|AssertionError|assert error["Code"] == "ValidationException", f"expected ValidationException for duplicate membership, got {error!r}"
Issue-Key: cleanrooms.delete-collaboration-cascades-membership-status|run#10|AssertionError|assert error["Code"] == "AccessDeniedException", (
Issue-Key: cleanrooms.get-membership-not-found|run#5|AssertionError|assert error["Message"] == f"No such membership: {bogus_id} for account {ctx.account}", error
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Clean Rooms error classification diverges on duplicate CreateMembership, deleted-collaboration reads, and GetMembership not-found text (3 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMembership

## Description
Three independent root causes in the Clean Rooms emulator, all in how refusals are CLASSIFIED (error shape / HTTP status) rather than in the happy path. All three are grounded in failing behavioral checks of the `cleanrooms` service; none of them is blocked by an unimplemented scaffold stub (the calls were served and answered, just wrongly).

=== Cause 1 of 3: duplicate CreateMembership answers ConflictException (409) instead of ValidationException (400) ===
AWS: CreateMembership models both ConflictException (409) and ValidationException (400) (Smithy: CreateMembership errors = AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400). Recorded behavior only for which one is used: a second CreateMembership for the same account/collaboration pair is rejected as **ValidationException, HTTP 400**, with a message built from the request/caller of the form "Cannot create membership for account: <accountId>. ..." — the recording treats "this account is already an active member" as an input-validation refusal, not a resource conflict.
Emulator: returns `{"Code": "ConflictException", "Message": "account 000000000000 already has membership <membershipId> in collaboration <collaborationId>"}` (HTTP 409).
Why: a client that distinguishes AWS error classes gets the wrong one. Code that catches ValidationException to report "already a member" never catches it, and generic SDK/retry layers treat 409 ConflictException as a transient, retryable conflict and will re-issue the create in a loop, where AWS's 400 is terminal. Error-class-driven branching (e.g. idempotent "ensure membership" helpers) takes the wrong branch.
Fix: the CreateMembership handler's duplicate-membership precondition in the cleanrooms service crate — it must raise the ValidationException variant (400) for an account that already holds a membership in the collaboration, and build the message from the caller's account id. Also restores the rest of that check, whose later leg asserts DeleteMembership on the creator's own ACTIVE membership is ConflictException (409); that leg was never reached and is unverified.
Traces: cleanrooms.create-membership-duplicate-conflict.

=== Cause 2 of 3: reads/writes against a deleted collaboration answer ValidationException instead of AccessDeniedException (403) ===
AWS: GetCollaboration does NOT model a not-found error at all (Smithy: GetCollaboration errors = AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400 — there is no ResourceNotFoundException in its closure). Recorded behavior: after DeleteCollaboration, GetCollaboration against that id is rejected with **AccessDeniedException, HTTP 403** — the caller's authorization to the collaboration disappears with the collaboration, so the service answers "not allowed" rather than "not found"/"invalid input". The same check's next leg records CreateMembership against a deleted collaboration also returning AccessDeniedException.
Emulator: returns `{"Code": "ValidationException", "Message": "collaboration <collaborationId> not found"}` (HTTP 400).
Why: the emulator leaks existence/non-existence as a 400 input error where AWS answers 403. A client cannot tell a malformed/unknown-collaboration input error from a revoked-authorization condition, so permission-aware code paths (and 403-specific handling such as surfacing "you no longer have access to this collaboration") never trigger; conversely argument-validation handlers fire on a well-formed id. Because AccessDeniedException is the only non-validation client error GetCollaboration models, any emulator answer for a deleted collaboration other than 403 AccessDeniedException is off-contract.
Fix: the post-delete authorization gate shared by the GetCollaboration handler (and the collaboration-existence check used by CreateMembership) in the cleanrooms service crate: a deleted/invisible collaboration must map to AccessDeniedException (403), not to the generic ValidationException not-found path. Also unblocks the remainder of that check, which asserts the membership record survives collaboration deletion and transitions to COLLABORATION_DELETED — that cascade is unverified because the check aborted at the first assertion.
Traces: cleanrooms.delete-collaboration-cascades-membership-status.

=== Cause 3 of 3: GetMembership not-found message is emulator-authored instead of built from the request and caller account ===
AWS: GetMembership models ResourceNotFoundException (404) (Smithy: GetMembership errors = AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). Recorded behavior only: the message for a well-formed but nonexistent membershipIdentifier is composed from the request and the caller — "No such membership: <membershipIdentifier> for account <callerAccountId>" — i.e. it names both the requested id and the account whose namespace was searched. That composition rule, not the literal recorded text, is the requirement here.
Emulator: the error CODE and status are already correct (ResourceNotFoundException), but the message is "membership <membershipIdentifier> not found" — it names the id and omits the caller account entirely, so it is not produced by AWS's rule.
Why: membership ids are account-scoped, so the account in the message is what tells a caller the lookup failed in THEIR account rather than that the id does not exist anywhere. Clients and operators that parse or log this message to diagnose cross-account/wrong-credentials mistakes lose that signal. This is the lowest-severity cause of the bundle: the status and error code are right, only the message-composition rule diverges.
Fix: the not-found message construction in the GetMembership handler in the cleanrooms service crate — build it from the request's membershipIdentifier plus the resolved caller account id.
Traces: cleanrooms.get-membership-not-found.

Note on what is NOT in this bundle: cleanrooms.concurrent-analysis-template-name-race also fails, but its only grounded cause is that CreateAnalysisTemplate is an unimplemented scaffold stub (all six racers error out, leaving zero winners), which is already filed as its own work item; the check swallows the ClientError so the suite could not auto-attribute it. No separate issue is filed for it here.

## Evidence
Behavioral suite: 1/21 passed for cleanrooms (5 mismatched, 15 errored).

Cause 1 — cleanrooms.create-membership-duplicate-conflict [mismatch]:
  assert error["Code"] == "ValidationException", f"expected ValidationException for duplicate membership, got {error!r}"
  AssertionError: expected ValidationException for duplicate membership, got
    {'Message': 'account 000000000000 already has membership 68601559-7dfe-4a34-97b2-69df03d2a760 in collaboration 066d34c7-2421-4931-898f-b7b2c427c524', 'Code': 'ConflictException'}
  Expected: Code=ValidationException (HTTP 400), message of the form "Cannot create membership for account: <accountId>. ..." (recorded example; the requirement is the error class/status).
  Actual:   Code=ConflictException (HTTP 409).

Cause 2 — cleanrooms.delete-collaboration-cascades-membership-status [mismatch]:
  assert error["Code"] == "AccessDeniedException", (f"expected AccessDeniedException reading a deleted collaboration, got {error!r}")
  AssertionError: expected AccessDeniedException reading a deleted collaboration, got
    {'Message': 'collaboration 642ada6b-2c60-4d48-94a1-7f9b815aa864 not found', 'Code': 'ValidationException'}
  Expected: Code=AccessDeniedException (HTTP 403) from GetCollaboration after DeleteCollaboration.
  Actual:   Code=ValidationException (HTTP 400). Smithy shows GetCollaboration does not even model a not-found error.

Cause 3 — cleanrooms.get-membership-not-found [mismatch]:
  assert error["Message"] == f"No such membership: {bogus_id} for account {ctx.account}", error
  AssertionError: {'Message': 'membership 72508a6a-d177-484f-a354-f292cd618ac0 not found', 'Code': 'ResourceNotFoundException'}
  Expected: ResourceNotFoundException (correct) with a message composed from the requested membershipIdentifier AND the caller account id.
  Actual:   ResourceNotFoundException with a message naming only the id (uuids/accounts above are environment-specific examples).

## Steps to Reproduce
All against the emulator's cleanrooms endpoint with any credentials.

Cause 1 (CreateMembership duplicate):
1. CreateCollaboration(name=<unique>, members=[], creatorMemberAbilities=["CAN_QUERY","CAN_RECEIVE_RESULTS"], creatorDisplayName="creator", queryLogStatus="DISABLED") -> note collaboration id C.
2. CreateMembership(collaborationIdentifier=C, queryLogStatus="DISABLED") -> succeeds, membership M.
3. CreateMembership(collaborationIdentifier=C, queryLogStatus="DISABLED") again with the same caller.
   Observed: 409 ConflictException. Expected: 400 ValidationException.

Cause 2 (deleted collaboration):
1. CreateCollaboration as above -> C; CreateMembership(collaborationIdentifier=C) -> M.
2. DeleteCollaboration(collaborationIdentifier=C).
3. GetCollaboration(collaborationIdentifier=C).
   Observed: 400 ValidationException "collaboration <C> not found". Expected: 403 AccessDeniedException.
4. (Also unverified past step 3: CreateMembership(collaborationIdentifier=C) should be 403 AccessDeniedException, and GetMembership(M) should still return M eventually reaching status COLLABORATION_DELETED.)

Cause 3 (GetMembership not-found message):
1. GetMembership(membershipIdentifier=<a fresh random uuid matching ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$>).
   Observed: ResourceNotFoundException, Message="membership <id> not found".
   Expected: ResourceNotFoundException, Message composed from the id and the caller account, e.g. "No such membership: <id> for account <accountId>".

Replay: run_behavioral_suite and re-run the three checks in trace_ids.
