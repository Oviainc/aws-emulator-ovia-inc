Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected|run#8|AssertionError|assert err == want, f"countNumber 0 must be refused; got {err!r}, expected {want!r}"
Issue-Commit: b3076e7c0c2ebb8fac474fe10cf99e18802cbe88

# ECR lifecycle-policy validation message drops the trailing newline AWS's validator emits inside the quoted failure text (1 cause)

> _Note: this issue was identified against the code base at commit `b3076e7c0c2ebb8fac474fe10cf99e18802cbe88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecyclePolicy

## Description
One root cause in ECR, found by the behavioral suite. Related open issues that do NOT cover this check (the replay report marked it owned by none of them): 20261023T153036Z-ecr-lifecycle-policy-text-is-reformatted-on-read.md (about the stored policy text being reformatted on read and lastEvaluatedAt being absent, not about the rejection path) and 20261024T080016Z-ecr-image-storage-class-transitions-scan-evaluation-media.md (its causes 4 and 6 are request-derived error-message templates for CreateRepository, ImageArchivedException and ScanNotFoundException — a different set of messages).

Cause 1 of 1: the InvalidParameterException message for a lifecycle-policy validation failure omits the newline that terminates the embedded validator text.

AWS: Smithy (com.amazonaws.ecr): PutLifecyclePolicy takes lifecyclePolicyText (@required, @length min 100 max 30720) and declares InvalidParameterException (client error, HTTP 400). The message is built from the request — it names the violated input member and quotes the lifecycle-policy validator's own failure text — so the rendering is deterministic rather than environment-specific. Recorded behavior: an imageCountMoreThan rule with countNumber 0 (one past the documented minimum of 1) is refused with HTTP 400 InvalidParameterException and the message "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: numeric instance is lower than the required minimum (minimum: 1, found: 0)\n'" — the embedded validator string ends with a literal newline INSIDE the closing quote, because the quoted span is the JSON-schema validator's raw output. The check records that newline as measured on real AWS.

Emulator: the status (400), the error code (InvalidParameterException), the member name ('LifecyclePolicyText'), the constraint sentence and the interpolated minimum/found values are all correct — the message is byte-identical except that the trailing "\n" before the closing quote is missing.

Why: the rejection itself is right, so this is the lowest-severity class of divergence; the cost is to clients that compare the message exactly or parse the quoted validator span as a delimited unit (deploy tooling that surfaces the validator text, test fixtures recorded against AWS). They see a string AWS never emits for an otherwise correctly-classified error.

Fix: the lifecycle-policy validation path's message construction in the PutLifecyclePolicy handler — the quoted span must reproduce the validator's raw output, newline terminator included, rather than a trimmed form of it. Note the same check also asserts the post-refusal read: GetLifecyclePolicy must still return the previously stored countNumber-10 rule unchanged; that leg was not reached because the message assertion fails first, so it should be re-checked alongside the fix (and it interacts with the read-path reformatting already filed as 20261023T153036Z).

Traces: ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected.

## Evidence
Cause 1 — AssertionError: countNumber 0 must be refused; got ('InvalidParameterException', "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: numeric instance is lower than the required minimum (minimum: 1, found: 0)'"), expected ('InvalidParameterException', "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: numeric instance is lower than the required minimum (minimum: 1, found: 0)\n'"). The error code and the HTTP 400 classification match; the only difference is the absent trailing newline inside the quoted validator text.

## Steps to Reproduce
Against the emulator's ECR endpoint:
1. ecr.create_repository(repositoryName='<name>').
2. ecr.put_lifecycle_policy(repositoryName='<name>', lifecyclePolicyText=json.dumps({"rules":[{"rulePriority":1,"description":"Keep only the last 10 images","selection":{"tagStatus":"any","countType":"imageCountMoreThan","countNumber":10},"action":{"type":"expire"}}]})) — this succeeds.
3. Repeat the same call with selection.countNumber set to 0 and capture the ClientError: Error.Code is InvalidParameterException (correct) and Error.Message is the text above WITHOUT the trailing newline inside the single quotes.
4. ecr.get_lifecycle_policy(repositoryName='<name>') must still return the countNumber-10 rule, unmodified by the refused put.
