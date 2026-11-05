Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke
Issue-Key: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke|call#13|Timeout|resp = client.invoke(FunctionName=fn, Payload=json.dumps({"marker": marker}).encode())
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# iam__lambda: the AddPermission-alone-authorizes-Invoke scenario never settles, so a function's resource policy is never shown to authorize an otherwise-unauthorized caller

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Cause 1 of 1: the scenario that proves a Lambda resource-policy grant alone authorizes Invoke for a principal with no identity-based permission never reaches a verdict — it is terminated at its budget, so neither the grant's effect nor its scoping is observable.

AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice): Invoke is POST /2015-03-31/functions/{FunctionName}/invocations with success code 200 and X-Amz-Function-Error / X-Amz-Executed-Version response headers; its modelled client errors include ResourceNotFoundException (404) and TooManyRequestsException (429), and AWS classifies an authorization failure as AccessDeniedException (HTTP 403). AWS's documented behavior: Lambda authorization is the union of the caller's identity-based policy and the function's resource policy, so a principal that holds NO identity-based lambda:InvokeFunction grant is nevertheless authorized once lambda:AddPermission has added a statement whose Principal is that principal and whose Action is lambda:InvokeFunction on that function — the Invoke then succeeds (200 with the handler's payload). With the statement removed, or when the statement names a different function or qualifier, the same call is refused 403 AccessDeniedException and the handler does not run.

Emulator: the check is killed by the harness with "Error: Check timed out after 60.0s". No Invoke result — no status code, no error code, no payload — is produced for either the granted or the ungranted leg, so the resource-policy contribution to the authorization decision is never expressed.

Why: a client relying on AWS's behavior cannot verify cross-principal invoke grants at all on this emulator. The two outcomes an operator must be able to tell apart — "authorized because the resource policy allows it" (200) and "denied because nothing allows it" (403 AccessDeniedException) — are indistinguishable from a request that simply never completes, which is the worst case for a permission-boundary test: a scenario written to catch an over-permissive emulator cannot fail it.

Fix: the Invoke authorization path in the lambda service — the authorization decision must consult the function's resource policy (as written by AddPermission/RemovePermission) alongside the caller's identity-based policy and answer promptly, 200 for an allowed call and 403 AccessDeniedException for an unauthorized one.

Traces: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke. The open issues 20261105T111951Z-iam-lambda-the-addpermission-invokefunction-authorization-checks-never.md and 20261105T085348Z-iam-lambda-2-net-new-root-causes-invokefunction.md carry the sibling checks xsvc-iam-2-lambda-invoke-without-permission-denied and xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied, but neither carries this check's replay key.

## Evidence
Expected (AWS): with a principal holding no identity-based lambda:InvokeFunction grant, Invoke -> 403 AccessDeniedException; after AddPermission(Action='lambda:InvokeFunction', Principal=<that principal>, StatementId=<id>), the identical Invoke -> HTTP 200 with the handler's payload and no X-Amz-Function-Error.

Actual (emulator): 'Error: Check timed out after 60.0s' — the check never obtains an Invoke response on either leg, so no status, error code or payload exists to compare.

## Steps to Reproduce
1. Create a Lambda function with a trivial handler that returns a recognisable payload.
2. Create a second principal (role/user) whose policies grant no lambda:InvokeFunction on that function, and obtain credentials for it.
3. Invoke the function as that principal. Expect 403 AccessDeniedException.
4. Call AddPermission on the function with Action='lambda:InvokeFunction', Principal=<that principal's ARN> and a fresh StatementId.
5. Invoke again as the same principal with no other change. Expect HTTP 200 and the handler's payload; observe that the scenario never completes instead.
