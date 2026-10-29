Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-classify-document-bytes-and-reader-config
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-classify-document-bytes-and-reader-config comprehend-endpoint-management-errors comprehend-document-classifier-management-errors comprehend-job-status-checks-classification-language-entities
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|DescribeEndpoint|4xx->4xx|$.Message
Issue-Key: comprehend|DescribeEndpoint|4xx->4xx|status
Issue-Key: comprehend|DescribeDocumentClassifier|4xx->4xx|status
Issue-Key: comprehend|StopTrainingDocumentClassifier|4xx->4xx|status
Issue-Key: comprehend|DeleteDocumentClassifier|4xx->4xx|$.Message
Issue-Key: comprehend|DeleteDocumentClassifier|4xx->4xx|status
Issue-Key: comprehend|DescribeDominantLanguageDetectionJob|4xx->4xx|status
Issue-Key: comprehend|StopDominantLanguageDetectionJob|4xx->4xx|status
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# Comprehend not-found refusals on the model, endpoint and job planes answer HTTP 404 with reworded text instead of AWS's 400

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Six net-new root causes, all in how the Comprehend emulator renders a "the ARN you named does not exist" refusal. The service answers these as HTTP 400 on the wire (Comprehend's awsJson1_1 frontend serializes ResourceNotFoundException with a 400 status, not the 404 its Smithy httpError trait declares), and the Message is per-operation fixed text that NEVER interpolates the requested ARN. The emulator answers 404 everywhere and, where it does carry a Message, interpolates the request's ARN or uses the wrong operation's sentence.

Cause 1 of 6: DescribeEndpoint for an endpoint ARN that was never created.
  AWS: Smithy: DescribeEndpoint errors InvalidRequestException(400) / ResourceNotFoundException(404 declared) / TooManyRequestsException; recorded behavior shows the live service answering HTTP 400. The recorded body's Message is the fixed sentence 'RESOURCE_NOT_FOUND: Could not find specified resource.' — all fixed text, with NO variable part: the requested endpoint ARN does not appear in it.
  Emulator: HTTP 404, and Message 'RESOURCE_NOT_FOUND: Could not find the endpoint <EndpointArn>.' — it substitutes the request's EndpointArn into a sentence AWS does not use.
  Why: a client that branches on status (400 client-input error vs 404) routes the refusal down the wrong path, and a client asserting on the documented message text never matches. Echoing the ARN also leaks a request value into a message AWS keeps constant.
  Fix: the DescribeEndpoint handler's not-found path and the shared error serializer that maps ResourceNotFoundException to a status.
  Traces: comprehend-classify-document-bytes-and-reader-config#task#3 (also comprehend-endpoint-management-errors#task#3).

Cause 2 of 6: DescribeDocumentClassifier for a classifier ARN that does not exist.
  AWS: Smithy: DescribeDocumentClassifier errors ResourceNotFoundException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: same status misrouting; a wait/describe loop that treats 404 as a transport failure rather than a modeled client error retries or aborts.
  Fix: the DescribeDocumentClassifier handler's not-found path / the ResourceNotFoundException status mapping.
  Traces: comprehend-document-classifier-management-errors#task#3.

Cause 3 of 6: StopTrainingDocumentClassifier for a classifier ARN that does not exist.
  AWS: Smithy: StopTrainingDocumentClassifier errors ResourceNotFoundException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: as above, for the training-control plane.
  Fix: the StopTrainingDocumentClassifier handler's not-found path.
  Traces: comprehend-document-classifier-management-errors#task#4.

Cause 4 of 6: DeleteDocumentClassifier for a classifier ARN that does not exist — wrong status AND the wrong sentence.
  AWS: Smithy: DeleteDocumentClassifier errors ResourceNotFoundException; recorded behavior shows HTTP 400 with Message 'Could not find specified model.' — entirely fixed text, model-flavoured because the target is a model, with no variable part.
  Emulator: HTTP 404 with Message 'RESOURCE_NOT_FOUND: Could not find specified resource.' — the generic resource sentence, so the per-operation wording is lost on top of the status.
  Why: a client distinguishing "model gone" from a generic resource miss by message text cannot.
  Fix: the DeleteDocumentClassifier handler's not-found path (it must state the model-specific sentence, not the crate's generic one).
  Traces: comprehend-document-classifier-management-errors#task#5.

Cause 5 of 6: DescribeDominantLanguageDetectionJob for a job id that does not exist.
  AWS: Smithy: DescribeDominantLanguageDetectionJob errors JobNotFoundException/InvalidRequestException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: async-job pollers key off the 400 client-error class; a 404 is read as an endpoint/routing fault.
  Fix: the DescribeDominantLanguageDetectionJob handler's not-found path.
  Traces: comprehend-job-status-checks-classification-language-entities#task#3.

Cause 6 of 6: StopDominantLanguageDetectionJob for a job id that does not exist.
  AWS: Smithy: StopDominantLanguageDetectionJob errors JobNotFoundException/InvalidRequestException; recorded behavior only — HTTP 400.
  Emulator: HTTP 404.
  Why: as above, for job cancellation.
  Fix: the StopDominantLanguageDetectionJob handler's not-found path.
  Traces: comprehend-job-status-checks-classification-language-entities#task#4.

## Evidence
Cause 1: expected status 400, actual 404; $.Message expected 'RESOURCE_NOT_FOUND: Could not find specified resource.', actual 'RESOURCE_NOT_FOUND: Could not find the endpoint arn:aws:comprehend:<region>:<account>:document-classifier-endpoint/<name>.' (the recording's ARN is the fixture's own value filling the emulator's template; the ARN is not part of AWS's message at all).
Cause 2: DescribeDocumentClassifier — status: expected 400, got 404.
Cause 3: StopTrainingDocumentClassifier — status: expected 400, got 404.
Cause 4: DeleteDocumentClassifier — status: expected 400, got 404; $.Message expected 'Could not find specified model.', got 'RESOURCE_NOT_FOUND: Could not find specified resource.'
Cause 5: DescribeDominantLanguageDetectionJob — status: expected 400, got 404.
Cause 6: StopDominantLanguageDetectionJob — status: expected 400, got 404.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture comprehend-classify-document-bytes-and-reader-config, step task#3 — DescribeEndpoint with an endpoint ARN that was never created (also reproduced by comprehend-endpoint-management-errors#task#3).
3. Causes 2-4: fixture comprehend-document-classifier-management-errors, steps task#3, task#4, task#5 — DescribeDocumentClassifier / StopTrainingDocumentClassifier / DeleteDocumentClassifier against a nonexistent classifier ARN.
4. Causes 5-6: fixture comprehend-job-status-checks-classification-language-entities, steps task#3, task#4 — Describe/Stop DominantLanguageDetectionJob with an unknown JobId.
5. Observe HTTP 404 (and the reworded Message in causes 1 and 4) where the recording has HTTP 400.
