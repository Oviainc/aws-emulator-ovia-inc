Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wellarchitected.milestone-freezes-answers-while-workload-moves wellarchitected.duplicate-milestone-name-rejected wellarchitected.share-with-own-account-rejected
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected.milestone-freezes-answers-while-workload-moves|run#8|KeyError|choice = target["Choices"][0]["ChoiceId"]
Issue-Key: wellarchitected.duplicate-milestone-name-rejected|run#10|AssertionError|assert exact == expected_exact, (
Issue-Key: wellarchitected.share-with-own-account-rejected|run#8|AssertionError|assert message == expected, (
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# wellarchitected: ListAnswers omits AnswerSummary$Choices, milestone-name uniqueness is case/whitespace-sensitive with an unscoped conflict message, and the self-share refusal message diverges (3 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAnswers

## Description
Three independent root causes in the Well-Architected Tool emulator, all grounded in failing behavioral checks. They are bundled because they belong to one service; each must be closed on its own.

=== Cause 1 of 3: ListAnswers never reports a question's choice catalog (AnswerSummary$Choices) ===
AWS: Smithy (com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json): AnswerSummary has member `Choices` targeting the `Choices` list of `Choice` {ChoiceId (len 1..64), Title, Description, HelpfulResource, ImprovementPlan, AdditionalResources}. Recorded behavior: every AnswerSummary of a lens review of the AWS-official `wellarchitected` framework lens carries that per-question choice catalog; it is the only place a client can discover the ChoiceId values that UpdateAnswer$SelectedChoices requires (the checks read `answers[0]["Choices"][0]["ChoiceId"]` and feed it straight to UpdateAnswer).
Emulator: ListAnswers builds every summary with `choices: None` — services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/list_answers.rs, fn `answer_summary` (the member is explicitly set to None because the shipped framework question catalog in src/state.rs carries no choices). The member is therefore absent from the wire response and the replay fails with `KeyError: 'Choices'`.
Why: a client cannot discover any ChoiceId, so it cannot answer any question of a lens review; the entire answer -> risk -> milestone workflow is unreachable even though UpdateAnswer itself is implemented. Clients that iterate Choices to render a questionnaire see an empty/absent list.
Fix: the lens question catalog in src/state.rs (give each framework question its choices) plus the AnswerSummary projection in src/ops/list_answers.rs; the same projection should fill Answer$Choices wherever GetAnswer/UpdateAnswer return an Answer.
Traces: wellarchitected.milestone-freezes-answers-while-workload-moves.

=== Cause 2 of 3: CreateMilestone decides duplicate names by exact string equality, and its ConflictException message is not scoped to the workload ===
AWS: Smithy: CreateMilestoneInput$MilestoneName documents "Milestone names must be unique within a workload"; ConflictException is httpError 409 with required Message/ResourceId/ResourceType. Recorded behavior: a duplicate name is refused with code `ConflictException` and a message built from the request and the resource scope — "[Conflict] Milestone with name <MilestoneName> already exists under workload <WorkloadId>. Capitalization and whitespace are ignored in milestone names." (example recorded text). That message states the comparison rule: uniqueness is decided case-insensitively and with surrounding whitespace ignored, so a differently-cased or space-padded spelling of an existing name is ALSO a 409, and a rejected CreateMilestone consumes no milestone number.
Emulator: the code matches (ConflictException) but the message is "[Conflict] A milestone with name <name> already exists." — it names neither the workload the uniqueness is scoped to nor the normalization rule. More substantively, src/ops/create_milestone.rs decides the conflict with exact equality (`recorded.values().find(|record| record.milestone_name == input.milestone_name)`), so an upper-cased or whitespace-padded spelling of an existing milestone name is ACCEPTED and mints a new milestone number, where AWS returns 409.
Why: a client that relies on AWS's normalization gets duplicate-by-AWS-rules milestones in the emulator, and the milestone numbers diverge from AWS from that point on (AWS's next accepted milestone is 2, the emulator's is 3+). The refusal text also loses the workload scope that identifies which resource conflicted.
Fix: the uniqueness comparison in the CreateMilestone handler (trim + case-fold before comparing) and the shared `milestone_name_conflict` message builder in src/state.rs.
Traces: wellarchitected.duplicate-milestone-name-rejected.

