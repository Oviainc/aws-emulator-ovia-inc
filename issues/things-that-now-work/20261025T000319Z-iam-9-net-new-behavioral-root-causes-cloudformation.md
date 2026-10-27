Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.deploy-iam-managed-policy-cloudformation-stack-delete-cascades iam.deploy-iam-managed-policy-via-cloudformation-attaches-and-enforces iam.deploy-iam-user-cfn-delete-removes-user-with-login-profile iam.deploy-iam-user-cfn-path-update-preserves-user-id iam.deploy-iam-user-cloudformation-stack-update-replaces-inline-policy iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected iam.deploy-iam-role-cloudformation-stack-update-changes-trust-policy iam.role-delete-with-attached-policy-rejected iam.deploy-iam-role-via-cloudformation-trust-policy-enforced iam.reject-delete-role-still-in-instance-profile iam.instance-profile-delete-with-role-conflict-rejected iam.permissions-boundary-lifecycle iam.iam-bound-deep-iam-3-managed-policy-create-over-6144-significant-chars-rejected iam.iam-bound-deep-iam-3-policy-size-whitespace-inside-strings-excluded-filled-rejected iam.iam-bound-deep-iam-3-role-inline-policy-over-10240-significant-chars-rejected iam.tag-policy-exceeds-limit-rejected iam.delegation-request-workflow-lifecycle iam.malformed-json-policy iam.trust-policy-validation iam.delete-instance-profile iam.delete-permissions-boundary-service-linked-role-rejected iam.delete-policy-conflict-while-attached-rejected iam.delete-policy-while-attached iam.get-human-readable-summary iam.get-nonexistent-entity-returns-nosuchentity iam.op-contract-generateOrganizationsAccessReport iam.op-contract-getDelegationRequest iam.tag-instance-profile-reserved-prefix-rejected iam.non-user-caller-must-specify-username-rejected iam.sep-policy-composition-managed-grant-eventually-applies-and-revocation-eventually-denied iam.concurrent-add-client-id-race-consistent-final-state iam.concurrent-instance-profile-tag-update-conflict iam.concurrent-oidc-thumbprint-update-conflict iam.concurrent-tag-oidc-provider-optimistic-lock iam.concurrent-update-saml-provider-race-accepted
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetPolicy|InvalidInput
Issue-Key: iam|CreateLoginProfile|NoSuchEntity
Issue-Key: iam|GetUser|NoSuchEntity
Issue-Key: iam|GetUserPolicy|NoSuchEntity
Issue-Key: iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected|run#10|AssertionError|assert final_status in ("ROLLBACK_COMPLETE", "CREATE_FAILED"), (
Issue-Key: iam.deploy-iam-role-cloudformation-stack-update-changes-trust-policy|run#14|WaiterError|cfn.get_waiter("stack_update_complete").wait(
Issue-Key: iam.role-delete-with-attached-policy-rejected|run#10|AssertionError|assert message == want, f"DeleteConflict message: expected {want!r}, got {message!r}"
Issue-Key: iam.deploy-iam-role-via-cloudformation-trust-policy-enforced|run#23|AssertionError|assert code_msg == "Cannot delete entity, must detach all policies first.", f"unexpected error message {code_msg!r}"
Issue-Key: iam.reject-delete-role-still-in-instance-profile|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.instance-profile-delete-with-role-conflict-rejected|run#18|AssertionError|assert msg == want, f"DeleteConflict message: wanted {want!r}, got {msg!r}"
Issue-Key: iam.permissions-boundary-lifecycle|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.iam-bound-deep-iam-3-managed-policy-create-over-6144-significant-chars-rejected|run#25|AssertionError|assert message == expected, f"message: expected {expected!r}, got {message!r}"
Issue-Key: iam.iam-bound-deep-iam-3-policy-size-whitespace-inside-strings-excluded-filled-rejected|run#18|AssertionError|assert message == expected, f"message: expected {expected!r}, got {message!r}"
Issue-Key: iam.iam-bound-deep-iam-3-role-inline-policy-over-10240-significant-chars-rejected|run#10|AssertionError|assert message == expected, f"message: expected {expected!r}, got {message!r}"
Issue-Key: iam.tag-policy-exceeds-limit-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.delegation-request-workflow-lifecycle|run#7|AssertionError|assert message_create == expected_create, (
Issue-Key: iam.malformed-json-policy|run#9|AssertionError|assert message == "Syntax errors in policy.", (
Issue-Key: iam.trust-policy-validation|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.delete-instance-profile|run#11|AssertionError|assert mutate_code == ERR_NO_SUCH_ENTITY, (
Issue-Key: iam.delete-permissions-boundary-service-linked-role-rejected|run#7|AssertionError|assert control_code == "NoSuchEntity", (
Issue-Key: iam.delete-policy-conflict-while-attached-rejected|run#11|AssertionError|assert code == "DeleteConflict", f"expected DeleteConflict deleting an attached policy, got {code!r}"
Issue-Key: iam.delete-policy-while-attached|run#10|AssertionError|assert code == ERR_DELETE_CONFLICT, f"Expected {ERR_DELETE_CONFLICT}, got {code!r}"
Issue-Key: iam.get-human-readable-summary|run#7|AssertionError|assert code_wrong_type == "ValidationError", (
Issue-Key: iam.get-nonexistent-entity-returns-nosuchentity|_expect#6|AssertionError|assert got_code == code, f"{label}: expected {code}, got {got_code}"
Issue-Key: iam.op-contract-generateOrganizationsAccessReport|run#8|AssertionError|assert code in _EXPECTED_NEGATIVE_CODES, f"unexpected error code: {code!r}"
Issue-Key: iam.tag-instance-profile-reserved-prefix-rejected|run#7|AssertionError|assert code == "InvalidInput", f"expected InvalidInput, got {code}"
Issue-Key: iam.non-user-caller-must-specify-username-rejected|run#15|AssertionError|assert isinstance(uploaded, dict), (
Issue-Key: iam.sep-policy-composition-managed-grant-eventually-applies-and-revocation-eventually-denied|run#16|AssertionError|assert before == "AccessDenied", f"ListUserTags is not granted yet; got {before!r}"
Issue-Key: iam.concurrent-add-client-id-race-consistent-final-state|run#14|AssertionError|assert status in retryable_codes, (
Issue-Key: iam.concurrent-instance-profile-tag-update-conflict|run#12|AssertionError|assert len(successes) >= 1, f"expected at least one successful tag write, got results={results}"
Issue-Key: iam.concurrent-oidc-thumbprint-update-conflict|run#11|AssertionError|assert len(successes) >= 1, f"expected at least one winner, got results={results}"
Issue-Key: iam.concurrent-tag-oidc-provider-optimistic-lock|run#15|AssertionError|assert code == "ConcurrentModification", (
Issue-Key: iam.concurrent-update-saml-provider-race-accepted|run#17|AssertionError|assert code == "ConcurrentModification", (
Issue-Related: 20261025T000319Z-iam-getdelegationrequest-stub.md
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# iam: 9 net-new behavioral root causes — CloudFormation-deployed IAM resources are not real, refusal messages are generated prose instead of AWS's text, and scaffolded operations hide every negative path

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Nine further distinct net-new root causes in the bootstrapped `iam` service, disjoint from the ten in the companion iam issue filed in this same sweep. None is marked ALREADY FILED or OWNED in the replay report. A fixer must close all nine before submitting.

