Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected comprehend.batch-detect-entities-batch-size-limit-rejected comprehend.describe-document-classifier-not-found comprehend.document-classifier-lifecycle-training-to-stopped comprehend.delete-resource-policy-without-policy-not-found comprehend.start-pii-entities-detection-job-missing-redaction-config-rejected
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected|run#4|AssertionError|assert code == "BatchSizeLimitExceededException", f"unexpected error code: {code!r}"
Issue-Key: comprehend.batch-detect-entities-batch-size-limit-rejected|run#4|AssertionError|assert code == "BatchSizeLimitExceededException", (
Issue-Key: comprehend.describe-document-classifier-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: comprehend.document-classifier-lifecycle-training-to-stopped|_retry_role_propagation#4|Timeout|time.sleep(interval_s)
Issue-Key: comprehend.delete-resource-policy-without-policy-not-found|_create_classifier#6|Timeout|time.sleep(6.0)
Issue-Key: comprehend.start-pii-entities-detection-job-missing-redaction-config-rejected|run#9|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got code={code!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Comprehend answers HTTP 501 NotImplemented for BatchDetectEntities, the document-classifier CRUD family and StartPiiEntitiesDetectionJob, so their AWS rejections and lifecycles are never produced (4 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectEntities

## Description
Four operation families in the bootstrapped comprehend service crate are still scaffold stubs that answer HTTP 501 with error code NotImplemented ('operation X is scaffolded but not implemented in this emulator'). Each is a separate cause with its own handler file; none of them is covered by the stub issues this suite filed for the Detect*/BatchDetect*/Start*Job operations it named. All files are under services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/.

Cause 1 of 4 — BatchDetectEntities is a stub, so its batch-size quota rejection never happens.
AWS: Smithy: BatchDetectEntities(TextList -> CustomerInputStringList @required @length min 1 — documented 'maximum of 25 documents', LanguageCode @required) -> {ResultList @required, ErrorList @required}; modeled errors BatchSizeLimitExceededException (HTTP 400), TextSizeLimitExceededException (400), InvalidRequestException (400), UnsupportedLanguageException (400), InternalServerException (500). Recorded behavior: a 26-document TextList is rejected wholesale with HTTP 400 BatchSizeLimitExceededException (recorded message example: 'Please use a smaller number of items in a single batch. Maximum batch size is 25.') with no partial ResultList/ErrorList, while a compliant list succeeds.
Emulator: every BatchDetectEntities call answers HTTP 501 / NotImplemented from the scaffold marker in src/ops/batch_detect_entities.rs (the SCAFFOLDED marker is still on the file and handle() returns Scaffolded::not_implemented).
Why: clients get a 501 where AWS returns either a result or a modeled 400; retry/batching logic that splits oversized batches on BatchSizeLimitExceededException is untestable, and the 501 is not a modeled error for this operation at all.
Fix: implement the handler in src/ops/batch_detect_entities.rs (detection plus the 25-document and per-document size preconditions, returning the modeled errors).
Traces: comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected, comprehend.batch-detect-entities-batch-size-limit-rejected.

Cause 2 of 4 — DescribeDocumentClassifier is a stub, so ResourceNotFoundException for an unknown classifier ARN never happens.
AWS: Smithy: DescribeDocumentClassifier models ResourceNotFoundException (HTTP 404, errorKind client) and returns DocumentClassifierProperties for an existing classifier. Recorded behavior: a syntactically valid but nonexistent DocumentClassifierArn is rejected with ResourceNotFoundException (recorded message example: 'RESOURCE_NOT_FOUND: Could not find specified resource.').
Emulator: answers HTTP 501 / NotImplemented (src/ops/describe_document_classifier.rs still carries the SCAFFOLDED marker), so the check observes error code 'NotImplemented' where it expects ResourceNotFoundException.
Why: existence checks are the primary control-plane probe for custom models; a 501 is neither the success shape nor the modeled 404, so nothing built on classifier existence can run.
Fix: implement the handler in src/ops/describe_document_classifier.rs over the service's classifier state, returning 404 ResourceNotFoundException for unknown ARNs.
Traces: comprehend.describe-document-classifier-not-found.

Cause 3 of 4 — CreateDocumentClassifier is a stub, so no classifier can be provisioned and the whole custom-model lifecycle (and the resource-policy APIs that need a real classifier) is unreachable.
AWS: Smithy: CreateDocumentClassifier returns DocumentClassifierArn and the classifier walks SUBMITTED -> TRAINING -> (TRAINED | STOPPED after StopTrainingDocumentClassifier) -> deleted; tags round-trip on the live ARN, and the resource-policy operations (PutResourcePolicy/DescribeResourcePolicy/DeleteResourcePolicy) operate on that ARN, returning plain 200s for an existing policy-less classifier and ResourceNotFoundException only for an ARN that was never created.
Emulator: CreateDocumentClassifier answers HTTP 501 / NotImplemented (src/ops/create_document_classifier.rs still carries the SCAFFOLDED marker). Both affected checks retry the 501 inside their provisioning helper and are killed by the check budget ('Check timed out after 60.0s'), so every downstream assertion — SUBMITTED->TRAINING, tag round-trip, STOPPED, delete-then-404, and the resource-policy contrast — is never reached.
Why: no custom-model workflow can be emulated at all, and the timeout hides the real signal: a client provisioning a classifier gets a 501 that AWS never returns.
Fix: implement src/ops/create_document_classifier.rs together with the classifier state machine it feeds (and the sibling reads in Cause 2) so the lifecycle and the resource-policy operations have a real resource to act on.
Traces: comprehend.document-classifier-lifecycle-training-to-stopped, comprehend.delete-resource-policy-without-policy-not-found.

