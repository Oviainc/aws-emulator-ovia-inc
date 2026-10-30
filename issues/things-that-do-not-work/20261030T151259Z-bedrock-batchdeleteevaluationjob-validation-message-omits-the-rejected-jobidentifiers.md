Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-evaluation-job-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-evaluation-job-lifecycle
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|BatchDeleteEvaluationJob|4xx->4xx|$.message
Issue-Commit: 3fb07f4222b77f896393e405672ee256545dfd3b

# bedrock BatchDeleteEvaluationJob validation message omits the rejected jobIdentifiers value

> _Note: this issue was identified against the code base at commit `3fb07f4222b77f896393e405672ee256545dfd3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteEvaluationJob

## Description
ONE cause: the emulator's ValidationException message builder drops the rejected value from the aggregate list-member constraint message for BatchDeleteEvaluationJob, where AWS quotes it.

Cause 1 of 1: BatchDeleteEvaluationJob called with a jobIdentifiers list whose member violates the modelled pattern.

AWS: 400 ValidationException with message "N validation error detected: Value '[<the rejected jobIdentifiers list, rendered>]' at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: (arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})]". Fixed text of the requirement: the literal " validation error detected: Value ", the rejected value in single quotes rendered as a bracketed list, " at ", the member name in single quotes, " failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to ", ", Member must have length greater than or equal to ", ", Member must satisfy regular expression pattern: " and the closing "]". The variable parts are the error count, the rejected value taken from the request, the member name, and the length bounds and regular expression taken from the model. Smithy: BatchDeleteEvaluationJob input jobIdentifiers (required, list @length min 1 max 25) targets EvaluationJobIdentifiers, whose member EvaluationJobIdentifier carries @length max 1011 and @pattern ^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$ and is @sensitive; errors include ValidationException (httpError 400). The model does not state the message text, so the wording above is recorded behavior only — the recording shows AWS quoting the value for this member despite the @sensitive trait, and quoting the length bounds as 'less than or equal to 1011' and 'greater than or equal to 0'.

Emulator: 400 ValidationException (status and error code both correct), but the message is emitted with the value elided — "1 validation error detected: Value at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: (arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})]". The "Value '<...>' at" segment is replaced by a bare "Value at".

Why: a client or test suite that asserts on AWS's validation text never matches, and — more practically — a caller batching up to 25 identifiers cannot tell from the emulator's message WHICH identifier it sent was rejected, because the message no longer carries the offending value. On AWS the echoed value is the only part of the response that localises the fault inside the batch; the emulator's message is identical for every malformed input.

Fix: the shared ValidationException message formatter in the bedrock control-plane input-validation layer, specifically its list-member/aggregate-constraint branch — the branch has been made unconditionally value-eliding, but eliding is correct only for the members AWS does not echo (jobIdentifier, testCaseId and the advanced-prompt-optimization identifiers, per the related issue below) and is wrong for EvaluationJobIdentifiers. The elide decision must be per-member, not global.

Traces: bedrock-evaluation-job-lifecycle#task#5.

Related open issue, which does NOT cover this step (it carries no replay key for bedrock-evaluation-job-lifecycle#task#5 and reports the OPPOSITE direction — the emulator echoing a value AWS omits): 20261029T211357Z-bedrock-validationexception-messages-echo-the-rejected-value-instead.md. Its cause 3 covers BatchDeleteAdvancedPromptOptimizationJob, a different operation and a different member. Fixing both requires the per-member decision described under Fix; a fix that merely flips the formatter one way or the other re-breaks the other issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

bedrock-evaluation-job-lifecycle#task#5 BatchDeleteEvaluationJob: expected status 400, actual status 400 (match); x-amzn-errortype matched; only $.message diverged.

  expected $.message: "1 validation error detected: Value '[arn:aws:bedrock:us-east-1:000000000000:evaluation-job/<name>]' at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: (arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})]"

  actual   $.message: "1 validation error detected: Value at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: (arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})]"

The divergence is exactly the missing "'[<value>]' " segment after "Value ". Verdict: mismatch; divergence key service=bedrock|BatchDeleteEvaluationJob|status=400->400|keys=$.message; owned_by: none.

The specific ARN, region, account and job name in the recording are request-supplied values that fill the template, not part of the requirement; the fixed text and the bracketed-list rendering of the value are.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture bedrock-evaluation-job-lifecycle, step task#5.
3. Inspect $.message of the 400 response: it begins "1 validation error detected: Value at 'jobIdentifiers' failed to satisfy constraint: ..." where AWS emits "1 validation error detected: Value '[<the identifier(s) sent>]' at 'jobIdentifiers' failed to satisfy constraint: ...".
4. Equivalently: call BatchDeleteEvaluationJob with jobIdentifiers containing any string that does not match ^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$ (for example an ARN whose job segment is not 12 chars of [a-z0-9]). Status 400 and errortype ValidationException are correct; only the message's value segment is missing.
