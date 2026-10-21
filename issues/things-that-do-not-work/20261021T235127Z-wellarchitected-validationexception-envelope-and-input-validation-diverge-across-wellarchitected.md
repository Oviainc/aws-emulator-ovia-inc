Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-workload-create-validation-envelopes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-workload-create-validation-envelopes wellarchitected-lens-review-answer-lifecycle wellarchitected-milestone-lifecycle wellarchitected-review-template-lifecycle wellarchitected-pagination-and-limit-validation
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|CreateWorkload|4xx->4xx|$.Message
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$.Message
Issue-Key: wellarchitected|CreateMilestone|4xx->4xx|$.Message
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->4xx|$.Message
Issue-Key: wellarchitected|CreateWorkload|4xx->4xx|$.Reason
Issue-Key: wellarchitected|CreateWorkload|4xx->2xx|$.Fields
Issue-Key: wellarchitected|CreateWorkload|4xx->2xx|$.Message
Issue-Key: wellarchitected|CreateWorkload|4xx->2xx|$.Reason
Issue-Key: wellarchitected|CreateWorkload|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|CreateWorkload|4xx->2xx|status
Issue-Key: wellarchitected|CreateWorkload|4xx->4xx|$.Fields[*]
Issue-Key: wellarchitected|CreateWorkload|4xx->4xx|$.Fields[*].Message
Issue-Key: wellarchitected|ListWorkloads|4xx->4xx|$.Fields[*].Message
Issue-Key: wellarchitected|ListWorkloads|4xx->4xx|$.Message
Issue-Key: wellarchitected|ListWorkloads|4xx->4xx|$.Reason
Issue-Key: wellarchitected|ListWorkloads|4xx->4xx|$.Fields
Issue-Key: wellarchitected|ListMilestones|4xx->4xx|$.Fields
Issue-Key: wellarchitected|ListMilestones|4xx->4xx|$.Message
Issue-Key: wellarchitected|ListMilestones|4xx->4xx|$.Reason
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# ValidationException envelope and input validation diverge across wellarchitected operations

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkload

## Description
The emulator's ValidationException path for wellarchitected is incomplete: on most constraint failures it returns a 400 whose JSON body omits the modelled members entirely (Message, Reason, Fields all absent), on others it fills Reason/Fields with values AWS does not use, and in one case it does not validate at all and returns 200. Smithy: ValidationException is @error("client") @httpError(400) with members Message (required, ExceptionMessage), Reason (enum ValidationExceptionReason: UNKNOWN_OPERATION | CANNOT_PARSE | FIELD_VALIDATION_FAILED | OTHER) and Fields (list of ValidationExceptionField{Name required, Message required}). Because Message is a REQUIRED member of the error shape, a body without it is not a valid ValidationException for any of these operations. 10 causes, all in this service.

Cause 1 of 10 — UpdateAnswer, invalid Reason enum value.
AWS: Smithy UpdateAnswer.Reason targets enum AnswerReason with values OUT_OF_SCOPE, BUSINESS_PRIORITIES, ARCHITECTURE_CONSTRAINTS, OTHER, NONE; an out-of-enum value is a 400 ValidationException carrying Message. Recorded response: 400 with Message describing the rejected value against the enum (example: '[Validation] [instance value ("SIM2REAL_PROBE") not found in enum (possible values: ["OUT_OF_SCOPE","BUSINESS_PRIORITIES","ARCHITECTURE_CONSTRAINTS","OTHER","NONE"])]').
Emulator: status 400 is right, but $.Message is absent (null) — the error body carries no message at all.
Why: SDK clients surface err.Message / error_message for a validation failure; an absent required member makes the failure unexplainable and can fail strict response deserialization.
Fix: UpdateAnswer's validation / ValidationException serializer — always populate the required Message member.
Traces: wellarchitected-lens-review-answer-lifecycle#task#10.

Cause 2 of 10 — CreateMilestone, MilestoneName below minimum length.
AWS: Smithy CreateMilestone.MilestoneName @length(min 3, max 100); a 2-character name is a 400 ValidationException whose Message is built from the request and names the offending value and the constraint (example: '[Validation] [string "ab" is too short (length: 2, required minimum: 3)]').
Emulator: 400 with $.Message absent.
Why: same as cause 1 — the required Message member is missing, so the caller cannot tell which constraint it violated.
Fix: CreateMilestone validation / ValidationException serializer.
Traces: wellarchitected-milestone-lifecycle#task#7.

Cause 3 of 10 — CreateWorkload, string members below minimum length (representative cause).
AWS: Smithy CreateWorkload.WorkloadName @length(min 3, max 100), Description @length(min 3, max 250), ReviewOwner @length(min 3, max 255); each violation is a 400 ValidationException with Message naming the value and the constraint (example: '[Validation] [string "ab" is too short (length: 2, required minimum: 3)]').
Emulator: 400 with $.Message absent, on all four recorded length cases of this cluster.
Fix: CreateWorkload validation / ValidationException serializer.
Traces: wellarchitected-workload-create-validation-envelopes#task#1 plus steps task#2, task#4, task#5 of the same fixture.

