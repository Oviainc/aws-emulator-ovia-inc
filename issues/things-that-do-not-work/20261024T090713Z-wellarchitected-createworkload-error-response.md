Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# CreateWorkload answers 404 "profile not found" for a ProfileArns entry that CreateProfile actually stored

_Filed: 2026-10-24T09:07:13Z_
_Operation: CreateWorkload | Class: error-response_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkloadInput$ProfileArns` (`@length(max: 1)`) is the documented way to apply a Well-Architected profile to a workload at create time. `CreateProfile` is implemented in this emulator: it mints a real `ProfileArn` (`arn:…:wellarchitected:<region>:<account>:profile/<32 hex>`), stores the record, and `GetProfile` resolves it with 200.

But `CreateWorkload` refuses ANY `ProfileArns` entry unconditionally, before the store is even consulted, with the modelled `ResourceNotFoundException` (HTTP 404) whose message says "profile with ARN <arn>" was not found — about a profile the same emulator just created and will happily return from `GetProfile`.

Expected (real service): HTTP 200, the workload is created, and `GetWorkload` reports `Workload$Profiles` carrying that `ProfileArn` with the profile's `ProfileVersion` (and the lens reviews gain `PrioritizedRiskCounts` for the questions the profile prioritises).

Actual: HTTP 404 `ResourceNotFoundException` naming an existing profile as missing, and no workload is created at all — so the documented "create a profile, then create a workload with it" workflow fails, and it fails with an error that is factually false about the store's contents. At minimum the refusal must not claim a stored profile does not exist; correctly, the create should succeed and record the association.

Note this is distinct from the already-open `CreateWorkload` idempotency-fingerprint issue (`20261024T045731Z-wellarchitected-createworkload-wrong-behavior`) and from the `AssociateProfiles` stub issue: `AssociateProfiles` is the separate attach-after-create path, while this is the implemented create path rejecting valid input it accepted into its own store moments earlier.

## Evidence

Smithy model: `smithy_query(query="operation", operation="CreateWorkload")` lists `ProfileArns` (target `WorkloadProfileArns`, `@length(max: 1)`) as an input of `CreateWorkloadInput`, and the `Workload` output shape declares `Profiles` (list of `WorkloadProfile{ProfileArn, ProfileVersion}`) and `PrioritizedRiskCounts` — i.e. a workload created with a profile is modelled to report it. `CreateProfile`'s own output is `{ProfileArn, ProfileVersion}`, and `GetProfile` resolves that ARN out of `state.profiles`.

Handler: `src/ops/create_workload.rs` lines 299-315 — `if let Some(arn) = input.profile_arns.unwrap_or_default().first() { return Err(CreateWorkloadErrorModeled::ResourceNotFoundException(crate::state::resource_not_found(crate::state::PROFILE_RESOURCE_TYPE, arn.clone(), &format!("profile with ARN {arn}"))).into()); }` — no lookup against `store.profiles` happens; every ARN, valid or not, is reported as absent. `src/state.rs` `to_workload` (lines 1406-1410) correspondingly hardcodes `profiles: None` / `prioritized_risk_counts: None`.

Test coverage: `tests/all/profile_lifecycle.rs` covers `CreateProfile`/`GetProfile` only; `tests/all/workload_lifecycle.rs` and `tests/all/workload_computed_members.rs` never send `ProfileArns`, so nothing asserts this path either way.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. POST /profiles (CreateProfile) with ProfileName, ProfileDescription, ProfileQuestions=[{"QuestionId":"PRIORITY","SelectedChoiceIds":["choice-1"]}] and ClientRequestToken="tok-profile"; note the returned ProfileArn
3. GET /profiles/{ProfileArn} (GetProfile) and observe HTTP 200 — the profile exists in the store
4. POST /workloads (CreateWorkload) with WorkloadName, Description, Environment=PRODUCTION, ReviewOwner, AwsRegions=["us-west-2"], Lenses=["wellarchitected"], ClientRequestToken="tok-workload" and ProfileArns=["<the ProfileArn from step 2>"]
5. Observe HTTP 404 ResourceNotFoundException claiming the profile with that ARN was not found, and no workload created; expected HTTP 200 with a WorkloadId, and a subsequent GET /workloads/{WorkloadId} reporting Workload.Profiles = [{ProfileArn, ProfileVersion}]
