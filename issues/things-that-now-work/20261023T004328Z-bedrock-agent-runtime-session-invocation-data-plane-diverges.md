Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.concurrent-put-invocation-step-distinct-ids-all-persisted bedrock-agent-runtime.concurrent-put-invocation-step-unique-ids bedrock-agent-runtime.concurrent-put-invocation-step-same-id-consistent-winner bedrock-agent-runtime.put-invocation-step-duplicate-stepid-serialized bedrock-agent-runtime.end-already-ended-session-rejected bedrock-agent-runtime.get-invocation-step-matches-specific-step-among-many bedrock-agent-runtime.get-invocation-step-wrong-invocation-scope-rejected bedrock-agent-runtime.get-invocation-step-wrong-session-scope-rejected bedrock-agent-runtime.invocation-lifecycle-create-put-get bedrock-agent-runtime.invocation-step-binary-payload-preserved bedrock-agent-runtime.invocation-step-timestamp-precision-preserved bedrock-agent-runtime.invocation-steps-chronological-order bedrock-agent-runtime.list-invocation-steps-filtered-by-invocation-id bedrock-agent-runtime.list-invocation-steps-multiple-puts bedrock-agent-runtime.list-invocation-steps-ordering bedrock-agent-runtime.list-invocation-steps-pagination-complete bedrock-agent-runtime.list-invocation-steps-pagination-completeness bedrock-agent-runtime.put-get-invocation-step-round-trip bedrock-agent-runtime.put-invocation-step-after-end-session-conflict bedrock-agent-runtime.put-invocation-step-auto-generated-id-when-omitted bedrock-agent-runtime.put-invocation-step-conflicting-content-rejected bedrock-agent-runtime.put-invocation-step-duplicate-id-conflicting-payload-rejected bedrock-agent-runtime.put-invocation-step-idempotent-same-content bedrock-agent-runtime.put-invocation-step-multiple-content-blocks bedrock-agent-runtime.put-invocation-step-payload-round-trip bedrock-agent-runtime.put-invocation-step-round-trip bedrock-agent-runtime.put-invocation-step-round-trip-payload-integrity bedrock-agent-runtime.create-session-with-metadata-and-tags-persisted bedrock-agent-runtime.tag-resource-is-additive-merge bedrock-agent-runtime.tag-resource-round-trip bedrock-agent-runtime.tag-resource-unknown-arn-not-found bedrock-agent-runtime.tag-untag-resource-round-trip bedrock-agent-runtime.untag-resource-nonexistent-key-is-noop bedrock-agent-runtime.untag-resource-removes-tag bedrock-agent-runtime.create-invocation-duplicate-id-rejected bedrock-agent-runtime.duplicate-invocation-id-rejected bedrock-agent-runtime.concurrent-create-invocation-same-id-race bedrock-agent-runtime.concurrent-create-invocation-same-id-single-winner bedrock-agent-runtime.concurrent-end-session-single-winner bedrock-agent-runtime.end-session-already-ended-rejected bedrock-agent-runtime.end-session-twice-rejected bedrock-agent-runtime.cross-principal-get-session-denied
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|PutInvocationStep|ValidationException
Issue-Key: bedrock-agent-runtime.concurrent-put-invocation-step-same-id-consistent-winner|run#14|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}"
Issue-Key: bedrock-agent-runtime.put-invocation-step-duplicate-stepid-serialized|run#16|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got outcomes={outcomes}"
Issue-Key: bedrock-agent-runtime|ListTagsForResource|ValidationException
Issue-Key: bedrock-agent-runtime|TagResource|ValidationException
Issue-Key: bedrock-agent-runtime|UntagResource|ValidationException
Issue-Key: bedrock-agent-runtime.create-invocation-duplicate-id-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-agent-runtime.duplicate-invocation-id-rejected|_error#8|AssertionError|raise AssertionError(
Issue-Key: bedrock-agent-runtime.concurrent-create-invocation-same-id-race|run#10|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}: {results}"
Issue-Key: bedrock-agent-runtime.concurrent-create-invocation-same-id-single-winner|run#11|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}"
Issue-Key: bedrock-agent-runtime.concurrent-end-session-single-winner|run#9|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}: {results}"
Issue-Key: bedrock-agent-runtime.end-session-already-ended-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-agent-runtime.end-session-twice-rejected|_error#7|AssertionError|raise AssertionError(f"expected ConflictException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-agent-runtime.cross-principal-get-session-denied|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# bedrock-agent-runtime session/invocation data plane diverges: PutInvocationStep rejects every RFC3339 timestamp, tagging refuses session ARNs, no id-uniqueness/terminal-state conflicts, no authorization (5 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutInvocationStep

## Description
Five distinct root causes in the bedrock-agent-runtime session/invocation data plane. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json.

