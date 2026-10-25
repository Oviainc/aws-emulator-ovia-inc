Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 43c3b52fd22125f2604949f65e29a8c5612a98db

# A DeploymentFilter that names no filter key, or carries an empty values list, is silently ignored and the unfiltered deployment set is returned

_Filed: 2026-10-24T07:07:13Z_
_Operation: ListDeployments | Class: validation_

> _Note: this issue was identified against the code base at commit `43c3b52fd22125f2604949f65e29a8c5612a98db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`matches_filter` treats two degenerate filters as "matches everything":
- a filter whose `values` is absent or an empty list (`None | Some([]) => return true`), and
- a filter whose `name` is absent (the `_ => return true` arm; the name-vocabulary check earlier in the handler also `continue`s past a filter with no name).

So `ListDeployments(filters=[{"name":"WORKLOAD_NAME"}])` or `ListDeployments(filters=[{"values":["SAP"]}])` answers HTTP 200 with EVERY stored deployment — the caller's constraint is dropped and the response is broader than what was asked for, with nothing on the wire to say the filter was ignored.

Expected: a filter pair that is not a name-and-values pair is refused with the modeled ValidationException (HTTP 400) — DeploymentFilter is documented as "A filter name and value pair that is used to return more specific results", and a half-specified filter cannot narrow anything. Actual: HTTP 200 with the unfiltered result set, so a client that builds a filter programmatically and omits one half silently receives deployments it filtered out.

## Evidence

Spec: Smithy model com.amazonaws.launchwizard#DeploymentFilter — "A filter name and value pair that is used to return more specific results from a describe operation"; `name` is a DeploymentFilterKey (WORKLOAD_NAME | DEPLOYMENT_STATUS) and `values` are documented as OR-joined values; ListDeployments declares ValidationException (HTTP 400).
Handler: services/service-com-amazonaws-launchwizard-launchwizard/src/ops/list_deployments.rs — `fn matches_filter` (around lines 210-221) returns `true` for `None | Some([])` values and `true` for any filter whose `name` is absent; the vocabulary loop above it (around lines 159-174) only validates names that are PRESENT (`let Some(name) = filter.name.as_deref() else { continue; }`), so neither half of a half-specified filter is ever refused.
Tests: tests/all/deployment_lifecycle.rs::list_deployments_pages_and_filters_the_stored_deployments exercises only well-formed filters (and refuses unknown filter NAMES); tests/all/validation_messages.rs covers only element-constraint violations of `filters.*.values`. No test covers an empty `values` list or a missing `name`, so nothing pins the current pass-through as intended.

## Steps to Reproduce

1. Create two deployments against different workloads (e.g. workloadName='SAP' and workloadName='SQLServer').
2. Call ListDeployments(filters=[{"name":"WORKLOAD_NAME","values":[]}]).
3. Expect a ValidationException (HTTP 400) for a filter that carries no values; observe HTTP 200 listing BOTH deployments (the filter was ignored).
4. Call ListDeployments(filters=[{"values":["SAP"]}]) (no filter name).
5. Expect a ValidationException naming the missing filter name; observe HTTP 200 listing both deployments again.