Cause 1 of 9: a CloudFormation-deployed AWS::IAM::ManagedPolicy reports its name as the resource's physical id, not its ARN.
AWS: the physical resource id of AWS::IAM::ManagedPolicy is the policy ARN, and Ref returns that ARN; IAM GetPolicy takes PolicyArn (Smithy GetPolicy.PolicyArn target arnType, length 20-2048) and models InvalidInputException (InvalidInput 400) for a value that is not an ARN.
Emulator: the stack hands back a bare policy name, and GetPolicy on it fails with `InvalidInput: ARN sim2real-behav-policy-59080688ef25 is not valid.` — so no CFN-deployed managed policy can be read back, attached or cascaded on stack delete.
Why: every template that passes a managed-policy Ref to another resource (ManagedPolicyArns, PermissionsBoundary, a role attachment) wires a name where an ARN is required; the deploy appears to succeed and the dependent grant silently never exists.
Fix: the AWS::IAM::ManagedPolicy resource provider in the CloudFormation service crate must set PhysicalResourceId/Ref to the created policy's ARN (and GetPolicy in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_policy.rs keeps refusing non-ARNs, which is correct).
Traces: iam.deploy-iam-managed-policy-via-cloudformation-attaches-and-enforces, iam.deploy-iam-managed-policy-cloudformation-stack-delete-cascades.

