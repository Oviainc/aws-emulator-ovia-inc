Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# DeletePortal (and DeleteProject, through the shared helper) leaves access policies naming the removed portal/project — records CreateAccessPolicy refuses to create and UpdateAccessPolicy can never rewrite

_Filed: 2026-10-29T07:37:16Z_
_Operation: DeletePortal | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePortal` removes the portal, its projects and those projects' dashboards through `state::forget_portal`/`forget_project`, but deliberately leaves every `AccessPolicyRecord` whose `Resource` names the removed portal or project. The store therefore keeps policies that `DescribeAccessPolicy`/`ListAccessPolicies` happily report about a Monitor resource this service no longer holds — a state no write of this crate could have produced, because `CreateAccessPolicy` refuses exactly that request through `state::missing_monitor_resource`, and one no caller can repair, because `UpdateAccessPolicy` asks the SAME check and so answers 404 for the resource the stored policy already names (the policy can only be deleted, never corrected).

This contradicts the crate's own sweep rule, which every other `forget_*` applies: `forget_asset` drops the asset's project memberships precisely so "`ListProjectAssets`" cannot report "an asset nothing can address", `forget_computation_model` drops the actions and executions that NAME the model, and `forget_portal`'s own doc says dropping the contained projects/dashboards "is what keeps a later read from resolving to a project or a dashboard whose portal is gone". The access policies that name that same portal are the one reference left pointing at it. `DeleteProject` reaches the identical outcome through `forget_project`, which the state module calls "the one place this containment rule is stated".

## Evidence

The handler states the carve-out — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_portal.rs:189-193:
```
        // The portal goes, and with it everything this service holds only as a part
        // of it: the projects created in it and those projects' dashboards, so no
        // later read can still resolve to a Monitor resource whose portal is gone.
        // The access policies that name the portal or one of its projects are left
        // alone, as `DeleteProject` leaves the ones naming the project it removes.
```
and the helper repeats it — src/state.rs:5453-5469:
```
    /// Forget an IoT SiteWise Monitor project and the dashboards created in it.
    ...
    /// The access policies that name the project are
    /// left alone: a policy is a resource of its own, addressed by its own id and
    /// removed by `DeleteAccessPolicy`.
    ...
    pub fn forget_project(&mut self, project_id: &str) -> Option<ProjectRecord> {
        let record = self.projects.remove(project_id)?;
        self.dashboards
            .retain(|_, dashboard| dashboard.project_id != project_id);
```

The rule it breaks, stated by the same helper — src/state.rs:5476-5483:
```
    /// A project is created IN a portal — the create refuses a portal this store
    /// does not hold — so a portal the service no longer holds can leave none of
    /// its projects addressable behind. Dropping them here is what keeps a later
    /// read from resolving to a project or a dashboard whose portal is gone, the
    /// same reason [`Self::forget_asset`] drops what is keyed off an asset.
```
and applied by the siblings — src/state.rs:5379-5384:
```
        // A project's membership names assets, so an asset this service no longer
        // holds is in no project: dropping the entry here is what keeps
        // `ListProjectAssets` from reporting an asset nothing can address.
        for project in self.projects.values_mut() {
            project.asset_ids.remove(asset_id);
        }
```
src/state.rs:5551-5570 (`forget_computation_model`): "An action and an execution each NAME the target resource they were taken on ... so a model this service no longer holds can leave none of them behind" followed by `self.actions.retain(...)` / `self.executions.retain(...)`.

The gate the leftover record could never pass on the way in — src/ops/create_access_policy.rs:241-251:
```
        // The policy is about a Monitor resource, so a resource this store does not
        // hold is the absence this operation's modelled 404 states. BOTH
        // alternatives a `Resource` can take name a resource this service holds, so
        // each is resolved against the map that holds it ...
        if let Some((resource, requested)) =
            store.missing_monitor_resource(&input.access_policy_resource)
        {
            return Err(not_found(resource, requested));
        }
```
and the same gate on every rewrite — src/ops/update_access_policy.rs:223-239 (`let missing_resource = store.missing_monitor_resource(&input.access_policy_resource)` ... `return Err(not_found(resource, &requested));`).

Test coverage: tests/all/portal_delete.rs and tests/all/project_delete.rs cover the project/dashboard cascade; tests/all/access_policies.rs and tests/all/access_policy_lifecycle.rs create and read policies against live portals/projects. No case deletes a portal or project that an access policy names, so nothing pins the surviving policy.

## Steps to Reproduce

1. CreatePortal -> portal P.
2. CreateProject in P -> project R.
3. CreateAccessPolicy with accessPolicyResource = {project: {id: R}} (or {portal: {id: P}}) -> 201 with an accessPolicyId A.
4. DeletePortal P (or DeleteProject R) -> 200.
5. DescribeAccessPolicy A -> observed: 200, reporting accessPolicyResource naming R/P, which DescribeProject/DescribePortal now answer 404 for; ListAccessPolicies(resourceType=PROJECT, resourceId=R) still lists it.
6. UpdateAccessPolicy A with that same resource (the only resource the policy holds) -> 404 from missing_monitor_resource: the record can be read and listed but never corrected.
7. Expected, by the rule src/state.rs:5476-5483 and the forget_asset/forget_computation_model siblings: removing the Monitor resource leaves no stored reference to it.
