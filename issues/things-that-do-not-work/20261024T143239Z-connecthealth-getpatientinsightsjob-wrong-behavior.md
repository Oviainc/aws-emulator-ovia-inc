Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Every patient insights job ends in FAILED, so SUCCEEDED and insightsOutput are unreachable

_Filed: 2026-10-24T14:32:39Z_
_Operation: GetPatientInsightsJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A patient insights job started with StartPatientInsightsJob always walks SUBMITTED -> IN_PROGRESS -> FAILED in this emulator, with a `statusDetails` saying insights generation is unavailable, and `insightsOutput` is never populated. Expected: a job whose request is accepted and whose input/output configuration is valid reaches the modelled terminal success status `SUCCEEDED` and reports an `insightsOutput.uri` under the `outputDataConfig.s3OutputPath` the start supplied (the model declares `JobStatus` = SUBMITTED | IN_PROGRESS | FAILED | SUCCEEDED and `InsightsOutput.uri` @required, i.e. a successful run is the normal outcome). Actual: the happy path of the only asynchronous workflow this service has — start a job, poll GetPatientInsightsJob until a terminal status, then read the output location — can never be exercised; every poll terminates in FAILED, so a client that treats FAILED as an error (the normal client contract) always errors out. A client-visible difference on both the `jobStatus` and the `insightsOutput`/`statusDetails` members of a 200 response.

## Evidence

Spec: smithy_query shape=JobStatus lists SUCCEEDED as a modelled terminal status and shape=InsightsOutput models a @required `uri`; GetPatientInsightsJobOutput declares `insightsOutput` and `statusDetails`. Handler/state: `src/state.rs:409-436` (`PatientInsightsJob::apply_due_transitions`) hardcodes `JobStatus::InProgress => { self.status = JobStatus::Failed; self.status_details = Some(PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE) }`, and `src/state.rs:97-109` documents the choice ("Why every patient insights job this emulator accepts ends in FAILED"); `src/ops/start_patient_insights_job.rs:306-321` stores `insights_output: None` and never writes one. Test coverage: `tests/all/patient_insights_job_lifecycle.rs:373-386` asserts the terminal status is FAILED and that no `insightsOutput` is present, and `tests/all/patient_insights_job_lifecycle.rs:664-666` states in-test that SUCCEEDED is a status "this emulator's own lifecycle never reaches" (it is only reached by seeding state directly), so the success path is unimplemented rather than merely untested.

## Steps to Reproduce

1. POST /domain with a name to create a domain and note the returned domainId.
2. POST /domain/{domainId}/patient-insights-job with valid patientContext, insightsContext, encounterContext, userContext, inputDataConfig and outputDataConfig (e.g. s3OutputPath s3://clinic-output/run1/); note the jobId.
3. Poll GET /domain/{domainId}/patient-insights-job/{jobId} until jobStatus is terminal (two PATIENT_INSIGHTS_JOB_STEP_SPAN steps, ~1s).
4. Observe jobStatus = FAILED with statusDetails "Patient insights generation is not available in this emulator..." and no insightsOutput member — expected is jobStatus SUCCEEDED with insightsOutput.uri pointing under the requested s3OutputPath.
