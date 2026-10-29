Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# An access policy is created about a project that does not exist, on the strength of a comment claiming CreateProject is still a scaffold

_Filed: 2026-10-28T21:06:52Z_
_Operation: CreateAccessPolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateAccessPolicy resolves only ONE of the two alternatives `Resource` declares. The `portal` form is looked up in `store.portals` and a portal the store does not hold draws the modelled 404; the `project` form is accepted unchecked and stored verbatim, so an access policy can be created about a project id nothing in the store holds. DescribeAccessPolicy and ListAccessPolicies then report that policy, naming a project that cannot be addressed by DescribeProject.

The handler states the reason for the asymmetry in a comment: "a project is accepted unchecked because no operation of this crate stores one yet (`CreateProject` is still a scaffold), so there is nothing a check could consult." That premise is false in this crate as it stands: `create_project.rs` is a full handler that writes `store.projects.insert(record.id.clone(), record)`, DescribeProject/UpdateProject/DeleteProject read that same map, and CreateDashboard already refuses a `projectId` the map does not hold with exactly the refusal this handler says it has nothing to consult for. So the check the comment calls impossible is the one a sibling handler of the same crate performs on the same map, two files away.

UpdateAccessPolicy carries the identical stale comment and the identical gap (`update_access_policy.rs:212-224`): it filters the portal form against `store.portals` and leaves the project form unresolved, so an update can re-point a policy at a project that does not exist.

This is the same defect shape as the already-FIXED `20261024T220209Z-iotsitewise-createpipeline-validation` ("Workspace-scoped creates accept a workspaceName no workspace holds, on the strength of a comment that denies the crate has a workspace registry"): a skipped existence check justified by a premise the crate has since outgrown.

## Evidence

The handler: only the portal half is resolved, and the comment that justifies skipping the other half —
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_access_policy.rs:241-250
    "// The policy is about a Monitor resource, so a resource this store does not
     // hold is the absence this operation's modelled 404 states. Checked for the
     // portal form, which this store holds; a project is accepted unchecked
     // because no operation of this crate stores one yet (`CreateProject` is
     // still a scaffold), so there is nothing a check could consult.
     if let Some(portal) = input.access_policy_resource.portal.as_ref() {
         if !store.portals.contains_key(&portal.id) {
             return Err(portal_not_found(&portal.id));
         }
     }"
  and the unchecked store that follows, src/ops/create_access_policy.rs:273-289 — "resource: input.access_policy_resource, ... store.access_policies.insert(record.id.clone(), record);"

The premise is false — CreateProject is implemented and stores projects:
  src/ops/create_project.rs:275 — "store.projects.insert(record.id.clone(), record);"
  (and its scaffold issue is closed: 20261024T100802Z-iotsitewise-createproject-stub.md is [fixed])

The sibling that performs the very check the comment calls impossible, on the same map:
  src/ops/create_dashboard.rs:213-227
    "// A dashboard is created IN a project, so a `projectId` that resolves to no
     // stored project names no container for the dashboard to live in — the
     // absence this operation's modelled 404 states, in the one wording every
     // not-found refusal in this service carries. The same check `CreateProject`
     // makes of the portal it is given.
     if !store.projects.contains_key(&input.project_id) {
         return Err(CreateDashboardError::Modeled(
             CreateDashboardErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                 message: crate::state::not_found_message(
                     crate::state::RESOURCE_PROJECT,
                     &input.project_id,
                 ),"
  and src/ops/batch_disassociate_project_assets.rs:209-218 — "// The project is what the membership is held ON, so an id that resolves to no stored project names no membership to remove anything from. ... if !store.projects.contains_key(&input.project_id) {"

The same stale comment and gap on the update side:
  src/ops/update_access_policy.rs:212-224
    "// The policy is about a Monitor resource ... a project is accepted unchecked
     // because no operation of this crate stores one yet (`CreateProject` is
     // still a scaffold), so there is nothing a check could consult. ...
     let missing_portal = input
         .access_policy_resource
         .portal
         .as_ref()
         .filter(|portal| !store.portals.contains_key(&portal.id))"

Test coverage: the behaviour is exercised only through the same stale premise, so the fix has to update it —
  tests/all/access_policy_lifecycle.rs:74-80 — "/// A modelled `ID` built from `seed` ... Used for the resources this suite names but does not create (a project, whose create is still a scaffold) and for the ids it expects to resolve to nothing."
  tests/all/access_policy_lifecycle.rs:86 — "\"project\" => json!({ \"project\": { \"id\": modelled_id(seed) } }),"
  tests/all/access_policy_lifecycle.rs:596 pins the portal half's refusal ("absent_portal"), so only the project half is unchecked.

## Steps to Reproduce

1. CreatePortal, then CreateProject in it to see that projects are really stored (DescribeProject answers 200).
2. POST /access-policies with {"accessPolicyIdentity": {"iamRole": {"arn": "arn:aws:iam::123456789012:role/monitor-admin"}}, "accessPolicyResource": {"project": {"id": "53000000-1c2d-4e5f-8a9b-0c1d2e3f4a5b"}}, "accessPolicyPermission": "ADMINISTRATOR"} — an id no CreateProject ever minted.
3. Observed: 201 and the policy is stored; GET /access-policies/{id} reports accessPolicyResource.project.id = 53000000-… while GET /projects/53000000-… answers 404.
4. Compare: POST /dashboards with that same projectId is refused 404 ResourceNotFoundException "Project with ID 53000000-… not found" (create_dashboard.rs:218), and the portal half of this very handler refuses an absent portal 404.
5. PUT /access-policies/{id} (UpdateAccessPolicy) with the same nonexistent project is likewise accepted.
