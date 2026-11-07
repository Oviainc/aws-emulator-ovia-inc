Issue-Source: trace-sweep-divergence
Issue-Trace: iot-vpce-traj-lenient-documents-accepted
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-vpce-traj-lenient-documents-accepted iot-vpce-traj-policy-document-refusals iot-policy-document-stored-verbatim-with-sourcevpce-condition iot-policy-list-lifecycle iot-policy-statement-without-condition-keeps-condition-absent iot-vpce-traj-fix-endpoint-via-policy-version iot-policy-versioning-limits
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreatePolicy|2xx->4xx|$.policyArn
Issue-Key: iot|CreatePolicy|2xx->4xx|$.policyDocument
Issue-Key: iot|CreatePolicy|2xx->4xx|$.policyName
Issue-Key: iot|CreatePolicy|2xx->4xx|$.policyVersionId
Issue-Key: iot|CreatePolicy|2xx->4xx|status
Issue-Key: iot|GetPolicy|2xx->4xx|$.creationDate
Issue-Key: iot|GetPolicy|2xx->4xx|$.defaultVersionId
Issue-Key: iot|GetPolicy|2xx->4xx|$.generationId
Issue-Key: iot|GetPolicy|2xx->4xx|$.lastModifiedDate
Issue-Key: iot|GetPolicy|2xx->4xx|$.policyArn
Issue-Key: iot|GetPolicy|2xx->4xx|$.policyDocument
Issue-Key: iot|GetPolicy|2xx->4xx|$.policyName
Issue-Key: iot|GetPolicy|2xx->4xx|status
Issue-Key: iot|CreatePolicy|4xx->2xx|$.message
Issue-Key: iot|CreatePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreatePolicy|4xx->2xx|status
Issue-Key: iot|CreatePolicy|4xx->4xx|status
Issue-Key: iot|GetPolicy|4xx->2xx|$.message
Issue-Key: iot|GetPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|GetPolicy|4xx->2xx|status
Issue-Key: iot|GetPolicy|2xx->2xx|$.generationId
Issue-Key: iot|GetPolicy|2xx->2xx|$.policyDocument
Issue-Key: iot|GetPolicy|2xx->2xx|$.defaultVersionId
Issue-Related: 20261107T022027Z-iot-already-exists-conflicts-9-causes-reworded-conflictexception.md
Issue-Related: 20261107T022027Z-iot-refusal-envelopes-8-causes-wrong-x-amzn.md
Issue-Commit: ff097a89ab5b098f807ae300b709617042ce117b

# iot policy-document plane: 10 causes — document validation is inverted, generationId is a UUID, and version reads are stale

