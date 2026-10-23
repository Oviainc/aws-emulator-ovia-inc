Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gameliftstreams.create-application-invalid-source-uri-rejected gameliftstreams.create-stream-group-client-token-conflict-rejected gameliftstreams.create-stream-group-default-application-not-found gameliftstreams.delete-stream-group-removes-resource gameliftstreams.create-stream-group-invalid-stream-class-rejected gameliftstreams.associate-applications-nonexistent-app-not-found gameliftstreams.create-stream-session-connection-not-found-rejected gameliftstreams.tag-resource-invalid-arn-rejected
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams.create-application-invalid-source-uri-rejected|_error_code#3|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: gameliftstreams.create-stream-group-client-token-conflict-rejected|_error_code#9|AssertionError|raise AssertionError(
Issue-Key: gameliftstreams.create-stream-group-default-application-not-found|_without_internal_id#8|AssertionError|assert sep and rest.endswith(")"), f"expected a trailing '(Internal ID: ...)', got {message!r}"
Issue-Key: gameliftstreams.delete-stream-group-removes-resource|_without_internal_id#13|AssertionError|assert sep and rest.endswith(")"), f"expected a trailing '(Internal ID: ...)', got {message!r}"
Issue-Key: gameliftstreams.create-stream-group-invalid-stream-class-rejected|run#5|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: gameliftstreams.associate-applications-nonexistent-app-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: gameliftstreams.create-stream-session-connection-not-found-rejected|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: gameliftstreams.tag-resource-invalid-arn-rejected|run#3|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# GameLift Streams: 7 control-plane fidelity defects (missing CreateApplication/CreateStreamGroup validation, ignored ClientToken conflict, wrong not-found message format, over-wide StreamClass set, 2 unimplemented operations, wrong TagResource ARN error code)

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication, CreateStreamGroup, AssociateApplications, CreateStreamSessionConnection, TagResource, GetStreamGroup

## Description
Seven distinct root causes in the gameliftstreams emulator, all found by the behavioral suite replay (4/33 checks passed). Each cause below is independent and must be closed before this issue is submitted.

=== Cause 1 of 7: CreateApplication accepts an ApplicationSourceUri that is not an s3:// URI ===
AWS: CreateApplication validates the scheme of ApplicationSourceUri synchronously and rejects a non-S3 URI with ValidationException (Smithy: CreateApplication binds POST /applications, success code 201, and models ValidationException httpError 400; CreateApplicationInput.ApplicationSourceUri carries only @length min 1 max 1024 and no @pattern, so the scheme rule itself is recorded behavior only). The recorded rejection is ValidationException/HTTP 400 with the message built from the offending member, e.g. "'ApplicationSourceUri' must be a valid S3 URI starting with 's3://'" (example text; the requirement is a ValidationException naming that member).
Emulator: the call SUCCEEDS with HTTP 201 and allocates an application. Replay recorded ApplicationSourceUri='https://example.com/not-an-s3-uri.zip' coming back verbatim in the response with Status=INITIALIZED and a real Arn/Id.
Why: a client that mistypes or templates the wrong source URI gets a live application id and no error, then discovers the failure only much later (or never, since the emulator never drives the application to ERROR). Infrastructure code that asserts "bad source URI is rejected at create time" passes against the emulator and fails against AWS.
Fix: input validation for the CreateApplication handler (the ops/create_application.rs handler in the gameliftstreams service crate), before any resource is allocated.
Traces: gameliftstreams.create-application-invalid-source-uri-rejected.

=== Cause 2 of 7: CreateStreamGroup ignores ClientToken reuse with different request parameters ===
AWS: ClientToken is the modeled @idempotencyToken of CreateStreamGroup (Smithy: CreateStreamGroupInput.ClientToken, @length 32..128, pattern ^[\x21-\x7E]+$) and CreateStreamGroup models ConflictException (httpError 409). Replaying the same ClientToken with DIFFERENT parameters is rejected with ConflictException/HTTP 409; the recorded message is "'ClientToken' value has already been used with different request parameters" (example text; the requirement is the ConflictException code/status).
Emulator: the conflicting second CreateStreamGroup SUCCEEDS with HTTP 201 and creates a SECOND, independent stream group (replay shows a fresh Arn .../streamgroup/sg-TIpEF1, Status=ACTIVATING).
Why: idempotency is silently absent. A retrying client (or any SDK automatic retry) duplicates billable GPU stream groups instead of getting back the original or a 409, and a client relying on the conflict to detect a parameter drift between retries never sees it.
Fix: the CreateStreamGroup handler's client-token bookkeeping — record the token with a fingerprint of the request parameters in service state and compare on reuse (same-params reuse returns the original resource, different-params reuse raises ConflictException).
Traces: gameliftstreams.create-stream-group-client-token-conflict-rejected.

