Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1b03c6e9884bdbb560f42766af0f40efd2be6453

# UpdateDeployment accepts a pattern version the crate's own update-target rule excludes

_Filed: 2026-10-28T21:07:59Z_
_Operation: UpdateDeployment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1b03c6e9884bdbb560f42766af0f40efd2be6453`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::DeploymentPattern::is_update_target` — the rule `ListDeploymentPatternVersions`' `updateFromVersion` filter is built on — states that the version a deployment ALREADY deploys is not a version an update can move it onto, and its doc comment says this is the rule `UpdateDeployment` applies. The `UpdateDeployment` handler does not apply it: it only checks that the named version is published by the pattern, so a request naming the version the deployment already deploys is accepted and the deployment is driven through UPDATE_IN_PROGRESS -> UPDATE_COMPLETED with an event claiming it moved to that version.

The two paths therefore disagree about one rule: `ListDeploymentPatternVersions` omits version X from the answer to "what can a deployment on X be updated to", while `UpdateDeployment` accepts X for exactly that deployment. One of the two has to change; the crate's own documentation says the exclusion is the rule.

## Evidence

The rule and its claim about `UpdateDeployment` — `src/state.rs:442-454`:

```
/// Whether an update of a deployment that currently deploys `from_version` of this
/// pattern can move it onto `version` of it.
///
/// An update names a version of the pattern it deploys and this catalogue publishes
/// several per pattern, so ANY published version is one a deployment can be moved
/// onto (`UpdateDeployment`) — except the one it already deploys, which moves no
/// version at all. ...
pub fn is_update_target(&self, from_version: &str, version: &str) -> bool {
    from_version != version
        && self.version(from_version).is_some()
        && self.version(version).is_some()
}
```

and the handler that reads it restates the tie — `src/ops/list_deployment_pattern_versions.rs:290-293`:

```
/// update can move a deployment onto from one of the versions it names — the rule
/// `crate::state::DeploymentPattern::is_update_target` states, which is the same rule
/// `UpdateDeployment` applies when it moves a deployment onto a version.
```

What `UpdateDeployment` actually checks — `src/ops/update_deployment.rs:270-283` (the deployment's currently deployed version is read at line 228 and is itself published, so it passes):

```
if input
    .deployment_pattern_version_name
    .as_ref()
    .is_some_and(|requested| pattern.version(requested).is_none())
{
    return Err(... DEPLOYMENT_PATTERN_VERSION_NOT_FOUND_MESSAGE ...);
}
```

No `from_version != version` check exists on this path, and `is_update_target` is never called from `update_deployment.rs` (it is referenced only by `list_deployment_pattern_versions.rs:311`). Test coverage: `tests/all/list_deployment_pattern_versions.rs:315-382` (`update_from_version_reports_the_versions_an_update_can_move_onto`) pins the exclusion on the list side — "A filter never answers a version it was asked to start FROM" — while no test on the update side names the already-deployed version.

## Steps to Reproduce

1. POST /createDeployment for SAP/SapHanaSingle; the record is stored with the pattern's current version 2.0.0 (state.rs:250-266, CATALOGUE_PATTERN_VERSIONS)
2. Wait for the deployment to reach COMPLETED, then POST /listDeploymentPatternVersions with filters [{"name":"updateFromVersion","values":["2.0.0"]}] — 2.0.0 is NOT reported, i.e. the service says it is not a version this deployment can be updated onto
3. POST /updateDeployment with the same deploymentId, the same specifications and "deploymentPatternVersionName":"2.0.0" — the request is accepted (200) and the deployment enters UPDATE_IN_PROGRESS with a DeploymentUpdateInProgress event naming version 2.0.0
4. The two answers contradict each other for the same (deployment, version) pair
