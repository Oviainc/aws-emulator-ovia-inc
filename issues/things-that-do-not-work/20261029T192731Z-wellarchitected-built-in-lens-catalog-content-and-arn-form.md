Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-catalog-list-lenses
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-catalog-list-lenses wellarchitected-lens-association-lifecycle wellarchitected-lens-review-answer-lifecycle wellarchitected-pagination-and-limit-validation wellarchitected-review-template-lifecycle
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|ListLenses|2xx->2xx|$.LensSummaries[*]
Issue-Key: wellarchitected|ListLensReviews|2xx->2xx|$.LensReviewSummaries[*].LensAlias
Issue-Key: wellarchitected|ListLensReviews|2xx->2xx|$.LensReviewSummaries[*].LensArn
Issue-Key: wellarchitected|ListLensReviews|2xx->2xx|$.LensReviewSummaries[*].LensName
Issue-Key: wellarchitected|ListLensReviews|2xx->2xx|$.LensReviewSummaries[*].LensVersion
Issue-Key: wellarchitected|ListLensReviews|2xx->2xx|$.LensReviewSummaries[*].RiskCounts.UNANSWERED
Issue-Key: wellarchitected|GetLensReview|2xx->2xx|$.LensReview.LensArn
Issue-Key: wellarchitected|GetLensReview|2xx->2xx|$.LensReview.LensName
Issue-Key: wellarchitected|GetLensReview|2xx->2xx|$.LensReview.LensVersion
Issue-Key: wellarchitected|UpdateLensReview|2xx->2xx|$.LensReview.JiraConfiguration
Issue-Key: wellarchitected|UpdateLensReview|2xx->2xx|$.LensReview.LensArn
Issue-Key: wellarchitected|UpdateLensReview|2xx->2xx|$.LensReview.LensName
Issue-Key: wellarchitected|GetAnswer|2xx->4xx|$.Answer
Issue-Key: wellarchitected|GetAnswer|2xx->4xx|$.LensAlias
Issue-Key: wellarchitected|GetAnswer|2xx->4xx|$.LensArn
Issue-Key: wellarchitected|GetAnswer|2xx->4xx|$.WorkloadId
Issue-Key: wellarchitected|GetAnswer|2xx->4xx|status
Issue-Key: wellarchitected|UpdateAnswer|2xx->4xx|$.Answer
Issue-Key: wellarchitected|UpdateAnswer|2xx->4xx|$.LensAlias
Issue-Key: wellarchitected|UpdateAnswer|2xx->4xx|$.LensArn
Issue-Key: wellarchitected|UpdateAnswer|2xx->4xx|$.WorkloadId
Issue-Key: wellarchitected|UpdateAnswer|2xx->4xx|status
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*]
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*].ChoiceId
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*].Description
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].Choices[*].Title
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.AnswerSummaries[*].QuestionId
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.LensArn
Issue-Key: wellarchitected|ListAnswers|2xx->2xx|$.NextToken
Issue-Key: wellarchitected|GetReviewTemplateLensReview|2xx->2xx|$.LensReview.LensArn
Issue-Key: wellarchitected|GetReviewTemplateLensReview|2xx->2xx|$.LensReview.LensName
Issue-Key: wellarchitected|GetReviewTemplateLensReview|2xx->2xx|$.LensReview.LensVersion
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Built-in lens catalog content and ARN form diverge from the AWS lens library

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListLenses

## Description
This bundle has 9 causes, all rooted in the seeded AWS-official lens catalog: the emulator ships its own invented lens set (aliases, names, versions, question ids, choice ids) and mints AWS-official lens ARNs in the REGIONAL form, so every surface that echoes lens identity or walks lens content diverges.