Cause 2 of 9: a stack declaring AWS::IAM::User reaches a terminal success state without the user existing in IAM.
AWS: when CreateStack reports CREATE_COMPLETE for a template with AWS::IAM::User, GetUser/GetUserPolicy/CreateLoginProfile for that user name succeed.
Emulator: after the stack settles, IAM answers `NoSuchEntity: The user with name sim2real-behav-user-7fbaef8cf51e cannot be found.` on GetUser, the same on GetUserPolicy, and CreateLoginProfile for the deployed user is refused NoSuchEntity 404.
Why: the stack state lies — a client provisions users (and their login profiles, inline policies, paths and UserId identity) through CloudFormation and every subsequent IAM call against them 404s, so no deployed-identity workflow can run.
Fix: the AWS::IAM::User resource provider must actually create the user (and its inline policies/login profile/path) in the iam service state (src/state.rs) during stack create/update, and fail the stack when it cannot.
Traces: iam.deploy-iam-user-cfn-path-update-preserves-user-id, iam.deploy-iam-user-cfn-delete-removes-user-with-login-profile, iam.deploy-iam-user-cloudformation-stack-update-replaces-inline-policy.

Cause 3 of 9: a stack whose AWS::IAM::Role carries a malformed trust policy reaches CREATE_COMPLETE.
AWS: CreateRole refuses a malformed AssumeRolePolicyDocument with MalformedPolicyDocument 400 (modeled on CreateRole), so the resource create fails and the stack ends CREATE_FAILED/ROLLBACK_COMPLETE.
Emulator: the stack ended 'CREATE_COMPLETE'.
Why: a template with a broken trust policy is reported as a successful deploy, so the error surfaces much later as an un-assumable role; CI gates that key on stack status never fire.
Fix: the AWS::IAM::Role provider must surface the IAM validation failure (see also cause 5 of the companion issue: the IAM handler itself does not validate the document) and roll the stack back.
Traces: iam.deploy-iam-role-cloudformation-malformed-trust-policy-rejected.

Cause 4 of 9: a stack update that changes a role's trust policy ends UPDATE_FAILED.
AWS: updating AWS::IAM::Role's AssumeRolePolicyDocument is an in-place update (UpdateAssumeRolePolicy) and the stack reaches UPDATE_COMPLETE.
Emulator: `WaiterError: Waiter StackUpdateComplete failed: ... we matched expected path: "UPDATE_FAILED"`.
Why: a client cannot evolve a trust policy through its template at all — the ordinary edit path for cross-account/service trust is blocked, and the stack is left in a failed state.
Fix: the AWS::IAM::Role provider's update path (map AssumeRolePolicyDocument changes to UpdateAssumeRolePolicy rather than failing).
Traces: iam.deploy-iam-role-cloudformation-stack-update-changes-trust-policy.

