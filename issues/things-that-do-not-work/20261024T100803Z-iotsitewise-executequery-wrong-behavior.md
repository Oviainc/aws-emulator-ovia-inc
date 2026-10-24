Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# ExecuteQuery refuses JOINs, GROUP BY and aggregate selections with an unmodeled 501 NotImplemented

_Filed: 2026-10-24T10:08:02Z_
_Operation: ExecuteQuery | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ExecuteQuery runs a real SQL engine over the store, but the planner refuses whole classes of statement the IoT SiteWise query language supports, and it refuses them with the scaffold error `NotImplemented` (HTTP 501), which is not in the operation's modeled error set.

Expected: a statement such as `SELECT a.asset_name, p.property_name, l.double_value FROM latest_value_time_series l JOIN asset_property p ON l.property_id = p.property_id JOIN asset a ON p.asset_id = a.asset_id` — the shape of the examples AWS publishes for the SiteWise query language — returns rows; `SELECT COUNT(*) FROM asset`, `GROUP BY`, `HAVING`, an `ORDER BY` over an expression and a `SELECT` of anything other than a bare column likewise compute a result; the documented `precomputed_aggregates` view returns rows. A statement the service genuinely cannot run is answered with one of the modeled errors (`InvalidRequestException` 400, `QueryTimeoutException`, `ValidationException`).

Actual: `plan()`/`materialise()` call the local `unsupported(..)` helper, which returns `ExecuteQueryError::NotImplemented` (HTTP 501, code `NotImplemented`) for: a JOIN (line 966), GROUP BY (line 935), HAVING (915), a selected expression that is not a column (1775), ORDER BY over a non-column key (1580), DISTINCT (891), WITH (838), LIMIT BY (1699), and the `precomputed_aggregates` view (1142). Since `NotImplemented` is not a modeled error for ExecuteQuery, no generated SDK exception class matches it: a client's `except InvalidRequestException` / `except QueryTimeoutException` never fires and a perfectly ordinary query looks like a server-side refusal.

## Evidence

Smithy model: ExecuteQuery's error set is AccessDeniedException(403), InternalFailureException(500), InvalidRequestException(400), QueryTimeoutException(400), ServiceUnavailableException(503), ThrottlingException(429), ValidationException(400) — `NotImplemented` is not a shape of com.amazonaws.iotsitewise at all. Handler: services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/execute_query.rs, `fn unsupported` (lines 1836-1846) plus its ~30 call sites listed above; the view catalog in `materialise` (lines 1054-1150) only computes asset, asset_property, raw_time_series and latest_value_time_series. Well-established service behavior: the SiteWise SQL reference and its published example queries join asset / asset_property / raw_time_series / latest_value_time_series and use aggregate functions with GROUP BY. Tests: tests/all/execute_query.rs and execute_query_follows_the_store.rs exercise single-view SELECT/WHERE/ORDER BY only — no test asserts a JOIN or an aggregate.

## Steps to Reproduce

1. Boot the emulator and point an iotsitewise client at it.
2. CreateAssetModel with a measurement property, CreateAsset from it, BatchPutAssetPropertyValue a few TQVs.
3. ExecuteQuery with `SELECT a.asset_name, p.property_name FROM asset a JOIN asset_property p ON a.asset_id = p.asset_id` -> HTTP 501 code NotImplemented ('operation ExecuteQuery does not implement a JOIN in this emulator') instead of rows.
4. ExecuteQuery with `SELECT COUNT(*) FROM asset` -> 501 NotImplemented ('a selected expression that is not a column').
5. ExecuteQuery with `SELECT asset_model_id, COUNT(*) FROM asset GROUP BY asset_model_id` -> 501 NotImplemented ('a GROUP BY clause').
6. ExecuteQuery with `SELECT * FROM precomputed_aggregates` -> 501 NotImplemented.