Cause 1 of 9: ListLenses returns a different lens library.
AWS: ListLenses (GET /lenses, Smithy: ListLensesOutput$LensSummaries of LensSummary{LensArn, LensAlias, LensName, LensType, Description, CreatedAt, UpdatedAt, LensVersion, Owner, LensStatus}) returns the AWS-official lens library, which includes the Well-Architected Framework lens (LensAlias 'wellarchitected', LensName 'AWS Well-Architected Framework') and the published industry/technology lenses such as 'connectedmobility' (LensName 'Connected Mobility Lens'). An AWS-official lens ARN is partition-global and account-less: 'arn:aws:wellarchitected::aws:lens/<alias>' — no region segment, and the literal 'aws' in the account position. Each summary also carries Owner (LensSummary$Owner, AwsAccountId, 12 digits), which for an AWS_OFFICIAL lens is 'AWS' in the recording.
Emulator: returns 200 with a catalog whose every entry differs — no 'wellarchitected' entry at the expected index, no 'connectedmobility' entry at all, LensArn emitted as 'arn:aws:wellarchitected:us-west-2::lens/<alias>' (region segment present, 'aws' account token dropped), LensVersion flattened to a single invented date for every lens, Owner omitted entirely, and locally authored Description text. All 16 LensSummaries positions mismatch.
Why: a client that selects a lens by alias/ARN from ListLenses and passes it to AssociateLenses, CreateReviewTemplate or GetLensReview gets an ARN AWS never issues, and a client looking for the default Framework lens does not find it.
Fix: the seeded lens catalog and the lens-ARN formatter (AWS-official lenses must format without a region and with the 'aws' account token); LensSummary serializer must emit Owner.
Traces: wellarchitected-catalog-list-lenses#task#1.

Cause 2 of 9: ListLensReviews orders and labels lens reviews from the wrong catalog.
AWS: ListLensReviews (GET /workloads/{WorkloadId}/lensReviews) returns LensReviewSummaries whose LensArn/LensAlias/LensName/LensVersion are the identity of the associated lens and whose RiskCounts.UNANSWERED is the unanswered-question count of THAT lens.
Emulator: returns 200 but the two summaries are swapped and mislabelled — summary[0] reports the 'serverless' lens where the Framework lens is expected and vice versa, LensName 'Serverless Applications Lens'/'Well-Architected Framework' instead of 'Serverless Lens'/'AWS Well-Architected Framework', LensVersion '2025-01-15'/'2023-10-03' instead of the catalog versions, and RiskCounts.UNANSWERED is attached to the wrong lens (9 against the Framework lens, 57 against the serverless lens).
Why: the per-lens risk rollup a console or report renders is attributed to the wrong lens.
Fix: the lens catalog seed plus the ListLensReviews handler's summary ordering/identity resolution.
Traces: wellarchitected-lens-association-lifecycle#task#3.

Cause 3 of 9: ListLensReviews emits the regional lens ARN and the short lens name.
AWS: LensReviewSummary$LensArn for the Framework lens is 'arn:aws:wellarchitected::aws:lens/wellarchitected', LensName is 'AWS Well-Architected Framework', LensVersion is the catalog version of the lens.
Emulator: 'arn:aws:wellarchitected:us-west-2::lens/wellarchitected', 'Well-Architected Framework', and an invented LensVersion.
Why: the ARN cannot be used against the real service and the display name does not match AWS's.
Fix: lens-ARN formatter and catalog seed, as cause 1.
Traces: wellarchitected-lens-association-lifecycle#task#5.

Cause 4 of 9: GetLensReview echoes the same wrong lens identity.
AWS: GetLensReviewOutput$LensReview (LensReview{LensAlias, LensArn, LensVersion, LensName, ...}) carries the partition-global AWS-official ARN and 'AWS Well-Architected Framework'.
Emulator: returns 200 with the regional ARN, 'Well-Architected Framework', and the wrong LensVersion.
Why: same as cause 3, on the single-review read path.
Fix: lens-ARN formatter and catalog seed.
Traces: wellarchitected-lens-review-answer-lifecycle#task#3.

Cause 5 of 9: UpdateLensReview echoes the wrong lens identity and omits JiraConfiguration.
AWS: UpdateLensReviewOutput$LensReview carries the partition-global ARN and 'AWS Well-Architected Framework', and LensReview$JiraConfiguration (JiraSelectedQuestionConfiguration) is present with its SelectedPillars list — empty when nothing is selected, not absent.
Emulator: returns 200 with the regional ARN, the short LensName, and LensReview.JiraConfiguration absent from the body.
Why: a client reading SelectedPillars off the update response dereferences a missing member.
Fix: the UpdateLensReview serializer (emit JiraConfiguration with an empty SelectedPillars list) plus the ARN formatter.
Traces: wellarchitected-lens-review-answer-lifecycle#task#4.

