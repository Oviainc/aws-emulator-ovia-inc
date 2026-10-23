Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-instance-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-instance-error-paths
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetInstance|4xx->4xx|$.code
Issue-Key: lightsail|GetInstance|4xx->4xx|$.message
Issue-Key: lightsail|GetInstance|4xx->4xx|status
Issue-Key: lightsail|DeleteInstance|4xx->4xx|$.code
Issue-Key: lightsail|DeleteInstance|4xx->4xx|$.message
Issue-Key: lightsail|DeleteInstance|4xx->4xx|status
Issue-Key: lightsail|CreateInstances|4xx->2xx|$.__type
Issue-Key: lightsail|CreateInstances|4xx->2xx|$.code
Issue-Key: lightsail|CreateInstances|4xx->2xx|$.message
Issue-Key: lightsail|CreateInstances|4xx->2xx|status
Issue-Commit: 196bb4609762a23c3b17bc928bd901fcc92dc085

# Lightsail instance error paths return wrong status and code

> _Note: this issue was identified against the code base at commit `196bb4609762a23c3b17bc928bd901fcc92dc085`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstance

## Description
The emulator's Lightsail instance operations do not produce Lightsail's error envelope: not-found rejections come back as a generic HTTP 404 with no error code, and an unknown blueprintId is not rejected at all. There are 3 causes in this issue and all 3 must be closed.

