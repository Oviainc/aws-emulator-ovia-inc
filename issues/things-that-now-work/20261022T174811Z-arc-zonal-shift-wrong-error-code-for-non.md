Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-zonal-shift.get-managed-resource-malformed-identifier-rejected arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected arc-zonal-shift.list-zonal-shifts-invalid-status-filter-rejected
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift.get-managed-resource-malformed-identifier-rejected|run#3|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got CODE={code!r}"
Issue-Key: arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected|run#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: arc-zonal-shift.list-zonal-shifts-invalid-status-filter-rejected|run#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# arc-zonal-shift: wrong error code for non-ARN resourceIdentifier, and ValidationException message construction diverges (3 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetManagedResource

## Description
Three distinct net-new divergences in the arc-zonal-shift (ARC zonal shift) emulator, all found by replaying the behavioral suite against a serving emulator (the service answers; it answers wrongly). Model consulted: services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json.

Cause 1 of 3 — GetManagedResource answers 404 ResourceNotFoundException for an identifier that is not ARN-shaped, where AWS answers 403 AccessDeniedException. Failing check: arc-zonal-shift.get-managed-resource-malformed-identifier-rejected.
AWS: Smithy models GetManagedResource with input member resourceIdentifier (@httpLabel, required, ResourceIdentifier @length min 8 max 1024, NO pattern) and errors AccessDeniedException (HTTP 403), ResourceNotFoundException (HTTP 404), ValidationException (HTTP 400). The model does not state which boundary wins, so this is recorded behavior: the replay shows real AWS rejecting a resourceIdentifier that is not ARN-shaped at all with AccessDeniedException / HTTP 403 and an EMPTY message (AccessDeniedException.message is optional in the model), because authorization matches resourceIdentifier against the caller's granted resource-ARN patterns before the service performs its own lookup/validation. A syntactically valid, same-account ARN that is simply not registered is the distinct ResourceNotFoundException / 404 boundary.
Emulator: every identifier that is absent from the managed-resource store — ARN-shaped or not — is answered with ResourceNotFoundException / HTTP 404 (observed code 'ResourceNotFoundException' for resourceIdentifier="this-is-not-an-arn-at-all").
Why: a client that distinguishes "you are not allowed to touch this resource" (403, retry never, surface a permissions problem) from "this resource is not registered with ARC" (404, register it / fall through to StartZonalShift) takes the wrong branch. Tests and guard code that assert AccessDeniedException for an unauthorized/garbage identifier pass against AWS and fail against the emulator, and the emulator is the more permissive of the two: it reports a lookup miss where AWS reports a denial.
Fix: the GetManagedResource handler's identifier-shape/authorization branch — it must classify a non-ARN-shaped resourceIdentifier as AccessDeniedException (403, empty message) before the store lookup that currently produces ResourceNotFoundException, and keep 404 for well-formed ARNs that are not registered.
Traces: arc-zonal-shift.get-managed-resource-malformed-identifier-rejected; the 404 branch is at services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/get_managed_resource.rs lines 181-191.

Cause 2 of 3 — the ValidationException message for a @pattern violation echoes the submitted value and reports the pattern with its anchors. Failing check: arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected.
AWS: Smithy models CancelZonalShift.zonalShiftId as ZonalShiftId @length min 6 max 36, @pattern "^[A-Za-z0-9-]+$", and the operation models ValidationException (HTTP 400). Status 400 and code ValidationException MATCH in the emulator — the divergence is the message, and its construction rule is known because the message is built deterministically from the request: AWS names the offending member and the constraint but does NOT echo the submitted value, and renders the regular expression WITHOUT the surrounding ^ and $ anchors. Recorded example: "1 validation error detected: Value at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+".
Emulator: it echoes the rejected value and the anchored pattern. Recorded example: "1 validation error detected: Value 'bad_id_1' at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-]+$".
Why: the echoed value leaks the caller's input back into an error string where AWS deliberately does not, and the anchored pattern is not the pattern a client can show a user or match on; clients and test suites that assert on the documented validation-message shape diverge. This is a message-construction rule, not a one-off string: the requirement is "no value echo, pattern reported unanchored", not the recorded text.
Fix: the shared constraint-validation error formatter in the smithy server runtime's centralized deserialize -> validate walk (the stage described in the ops/*.rs module docs), not this operation's handler — the same formatter produces Cause 3.
Traces: arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected.

