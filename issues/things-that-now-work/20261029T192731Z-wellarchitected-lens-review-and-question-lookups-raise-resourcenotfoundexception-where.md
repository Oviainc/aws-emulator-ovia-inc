Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-lens-association-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-lens-association-lifecycle wellarchitected-lens-review-answer-lifecycle wellarchitected-review-template-lifecycle wellarchitected-pagination-and-limit-validation
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|AssociateLenses|4xx->4xx|$.Message
Issue-Key: wellarchitected|AssociateLenses|4xx->4xx|$.Reason
Issue-Key: wellarchitected|AssociateLenses|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|AssociateLenses|4xx->4xx|status
Issue-Key: wellarchitected|GetLensReview|4xx->4xx|$.Fields
Issue-Key: wellarchitected|GetLensReview|4xx->4xx|$.Message
Issue-Key: wellarchitected|GetLensReview|4xx->4xx|$.Reason
Issue-Key: wellarchitected|GetLensReview|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|GetLensReview|4xx->4xx|status
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$.Fields
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$.Message
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$.Reason
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|GetAnswer|4xx->4xx|status
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$.Fields
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$.Reason
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|status
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->4xx|$.Reason
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->4xx|status
Issue-Key: wellarchitected|GetReviewTemplateLensReview|4xx->4xx|$.Fields
Issue-Key: wellarchitected|GetReviewTemplateLensReview|4xx->4xx|$.Message
Issue-Key: wellarchitected|GetReviewTemplateLensReview|4xx->4xx|$.Reason
Issue-Key: wellarchitected|GetReviewTemplateLensReview|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|GetReviewTemplateLensReview|4xx->4xx|status
Issue-Key: wellarchitected|ListAnswers|4xx->2xx|$.Fields
Issue-Key: wellarchitected|ListAnswers|4xx->2xx|$.Message
Issue-Key: wellarchitected|ListAnswers|4xx->2xx|$.Reason
Issue-Key: wellarchitected|ListAnswers|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|ListAnswers|4xx->2xx|status
Issue-Related: 20261029T192731Z-wellarchitected-validationexception-envelopes-use-generic-framework-text-and-omit.md
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Lens, review and question lookups raise ResourceNotFoundException where AWS raises ValidationException

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateLenses

## Description
This bundle has 7 causes sharing one fault: when an input NAMES something that does not resolve (a lens alias, a lens review, a question id, a choice id, a pillar id), Well-Architected treats it as INPUT validation and answers 400 ValidationException with Reason/Fields; the emulator instead resolves it as a resource lookup and answers 404 ResourceNotFoundException (or, for one path, does not validate at all). Smithy: every one of these operations models BOTH ValidationException (httpError 400, with Reason: ValidationExceptionReason and Fields: list<ValidationExceptionField{Name,Message}>) and ResourceNotFoundException (httpError 404), so the choice between them is a contract, not a detail.

Cause 1 of 7: AssociateLenses with an unknown lens alias.
AWS: 400 ValidationException with Message '[Validation] Failed to get lenses <lensAlias>. User might be not authorized to access lens.' — fixed text apart from <lensAlias>, the unresolved alias from LensAliases in the request — and Reason emitted (empty in the recording).
Emulator: 404 with x-amzn-errortype 'ResourceNotFoundException' and '[ResourceNotFound] No lens with alias <lensAlias> was found in account <accountId>.', no Reason.
Why: a client retries or fails differently on 404 than on 400, and an SDK's modeled exception type is wrong, so a catch of ValidationException does not fire.
Fix: AssociateLenses handler's lens-alias resolution — raise ValidationException, not ResourceNotFoundException.
Traces: wellarchitected-lens-association-lifecycle#task#6.

Cause 2 of 7: GetLensReview for a lens that has no review in the workload.
AWS: 400 ValidationException with Reason 'FIELD_VALIDATION_FAILED', Fields [{Name 'LensAlias', Message 'No lens review for lens alias <lensAlias> was found in workload <workloadId>.'}] and the same sentence as Message prefixed '[Validation] ' — fixed text apart from the alias and the workload id, both from the request.
Emulator: 404 ResourceNotFoundException '[ResourceNotFound] No review of lens <lensAlias> in a workload with ID <workloadId> was found in account <accountId>.', with no Reason and no Fields.
Why: wrong status, wrong exception type, and the Fields a client reads to attribute the error are absent.
Fix: GetLensReview handler's review lookup and the ValidationException serializer.
Traces: wellarchitected-lens-association-lifecycle#task#8.