=== Cause 3 of 7: not-found error messages do not follow the service's message format and omit the per-request "(Internal ID: ...)" suffix ===
AWS: recorded behavior only (the model types ResourceNotFoundException with a single required Message and httpError 404, and says nothing about its text). Across this whole API the message is built from the request's resolved resource ARN in the shape "Resource '<arn>' does not exist", and EVERY GameLift Streams error message ends with a per-request " (Internal ID: <uuid>)" suffix — the behavioral harness depends on exactly that rule to strip it. So two requirements: (a) the message names the resolved ARN of the resource that was not found, (b) the message carries the trailing internal-id suffix. The uuid value itself is per-request and is NOT a requirement.
Emulator: returns the right code (ResourceNotFoundException) but an ad-hoc, suffix-less message: "no application matches DefaultApplicationIdentifier a-837180f8cd7b" (CreateStreamGroup with a bogus DefaultApplicationIdentifier) and "no stream group matches identifier arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-y0wpx1" (GetStreamGroup after DeleteStreamGroup). Neither resolves a bare identifier to an ARN nor appends the internal-id suffix.
Why: clients and test suites that parse these messages (to extract the ARN, or to strip the internal id before comparing) break; the format is uniform in AWS and is relied on as such.
Fix: the shared error-construction/serializer path for gameliftstreams not-found errors (one helper that resolves identifier -> ARN and appends the internal-id suffix), used by the CreateStreamGroup and GetStreamGroup handlers.
Traces: gameliftstreams.create-stream-group-default-application-not-found, gameliftstreams.delete-stream-group-removes-resource.

=== Cause 4 of 7: the StreamClass set CreateStreamGroup accepts is wider than the one AWS accepts ===
AWS: recorded behavior only. The recorded ValidationException for an unknown StreamClass enumerates the accepted set as [gen4n_high, gen4n_ultra, gen4n_win2022, gen5n_high, gen5n_ultra, gen5n_win2022, gen6n_small, gen6n_medium, gen6n_high, gen6n_ultra, gen6n_ultra_win2022, gen6n_pro, gen6n_pro_win2022, gen6e_pro, gen6e_pro_win2022] — 15 values. Note the on-disk Smithy enum com.amazonaws.gameliftstreams#StreamClass additionally contains gen6n_small_win2022 and gen6n_medium_win2022, so the live service accepts a NARROWER set than the model declares.
Emulator: validates against the full model enum, so its set includes gen6n_small_win2022 and gen6n_medium_win2022 (visible in the emulator's enumerated validation message) and it therefore admits two StreamClass values AWS rejects.
Why: overly permissive — a client that passes gen6n_small_win2022 gets a stream group from the emulator and a ValidationException from AWS, which is exactly the class of bug this emulator exists to catch.
Fix: the CreateStreamGroup handler's StreamClass validation — constrain to the service-accepted set rather than the raw model enum.
Traces: gameliftstreams.create-stream-group-invalid-stream-class-rejected.

=== Cause 5 of 7: AssociateApplications is not implemented ===
AWS: AssociateApplications is a real operation (Smithy: POST /streamgroups/{Identifier}/associations, success 200, errors AccessDeniedException 403, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400, ThrottlingException 429, InternalServerException 500). For an ApplicationIdentifier that does not exist on an ACTIVE stream group it returns ResourceNotFoundException/HTTP 404 with the message "Resource '<application-arn>' does not exist" (plus the internal-id suffix of Cause 3), and leaves the group's AssociatedApplications unchanged.
Emulator: every call returns error code NotImplemented ("operation AssociateApplications is scaffolded but not implemented in this emulator") — replay asserted ResourceNotFoundException and got 'NotImplemented'.
Why: no client can associate an application with a stream group at all, so nothing downstream of association (CreateStreamUrl, default application, stream sessions) can be exercised. This is NOT the stub already filed for AddStreamGroupLocations/TagResource/ListTagsForResource/UpdateStreamGroup — those are separate operations with their own issues; AssociateApplications has none because the check caught the error instead of propagating it.
Fix: implement the AssociateApplications handler (ops/associate_applications.rs in the gameliftstreams service crate) and its association state.
Traces: gameliftstreams.associate-applications-nonexistent-app-not-found.

=== Cause 6 of 7: CreateStreamSessionConnection is not implemented ===
AWS: CreateStreamSessionConnection is a real operation (Smithy: POST /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier}/connections, success 200, errors include ResourceNotFoundException 404 and ValidationException 400). For a stream group / stream session pair that does not exist it returns ResourceNotFoundException/HTTP 404; the recorded message is "Stream session '<streamsession-arn>' does not exist" (example text, plus the internal-id suffix of Cause 3).
Emulator: returns error code NotImplemented ("operation CreateStreamSessionConnection is scaffolded but not implemented in this emulator") — replay asserted ResourceNotFoundException and got 'NotImplemented'. This identifier-lookup boundary needs no real stream group, so it is reachable with no prerequisites.
Why: the reconnect API is entirely unusable, and even its cheap not-found boundary answers with a code no AWS client handles.
Fix: implement the CreateStreamSessionConnection handler (ops/create_stream_session_connection.rs in the gameliftstreams service crate), at minimum the identifier-resolution/not-found path.
Traces: gameliftstreams.create-stream-session-connection-not-found-rejected.

