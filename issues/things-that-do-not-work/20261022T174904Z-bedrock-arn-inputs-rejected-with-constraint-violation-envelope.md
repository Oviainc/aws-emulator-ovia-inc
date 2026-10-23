Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-custom-model-deployment-read-and-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-custom-model-deployment-read-and-errors bedrock-model-copy-job-read-and-errors bedrock-model-invocation-jobs bedrock-provisioned-model-throughput-lifecycle
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|CreateCustomModelDeployment|4xx->4xx|$.message
Issue-Key: bedrock|GetModelCopyJob|4xx->4xx|$.message
Issue-Key: bedrock|CreateModelCopyJob|4xx->4xx|$.message
Issue-Key: bedrock|GetModelInvocationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopModelInvocationJob|4xx->4xx|$.message
Issue-Key: bedrock|GetProvisionedModelThroughput|4xx->4xx|$.message
Issue-Key: bedrock|UpdateProvisionedModelThroughput|4xx->4xx|$.message
Issue-Key: bedrock|DeleteProvisionedModelThroughput|4xx->4xx|$.message
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Bedrock ARN inputs rejected with constraint-violation envelope instead of the service ARN message

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomModelDeployment

## Description
Eight causes, one fault: for members whose value is an ARN, bedrock does NOT answer with a Smithy constraint-violation message. It performs its own ARN check and answers 400 ValidationException with a short service message ('The provided ARN is invalid for the service region', or 'Invalid custom model ARN format' for a source model ARN). The emulator instead runs the member's @pattern and answers 400 with the generic '1 validation error detected: Value '<arn>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: ...' envelope. Status (400) and error code (ValidationException) agree; the message ENVELOPE is the divergence, and it is a requirement here because the two envelopes are produced by different rules: a constraint violation reports the member and the model constraint, whereas AWS's reply reports an ARN that is syntactically acceptable to the service but not usable in this region. A client that branches on the message shape - or on the presence of 'validation error detected' - takes the wrong branch, and one that reads AWS's message learns the real reason (wrong region / wrong ARN format) that the emulator never reports.
Hypothesis: the recorded probes all submit an ARN whose region segment (us-east-1) differs from the endpoint region, which is what AWS's 'invalid for the service region' wording refers to; the model states no such check, so this is the recorded behavior plus that inference, not a modeled rule.
Fix: the ARN-bearing input handling for these operations (an ARN/region check that runs before, and in place of, generic @pattern validation), plus the ValidationException message produced there.

Cause 1 of 8: CreateCustomModelDeployment, modelArn.
AWS: recorded behavior only - 400 ValidationException, message 'The provided ARN is invalid for the service region'. Smithy: CreateCustomModelDeployment models ValidationException at httpError 400.
Emulator: 400 with '1 validation error detected: Value 'arn:aws:bedrock:us-east-1:000000000000:custom-model/nonexistent-model-00000' at 'modelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:...$'.
Traces: bedrock-custom-model-deployment-read-and-errors#task#3.

Cause 2 of 8: GetModelCopyJob, jobArn. AWS: 400 ValidationException 'The provided ARN is invalid for the service region'. Emulator: constraint-violation envelope quoting the model-copy-job pattern.
Traces: bedrock-model-copy-job-read-and-errors#task#2.

Cause 3 of 8: CreateModelCopyJob, sourceModelArn. AWS: 400 ValidationException 'Invalid custom model ARN format' - a distinct service message for this member, not the region one. Emulator: constraint-violation envelope quoting the custom-model/foundation-model pattern.
Traces: bedrock-model-copy-job-read-and-errors#task#3.

Cause 4 of 8: GetModelInvocationJob, jobIdentifier. AWS: 400 'The provided ARN is invalid for the service region'. Emulator: constraint-violation envelope.
Traces: bedrock-model-invocation-jobs#task#2.

Cause 5 of 8: StopModelInvocationJob, jobIdentifier. Same divergence.
Traces: bedrock-model-invocation-jobs#task#3.

