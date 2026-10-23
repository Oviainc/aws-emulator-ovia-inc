Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-cloudtrail-arn-format-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-cloudtrail-arn-format-errors cloudtrail-invalid-trail-name-format-errors cloudtrail-trail-error-paths cloudtrail-trail-lifecycle-error-paths cloudtrail-trail-lifecycle-missing-trail cloudtrail-trail-operations-notfound-and-validation-errors cloudtrail-trail-creation-validation-errors
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|GetTrail|4xx->5xx|$.Message
Issue-Key: cloudtrail|GetTrail|4xx->5xx|$.__type
Issue-Key: cloudtrail|GetTrail|4xx->5xx|status
Issue-Key: cloudtrail|StopLogging|4xx->5xx|$.Message
Issue-Key: cloudtrail|StopLogging|4xx->5xx|$.__type
Issue-Key: cloudtrail|StopLogging|4xx->5xx|status
Issue-Key: cloudtrail|DeleteTrail|4xx->5xx|$.Message
Issue-Key: cloudtrail|DeleteTrail|4xx->5xx|$.__type
Issue-Key: cloudtrail|DeleteTrail|4xx->5xx|status
Issue-Key: cloudtrail|StartLogging|4xx->5xx|$.Message
Issue-Key: cloudtrail|StartLogging|4xx->5xx|$.__type
Issue-Key: cloudtrail|StartLogging|4xx->5xx|status
Issue-Key: cloudtrail|CreateTrail|4xx->5xx|$.Message
Issue-Key: cloudtrail|CreateTrail|4xx->5xx|$.__type
Issue-Key: cloudtrail|CreateTrail|4xx->5xx|status
Issue-Key: cloudtrail|GetTrailStatus|4xx->5xx|$.Message
Issue-Key: cloudtrail|GetTrailStatus|4xx->5xx|$.__type
Issue-Key: cloudtrail|GetTrailStatus|4xx->5xx|status
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# CloudTrail trail validation and not-found rejections answer 501 NotImplemented instead of their modeled 400 error codes

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrail

## Description
SIX causes (Cause 1 of 6 .. Cause 6 of 6) sharing one fault: every negative path of the CloudTrail trail surface — malformed trail ARN, malformed trail name, unknown trail — is refused with HTTP 501 and the unmodeled error code `NotImplemented` instead of the modeled 4xx error the recording shows. None of these operations is a scaffold stub (the replay report reports no stub_path and no owning issue for any of the six clusters). Related open issue: 'Every CloudTrail operation answers HTTP 501 NotImplemented from the dispatcher, including the ones whose handlers are fully implemented' (20261022T220446Z-cloudtrail-every-cloudtrail-operation-answers-http-501-notimplemented-from.md) names the same suspected dispatch root cause from the BEHAVIORAL corpus, but it carries no wire replay key, so these six fixture steps are not gated by it and must be re-replayed from this issue. Note that the error-code and status requirements below are independent of the dispatch fix: once an operation is reachable, its validation must still classify these inputs.