=== Cause 3 of 3: CreateWorkloadShare self-share refusal uses a generic field-constraint message that discloses the caller's account id ===
AWS: 400 ValidationException (Smithy: ValidationException httpError 400). Recorded behavior only for the text: the self-share refusal is a fixed sentence about the principal relationship — "[Validation] The shared with principal cannot be the same as the sharer's principal." (example recorded text) — naming no account id.
Emulator: same status and same code (ValidationException, 400), but the message is the generic field-constraint template "[Validation] Value at SharedWith failed to satisfy constraint: Member must not be the principal sharing the workload (<account id>)" — built in src/ops/create_workload_share.rs at the `input.shared_with == shared_by` branch via `field_validation`, and it embeds the owning account id in the refusal.
Why: the status and code agree, so this is the message alone; the requirement is the fixed, principal-relationship wording that discloses no account id, not the literal recorded string. A client or log consumer matching the service's own refusal text sees a different sentence, and the emulator discloses an account id AWS does not put in this message. Lowest-severity cause of this bundle.
Fix: the self-share branch of the CreateWorkloadShare handler (use the service's own self-share sentence instead of the generic field-constraint template).
Traces: wellarchitected.share-with-own-account-rejected.

## Evidence
Cause 1 — wellarchitected.milestone-freezes-answers-while-workload-moves [error]:
  File "check_wellarchitected_milestone_freezes_answers_while_workload_moves.py", line 92, in run
    choice = target["Choices"][0]["ChoiceId"]
  KeyError: 'Choices'
Expected: each AnswerSummary from ListAnswers carries Choices = [{ChoiceId, Title, ...}, ...].
Actual: Choices is absent (list_answers.rs sets `choices: None`).

Cause 2 — wellarchitected.duplicate-milestone-name-rejected [mismatch]:
  AssertionError: a duplicate milestone name failed with ('ConflictException', '[Conflict] A milestone with name sim2real-behav-ms-<id> already exists.'), expected ('ConflictException', '[Conflict] Milestone with name sim2real-behav-ms-<id> already exists under workload <workload-id>. Capitalization and whitespace are ignored in milestone names.')
Expected: ConflictException (409) scoped to the workload; the same name upper-cased, and the same name space-padded, are each refused the same way; the next accepted milestone is number 2.
Actual: ConflictException with an unscoped message, and the handler compares names with `==`, so `name.upper()` and `"  name  "` would be accepted as new milestones (src/ops/create_milestone.rs, duplicate-name branch).

Cause 3 — wellarchitected.share-with-own-account-rejected [mismatch]:
  AssertionError: sharing with the sharer's own principal was refused with message '[Validation] Value at SharedWith failed to satisfy constraint: Member must not be the principal sharing the workload (<account id>)', expected "[Validation] The shared with principal cannot be the same as the sharer's principal."
Expected: 400 ValidationException whose message names the sharer/shared-with principal relationship and no account id.
Actual: 400 ValidationException with the generic per-field constraint template, including the owning account id (src/ops/create_workload_share.rs, `input.shared_with == shared_by` branch).

## Steps to Reproduce
Cause 1:
1. CreateWorkload(Lenses=["wellarchitected"], AwsRegions=[region], Environment=PREPRODUCTION, ReviewOwner=..., ClientRequestToken=uuid).
2. ListAnswers(WorkloadId=..., LensAlias="wellarchitected", MaxResults=50).
3. Read AnswerSummaries[0]["Choices"] — absent on the emulator, present on AWS; without it there is no ChoiceId to pass to UpdateAnswer.

Cause 2:
1. CreateWorkload as above.
2. CreateMilestone(WorkloadId=..., MilestoneName=N, ClientRequestToken=uuid) -> MilestoneNumber 1.
3. CreateMilestone with the same N and a fresh token -> ConflictException; compare the message against "[Conflict] Milestone with name N already exists under workload <WorkloadId>. Capitalization and whitespace are ignored in milestone names."
4. CreateMilestone with N.upper(), then with "  N  ", each with a fresh token -> AWS returns ConflictException for both; the emulator accepts them.
5. CreateMilestone with a genuinely different name -> AWS numbers it 2.

Cause 3:
1. CreateWorkload as above; read the caller's account id from STS GetCallerIdentity.
2. CreateWorkloadShare(WorkloadId=..., SharedWith=<own account id>, PermissionType="READONLY", ClientRequestToken=uuid).
3. Inspect Error.Code (ValidationException, matches) and Error.Message (diverges, and embeds the account id).
