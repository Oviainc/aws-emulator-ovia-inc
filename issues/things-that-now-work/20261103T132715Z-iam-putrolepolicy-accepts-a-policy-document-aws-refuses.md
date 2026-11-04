Issue-Source: trace-sweep-divergence
Issue-Trace: iam-execute-api-grant-rewrite-document-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-execute-api-grant-rewrite-document-mistakes
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|PutRolePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: iam|PutRolePolicy|4xx->2xx|status
Issue-Key: iam|ListRolePolicies|2xx->2xx|$.ListRolePoliciesResponse.ListRolePoliciesResult.PolicyNames
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM PutRolePolicy accepts a policy document AWS refuses as malformed and persists it

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRolePolicy

## Description
PutRolePolicy stores a policy document that AWS rejects, and the stored inline policy then shows up on the role. 2 causes.

Cause 1 of 2: a malformed policy document is accepted with 200 instead of being refused.
AWS: Smithy PutRolePolicy.PolicyDocument is @required (policyDocumentType, @length 1..131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$) and the operation models MalformedPolicyDocumentException with awsQueryError code MalformedPolicyDocument and httpError 400. Recording: 400 with Error.Type Sender, Error.Code MalformedPolicyDocument and the fixed message "The policy failed legacy parsing" (no variable part).
Emulator: 200 — the document passes only the character-range/length constraints and is written.
Why: a caller submitting a policy whose structure AWS cannot parse is told the grant is in place. It believes the role has a permission that, on AWS, was never created, and the mistake surfaces far from its cause.
Fix: PutRolePolicy's validation — parse and structurally validate the policy document (Version/Statement/Effect/Action/Resource) before writing, and refuse with MalformedPolicyDocument.
Traces: iam-execute-api-grant-rewrite-document-mistakes#task#2.

Cause 2 of 2: ListRolePolicies then lists the inline policy that should not exist.
AWS: the recording shows ListRolePoliciesResult.PolicyNames as an empty member, because all three PutRolePolicy attempts in the fixture were refused.
Emulator: PolicyNames contains 'invoke-execute-api'.
Why: the accepted write is observable, so a caller that checks "did my put land?" is confirmed in its mistake; any later remediation step targets a policy name AWS does not have.
Fix: state — the role's inline-policy map must not gain an entry from a refused PutRolePolicy (same root cause as Cause 1).
Traces: iam-execute-api-grant-rewrite-document-mistakes#task#5.

The open issue 20261029T184359Z-iam-malformed-iam-policy-documents-are-accepted-instead-of.md describes this class of fault on other operations but carries no replay key for either step above.

## Evidence
Replay of trace set simparity-wire-aws-v1.

Cause 1 — iam-execute-api-grant-rewrite-document-mistakes#task#2, PutRolePolicy (cluster of 3, also #task#3 and #task#4): status expected 400, got 200; $.ErrorResponse expected {'Error': {'Type': 'Sender', 'Code': 'MalformedPolicyDocument', 'Message': 'The policy failed legacy parsing'}, 'RequestId': '<shaped-not-asserted>'}, got None.

Cause 2 — iam-execute-api-grant-rewrite-document-mistakes#task#5, ListRolePolicies, 200 vs 200: $.ListRolePoliciesResponse.ListRolePoliciesResult.PolicyNames expected '' (empty), got {'member': 'invoke-execute-api'}.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay iam-execute-api-grant-rewrite-document-mistakes and compare step task#2 (PutRolePolicy): the emulator answers 200 where the recording has 400 MalformedPolicyDocument.
3. Continue to step task#5 (ListRolePolicies) and observe the inline policy name present where the recording has an empty PolicyNames.
