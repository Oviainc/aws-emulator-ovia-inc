Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3outposts.create-endpoint-invalid-outpost-id-format-validation-error s3outposts.create-endpoint-invalid-security-group-id-format-validation-error s3outposts.delete-endpoint-invalid-endpoint-id-format-validation-error s3outposts.list-shared-endpoints-invalid-outpost-id-format-validation-error
Issue-Shape-Id: com.amazonaws.s3outposts#S3Outposts
Issue-Key: s3outposts.create-endpoint-invalid-outpost-id-format-validation-error|run#10|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Key: s3outposts.create-endpoint-invalid-security-group-id-format-validation-error|run#10|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Key: s3outposts.delete-endpoint-invalid-endpoint-id-format-validation-error|run#9|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Key: s3outposts.list-shared-endpoints-invalid-outpost-id-format-validation-error|run#8|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# ResourceNotFoundException for an unresolvable OutpostId carries a descriptive message instead of AWS's "Access Denied"

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint

## Description
Cause 1 of 1: the single `ResourceNotFoundException$Message` this service answers an unresolvable `OutpostId` reference with is the wrong text, so every operation that resolves an Outpost (CreateEndpoint, DeleteEndpoint, ListSharedEndpoints) returns a non-AWS message on the 404 path.

Failing check ids for this cause: s3outposts.create-endpoint-invalid-outpost-id-format-validation-error, s3outposts.create-endpoint-invalid-security-group-id-format-validation-error, s3outposts.delete-endpoint-invalid-endpoint-id-format-validation-error, s3outposts.list-shared-endpoints-invalid-outpost-id-format-validation-error.

AWS: Smithy model `com.amazonaws.s3outposts#ResourceNotFoundException` is bound to CreateEndpoint, DeleteEndpoint and ListSharedEndpoints with `@httpError 404` and documents only "The requested resource was not found." — the model carries no message text, so this is recorded behavior only. The recorded responses show that when S3 on Outposts cannot resolve a well-formed `OutpostId` (one matching `^(op-[a-f0-9]{17}|\d{12}|ec2)$`) or an `EndpointId` (`^[a-zA-Z0-9]{19}$`) reached through such an Outpost, the service answers HTTP 404, error code `ResourceNotFoundException`, and Message exactly `Access Denied`. That message is entirely FIXED TEXT: it has no variable parts, it interpolates no id, no Region and no resource kind from the request or from state, and it is the same string for every one of the three operations and for every branch of `OutpostId`. Hypothesis: the service deliberately does not disclose whether the Outpost is absent, S3-less, or merely not visible to the caller, which is why the 404 reuses authorization-style wording.

Emulator: all three operations return the correct status (404) and the correct code (`ResourceNotFoundException`), but Message is the service constant `The specified outpost does not exist, or does not have S3 capacity provisioned on it.` Replay output for all four checks: `AssertionError: message='The specified outpost does not exist, or does not have S3 capacity provisioned on it.'` where the recorded value is `'Access Denied'`. The emulator's text is a different fixed string, not a differently-filled template, so it diverges for every request, not just the recorded ones.

Why: a client that discriminates S3 on Outposts failure classes by message text — exactly what these checks do when they use the unknown-but-well-formed id as the positive control that isolates a `ValidationException` pattern rejection from a resolution failure — sees a message AWS never emits. Any assertion, log scraper, retry classifier or user-facing error surface keyed on the 404 message string behaves differently against the emulator than against AWS, and the emulator additionally leaks information (that the Outpost is absent or has no S3 capacity) that AWS deliberately withholds behind `Access Denied`.

Fix: belongs in the service's shared state/error-payload helper, not in the three handlers — the one constant feeding the `ResourceNotFoundException` payload returned by the shared Outpost resolution must carry AWS's fixed text `Access Denied`. The handlers already lift that one payload into their own error enums correctly, and the status and code are already right, so no handler or serializer change is needed. Note this is a distinct root cause from the open issues 20261029T031535Z-s3outposts-createendpoint-wrong-behavior--r4.md (subnet→VPC table not Region-scoped), 20261028T215845Z-s3outposts-deleteendpoint-maintainability.md (duplicated 404 assertions in the harness) and 20261028T213739Z-s3outposts-listoutpostswiths3-pagination.md (NextToken cross-Region) — none of those touches the 404 message text.

Traces: the four failing checks below. Emulator source carrying the cause: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs lines 1356-1368 (the `OUTPOST_NOT_FOUND_MESSAGE` constant) and lines 1376-1389 (`not_found()`, which stamps it into every payload); reached from services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs lines 865-874 (`resolve_outpost`) and services/service-com-amazonaws-s3outposts-s3outposts/src/ops/create_endpoint.rs lines 205-208.

## Evidence
All four checks assert the message of the positive-control 404 and fail identically.

Expected (recorded AWS behavior), for each of the three operations:
  HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "Access Denied"

Actual (emulator):
  HTTP 404, Error.Code = "ResourceNotFoundException" (correct), Error.Message = "The specified outpost does not exist, or does not have S3 capacity provisioned on it."

Replay excerpts:
- s3outposts.create-endpoint-invalid-outpost-id-format-validation-error, line 49:
  assert control_message == ACCESS_DENIED -> AssertionError: message='The specified outpost does not exist, or does not have S3 capacity provisioned on it.'
  (control call: create_endpoint(OutpostId="op-"+"0"*17, SubnetId="subnet-00000000", SecurityGroupId="sg-00000000"))
- s3outposts.create-endpoint-invalid-security-group-id-format-validation-error, line 47: same assertion, same actual message.
  (control call: create_endpoint(OutpostId="op-"+"0"*17, SubnetId="subnet-00000000", SecurityGroupId="sg-00000000"))
- s3outposts.delete-endpoint-invalid-endpoint-id-format-validation-error, line 37: same assertion, same actual message.
  (control call: delete_endpoint(EndpointId="A1B2C3D4E5F6G7H8I9J", OutpostId="op-"+"0"*17))
- s3outposts.list-shared-endpoints-invalid-outpost-id-format-validation-error, line 41: same assertion, same actual message.
  (control call: list_shared_endpoints(OutpostId="op-"+"0"*17))

Note the ValidationException half of every one of these four checks PASSES: the 400 code and the full `1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` message are already correct. Only the 404 message diverges, which localizes the defect to the single shared not-found payload.

The diverging message has no per-run or environment-specific component — no id, host, timestamp or request id — so the divergence is the fixed text itself and is reproducible in any environment.

## Steps to Reproduce
1. Boot the emulator and point a botocore/boto3 `s3outposts` client at it.
2. Call any operation that resolves an Outpost with a WELL-FORMED but unknown identifier, e.g.:
   - s3o.create_endpoint(OutpostId="op-" + "0"*17, SubnetId="subnet-" + "0"*8, SecurityGroupId="sg-" + "0"*8)
   - s3o.delete_endpoint(EndpointId="A1B2C3D4E5F6G7H8I9J", OutpostId="op-" + "0"*17)
   - s3o.list_shared_endpoints(OutpostId="op-" + "0"*17)
3. Catch botocore.exceptions.ClientError and read err.response["Error"]["Code"] and ["Message"].
4. Observe Code == "ResourceNotFoundException" (correct) but Message == "The specified outpost does not exist, or does not have S3 capacity provisioned on it." where AWS returns the fixed text "Access Denied".
5. Equivalently, replay the four checks listed in trace_ids: each passes its ValidationException assertions and fails on the positive-control message assertion.