Cause 6 of 9: GetAnswer cannot find a question the Framework lens asks.
AWS: GetAnswer (GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}) returns 200 with WorkloadId, LensAlias, LensArn and Answer for a question id of the lens — e.g. QuestionId 'cloud-financial-management' in pillar 'costOptimization'.
Emulator: returns 404 with no body members, because its seeded Framework lens asks questions under invented ids instead of AWS's.
Why: every client that addresses an answer by AWS's documented question id gets ResourceNotFound, so no review can be read or driven.
Fix: the seeded Framework lens question/choice content used by the answer store.
Traces: wellarchitected-lens-review-answer-lifecycle#task#5.

Cause 7 of 9: UpdateAnswer cannot find the same question.
AWS: UpdateAnswer (PATCH .../answers/{QuestionId}) returns 200 with the updated Answer, including Notes and the recomputed Risk.
Emulator: returns 404 with no body members, for the same reason as cause 6.
Why: answers cannot be recorded against AWS question ids at all.
Fix: as cause 6.
Traces: wellarchitected-lens-review-answer-lifecycle#task#6.

Cause 8 of 9: ListAnswers returns invented question and choice ids and omits Choice$Description.
AWS: ListAnswersOutput$AnswerSummaries carries the lens's own QuestionId values (e.g. 'cloud-financial-management') and Choices of Choice{ChoiceId, Title, Description} with AWS's choice ids (e.g. 'cost_cloud_financial_management_function') and the choice Description prose; LensArn is the partition-global AWS-official ARN.
Emulator: returns 200 with QuestionId 'cost-01', ChoiceIds 'cost-01-bp-01'..., every Choice.Description absent, and the regional LensArn.
Why: a client cannot map the returned choice ids back to the ids it must send in UpdateAnswer$SelectedChoices, and the improvement text is missing.
Fix: the seeded lens content and the AnswerSummary/Choice serializer.
Traces: wellarchitected-pagination-and-limit-validation#task#7.

Cause 9 of 9: GetReviewTemplateLensReview echoes the wrong lens identity.
AWS: GetReviewTemplateLensReviewOutput$LensReview carries the partition-global AWS-official ARN, 'AWS Well-Architected Framework', and the catalog LensVersion.
Emulator: returns 200 with the regional ARN, the short LensName and the wrong LensVersion.
Why: same as cause 3, on the review-template path.
Fix: lens-ARN formatter and catalog seed.
Traces: wellarchitected-review-template-lifecycle#task#7.

Note: no open wellarchitected issue carries these replay keys; the existing 20261029T120344Z-wellarchitected-listlenses-pagination issue is about NextToken decoding, not catalog content or ARN form.

## Evidence
Replay of trace set simparity-wire-aws-v1, 9 mismatch clusters, all service=wellarchitected.

Cause 1 — wellarchitected-catalog-list-lenses#task#1, ListLenses, expected 200 / actual 200: $.LensSummaries[0] expected {LensAlias 'wellarchitected', LensName 'AWS Well-Architected Framework', LensArn 'arn:aws:wellarchitected::aws:lens/wellarchitected', Owner 'AWS', LensVersion '2025-02-25', LensType 'AWS_OFFICIAL', LensStatus 'PUBLISHED'}, got {LensAlias 'dataanalytics', LensArn 'arn:aws:wellarchitected:us-west-2::lens/dataanalytics', no Owner, LensVersion '2025-01-15'}; $.LensSummaries[1] expected LensAlias 'connectedmobility', got 'financialservices'; all of $.LensSummaries[0..15] mismatch.

