Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# Refused mutations emit no audit event although the crate's audit contract says attempted mutations are recorded

_Filed: 2026-10-27T08:18:35Z_
_Operation: CreateSuiteDefinition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::audit` documents the crate-wide rule that every mutating operation emits exactly one audit event saying how the call ended, and that an ATTEMPTED mutation is recorded too with the outcome that refused it. Only `UpdateSuiteDefinition` follows that rule on a refusal path; every other mutating handler returns its error before reaching any `audit` call, so a refused attempt leaves no record at all (zero events, not one).

CreateSuiteDefinition has three such early returns (configuration violation, clientToken reused with different parameters, store occupancy bound) and audits only on success and on idempotent replay. The same omission is in StartSuiteRun (unknown definition, unknown version, ConflictException), TagResource (unknown resourceArn, tag limit), UntagResource (unknown resourceArn) and StopSuiteRun (unknown run) — while DeleteSuiteDefinition goes out of its way to audit even its no-op ("absent") outcome. The audit trail the state module advertises as what makes a destructive or attempted call attributable after the fact therefore silently loses every refused attempt.

## Evidence

The rule, in services/.../src/state.rs:783-791:
  "/// Every mutating operation here emits exactly one event on the `emulator::audit` target:
   /// who presented which access key id, in which region, which operation, which resource,
   /// and how the call ended (the subscriber stamps the time). ...
   ///
   /// An ATTEMPTED mutation is recorded too, with the outcome that refused it (an update of
   /// an id the store does not hold, say): `resource` is then the ARN the request NAMED, not
   /// a claim that such a resource is stored."

The sibling that honours it — services/.../src/ops/update_suite_definition.rs:196-212:
  "let Some(definition) = store.definitions.get_mut(&input.suite_definition_id) else {
       crate::state::audit(Self::NAME, ctx.parts(), &region, &requested_arn, \"not-found\");
       return Err(UpdateSuiteDefinitionErrorModeled::ResourceNotFoundException(..."

The handler that skips it — services/.../src/ops/create_suite_definition.rs:162-169 (and again at 188-198 and 218-228), returning with no audit call:
  "if let Some(violation) = crate::state::definition_configuration_violation(&config) {
       return Err(CreateSuiteDefinitionErrorModeled::ValidationException(
           ValidationException { message: Some(violation) },
       )
       .into());
   }"
The only `audit` calls in that file are on the replay path (create_suite_definition.rs:200) and the success path (create_suite_definition.rs:264).

A whole-crate grep for `state::audit` shows eight call sites across eight mutating handlers, of which exactly one (update_suite_definition.rs:197) sits on a refusal path; StartSuiteRun's three refusals (start_suite_run.rs:199-205, 212-219, 241-259), TagResource's two (tag_resource.rs:161-171, 175-188), UntagResource's one (untag_resource.rs:167-177) and StopSuiteRun's not-found (stop_suite_run.rs:145-154) all return before their single audit call at start_suite_run.rs:300 / tag_resource.rs:190 / untag_resource.rs:186 / stop_suite_run.rs:166. DeleteSuiteDefinition, by contrast, audits even a no-op: "let mut outcome = \"absent\";" (delete_suite_definition.rs:151) then "crate::state::audit(Self::NAME, ctx.parts(), &region, &arn, outcome);" (delete_suite_definition.rs:171). No test in tests/all/ asserts anything about the audit target, so nothing pins the current gap.

## Steps to Reproduce

1. Run the emulator with the `emulator::audit` tracing target enabled.
2. POST /suiteDefinitions with a suiteDefinitionConfiguration whose rootGroup is "" and intendedForQualification is false → 400 ValidationException; no audit event is emitted.
3. POST /suiteDefinitions twice with the same clientToken but different tags → the second call is 400 ValidationException ("clientToken was already used..."); no audit event is emitted.
4. For contrast, PATCH /suiteDefinitions/00000000-0000-4000-8000-000000000000 with a legal configuration → 404 ResourceNotFoundException AND one audit event with outcome "not-found" (update_suite_definition.rs:197), which is what state.rs:789-791 says every attempted mutation does.