Cause 5 of 9: refusal messages for the delete-conflict and boundary-not-attachable families are generated prose instead of AWS's fixed text.
AWS: these are AWS's fixed strings, not request-derived prose, and the recordings show them verbatim: DeleteConflict on a role with attachments is 'Cannot delete entity, must detach all policies first.'; deleting a role still in an instance profile is 'Cannot delete entity, must remove roles from instance profile first.'; deleting an instance profile that still holds a role is 'Cannot delete entity, must remove roles from instance profile first.'; PutRolePermissionsBoundary/PutUserPermissionsBoundary with a nonexistent policy is 'Scope ARN: <arn> does not exist or is not attachable.' (modeled PolicyNotAttachableException / 400). The status codes and codes themselves are correct here; only the message bodies diverge.
Emulator: 'Cannot delete entity: 1 managed policy must be detached from the role <role> first.', 'Cannot delete entity: the role must be removed from instance profile <profile> from the role <role> first.' (also ungrammatical), 'Cannot delete entity: the role <role> must be removed from instance profile <profile> first.', and 'Policy <arn> does not exist or is not attachable.' — each interpolating resource names AWS does not include.
Why: clients and runbooks match on AWS's exact text to decide whether to detach-then-retry; a reworded message breaks that branch, and the emulator's variants additionally leak which entity blocked the delete.
Fix: the message constants in src/ops/delete_role.rs, src/ops/delete_instance_profile.rs, src/ops/delete_policy.rs and src/ops/put_role_permissions_boundary.rs / put_user_permissions_boundary.rs.
Traces: iam.role-delete-with-attached-policy-rejected, iam.deploy-iam-role-via-cloudformation-trust-policy-enforced, iam.reject-delete-role-still-in-instance-profile, iam.instance-profile-delete-with-role-conflict-rejected, iam.permissions-boundary-lifecycle.

Cause 6 of 9: policy-size quota refusals use the wrong message template.
AWS: an over-quota managed policy document is refused with 'Cannot exceed quota for PolicySize: 6144', and an over-quota role inline policy with 'Maximum policy size of 10240 bytes exceeded for role <role>' — the unit word is 'bytes' and the managed-policy case names the quota, not the policy. (The 6144/10240 significant-character limits themselves are enforced correctly; only the template diverges.)
Emulator: 'Maximum policy size of 6144 characters exceeded for policy <name>' for the managed-policy cases and 'Maximum policy size of 10240 characters exceeded for role <role>' for the inline case.
Why: tooling that distinguishes 'quota' refusals from per-entity size refusals by message cannot, and the unit reported to the user is wrong.
Fix: the size-refusal message templates in src/ops/create_policy.rs and src/ops/put_role_policy.rs.
Traces: iam.iam-bound-deep-iam-3-managed-policy-create-over-6144-significant-chars-rejected, iam.iam-bound-deep-iam-3-policy-size-whitespace-inside-strings-excluded-filled-rejected, iam.iam-bound-deep-iam-3-role-inline-policy-over-10240-significant-chars-rejected.

Cause 7 of 9: constraint-violation messages echo the rejected value where AWS omits it.
AWS: the message is built from the request by a known rule — 'N validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must ...' — and for these members AWS does not interpolate the offending value (recorded: "Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50" and "Value at 'sessionDuration' failed to satisfy constraint: Member must have value less than or equal to 43200").
Emulator: interpolates it — "Value '51 items' at 'tags' ..." and "Value '999999' at 'sessionDuration' ...".
Why: the member name and constraint are the contract clients parse; adding a synthesized value rendering ('51 items') diverges from that rule and can echo caller input back in an error surface.
Fix: the shared constraint-violation message builder used by the tagging and delegation-request handlers in src/ops/.
Traces: iam.tag-policy-exceeds-limit-rejected, iam.delegation-request-workflow-lifecycle.

Cause 8 of 9: malformed/empty policy documents are refused with a descriptive message instead of AWS's fixed text.
AWS: a document that is not parseable JSON is refused with exactly 'Syntax errors in policy.', and a document with no Statement element with 'Policy has no statements' (MalformedPolicyDocument 400 in both cases).
Emulator: 'Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element' and 'Syntax errors in policy: AssumeRolePolicyDocument must be a JSON object carrying a Statement element' — one message for two distinct AWS cases, neither matching.
Why: the two conditions are indistinguishable to a client, and matchers on AWS's exact strings fail.
Fix: the policy-document parse/validate path shared by src/ops/create_policy.rs and src/ops/create_role.rs — distinguish 'unparseable' from 'no statements' and use AWS's constants.
Traces: iam.malformed-json-policy, iam.trust-policy-validation.

