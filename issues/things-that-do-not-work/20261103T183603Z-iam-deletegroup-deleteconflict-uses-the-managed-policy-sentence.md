Issue-Source: trace-sweep-divergence
Issue-Trace: iam-group-inline-and-managed-policies
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-group-inline-and-managed-policies
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|DeleteGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 4988eaf067dbbfdfac9330bbeed709d39fc37f35

# IAM DeleteGroup DeleteConflict uses the managed-policy sentence for an inline-policy conflict

> _Note: this issue was identified against the code base at commit `4988eaf067dbbfdfac9330bbeed709d39fc37f35`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGroup

## Description
One root cause.

Cause 1 of 1: DeleteGroup refuses a group that still has dependencies with a single hard-coded DeleteConflict sentence — the one AWS reserves for ATTACHED MANAGED policies — instead of the sentence that matches the dependency actually blocking the delete.

AWS: DeleteGroup refuses a non-empty group with DeleteConflict (HTTP 409). Smithy: DeleteGroup models DeleteConflictException with awsQueryError code 'DeleteConflict', httpResponseCode 409 (model services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json). The model does not state the message text, so the message requirement is recorded behavior only: AWS words the refusal per blocking dependency, and for a group that still carries an INLINE (embedded) group policy the message is exactly 'Cannot delete entity, must delete policies first.' The sentence 'Cannot delete entity, must detach all policies first.' is AWS's wording for the different case where MANAGED policies are still attached. Both are entirely fixed text — there are no variable parts substituted from the request or from state, so the whole sentence (including the trailing period) is the requirement.

Emulator: answers HTTP 409 with Code 'DeleteConflict' (both correct) but Message 'Cannot delete entity, must detach all policies first.' for the inline-policy conflict, i.e. it emits the managed-policy sentence unconditionally rather than selecting the sentence for the dependency it found.

Why: DeleteConflict is the one IAM refusal a client is expected to act on by remediating the specific dependency. A client (or an operator reading the log) that keys off AWS's inline-policy sentence is told to detach managed policies — of which there may be none — and never deletes the inline policy that is actually blocking the delete, so its cleanup loop never converges. Message-matching clients and runbooks diverge even though the status and error code match.

Fix: the DeleteConflict message construction in IAM's DeleteGroup handler — it must choose the message from the dependency it detected (inline group policies -> 'Cannot delete entity, must delete policies first.'; attached managed policies -> 'Cannot delete entity, must detach all policies first.') rather than emitting one constant. The same selection applies to the sibling DeleteUser/DeleteRole conflict paths that share this wording.

Traces: iam-group-inline-and-managed-policies#task#10 (the only recorded step in this cluster).

Related but NOT covering this: open issue 20261029T192356Z-iam-10-net-new-refusal-message-and-response.md collects IAM refusal-message rewordings, but none of its ten causes is the DeleteGroup DeleteConflict sentence; and 20261029T184359Z-iam-nosuchentity-messages-use-emulator-prose-instead-of.md covers NoSuchEntity wording, a different error class. The replay reports this cluster as owned by no open issue.

## Evidence
Fixture iam-group-inline-and-managed-policies, step task#10, operation DeleteGroup. Expected status 409, actual status 409 (match). Expected error Code 'DeleteConflict', actual 'DeleteConflict' (match). Divergence is confined to $.ErrorResponse.Error.Message:
  expected: 'Cannot delete entity, must delete policies first.'
  actual:   'Cannot delete entity, must detach all policies first.'
Replay detail verbatim: "$.ErrorResponse.Error.Message: value_mismatch (expected 'Cannot delete entity, must delete policies first.', got 'Cannot delete entity, must detach all policies first.')". divergence_key: service=iam|DeleteGroup|status=409->409|keys=$.ErrorResponse.Error.Message. Cluster size 1 (affected: iam-group-inline-and-managed-policies#task#10).
Message anatomy: both candidate sentences are 100% fixed text with no request- or state-derived substitutions; the defect is sentence SELECTION, not substitution.

## Steps to Reproduce
1. Against the emulator's IAM Query endpoint, CreateGroup a group.
2. PutGroupPolicy on it to leave exactly one INLINE group policy in place (attach no managed policies, add no users).
3. Call DeleteGroup with that GroupName.
4. Observe the response: HTTP 409 with ErrorResponse.Error.Code 'DeleteConflict' (correct) but ErrorResponse.Error.Message 'Cannot delete entity, must detach all policies first.'; AWS returns 'Cannot delete entity, must delete policies first.' for this inline-policy conflict.
5. For contrast, repeat with only an AttachGroupPolicy managed attachment and no inline policy: AWS returns 'Cannot delete entity, must detach all policies first.' there, which is the sentence the emulator emits in both cases.
Replay: re-run the wire corpus and confirm fixture iam-group-inline-and-managed-policies step task#10 passes.