Cause 3 of 3 — the ValidationException message for an enum violation echoes the submitted value and renders the enum value set in a different order. Failing check: arc-zonal-shift.list-zonal-shifts-invalid-status-filter-rejected.
AWS: Smithy models ListZonalShifts.status (@httpQuery "status") as enum ZonalShiftStatus with values ACTIVE, EXPIRED, CANCELED, and the operation models ValidationException (HTTP 400). Status 400 and code ValidationException MATCH — again only the message diverges. Known part of the rule (same as Cause 2, and corroborated by it): the message does not echo the submitted value. Recorded-behavior-only part: AWS renders the admissible set as [EXPIRED, ACTIVE, CANCELED], which is NOT the model's declaration order. Recorded example: "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [EXPIRED, ACTIVE, CANCELED]". Hypothesis: AWS emits the service-internal enum ordering rather than the modeled order.
Emulator: "1 validation error detected: Value 'PENDING' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, EXPIRED, CANCELED]" — value echoed, set in model declaration order.
Why: a client that enumerates the admissible values out of the error message, or a conformance test pinned to the AWS message, diverges; and the value echo repeats the input-leak of Cause 2.
Fix: the same shared constraint-validation error formatter (enum-constraint branch): drop the value echo, and emit the enum value set in the order AWS reports for this shape.
Traces: arc-zonal-shift.list-zonal-shifts-invalid-status-filter-rejected.

Not filed here (already covered, per the replay report): the checks blocked by UNIMPLEMENTED GetAutoshiftObserverNotificationStatus and by UNIMPLEMENTED ec2 CreateVpc (ALREADY FILED), and the three checks OWNED by 20261022T120642Z-sagemaker-behavioral-checks-blocked-by-neighbouring-service-gaps.md (ec2 DescribeVpcs rejecting the 'isDefault' filter).

## Evidence
Behavioral suite: 1/22 passed for arc-zonal-shift; 3 mismatches survive after skipping ALREADY FILED and OWNED clusters.

Cause 1 — arc-zonal-shift.get-managed-resource-malformed-identifier-rejected [mismatch]
  call: GetManagedResource(resourceIdentifier="this-is-not-an-arn-at-all")
  expected (AWS): HTTP 403, Error.Code="AccessDeniedException", Error.Message="" (empty)
  actual (emulator): Error.Code='ResourceNotFoundException' (HTTP 404)
  assertion: AssertionError: expected AccessDeniedException, got CODE='ResourceNotFoundException'

Cause 2 — arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected [mismatch]
  call: CancelZonalShift(zonalShiftId="bad_id_1")  (length 8, inside 6..36; violates the pattern)
  code: ValidationException / HTTP 400 in BOTH — matches.
  expected message (AWS, example): "1 validation error detected: Value at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+"
  actual message (emulator, example): "1 validation error detected: Value 'bad_id_1' at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-]+$"
  diverging rule: value echoed (AWS omits it); pattern anchored (AWS reports it unanchored).

Cause 3 — arc-zonal-shift.list-zonal-shifts-invalid-status-filter-rejected [mismatch]
  call: ListZonalShifts(status="PENDING")
  code: ValidationException / HTTP 400 in BOTH — matches.
  expected message (AWS, example): "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [EXPIRED, ACTIVE, CANCELED]"
  actual message (emulator, example): "1 validation error detected: Value 'PENDING' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, EXPIRED, CANCELED]"
  diverging rule: value echoed (AWS omits it); enum set order differs (AWS order recorded only).

Model citations: GetManagedResource input resourceIdentifier=ResourceIdentifier @length 8..1024 (no pattern), errors {AccessDeniedException 403, ResourceNotFoundException 404, ValidationException 400, ThrottlingException 429, InternalServerException 500}; CancelZonalShift input zonalShiftId=ZonalShiftId @length 6..36 @pattern ^[A-Za-z0-9-]+$; ListZonalShifts input status=ZonalShiftStatus enum values [ACTIVE, EXPIRED, CANCELED]; AccessDeniedException.message optional, ResourceNotFoundException.message/ValidationException.message required.

## Steps to Reproduce
Boot the emulator and point an arc-zonal-shift client at it (botocore client 'arc-zonal-shift'), then:

Cause 1:
  1. arc.get_managed_resource(resourceIdentifier="this-is-not-an-arn-at-all")
  2. Observe Error.Code='ResourceNotFoundException' with HTTP 404; AWS answers HTTP 403 AccessDeniedException with an empty message.
  3. Positive control (passes): arc.list_managed_resources() returns an 'items' key.

Cause 2:
  1. arc.cancel_zonal_shift(zonalShiftId="bad_id_1")  — 8 chars, so botocore's client-side length check lets it through, but it violates the member pattern.
  2. Observe ValidationException (correct) whose message echoes "Value 'bad_id_1'" and reports the pattern as ^[A-Za-z0-9-]+$; AWS omits the value and reports [A-Za-z0-9-]+.

Cause 3:
  1. arc.list_zonal_shifts(status="PENDING")
  2. Observe ValidationException (correct) whose message echoes "Value 'PENDING'" and lists the set as [ACTIVE, EXPIRED, CANCELED]; AWS omits the value and lists [EXPIRED, ACTIVE, CANCELED].
  3. Positive control (passes): arc.list_zonal_shifts(status="ACTIVE") returns an 'items' key.

Re-replay these three checks to confirm a fix: arc-zonal-shift.get-managed-resource-malformed-identifier-rejected, arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected, arc-zonal-shift.list-zonal-shifts-invalid-status-filter-rejected.
