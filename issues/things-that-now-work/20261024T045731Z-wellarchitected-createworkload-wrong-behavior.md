Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# CreateWorkload idempotency replay ignores DiscoveryConfig and JiraConfiguration when comparing parameters

_Filed: 2026-10-24T04:57:31Z_
_Operation: CreateWorkload | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkloadInput$ClientRequestToken` is `@idempotencyToken`, and the documented rule is conditioned on the parameters matching: "If you retry a request with the same client request token and the SAME PARAMETERS after the original request has completed successfully, the result of the original request is returned." The handler implements that by hashing a `parameter_fingerprint` of the request and answering a mismatch with `ConflictException` (409).

The fingerprint omits two input members the operation stores on the workload: `DiscoveryConfig` and `JiraConfiguration`. So two requests that carry the same `ClientRequestToken` but different `DiscoveryConfig` (e.g. `TrustedAdvisorIntegrationStatus` ENABLED vs DISABLED, or a different `WorkloadResourceDefinition`) or a different `JiraConfiguration` are treated as the SAME request: the second is replayed and answers the first workload's id, silently discarding the different configuration the caller asked for. Expected: a replay under one token with different parameters is refused with the modelled `ConflictException` (409), exactly as the handler already does for a different `WorkloadName`, `Lenses`, `Tags`, etc.

Secondary symptom of the same class, in the same handler: when `ReviewTemplateArns` names a stored review template, only the template's LENSES and NOTES are carried over to the new workload (lines 390-417). The template's recorded ANSWERS are not copied into the new workload's lens reviews, so a workload created from a template reports every question as `UNANSWERED` — `GetWorkload`'s `RiskCounts` and `ListAnswers` show nothing pre-filled. Pre-populating answers is the purpose of a review template, which is why this operation's own doc requires `wellarchitected:GetReviewTemplateAnswer` and `wellarchitected:ListReviewTemplateAnswers` permissions to create a workload from one. (Observable only once a template can hold answers — `UpdateReviewTemplateAnswer` is still a stub — but the copy is missing from the implemented create path.)

## Evidence

Spec: `smithy_query(query="shape", shape="ClientRequestToken")` — "same client request token and the same parameters ... the result of the original request is returned"; `smithy_query(query="operation", operation="CreateWorkload")` lists `DiscoveryConfig` (WorkloadDiscoveryConfig) and `JiraConfiguration` (WorkloadJiraConfigurationInput) as inputs, and models `ConflictException` (409). CreateWorkload doc: "When creating a workload using a review template, you must have the following IAM permissions: wellarchitected:GetReviewTemplate wellarchitected:GetReviewTemplateAnswer wellarchitected:ListReviewTemplateAnswers wellarchitected:GetReviewTemplateLensReview". Handler: `src/ops/create_workload.rs` lines 296-317 build the fingerprint from AccountIds, Applications, ArchitecturalDesign, AwsRegions, Description, Environment, Industry, IndustryType, Lenses, NonAwsRegions, Notes, PillarPriorities, ReviewOwner, ReviewTemplateArns, Tags, WorkloadName — neither `input.discovery_config` nor `input.jira_configuration` appears, yet both are stored on the record at lines 453-454; the replay branch at lines 326-344 then returns the original workload. Lines 392-417 copy `template.lenses` and `template.notes` only; `template.answers` is never read. Test coverage: `tests/all/workload_lifecycle.rs` exercises token replay/conflict over the members that ARE in the fingerprint; `tests/all/review_template_lifecycle.rs` (lines 475-594) asserts only that the template's lenses and notes carry over. Nothing pins the two omitted members or the answer carry-over.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. CreateWorkload with WorkloadName="idem-discovery", Description, Environment=PRODUCTION, ReviewOwner, AwsRegions=["us-west-2"], Lenses=["wellarchitected"], ClientRequestToken="tok-idem-1" and DiscoveryConfig={"TrustedAdvisorIntegrationStatus":"DISABLED"}; note the WorkloadId
3. Repeat the call with the SAME ClientRequestToken="tok-idem-1" but DiscoveryConfig={"TrustedAdvisorIntegrationStatus":"ENABLED"} (and/or a JiraConfiguration block)
4. Observe HTTP 200 returning the first workload's id, with the requested configuration silently dropped; expected HTTP 409 ConflictException because the parameters differ under the same token
5. GetWorkload on that id and observe DiscoveryConfig still reporting DISABLED and no JiraConfiguration
6. For the secondary symptom: CreateReviewTemplate with Lenses=["wellarchitected"], then CreateWorkload with ReviewTemplateArns=[<template arn>] and compare ListAnswers for the new workload against the template's ListReviewTemplateAnswers — the template's answers are not applied to the workload