Cause 3 of 7: GetAnswer for a question id the lens does not ask.
AWS: 400 ValidationException, Reason 'FIELD_VALIDATION_FAILED', Fields [{Name 'QuestionId', Message 'No question with ID <questionId> was found in the lens with alias <lensAlias>.'}], Message that sentence prefixed '[Validation] '.
Emulator: 404 ResourceNotFoundException '[ResourceNotFound] No question with ID <questionId> in a review of lens <lensAlias> in a workload with ID <workloadId> was found in account <accountId>.', no Reason/Fields.
Why: as cause 2; a client cannot tell a bad question id (fix the request) from a missing workload (fix the state).
Fix: GetAnswer handler's question resolution.
Traces: wellarchitected-lens-review-answer-lifecycle#task#8.

Cause 4 of 7: UpdateAnswer with SelectedChoices the question does not offer.
AWS: 400 ValidationException, Message '[Validation] The validation of one or more fields has failed.' (fixed text, no variable part), Reason 'FIELD_VALIDATION_FAILED', and Fields [{Name 'SelectedChoices', Message 'SelectedChoices contains invalid choices. Valid choices are: [<choiceId>, ...].'}] where the bracketed list is the question's own choice ids in order.
Emulator: 404 ResourceNotFoundException reporting the QUESTION as not found, no Reason/Fields.
Why: the client is told the question does not exist when the question exists and only a choice id was wrong, and loses the valid-choice list AWS hands back.
Fix: UpdateAnswer handler's choice validation (validate choices against the resolved question, raise ValidationException with Fields).
Traces: wellarchitected-lens-review-answer-lifecycle#task#9.

Cause 5 of 7: CreateReviewTemplate with an unknown lens alias.
AWS: 400 ValidationException with the same '[Validation] Failed to get lenses <lensAlias>. User might be not authorized to access lens.' message as cause 1 and Reason emitted.
Emulator: 404 ResourceNotFoundException '[ResourceNotFound] No lens with alias <lensAlias> was found in account <accountId>.'
Why/Fix: as cause 1, on the review-template create path.
Traces: wellarchitected-review-template-lifecycle#task#6.

Cause 6 of 7: GetReviewTemplateLensReview for a lens with no review in the template.
AWS: 400 ValidationException with Reason 'FIELD_VALIDATION_FAILED' and Fields carrying one entry whose Name is 'LensAlias'.
Emulator: 404 ResourceNotFoundException '[ResourceNotFound] No review of lens <lensAlias> in a review template with ARN <templateArn> was found in account <accountId>.', no Reason/Fields.
Why/Fix: as cause 2, on the review-template read path.
Traces: wellarchitected-review-template-lifecycle#task#8.

Cause 7 of 7: ListAnswers does not validate PillarId at all.
AWS: ListAnswers (GET .../answers, PillarId @length 1..64 httpQuery) refuses a PillarId that is not a pillar of the lens with 400 ValidationException, Reason 'FIELD_VALIDATION_FAILED' and Fields carrying one entry whose Name is 'PillarId'.
Emulator: returns 200 with no error members at all.
Why: a typo in a pillar filter silently yields a result set instead of an error, so a client cannot tell an empty/unfiltered page from a rejected filter.
Fix: ListAnswers handler's PillarId validation against the lens's pillar set.
Traces: wellarchitected-pagination-and-limit-validation#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1, 7 mismatch clusters, all service=wellarchitected.

Cause 1 — wellarchitected-lens-association-lifecycle#task#6, AssociateLenses, expected 400, got 404: $.Message expected '[Validation] Failed to get lenses sim2real-probe-nope. User might be not authorized to access lens.', got '[ResourceNotFound] No lens with alias sim2real-probe-nope was found in account 000000000000.'; $.Reason expected '' (present), got absent; $headers.x-amzn-errortype expected (shaped), got 'ResourceNotFoundException'.