=== Cause 1 of 5: PutInvocationStep rejects every well-formed invocationStepTime with ValidationException 'invalid timestamp: failed to parse int' ===
AWS: Smithy: PutInvocationStep input member invocationStepTime -> DateTimestamp, required, @timestampFormat("date-time"). The protocol is restJson1, so the member arrives in the JSON body as an RFC3339/ISO-8601 date-time STRING (e.g. "2026-10-23T00:33:20.654272Z"), not as an epoch number. AWS accepts it and answers HTTP 201 with the modeled invocationStepId. ValidationException (400) is modeled on the operation but only for inputs that actually violate a constraint.
Emulator: every PutInvocationStep call is refused with ValidationException and the message 'invalid timestamp: failed to parse int' — the handler deserializes the member as an integer epoch instead of honouring @timestampFormat("date-time"), so NO value any SDK can send is accepted.
Why: the entire invocation-step plane is unreachable. A client cannot write a single step, so GetInvocationStep / ListInvocationSteps round-trips, ordering, pagination, payload integrity, idempotency and the ConflictException paths can never be exercised; the two concurrency checks below see zero winners rather than one, because all racers fail.
Fix: the input deserialization/validation of PutInvocationStep's handler (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/put_invocation_step.rs) must parse DateTimestamp members with the date-time wire format; the same decoder is shared by CreateSession/GetSession createdAt-style members.
Traces: 27 checks, see trace_ids (all the put/get/list-invocation-step checks plus concurrent-put-invocation-step-* and put-invocation-step-duplicate-stepid-serialized, whose 'expected exactly 1 winner, got 0' / outcomes={'a':'error','b':'error'} is this rejection firing on every racer).

=== Cause 2 of 5: TagResource / UntagResource / ListTagsForResource reject bedrock session ARNs with ValidationException ===
AWS: Smithy: TagResource.resourceArn -> TaggableResourcesArn, @httpLabel, required, @length(20,1011), @pattern "(^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session)/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$)". A session ARN is the ONLY taggable resource shape this service accepts; tagging a session succeeds (200) and ListTagsForResource returns the merged map. ResourceNotFoundException (404) is the modeled answer for a well-formed ARN naming no session.
Emulator: all three tag operations answer ValidationException (400) for a perfectly well-formed session ARN, echoing a pattern that belongs to a DIFFERENT service's taggable-resource shape — '(^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:(agent|agent-alias|knowledge-base|flow|prompt)/[A-Z0-9]{10}...|...retrieval-instruction/[a-z0-9]{10}$)'. That pattern admits only bedrock-agent (control-plane) resource types, never 'session', and additionally contains the broken escape '/d{12}' where '\\d{12}' was meant, so it could not match any real ARN either.
Why: tagging a session is impossible, and the modeled ResourceNotFoundException path for an unknown ARN is unreachable because validation fires first. Any client that tags sessions for cost allocation or access control gets a 400 on a request AWS accepts.
Fix: the resourceArn validation used by the tag_resource / untag_resource / list_tags_for_resource handlers in services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/ must use this service's own TaggableResourcesArn pattern (session ARNs), and the unknown-ARN branch must return ResourceNotFoundException.
Traces: 7 checks, see trace_ids.

=== Cause 3 of 5: CreateInvocation does not enforce invocationId uniqueness — duplicates are accepted with 201 instead of ConflictException ===
AWS: Smithy: CreateInvocation is @idempotent, input invocationId -> Uuid @pattern ^[a-f0-9]{8}-...-[a-f0-9]{12}$, and ConflictException (409) is modeled on the operation. Recorded behavior: creating a second invocation with an invocationId already present in the session is REJECTED; under N concurrent creates of the same id exactly one call wins and the rest are refused.
Emulator: the duplicate CreateInvocation succeeds and returns HTTP 201 with the same invocationId and sessionId; with 10 concurrent creates of one id all 10 returned ('ok', '<same invocationId>') and with 8 racers all 8 won. No ConflictException is ever produced.
Why: the invocation namespace is not a key. A client that relies on CreateInvocation to claim an id (de-duplicating retries, fencing a worker) believes it owns an invocation another caller also owns, and step writes from both callers silently interleave into one invocation.
Fix: the create_invocation handler and the session state it writes into (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/create_invocation.rs and the service state) must treat (sessionId, invocationId) as unique and answer ConflictException 409 on a second create, atomically under concurrency.
Traces: 4 checks, see trace_ids.