Cause 4 of 10 — CreateReviewTemplate, TemplateName pattern violation.
AWS: Smithy CreateReviewTemplate.TemplateName @pattern("^[A-Za-z0-9-_.,:/()@!&?#+'’\\s]+$"); an input containing '*' is a 400 ValidationException with Message naming the regex and the rejected string (example: '[Validation] [ECMA 262 regex ... does not match input string "sim2real*probe*tpl"]').
Emulator: 400 with $.Message absent.
Fix: CreateReviewTemplate validation / ValidationException serializer.
Traces: wellarchitected-review-template-lifecycle#task#5.

Cause 5 of 10 — CreateWorkload, missing required member reported as the wrong failure.
AWS: recorded behavior shows a request lacking ReviewOwner is rejected as a missing-required-property validation failure with Reason EMPTY in the body (Message example: '[Validation] [object has missing required properties (["ReviewOwner"])]'); the request is not reported as a region problem.
Emulator: 400 but blames a different member — Message '1 validation error detected: Value at AwsRegions failed to satisfy constraint: Either AwsRegions, NonAwsRegions, or both must be specified' and Reason 'FIELD_VALIDATION_FAILED'.
Why: the client is told to fix AwsRegions when the actual problem is the absent ReviewOwner, so retry logic driven off Reason/Fields corrects the wrong member and never succeeds. The modelled Reason value is a requirement, not message text.
Fix: CreateWorkload validation — evaluate required-member presence before the regions cross-member rule, and set Reason accordingly.
Traces: wellarchitected-workload-create-validation-envelopes#task#3.

Cause 6 of 10 — CreateWorkload, regions cross-member rule reports only one Fields entry.
AWS: when neither AwsRegions nor NonAwsRegions is supplied, the recorded 400 ValidationException carries TWO ValidationExceptionField entries, one named AwsRegions and one named NonAwsRegions, each with the same message (example 'Either AwsRegions or NonAwsRegions is required.'), and Message repeats it.
Emulator: emits a single Fields entry (AwsRegions only); $.Fields[1] is absent.
Why: a client that walks Fields to highlight every offending member highlights only one of the two, and the modelled Fields list is therefore structurally wrong (one element instead of two).
Fix: CreateWorkload validation — the cross-member regions check must report both member names in Fields.
Traces: wellarchitected-workload-create-validation-envelopes#task#7.

Cause 7 of 10 — ListWorkloads, invalid NextToken classified with the wrong Reason.
AWS: Smithy ListWorkloads.NextToken @pattern("^[A-Za-z0-9+\\/=_-]+$") and the op is @paginated; an undecodable token is a 400 ValidationException with Reason 'CANNOT_PARSE' (a modelled ValidationExceptionReason value), message example '[Validation] The next token that was passed in is invalid.'.
Emulator: 400 with Reason 'FIELD_VALIDATION_FAILED' and a field-constraint style message.
Why: Reason is a modelled enum a client branches on — CANNOT_PARSE means 'restart pagination', FIELD_VALIDATION_FAILED means 'fix the member'; the emulator sends callers down the wrong branch.
Fix: ListWorkloads pagination-token handling — classify token decode failures as CANNOT_PARSE.
Traces: wellarchitected-pagination-and-limit-validation#task#3.

Cause 8 of 10 — ListWorkloads, MaxResults out of range returns an empty error body.
AWS: Smithy ListWorkloads.MaxResults @range(min 1, max 50); an out-of-range value is a 400 ValidationException with Reason 'FIELD_VALIDATION_FAILED', a Fields entry named MaxResults, and Message (example 'MaxResults must be between 1 and 50.').
Emulator: status 400 but $.Fields, $.Message and $.Reason are all absent.
Why: the response is a bare 400 with no modelled content — the required Message is missing and nothing identifies the member or the range.
Fix: ListWorkloads validation / ValidationException serializer.
Traces: wellarchitected-pagination-and-limit-validation#task#4.

Cause 9 of 10 — ListMilestones, MaxResults out of range returns an empty error body.
AWS: Smithy ListMilestones.MaxResults @range(min 1, max 50); same envelope as cause 8 (Reason FIELD_VALIDATION_FAILED, Fields[MaxResults], Message).
Emulator: 400 with $.Fields, $.Message and $.Reason all absent.
Fix: ListMilestones validation / ValidationException serializer (shared range-check helper with cause 8).
Traces: wellarchitected-pagination-and-limit-validation#task#5.