Cause 4 of 4 — StartPiiEntitiesDetectionJob is a stub, so its mode/配置 precondition and job minting never happen.
AWS: Smithy: StartPiiEntitiesDetectionJob models InvalidRequestException (HTTP 400) and returns a JobId/JobStatus. Recorded behavior: Mode='ONLY_REDACTION' without a RedactionConfig is rejected with HTTP 400 InvalidRequestException and no job is minted (recorded message example: 'INPUT_CONFIG_INVALID_FOR_MODE: A RedactionConfig definition is required for PII detection requests that set the Mode parameter to ONLY_REDACTION. ...'), while the same request with Mode='ONLY_OFFSETS' succeeds and returns a real JobId that DescribePiiEntitiesDetectionJob can read back.
Emulator: answers HTTP 501 / NotImplemented (src/ops/start_pii_entities_detection_job.rs still carries the SCAFFOLDED marker); the check sees error code 'NotImplemented' instead of InvalidRequestException.
Why: both the rejection path and the happy path of PII redaction jobs are absent, so a client cannot start a PII job or test its mode validation.
Fix: implement src/ops/start_pii_entities_detection_job.rs — validate the Mode/RedactionConfig combination (400 InvalidRequestException) and mint the job in the service's job state.
Traces: comprehend.start-pii-entities-detection-job-missing-redaction-config-rejected.

## Evidence
Cause 1: batch_detect_entities(TextList=<26 docs>, LanguageCode='en') — expected error code 'BatchSizeLimitExceededException' (HTTP 400); got 'NotImplemented' (HTTP 501, 'operation BatchDetectEntities is scaffolded but not implemented in this emulator'). src/ops/batch_detect_entities.rs line 4 '//! SCAFFOLDED, NOT IMPLEMENTED', line 177 Err(crate::scaffolded::Scaffolded::not_implemented("BatchDetectEntities")).
Cause 2: describe_document_classifier(DocumentClassifierArn=<nonexistent>) — expected 'ResourceNotFoundException' (HTTP 404); got 'NotImplemented' (HTTP 501). src/ops/describe_document_classifier.rs line 4 carries the SCAFFOLDED marker.
Cause 3: create_document_classifier(...) answers HTTP 501 NotImplemented (src/ops/create_document_classifier.rs line 4 SCAFFOLDED marker); both dependent checks report 'Check timed out after 60.0s' because their provisioning helpers retry the 501 until the budget expires, so SUBMITTED->TRAINING->STOPPED, tag round-trip, delete-then-404 and the DeleteResourcePolicy/DescribeResourcePolicy contrast are never evaluated.
Cause 4: start_pii_entities_detection_job(Mode='ONLY_REDACTION', no RedactionConfig) — expected 'InvalidRequestException' (HTTP 400); got code 'NotImplemented' (HTTP 501). src/ops/start_pii_entities_detection_job.rs line 4 carries the SCAFFOLDED marker.

## Steps to Reproduce
1. Boot the emulator and point a botocore comprehend client at it.
2. Cause 1: comprehend.batch_detect_entities(TextList=['a']*26, LanguageCode='en') — expect 400 BatchSizeLimitExceededException, observe 501 NotImplemented; also try a 2-document compliant list and observe 501 instead of ResultList/ErrorList.
3. Cause 2: comprehend.describe_document_classifier(DocumentClassifierArn='arn:aws:comprehend:<region>:<account>:document-classifier/does-not-exist') — expect 404 ResourceNotFoundException, observe 501.
4. Cause 3: comprehend.create_document_classifier(DocumentClassifierName=..., DataAccessRoleArn=..., InputDataConfig={'S3Uri': ...}, LanguageCode='en') — observe 501; consequently the classifier lifecycle and resource-policy checks time out.
5. Cause 4: comprehend.start_pii_entities_detection_job(Mode='ONLY_REDACTION', LanguageCode='en', DataAccessRoleArn=..., InputDataConfig=..., OutputDataConfig=...) — expect 400 InvalidRequestException, observe 501.
6. Replay: comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected, comprehend.batch-detect-entities-batch-size-limit-rejected, comprehend.describe-document-classifier-not-found, comprehend.document-classifier-lifecycle-training-to-stopped, comprehend.delete-resource-policy-without-policy-not-found, comprehend.start-pii-entities-detection-job-missing-redaction-config-rejected.
