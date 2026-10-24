Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# GetBaseline returns a baseline ARN with an account-id segment and a partly invented baseline catalog

_Filed: 2026-10-24T08:24:49Z_
_Operation: GetBaseline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetBaseline rebuilds the ARN it reports through `engine::emulator_arn`, which inserts the emulator's canonical ACCOUNT ID into the ARN's account segment, so the answer looks like `arn:aws:controltower:us-east-1:000000000000:baseline/17BSJV3IGJ2QSGA2`.

Expected (real AWS): a baseline is service-managed data owned by no account, so its ARN carries an EMPTY account segment — `arn:aws:controltower:<region>::baseline/<ID>`, which is the form AWS's own GetBaseline/ListBaselines/EnableBaseline examples use and the reason the model's `BaselineArn` pattern admits `[0-9]{0,12}` (zero digits) in the account position. A client that round-trips the ARN AWS gave it (into EnableBaseline, or into an IAM policy / stored config) therefore sees a different string from the emulator than from AWS.

Secondary symptom of the same handler/catalog: only two of the seven catalog entries carry the baseline IDs AWS publishes in its API examples (`17BSJV3IGJ2QSGA2` = AWSControlTowerBaseline, `LN25R72TTG6IGPTQ` = IdentityCenterBaseline); the IDs behind AuditBaseline, LogArchiveBaseline and the three Backup baselines were invented for this emulator (see the comment at src/state.rs:1172-1175). A caller that passes a real AWS baseline ARN for one of those baselines gets 404 ResourceNotFoundException instead of the baseline's name/description, and the `description` strings are acknowledged paraphrases rather than AWS's wording.

## Evidence

Spec: GetBaseline output `arn` is typed `BaselineArn`, pattern `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$` — the `{0,12}` account segment exists because baseline ARNs carry no account; every AWS GetBaseline/ListBaselines example shows `arn:aws:controltower:<region>::baseline/<ID>`.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_baseline.rs lines 188-198 — `arn: crate::state::baseline_arn(&crate::state::request_region(ctx.parts()), baseline.id)`; src/state.rs:1227-1229 builds it via `::engine::emulator_arn(ARN_NAMESPACE, region, "baseline/<id>")`, which fills in the emulator account id. Catalog: src/state.rs:1176-1219 (`BASELINE_CATALOG`), whose doc comment states the non-example IDs "are this emulator's own".

Tests: tests/all/baseline_catalog.rs:101-141 PINS the current output, asserting the served arn equals `arn:aws:controltower:{REQUEST_REGION}:{emulator account id}:baseline/{id}` — i.e. the account-bearing form is asserted, so no test covers the AWS form. No existing open/fixed issue addresses GetBaseline's ARN form or catalog contents (only the already-fixed GetBaseline stub issue).

## Steps to Reproduce

1. POST /get-baseline with {"baselineIdentifier":"arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2"} against the emulator.
2. Observe the response `arn` contains an account-id segment (`arn:aws:controltower:<request-region>:<account>:baseline/17BSJV3IGJ2QSGA2`), whereas real AWS answers with the account-less `arn:aws:controltower:<region>::baseline/17BSJV3IGJ2QSGA2`.
3. POST /get-baseline with a real AWS baseline ARN for a baseline other than AWSControlTowerBaseline/IdentityCenterBaseline (e.g. the published AuditBaseline ARN) — the emulator answers 404 ResourceNotFoundException because the catalog stores an invented ID for that baseline.
