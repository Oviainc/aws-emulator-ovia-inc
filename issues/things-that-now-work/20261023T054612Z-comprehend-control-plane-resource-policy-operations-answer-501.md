Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehend.delete-resource-policy-without-policy-not-found comprehend.put-resource-policy-nonexistent-resource-rejected comprehend.describe-document-classifier-not-found comprehend.document-classifier-lifecycle-training-to-stopped comprehend.dominant-language-detection-job-completes-and-writes-output comprehend.start-key-phrases-detection-job-reaches-completed comprehend.start-pii-entities-detection-job-missing-redaction-config-rejected
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend.delete-resource-policy-without-policy-not-found|run#11|AssertionError|assert del_code == "ResourceNotFoundException", (
Issue-Key: comprehend.put-resource-policy-nonexistent-resource-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: comprehend.describe-document-classifier-not-found|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: comprehend.document-classifier-lifecycle-training-to-stopped|_create_classifier_until_training#4|AssertionError|raise AssertionError(
Issue-Key: comprehend.dominant-language-detection-job-completes-and-writes-output|run#8|AssertionError|assert "IN_PROGRESS" in seen_statuses, f"job never observed IN_PROGRESS, statuses seen: {seen_statuses!r}"
Issue-Key: comprehend.start-key-phrases-detection-job-reaches-completed|_run_job_to_completion#8|AssertionError|raise AssertionError(
Issue-Key: comprehend.start-pii-entities-detection-job-missing-redaction-config-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 1d15d4851343e3a31be69cd8cea36918aab666fe

# Comprehend control plane: resource-policy operations answer 501, classifier training and async detection jobs never run, and not-found/validation messages diverge (6 causes)

> _Note: this issue was identified against the code base at commit `1d15d4851343e3a31be69cd8cea36918aab666fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
Six distinct root causes in the Comprehend control plane (resource policies, custom-model training, async detection jobs). comprehend is bootstrapped and answering these calls, so each is a fidelity defect. None of these six checks is marked ALREADY FILED or OWNED in the replay report, and no open issue names the Comprehend resource-policy, classifier-training or detection-job surfaces (the open comprehend issues cover only the BatchDetectKeyPhrases / BatchDetectSyntax / ContainsPiiEntities / DetectEntities / DetectKeyPhrases / DetectPiiEntities / DetectToxicContent stubs).

Cause 1 of 6: DeleteResourcePolicy and PutResourcePolicy answer HTTP 501 NotImplemented instead of their modeled behavior.
AWS: Smithy models DeleteResourcePolicy (input ResourceArn required, ComprehendModelArn pattern document-classifier|entity-recognizer; errors InternalServerException 500, InvalidRequestException 400, ResourceNotFoundException 404; empty 200 output) and PutResourcePolicy (input ResourceArn + ResourcePolicy required; output PolicyRevisionId; same error set). Recorded behavior: against a ResourceArn that was never created BOTH operations answer 404 ResourceNotFoundException with message "RESOURCE_NOT_FOUND: Could not find specified resource."; against an existing custom model with no policy attached DeleteResourcePolicy is a plain 200 no-op and DescribeResourcePolicy returns 200 with no ResourcePolicy key; PutResourcePolicy on a real model attaches a policy that round-trips through DescribeResourcePolicy and disappears after DeleteResourcePolicy.
Emulator: both calls return error code NotImplemented (the handler is the untouched scaffold — services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/delete_resource_policy.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED ... answers HTTP 501 NotImplemented" marker and `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteResourcePolicy"))` at the end of `handle`; put_resource_policy.rs behaves the same way).
Why: the whole model-sharing / cross-account ImportModel flow is unusable — a client cannot attach, read back or remove a model resource policy, and a 501 is neither a modeled error nor retryable, so SDK error handling for ResourceNotFoundException never engages.
Fix: implement the two handlers against the service's policy state (attach/replace with a PolicyRevisionId, delete as an idempotent no-op, 404 ResourceNotFoundException when the model ARN does not exist) in .../src/ops/put_resource_policy.rs and .../src/ops/delete_resource_policy.rs.
Traces: comprehend.delete-resource-policy-without-policy-not-found, comprehend.put-resource-policy-nonexistent-resource-rejected.

Cause 2 of 6: the ResourceNotFoundException message echoes the requested ARN instead of AWS's fixed text.
AWS: DescribeDocumentClassifier models ResourceNotFoundException (httpError 404). Recorded behavior: for a syntactically valid but nonexistent DocumentClassifierArn the message is the CONSTANT "RESOURCE_NOT_FOUND: Could not find specified resource." — it does not name the resource (the same constant is recorded for the resource-policy operations in cause 1). The rule is "constant text, no request-derived substitution".
Emulator: the error code is correct, but the message is built from the request and names the ARN, e.g. "RESOURCE_NOT_FOUND: Could not find the document classifier arn:aws:comprehend:<region>:<account>:document-classifier/<name>."
Why: besides breaking callers that match the service's fixed message, echoing the caller-supplied ARN is a different disclosure contract than AWS's deliberately opaque not-found text.
Fix: raise ResourceNotFoundException with the service's fixed message from the shared not-found path used by the classifier/recognizer describe handlers (.../src/ops/describe_document_classifier.rs).
Traces: comprehend.describe-document-classifier-not-found.

