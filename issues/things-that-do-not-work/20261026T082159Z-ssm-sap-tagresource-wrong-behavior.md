Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# A component ARN resolves to nothing, so the component Tags GetComponent/ListComponents read can never exist

_Filed: 2026-10-26T08:21:59Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegionState::holds_resource` is the one place this crate decides what an ARN names, and it resolves only a registered application's own ARN and the ARNs of the databases derived from that registration. `TagResource` (and `ListTagsForResource`) refuse anything else with 404 `ResourceNotFoundException`. But the crate also mints component ARNs (`component_arn`), reports them in `ComponentSummary$Arn`, and looks a component's tags up in `tags_by_arn` keyed by that very ARN in both `GetComponent` and `ListComponents`. Since the only write paths into `tags_by_arn` are `RegisterApplication` (application ARN) and `apply_tags` behind the `holds_resource` gate, a component ARN can never become a key: those two tag lookups are dead, `Component`/`ComponentSummary` tags are permanently absent, and tagging a resource this service itself reports with an ARN answers 404. The two halves of the crate disagree about whether a component is a resource of this service.

## Evidence

Resolver side — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2454-2480: "Whether `arn` names a resource THIS Region holds. The resources AWS Systems Manager for SAP holds are the registered SAP application and the SAP HANA databases of that application, so an ARN resolves here when it is a registered application's own ARN ... or the ARN of one of the databases derived from that registration — and nothing else", implemented as `resolve_application_id(None, Some(arn), None).is_some() || ...databases(region)...any(|database| database.arn == arn)`.

Refusal that follows from it — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/tag_resource.rs:176-186: `if !state.region(&region).holds_resource(&region, &input.resource_arn) { return Err(TagResourceErrorModeled::ResourceNotFoundException(crate::state::not_found(crate::state::resource_not_found_message(&input.resource_arn)))...)}`.

Reader side that assumes the opposite — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/get_component.rs:184-192: "The tags of the component, read from the one place every tag of this service is held, keyed by the resource's own ARN" with `region_state.tags_for(&crate::state::component_arn(&region, &application.id, &component.id))`; and services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_components.rs:238-242: "The component's ARN has ONE construction site per item: the key its tags are held under is the same spelling the summary reports" with `let arn = crate::state::component_arn(...); let tags = region_state.tags_for(&arn);`.

The crate's own test states the assumption rather than the restriction — tests/all/component_listing.rs:379-381: "The tags a registration carried are the APPLICATION's, held under the application's ARN; a component is tagged through its own ARN and nothing has tagged one." and tests/all/component_reads.rs:506-511: "a component nobody tagged carries no tags" — neither pins a 404 for a component ARN, so nothing covers the refusal this gap produces.

## Steps to Reproduce

1. POST /register-application with ApplicationId "comp-tag", ApplicationType "HANA", Sid "HDB", one instance and credentials, so the registration describes the HANA component "HDB-HANA".
2. POST /list-components with {"ApplicationId":"comp-tag"} — note the reported `Arn`: arn:aws:ssm-sap:<region>:<account>:component/comp-tag/HDB-HANA.
3. POST /tags/<that component ARN> with {"tags":{"env":"dev"}} — the ARN names a component this same service just reported, yet `holds_resource` resolves only application and database ARNs, so the call answers 404 ResourceNotFoundException "Resource <arn> doesn't exist.".
4. POST /get-component with {"ApplicationId":"comp-tag","ComponentId":"HDB-HANA"} — `Tags` is absent and can never be anything else, because get_component.rs:188 looks tags up under the component ARN that no write path can ever key.