Cause 9 of 9: calls that land on scaffolded IAM operations surface NotImplemented, so every negative path and concurrency contract behind them is unobservable.
AWS: each of these operations answers a modeled error or a defined success: tagging a deleted instance profile -> NoSuchEntity 404; DeletePermissionsBoundary on a role with no boundary -> NoSuchEntity 404; DeletePolicy on an attached policy -> DeleteConflict 409; GetGroup on an absent group -> NoSuchEntity 404; GetHumanReadableSummary with a wrong-resource-type EntityArn -> ValidationError 400 and with an absent one -> NoSuchEntity 404; GetDelegationRequest for an absent request -> NoSuchEntity 404; GenerateOrganizationsAccessReport -> its modeled Organizations error; TagInstanceProfile with a reserved 'aws:' tag prefix -> InvalidInput 400; UploadSigningCertificate with no UserName from a user-signed call -> implicit-caller resolution success; and concurrent AddClientIDToOpenIDConnectProvider / TagInstanceProfile / UpdateOpenIDConnectProviderThumbprint / TagOpenIDConnectProvider / UpdateSAMLProvider racers -> at least one winner with losers refused ConcurrentModification 409 only.
Emulator: every one answers error code 'NotImplemented' with 'operation <X> is scaffolded but not implemented in this emulator'. Individual per-operation stub issues exist (e.g. 20261024T065715Z-iam-attachuserpolicy-stub.md, 20261023T140640Z-iam-uploadservercertificate-stub.md, 20261022T131025Z-iam-simulateprincipalpolicy-stub.md); the replay report did not attribute these checks to any of them, and implementing the stub alone is not enough — each handler must also carry the not-found/conflict/validation and optimistic-locking semantics listed above.
Why: a client sees a single server-side 'Receiver/NotImplemented' where AWS distinguishes not-found, conflict, validation and contention; retry logic keyed on ConcurrentModification spins forever, and error-handling branches are never exercised.
Fix: implement the handlers in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/ (replacing src/scaffolded.rs dispatch) together with the per-entity existence/conflict checks and a version/optimistic-lock on the OIDC-provider, SAML-provider and instance-profile tag records in src/state.rs.
Traces: iam.delete-instance-profile, iam.delete-permissions-boundary-service-linked-role-rejected, iam.delete-policy-conflict-while-attached-rejected, iam.delete-policy-while-attached, iam.get-nonexistent-entity-returns-nosuchentity, iam.get-human-readable-summary, iam.op-contract-getDelegationRequest, iam.op-contract-generateOrganizationsAccessReport, iam.tag-instance-profile-reserved-prefix-rejected, iam.non-user-caller-must-specify-username-rejected, iam.sep-policy-composition-managed-grant-eventually-applies-and-revocation-eventually-denied, iam.concurrent-add-client-id-race-consistent-final-state, iam.concurrent-instance-profile-tag-update-conflict, iam.concurrent-oidc-thumbprint-update-conflict, iam.concurrent-tag-oidc-provider-optimistic-lock, iam.concurrent-update-saml-provider-race-accepted.

