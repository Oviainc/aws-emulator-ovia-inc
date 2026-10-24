Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# Referenced dataAutomationProjectArn / blueprintArn are never resolved, so a submission naming a project or blueprint that does not exist is accepted

_Filed: 2026-10-24T09:34:26Z_
_Operation: InvokeDataAutomationAsync | Class: validation_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvokeDataAutomationAsync` tells the service WHAT to extract through either `dataAutomationConfiguration.dataAutomationProjectArn` or an inline `blueprints` list. The handler enforces the "exactly one of the two" rule and the modelled `@pattern`/`@length` on each ARN, but it never resolves the ARN it was handed: the project ARN is copied onto the stored record (`invocation.data_automation_project_arn = Some(...)`) and the blueprint list is copied verbatim, with no lookup against any project or blueprint registry.

Expected: a submission naming a data-automation project or a blueprint that does not exist is refused up front with the operation's own `ValidationException` (HTTP 400) — the only client-fault error this operation models, since it declares no `ResourceNotFoundException`; the real service has to resolve the project's standard/custom output configuration (or the blueprint) before it can start a job, so a dangling reference cannot produce a job that runs to completion.

Actual: ANY well-formed project/blueprint ARN is accepted with HTTP 200 and an `invocationArn`, and `GetDataAutomationStatus` then reports that job progressing to `Success`. A client can "successfully" run data automation against a project it never created, or against a project it deleted, and nothing distinguishes that from a real submission.

This is specifically fixable in this repo rather than an inherent limitation: the handler module doc asserts that these ARNs "belong to the data-automation CONTROL plane, which is a different service and is not emulated here", but that control plane IS emulated in this monorepo — `services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice` implements `CreateDataAutomationProject` / `GetDataAutomationProject` / `CreateBlueprint` / `GetBlueprint` (they are not in that crate's scaffold census), and both services mount on the same shared engine, so the runtime can resolve the reference out of that service's store the way other crates in this repo already read a sibling service's state.

## Evidence

Spec: `smithy_query` operation `InvokeDataAutomationAsync` declares errors `AccessDeniedException | InternalServerException | ServiceQuotaExceededException | ThrottlingException | ValidationException` — no `ResourceNotFoundException`, so a bad resource reference is a `ValidationException` on this operation. Shapes: `DataAutomationConfiguration.dataAutomationProjectArn` (required, pattern `...:data-automation-project/[a-zA-Z0-9-_]+$`) and `Blueprint.blueprintArn` (required, pattern `...:blueprint/...`) name resources owned by the Bedrock Data Automation (build-time) service. Handler: `src/ops/invoke_data_automation_async.rs` lines 179-189 only apply `crate::state::extraction_target_refusal` (presence rule), lines 251-256 store `configuration.data_automation_project_arn` and `input.blueprints` with no resolution of either; `src/state.rs` has no project/blueprint registry and its `Invocation` holds the ARNs as opaque strings. Module doc lines 10-20 state the referenced ARNs are "recorded, not resolved" because the control plane "is not emulated here" — contradicted by `services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/` where `create_data_automation_project.rs`, `get_data_automation_project.rs`, `create_blueprint.rs` and `get_blueprint.rs` carry no `//! SCAFFOLDED, NOT IMPLEMENTED:` header. Test coverage: `tests/all/data_automation_invocations.rs` only ever submits a hard-coded `PROJECT_ARN` constant (line 64) and public blueprint ARNs (line 1229 `public_blueprint`) and asserts they are ACCEPTED (`a_submission_must_name_exactly_one_of_a_project_or_blueprints`, lines 1244+); no test creates a project first, and no test asserts anything about an unknown project/blueprint, so the gap is uncovered rather than pinned.

## Steps to Reproduce

1. Mount this service on an engine (no project ever created) and POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and body {"inputConfiguration":{"s3Uri":"s3://in-bucket/assets/doc.pdf"},"outputConfiguration":{"s3Uri":"s3://out-bucket/project/"},"dataAutomationProfileArn":"arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1","dataAutomationConfiguration":{"dataAutomationProjectArn":"arn:aws:bedrock:us-west-2:000000000000:data-automation-project/never-created"}}
2. Observe HTTP 200 with an `invocationArn`; expected a 400 `ValidationException` because no such data-automation project exists
3. Poll `GetDataAutomationStatus` with that `invocationArn`: the job reaches `status: Success`, i.e. a job ran against a project that was never created
4. Repeat with `blueprints: [{"blueprintArn":"arn:aws:bedrock:us-west-2:000000000000:data-automation-blueprint-placeholder"}]` replaced by a well-formed but unknown customer blueprint ARN (`arn:aws:bedrock:us-west-2:000000000000:blueprint/aaaaaaaaaaaa`) and no `dataAutomationConfiguration`: the submission is likewise accepted
5. Grep the crate for any lookup of a project or blueprint (`grep -rn "project" src/state.rs`): the ARNs are stored as strings and never resolved against the emulated build-time service's store