=== Cause 4 of 5: EndSession on an already-ENDED session returns 200 instead of ConflictException ===
AWS: Smithy: EndSession (PATCH /sessions/{sessionIdentifier}, success 200) models ConflictException (409). A session is a state machine ACTIVE -> ENDED -> deleted; recorded behavior is that ending a session that is already ENDED is refused with ConflictException, and under concurrent EndSession calls exactly one wins.
Emulator: the second EndSession returns HTTP 200 with sessionStatus 'ENDED' as though it had performed the transition; 6 concurrent EndSession calls all returned ('ok','ENDED'). The terminal state is treated as idempotently re-enterable.
Why: a client cannot tell whether IT ended the session or merely observed an already-ended one, so leader-election / single-finalizer patterns built on EndSession elect every caller.
Fix: the end_session handler must gate the transition on the current status and answer ConflictException 409 when the session is not ACTIVE (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/end_session.rs).
Traces: 3 checks, see trace_ids.

=== Cause 5 of 5: GetSession is answered for a principal with no bedrock permissions — no authorization is applied ===
AWS: Smithy: GetSession models AccessDeniedException (403). Recorded behavior: a session created by one principal and read by an unrelated, locked-down principal (STS credentials whose policy allows no bedrock action) is refused with AccessDeniedException, while the owning principal keeps reading the same session; the recorded message is built from the request — 'User: <principal-arn> is not authorized to perform: bedrock:GetSession on resource: <session-arn> because no identity-based policy allows the bedrock:GetSession action'.
Emulator: the stranger's GetSession on the owner's session ARN SUCCEEDS (the check's rejection helper reports 'it SUCCEEDED and returned None'); no 403 is ever produced.
Why: the emulator is over-permissive on a negative-class invariant. A client or test suite that proves tenant isolation or least-privilege against this emulator gets a pass that real AWS would fail, so an IAM regression ships undetected.
Fix: GetSession's handler must evaluate the caller's identity-based policy for bedrock:GetSession on the session ARN and return AccessDeniedException 403 (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/get_session.rs plus the shared authorization path).
Traces: 1 check, see trace_ids.

## Evidence
Cause 1 — expected: HTTP 201 + invocationStepId for invocationStepTime sent as an RFC3339 date-time string (Smithy @timestampFormat("date-time")). actual: 400 ValidationException, message 'invalid timestamp: failed to parse int' on every PutInvocationStep. Downstream: 'expected exactly 1 winner, got 0' and "outcomes={'a': 'error', 'b': 'error'}".

Cause 2 — expected: 200 for TagResource with resourceArn 'arn:aws:bedrock:us-west-2:000000000000:session/<uuid>' (modeled pattern admits ':(session)/<uuid>'). actual: 400 ValidationException "1 validation error detected: Value 'arn:aws:bedrock:us-west-2:000000000000:session/0cabde93-58b4-4d68-b230-90df328e1acb' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: (^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:(agent|agent-alias|knowledge-base|flow|prompt)/[A-Z0-9]{10}...)" — same rejection from TagResource, UntagResource and ListTagsForResource.

Cause 3 — expected: ConflictException 409 on the duplicate CreateInvocation. actual: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'invocationId': '709c7017-6217-4e18-beac-30598dc15717', 'sessionId': 'e1bc09e7-2008-4b42-9520-377c5ff5a26b'}"; concurrency: "expected exactly 1 winner, got 10: [('ok','ea4f34b7-9e38-4cd7-87e4-10e3b5bd6554') x10]" and "expected exactly 1 winner, got 8".

Cause 4 — expected: ConflictException 409 on the second EndSession. actual: "expected ConflictException, but the call SUCCEEDED and returned {'HTTPStatusCode': 200, ..., 'sessionStatus': 'ENDED'}"; concurrency: "expected exactly 1 winner, got 6: [('ok','ENDED') x6]".

Cause 5 — expected: AccessDeniedException 403 for the locked-down principal. actual: call succeeded — "expected the call to be rejected, but it SUCCEEDED and returned None".

## Steps to Reproduce
Cause 1: create_session(); create_invocation(sessionIdentifier=<id>); put_invocation_step(sessionIdentifier=<id>, invocationIdentifier=<invocation uuid>, invocationStepTime=datetime.now(timezone.utc), payload={'contentBlocks':[{'text':'x'}]}) -> emulator answers 400 ValidationException 'invalid timestamp: failed to parse int'.
Cause 2: create_session(); tag_resource(resourceArn=<sessionArn from CreateSession>, tags={'k':'v'}) -> 400 ValidationException on resourceArn; same for untag_resource and list_tags_for_resource.
Cause 3: create_session(); iid=str(uuid4()); create_invocation(sessionIdentifier=<id>, invocationId=iid) twice (and 10x concurrently) -> every call returns 201.
Cause 4: create_session(); end_session(sessionIdentifier=<id>); end_session(sessionIdentifier=<id>) again (and 6x concurrently) -> every call returns 200 with sessionStatus ENDED.
Cause 5: create_session() as principal A; mint STS credentials for a role with no bedrock permissions; call get_session(sessionIdentifier=<sessionArn>) with those credentials -> succeeds instead of 403 AccessDeniedException.