Cause 3 of 6: CreateDocumentClassifier never trains — the classifier goes SUBMITTED -> IN_ERROR.
AWS: Smithy CreateDocumentClassifier takes InputDataConfig (S3Uri) + DataAccessRoleArn and returns a DocumentClassifierArn; recorded behavior is the documented lifecycle SUBMITTED -> TRAINING -> TRAINED (or TRAINED_WITH_WARNING), with StopTrainingDocumentClassifier moving a TRAINING classifier to STOPPED, and the classifier remaining describable and taggable throughout.
Emulator: the classifier is created but DescribeDocumentClassifier reports statuses ['SUBMITTED', 'IN_ERROR'] within seconds and never enters TRAINING, with Message "Could not read the training documents from s3://<bucket>/train.csv: this emulator serves no object storage and runs no model training, so no document was read and no classifier was trained." The training CSV had just been written successfully through this same emulator's S3 surface (s3 is bootstrapped and served the create_bucket/put_object calls in the same check), so the stated reason does not hold.
Why: every custom-model scenario is blocked — a client polling for TRAINING/TRAINED sees a terminal IN_ERROR, the SUBMITTED->TRAINING->STOPPED stop path can never be exercised, and anything downstream of a trained classifier (versions, endpoints, resource policies, ImportModel) is unreachable.
Fix: drive the classifier state machine through TRAINING to a terminal TRAINED state, reading the training input through the emulator's own S3 state rather than declaring object storage absent (.../src/ops/create_document_classifier.rs plus the classifier lifecycle in the service state).
Traces: comprehend.document-classifier-lifecycle-training-to-stopped.

Cause 4 of 6: async detection jobs skip IN_PROGRESS.
AWS: StartDominantLanguageDetectionJob returns a JobId/JobArn/JobStatus and the job walks the documented JobStatus state machine SUBMITTED -> IN_PROGRESS -> COMPLETED (Smithy: JobStatus enum on the Start*/Describe* job operations).
Emulator: polling DescribeDominantLanguageDetectionJob observes only ['SUBMITTED', 'COMPLETED'] — the job flips straight to COMPLETED and IN_PROGRESS is never observable.
Why: clients that wait for, log, or branch on IN_PROGRESS (progress reporting, cancel-while-running paths, state-machine assertions) never see the state AWS guarantees, and a job that completes instantaneously hides any ordering bug in code that assumes a running phase.
Fix: make the async job state machine pass through IN_PROGRESS for an observable interval before COMPLETED, in the shared detection-job lifecycle behind the Start*/Describe* dominant-language job handlers (.../src/ops/start_dominant_language_detection_job.rs and the job state).
Traces: comprehend.dominant-language-detection-job-completes-and-writes-output.

Cause 5 of 6: StartKeyPhrasesDetectionJob ends FAILED and writes no output.
AWS: the job is accepted and walks SUBMITTED -> IN_PROGRESS -> COMPLETED, writing a real result object under the configured OutputDataConfig S3Uri whose KeyPhrases name terms from the input document (Smithy: StartKeyPhrasesDetectionJob requires InputDataConfig/OutputDataConfig/DataAccessRoleArn/LanguageCode and returns JobId/JobArn/JobStatus).
Emulator: the job reaches terminal FAILED on the first poll with Message "Key-phrase extraction of the documents at s3://<bucket>/input/doc.txt was not performed by this emulator, so no document was analyzed and no output was written to s3://<bucket>/output/." — the input object had just been written through this emulator's own S3 surface.
Why: no async key-phrase pipeline can be emulated end to end; clients that poll to COMPLETED and then read the output archive get a terminal failure, and consumers of the output location find nothing there.
Fix: run the job against the emulator's S3 state and write a result object at the configured output prefix, completing the job instead of failing it (.../src/ops/start_key_phrases_detection_job.rs and the detection-job lifecycle).
Traces: comprehend.start-key-phrases-detection-job-reaches-completed.

Cause 6 of 6: the ONLY_REDACTION / missing-RedactionConfig refusal message diverges from AWS's text.
AWS: StartPiiEntitiesDetectionJob models Mode (required) and an optional RedactionConfig, with InvalidRequestException (httpError 400). Recorded behavior: Mode='ONLY_REDACTION' without RedactionConfig is refused with InvalidRequestException and the documented, request-independent text "INPUT_CONFIG_INVALID_FOR_MODE: A RedactionConfig definition is required for PII detection requests that set the Mode parameter to ONLY_REDACTION. Add a RedactionConfig definition to your request, including a non-empty list of PII entity types to redact, and try again."
Emulator: the status and error code are correct, but the message is a shortened rewrite — "INPUT_CONFIG_INVALID_FOR_MODE: A RedactionConfig definition that includes the PiiEntityTypes parameter is required for PII detection requests that set the Mode parameter to ONLY_REDACTION." — dropping the remediation clause AWS always appends for this reason code.
Why: callers that surface or match the service's remediation text for INPUT_CONFIG_INVALID_FOR_MODE lose it; the reason-code prefix alone no longer identifies the full documented condition.
Fix: emit the service's full fixed message for this precondition in the StartPiiEntitiesDetectionJob validation (.../src/ops/start_pii_entities_detection_job.rs).
Traces: comprehend.start-pii-entities-detection-job-missing-redaction-config-rejected.