## Evidence
Cause 1: botocore.errorfactory.InvalidInputException: An error occurred (InvalidInput) when calling the GetPolicy operation: ARN sim2real-behav-policy-59080688ef25 is not valid. (also 'ARN sim2real-cfnpolydel-c55db0db66dc-policy is not valid.'). Expected: the stack's managed-policy id is an ARN and GetPolicy returns 200.
Cause 2: botocore.errorfactory.NoSuchEntityException: ... GetUser ... The user with name sim2real-behav-user-7fbaef8cf51e cannot be found.; same for CreateLoginProfile and GetUserPolicy after the stack settled.
Cause 3: AssertionError: expected ROLLBACK_COMPLETE or CREATE_FAILED for a malformed trust policy, got 'CREATE_COMPLETE'.
Cause 4: botocore.exceptions.WaiterError: Waiter StackUpdateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "UPDATE_FAILED" at least once.
Cause 5: AssertionError: DeleteConflict message: expected 'Cannot delete entity, must detach all policies first.', got 'Cannot delete entity: 1 managed policy must be detached from the role sim2real-roledelconflict-419102e073f1 first.'; AssertionError: expected message 'Cannot delete entity, must remove roles from instance profile first.', got 'Cannot delete entity: the role must be removed from instance profile sim2real-behav-iam-ip-profile-f14efbc1aece from the role sim2real-behav-iam-ip-role-0059f89ca5d7 first.'; AssertionError: expected message 'Scope ARN: arn:aws:iam::000000000000:policy/... does not exist or is not attachable.', got 'Policy arn:aws:iam::000000000000:policy/... does not exist or is not attachable.'
Cause 6: AssertionError: message: expected 'Cannot exceed quota for PolicySize: 6144', got 'Maximum policy size of 6144 characters exceeded for policy sim2real-psz-1831327d8bec-over'; AssertionError: message: expected 'Maximum policy size of 10240 bytes exceeded for role sim2real-psr-b3a62c21a0c9', got '... 10240 characters exceeded for role ...'.
Cause 7: AssertionError: expected message "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", got "... Value '51 items' at 'tags' ..."; same shape for sessionDuration with Value '999999'.
Cause 8: AssertionError: the malformed document must be refused with AWS's exact message 'Syntax errors in policy.'; got 'Syntax errors in policy: PolicyDocument must be a JSON object carrying a Statement element'; AssertionError: expected message 'Policy has no statements', got 'Syntax errors in policy: AssumeRolePolicyDocument must be a JSON object carrying a Statement element'.
Cause 9: AssertionError: expected NoSuchEntity tagging a deleted profile, got 'NotImplemented'; AssertionError: Expected DeleteConflict, got 'NotImplemented'; AssertionError: GetGroup: expected NoSuchEntity, got NotImplemented; AssertionError: expected ValidationError for a wrong-resource-type EntityArn (role), got NotImplemented; AssertionError: expected NoSuchEntity, got {'Type': 'Receiver', 'Code': 'NotImplemented', ...}; AssertionError: expected InvalidInput, got NotImplemented; AssertionError: expected only ConcurrentModification for losing racers, got 'NotImplemented'; AssertionError: expected at least one successful tag write, got results=[('error','NotImplemented') x10].

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: deploy a stack with AWS::IAM::ManagedPolicy, read the resource's physical id, pass it to iam GetPolicy(PolicyArn=...) — observe InvalidInput.
3. Cause 2: deploy a stack with AWS::IAM::User (with a login profile and an inline policy), wait for CREATE_COMPLETE, then call GetUser/GetUserPolicy/CreateLoginProfile for that user name — observe NoSuchEntity.
4. Cause 3/4: deploy AWS::IAM::Role with a malformed AssumeRolePolicyDocument (observe CREATE_COMPLETE); deploy a valid one and update only its trust policy (observe UPDATE_FAILED).
5. Cause 5: attach a managed policy to a role then DeleteRole; add a role to an instance profile then DeleteRole and DeleteInstanceProfile; PutRolePermissionsBoundary with a nonexistent policy ARN — compare message text.
6. Cause 6: CreatePolicy with a >6144 significant-character document and PutRolePolicy with a >10240 one — compare message text.
7. Cause 7: TagPolicy with 51 tags; create a delegation request with sessionDuration=999999 — compare message text.
8. Cause 8: CreatePolicy with unparseable JSON and CreateRole with a document carrying no Statement — compare message text.
9. Cause 9: call each operation named in that cause (and run the five concurrent racer checks) — observe NotImplemented instead of the modeled not-found/conflict/validation/ConcurrentModification outcomes.
10. Re-replay the check ids in trace_ids to confirm.