=== Cause 7 of 7: TagResource returns the wrong error code for a malformed ResourceArn ===
AWS: recorded behavior only for the code choice. TagResource's ResourceArn is @httpLabel with pattern ^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$, and the operation's modeled errors are AccessDeniedException/InternalServerException/ThrottlingException/ValidationException. For a ResourceArn that is not an ARN at all the recording shows error code BadRequestException with HTTP 400 and the message "Invalid input resource arn: bad syntax" — i.e. this operation's malformed-ARN boundary uses a DIFFERENT error code than the ValidationException/ResourceNotFoundException used elsewhere in the API, and that code is not even in the model's error list.
Emulator: returns ValidationException for ResourceArn="not-an-arn".
Why: a client that branches on the error code (retry vs. surface-to-user, or catch BadRequestException specifically) takes the wrong branch. Distinct from the already-filed "TagResource is unimplemented" stub issue: this path rejects before reaching the stub, so implementing the stub does not fix the code.
Fix: the TagResource handler's ARN-syntax validation (ops/tag_resource.rs) — raise BadRequestException, not ValidationException, when ResourceArn fails ARN syntax.
Traces: gameliftstreams.tag-resource-invalid-arn-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, gameliftstreams 4/33 passed (12%), 29 failed.

Cause 1 — gameliftstreams.create-application-invalid-source-uri-rejected:
  expected: ValidationException, message "'ApplicationSourceUri' must be a valid S3 URI starting with 's3://'"
  actual:   HTTP 201 success -> {'Arn': 'arn:aws:gameliftstreams:us-west-2:000000000000:application/a-8vuz81', 'ApplicationSourceUri': 'https://example.com/not-an-s3-uri.zip', 'Status': 'INITIALIZED', ...}
  assertion: "expected a rejection, but the call SUCCEEDED and returned {...}"

Cause 2 — gameliftstreams.create-stream-group-client-token-conflict-rejected:
  expected: ConflictException (HTTP 409), message "'ClientToken' value has already been used with different request parameters"
  actual:   HTTP 201 success -> second group {'Arn': '...:streamgroup/sg-TIpEF1', 'Status': 'ACTIVATING', ...}
  assertion: "expected ConflictException from the conflicting CreateStreamGroup call, but it succeeded"

Cause 3 — gameliftstreams.create-stream-group-default-application-not-found:
  expected: ResourceNotFoundException, message "Resource '<arn>' does not exist (Internal ID: <uuid>)"
  actual:   code ResourceNotFoundException but message 'no application matches DefaultApplicationIdentifier a-837180f8cd7b'
  assertion: "expected a trailing '(Internal ID: ...)'"
       and — gameliftstreams.delete-stream-group-removes-resource:
  actual:   message 'no stream group matches identifier arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-y0wpx1' (no ARN-form 'Resource ... does not exist', no internal-id suffix)

