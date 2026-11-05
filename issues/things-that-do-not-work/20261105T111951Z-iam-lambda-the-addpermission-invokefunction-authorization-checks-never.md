Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke iam__lambda.xsvc-iam-2-lambda-invoke-without-permission-denied
Issue-Key: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke|call#13|Timeout|assert body == {"echo": {"marker": marker}}, f"payload not echoed: {body!r}"
Issue-Key: iam__lambda.xsvc-iam-2-lambda-invoke-without-permission-denied|call#13|Timeout|resp = as_caller.invoke(FunctionName=fn, Payload=json.dumps({"marker": marker}).encode())
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# iam__lambda: the AddPermission/InvokeFunction authorization checks never complete

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Cause 1 of 1 — the two checks that exercise lambda:InvokeFunction authorization through a function resource policy never reach a verdict.
AWS: AddPermission grants lambda:InvokeFunction to a named principal and takes effect immediately, so the granted principal's Invoke succeeds while a principal no statement names is refused with AccessDeniedException (403) carrying the "User: <principal ARN> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN>" message; both outcomes are observable within a second or two.
Emulator: recorded behavior only — neither leg settles and the harness reports "Check timed out after 60.0s", so no refusal or grant is ever observed. The open issue 20261105T085348Z-iam-lambda-2-net-new-root-causes-invokefunction.md records that InvokeFunction is never refused, for a different check; these two carry their own replay keys and are additionally not reaching any verdict at all.
Why: a client cannot tell whether a resource-policy grant has taken effect, and an unauthorised caller is neither admitted nor refused in bounded time.
Fix: the AddPermission/RemovePermission resource-policy state and the authorization gate in the Invoke handler, which must answer in bounded time.
Traces: both check ids in trace_ids.

## Evidence
Error: Check timed out after 60.0s for iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke and .xsvc-iam-2-lambda-invoke-without-permission-denied. Expected: the granted principal's Invoke returns 200 and the ungranted principal's Invoke is refused with AccessDeniedException (HTTP 403).

## Steps to Reproduce
CreateFunction; AddPermission for a specific principal with Action=lambda:InvokeFunction; Invoke as that principal (expect 200) and as a principal no statement names (expect 403 AccessDeniedException).
