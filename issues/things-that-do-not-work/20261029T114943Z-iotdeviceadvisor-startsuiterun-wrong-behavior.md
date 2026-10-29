Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d626157daccf58583369e714487cbead343a069c

# StartSuiteRun mints a suite-run ARN in the caller's region, so a run can claim a scope its parent definition's ARN says it does not live in

_Filed: 2026-10-29T11:49:43Z_
_Operation: StartSuiteRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d626157daccf58583369e714487cbead343a069c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate holds ONE account-global store keyed by id, but it stamps every ARN it mints with the region of the request that minted it (`crate::state::request_region`). `StartSuiteRun` looks the parent definition up by id alone, with no comparison against the scope the definition's stored ARN records, and then mints the child run's ARN from the CURRENT request's region. A client signed for `eu-central-1` can therefore start a run on a definition whose own ARN says `us-east-1`, and the resulting `suiteRunArn` names `eu-central-1` while the `suiteDefinitionArn` of the very definition it is a run of names `us-east-1`: within one store a child resource claims a scope its parent is not in.

That contradicts the rule this crate states and enforces for the ARN-addressed half of its own surface. `ComAmazonawsIotdeviceadvisorIotsenateserviceState::holds_tagged_resource` is documented as treating region as part of the resource's identity — an ARN carrying the right id but another region "names a resource of that other scope, which this store does not hold" — and `TagResource`/`UntagResource`/`ListTagsForResource` all refuse such an ARN with the modelled 404 by comparing the whole stored ARN string. So after the cross-region start above, the run is addressable for tagging ONLY under `eu-central-1`, while a client that derives the run's ARN from the region its definition reports (`us-east-1`) is answered 404 — and the definition and its run can never be addressed under one scope at all.

The same root cause makes the id-keyed readers (`GetSuiteDefinition`, `GetSuiteRun`, `ListSuiteDefinitions`, `ListSuiteRuns`) hand back resources of every region a caller has ever signed for, so the store demonstrably holds several scopes at once — exactly what the quoted doc says it does not. The defect to fix is the one write: the run's ARN should be derived from the definition it runs (its stored scope), not from whichever request happened to start it, so parent and child always agree about the resource scope the crate's own ARN rule reads out of them.

## Evidence

The scope rule this crate states and enforces — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs:575-581:
```
    /// True when this store holds the resource `resource` names, UNDER THE ARN it was
    /// named by.
    ///
    /// The ARN identifies the resource whole: an ARN carrying the right id but another
    /// partition, region or account names a resource of that other scope, which this
    /// store does not hold, so it is not found rather than silently treated as the local
    /// one. A pure predicate — a tag read never advances anything.
```
and the comparison that enforces it — state.rs:582-595 (`definition.arn == arn`, `run.arn == arn`), reached from src/ops/tag_resource.rs:175-185:
```
        if !store.holds_tagged_resource(&input.resource_arn, &resource) {
            return Err(trail.refused(
                TagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
```
The handler that breaks it — src/ops/start_suite_run.rs:174 takes the region off THIS request:
```
        let region = crate::state::request_region(ctx.parts());
```
src/ops/start_suite_run.rs:216 resolves the parent by id only, with no scope check:
```
        let Some(definition) = store.definitions.get(&input.suite_definition_id) else {
```
and src/ops/start_suite_run.rs:287-291 mints the child's ARN from that request region rather than from the parent's stored ARN:
```
        let id = crate::state::new_uuid();
        let arn = crate::state::arn(
            &region,
            &format!("suiterun/{}/{id}", input.suite_definition_id),
        );
```
The region really is interpolated per request — state.rs:925-937 (`request_region`: "The region to stamp into ARNs and endpoints: the SigV4 credential scope's region ...") and state.rs:971-973 (`arn` -> `emulator_arn(ARN_NAMESPACE, region, resource)`).

That the store holds several scopes at once is pinned by the crate's own test — tests/all/suite_lifecycle.rs:507-522 creates definitions in four regions against ONE emulator and asserts only the ARN text:
```
    for region in ["us-east-1", "eu-central-1", "ap-southeast-4", "il-central-1"] {
        let first = emulator.create_definition(&configuration, region).await;
...
        assert_eq!(
            first.string("suiteDefinitionArn"),
            format!("arn:aws:iotdeviceadvisor:{region}:000000000000:suitedefinition/{first_id}"),
            "case {region:?}: the ARN must carry the request's region"
        );
```
The id-keyed readers apply no scope check either — src/ops/get_suite_definition.rs:162 (`store.definitions.get(&input.suite_definition_id)`) and src/ops/list_suite_definitions.rs:176-179 (`store.definitions.range(..)` over the whole map).

Test coverage: nothing pins the cross-region start. tests/all/suite_lifecycle.rs:498-523 is the only multi-region case and never starts a run, reads a definition from another region, or compares a run ARN with its definition's; tests/all/resource_tags.rs:648-652 exercises the 404 for "another region, another account" on the tagging side only. So the behaviour above is untested rather than justified.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /suiteDefinitions with an Authorization header whose SigV4 credential scope names us-east-1, body {"suiteDefinitionConfiguration":{"suiteDefinitionName":"x","rootGroup":"g","devicePermissionRoleArn":"arn:aws:iam::000000000000:role/r"}}. Note the returned suiteDefinitionId D and suiteDefinitionArn arn:aws:iotdeviceadvisor:us-east-1:000000000000:suitedefinition/D.
3. POST /suiteDefinitions/D/suiteRuns with a credential scope naming eu-central-1 and body {"suiteRunConfiguration":{"primaryDevice":{"thingArn":"arn:aws:iot:us-east-1:000000000000:thing/t"}}}. Observed: HTTP 200 with suiteRunArn arn:aws:iotdeviceadvisor:eu-central-1:000000000000:suiterun/D/R — a child whose region differs from the parent's.
4. GET /suiteDefinitions/D (any region): the parent still reports arn:...:us-east-1:...:suitedefinition/D, so no single scope contains both the definition and its run.
5. POST /tags/arn%3Aaws%3Aiotdeviceadvisor%3Aus-east-1%3A000000000000%3Asuiterun%2FD%2FR with {"tags":{"k":"v"}} — the run ARN as a client would derive it from the definition's region. Observed: HTTP 404 ResourceNotFoundException per state.rs:575-581, while the same call against the eu-central-1 form succeeds.
6. GET /suiteDefinitions with a credential scope naming ap-southeast-4: the us-east-1 definition is listed, showing the store holds resources of scopes the quoted rule says it does not hold.