Cause 4 — gameliftstreams.create-stream-group-invalid-stream-class-rejected:
  expected enum set (AWS): [... gen6n_pro, gen6n_pro_win2022, gen6e_pro, gen6e_pro_win2022]
  actual enum set (emulator): [... gen6n_pro, gen6n_pro_win2022, gen6n_small_win2022, gen6n_medium_win2022, gen6e_pro, gen6e_pro_win2022]
  -> emulator accepts gen6n_small_win2022 and gen6n_medium_win2022, AWS does not.

Cause 5 — gameliftstreams.associate-applications-nonexistent-app-not-found:
  expected: ResourceNotFoundException (HTTP 404)
  actual:   'NotImplemented' — "operation AssociateApplications is scaffolded but not implemented in this emulator"

Cause 6 — gameliftstreams.create-stream-session-connection-not-found-rejected:
  expected: ResourceNotFoundException (HTTP 404)
  actual:   'NotImplemented' — "operation CreateStreamSessionConnection is scaffolded but not implemented in this emulator"

Cause 7 — gameliftstreams.tag-resource-invalid-arn-rejected:
  expected: BadRequestException (HTTP 400), message "Invalid input resource arn: bad syntax"
  actual:   ValidationException

Model references used: com.amazonaws.gameliftstreams#StreamClass (17 values incl. gen6n_small_win2022/gen6n_medium_win2022); CreateApplication (POST /applications, 201, ValidationException 400; ApplicationSourceUri @length 1..1024, no @pattern); CreateStreamGroup (POST /streamgroups, 201, ConflictException 409, ResourceNotFoundException 404; ClientToken @idempotencyToken @length 32..128); AssociateApplications (POST /streamgroups/{Identifier}/associations, 200, ResourceNotFoundException 404); CreateStreamSessionConnection (POST /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier}/connections, 200, ResourceNotFoundException 404); TagResource (POST /tags/{ResourceArn}, ResourceArn @httpLabel pattern ^arn:aws:gameliftstreams:...$, no BadRequestException modeled).

## Steps to Reproduce
Boot the emulator and point a botocore gameliftstreams client at it (region us-west-2), then:

Cause 1: create_application(Description='d', RuntimeEnvironment={'Type':'UBUNTU','Version':'22_04_LTS'}, ExecutablePath='run.sh', ApplicationSourceUri='https://example.com/not-an-s3-uri.zip') -> emulator returns 201; AWS returns ValidationException.

Cause 2: tok = 'a'*40; create_stream_group(Description='d1', StreamClass='gen4n_high', LocationConfigurations=[{'LocationName':'us-west-2','MaximumCapacity':2}], ClientToken=tok) then create_stream_group(Description='d2-DIFFERENT', StreamClass='gen4n_high', LocationConfigurations=[...], ClientToken=tok) -> emulator returns a second 201; AWS returns ConflictException 409.

Cause 3: create_stream_group(Description='d', StreamClass='gen4n_high', DefaultApplicationIdentifier='a-doesnotexist') and, separately, get_stream_group(Identifier=<arn of a deleted group>) -> inspect Error.Message: AWS form is "Resource '<arn>' does not exist (Internal ID: <uuid>)"; emulator emits 'no application matches DefaultApplicationIdentifier ...' / 'no stream group matches identifier ...' with no internal-id suffix.

Cause 4: create_stream_group(Description='d', StreamClass='not_a_real_stream_class', ...) and read the enumerated set in the ValidationException message; also try StreamClass='gen6n_small_win2022' -> emulator admits it, AWS rejects it.

Cause 5: associate_applications(Identifier=<any stream group id>, ApplicationIdentifiers=['a-doesnotexist']) -> NotImplemented.

Cause 6: create_stream_session_connection(Identifier='sg-bogus', StreamSessionIdentifier='ssdoesnotexist0', SignalRequest='{"type":"offer","sdp":"v=0"}') -> NotImplemented.

Cause 7: tag_resource(ResourceArn='not-an-arn', Tags={'k':'v'}) -> ValidationException instead of BadRequestException.

Or replay the checks listed in trace_ids.
