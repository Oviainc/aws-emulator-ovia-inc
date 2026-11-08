Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.concurrent-create-scope-same-target-one-wins networkflowmonitor.update-scope-add-target-reflected
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor.concurrent-create-scope-same-target-one-wins|run#14|AssertionError|assert outcomes.count("ok") == 1, f"expected exactly one winner, got {results}"
Issue-Key: networkflowmonitor.update-scope-add-target-reflected|_error_code#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 86516d64c0849b0da4294aba853b3ef6f42bc833

# networkflowmonitor: two causes — the one-scope-per-account/Region quota is not enforced (CreateScope mints up to 100 scopes, and its 402 message is emulator-authored), and UpdateScope silently accepts re-adding a target already in scope instead of refusing it

> _Note: this issue was identified against the code base at commit `86516d64c0849b0da4294aba853b3ef6f42bc833`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScope

## Description
Two net-new root causes in the NetworkFlowMonitor scope surface. Both are grounded in failing behavioral checks; no existing open issue mentions networkflowmonitor (list_existing_issues query='networkflowmonitor' -> none).

Cause 1 of 2: CreateScope does not enforce NetworkFlowMonitor's singleton-scope quota, and its ServiceQuotaExceededException message is emulator-authored.
AWS: NetworkFlowMonitor allows ONE scope per account/Region. A CreateScope that would create a second scope for the account is refused with the modeled ServiceQuotaExceededException (Smithy: CreateScope errors include ServiceQuotaExceededException, errorKind client, httpError 402 — "The request exceeded a service quota"). Recorded behavior: the refusal message is the fixed text "Maximum limit of [" + the quota value + "] for resource [" + the resource type + "] exceeded for account [" + the caller's account id + "]." — i.e. fixed text "Maximum limit of [", "] for resource [", "] exceeded for account [", "]."; variable parts are the quota (1 for resource scope), the resource type (scope) and the caller's account id taken from the credentials/STS identity. AWS appends a per-call " (Service: ..., Status Code: ..., Request ID: ...)" suffix, which the check strips and which is NOT part of the requirement. Because the quota is evaluated against the slot the winner just claimed, two concurrent CreateScope calls for the identical target on a cold account yield exactly one success and one ServiceQuotaExceededException (not ConflictException); when the slot is already occupied, BOTH are refused with that same error.
Emulator: the ceiling is the emulator's own invented one — services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/state.rs declares `pub const MAX_SCOPES: usize = 100;` and its own doc comment concedes "AWS publishes its own quota and it is not this number". So two concurrent CreateScope calls for the identical target BOTH return 200, minting scopeId 00000000-0000-4000-8000-000000000001 and 00000000-0000-4000-8000-000000000002 (outcomes [('ok', ...001), ('ok', ...002)]). Nothing is ever refused until the 100th scope, and when it finally is, state.rs `quota_exceeded()` words it "this emulator holds at most {limit} {resource_type} resources" instead of AWS's "Maximum limit of [1] for resource [scope] exceeded for account [<accountId>]." — different fixed text, and it names no account at all.
Why: a client that relies on the account having a single scope reads a second scope as success and ends up with two topologies and two conflicting monitoring slots; code that catches ServiceQuotaExceededException to discover and reuse the pre-existing scope (exactly what the suite's own _acquire_scope helper does) never takes that branch, and any client that parses or logs the quota message sees neither the limit AWS states nor the account it applies to.
Fix: the CreateScope handler's quota check in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/create_scope.rs, plus the ceiling constant and the refusal-message builder in src/state.rs. The guard must be the per-account/Region singleton rule (evaluated under the same store lock that mints, so concurrent creates serialize into one winner), and the message builder must emit AWS's template with the limit, resource type and caller account id filled in.
Traces: networkflowmonitor.concurrent-create-scope-same-target-one-wins (AssertionError: expected exactly one winner, got [('ok', '00000000-0000-4000-8000-000000000001', None), ('ok', '00000000-0000-4000-8000-000000000002', None)]). Code citations: src/state.rs:83 (MAX_SCOPES), src/state.rs:1295 (quota_exceeded), src/ops/create_scope.rs:207-215.

Cause 2 of 2: UpdateScope resourcesToAdd silently de-duplicates a target the scope already holds instead of refusing the add.
AWS: adding, via UpdateScope resourcesToAdd, an account/region pair that is already in the scope is a request fault, refused with the modeled ValidationException (Smithy: UpdateScope errors include ValidationException, errorKind client, httpError 400 — "Invalid request"). Recorded behavior: the message's fixed text is "invalid input -> (cannot add account/region pairs=[[" ... "]] as they are already in scope.)"; the variable part is the list of already-present pairs taken from the request, each rendered as "<accountId>#<region>" (for one duplicate target: "invalid input -> (cannot add account/region pairs=[[<accountId>#<region>]] as they are already in scope.)"). The refusal happens before any write, so the scope's targets and its SUCCEEDED status are unchanged.
Emulator: the call SUCCEEDS with HTTP 200 and returns {scopeId, status: 'SUCCEEDED', scopeArn}. src/ops/update_scope.rs validates list sizes, cross-region targets, add/delete overlap, owner removal and emptiness, but never checks whether an addition is already present; it then hands the lists to state::merged_targets, which is documented to hold "a target named twice ... once" and so folds the duplicate away as a silent no-op.
Why: a client that re-sends an add (a retry, a converging reconciler, or an idempotency-unaware caller) is told the add was applied when AWS would have told it the pair was already in scope. The negative class disappears entirely: there is no 400 to distinguish "you asked for something already true" from "we added it", so drift-detection logic and error-driven reconciliation both take the wrong branch.
Fix: UpdateScope's validation in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/update_scope.rs — add an already-present check over resourcesToAdd against the stored scope's targets (keyed the same way state::target_identity keys them), returning the modeled ValidationException with AWS's wording before any mutation; the message builder belongs beside the existing cross_region_target()/owner_not_removable() helpers.
Traces: networkflowmonitor.update-scope-add-target-reflected (AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'scopeId': ..., 'status': 'SUCCEEDED', 'scopeArn': ...}). Code citations: src/ops/update_scope.rs:180-272, src/state.rs:905 (merged_targets).

