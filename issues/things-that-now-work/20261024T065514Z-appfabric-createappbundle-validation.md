Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# CreateAppBundle lets an account hold many app bundles, while AppFabric allows one per Region

_Filed: 2026-10-24T06:55:14Z_
_Operation: CreateAppBundle | Class: validation_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AppFabric's published service quota is ONE app bundle per AWS account per Region (not adjustable), and the AppFabric console/API refuse a second CreateAppBundle in the same Region with the modeled ServiceQuotaExceededException (HTTP 402). The emulator instead allows up to its internal resource guard of 1000 bundles (crate::state::MAX_APP_BUNDLES), so a second CreateAppBundle returns HTTP 201 and persists a second bundle — a state no real account can reach.

Expected: the first CreateAppBundle in a Region succeeds (201); a second one, while the first still exists, is refused with ServiceQuotaExceededException (HTTP 402), and succeeds again only after DeleteAppBundle removed the first. Actual: every call up to 1000 succeeds, so ListAppBundles can report many bundles and a client's quota-handling path is never exercised.

## Evidence

Spec: CreateAppBundle models ServiceQuotaExceededException (HTTP 402) — "The request exceeds a service quota" (OPERATIONS.md, CreateAppBundle); well-established AppFabric quota of 1 app bundle per account per Region. The repo's own trace-derived issue 20261023T031706Z-appfabric-8-behavioral-divergences... states it in its reproduction notes: "AppFabric enforces one app bundle per account/region so clear any existing bundle first."
Handler: src/ops/create_app_bundle.rs lines 182-192 checks only `store.app_bundles.len() >= crate::state::MAX_APP_BUNDLES`, with MAX_APP_BUNDLES = 1_000 documented in src/state.rs line 54 as an emulator resource guard rather than the AWS quota.
Test coverage: tests/all/app_bundle_ingestion_lifecycle.rs `a_create_past_the_store_ceiling_is_refused` (lines 1282-1308) asserts the 1000-record guard; no test asserts the per-Region quota of one bundle, and several tests create two or more bundles.

## Steps to Reproduce

1. POST /appbundles with an empty body -> 201, note appBundle.arn
2. POST /appbundles again with an empty body (different or absent clientToken)
3. Observe HTTP 201 and a second bundle; real AppFabric answers HTTP 402 ServiceQuotaExceededException because the account already holds its one app bundle for the Region
4. GET /appbundles -> the emulator lists two bundles where AWS can only ever list one
