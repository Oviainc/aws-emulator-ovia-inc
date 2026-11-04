Issue-Source: trace-sweep-divergence
Issue-Trace: iam-ekscc-traj-attach-cluster-policy-mistakes
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-ekscc-traj-attach-cluster-policy-mistakes iam-emr-roles-policy-path-and-rerun-mistakes iam-gpn-traj-attach-glue-service-role-to-the-worker iam-acquire-role-backup-service-lifecycle iam-acquire-role-power-user-lifecycle iam-ebecs-events-role-run-task-pass-role-simulation iam-list-caller-policies-readonly iam-read-only-inspection-of-operator-identity
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|DeleteRole|2xx->4xx|status
Issue-Key: iam|AcquireRole|2xx->2xx|$.AcquireRoleResponse.AcquireRoleResult.Role.AssumeRolePolicyDocument
Issue-Key: iam|ListAttachedRolePolicies|2xx->2xx|$.ListAttachedRolePoliciesResponse.ListAttachedRolePoliciesResult.AttachedPolicies.member[*].PolicyArn
Issue-Key: iam|ListAttachedRolePolicies|2xx->2xx|$.ListAttachedRolePoliciesResponse.ListAttachedRolePoliciesResult.AttachedPolicies.member[*].PolicyName
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.MissingContextValues.member[*]
Issue-Key: iam|SimulatePrincipalPolicy|2xx->2xx|$.SimulatePrincipalPolicyResponse.SimulatePrincipalPolicyResult.EvaluationResults.member.ResourceSpecificResults.member.MissingContextValues.member[*]
Issue-Key: iam|ListAttachedUserPolicies|2xx->2xx|$.ListAttachedUserPoliciesResponse.ListAttachedUserPoliciesResult.AttachedPolicies
Issue-Related: 20261029T184359Z-iam-modeled-empty-members-are-omitted-from-iam-success.md
Issue-Related: 20261103T132715Z-iam-simulateprincipalpolicy-evaluation-results-omit-modeled-detail-members.md
Issue-Commit: e6e32aafbb62326e8cdd2e3915a7b6a44f1d783b

# iam role and policy projections: 5 net-new causes — DeleteRole conflict after cleanup, unencoded trust policy, attached-policy ordering, simulate detail members, empty caller attachments