Cause 2 — wellarchitected-lens-association-lifecycle#task#8, GetLensReview, expected 400, got 404: $.Fields expected [{'Name': 'LensAlias', 'Message': 'No lens review for lens alias serverless was found in workload f2a4f416239d2d08a04e238c3c29994c.'}], got absent; $.Message expected '[Validation] No lens review for lens alias serverless was found in workload f2a4f416239d2d08a04e238c3c29994c.', got '[ResourceNotFound] No review of lens serverless in a workload with ID f2a4f416239d2d08a04e238c3c29994c was found in account 000000000000.'; $.Reason expected 'FIELD_VALIDATION_FAILED', got absent; x-amzn-errortype got 'ResourceNotFoundException'.

Cause 3 — wellarchitected-lens-review-answer-lifecycle#task#8, GetAnswer, expected 400, got 404: $.Fields expected [{'Name': 'QuestionId', 'Message': 'No question with ID sim2real-probe-nope was found in the lens with alias wellarchitected.'}], got absent; $.Message expected '[Validation] No question with ID sim2real-probe-nope was found in the lens with alias wellarchitected.', got '[ResourceNotFound] No question with ID sim2real-probe-nope in a review of lens wellarchitected in a workload with ID 061b94f63e1f3962bc26a16b3ba7ed97 was found in account 000000000000.'; $.Reason expected 'FIELD_VALIDATION_FAILED', got absent.

Cause 4 — wellarchitected-lens-review-answer-lifecycle#task#9, UpdateAnswer, expected 400, got 404: $.Fields expected [{'Name': 'SelectedChoices', 'Message': 'SelectedChoices contains invalid choices. Valid choices are: [cost_cloud_financial_management_function, ... cost_cloud_financial_management_no].'}], got absent; $.Message expected '[Validation] The validation of one or more fields has failed.', got '[ResourceNotFound] No question with ID cloud-financial-management in a review of lens wellarchitected in a workload with ID 061b94f63e1f3962bc26a16b3ba7ed97 was found in account 000000000000.'; $.Reason expected 'FIELD_VALIDATION_FAILED', got absent.

Cause 5 — wellarchitected-review-template-lifecycle#task#6, CreateReviewTemplate, expected 400, got 404: $.Message expected '[Validation] Failed to get lenses sim2real-probe-nope. User might be not authorized to access lens.', got '[ResourceNotFound] No lens with alias sim2real-probe-nope was found in account 000000000000.'; $.Reason expected '', got absent; x-amzn-errortype got 'ResourceNotFoundException'.

Cause 6 — wellarchitected-review-template-lifecycle#task#8, GetReviewTemplateLensReview, expected 400, got 404: $.Fields expected [{'Name': 'LensAlias', ...}], got absent; $.Reason expected 'FIELD_VALIDATION_FAILED', got absent; $.Message got '[ResourceNotFound] No review of lens serverless in a review template with ARN arn:aws:wellarchitected:us-west-2:000000000000:review-template/c025b81ef8716ee41dfed382aa81f00d was found in account 000000000000.'; x-amzn-errortype got 'ResourceNotFoundException'.

Cause 7 — wellarchitected-pagination-and-limit-validation#task#6, ListAnswers, expected 400, got 200: $.Fields expected [{'Name': 'PillarId', ...}], got absent; $.Message expected (shaped), got absent; $.Reason expected 'FIELD_VALIDATION_FAILED', got absent; x-amzn-errortype expected (shaped), got absent.

Smithy: AssociateLenses, GetLensReview, GetAnswer, UpdateAnswer, CreateReviewTemplate, GetReviewTemplateLensReview and ListAnswers each model ValidationException (httpError 400) AND ResourceNotFoundException (httpError 404); ValidationException{Message, Reason: ValidationExceptionReason, Fields: list<ValidationExceptionField{Name, Message}>}; ListAnswers$PillarId httpQuery 'PillarId' @length 1..64.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. PATCH /workloads/{id}/associateLenses with an alias no lens has (cause 1) and POST /reviewTemplates naming that alias in Lenses (cause 5) — both answer 404 ResourceNotFoundException instead of 400 ValidationException.
4. GET /workloads/{id}/lensReviews/serverless on a workload with no serverless review (cause 2) and GET /reviewTemplates/{arn}/lensReviews/serverless (cause 6).
5. GET /workloads/{id}/lensReviews/wellarchitected/answers/{unknown-question} (cause 3) and PATCH an existing answer with a SelectedChoices entry the question does not offer (cause 4).
6. GET /workloads/{id}/lensReviews/wellarchitected/answers?PillarId=<not a pillar of the lens> — 200 instead of 400 (cause 7).
