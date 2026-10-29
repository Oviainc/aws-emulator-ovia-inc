Issue-Source: trace-sweep-divergence
Issue-Trace: iam-err-malformed-policy-create-policy
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-err-malformed-policy-create-policy iam-err-malformed-policy-put-user-policy iam-err-malformed-policy-trust-policy
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|CreatePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: iam|CreatePolicy|4xx->2xx|status
Issue-Key: iam|CreatePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|CreatePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|CreatePolicy|4xx->4xx|status
Issue-Key: iam|CreatePolicy|2xx->4xx|$.CreatePolicyResponse
Issue-Key: iam|CreatePolicy|2xx->4xx|status
Issue-Key: iam|PutUserPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: iam|PutUserPolicy|4xx->2xx|status
Issue-Key: iam|CreateRole|4xx->2xx|$.ErrorResponse
Issue-Key: iam|CreateRole|4xx->2xx|status
Issue-Key: iam|CreateRole|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|CreateRole|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|CreateRole|4xx->4xx|status
Issue-Key: iam|CreateRole|2xx->4xx|$.CreateRoleResponse
Issue-Key: iam|CreateRole|2xx->4xx|status
Issue-Key: iam|UpdateAssumeRolePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: iam|UpdateAssumeRolePolicy|4xx->2xx|status
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Malformed IAM policy documents are accepted instead of MalformedPolicyDocument

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
Four write paths perform no policy-document semantic validation, so documents AWS refuses are stored with HTTP 200. Because the first (invalid) create succeeds, each fixture's subsequent attempts then collide on the name and are refused EntityAlreadyExists 409, and the final valid create is refused too - so one missing validation also inverts the outcome of every later step in the fixture.

Cause 1 of 4 - CreatePolicy. AWS: an identity policy document is validated before storage and a semantically invalid one is refused HTTP 400 with Code MalformedPolicyDocument (Smithy: CreatePolicy errors include MalformedPolicyDocumentException, httpError 400, awsQueryError code MalformedPolicyDocument). The message states the rule that was broken, e.g. 'Policy document should not specify a principal.' for an identity policy carrying Principal, and 'Actions/Conditions must be prefaced by a vendor, e.g., iam, sdb, ec2, etc.' for an unprefixed action - fixed text per rule, with no request values interpolated. Emulator: HTTP 200, the document is stored. Why: a client that relies on IAM to reject an unsafe or unparseable policy gets a policy that exists in the emulator but would never exist in AWS; the deployment diverges silently. Fix: policy-document validation in the CreatePolicy handler (shared validator for all policy-document inputs).
Traces: iam-err-malformed-policy-create-policy#task#1 (200 instead of 400), #task#2 (409 EntityAlreadyExists instead of 400 MalformedPolicyDocument - cascade of the accepted create), #task#10 (409 instead of 200 for the valid document).

Cause 2 of 4 - PutUserPolicy. AWS: same validation on the inline user policy; a document specifying a Principal is refused 400 MalformedPolicyDocument 'Policy document should not specify a principal.'. Emulator: HTTP 200. Fix: run the same validator in PutUserPolicy (and the group/role inline equivalents).
Traces: iam-err-malformed-policy-put-user-policy#task#2.

Cause 3 of 4 - CreateRole trust policy. AWS: the AssumeRolePolicyDocument is validated as a trust policy - it must name a Principal and may only grant STS AssumeRole actions. A document with no Principal is refused 400 MalformedPolicyDocument 'Missing required field Principal'; one granting a non-STS action is refused 'AssumeRole policy may only specify STS AssumeRole actions.'. Emulator: HTTP 200, role created. Fix: trust-policy validation in the CreateRole handler.
Traces: iam-err-malformed-policy-trust-policy#task#1 (200 instead of 400), #task#2 (409 instead of 400 - cascade), #task#5 (409 instead of 200 for the valid trust policy).

Cause 4 of 4 - UpdateAssumeRolePolicy. AWS: the same trust-policy rules apply on update; a Principal-less document is refused 400 MalformedPolicyDocument 'Missing required field Principal'. Emulator: HTTP 200 and the role's trust policy is replaced with the invalid document. Why: the role becomes unassumable-by-nobody in a way AWS would never allow, and the client gets no signal. Fix: run the trust-policy validator in UpdateAssumeRolePolicy.
Traces: iam-err-malformed-policy-trust-policy#task#6.

Note: open issues 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four and 20261027T214234Z-iam-6-net-new-behavioral-root-causes-enablemfadevice describe policy-document validation behaviorally; neither carries a replay key for these fixture steps.

## Evidence
Cause 1: expected (recorded AWS) HTTP 400, ErrorResponse/Error{Type Sender, Code MalformedPolicyDocument, Message 'Policy document should not specify a principal.'}; actual HTTP 200 with a CreatePolicyResponse. Downstream, expected HTTP 400 Code MalformedPolicyDocument Message 'Actions/Conditions must be prefaced by a vendor, e.g., iam, sdb, ec2, etc.'; actual HTTP 409 Code EntityAlreadyExists Message 'A policy called <policyName> already exists. Duplicate names are not allowed.' (the name is the request's PolicyName). Final step: expected HTTP 200 CreatePolicyResult/Policy; actual HTTP 409.
Cause 2: expected HTTP 400 Code MalformedPolicyDocument Message 'Policy document should not specify a principal.'; actual HTTP 200.
Cause 3: expected HTTP 400 Code MalformedPolicyDocument Message 'Missing required field Principal'; actual HTTP 200. Downstream expected 400 MalformedPolicyDocument 'AssumeRole policy may only specify STS AssumeRole actions.'; actual 409 EntityAlreadyExists 'Role with name <roleName> already exists.'.
Cause 4: expected HTTP 400 Code MalformedPolicyDocument Message 'Missing required field Principal'; actual HTTP 200.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-err-malformed-policy-create-policy#task#1: CreatePolicy with a document containing a Principal -> 200 instead of 400 MalformedPolicyDocument; task#2..#9 then get 409 EntityAlreadyExists instead of 400; task#10's valid document gets 409 instead of 200.
3. Replay iam-err-malformed-policy-put-user-policy#task#2 (PutUserPolicy with Principal) -> 200.
4. Replay iam-err-malformed-policy-trust-policy#task#1 (CreateRole with a Principal-less trust policy) and #task#6 (UpdateAssumeRolePolicy with the same) -> 200.
