Issue-Source: trace-sweep-divergence
Issue-Trace: iam-bap-identity-side-grants-via-group-and-managed-policy
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-bap-identity-side-grants-via-group-and-managed-policy iam-execute-api-role-lookup-mistakes
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetGroupPolicy|2xx->2xx|$.GetGroupPolicyResponse.GetGroupPolicyResult.PolicyName
Issue-Key: iam|GetRolePolicy|2xx->2xx|$.GetRolePolicyResponse.GetRolePolicyResult.PolicyName
Issue-Related: 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM inline policy reads do not URL-encode the document and do not echo the requested policy name

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRolePolicy

## Description
Inline-policy read operations return the policy document as raw JSON instead of the URL-encoded form AWS serves, and return the stored spelling of the policy name instead of the spelling the request used. 2 causes.

Cause 1 of 2: GetGroupPolicy returns a raw-JSON PolicyDocument and a lower-cased PolicyName.
AWS: Smithy GetGroupPolicy output declares PolicyDocument (@required, policyDocumentType) and PolicyName (@required, policyNameType, @pattern ^[\w+=,.@-]+$). The recording shows PolicyDocument percent-encoded (e.g. %7B%22Version%22%3A... for {"Version": ...}) — the variable part is the stored document, the requirement is that it is URL-encoded — and PolicyName returned with the capitalisation the request used ('Bucket-Read'). Hypothesis: IAM resolves an inline policy name case-insensitively and echoes the requested spelling.
Emulator: PolicyDocument is the raw JSON text and PolicyName is the lower-cased stored name ('bucket-read').
Why: SDKs URL-decode PolicyDocument before parsing, so a raw-JSON body comes back with its percent sequences un-decoded or fails to parse; a caller comparing the returned PolicyName with the one it asked for sees a mismatch and concludes it read a different policy.
Fix: the GetGroupPolicy response serializer (document encoding) and the inline-policy lookup's name echo.
Traces: iam-bap-identity-side-grants-via-group-and-managed-policy#task#10.

Cause 2 of 2: GetRolePolicy has the same two defects.
AWS: Smithy GetRolePolicy output declares RoleName, PolicyName and PolicyDocument all @required; the recording shows PolicyDocument percent-encoded and PolicyName echoed as requested ('INVOKE-EXECUTE-API').
Emulator: raw JSON document, and PolicyName returned as the stored 'invoke-execute-api'.
Why: as Cause 1.
Fix: the GetRolePolicy response serializer and the inline-policy lookup's name echo.
Traces: iam-execute-api-role-lookup-mistakes#task#6.

The open issue 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md records the same encoding fault on other operations but carries no replay key for either step above, and does not cover the PolicyName echo.

## Evidence
Replay of trace set simparity-wire-aws-v1; both steps returned 200 with a diverging body.

Cause 1 — iam-bap-identity-side-grants-via-group-and-managed-policy#task#10, GetGroupPolicy: $.GetGroupPolicyResponse.GetGroupPolicyResult.PolicyDocument expected '%7B%22Version%22%3A%20%222012-10-17%22%2C%20%22Statement%22%3A%20%5B%7B%22Sid%22%3A%20%22GroupBucketRead%22...%7D%5D%7D', got '{"Version": "2012-10-17", "Statement": [{"Sid": "GroupBucketRead", ...}]}'; PolicyName expected 'Bucket-Read', got 'bucket-read'.

Cause 2 — iam-execute-api-role-lookup-mistakes#task#6, GetRolePolicy: $.GetRolePolicyResponse.GetRolePolicyResult.PolicyDocument expected '%7B%22Version%22%3A%20%222012-10-17%22%2C%20%22Statement%22%3A%20%5B%7B%22Sid%22%3A%20%22InvokeAgreements%22...%7D%5D%7D', got '{"Version": "2012-10-17", "Statement": [{"Sid": "InvokeAgreements", ...}]}'; PolicyName expected 'INVOKE-EXECUTE-API', got 'invoke-execute-api'.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay iam-bap-identity-side-grants-via-group-and-managed-policy step task#10 and compare GetGroupPolicyResult.PolicyDocument / PolicyName with the recording.
3. Replay iam-execute-api-role-lookup-mistakes step task#6 and compare GetRolePolicyResult.PolicyDocument / PolicyName.
