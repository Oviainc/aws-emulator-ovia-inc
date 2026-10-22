Issue-Source: trace-sweep-divergence
Issue-Trace: migrationhubstrategy-assessment-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: migrationhubstrategy-assessment-error-paths migrationhubstrategy-assessment-lifecycle
Issue-Shape-Id: com.amazonaws.migrationhubstrategy#AWSMigrationHubStrategyRecommendation
Issue-Key: migrationhub-strategy|GetAssessment|4xx->4xx|$.message
Issue-Key: migrationhub-strategy|GetAssessment|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: migrationhub-strategy|GetAssessment|4xx->4xx|status
Issue-Key: migrationhub-strategy|StopAssessment|4xx->4xx|$.message
Issue-Key: migrationhub-strategy|StopAssessment|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: migrationhub-strategy|StopAssessment|4xx->4xx|status
Issue-Key: migrationhub-strategy|GetLatestAssessmentId|4xx->4xx|$.message
Issue-Key: migrationhub-strategy|GetLatestAssessmentId|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: migrationhub-strategy|GetLatestAssessmentId|4xx->4xx|status
Issue-Key: migrationhub-strategy|StartAssessment|4xx->2xx|$.message
Issue-Key: migrationhub-strategy|StartAssessment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: migrationhub-strategy|StartAssessment|4xx->2xx|status
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# Assessment error paths return wrong status and error code

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssessment

## Description
The MigrationHubStrategy emulator maps the assessment error paths onto the wrong HTTP status and the wrong modeled exception: ValidationException (400) and ResourceNotFoundException (404) are swapped on the read/stop paths, and StartAssessment performs no input validation at all. Four distinct causes, all in this service.

Cause 1 of 4 - GetAssessment with an assessment id that names no assessment.
AWS: recorded behavior only - the recorded response is HTTP 400 with x-amzn-errortype: ValidationException (message example: 'Looks like you provided invalid or missing parameters.'). Smithy (migrationhubstrategy model): GetAssessment input id is an AsyncTaskId @length(0,52) @pattern [0-9a-z-:]+ bound as an httpLabel on GET /get-assessment/{id}; ValidationException is httpError 400, ResourceNotFoundException is httpError 404. The probed id is pattern- and length-legal, and AWS still answers 400/ValidationException rather than a not-found.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException (message example: 'Assessment <id> not found').
Why: a client that distinguishes 'bad input, do not retry with this id' (400 ValidationException) from 'resource gone' (404 ResourceNotFoundException) takes the wrong branch - SDK error-type dispatch on ResourceNotFoundException fires where AWS raises ValidationException, and retry/paging wrappers that treat 404 as a benign empty result will swallow a request AWS rejected.
Fix: the GetAssessment handler's error mapping (which exception it raises for an unresolvable assessment id) in the migrationhubstrategy service crate.
Traces: migrationhubstrategy-assessment-error-paths#task#1.

Cause 2 of 4 - StopAssessment with an assessmentId that names no assessment.
AWS: recorded behavior only - the recorded response is HTTP 404 with x-amzn-errortype: ResourceNotFoundException (message example: 'Invalid resourceId: '). Smithy: StopAssessment is POST /stop-assessment, assessmentId required, AsyncTaskId @length(0,52) @pattern [0-9a-z-:]+; ResourceNotFoundException doc 'The specified ID in the request is not found.' (404).
Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException (message example: 'assessmentId <id> does not name an assessment').
Why: exactly the inverse of cause 1 - the emulator reports a well-formed-but-unknown id as a validation failure, so a client that retries or reconciles on ResourceNotFoundException never sees the not-found signal and instead surfaces a non-retryable 400 to the caller.
Fix: the StopAssessment handler's error mapping for an unresolvable assessmentId.
Traces: migrationhubstrategy-assessment-error-paths#task#2.

Cause 3 of 4 - GetLatestAssessmentId in an account where no assessment has ever been started.
AWS: recorded behavior only - the recorded response is HTTP 404 with x-amzn-errortype: ResourceNotFoundException (message example: 'Something went wrong, please try again later.'). Smithy: GetLatestAssessmentId is GET /get-latest-assessment-id with an empty input; ResourceNotFoundException is httpError 404.
Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException (message example: 'no assessment has been started in this account, so there is no latest assessment id').
Why: the operation takes no input, so a 400 ValidationException is not a reachable outcome on AWS - a client cannot have sent anything invalid. Callers that probe for 'has an assessment ever run here' by catching ResourceNotFoundException instead get a hard validation error, which SDK retry policies classify as a caller bug.
Fix: the GetLatestAssessmentId handler's empty-state branch (which exception it raises when no assessment exists).
Traces: migrationhubstrategy-assessment-lifecycle#task#1.

