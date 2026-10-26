Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# Environment$checks never reports CONNECTOR_HEALTH, and the connector records report it as a per-connector result instead of the documented aggregate

_Filed: 2026-10-26T23:25:24Z_
_Operation: GetEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Check$type` — the enum member of the ENVIRONMENT check shape (`Check` is "A check on the environment to identify environment health and validate VMware VCF licensing compliance") — documents `CONNECTOR_HEALTH` as "Aggregate health across all connectors in the environment". Its subject is therefore the environment and its value is a roll-up over every connector the environment holds.

The crate does the opposite twice over:
1. `StoredEnvironment::refresh_checks` builds the environment's `checks` from a fixed three-entry table (KEY_REUSE, KEY_COVERAGE, HOST_COUNT), so `Environment$checks` as reported by GetEnvironment, CreateEnvironment and DeleteEnvironment never contains a CONNECTOR_HEALTH entry, even for an environment holding ACTIVE or failed connectors — and `environmentStatus`, the roll-up of `checks`, therefore ignores connector health entirely.
2. `StoredConnector::checks` puts a `CONNECTOR_HEALTH` entry on EACH connector carrying that single connector's own result, so the one place the type does appear is not an aggregate across the environment's connectors at all: two connectors of one environment can report contradictory CONNECTOR_HEALTH results (one PASSED, one FAILED) while the environment reports no CONNECTOR_HEALTH check.

Everything the aggregate needs already exists in the crate: `StoredConnector::decided_result()`/`roll_up()` compute a connector's health, `refresh_checks` already assembles the environment's check list, and `environment_status()` already rolls checks up the same way.

## Evidence

Spec (`smithy_query(query="shape", shape="Check")`, member `type`): "The check type. Amazon EVS performs the following checks: … <code>CONNECTOR_HEALTH</code>: Aggregate health across all connectors in the environment." `Check` itself: "A check on the environment to identify environment health and validate VMware VCF licensing compliance."

Code — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1764-1768, the complete environment check table:
```
        let results = [
            (CHECK_TYPE_KEY_REUSE, key_reuse),
            (CHECK_TYPE_KEY_COVERAGE, key_coverage),
            (CHECK_TYPE_HOST_COUNT, host_count),
        ];
```
and state.rs:71-74, which states the contradicting rule outright: "Of the other ten types, FOUR have a connector for their subject rather than the environment's own record — `CONNECTOR_HEALTH` and the three per-appliance reachability checks — and are reported on the connector they are about, decided by that connector's own stored record ([`StoredConnector::checks`])." Same claim at state.rs:210-215: "The `CheckType` of the check run on EVERY connector, whatever appliance type it connects to" / `pub const CHECK_TYPE_CONNECTOR_HEALTH: &str = "CONNECTOR_HEALTH";`.
The per-connector emission is state.rs:763-782: `let Some(result) = self.decided_result() else { return Vec::new(); };` … `[Some(CHECK_TYPE_CONNECTOR_HEALTH), reachability_check_type(&self.connector_type)]` … `result: Some(result.to_string())` — one connector's own state, labelled with the aggregate's type.
The reporting path: src/ops/get_environment.rs:166-182 reads the refreshed record (`state.materialise_due_transitions();` → `recompute_environment_checks`) and answers `environment.to_wire()`, whose `checks` is state.rs:1816 `checks: (!self.checks.is_empty()).then(|| self.checks.clone())`.

Test coverage: tests/all/environment_health_checks.rs:64-73 pins exactly the three types (`fn reported_check_types()` returning KEY_REUSE, KEY_COVERAGE, HOST_COUNT, with the comment "Read from the service's own catalog, so a check type added there fails this suite rather than passing unnoticed"), and tests/all/connector_health_checks.rs:74/90 pins the per-connector `CONNECTOR_HEALTH` entry (`vec![CONNECTOR_HEALTH, reachability_check_type(connector_type)]`). No test asserts an environment-level CONNECTOR_HEALTH check.

## Steps to Reproduce

1. Register a VPC and service access subnet in the EC2 store and CreateEnvironment with a provided vcfVersion (so the environment settles to CREATED).
2. Call CreateEnvironmentConnector twice for the same environment with two different types (e.g. VCENTER and OPERATIONS_MANAGER) and wait for both to reach ACTIVE.
3. Call GetEnvironment and inspect `environment.checks`: it holds exactly KEY_REUSE, KEY_COVERAGE and HOST_COUNT — no CONNECTOR_HEALTH entry — although the model documents CONNECTOR_HEALTH as "Aggregate health across all connectors in the environment".
4. Call ListEnvironmentConnectors: each connector carries its OWN CONNECTOR_HEALTH entry, so the only CONNECTOR_HEALTH results in the service are per-connector rather than the aggregate, and they disagree with one another as soon as one connector's state differs from the other's.
