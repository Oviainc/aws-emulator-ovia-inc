Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-error-paths
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-error-paths
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# emr-serverless UntagResource answers the wrong error-type identity on its 400 refusal

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
One cause in the emr-serverless tagging surface: UntagResource refuses the request with the right HTTP status but advertises an error-type identity that does not match the one AWS records, and it has no operation-level error path that could produce the right one.

Cause 1 of 1: UntagResource's 400 carries an x-amzn-errortype value that diverges in shape from the recorded error identity.
AWS: Smithy: UntagResource DELETE /tags/{resourceArn}, success 200 with an empty body; input resourceArn (ResourceArn, @required, @httpLabel, @length 60..1024, pattern ^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+(\/jobruns\/[0-9a-zA-Z]+|\/sessions\/[0-9a-zA-Z]+)?$) and tagKeys (TagKeyList, @required, @httpQuery "tagKeys", @length 1..200, member TagKey @length 1..128 pattern ^[A-Za-z0-9 /_.:=+@-]+$); modeled errors ValidationException (HTTP 400, com.amazonaws.emrserverless#ValidationException, @required message), ResourceNotFoundException (HTTP 404) and InternalServerException (HTTP 500). The service is restJson1, so the error identity a client matches on travels in the x-amzn-errortype response header. Recorded behavior: this step's UntagResource is refused with HTTP 400 and an x-amzn-errortype header whose value the corpus asserts by SHAPE rather than literally; the emulator's value does not satisfy that shape. The recording does not pin a literal error code for this step, so the requirement here is the error-type identity's form, not a specific message string.
Emulator: answers HTTP 400 (status matches) but sets x-amzn-errortype to the bare literal "ValidationException" from its generic pre-handler boundary-constraint refusal, which replay reports as $headers.x-amzn-errortype shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException'). UntagResource is additionally wholly unimplemented: services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/untag_resource.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() body returns crate::scaffolded::Scaffolded::not_implemented("UntagResource"), so the ONLY 400 this operation can emit is the generic boundary refusal; no code path exists that could emit the operation's own modeled ValidationException (or ResourceNotFoundException for an ARN that resolves to no application) with the identity AWS records. Hypothesis: the recorded header is not the bare exception name the emulator emits — emr-serverless records the restJson1 error type in a qualified form (ExceptionName plus a Coral namespace suffix), or names a different exception for this refusal — and the emulator's generic refusal path cannot produce either.
Why: a restJson1 client dispatches on x-amzn-errortype, not on the status alone. With the wrong identity on the header, an SDK deserialises the 400 into a generic/unknown service error instead of the modeled exception, so a caller's `catch ValidationException` (or `catch ResourceNotFoundException`) branch never runs and retry/classification logic that keys off the exception type misfires — while the status alone makes the response look correct.
Fix: UntagResource's handler plus the error serializer that populates x-amzn-errortype — implement the operation so a bad or unknown resourceArn is refused through the operation's OWN modeled error (com.amazonaws.emrserverless#ValidationException / ResourceNotFoundException) rather than falling through to the generic scaffold/boundary ValidationException, and emit the error-type header in the form the recording carries.
Traces: emr-serverless-error-paths#task#7; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/untag_resource.rs:1-12 and :139-157; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/scaffolded.rs:180-199 and :261-291.

Note: the other three emr-serverless clusters in this replay (CreateApplication task#3, DeleteApplication teardown#8, GetApplication teardown#9) are already owned by open issue 20261022T042729Z-emr-serverless-application-lifecycle-and-create-time-engine.md, which does NOT list any UntagResource key — this divergence is net-new and not covered by it.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, score 43.5%, 23 steps: 10 passed, 13 mismatched, 0 errored, 0 blocked; state_reset_failures=0, so this is not a cross-fixture state leak).

Cause 1 — emr-serverless-error-paths#task#7, UntagResource, verdict mismatch:
  expected_status 400, actual_status 400 (status matches)
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  divergence_key: service=emr-serverless|UntagResource|status=400->400|keys=$headers.x-amzn-errortype
  affected: ['emr-serverless-error-paths#task#7'] (affected_count 1)
  stub_path: "" (the 400 is produced before the handler, so the scaffold 501 is never reached and no stub issue covers it); owned_by: [] (no open issue carries this replay key).

Model evidence (smithy_query on services/service-com-amazonaws-emrserverless-awstoledowebservice/smithy-model/com-amazonaws-emrserverless-awstoledowebservice.json):
  UntagResource http = {method DELETE, uri /tags/{resourceArn}, code 200 (defaulted)}, idempotent; errors = ValidationException 400 (client), ResourceNotFoundException 404 (client), InternalServerException 500 (server).
  com.amazonaws.emrserverless#ValidationException = structure, errorKind client, httpError 400, member message (String1024, @required, @length 1..1024, pattern .*\S.*).

Code evidence: ops/untag_resource.rs retains the machine-readable '//! SCAFFOLDED, NOT IMPLEMENTED:' marker and its handle() returns Scaffolded::not_implemented("UntagResource"); scaffolded.rs Scaffolded::ValidationException hard-codes shape_name() = "ValidationException" and http_status() = 400, which is the identity observed on the wire.

## Steps to Reproduce
1. Clone the repo (git_clone) and boot the emulator built from that checkout.
2. Replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
3. Inspect fixture emr-serverless-error-paths, step task#7 — the UntagResource error-path call (DELETE /tags/{resourceArn}?tagKeys=...).
4. Observe: the emulator answers HTTP 400 (matching the recording's status) but with x-amzn-errortype: ValidationException, which the comparator reports as a shape mismatch against the recorded error-type identity. The recording's header is asserted by shape and the emulator's bare literal does not satisfy it.
5. Confirm the structural cause: services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/untag_resource.rs is still a scaffold (handle() returns Scaffolded::not_implemented), so the operation has no path that can emit its own modeled com.amazonaws.emrserverless#ValidationException or ResourceNotFoundException — every 400 it produces comes from the generic boundary/scaffold refusal in src/scaffolded.rs.
6. After fixing, re-replay emr-serverless-error-paths#task#7 and check that $headers.x-amzn-errortype no longer diverges while the status stays 400.
