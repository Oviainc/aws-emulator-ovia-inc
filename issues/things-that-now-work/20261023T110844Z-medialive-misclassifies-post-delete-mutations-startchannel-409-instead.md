Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.delete-running-channel-rejected medialive.input-security-group-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.delete-running-channel-rejected|run#24|AssertionError|assert mutate_code == "NotFoundException", f"expected NotFoundException starting a deleted channel, got {mutate_code}"
Issue-Key: medialive|UpdateInputSecurityGroup|NotFoundException
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# medialive misclassifies post-delete mutations: StartChannel 409 instead of 404, UpdateInputSecurityGroup 404 on a live DELETED tombstone

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartChannel

## Description
The medialive emulator classifies mutations against deleted resources inconsistently with AWS. This bundle has TWO causes; both must be closed.

Cause 1 of 2: StartChannel on a channel that has already been deleted answers ConflictException instead of NotFoundException. Failing check: medialive.delete-running-channel-rejected.
AWS: Smithy (com.amazonaws.medialive#MediaLive): StartChannel is POST /prod/channels/{ChannelId}/start, success code 200, and models both ConflictException (client, httpError 409) and NotFoundException (client, httpError 404). Recorded behavior: once DeleteChannel has driven the channel to terminal state DELETED, a further StartChannel on that id is answered 404 NotFoundException (recorded message example: "Channel <id> already deleted"). ConflictException is reserved for a state conflict on a channel that still exists — e.g. DeleteChannel while the channel is RUNNING, which the same check confirms the emulator gets right.
Emulator: after the full delete-and-settle sequence (create -> IDLE -> StartChannel -> RUNNING -> StopChannel -> IDLE -> DeleteChannel -> terminal DELETED), StartChannel on that id returns error code ConflictException (409) rather than NotFoundException (404).
Why: a client cannot distinguish "this resource is gone, stop retrying and recreate it" from "this resource exists but is busy, retry later". Standard SDK/IaC retry and reconcile logic treats 409 ConflictException as retryable and 404 NotFoundException as terminal-and-recreate, so against this emulator a caller spins on a permanently dead channel id instead of recreating it, and waiters never settle.
Fix: the StartChannel handler's existence/state check in the medialive channel operations (and the shared channel-state guard used by the other channel mutations) — an id that is absent or in terminal DELETED must be resolved to NotFoundException BEFORE the IDLE/RUNNING state-conflict check that yields ConflictException. The existence check must run first, the state check second.
Traces: check medialive.delete-running-channel-rejected (assertion at check line 184: "expected NotFoundException starting a deleted channel, got ConflictException").

Cause 2 of 2: UpdateInputSecurityGroup rejects a soft-deleted input security group with NotFoundException even though DescribeInputSecurityGroup still returns that same id in state DELETED. Failing check: medialive.input-security-group-lifecycle.
AWS: Smithy: UpdateInputSecurityGroup is PUT /prod/inputSecurityGroups/{InputSecurityGroupId}, success code 200, errors include NotFoundException (404) and ConflictException (409); DescribeInputSecurityGroup is GET on the same path with a State member of enum InputSecurityGroupState. Recorded behavior only: MediaLive SOFT-deletes an InputSecurityGroup — after DeleteInputSecurityGroup the id drops out of ListInputSecurityGroups but stays individually addressable via DescribeInputSecurityGroup in state DELETED, and remains MUTABLE: UpdateInputSecurityGroup against the DELETED tombstone succeeds (200) and the subsequent Describe reflects the new WhitelistRules. The two post-delete contracts AWS exposes are therefore self-consistent: either the id is hard-gone (Describe 404 NotFoundException AND Update 404) or it is a soft tombstone (Describe 200 State=DELETED AND Update 200).
Emulator: it implements half of each. DescribeInputSecurityGroup keeps answering 200 with State=DELETED (so the check takes the soft-delete branch), but UpdateInputSecurityGroup on that same id raises 404 NotFoundException (observed message example: "Input security group <id> already deleted"), aborting the check with an unhandled error.
Why: the readable-but-unwritable state exists on no real AWS path, so a client has no correct way to handle it. Code that reads State=DELETED and then re-asserts whitelist rules on the tombstone (a common idempotent configure-then-verify loop) gets a hard 404 the AWS contract never produces, while code written for the hard-delete contract is instead misled by the 200 Describe into believing the group is still addressable.
Fix: the medialive input-security-group state model and the UpdateInputSecurityGroup handler must agree on ONE delete semantics. Keeping the soft delete that DescribeInputSecurityGroup already implements, UpdateInputSecurityGroup must accept a group whose State is DELETED, apply the request's WhitelistRules (replacing, not merging) and return 200 — the tombstone-exclusion logic belongs only in ListInputSecurityGroups, not in the mutation path's existence check.
Traces: check medialive.input-security-group-lifecycle (unhandled botocore.errorfactory.NotFoundException from UpdateInputSecurityGroup at check line 674, after _wait_for_deleted resolved to the DELETED branch).

Neither cause is covered by the open issue "medialive template update operations do not persist mutated fields" (20261022T171117Z-medialive-template-update-operations-do-not-persist-mutated.md), which concerns the CloudWatch-alarm / EventBridge rule template resources' update persistence, not channel or input-security-group delete semantics.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator, medialive 14/21 passed.

Cause 1 — [mismatch] medialive.delete-running-channel-rejected:
  AssertionError: expected NotFoundException starting a deleted channel, got ConflictException
  (check_medialive_delete_running_channel_rejected.py line 50 / body line 184)
  Expected: StartChannel on a DELETED channel id -> HTTP 404, Error.Code = NotFoundException (recorded message example "Channel <id> already deleted").
  Actual:   HTTP 409, Error.Code = ConflictException.
  Everything earlier in the same check passed, including the correct ConflictException for DeleteChannel while RUNNING and the successful stop-then-delete, so the channel genuinely reached terminal DELETED before the diverging call.

Cause 2 — [error] medialive.input-security-group-lifecycle:
  botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the UpdateInputSecurityGroup operation: Input security group <id> already deleted
  Expected: after DeleteInputSecurityGroup, because DescribeInputSecurityGroup still answers 200 with State=DELETED, UpdateInputSecurityGroup on that id -> HTTP 200, and the following DescribeInputSecurityGroup reports the replaced WhitelistRules.
  Actual:   UpdateInputSecurityGroup -> HTTP 404, Error.Code = NotFoundException, while DescribeInputSecurityGroup on the very same id keeps returning 200/State=DELETED.
  The create/update/describe round-trip earlier in the check passed (IDLE on create, whitelist replace reflected), so only the post-delete mutation path diverges.

Smithy consulted (services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json): StartChannel POST /prod/channels/{ChannelId}/start code 200, errors {ConflictException 409, NotFoundException 404, ...}; DeleteChannel DELETE /prod/channels/{ChannelId} code 200, same error set; UpdateInputSecurityGroup PUT /prod/inputSecurityGroups/{InputSecurityGroupId} code 200, errors {ConflictException 409, NotFoundException 404, ...}; DescribeInputSecurityGroup GET same path code 200 with State: InputSecurityGroupState.

## Steps to Reproduce
Cause 1 (StartChannel after delete):
1. CreateInput (pull input) and CreateChannel with a minimal encoder config; poll DescribeChannel until State=IDLE.
2. StartChannel; poll until State=RUNNING.
3. DeleteChannel -> expect 409 ConflictException (emulator is correct here).
4. StopChannel; poll until State=IDLE; DeleteChannel; poll until the channel is terminally DELETED/gone.
5. POST /prod/channels/<id>/start on the deleted id. Expected 404 NotFoundException; emulator returns 409 ConflictException.
6. Equivalent replay: check medialive.delete-running-channel-rejected.

Cause 2 (UpdateInputSecurityGroup on the DELETED tombstone):
1. CreateInputSecurityGroup with WhitelistRules=[{Cidr: 203.0.113.0/24}] -> State IDLE.
2. UpdateInputSecurityGroup with two different CIDRs; DescribeInputSecurityGroup reflects them.
3. DeleteInputSecurityGroup, then poll DescribeInputSecurityGroup: the emulator keeps answering 200 with State=DELETED (soft delete), and ListInputSecurityGroups no longer lists the id.
4. PUT /prod/inputSecurityGroups/<id> with WhitelistRules=[{Cidr: 192.0.2.0/24}]. Expected 200 followed by a Describe showing that single CIDR; emulator returns 404 NotFoundException.
5. Equivalent replay: check medialive.input-security-group-lifecycle.