Cause 10 of 10 — CreateWorkload accepts an unknown AWS region and succeeds.
AWS: recorded behavior only — a value in AwsRegions that is not a real region is rejected with 400 ValidationException, Reason 'FIELD_VALIDATION_FAILED' and a Fields entry named AwsRegions (message example 'Unknown region sim2real-probe-region.'); x-amzn-errortype is set.
Emulator: returns 200 and creates the workload; no error body at all.
Why: this is the most severe cause in the bundle — a request AWS refuses is accepted, so state the real service would never hold is created and downstream steps diverge silently rather than failing fast.
Fix: CreateWorkload validation — validate AwsRegions members against the known region set before mutating state.
Traces: wellarchitected-workload-create-validation-envelopes#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 12 fixtures replayed, no state-reset failures).

Cause 1 wellarchitected-lens-review-answer-lifecycle#task#10 UpdateAnswer: expected 400 / actual 400 — $.Message: missing (expected '[Validation] [instance value ("SIM2REAL_PROBE") not found in enum (possible values: ["OUT_OF_SCOPE","BUSINESS_PRIORITIES","ARCHITECTURE_CONSTRAINTS","OTHER","NONE"])]', got None).
Cause 2 wellarchitected-milestone-lifecycle#task#7 CreateMilestone: 400/400 — $.Message: missing (expected '[Validation] [string "ab" is too short (length: 2, required minimum: 3)]', got None).
Cause 3 wellarchitected-workload-create-validation-envelopes#task#1 CreateWorkload (cluster of 4: task#1, task#2, task#4, task#5): 400/400 — $.Message: missing (expected '[Validation] [string "ab" is too short (length: 2, required minimum: 3)]', got None).
Cause 4 wellarchitected-review-template-lifecycle#task#5 CreateReviewTemplate: 400/400 — $.Message: missing (expected '[Validation] [ECMA 262 regex "^[A-Za-z0-9-_.,:/()@!&?#+\'’\\s]+$" does not match input string "sim2real*probe*tpl"]', got None).
Cause 5 wellarchitected-workload-create-validation-envelopes#task#3 CreateWorkload: 400/400 — $.Message: value_mismatch (expected '[Validation] [object has missing required properties (["ReviewOwner"])]', got '1 validation error detected: Value at AwsRegions failed to satisfy constraint: Either AwsRegions, NonAwsRegions, or both must be specified'); $.Reason: value_mismatch (expected '', got 'FIELD_VALIDATION_FAILED').
Cause 6 wellarchitected-workload-create-validation-envelopes#task#7 CreateWorkload: 400/400 — $.Fields[1]: missing (expected {'Name': 'NonAwsRegions', 'Message': 'Either AwsRegions or NonAwsRegions is required.'}, got None); $.Fields[0].Message and $.Message also differ in wording.
Cause 7 wellarchitected-pagination-and-limit-validation#task#3 ListWorkloads: 400/400 — $.Reason: value_mismatch (expected 'CANNOT_PARSE', got 'FIELD_VALIDATION_FAILED').
Cause 8 wellarchitected-pagination-and-limit-validation#task#4 ListWorkloads: 400/400 — $.Fields: missing (expected [{'Name': 'MaxResults', 'Message': 'MaxResults must be between 1 and 50.'}], got None); $.Reason: missing (expected 'FIELD_VALIDATION_FAILED', got None); $.Message: missing.
Cause 9 wellarchitected-pagination-and-limit-validation#task#5 ListMilestones: 400/400 — identical: $.Fields, $.Reason, $.Message all missing.
Cause 10 wellarchitected-workload-create-validation-envelopes#task#6 CreateWorkload: status expected 400, got 200 — $.Fields missing (expected [{'Name': 'AwsRegions', 'Message': 'Unknown region sim2real-probe-region.'}]), $.Reason missing (expected 'FIELD_VALIDATION_FAILED'), $.Message missing, $headers.x-amzn-errortype missing.

Statuses and the modelled Reason/Fields values above are requirements; the quoted message strings are examples of messages built from the request members and their constraints.

## Steps to Reproduce
1. git clone the emulator repo and boot the emulator (wellarchitected is bootstrapped).
2. Replay trace set simparity-wire-aws-v1.
3. Representative cause: replay fixture wellarchitected-workload-create-validation-envelopes, step task#1 — POST /workloads with WorkloadName 'ab' (below @length min 3); the emulator answers 400 with no $.Message.
4. Replay the remaining steps listed in trace_ids: wellarchitected-lens-review-answer-lifecycle#task#10, wellarchitected-milestone-lifecycle#task#7, wellarchitected-review-template-lifecycle#task#5, wellarchitected-workload-create-validation-envelopes#task#3, #task#6, #task#7, wellarchitected-pagination-and-limit-validation#task#3, #task#4, #task#5. All ten causes must pass before this issue is closed.
