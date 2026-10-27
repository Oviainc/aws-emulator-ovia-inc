Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-invoke-rest-api-nonexistent-environment-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-invoke-rest-api-nonexistent-environment-not-found
Issue-Shape-Id: com.amazonaws.mwaa#AmazonMWAA
Issue-Key: mwaa|InvokeRestApi|4xx->4xx|$
Issue-Commit: 0550644949cffe934022bf1b5a8b285c9bc5e880

# InvokeRestApi not-found answers a modeled ResourceNotFoundException document instead of the bare message body AWS returns

> _Note: this issue was identified against the code base at commit `0550644949cffe934022bf1b5a8b285c9bc5e880`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeRestApi

## Description
One cause.

Cause 1 of 1: InvokeRestApi against an environment name that does not exist returns the wrong response BODY SHAPE for its 404.

AWS: recorded behavior only. The Smithy model (services/service-com-amazonaws-mwaa-amazonmwaa, AmazonMWAA 2020-07-01, restJson1) binds InvokeRestApi to POST /restapi/{Name} with hostPrefix 'env.' and models ResourceNotFoundException (errorKind client, httpError 404, single member 'message'); the model says nothing about how the env.* data-plane path serializes a missing-environment failure. The recording shows the data plane answering HTTP 404 whose body is the bare message value and nothing else - a scalar at the document root, with no error-document wrapper and no error-code discriminator. The message text for this error class is the fixed string 'Environment not found'; it has no variable parts (the requested environment name is NOT interpolated into it), so the whole body is that fixed text. Status 404 is a requirement, as is the body being just that message rather than an error structure.

Emulator: the emulator gets the status right (404) but serializes the failure as a restJson1/awsJson-style error document, i.e. an object carrying an error-code discriminator plus a message member - {"__type": "ResourceNotFoundException", "message": "Environment not found"} - where AWS puts the bare message. The replay reports this as '$: type_mismatch (expected 'Environment not found', got {'__type': 'ResourceNotFoundException', 'message': 'Environment not found'})', i.e. the divergence is at the document root: scalar expected, object produced. Two things are wrong at once: the wrapper exists at all, and the wrapper it uses carries '__type', which is not even the restJson1 error discriminator (restJson1 signals the error shape with the X-Amzn-Errortype header / 'code' field, not '__type'), so this error path is serialized with the wrong protocol's error envelope.

Why: a client that reads the 404 body as the human-readable failure text - which is all AWS gives it here - gets a JSON object instead of a string, so any direct body read, string comparison or log of the failure reason breaks or prints a serialized object. Conversely a client relying on this path being an UNMODELED 404 (the SDK surfaces it as a generic service error because there is no error-code discriminator on the wire) instead receives a discriminator and gets a typed ResourceNotFoundException, so error-handling branches that distinguish 'environment missing' from 'Airflow REST call failed' take the opposite branch against the emulator than against AWS. Because the wrapper also uses the wrong protocol's discriminator key, hand-rolled clients and non-AWS SDK HTTP clients that look for the restJson1 discriminator find none and fall through to their unknown-error path.

Fix: the InvokeRestApi handler's not-found error path and the mwaa error serializer it routes through. The missing-environment failure on the env.* data-plane route must emit a 404 whose body is only the error message text, not an error structure; and wherever this service does emit a modeled error document, it must use the restJson1 error envelope rather than the '__type' discriminator.

Traces: mwaa-invoke-rest-api-nonexistent-environment-not-found#task#1 (InvokeRestApi on a nonexistent environment name).

## Evidence
Fixture mwaa-invoke-rest-api-nonexistent-environment-not-found, step task#1, operation InvokeRestApi.
Verdict: mismatch. Expected status 404, actual status 404 (status agrees).
Body divergence at document root:
  Expected (AWS, recorded): 'Environment not found'  - a bare scalar message body.
  Actual (emulator): {"__type": "ResourceNotFoundException", "message": "Environment not found"}  - an error document with a '__type' discriminator.
Replay detail: "$: type_mismatch (expected 'Environment not found', got {'__type': 'ResourceNotFoundException', 'message': 'Environment not found'})".
divergence_key: service=mwaa|InvokeRestApi|status=404->404|keys=$
Smithy: InvokeRestApi POST /restapi/{Name}, hostPrefix 'env.', protocol restJson1; errors include ResourceNotFoundException (client, httpError 404, member 'message'). The model does not specify a bare-string error body, hence 'recorded behavior only' for the body shape; note also that restJson1 does not use '__type' as its error discriminator.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-ga-prod-v1).
2. Call MWAA InvokeRestApi (POST /restapi/{Name} on the env.* data-plane route) with a Name that identifies no existing environment, plus a valid Path and Method.
3. Observe the response: status is 404 as expected, but the body is an error document ({"__type": "ResourceNotFoundException", "message": ...}) instead of the bare message text 'Environment not found' that the recorded AWS response carries at the document root.
4. Equivalently: replay fixture mwaa-invoke-rest-api-nonexistent-environment-not-found, step task#1, which reports '$: type_mismatch'.
