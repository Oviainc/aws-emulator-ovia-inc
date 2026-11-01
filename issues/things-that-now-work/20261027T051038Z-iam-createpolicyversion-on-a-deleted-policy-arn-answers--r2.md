Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.e2e-service-role-setup
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.e2e-service-role-setup|run#43|AssertionError|assert got == policy_unattachable, (
Issue-Commit: 4846872b237bc4d9f92d9945507291eab648a721

# iam: CreatePolicyVersion on a deleted policy ARN answers NoSuchEntity with the wrong message — 'was not found.' instead of AWS's 'does not exist or is not attachable.'

> _Note: this issue was identified against the code base at commit `4846872b237bc4d9f92d9945507291eab648a721`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyVersion

## Description
REOPENS 20261026T100135Z-iam-createpolicyversion-on-a-deleted-policy-arn-answers.md: its fix left the call refused. That issue was filed and marked fixed against this single replay key (`iam.e2e-service-role-setup`, its only Issue-Trace-Id), and the check still fails on the same CreatePolicyVersion refusal. The error code and status it set out to correct are now right — the scaffold's 501/NotImplemented is gone and the call is refused NoSuchEntity — but the refusal's message is not the one AWS emits, so the check the fix was gated on does not pass. This is not a new defect; it is the residue of that fix.

Cause 1 of 1: the not-found refusal for a managed-policy ARN that CreatePolicyVersion cannot resolve uses the generic policy-not-found sentence instead of IAM's CreatePolicyVersion wording.
AWS: Smithy iam#CreatePolicyVersion takes PolicyArn (required, arnType, @length 20-2048) and PolicyDocument (required, policyDocumentType) and models NoSuchEntityException (awsQueryError code NoSuchEntity, httpResponseCode 404), documented as "The request was rejected because it referenced a resource entity that does not exist. The error message describes the resource." Recorded behavior for an ARN whose policy was just deleted: code NoSuchEntity, HTTP 404, message "Policy <policyArn> does not exist or is not attachable." — fixed text "Policy " + " does not exist or is not attachable.", with one variable part, the PolicyArn copied verbatim from the request. The "or is not attachable" clause is part of the fixed text: IAM uses one sentence for both "no such managed policy" and "this ARN names a policy you cannot attach a version to", which is what makes the two indistinguishable to a caller by design.
Emulator: the same call is refused with the correct code and status but the message "Policy <policyArn> was not found." — the generic policy-not-found sentence this service uses for GetPolicy/DetachRolePolicy, which drops the "or is not attachable" clause.
Why: this is the leg of the managed-policy lifecycle where an IaC or cleanup loop proves a dead ARN is unusable. A caller that matches IAM's exact message to tell "the policy is gone" from "the ARN is not attachable" — or that simply asserts IAM's text — sees a different string, so the convergence check that the earlier fix was meant to unblock still cannot be written against the emulator. Note the same wording defect in the opposite direction was already filed for a sibling operation (20261026T024842Z-iam-getpolicy-s-nosuchentity-message-says-does-not.md, GetPolicy saying 'does not exist.' where AWS says 'was not found.'), so the two sentences are not interchangeable: each operation has its own fixed text.
Fix: the NoSuchEntity message rendering in the CreatePolicyVersion handler's PolicyArn resolution step — it must emit CreatePolicyVersion's own fixed sentence with the request's PolicyArn, rather than reusing the shared policy-not-found constant.
Traces: iam.e2e-service-role-setup.

## Evidence
Suite simparity-behavioral-ga-prod-v1, replayed against this checkout. `iam` is bootstrapped and CreatePolicyVersion now answers (no NotImplemented), confirming the reopened fix landed partially.

iam.e2e-service-role-setup [mismatch], check line 125:
  AssertionError: CreatePolicyVersion on the deleted policy: expected ('NoSuchEntity', 'Policy arn:aws:iam::000000000000:policy/sim2real/p-f87ea0b3739b/sim2real-behav-pol-9202a69570a8 does not exist or is not attachable.'), got ('NoSuchEntity', 'Policy arn:aws:iam::000000000000:policy/sim2real/p-f87ea0b3739b/sim2real-behav-pol-9202a69570a8 was not found.')

Expected: HTTP 404, Error.Code 'NoSuchEntity' (Smithy NoSuchEntityException on CreatePolicyVersion, awsQueryError NoSuchEntity / httpResponseCode 404), Error.Message = fixed text 'Policy ' + ' does not exist or is not attachable.' with the request's PolicyArn as its only variable part.
Actual: the same code and status with Error.Message = 'Policy <PolicyArn> was not found.' — the account id 000000000000 and the generated policy path/name above are this run's values filling the template, not part of it.

Prior state of this check under the reopened issue: 'AssertionError: expected NoSuchEntity, got NotImplemented' (HTTP 501, 'operation CreatePolicyVersion is scaffolded but not implemented in this emulator'). The code/status half of that divergence is resolved; the message half is not, and the check remains failing.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an `iam` client at it.
2. CreateRole with a lambda.amazonaws.com trust policy under a unique path; CreatePolicy with a minimal s3:GetObject document under the same path.
3. AttachRolePolicy, then DetachRolePolicy, confirming GetPolicy(...).Policy.AttachmentCount goes 1 -> 0.
4. DeletePolicy(PolicyArn=<arn>). Confirm GetPolicy(PolicyArn=<arn>) is refused NoSuchEntity 404 and the ARN is gone from ListPolicies(Scope='Local', PathPrefix=<path>).
5. Call CreatePolicyVersion(PolicyArn=<the deleted arn>, PolicyDocument=<the same document>).
6. Observe NoSuchEntity 404 with message 'Policy <arn> was not found.' instead of AWS's 'Policy <arn> does not exist or is not attachable.'
   Replay: iam.e2e-service-role-setup