## Evidence
Cause 1 — DeleteResourcePolicy on a never-created classifier ARN: expected error code 'ResourceNotFoundException' (404, message "RESOURCE_NOT_FOUND: Could not find specified resource."), actual 'NotImplemented'. PutResourcePolicy on a nonexistent ResourceArn: expected 'ResourceNotFoundException', actual 'NotImplemented'. Handler source confirms the scaffold: services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/delete_resource_policy.rs lines 4-6 ("SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented") and line 156 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteResourcePolicy"))`).

Cause 2 — DescribeDocumentClassifier on a nonexistent ARN: code ResourceNotFoundException matched; expected message 'RESOURCE_NOT_FOUND: Could not find specified resource.', actual 'RESOURCE_NOT_FOUND: Could not find the document classifier arn:aws:comprehend:us-west-2:000000000000:document-classifier/sim2real-nonexistent-classifier-<id>.'

Cause 3 — expected to observe TRAINING; actual statuses seen ['SUBMITTED', 'IN_ERROR'] with Message "Could not read the training documents from s3://sim2real-comprehend-<id>/train.csv: this emulator serves no object storage and runs no model training, so no document was read and no classifier was trained." (the same check had already created the bucket and put train.csv through this emulator's S3).

Cause 4 — expected 'IN_PROGRESS' among the polled DescribeDominantLanguageDetectionJob statuses; actual seen statuses ['SUBMITTED', 'COMPLETED'].

Cause 5 — expected JobStatus COMPLETED with output written under the OutputDataConfig prefix; actual "job ended in FAILED after 1 attempt(s): Key-phrase extraction of the documents at s3://sim2real-comprehend-<id>/input/doc.txt was not performed by this emulator, so no document was analyzed and no output was written to s3://sim2real-comprehend-<id>/output/."

Cause 6 — code InvalidRequestException matched; expected message 'INPUT_CONFIG_INVALID_FOR_MODE: A RedactionConfig definition is required for PII detection requests that set the Mode parameter to ONLY_REDACTION. Add a RedactionConfig definition to your request, including a non-empty list of PII entity types to redact, and try again.', actual 'INPUT_CONFIG_INVALID_FOR_MODE: A RedactionConfig definition that includes the PiiEntityTypes parameter is required for PII detection requests that set the Mode parameter to ONLY_REDACTION.'

## Steps to Reproduce
1. Boot the emulator; create an S3 bucket and an IAM role through it (both services are bootstrapped) and upload a training CSV / input document.
2. Cause 1: comprehend.delete_resource_policy(ResourceArn=<well-formed classifier ARN that was never created>) and comprehend.put_resource_policy(ResourceArn=<same>, ResourcePolicy=json.dumps(policy)) — expect 404 ResourceNotFoundException; observe NotImplemented. Also call delete_resource_policy / describe_resource_policy on an existing policy-less classifier (AWS: 200 no-op / 200 with no ResourcePolicy key).
3. Cause 2: comprehend.describe_document_classifier(DocumentClassifierArn=<nonexistent but well-formed ARN>) and compare Error.Message to the fixed AWS text.
4. Cause 3: comprehend.create_document_classifier(DocumentClassifierName=..., DataAccessRoleArn=<role>, LanguageCode='en', InputDataConfig={'S3Uri': 's3://<bucket>/train.csv'}) then poll describe_document_classifier — observe SUBMITTED -> IN_ERROR instead of SUBMITTED -> TRAINING.
5. Cause 4: comprehend.start_dominant_language_detection_job(...) then poll describe_dominant_language_detection_job and record every JobStatus seen — IN_PROGRESS never appears.
6. Cause 5: comprehend.start_key_phrases_detection_job(InputDataConfig={'S3Uri':'s3://<bucket>/input/doc.txt'}, OutputDataConfig={'S3Uri':'s3://<bucket>/output/'}, LanguageCode='en', DataAccessRoleArn=<role>) then poll — job ends FAILED and nothing is written under output/.
7. Cause 6: comprehend.start_pii_entities_detection_job(Mode='ONLY_REDACTION', LanguageCode='en', DataAccessRoleArn=<role>, InputDataConfig=..., OutputDataConfig=...) with no RedactionConfig and compare Error.Message.
8. Re-replay the checks listed in trace_ids to confirm.