Cause 1 of 6 — GetTrail with a syntactically invalid CloudTrail ARN (fixture cloudtrail-cloudtrail-arn-format-errors, step task#1).
AWS: Smithy: GetTrail models CloudTrailARNInvalidException (client error). Recorded response: HTTP 400 with __type `CloudTrailARNInvalidException`; the message is built from the request and names the offending ARN (example: 'The ARN arn:aws:cloudtrail:us-west-2:000000000000:badtrail is invalid.'), so the rule — reject an ARN whose resource part is not trail/<name> and name it — is the requirement, not the literal text.
Emulator: HTTP 501, __type `NotImplemented`, no Message member.
Why: a client branching on CloudTrailARNInvalidException sees a code absent from its model and a 5xx, so an SDK treats a caller input error as a retryable server fault.
Fix: GetTrail's input validation (ARN-form check) in the cloudtrail crate, plus the dispatch/binding layer that currently prevents the handler from running.
Traces: cloudtrail-cloudtrail-arn-format-errors#task#1 (+5 more fixtures in the cluster).

Cause 2 of 6 — StopLogging with a syntactically invalid CloudTrail ARN (fixture cloudtrail-cloudtrail-arn-format-errors, step task#3).
AWS: Smithy: StopLogging models CloudTrailARNInvalidException. Recorded response: HTTP 400, __type `CloudTrailARNInvalidException`, message naming the ARN (example: 'The ARN arn:aws:cloudtrail:us-west-2:000000000000:trailwrong is invalid.').
Emulator: HTTP 501 NotImplemented.
Why: logging-state changes cannot be validated; a bad identifier is indistinguishable from an outage.
Fix: StopLogging input validation (ARN form) + dispatch/binding.
Traces: cloudtrail-cloudtrail-arn-format-errors#task#3 (+4 more fixtures).

Cause 3 of 6 — DeleteTrail with a name formatted as an IP address (fixture cloudtrail-invalid-trail-name-format-errors, step task#3).
AWS: Smithy: DeleteTrail models InvalidTrailNameException (httpError 400). Recorded response: HTTP 400, __type `InvalidTrailNameException`, message stating the violated naming rule (example: 'Trail names must not be formatted as an IP address.'). The documented name rules (ASCII letters/digits/period/underscore/dash, start and end with a letter or number, 3-128 chars, no adjacent periods/underscores/dashes, not an IP address) are the requirement.
Emulator: HTTP 501 NotImplemented.
Why: a client cannot tell an invalid name from a broken service, and a validation loop that expects 400 retries a 5xx instead.
Fix: shared trail-name validation applied in DeleteTrail + dispatch/binding.
Traces: cloudtrail-invalid-trail-name-format-errors#task#3 (+4 more fixtures).

Cause 4 of 6 — StartLogging with a trail name that violates the naming rules (fixture cloudtrail-invalid-trail-name-format-errors, step task#4).
AWS: Smithy: StartLogging models InvalidTrailNameException (httpError 400). Recorded response: HTTP 400, __type `InvalidTrailNameException`, message naming the violated rule (example: 'Trail name must end with a letter or number.').
Emulator: HTTP 501 NotImplemented.
Why: same as above on the start-logging path, which is the first call most audit setups make after create.
Fix: trail-name validation in StartLogging + dispatch/binding.
Traces: cloudtrail-invalid-trail-name-format-errors#task#4 (+4 more fixtures).

Cause 5 of 6 — CreateTrail with a trail name that violates the naming rules (fixture cloudtrail-trail-creation-validation-errors, step task#1).
AWS: Smithy: CreateTrail models InvalidTrailNameException (httpError 400) among its client errors. Recorded response: HTTP 400, __type `InvalidTrailNameException`, message naming the violated rule (example: 'Trail name must end with a letter or number.').
Emulator: HTTP 501 NotImplemented.
Why: the create path accepts nothing and rejects nothing correctly, so a client cannot validate user-supplied trail names at all.
Fix: CreateTrail input validation + dispatch/binding.
Traces: cloudtrail-trail-creation-validation-errors#task#1 (+4 more fixtures).

Cause 6 of 6 — GetTrailStatus on a trail that does not exist (fixture cloudtrail-trail-creation-validation-errors, step task#3).
AWS: Smithy: GetTrailStatus models TrailNotFoundException. Recorded response: HTTP 400 with __type `TrailNotFoundException` and a message built from the request naming the trail ARN and the account (example: 'Unknown trail: arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-probe-cloudtrail-absent-trail for the user: 000000000000'). Note the model's @httpError on TrailNotFoundException is 404 while the recording shows 400 for this awsJson1_1 operation; the recorded 400 plus the error code is the wire requirement.
Emulator: HTTP 501, __type `NotImplemented`.
Why: existence checks on a trail cannot be performed; polling code that treats TrailNotFoundException as 'not yet created' instead sees a server error and aborts.
Fix: GetTrailStatus handler's trail lookup against crate::state and its not-found mapping + dispatch/binding.
Traces: cloudtrail-trail-creation-validation-errors#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0). Every cluster below reports stub_path = "" and owned_by = [].
- GetTrail — expected 400 / __type CloudTrailARNInvalidException, got 501 / __type NotImplemented; $.Message missing (recorded example 'The ARN arn:aws:cloudtrail:us-west-2:000000000000:badtrail is invalid.'). Cluster cloudtrail-cloudtrail-arn-format-errors#task#1, 6 steps affected.
- StopLogging — expected 400 / CloudTrailARNInvalidException, got 501 / NotImplemented; $.Message missing (example 'The ARN ...:trailwrong is invalid.'). Cluster cloudtrail-cloudtrail-arn-format-errors#task#3, 5 steps.
- DeleteTrail — expected 400 / InvalidTrailNameException, got 501 / NotImplemented; $.Message missing (example 'Trail names must not be formatted as an IP address.'). Cluster cloudtrail-invalid-trail-name-format-errors#task#3, 5 steps.
- StartLogging — expected 400 / InvalidTrailNameException, got 501 / NotImplemented; $.Message missing (example 'Trail name must end with a letter or number.'). Cluster cloudtrail-invalid-trail-name-format-errors#task#4, 5 steps.
- CreateTrail — expected 400 / InvalidTrailNameException, got 501 / NotImplemented; $.Message missing (example 'Trail name must end with a letter or number.'). Cluster cloudtrail-trail-creation-validation-errors#task#1, 5 steps.
- GetTrailStatus — expected 400 / TrailNotFoundException, got 501 / NotImplemented; $.Message missing (example 'Unknown trail: arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-probe-cloudtrail-absent-trail for the user: 000000000000'). Cluster cloudtrail-trail-creation-validation-errors#task#3.
Account ids, region and generated trail names in the recorded messages are environment-specific examples; the diverging requirements are the HTTP status and the error code, plus the message rule (name the offending ARN / the violated naming rule / the unknown trail).

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS SDK CloudTrail client at it (region us-west-2, awsJson1_1).
2. GetTrail with Name = 'arn:aws:cloudtrail:us-west-2:<account>:badtrail' (an ARN whose resource part is not trail/<name>): expect HTTP 400 with error code CloudTrailARNInvalidException naming that ARN. Observe HTTP 501 with code NotImplemented and no Message.
3. StopLogging with Name = 'arn:aws:cloudtrail:us-west-2:<account>:trailwrong': expect HTTP 400 CloudTrailARNInvalidException. Observe HTTP 501 NotImplemented.
4. DeleteTrail with Name = '10.0.0.1': expect HTTP 400 InvalidTrailNameException stating that trail names must not be formatted as an IP address. Observe HTTP 501 NotImplemented.
5. StartLogging with a name ending in a dash (e.g. 'probe-trail-'): expect HTTP 400 InvalidTrailNameException stating the name must end with a letter or number. Observe HTTP 501 NotImplemented.
6. CreateTrail with the same invalid name and any S3BucketName: expect HTTP 400 InvalidTrailNameException. Observe HTTP 501 NotImplemented.
7. GetTrailStatus with a well-formed name that was never created: expect HTTP 400 TrailNotFoundException naming the unknown trail. Observe HTTP 501 NotImplemented.
8. Replay the fixture steps in Issue-Trace/Issue-Step and Issue-Trace-Ids to confirm all six causes.
