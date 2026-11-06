Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# A bad updateFromVersion filter value is refused with DeploymentFilterValue's pattern, not DeploymentPatternVersionFilterValue's

_Filed: 2026-10-28T11:49:57Z_
_Operation: ListDeploymentPatternVersions | Class: error-response_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every boundary refusal of this service is worded by `crate::state::validation_report`, which reports a failing list ELEMENT at its collection with that collection's composed element constraints, looked up in `LIST_ELEMENT_CONSTRAINTS` by the violation path with the collection steps stripped (`member_trail`). Both `ListDeployments$filters[].values` and `ListDeploymentPatternVersions$filters[].values` reduce to the SAME trail `"filters.values"`, and the table holds only the `ListDeployments` entry — so a `ListDeploymentPatternVersions` filter value that breaks `DeploymentPatternVersionFilterValue`'s pattern is refused with the requirement set of a *deployment* filter value. The caller is told to satisfy `(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)` — an expression that NO legal `updateFromVersion` value (a dotted version such as `1.0.0`) can ever match — instead of the dotted-version pattern the member actually declares. The table's own doc asserts the opposite of what the model says, and the handler that reads this member quotes the real pattern two files away, so the crate contradicts itself as well as the model.

## Evidence

Model (smithy_query shape=DeploymentPatternVersionFilterValues): member `{"target": "DeploymentPatternVersionFilterValue", "length": {"min": 1, "max": 100}, "pattern": "^(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)$"}` — a constrained element of an INPUT list (`ListDeploymentPatternVersions$filters` -> `FilterList` -> `DeploymentPatternVersionFilter$values`).

But services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:2648-2653 claims: "Source: `smithy_query(query=\"closure\",operation=\"ListDeployments,UntagResource\" ...)`. Those two operations hold the only input lists in the model whose element shape is constrained; every other input list (`DeploymentFilterList`) carries a structure element whose own members are reported under their own names." — and the table therefore holds exactly two entries, state.rs:2654-2664:
```
    // `DeploymentFilter$values` -> `DeploymentFilterValues`, element
    // `DeploymentFilterValue`: @length 1..100, @pattern `(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)`.
    (
        "filters.values",
        ElementConstraints { max_length: Some(100), min_length: Some(1),
            pattern: Some(r"(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)"), },
    ),
```
The same crate's own handler contradicts that claim — services/service-com-amazonaws-launchwizard-launchwizard/src/ops/list_deployment_pattern_versions.rs:288-290: "its values are version numbers (`DeploymentPatternVersionFilterValue` is `^(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)$`)".

The collision is in the lookup key: state.rs:2691 `fn member_trail(path: &str) -> String` strips the collection steps (`filters.1.member.values` -> `filters.values`), and state.rs:2681-2687 `element_constraints` returns the FIRST entry whose member equals that trail; state.rs:2556-2569 then renders max_length, min_length and `composed_pattern(pattern)` from it. The refusal path is wired for every operation: src/service.rs:114-116 `Refusal::Invalid(violations) ... message: crate::state::validation_report(violations)`.

Test coverage: tests/all/validation_messages.rs:220-222 pins `FILTER_VALUE_CONSTRAINTS` and tests/all/validation_messages.rs:293-299 exercises it only against `"/listDeployments"` — no row covers `/listDeploymentPatternVersions`, so the wrong wording is unpinned and undetected.

## Steps to Reproduce

1. POST /listDeploymentPatternVersions with body {"workloadName": "SAP", "deploymentPatternName": "SapHanaSingle", "filters": [{"name": "updateFromVersion", "values": ["1.0"]}]} (a value that breaks DeploymentPatternVersionFilterValue's dotted-version pattern).
2. Observe the 400 ValidationException message: "1 validation error detected: Value at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [... Member must satisfy regular expression pattern: .*(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$).*]".
3. Expected: the composed requirement of the element shape this member actually declares, i.e. the pattern `.*^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$.*`. Actual: the pattern of DeploymentFilterValue, which no valid updateFromVersion value can satisfy.
4. Compare with POST /listDeployments {"filters": [{"name": "WORKLOAD_NAME", "values": ["not a valid name!!"]}]}, which is the one case tests/all/validation_messages.rs covers and which gets the same (there correct) wording.