Cause 4 of 4 - StartAssessment without a valid s3bucketForAnalysisData is accepted instead of rejected.
AWS: the recorded response is HTTP 400 with x-amzn-errortype: ValidationException (message example: 'S3 bucket s3bucketForAnalysisData is required for perform assessment. Please specify s3 bucket with "migrationhub-strategy-" prefix'). Smithy documents the rule: StartAssessmentRequest.s3bucketForAnalysisData - 'The S3 bucket used by the collectors to send analysis data to the service. The bucket name must begin with migrationhub-strategy-', target @length(0,63) @pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+; ValidationException is httpError 400 ('The request body isn't valid.').
Emulator: answers HTTP 200 with a StartAssessment success body - no x-amzn-errortype header and no message at all - and starts an assessment.
Why: this is the most damaging of the four: the emulator accepts a request AWS refuses, so an assessment id is handed back for a workflow AWS would never have begun. Every downstream step in the fixture then exercises state that cannot exist on AWS, and a client's own validation of the migrationhub-strategy- bucket-prefix requirement is never tested.
Fix: StartAssessment input validation in the migrationhubstrategy service crate - require s3bucketForAnalysisData and enforce the documented 'migrationhub-strategy-' bucket-name prefix, raising ValidationException (400) when it is absent or non-conforming.
Traces: migrationhubstrategy-assessment-lifecycle#task#2 (cluster also covers migrationhubstrategy-assessment-lifecycle#task#3).

## Evidence
Replay of trace set simparity-wire-aws-v1 (23 steps, 0 passed, 23 mismatched):

1) GetAssessment - fixture migrationhubstrategy-assessment-error-paths step task#1: status expected 400, got 404; $headers.x-amzn-errortype expected 'ValidationException', got 'ResourceNotFoundException'; $.message expected 'Looks like you provided invalid or missing parameters.', got 'Assessment 00000000-0000-0000-0000-000000000000 not found'.

2) StopAssessment - fixture migrationhubstrategy-assessment-error-paths step task#2: status expected 404, got 400; $headers.x-amzn-errortype expected 'ResourceNotFoundException', got 'ValidationException'; $.message expected 'Invalid resourceId: ', got 'assessmentId 00000000-0000-0000-0000-000000000000 does not name an assessment'.

3) GetLatestAssessmentId - fixture migrationhubstrategy-assessment-lifecycle step task#1: status expected 404, got 400; $headers.x-amzn-errortype expected 'ResourceNotFoundException', got 'ValidationException'; $.message expected 'Something went wrong, please try again later.', got 'no assessment has been started in this account, so there is no latest assessment id'.

4) StartAssessment - fixture migrationhubstrategy-assessment-lifecycle step task#2 (cluster of 2, also task#3): status expected 400, got 200; $headers.x-amzn-errortype expected 'ValidationException', got None (header absent); $.message expected 'S3 bucket s3bucketForAnalysisData is required for perform assessment. Please specify s3 bucket with "migrationhub-strategy-" prefix', got None.

Model evidence (smithy_query on services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/smithy-model/com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation.json): ValidationException httpError 400, ResourceNotFoundException httpError 404; StartAssessmentRequest.s3bucketForAnalysisData documents the required 'migrationhub-strategy-' bucket prefix. Status codes and error codes above are requirements; the quoted messages are examples of the recorded text.

state_reset_failures = 0 and bootstrap_signal = false for this replay, so none of these are state-bleed artifacts. The MigrationHubStrategy bootstrap issue is in the resolved set, so the service is stood up and answering - it is answering wrongly.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. GET /get-assessment/00000000-0000-0000-0000-000000000000 (fixture migrationhubstrategy-assessment-error-paths, step task#1) - expect 400 / x-amzn-errortype ValidationException, observe 404 / ResourceNotFoundException.
4. POST /stop-assessment with assessmentId 00000000-0000-0000-0000-000000000000 (same fixture, step task#2) - expect 404 / ResourceNotFoundException, observe 400 / ValidationException.
5. Against a fresh emulator state, GET /get-latest-assessment-id with no assessment ever started (fixture migrationhubstrategy-assessment-lifecycle, step task#1) - expect 404 / ResourceNotFoundException, observe 400 / ValidationException.
6. POST /start-assessment without a valid s3bucketForAnalysisData (same fixture, step task#2) - expect 400 / ValidationException, observe 200 with an assessmentId.