Cause 1 of 3: GetInstance on a missing instance (fixture lightsail-instance-error-paths, step task#1).
AWS: Smithy: GetInstance.instanceName is @required ResourceName (pattern ^\w[\w\-]*\w$); modeled errors include NotFoundException ('Lightsail throws this exception when it cannot find a resource'), whose members are code, message, docs and tip. The model declares httpError 404 on NotFoundException, but the recording shows AWS answering HTTP 400 with body field code='DoesNotExist' - the recorded wire status governs. The message is built from the request (resource type plus the requested name), e.g. 'The Instance does not exist: sim2real-probe-lightsail-instance-missing-f0ca242d'; that construction rule is the requirement, the exact recorded text is an example.
Emulator: answers HTTP 404 with no code member at all and a message in its own idiom ('no instance named <name> exists').
Why: Lightsail SDK clients map the error by the body's code/__type, not by the HTTP status, so a client that catches NotFoundException/DoesNotExist (the normal 'create if absent' probe) sees an unmodelled error instead and either aborts or retries a non-retryable condition; the 404 also makes generic HTTP middleware treat the call as a bad endpoint rather than a missing resource.
Fix: the not-found path of the GetInstance handler and the service's error serializer (emit the Lightsail error envelope with code and the resource-type/name message at the recorded status), in the lightsail service crate.
Traces: lightsail-instance-error-paths#task#1 (expected 400, got 404; $.code missing).

Cause 2 of 3: DeleteInstance on a missing instance (fixture lightsail-instance-error-paths, step task#2).
AWS: Smithy: DeleteInstance.instanceName is @required ResourceName; modeled errors include the same NotFoundException. Recorded: HTTP 400 with code='DoesNotExist' and the same request-derived message 'The Instance does not exist: <instanceName>'.
Emulator: answers HTTP 404 with no code member and the same ad-hoc message as cause 1 - the identical envelope defect on the delete path.
Why: idempotent teardown logic keys off DoesNotExist to treat a delete of an already-gone instance as success; without the code it cannot distinguish 'already deleted' from a transport failure and leaves the caller unable to converge.
Fix: the not-found path of the DeleteInstance handler plus the same shared error serializer as cause 1, in the lightsail service crate.
Traces: lightsail-instance-error-paths#task#2 (expected 400, got 404; $.code missing).

Cause 3 of 3: CreateInstances with an unknown blueprintId (fixture lightsail-instance-error-paths, step task#8).
AWS: Smithy: CreateInstances requires instanceNames, availabilityZone, blueprintId and bundleId (blueprintId is NonEmptyString, pattern \S); modeled errors include NotFoundException and InvalidInputException. Recorded: for a blueprintId that is not in the blueprint catalogue AWS rejects the request with HTTP 400 carrying __type='NotFoundException', code='DoesNotExist' and a message naming the rejected blueprint, e.g. 'The specified blueprint does not exist or is deprecated: nonexistent-blueprint-999' - message built from the request, so that rule is the requirement and the text is an example.
Emulator: accepts the request and answers HTTP 200 with no error members at all, so a nonexistent blueprint produces an instance.
Why: this is a missing validation rather than a formatting difference - the emulator will happily create instances from blueprint ids that do not exist, so test suites that assert their blueprint handling rejects typos/deprecated blueprints pass against the emulator and fail against AWS, and downstream state is populated with an unreal blueprint.
Fix: CreateInstances input validation (resolve blueprintId against the blueprint catalogue before creating state and raise NotFoundException/DoesNotExist), in the lightsail service crate.
Traces: lightsail-instance-error-paths#task#8 (expected 400, got 200; $.__type, $.code and $.message all absent).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 34 steps, 0 passed).

Cause 1 - lightsail-instance-error-paths#task#1, GetInstance:
  status: expected 400, got 404
  $.code: missing (expected 'DoesNotExist', got None)
  $.message: expected 'The Instance does not exist: sim2real-probe-lightsail-instance-missing-f0ca242d', got "no instance named 'sim2real-probe-lightsail-instance-missing-f0ca242d' exists"
  divergence_key: service=lightsail|GetInstance|status=400->404|keys=$.code,$.message,status

Cause 2 - lightsail-instance-error-paths#task#2, DeleteInstance:
  status: expected 400, got 404
  $.code: missing (expected 'DoesNotExist', got None)
  $.message: expected 'The Instance does not exist: sim2real-probe-lightsail-instance-missing-f0ca242d', got "no instance named 'sim2real-probe-lightsail-instance-missing-f0ca242d' exists"
  divergence_key: service=lightsail|DeleteInstance|status=400->404|keys=$.code,$.message,status

Cause 3 - lightsail-instance-error-paths#task#8, CreateInstances (blueprintId 'nonexistent-blueprint-999'):
  status: expected 400, got 200
  $.__type: missing (expected 'NotFoundException', got None)
  $.code: missing (expected 'DoesNotExist', got None)
  $.message: missing (expected 'The specified blueprint does not exist or is deprecated: nonexistent-blueprint-999', got None)
  divergence_key: service=lightsail|CreateInstances|status=400->200|keys=$.__type,$.code,$.message,status

Smithy consulted: lightsail-2016-11-28 GetInstance / DeleteInstance / CreateInstances (input requiredness and the modelled error list incl. NotFoundException and InvalidInputException) and shape NotFoundException (errorKind client, httpError 404, members code/docs/message/tip). The recordings show the wire status for this error is 400 with body code 'DoesNotExist'.

State-reset failures in this replay: 0, so these steps ran against clean per-fixture state.

Related open issues (do not cover this): the two open lightsail issues concern CreateKeyPair/DeleteKeyPair answering 501 and carry no replay key for any instance operation.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Cause 1: fixture lightsail-instance-error-paths, step task#1 - POST /ls/api/2016-11-28/GetInstance (X-Amz-Target Lightsail_20161128.GetInstance) with {"instanceName": "sim2real-probe-lightsail-instance-missing-f0ca242d"} for an instance that does not exist. Expect HTTP 400 with body code='DoesNotExist' and a message naming the Instance and the requested name; observe HTTP 404 with no code.
4. Cause 2: fixture lightsail-instance-error-paths, step task#2 - same request against DeleteInstance; expect HTTP 400 / code='DoesNotExist'; observe HTTP 404 with no code.
5. Cause 3: fixture lightsail-instance-error-paths, step task#8 - call CreateInstances with a valid instanceName/availabilityZone/bundleId and blueprintId='nonexistent-blueprint-999'. Expect HTTP 400 with __type='NotFoundException', code='DoesNotExist' and a message naming the rejected blueprint; observe HTTP 200 and a created instance.
6. Assert on the HTTP status and the error code field in each case.
