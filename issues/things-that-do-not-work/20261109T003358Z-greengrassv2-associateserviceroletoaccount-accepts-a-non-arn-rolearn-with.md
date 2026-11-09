Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.associate-service-role-untrusted-role-is-accepted
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.associate-service-role-untrusted-role-is-accepted|_error_code#6|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 2e69f962e88ed4d502b4322332b0a570b57350b4

# greengrassv2 AssociateServiceRoleToAccount accepts a non-ARN roleArn with HTTP 200 instead of refusing 400 "That role ARN is invalid."

> _Note: this issue was identified against the code base at commit `2e69f962e88ed4d502b4322332b0a570b57350b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateServiceRoleToAccount

## Description
1 cause to close.

Cause 1 of 1: AssociateServiceRoleToAccount performs no validation of `roleArn`, so a value that is not an ARN at all is accepted and persisted as the account's Greengrass service-role association.

AWS:      Smithy: AssociateServiceRoleToAccount PUT /greengrass/servicerole, input member `roleArn` (String, required, jsonName RoleArn), modeled errors ValidationException (HTTP 400) and InternalServerException (HTTP 500); the model places NO @pattern/@length constraint on `roleArn`, so the ARN check is a server-side rule the model does not express. Recorded behavior: a roleArn that is not an IAM role ARN is refused with HTTP 400 and the message `That role ARN is invalid.` — entirely FIXED text, with no variable part: AWS does not echo the offending value, the account id or the member name. The response body carries only that message and no error code, so botocore surfaces `Error.Code` as the HTTP status string `"400"` rather than a modeled exception name. The association is left unchanged by the refusal. Note that AWS validates only the ARN's SHAPE here: a syntactically valid role ARN whose trust policy does not name Greengrass IS accepted (trust-policy compatibility is not checked synchronously), so the fix must not over-reject.
Emulator: the request succeeds. The handler reads state, unconditionally replaces `service_role` with the supplied string and returns HTTP 200 with `associatedAt` — the replay observed `{'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'associatedAt': '<timestamp>'}` for a roleArn of the form `not-an-arn-<suffix>`. The bogus value is then persisted, so GetServiceRoleForAccount subsequently reports a non-ARN as the account's service role.
Why:      a client that relies on AWS rejecting a malformed role ARN gets a 200 and believes the association succeeded; the account-level singleton is left holding a value that can never be assumed, and the failure surfaces far later (at device connectivity/client-device verification time) instead of at the call that caused it. Code branching on the 400 / `That role ARN is invalid.` refusal never fires, and a client that reads the association back to confirm it sees its own garbage echoed as valid configuration.
Fix:      the request validation of AssociateServiceRoleToAccount's handler — reject a `roleArn` that is not a well-formed IAM role ARN before mutating state, answering HTTP 400 with the fixed message above and an error body carrying no error code, and leaving the existing association untouched. A syntactically well-formed role ARN must still be accepted regardless of its trust policy.
Traces:   greengrassv2.associate-service-role-untrusted-role-is-accepted (the handler is services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/associate_service_role_to_account.rs; its `handle` at lines 138-183 goes straight from the state lookup to `state.lock().service_role.replace(...)` with no check on `input.role_arn`).

Related but NOT covering this: the open issue 20261103T065241Z-greengrassv2-refusal-responses-diverge-from-aws-10-net.md enumerates diverging refusal wording for component/deployment/tag paths, and 20261103T065241Z-greengrassv2-getserviceroleforaccount-stub.md covers the GetServiceRoleForAccount stub. Neither names AssociateServiceRoleToAccount or a MISSING refusal on this path.

## Evidence
Check greengrassv2.associate-service-role-untrusted-role-is-accepted, step `gg.associate_service_role_to_account(roleArn=<non-ARN string>)`:

expected (AWS): ClientError, HTTP 400, Error.Code == "400" (no error code in the body, so botocore falls back to the status), Error.Message == "That role ARN is invalid." (fixed text, no variable parts), and GetServiceRoleForAccount still reporting the pre-call roleArn.

actual (emulator): no exception at all —
  AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '39', ...}, 'RetryAttempts': 0}, 'associatedAt': '<timestamp>'}
raised from the check's `_error_code` helper (check_greengrassv2_associate_service_role_untrusted_role_is_accepted.py line 102, called at line 37).

The non-ARN value is also persisted: the handler replaces the account's single `service_role` entry before returning, so a subsequent GetServiceRoleForAccount reports it. (The timestamp and the random name suffix in the recorded run are per-run values, not requirements.)

The check's second half — associating a syntactically valid role ARN whose trust policy names ec2.amazonaws.com rather than Greengrass — must keep SUCCEEDING; it was never reached because the first assertion failed.

## Steps to Reproduce
Point a greengrassv2 client at the emulator.
1. Optionally record the current association: `gg.get_service_role_for_account()` (a 404-ish ClientError means none is associated).
2. Call `gg.associate_service_role_to_account(roleArn="not-an-arn-" + <any suffix>)`.
   Expect: ClientError with HTTP status 400, Error.Code == "400", Error.Message == "That role ARN is invalid.".
   Observe: HTTP 200 and a response containing `associatedAt`.
3. Call `gg.get_service_role_for_account()` and observe the malformed string returned as `roleArn`; AWS would still report the value from step 1.
4. Regression guard: create an IAM role whose trust policy allows only `ec2.amazonaws.com` to assume it and call `gg.associate_service_role_to_account(roleArn=<that role's ARN>)` — this must continue to succeed, and GetServiceRoleForAccount must report that ARN.