Cause 6 of 8: GetProvisionedModelThroughput, provisionedModelId. AWS: 400 'The provided ARN is invalid for the service region'. Emulator: constraint-violation envelope quoting the provisioned-model pattern.
Traces: bedrock-provisioned-model-throughput-lifecycle#task#3.

Cause 7 of 8: UpdateProvisionedModelThroughput, provisionedModelId. Same divergence.
Traces: bedrock-provisioned-model-throughput-lifecycle#task#4.

Cause 8 of 8: DeleteProvisionedModelThroughput, provisionedModelId. Same divergence.
Traces: bedrock-provisioned-model-throughput-lifecycle#task#5.

## Evidence
Replay of simparity-wire-aws-v1; every cause below is status-matched (400 expected, 400 actual) with $.message diverging.

Cause 1 (bedrock-custom-model-deployment-read-and-errors#task#3, CreateCustomModelDeployment): expected $.message 'The provided ARN is invalid for the service region'; actual "1 validation error detected: Value 'arn:aws:bedrock:us-east-1:000000000000:custom-model/nonexistent-model-00000' at 'modelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|...)/[a-z0-9]{12}$".

Cause 2 (bedrock-model-copy-job-read-and-errors#task#2, GetModelCopyJob): expected 'The provided ARN is invalid for the service region'; actual "...Value 'arn:aws:bedrock:us-east-1:000000000000:model-copy-job/nonexistent-job-00000' at 'jobArn' ... pattern: ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-copy-job/[a-z0-9]{12}$".

Cause 3 (bedrock-model-copy-job-read-and-errors#task#3, CreateModelCopyJob): expected 'Invalid custom model ARN format'; actual the constraint-violation envelope for 'sourceModelArn'.

Causes 4/5 (bedrock-model-invocation-jobs#task#2, #task#3): expected 'The provided ARN is invalid for the service region'; actual "...Value 'arn:aws:bedrock:us-east-1:000000000000:model-invocation-job/sim2real-probe-bedrock-nonexistent-inv-job-42cb3346' at 'jobIdentifier' ... pattern: ^((arn:aws...model-invocation-job/)?[a-z0-9]{12})$".

Causes 6/7/8 (bedrock-provisioned-model-throughput-lifecycle#task#3, #task#4, #task#5): expected 'The provided ARN is invalid for the service region'; actual "...Value 'arn:aws:bedrock:us-east-1:000000000000:provisioned-model/nonexistent' at 'provisionedModelId' ... pattern: ^((([0-9a-zA-Z][_-]?)+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:provisioned-model/[a-z0-9]{12}))$".

Model evidence: smithy_query over the bedrock control-plane model shows these members carry anchored @pattern traits and the operations model ValidationException at httpError 400; nothing in the model describes a per-region ARN check, so the short service messages above are taken from the recorded responses.

## Steps to Reproduce
1. Boot the emulator built from mainline (region us-west-2 endpoint).
2. Issue each request with an ARN from another region:
   a. POST /model-customization-deployments with modelArn=arn:aws:bedrock:us-east-1:000000000000:custom-model/nonexistent-model-00000
   b. GET /model-copy-jobs/arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Amodel-copy-job%2Fnonexistent-job-00000
   c. POST /model-copy-jobs with sourceModelArn=arn:aws:bedrock:us-west-2:000000000000:custom-model/nonexistent-model-00000
   d. GET /model-invocation-jobs/<us-east-1 model-invocation-job ARN> and POST .../stop
   e. GET, PATCH and DELETE /provisioned-model-throughput/<us-east-1 provisioned-model ARN>
3. Each must answer 400 ValidationException whose message is the short service ARN message ('The provided ARN is invalid for the service region'; 'Invalid custom model ARN format' for sourceModelArn), not a '1 validation error detected: ...' constraint envelope.
4. Or simply replay the corpus and inspect those five fixtures.