Note: the same check's FIRST negative (a cross-region target in resourcesToAdd) passes — update_scope.rs:223 already refuses it with the correct wording — so only the duplicate-add rule is missing there.

## Evidence
Cause 1 — check networkflowmonitor.concurrent-create-scope-same-target-one-wins.
Expected (AWS): of two concurrent CreateScope calls with the identical target on a cold account, exactly one returns 200 and the other is refused 402 ServiceQuotaExceededException with message "Maximum limit of [1] for resource [scope] exceeded for account [<accountId>]." (fixed text as quoted; [1] is the quota for resource scope, <accountId> is the caller's account id). If the account already holds the singleton scope, BOTH calls are refused with that same error and the existing scope's targets are untouched.
Actual (emulator): both calls returned 200 — outcomes [('ok', '00000000-0000-4000-8000-000000000001', None), ('ok', '00000000-0000-4000-8000-000000000002', None)] — so two scopes now exist for one account/Region. AssertionError: "expected exactly one winner". The ceiling that would eventually refuse is MAX_SCOPES = 100 (src/state.rs:83), and its message is "this emulator holds at most 100 scope resources" (src/state.rs:1295) rather than AWS's template.

Cause 2 — check networkflowmonitor.update-scope-add-target-reflected.
Expected (AWS): UpdateScope(scopeId=<id>, resourcesToAdd=[<the target the scope already holds>]) is refused 400 ValidationException, message "invalid input -> (cannot add account/region pairs=[[<accountId>#<region>]] as they are already in scope.)"; the scope's targets and SUCCEEDED status are unchanged.
Actual (emulator): HTTP 200 with {'scopeId': '00000000-0000-4000-8000-000000000001', 'status': 'SUCCEEDED', 'scopeArn': 'arn:aws:networkflowmonitor:<region>:<account>:scope/<scopeId>'} — no error raised. AssertionError: "expected the call to be rejected, but it SUCCEEDED and returned ...". The preceding cross-region negative in the same check DID produce the correct 400 with the correct message, so the gap is specific to the duplicate-add rule.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point a networkflowmonitor client at it; note the caller's account id from sts:GetCallerIdentity.
2. On a cold account (ListScopes returns no scopes), fire two concurrent CreateScope calls with the identical target {targetIdentifier: {targetId: {accountId: <account>}, targetType: 'ACCOUNT'}, region: <request region>}.
3. Observe both return 200 with distinct scopeIds. AWS returns 200 for exactly one and 402 ServiceQuotaExceededException "Maximum limit of [1] for resource [scope] exceeded for account [<account>]." for the other.
4. Issue a third CreateScope with the same target: it also succeeds, up to 100 scopes.

Cause 2:
1. Acquire the account's scope (CreateScope with target {targetIdentifier: {targetId: {accountId: <account>}, targetType: 'ACCOUNT'}, region: <request region>}) and wait for status SUCCEEDED.
2. Call UpdateScope(scopeId=<that scope>, resourcesToAdd=[that same target]).
3. Observe HTTP 200 with status SUCCEEDED. AWS refuses with 400 ValidationException "invalid input -> (cannot add account/region pairs=[[<account>#<region>]] as they are already in scope.)" and leaves the scope untouched.

Or replay directly: run_behavioral_suite and re-run the two checks in trace_ids.
