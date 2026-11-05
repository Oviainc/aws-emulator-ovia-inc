Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-lens-review-answer-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-lens-review-answer-lifecycle wellarchitected-pagination-and-limit-validation
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.Choices[*]
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.Choices[*].ChoiceId
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.Choices[*].Description
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.Choices[*].Title
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.HelpfulResourceUrl
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.QuestionDescription
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*]
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*].Notes
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*].Reason
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.Choices[*]
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.Choices[*].ChoiceId
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.Choices[*].Description
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.Choices[*].Title
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.HelpfulResourceUrl
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.ImprovementPlanUrl
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.QuestionDescription
Issue-Key: wellarchitected|UpdateAnswer|2xx->2xx|$.Answer.Risk
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*]
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*].Notes
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.ChoiceAnswers[*].Reason
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.ImprovementPlanUrl
Issue-Key: wellarchitected|GetAnswer|2xx->2xx|$.Answer.Risk
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$.Fields[*].Message
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*]
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*].ChoiceId
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*].Description
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*].Title
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.NextToken
Issue-Commit: 2e710d778938761c1f752c2d9288acfa8834ac9b

# AWS official lens question catalog content and derived answer risk diverge

> _Note: this issue was identified against the code base at commit `2e710d778938761c1f752c2d9288acfa8834ac9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAnswer

## Description
This bundle has 5 net-new causes, all in service wellarchitected, all grounded in wire fixtures of trace set simparity-wire-aws-v1. They share one fault: the built-in question/choice catalog for the AWS official Well-Architected Framework lens is authored by the emulator rather than taken from the lens AWS serves, so the choice ids, the per-choice descriptions, the question-level description and help/improvement URLs, the choice count, the risk derived from a selection, and even the valid-choice list inside a validation message are all wrong wherever an answer is projected. The replay marked none of these five clusters owned or stub-blocked. Related open issue 20261103T105038Z-wellarchitected-answer-choiceanswers-projection-improvement-plan-details-and-two.md reports the ChoiceAnswers projection shape from the behavioral suite and does NOT carry any of these wire keys; 20261029T223409Z-wellarchitected-custom-lens-publish-path-empty-selection-risk-derivation.md concerns CUSTOM-lens risk derivation from authored riskRules, not the official lens's catalog risk rules.

Cause 1 of 5: GetAnswer omits the question's QuestionDescription and HelpfulResourceUrl and returns authored choice ids/titles/descriptions, one choice short, for an official-lens question.
AWS: Smithy (services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json): GetAnswer GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}, code 200, output GetAnswerOutput{WorkloadId, MilestoneNumber, LensAlias, LensArn, Answer}; Answer{QuestionId, PillarId, QuestionTitle, QuestionDescription (@length 1..1024), ImprovementPlanUrl (@length 1..2048), HelpfulResourceUrl (@length 1..2048), Choices, SelectedChoices, ChoiceAnswers, IsApplicable, Risk, Notes, Reason}; Choice{ChoiceId (@length 1..64), Title (@length 1..512), Description (@length 1..1024), HelpfulResource, ImprovementPlan, AdditionalResources}. The model does not enumerate the official lens's content, so the content itself is recorded behavior only: for a framework-lens question the recording shows QuestionDescription present (the lens's published question description), HelpfulResourceUrl present (the lens's published per-question helpful-resources URL under https://wa.aws.amazon.com/wellarchitected/<lensVersion>/...), and a Choices list that contains every published choice of that question, including the terminal 'None of these' choice, each with the published ChoiceId, Title and full Description.
Emulator: Answer.QuestionDescription and Answer.HelpfulResourceUrl are absent; the Choices list is one entry short (the recording's last choice, the 'None of these' choice whose id ends in '_no', has no counterpart — $.Answer.Choices[9] missing); several ChoiceIds differ from the published ones (e.g. '..._cloud_budget_forecast' vs '..._budget_forecast', '..._cost_aware_process' vs '..._cost_awareness'); and every Choice.Description is a short re-worded sentence instead of the published description. QuestionId, PillarId and QuestionTitle match.
Why: choice ids are the API's primary keys — a client that submits the published ids in SelectedChoices (or stores answers keyed by id) cannot address the emulator's choices at all, and the missing 'None of these' choice removes the only way to answer a question negatively. A client that renders QuestionDescription or links HelpfulResourceUrl gets nothing.
Fix: the AWS official lens question/choice catalog data behind the answer serializer shared by GetAnswer/UpdateAnswer/ListAnswers — carry the published choice ids, titles and full descriptions, the complete choice set, and the question-level description and helpful-resource/improvement-plan URLs.
Traces: wellarchitected-lens-review-answer-lifecycle#task#5.

Cause 2 of 5: UpdateAnswer's 200 response repeats the same catalog divergences, additionally omits ImprovementPlanUrl, and derives the wrong Risk for the submitted selection.
AWS: Smithy: UpdateAnswer PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}, code 200, output UpdateAnswerOutput{WorkloadId, LensAlias, LensArn, Answer}; Answer$Risk -> Risk enum [UNANSWERED, HIGH, MEDIUM, NONE, NOT_APPLICABLE]; Answer$ImprovementPlanUrl @length 1..2048; Answer$ChoiceAnswers -> list<ChoiceAnswer{ChoiceId, Status, Reason, Notes}> documented 'A list of selected choices to a question in your workload'. Recorded behavior: after a selection that satisfies none of the question's risk-reducing rules, the official lens's risk rules yield Risk HIGH for that answer; the response also carries the question's ImprovementPlanUrl, and ChoiceAnswers holds one entry per ANSWERED choice with Reason and Notes present.
Emulator: returns Risk 'MEDIUM' where the recording has 'HIGH'; ImprovementPlanUrl, HelpfulResourceUrl and QuestionDescription are absent; ChoiceAnswers lists an entry for every defined choice (extra entries $.Answer.ChoiceAnswers[1] .. [8]) and the entries lack Reason and Notes; and the Choices list shows the same wrong ids/descriptions and missing tenth choice as cause 1.
Why: Risk is what drives the whole workload report — an answer graded MEDIUM instead of HIGH mis-states the workload's high-risk count and hides a finding a client is expected to act on. Clients that follow ImprovementPlanUrl to remediation guidance get nothing.
Fix: the official lens's risk-rule evaluation in the UpdateAnswer handler (derive the answer risk from the lens's published risk rules for the question, not from a count of selected choices) plus the shared answer serializer/catalog from cause 1. The ChoiceAnswers projection part is the same projection open issue 20261103T105038Z-...-answer-choiceanswers-projection-improvement-plan-details-and-two.md names from the behavioral suite; that issue carries no wire key, so this wire step is not covered by it.
Traces: wellarchitected-lens-review-answer-lifecycle#task#6.

Cause 3 of 5: a GetAnswer read-back after the update shows the stored answer with the same wrong catalog content, wrong Risk and over-broad ChoiceAnswers, so the divergence is persisted, not confined to the write path.
AWS: Smithy: GetAnswer GET .../answers/{QuestionId} -> 200 {.., Answer}. Recorded behavior: reading the answer back returns exactly what the update returned — Risk HIGH, QuestionDescription/HelpfulResourceUrl/ImprovementPlanUrl present, ChoiceAnswers limited to the answered choices with Reason and Notes.
Emulator: the read-back carries Risk 'MEDIUM', absent QuestionDescription/HelpfulResourceUrl/ImprovementPlanUrl, ChoiceAnswers entries for all nine of its choices without Reason/Notes, and the wrong choice ids/descriptions.
Why: a client that writes then re-reads (the normal pattern for confirming an answer) sees the diverging state both times, so no read path offers the correct ids or risk.
Fix: same catalog + risk-rule evaluation + ChoiceAnswers projection, shared by the GetAnswer handler.
Traces: wellarchitected-lens-review-answer-lifecycle#task#7.

Cause 4 of 5: UpdateAnswer's invalid-choice refusal message lists the emulator's authored choice ids as the valid set.
AWS: Smithy: UpdateAnswer errors include ValidationException (httpError 400) with Fields -> list<ValidationExceptionField{Name, Message}>. Recorded behavior: when SelectedChoices names a choice that the question does not define, AWS answers 400 ValidationException whose field message's fixed text is 'SelectedChoices contains invalid choices. Valid choices are: [' ... '].' and whose variable part is the comma-separated list of the question's published choice ids, in the lens's choice order and including the terminal 'None of these' id.
Emulator: returns the right status (400), error class and message template, but the enumerated list is its own catalog's ids — it substitutes '..._cloud_budget_forecast' for '..._budget_forecast', '..._cost_aware_process' for '..._cost_awareness', '..._report_notify'/'..._keep_up_to_date'/'..._cost_aware_culture'/'..._business_value' for the published '..._scheduled'/'..._quantify_value'/'..._usage_report'/'..._culture', and omits the '..._no' choice.
Why: this message is the one place a client is told which ids to use; listing ids that AWS does not accept sends a client straight into a second failure against real AWS, and the omitted 'None of these' id can never be discovered.
Fix: the catalog of cause 1 (the message renderer already builds the list from the question's choice set, so correcting the catalog corrects the message).
Traces: wellarchitected-lens-review-answer-lifecycle#task#9.

Cause 5 of 5: ListAnswers projects the same wrong choice catalog into AnswerSummaries, and its NextToken is a hex-encoded internal cursor rather than AWS's opaque token form.
AWS: Smithy: ListAnswers GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers, code 200, output ListAnswersOutput{WorkloadId, MilestoneNumber, LensAlias, LensArn, AnswerSummaries, NextToken}; AnswerSummary{QuestionId, PillarId, QuestionTitle, Choices, SelectedChoices, ChoiceAnswerSummaries, IsApplicable, Risk, Reason, QuestionType}; NextToken @pattern ^[A-Za-z0-9+\/=_-]+$, paginated{inputToken NextToken, outputToken NextToken, pageSize MaxResults}. Recorded behavior: each summary's Choices carry the published ids/titles/descriptions and the full choice set, and the page token is an opaque continuation token whose contents are not the request's own filter values.
Emulator: every AnswerSummaries[0].Choices entry shows the same wrong ChoiceIds, abbreviated Descriptions and missing tenth choice as cause 1, and NextToken is the hex encoding of a JSON cursor object (it decodes to a JSON document naming the next pillar), which fails the recorded token shape. Hypothesis: the pagination cursor is emitted by hex-encoding internal JSON state instead of the opaque token form the recording shows.
Why: a paginating client enumerating a lens review's answers gets choice ids it cannot use, and a token whose decoded content is internal state — clients or tools that validate/round-trip the token form against AWS's shape diverge, and the leak exposes server-side paging state.
Fix: the shared answer/choice serializer and catalog from cause 1 for the summaries, and the ListAnswers pagination token encoder for the token form.
Traces: wellarchitected-pagination-and-limit-validation#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 12 fixtures replayed, 94 steps, 71 passed, 23 mismatched, score 75.5%, 19 divergence clusters; emulator_up true, bootstrap_signal false. All five clusters below carry empty owned_by and empty stub_path.

Cause 1 — fixture wellarchitected-lens-review-answer-lifecycle, step task#5, GetAnswer, expected 200 / actual 200, mismatch. Detail: $.Answer.QuestionDescription missing (expected the published question description, 'Implementing Cloud Financial Management enables organizations to realize business value and financial success as they optimize their cost and usage and scale on AWS.', got None); $.Answer.HelpfulResourceUrl missing (expected the lens's per-question helpful-resources URL under https://wa.aws.amazon.com/wellarchitected/<lensVersion>/..., got None); $.Answer.Choices[0].Description value_mismatch (expected the published multi-sentence description, got 'Create a team that is accountable for establishing and maintaining cost awareness across the organization.'); $.Answer.Choices[2].ChoiceId value_mismatch (expected 'cost_cloud_financial_management_budget_forecast', got 'cost_cloud_financial_management_cloud_budget_forecast'); further ChoiceId/Title/Description mismatches on Choices[3]..[8]; $.Answer.Choices[9] missing entirely (the recording's tenth, 'None of these', choice).

Cause 2 — same fixture, step task#6, UpdateAnswer, expected 200 / actual 200, mismatch. Detail adds: $.Answer.Risk value_mismatch (expected 'HIGH', got 'MEDIUM'); $.Answer.ImprovementPlanUrl missing (expected the published improvement-plan URL, got None); $.Answer.QuestionDescription and $.Answer.HelpfulResourceUrl missing; divergence keys also list $.Answer.ChoiceAnswers[0].Notes, $.Answer.ChoiceAnswers[0].Reason and $.Answer.ChoiceAnswers[1]..[8] (entries the recording does not have), plus the same Choices id/description keys as cause 1.

Cause 3 — same fixture, step task#7, GetAnswer, expected 200 / actual 200, mismatch. Same key set as cause 2 ($.Answer.Risk HIGH vs MEDIUM, missing QuestionDescription/HelpfulResourceUrl/ImprovementPlanUrl, ChoiceAnswers[1]..[8] extra and [0] missing Reason/Notes, Choices ids/descriptions), proving the divergence is in stored/served state, not only in the update response.

Cause 4 — same fixture, step task#9, UpdateAnswer, expected 400 / actual 400, mismatch. Detail: $.Fields[0].Message value_mismatch — expected 'SelectedChoices contains invalid choices. Valid choices are: [cost_cloud_financial_management_function, cost_cloud_financial_management_partnership, cost_cloud_financial_management_budget_forecast, cost_cloud_financial_management_cost_awareness, cost_cloud_financial_management_proactive_process, cost_cloud_financial_management_scheduled, cost_cloud_financial_management_quantify_value, cost_cloud_financial_management_usage_report, cost_cloud_financial_management_culture, cost_cloud_financial_management_no].', got 'SelectedChoices contains invalid choices. Valid choices are: [cost_cloud_financial_management_function, cost_cloud_financial_management_partnership, cost_cloud_financial_management_cloud_budget_forecast, cost_cloud_financial_management_cost_aware_process, cost_cloud_financial_management_report_notify, cost_cloud_financial_management_proactive_process, cost_cloud_financial_management_keep_up_to_date, cost_cloud_financial_management_cost_aware_culture, cost_cloud_financial_management_business_value].' Fixed text: 'SelectedChoices contains invalid choices. Valid choices are: [' and '].'; variable part: the question's published choice ids in lens order (ten of them here, including the '_no' choice) — the ids quoted are this question's catalog ids, which is exactly what must come from the catalog.

Cause 5 — fixture wellarchitected-pagination-and-limit-validation, step task#7, ListAnswers, expected 200 / actual 200, mismatch. Detail: $.NextToken value_mismatch against the recorded token shape, got '7b2271223a22636c6f75642d66696e616e6369616c2d6d616e6167656d656e74227d' (the hex encoding of {"q":"cloud-financial-management"}); plus $.AnswerSummaries[0].Choices[0..9] ChoiceId/Title/Description mismatches identical to cause 1, including the missing tenth choice.

Smithy citations: GetAnswer GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId} -> 200 {.., Answer}; UpdateAnswer PATCH same path -> 200 {.., Answer}, errors ValidationException(400); ListAnswers GET .../answers -> 200 {.., AnswerSummaries, NextToken @pattern ^[A-Za-z0-9+/=_-]+$}, paginated; Answer{QuestionDescription @length 1..1024, HelpfulResourceUrl/ImprovementPlanUrl @length 1..2048, Choices, ChoiceAnswers, Risk}; Risk enum [UNANSWERED, HIGH, MEDIUM, NONE, NOT_APPLICABLE]; Choice{ChoiceId @length 1..64, Title @length 1..512, Description @length 1..1024}; AnswerSummary{.., Choices, ChoiceAnswerSummaries, Risk}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 and look at service wellarchitected; the five clusters below are the ones with no owned_by and no stub_path.

Cause 1: CreateWorkload with the default 'wellarchitected' lens, then GET /workloads/{WorkloadId}/lensReviews/wellarchitected/answers/{QuestionId} for a framework-lens question. The Answer has no QuestionDescription and no HelpfulResourceUrl, nine Choices instead of ten (no 'None of these' choice), several ChoiceIds renamed and every Choice.Description shortened. Re-replay wellarchitected-lens-review-answer-lifecycle#task#5.

Cause 2: PATCH the same answer path with a SelectedChoices set that the lens's risk rules do not satisfy. The 200 response reports Risk 'MEDIUM' where AWS reports 'HIGH', omits ImprovementPlanUrl/HelpfulResourceUrl/QuestionDescription, and returns one ChoiceAnswers entry per defined choice (none carrying Reason or Notes). Re-replay wellarchitected-lens-review-answer-lifecycle#task#6.

Cause 3: GET the same answer again after that update; the stored answer shows the identical divergences. Re-replay wellarchitected-lens-review-answer-lifecycle#task#7.

Cause 4: PATCH the same answer path with a SelectedChoices entry that is not a choice of the question. The 400 ValidationException's field message enumerates the emulator's authored choice ids rather than the published ones. Re-replay wellarchitected-lens-review-answer-lifecycle#task#9.

Cause 5: GET /workloads/{WorkloadId}/lensReviews/wellarchitected/answers?MaxResults=<small> to force a page break. The AnswerSummaries' Choices show the same wrong ids/descriptions and missing choice, and NextToken is a hex-encoded JSON cursor (hex-decode it to see the internal paging state) instead of AWS's opaque token form. Re-replay wellarchitected-pagination-and-limit-validation#task#7.