> _Note: this issue was identified against the code base at commit `ff097a89ab5b098f807ae300b709617042ce117b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
The emulator's IoT policy plane validates policy documents in exactly the wrong places: it refuses documents AWS accepts (case-insensitive Effect) and accepts documents AWS refuses (unknown Version, unknown Condition operator), it rewrites the stored document instead of keeping it verbatim, it mints generationId as a UUID instead of the content digest AWS returns, and SetDefaultPolicyVersion does not move the default that GetPolicy reports. Because CreatePolicy refuses or accepts the wrong requests, several later steps in the same fixtures cascade into wrong statuses. 10 net-new causes, all in service iot.

Cause 1 of 10: CreatePolicy refuses a document whose Effect is lowercase 'allow'.
AWS: Smithy: CreatePolicy.policyDocument is @required, @length(0..404600), @pattern ^[\s\S]*$; the only document-shaped refusal is MalformedPolicyException (httpError 400). Recorded behavior: a statement with "Effect": "allow" is ACCEPTED — 200 with policyName, policyArn, the document echoed back and policyVersionId '1'.
Emulator: 400, no policyArn/policyDocument/policyName/policyVersionId in the body.
Why: AWS's IoT policy parser is lenient about Effect casing; a caller whose template emits lowercase effects cannot create any policy at all against the emulator, and the whole dependent flow stops at step 1.
Fix: the CreatePolicy handler's policy-document validation — Effect comparison must be case-insensitive.
Traces: iot-vpce-traj-lenient-documents-accepted#task#1.

Cause 2 of 10: GetPolicy cannot read the policy cause 1 refused.
AWS: Smithy: GetPolicy returns policyName/policyArn/policyDocument/defaultVersionId/creationDate/lastModifiedDate/generationId, with ResourceNotFoundException (404) when absent. Recorded behavior: 200 with all of those members populated for the policy created in step 1.
Emulator: 404 with every member absent.
Why: the read is a direct consequence of cause 1 but is its own wire divergence (wrong status and an empty body on an operation a client will retry); it must be re-replayed after the create is fixed.
Fix: no separate fix beyond cause 1 — this step verifies the create persisted; file under the CreatePolicy validation component.
Traces: iot-vpce-traj-lenient-documents-accepted#task#2.

Cause 3 of 10: CreatePolicy's malformed-JSON refusal uses the parser's own diagnostic.
AWS: Smithy: MalformedPolicyException (400). Recorded behavior: 400 whose message is the fixed sentence 'Policy document is Malformed: Invalid policy syntax.' — the 'Policy document is Malformed: ' prefix plus the reason clause; no parser offsets are included.
Emulator: 400 'policyDocument is not a valid JSON document: EOF while parsing a list at line 1 column 40' — the serde diagnostic, with line/column offsets AWS never emits.
Why: AWS's prefix is the discriminator a client uses to recognise a document problem; leaking the parser's internal text both breaks matching and exposes implementation detail.
Fix: the CreatePolicy handler's document-parse failure path.
Traces: iot-vpce-traj-policy-document-refusals#task#1.

Cause 4 of 10: CreatePolicy accepts a document whose Version is not 2012-10-17.
AWS: recorded behavior: 400 MalformedPolicyException with the fixed message 'Policy document is Malformed: Unknown policy document version. It must be version 2012-10-17'.
Emulator: 200 with an empty assertion body — the policy is created.
Why: the emulator silently accepts a document AWS rejects, so a caller gets a policy that would never exist in AWS and no signal that its template is wrong.
Fix: the CreatePolicy handler's document validation — check the Version field.
Traces: iot-vpce-traj-policy-document-refusals#task#2.

Cause 5 of 10: CreatePolicy accepts an unknown Condition operator, and the retry then collides.
AWS: recorded behavior: 400 MalformedPolicyException with the message 'Policy document is Malformed: Invalid Condition type : ' + <the unrecognised operator> — fixed text 'Policy document is Malformed: Invalid Condition type : ' (note the spaces around the colon), variable part the operator from the document.
Emulator: the first such request is ACCEPTED, so the subsequent attempts in the fixture answer 409 ResourceAlreadyExistsException 'Policy <name> already exists' instead of the 400 AWS returns for each.
Why: accepting an invalid Condition operator produces a policy that authorizes nothing predictable, and the resulting name collision turns every later refusal in the flow into the wrong error class.
Fix: the CreatePolicy handler's document validation — validate Condition operator names before persisting.
Traces: iot-vpce-traj-policy-document-refusals#task#3 (also iot-vpce-traj-policy-document-refusals#task#4, #task#5, #task#6).

Cause 6 of 10: GetPolicy answers 200 where AWS reports a missing default version.
AWS: recorded behavior: 404 with the fixed message 'Policy default version not found' (no variable part) when the named policy has no default version.
Emulator: 200 with no message member — it serves the policy.
Why: a client that must distinguish 'policy exists but has no usable default version' from 'policy readable' is told the wrong thing and will attach a policy with no effective document.
Fix: the GetPolicy handler's default-version resolution — refuse with ResourceNotFoundException when the default version is unresolvable.
Traces: iot-vpce-traj-policy-document-refusals#task#9.

Cause 7 of 10: CreatePolicy answers 409 where AWS creates the policy.
AWS: recorded behavior: 200 with policyName, policyArn, the document echoed verbatim and policyVersionId '1'.
Emulator: 409 with none of those members — because of cause 5 the name was already consumed by a document AWS would have rejected.
Why: the caller's legitimate create fails permanently; this step is the one that proves cause 5's cascade is cleared.
Fix: follows from cause 5; verify under the CreatePolicy validation component.
Traces: iot-vpce-traj-policy-document-refusals#task#10.

Cause 8 of 10: GetPolicy returns a REWRITTEN policy document — the Version field is dropped.
AWS: Smithy: GetPolicy.policyDocument is the document string, @length(0..404600). Recorded behavior: the document is returned byte-for-byte as submitted, including its leading "Version" member.
Emulator: returns the document with the Version member removed (statements preserved), and a generationId that is a random UUID rather than the content digest.
Why: IoT policy documents are opaque strings; rewriting one means a caller that round-trips GetPolicy into CreatePolicyVersion submits a document AWS would reject for a missing Version, and any digest or diff computed over it is wrong.
Fix: the policy state must store the submitted document verbatim and the GetPolicy serializer must not re-serialize it.
Traces: iot-vpce-traj-policy-document-refusals#task#11.

Cause 9 of 10: GetPolicy.generationId is a random UUID instead of a content digest.
AWS: Smithy: GetPolicy output member generationId targets GenerationId (string); the model states no format, so recorded behavior only: AWS returns a 64-character lowercase hex digest of the policy content, stable for identical content.
Emulator: returns a UUIDv4 (8-4-4-4-12 hyphenated form), which is neither hex-64 nor content-derived.
Why: generationId is how a client detects that a policy's content changed; a value that is random per policy (and not a digest) makes change detection and optimistic-concurrency checks impossible. This is a FORMAT divergence, not a per-run value difference — the recorded fixture asserts the shape and the emulator's value fails the shape, not merely the literal.
Fix: the policy state's generationId computation / the GetPolicy serializer.
Traces: iot-policy-document-stored-verbatim-with-sourcevpce-condition#task#2 (also iot-policy-list-lifecycle#task#4, iot-policy-statement-without-condition-keeps-condition-absent#task#2, iot-vpce-traj-fix-endpoint-via-policy-version#task#5, iot-vpce-traj-lenient-documents-accepted#task#4, #task#6, #task#8, #task#10, #task#12).

Cause 10 of 10: GetPolicy reports a stale defaultVersionId and the stale document after versions are added and the default is moved.
AWS: Smithy: GetPolicy.defaultVersionId targets PolicyVersionId with @pattern ^[0-9]+$. Recorded behavior: after creating further policy versions and setting the default, GetPolicy reports the CURRENT default version id and the document belonging to that version.
Emulator: reports defaultVersionId '1' and version 1's document, i.e. the default never moves and the document served is the original one.
Why: SetDefaultPolicyVersion is the supported way to roll a policy forward or back; if GetPolicy keeps reporting version 1, a client believes a rollout it performed had no effect, and every device-attached policy read is wrong.
Fix: the policy state's version registry and the SetDefaultPolicyVersion handler must update the default the GetPolicy read resolves.
Traces: iot-policy-versioning-limits#task#9.

Related open issues that do NOT cover these: '20261106T201842Z-iot-refusal-fidelity-10-net-new-causes-createpolicy.md' and '20261106T201842Z-iot-createpolicyversion-stub.md' carry no replay key for any of the ten fixture steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

1. CreatePolicy, iot-vpce-traj-lenient-documents-accepted#task#1 — status expected 200, got 400; $.policyArn/$.policyDocument/$.policyName/$.policyVersionId all expected present, got absent. Submitted document had "Effect": "allow".
2. GetPolicy, iot-vpce-traj-lenient-documents-accepted#task#2 — status expected 200, got 404; creationDate, defaultVersionId, generationId, lastModifiedDate, policyArn, policyDocument, policyName all expected present, got absent.
3. CreatePolicy, iot-vpce-traj-policy-document-refusals#task#1 — status 400 both; $.message expected 'Policy document is Malformed: Invalid policy syntax.', got 'policyDocument is not a valid JSON document: EOF while parsing a list at line 1 column 40'.
4. CreatePolicy, iot-vpce-traj-policy-document-refusals#task#2 — status expected 400, got 200; $.message expected 'Policy document is Malformed: Unknown policy document version. It must be version 2012-10-17', got absent; x-amzn-errortype expected present, got absent.
5. CreatePolicy, iot-vpce-traj-policy-document-refusals#task#3 — status expected 400, got 409; $.message expected 'Policy document is Malformed: Invalid Condition type : StringEqualsNope', got 'Policy sim2real-probe-iotvpce-doc-23a5ec8b already exists'; errortype got 'ResourceAlreadyExistsException'. Cluster of 4.
6. GetPolicy, iot-vpce-traj-policy-document-refusals#task#9 — status expected 404, got 200; $.message expected 'Policy default version not found', got absent.
7. CreatePolicy, iot-vpce-traj-policy-document-refusals#task#10 — status expected 200, got 409; policyArn/policyDocument/policyName/policyVersionId expected present, got absent.
8. GetPolicy, iot-vpce-traj-policy-document-refusals#task#11 — $.policyDocument expected to begin with the submitted '{"Version": "2012-10-17", "Statement": [...]}', got the same statements with the "Version" member removed; $.generationId expected a 64-hex digest, got '0ded199e-15fe-4be9-ae3a-31677cb8d097'.
9. GetPolicy, iot-policy-document-stored-verbatim-with-sourcevpce-condition#task#2 — $.generationId shape assertion failed: got 'aa7e9387-f90a-4ff2-9a55-cb29272ddd6c' (UUID) where a 64-char hex digest is required. Cluster of 9 steps.
10. GetPolicy, iot-policy-versioning-limits#task#9 — $.defaultVersionId expected '3', got '1'; $.policyDocument expected the version-3 document (Resource 'arn:aws:iot:*:*:client/device-3'), got the version-1 document (Resource '*'); $.generationId expected a 64-hex digest, got '5820a1be-eba0-4885-921b-a1005c809d24'.

(Policy names, UUIDs and digests above are this recording's values; the requirements are the fixed message texts, the verbatim-document rule, the hex-64 generationId format and the current-default-version rule described per cause.)

## Steps to Reproduce
1. git clone the repo and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Observe the ten clusters listed under Evidence, starting with fixture iot-vpce-traj-lenient-documents-accepted step task#1.
4. Equivalently by hand: POST /policies/<p> with "Effect": "allow" (expect 200); GET /policies/<p>; POST /policies/<p2> with truncated JSON; POST /policies/<p3> with "Version": "2008-10-17"; POST /policies/<p4> with Condition operator 'StringEqualsNope'; GET /policies/<p> and compare policyDocument byte-for-byte with what was submitted and generationId against the 64-hex digest form; then POST /policies/<p>/version twice, PATCH /policies/<p>/version/3, and GET /policies/<p> to check defaultVersionId and the served document.
