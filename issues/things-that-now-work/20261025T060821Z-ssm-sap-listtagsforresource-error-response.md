Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# ListTagsForResource refuses the component ARNs this crate mints and holds tags under

_Filed: 2026-10-25T06:08:21Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTagsForResource` decides whether an ARN names a resource with `RegionState::holds_resource`, which knows only application ARNs and database ARNs. A component ARN — the ARN this crate mints with `component_arn` and reports to callers as `Component$Arn` — resolves to nothing there, so the read answers 404 `ResourceNotFoundException` "Resource <arn> doesn't exist." for a resource the service itself just handed the caller.

That contradicts the same crate's `GetComponent`, which treats a component ARN as a first-class tag key: it reads the component's tags out of `RegionState::tags_by_arn` keyed by exactly that ARN, with the comment that this is "the one place every tag of this service is held, keyed by the resource's own ARN". So one handler looks tags up under a component ARN while the resolver that decides whether an ARN exists declares that no component ARN names anything — the two cannot both be right, and a client that takes `Component$Arn` from `GetComponent` into `ListTagsForResource` gets a not-found for a component that demonstrably exists.

No test pins the component-ARN case: the refusal table of `tests/all/resource_tags.rs` covers application and database ARNs only.

## Evidence

The resolver excludes components by construction — `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:1282-1294`:
```
/// Whether `arn` names a resource THIS Region holds.
///
/// The resources AWS Systems Manager for SAP holds are the registered SAP
/// application and the SAP HANA databases of that application, so an ARN resolves
/// here when it is a registered application's own ARN
/// ([`StoredApplication::arn`]) or the ARN of one of the databases derived from
/// that registration ([`StoredApplication::databases`]) — and nothing else
pub fn holds_resource(&self, region: &str, arn: &str) -> bool {
```
and that is the only gate `ListTagsForResource` applies — `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_tags_for_resource.rs:177-184`:
```
if !region_state.holds_resource(&region, &input.resource_arn) {
    return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
        crate::state::not_found(crate::state::resource_not_found_message(
            &input.resource_arn,
        )),
```

Yet the crate mints component ARNs and reports them: `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:1687-1693` (`component_arn` → `component/<ApplicationId>/<ComponentId>`), returned as `Component$Arn` at `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:614`: `arn: Some(component_arn(region, &self.id, &component.id)),` — and holds tags under that same key, `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/get_component.rs:184-192`:
```
// The tags of the component, read from the one place every tag of this service is
// held, keyed by the resource's own ARN — the SAME spelling the projection above
// reports, so the two cannot drift.
let tags = region_state.tags_for(&crate::state::component_arn(
    &region,
    &application.id,
    &component.id,
));
```

Unpinned: the not-found rows of `services/service-com-amazonaws-ssmsap-ssmsap/tests/all/resource_tags.rs:350-386` are application ARNs, database ARNs and another service's ARN — no component ARN appears in that table or anywhere in the suite.

## Steps to Reproduce

1. POST /register-application {"ApplicationId":"HDB-TEST","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"sap/hdb/admin"}],"ComponentsInfo":[{"ComponentType":"HANA","Sid":"HDB","Ec2InstanceId":"i-0123456789abcdef0"}]}
2. POST /get-component {"ApplicationId":"HDB-TEST","ComponentId":"HDB-HANA"} — 200, Component.Arn = arn:aws:ssm-sap:us-east-1:<account>:component/HDB-TEST/HDB-HANA
3. GET /tags/<that ARN, URL-encoded> — 404 ResourceNotFoundException "Resource arn:aws:ssm-sap:us-east-1:<account>:component/HDB-TEST/HDB-HANA doesn't exist.", although GetComponent reads that component's tags out of tags_by_arn under exactly this ARN
4. Compare with GET /tags/arn:aws:ssm-sap:us-east-1:<account>:database/HDB-TEST/HDB, which answers 200 with an empty tag set for a database derived from the same registration
