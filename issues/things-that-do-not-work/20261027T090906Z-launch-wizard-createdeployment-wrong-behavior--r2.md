Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c15dfa091c78a47417078a336c74dcf21189cb0f

# A dryRun CreateDeployment at the store ceiling answers DryRunOperation instead of the ResourceLimitException the real create gets

_Filed: 2026-10-27T09:09:06Z_
_Operation: CreateDeployment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c15dfa091c78a47417078a336c74dcf21189cb0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `dryRun` short-circuit in `CreateDeployment` is placed ABOVE the only remaining refusal the handler can raise — the `ResourceLimitException` that `make_room_for_a_deployment()` produces when the store is full of live records. The handler's own comment states the rule the placement is supposed to satisfy ("a pre-flight of a request this service would refuse reports that refusal"), and the crate's dry-run test suite states the same rule as the contract under test. The code under that comment breaks it: with the store at its ceiling and no already-deleted record to evict, a `dryRun: true` create returns the pre-flight error `DryRunOperation` (HTTP 412) — i.e. "the real request would have succeeded" — while the same body with `dryRun: false` is refused 400 `ResourceLimitException`. The pre-flight therefore reports the wrong outcome for exactly the case it exists to warn about.

The fix is not to move the mutating `make_room_for_a_deployment()` above the short-circuit (a pre-flight must persist nothing, including the eviction of deleted records); it is to decide "would this create fit?" read-only before the short-circuit and raise the modelled `ResourceLimitException` from there, leaving the eviction where it is for the performing path.

## Evidence

Handler comment stating the rule, immediately followed by the short-circuit — services/service-com-amazonaws-launchwizard-launchwizard/src/ops/create_deployment.rs:296-301:
```
        // `state::dry_run_operation`). Answered after every check above, so a
        // pre-flight of a request this service would refuse reports that refusal, and
        // before anything is written, so it persists NOTHING.
        if input.dry_run {
            return Err(crate::state::dry_run_operation());
        }
```
The refusal that sits BELOW it, so a pre-flight never reports it — src/ops/create_deployment.rs:303-317:
```
        // The store lives as long as the process, so it has a ceiling; see
        // `state::MAX_DEPLOYMENTS` for how room is made and why this is the error the
        // service answers when there is none.
        if !store.make_room_for_a_deployment() {
            return Err(CreateDeploymentErrorModeled::ResourceLimitException(
```
The same rule stated as the contract under test — tests/all/dry_run_preflight.rs:17-18:
```
//! 2. a pre-flight of a request the service would REFUSE reports that refusal instead
//!    — the pre-flight answers the outcome the real request would have had;
```
That suite's refusal table (tests/all/dry_run_preflight.rs:244-247, 255-314) enumerates "One row per reason this service refuses a create: a workload and a pattern the catalogue does not publish, a settings map the pattern's own schema warns about ..., and a name a live deployment already holds" — the resource-limit refusal is the one reason it does not cover, so nothing pins this path.
That the performing path IS refused at the ceiling — tests/all/deployment_lifecycle.rs:905-925:
```
    // The store itself is bounded: filling it and asking for one more is the modelled
    // resource-limit error, not unbounded growth.
...
            create_body(&harness, "SAP", "SapHanaSingle", "one-too-many", false).await,
            400,
            "ResourceLimitException",
```
(the `false` is `dryRun`, so only the performing path is covered).

## Steps to Reproduce

1. Start the emulator and create `state::MAX_DEPLOYMENTS` (200) deployments against `SAP`/`SapHanaSingle`, deleting none, as tests/all/deployment_lifecycle.rs:907-916 does.
2. POST /createDeployment with a conforming body and `"dryRun": false` — observe HTTP 400 `ResourceLimitException` (the refusal the real request gets).
3. POST /createDeployment with the SAME body and `"dryRun": true` — observe HTTP 412 `DryRunOperation`, the pre-flight's "you have the permissions, the request would be made" answer.
4. Compare with the rule quoted at src/ops/create_deployment.rs:296-298 and tests/all/dry_run_preflight.rs:17-18: the pre-flight must report the refusal the real request would have had.
