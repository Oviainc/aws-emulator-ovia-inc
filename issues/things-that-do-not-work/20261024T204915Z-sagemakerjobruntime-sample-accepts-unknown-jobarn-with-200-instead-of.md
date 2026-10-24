Issue-Source: trace-sweep-divergence
Issue-Trace: sagemakerjobruntime-resource-not-found-nonexistent-job
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemakerjobruntime-resource-not-found-nonexistent-job
Issue-Shape-Id: com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService
Issue-Key: sagemakerjobruntime|Sample|4xx->2xx|$.Message
Issue-Key: sagemakerjobruntime|Sample|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: sagemakerjobruntime|Sample|4xx->2xx|status
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# Sample accepts unknown JobArn with 200 instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Sample

## Description
Cause 1 of 1: Sample does not validate that the JobArn in the X-Amzn-SageMaker-Job-Arn header refers to an existing job, so a request for a job that was never created is served as a successful inference instead of being rejected.

AWS: Sample (restJson1, POST /sample) takes a required JobArn header member (Smithy: SampleRequest.JobArn, httpHeader X-Amzn-SageMaker-Job-Arn, @required, @length min 1 max 256, @pattern ^arn:aws[a-z\-]*:sagemaker:...:job/...$) that identifies which model session the inference is routed to. The operation's modeled client errors include ValidationException (Smithy: ValidationException, errorKind client, httpError 400, required member Message targeting FailureReason) and ResourceNotFoundException (404). The recorded response for a syntactically valid but nonexistent job ARN is HTTP 400 with the error type header x-amzn-errortype set to 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.agenticrftruntimeservice/' and a Message built from the request that names the unresolvable job ARN (recorded example: "Could not find job with jobArn arn:aws:sagemaker:us-west-2:000000000000:job/AgentRFT/sim2real-probe-sagemakerjobruntime-ghost-cdf5a33f"). The requirement is the 400 status, the ValidationException error type, and a Message that names the JobArn from the request; the recorded wording is an example.

Emulator: the replayed step returns HTTP 200. The response carries no $.Message member and no x-amzn-errortype header, i.e. the emulator serves a normal Sample success response for a job ARN that does not exist in its state.

Why: a client that relies on AWS's behavior cannot distinguish a live job from a missing/expired/misaddressed one. SDK error handling never fires, so a harness that probes a job ARN before sampling (or that retries/fails over on ValidationException) proceeds against a nonexistent session and silently consumes a fabricated inference response; negative tests asserting 400/ValidationException pass vacuously.

Fix: belongs in the Sample operation handler's request validation for the sagemakerjobruntime service — resolve the JobArn header against job/session state and, when no job matches, short-circuit with the modeled ValidationException (HTTP 400, x-amzn-errortype ValidationException, Message naming the requested jobArn) instead of falling through to the inference success path.

Traces: sagemakerjobruntime-resource-not-found-nonexistent-job#task#1.

## Evidence
Fixture sagemakerjobruntime-resource-not-found-nonexistent-job, step task#1 (operation Sample, service sagemakerjobruntime), verdict mismatch.
Expected: status 400; $.Message = 'Could not find job with jobArn arn:aws:sagemaker:us-west-2:000000000000:job/AgentRFT/sim2real-probe-sagemakerjobruntime-ghost-cdf5a33f' (example of a message naming the unresolvable JobArn); header x-amzn-errortype = 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.agenticrftruntimeservice/'.
Actual: status 200; $.Message missing (None); header x-amzn-errortype missing (None).
Divergence key: service=sagemakerjobruntime|Sample|status=400->200|keys=$.Message,$headers.x-amzn-errortype,status.
Smithy: Sample errors include ValidationException (client, httpError 400) and ResourceNotFoundException (client, httpError 404); SampleRequest.JobArn is @required with @pattern on a sagemaker job ARN.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture sagemakerjobruntime-resource-not-found-nonexistent-job, step task#1.
2. Equivalently: POST /sample with header X-Amzn-SageMaker-Job-Arn set to a well-formed sagemaker job ARN that was never created in the emulator (e.g. arn:aws:sagemaker:us-west-2:000000000000:job/AgentRFT/<nonexistent-name>), header X-Amzn-SageMaker-Trajectory-Id set to any value, and an OpenAI-compatible JSON inference body.
3. Observe HTTP 200 with a success payload and no x-amzn-errortype header, instead of HTTP 400 with x-amzn-errortype ValidationException and a Message naming the unresolvable jobArn.