Cause 2 — wellarchitected-lens-association-lifecycle#task#3, ListLensReviews, 200/200: $.LensReviewSummaries[0].LensArn expected 'arn:aws:wellarchitected::aws:lens/wellarchitected', got 'arn:aws:wellarchitected:us-west-2::lens/serverless'; [0].LensName expected 'AWS Well-Architected Framework', got 'Serverless Applications Lens'; [0].RiskCounts.UNANSWERED expected 57, got 9; [1].LensName expected 'Serverless Lens', got 'Well-Architected Framework'; [1].RiskCounts.UNANSWERED expected 9, got 57.

Cause 3 — wellarchitected-lens-association-lifecycle#task#5, ListLensReviews, 200/200: [0].LensArn expected 'arn:aws:wellarchitected::aws:lens/wellarchitected', got 'arn:aws:wellarchitected:us-west-2::lens/wellarchitected'; [0].LensName expected 'AWS Well-Architected Framework', got 'Well-Architected Framework'; [0].LensVersion expected '2025-02-25', got '2023-10-03'. (Cluster also covers wellarchitected-lens-review-answer-lifecycle#task#2.)

Cause 4 — wellarchitected-lens-review-answer-lifecycle#task#3, GetLensReview, 200/200: $.LensReview.LensArn / .LensName / .LensVersion diverge exactly as cause 3.

Cause 5 — wellarchitected-lens-review-answer-lifecycle#task#4, UpdateLensReview, 200/200: $.LensReview.LensArn and .LensName as cause 3; $.LensReview.JiraConfiguration expected {'SelectedPillars': []}, got absent.

Cause 6 — wellarchitected-lens-review-answer-lifecycle#task#5, GetAnswer, expected 200, got 404: $.WorkloadId, $.LensAlias ('wellarchitected'), $.LensArn ('arn:aws:wellarchitected::aws:lens/wellarchitected') and $.Answer (QuestionId 'cloud-financial-management', PillarId 'costOptimization', Risk 'UNANSWERED', Choices 'cost_cloud_financial_management_*') all absent. (Cluster also covers the same fixture's task#7.)

Cause 7 — wellarchitected-lens-review-answer-lifecycle#task#6, UpdateAnswer, expected 200, got 404: $.Answer (same question, Risk 'HIGH', Notes 'answered by the SimParity wire probe') absent.

Cause 8 — wellarchitected-pagination-and-limit-validation#task#7, ListAnswers, 200/200: $.LensArn expected 'arn:aws:wellarchitected::aws:lens/wellarchitected', got the us-west-2 form; $.AnswerSummaries[0].QuestionId expected 'cloud-financial-management', got 'cost-01'; [0].Choices[0].ChoiceId expected 'cost_cloud_financial_management_function', got 'cost-01-bp-01'; [0].Choices[0..8].Description expected prose, got absent.

Cause 9 — wellarchitected-review-template-lifecycle#task#7, GetReviewTemplateLensReview, 200/200: $.LensReview.LensArn / .LensName / .LensVersion diverge exactly as cause 3.

Smithy (com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json): LensSummary{LensArn, LensAlias, LensName, LensType, Description, CreatedAt, UpdatedAt, LensVersion, Owner, LensStatus}; LensReview{LensAlias, LensArn, LensVersion, LensName, LensStatus, JiraConfiguration, RiskCounts, ...}; AnswerSummary$Choices -> Choice{ChoiceId, Title, Description, ...}; GetAnswer GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId} -> 200.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: GET /lenses and compare LensSummaries against the AWS lens library — note the missing 'wellarchitected'/'connectedmobility' entries, the 'arn:aws:wellarchitected:us-west-2::lens/...' ARN form and the absent Owner.
4. Causes 2-5, 9: create a workload, associate the serverless lens, then GET /workloads/{id}/lensReviews, GET and PATCH /workloads/{id}/lensReviews/wellarchitected, and GET /reviewTemplates/{arn}/lensReviews/wellarchitected; compare LensArn/LensName/LensVersion and the presence of LensReview.JiraConfiguration.
5. Causes 6-8: GET /workloads/{id}/lensReviews/wellarchitected/answers/cloud-financial-management (404 instead of 200), PATCH the same path (404), and GET .../answers and inspect QuestionId/ChoiceId/Choice.Description.