> _Note: this issue was identified against the code base at commit `e6e32aafbb62326e8cdd2e3915a7b6a44f1d783b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRole

## Description
Five independent, net-new divergences in the IAM emulator's role/policy state and response projections. All five are iam and none is covered by an open issue carrying its replay key. 5 causes.

Cause 1 of 5: DeleteRole answers 409 DeleteConflict for a role whose attachments have already been removed.
AWS: Smithy DeleteRole models DeleteConflictException (awsQueryError code 'DeleteConflict', httpResponseCode 409) and AWS raises it ONLY while the role still has attached managed policies, inline policies, or membership in an instance profile; once those are removed DeleteRole succeeds with HTTP 200 and an empty DeleteRoleResponse carrying only ResponseMetadata.RequestId. The recordings show exactly that: after the fixture's teardown detaches/deletes the role's policies, AWS answers 200.
Emulator: answers HTTP 400-class 409 for the same request, i.e. it still believes the role has dependents. Hypothesis: the emulator's role dependency bookkeeping is not decremented by DetachRolePolicy / DeleteRolePolicy / RemoveRoleFromInstanceProfile, so the conflict latch is never cleared. The error body was not captured beyond the status; the requirement for the 409 path is the modelled DeleteConflict code with AWS's sentence 'Cannot delete entity, must detach all policies first.' — fixed text, no variable parts — and the 200 path must return no error at all.
Why: a client (and every teardown/CI script) can never delete a role it has correctly cleaned up. Roles leak, name reuse fails with EntityAlreadyExists on the next run, and a caller that treats DeleteConflict as 'something is still attached' loops forever re-listing attachments that are already gone.
Fix: the DeleteRole handler's delete-conflict precondition and the role-dependency state it reads (detach/delete of role policies and instance-profile removal must clear it).
Traces: iam-ekscc-traj-attach-cluster-policy-mistakes#teardown#12, iam-emr-roles-policy-path-and-rerun-mistakes#teardown#19, iam-gpn-traj-attach-glue-service-role-to-the-worker#teardown#11.

Cause 2 of 5: AcquireRole returns Role.AssumeRolePolicyDocument as raw JSON instead of URL-encoded JSON.
AWS: Smithy AcquireRole output member Role (required), whose AssumeRolePolicyDocument is documented as 'The policy that grants an entity permission to assume the role.' IAM returns policy documents in query-protocol responses percent-encoded (RFC 3986) — the recording shows the trust policy as '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B...%5D%7D'. The requirement is the encoding, not the recorded document: every reserved character of the stored JSON ('{', '}', '"', ':', ',', '[', ']') must be percent-escaped; the JSON content itself comes from the role template/trust policy of the request.
Emulator: emits the document verbatim as unescaped JSON ('{"Version":"2012-10-17","Statement":[...]}').
Why: every AWS SDK URL-decodes this member before handing it to the caller, so a raw document comes back mangled — '+' becomes a space and any literal '%' sequence in the policy is destroyed — and a client that round-trips the returned document into UpdateAssumeRolePolicy submits a corrupted policy.
Fix: the AcquireRole (and shared Role projection) response serializer — policy-document members must be percent-encoded on the way out.
Related open issue 20261103T132715Z-iam-inline-policy-reads-do-not-url-encode.md describes the same missing encoding for INLINE policy reads and 20261103T132715Z-iam-acquirerole-answers-404-so-no-role-template.md describes AcquireRole answering 404; neither carries this replay key or covers Role.AssumeRolePolicyDocument on a successful AcquireRole.
Traces: iam-acquire-role-backup-service-lifecycle#task#1, iam-acquire-role-power-user-lifecycle#task#1.

Cause 3 of 5: ListAttachedRolePolicies returns the attached managed policies in the emulator's own attachment order, not AWS's order.
AWS: Smithy ListAttachedRolePolicies output AttachedPolicies is attachedPoliciesListType (a list, paginated by Marker/MaxItems); ordering is recorded behavior only. The recordings show the same two AWS-managed service-role policies returned in the opposite order from the emulator, consistently at both the task and teardown read of the same role — so the order is stable on AWS and stable-but-different in the emulator. Hypothesis: AWS orders the list by policy identifier rather than by attachment sequence.
Emulator: serializes the list in insertion/attachment order.
Why: a client that reads AttachedPolicies positionally (member[0] as 'the' policy, or diffing two listings element-by-element to detect drift) sees a spurious difference, and a paginated read with MaxItems=1 returns a different policy than AWS would on the same page.
Fix: the ListAttachedRolePolicies handler's ordering of AttachedPolicies before serialization/pagination (the same ordering rule belongs on ListAttachedUserPolicies / ListAttachedGroupPolicies).
Traces: iam-acquire-role-backup-service-lifecycle#task#3, iam-acquire-role-backup-service-lifecycle#teardown#4.

Cause 4 of 5: SimulatePrincipalPolicy omits OrganizationsDecisionDetail, EvalDecisionDetails and MissingContextValues from the evaluation result, both top-level and per-resource.
AWS: Smithy shape EvaluationResult declares OrganizationsDecisionDetail (target OrganizationsDecisionDetail, 'details how Organizations and its service control policies affect the results of the simulation'), EvalDecisionDetails (EvalDecisionDetailsType map) and MissingContextValues (ContextKeyNamesResultListType, 'a list of context keys that are required by the included input policies but that were not provided by one of the input parameters'), and ResourceSpecificResult declares its own MissingContextValues. The recording shows OrganizationsDecisionDetail present with AllowedByOrganizations=true, EvalDecisionDetails present as an empty member, and the condition key the evaluated policy needs but the request did not supply listed in MissingContextValues both top-level and mirrored under ResourceSpecificResults.
Emulator: all four members are absent from the response.
Why: a caller cannot distinguish 'no SCP evaluated' from 'allowed by Organizations', and a caller using the simulator to discover which ContextEntries it must supply reads an absent list as 'nothing missing' and treats a conditional, context-dependent allow as unconditional — the pre-flight permission check silently over-reports access.
Fix: the SimulatePrincipalPolicy result projection (EvaluationResult / ResourceSpecificResult member serialization) and the context-key collection step feeding MissingContextValues.
Open issue 20261103T132715Z-iam-simulateprincipalpolicy-evaluation-results-omit-modeled-detail-members.md describes the same family of omissions but its replay keys do not cover this step's key set, so this cause is unexercised by it.
Traces: iam-ebecs-events-role-run-task-pass-role-simulation#task#9.

Cause 5 of 5: ListAttachedUserPolicies reports no attached managed policies for the calling identity.
AWS: Smithy ListAttachedUserPolicies output AttachedPolicies is attachedPoliciesListType, each AttachedPolicy carrying PolicyName and PolicyArn. The recordings read back the operator/caller user and AWS returns the AWS-managed administrator policy attached to it — PolicyName equal to the managed policy's name and PolicyArn of the form arn:aws:iam::aws:policy/<PolicyName> (fixed 'arn:aws:iam::aws:policy/' prefix, name taken from the attachment in state).
Emulator: returns an AttachedPolicies element with no members at all (an empty string where a member structure is expected), i.e. the ambient caller principal has no managed-policy attachment in emulator state.
Why: a read-only identity-inspection flow — the standard 'what can I do?' bootstrap in SDKs, CLIs and consoles — concludes the caller has no permissions, and any client that gates subsequent calls on finding an administrator attachment refuses to proceed against the emulator.
Fix: the seeded state for the ambient caller user (its managed-policy attachments) and, for the empty case, the ListAttachedUserPolicies serializer, which must emit a well-formed empty list rather than an empty scalar.
Traces: iam-list-caller-policies-readonly#task#3, iam-read-only-inspection-of-operator-identity#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (219 fixtures, 1595 steps) against the emulator built from this checkout. All five clusters report stub_path empty and owned_by empty, i.e. net-new and not covered by any open issue's replay key.

Cause 1 — iam-ekscc-traj-attach-cluster-policy-mistakes#teardown#12, DeleteRole: status expected 200, got 409. Same cluster: iam-emr-roles-policy-path-and-rerun-mistakes#teardown#19, iam-gpn-traj-attach-glue-service-role-to-the-worker#teardown#11 (3 steps).

Cause 2 — iam-acquire-role-backup-service-lifecycle#task#1, AcquireRole, 200 vs 200: $.AcquireRoleResponse.AcquireRoleResult.Role.AssumeRolePolicyDocument expected '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22backup.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%2C...%7D%5D%7D', got '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"backup.amazonaws.com"},"Action":"sts:AssumeRole",...}]}' — identical JSON, missing percent-encoding. Also iam-acquire-role-power-user-lifecycle#task#1 (2 steps).

Cause 3 — iam-acquire-role-backup-service-lifecycle#task#3, ListAttachedRolePolicies, 200 vs 200: member[0].PolicyName expected 'AWSBackupServiceRolePolicyForRestores', got 'AWSBackupServiceRolePolicyForBackup'; member[1].PolicyName expected 'AWSBackupServiceRolePolicyForBackup', got 'AWSBackupServiceRolePolicyForRestores' (and the matching PolicyArn pair) — the same two entries, transposed. Also iam-acquire-role-backup-service-lifecycle#teardown#4 (2 steps).

Cause 4 — iam-ebecs-events-role-run-task-pass-role-simulation#task#9, SimulatePrincipalPolicy, 200 vs 200: $...EvaluationResults.member.OrganizationsDecisionDetail expected {'AllowedByOrganizations': 'true'}, got None; .EvalDecisionDetails expected '' (present-but-empty), got None; .MissingContextValues.member[0] expected 'ecs:cluster', got None; .ResourceSpecificResults.member.MissingContextValues.member[0] expected 'ecs:cluster', got None (1 step).

Cause 5 — iam-list-caller-policies-readonly#task#3, ListAttachedUserPolicies, 200 vs 200: $.ListAttachedUserPoliciesResponse.ListAttachedUserPoliciesResult.AttachedPolicies type_mismatch — expected {'member': {'PolicyArn': 'arn:aws:iam::aws:policy/AdministratorAccess', 'PolicyName': 'AdministratorAccess'}}, got ''. Also iam-read-only-inspection-of-operator-identity#task#6 (2 steps).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay fixture iam-ekscc-traj-attach-cluster-policy-mistakes and compare step teardown#12 (DeleteRole) against the recording: the role's policies were detached/deleted earlier in the same fixture, AWS answers 200, the emulator answers 409.
3. Replay the steps named in trace_ids, one per remaining cause:
   - iam-acquire-role-backup-service-lifecycle#task#1 (AcquireRole) — diff Role.AssumeRolePolicyDocument and check it is percent-encoded.
   - iam-acquire-role-backup-service-lifecycle#task#3 (ListAttachedRolePolicies) — diff the order of AttachedPolicies.member[0]/member[1].
   - iam-ebecs-events-role-run-task-pass-role-simulation#task#9 (SimulatePrincipalPolicy) — diff EvaluationResults.member for OrganizationsDecisionDetail, EvalDecisionDetails, MissingContextValues and ResourceSpecificResults.member.MissingContextValues.
   - iam-list-caller-policies-readonly#task#3 (ListAttachedUserPolicies) — diff AttachedPolicies; it must be a member list naming the caller's AWS-managed administrator attachment, not an empty scalar.
4. Re-replay all of the above plus the other cluster members (iam-emr-roles-policy-path-and-rerun-mistakes#teardown#19, iam-gpn-traj-attach-glue-service-role-to-the-worker#teardown#11, iam-acquire-role-power-user-lifecycle#task#1, iam-acquire-role-backup-service-lifecycle#teardown#4, iam-read-only-inspection-of-operator-identity#task#6) to confirm all five causes are closed.
