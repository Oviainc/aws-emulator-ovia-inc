Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Validate EnableBaseline's baselineIdentifier, baselineVersion, targetIdentifier and parameters instead of persisting anything ARN-shaped

_Filed: 2026-10-24T06:10:10Z_
_Operation: EnableBaseline | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

EnableBaseline persists whatever identifiers, version and parameters the request carries, without checking any of them against the Control Tower baseline catalog or the organization, so requests real AWS rejects with a 4xx answer 200 and create an EnabledBaseline resource that could not exist in AWS.

Expected (real AWS): `baselineIdentifier` must name a baseline from the service's fixed catalog (the ARNs `ListBaselines`/`GetBaseline` return, of the form `arn:aws:controltower:<region>::baseline/<16 uppercase alnum>` — AWSControlTowerBaseline, IdentityCenterBaseline, AuditBaseline, BackupBaseline, …). An ARN that is not a baseline ARN (e.g. `arn:aws:s3:::some-bucket-name-x`, or a well-formed-looking `…:baseline/notacatalogentry`) is refused with ValidationException (400), and a syntactically valid baseline ARN that names no baseline is refused (ValidationException / ResourceNotFoundException). `baselineVersion` must be a version that baseline actually offers (e.g. `4.0`); an arbitrary value such as `10.10.1000` is refused. `targetIdentifier` must name an OU/account registered with the organization; an ARN that is not an Organizations target, or a target that does not exist, is refused with ValidationException/ResourceNotFoundException. `parameters` keys must be parameters the baseline declares (e.g. `IdentityCenterEnabledForBaseline` for AWSControlTowerBaseline); an unknown key is refused.

Actual (emulator): `src/ops/enable_baseline.rs` does no such check — the handler's own comment states "the identifier and version are persisted as given and reported back unchanged rather than checked against a catalog this service cannot yet consult". The only checks are the front door's generic `Arn` length/pattern (`^arn:aws[0-9a-zA-Z_\-:\/]+$`), a duplicate baseline/target-pair conflict, and the enablement-count quota. Everything else is written into `state.enabled_baselines` verbatim (`record_enablement`, src/state.rs:589-623) and reported back by `GetBaselineOperation` as a SUCCEEDED ENABLE_BASELINE operation, so an enablement of a non-existent baseline on a non-existent target becomes persisted state and the account's enablement quota is consumed by it.

## Evidence

Smithy model: EnableBaseline (POST /enable-baseline) declares ValidationException (400) and ResourceNotFoundException (404) among its errors; `baselineIdentifier`/`targetIdentifier` are typed as the generic `Arn` (len 20..2048, pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$), i.e. the wire type cannot express "is a baseline" and the handler must perform the semantic check. The model's own `BaselineArn` (used by GetBaseline$baselineIdentifier, pattern `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`) and the `ListBaselines`/`GetBaseline` pair establish that baselines are a fixed, service-owned catalog, not caller-chosen ARNs; EnabledBaselineDetails$baselineIdentifier/$baselineVersion are reported back from this stored value.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/enable_baseline.rs lines 188-243 — duplicate-pair check, then `store.record_enablement(&arn, &input.baseline_identifier, &input.baseline_version, &input.target_identifier, parameters, now)` with no catalog/target/parameter validation (see the comment at lines 217-221); src/state.rs:589-623 stores the values as given. Note EnableBaseline IS implemented here (it computes from and writes persisted state; it carries no `//! SCAFFOLDED, NOT IMPLEMENTED:` header), despite an older open stub issue filed against it.

Tests: services/.../tests/all/baseline_operations.rs drives `/enable-baseline` with realistic-looking but arbitrary baseline ARNs and versions (`baseline/QQ1234567890ABCD`, version `10.10.1000`) and asserts they round-trip; no test asserts that an identifier outside the baseline catalog, an unsupported baselineVersion, a non-Organizations target or an unknown parameter key is refused. The existing backlog has no EnableBaseline validation issue (only a stale stub issue).

## Steps to Reproduce

1. POST /enable-baseline with {"baselineIdentifier":"arn:aws:s3:::my-example-bucket-name","baselineVersion":"4.0","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"} — the emulator answers 200 with an EnabledBaseline `arn` and an `operationIdentifier`; AWS refuses the request because that ARN names no baseline.
2. POST /get-baseline-operation with the returned operationIdentifier and wait past the settle span — the emulator reports operationType ENABLE_BASELINE, status SUCCEEDED, i.e. the bogus enablement is now persisted state (and counts against the enablement quota).
3. POST /enable-baseline with a catalog-shaped baseline ARN but an unsupported version, e.g. {"baselineIdentifier":"arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2","baselineVersion":"10.10.1000","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-bb22-2b"} — emulator answers 200; AWS refuses the unsupported baseline version.
4. POST /enable-baseline with a targetIdentifier that is not an Organizations OU/account at all, e.g. {"baselineIdentifier":"arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2","baselineVersion":"4.0","targetIdentifier":"arn:aws:iam::123456789012:role/not-a-target"} — emulator answers 200 and stores the enablement; AWS refuses with a validation/not-found failure.
5. POST /enable-baseline with {"parameters":[{"key":"NoSuchParameter","value":true}]} on an otherwise valid baseline/target pair — emulator answers 200 and stores the unknown parameter; AWS refuses an undeclared parameter key.
