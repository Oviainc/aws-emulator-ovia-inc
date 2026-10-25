Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# Reconcile the HANA ComponentId ListDatabases reports with the components GetComponent can resolve

_Filed: 2026-10-25T06:08:21Z_
_Operation: GetComponent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate derives two disagreeing answers to "which components does this application have?".

`StoredApplication::databases` attributes every derived database to the component id `<Sid>-HANA` whenever the registration carried a `Sid` — unconditionally, without checking that such a component exists. The component SET, however, is built exclusively from the registration's `ComponentsInfo` (`RegisterApplication`), and `GetComponent` resolves only that set.

So for the ordinary HANA registration (`Sid` + `Credentials`, no `ComponentsInfo` — `ComponentsInfo` is modelled optional and documented as "an optional parameter for component details to which the SAP ABAP application is attached"):

* `ListDatabases` reports `DatabaseSummary.ComponentId = "HDB-HANA"` for every database,
* `GetApplication` reports `Application.Components = []` (the application has no components at all),
* `GetComponent(ApplicationId, "HDB-HANA")` is refused 400 `ValidationException` "No component with ID HDB-HANA exists for application …" — the very refusal the crate's own suite pins for "a well-formed component id the registration never declared".

A client that follows `ComponentId` out of a `ListDatabases` page into `GetComponent` therefore gets a 400 for a component the listing just named, and `Application.Components` contradicts both. One of the two derivations has to change so that the component a database names is a component the application has (either the implied HANA component becomes resolvable, or a database of an application that declared no such component reports no `ComponentId`).

## Evidence

The id is minted unconditionally from the SID — `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:488-496`:
```
pub fn databases(&self, region: &str) -> Vec<ApplicationDatabase> {
    // The component a HANA database belongs to is the application's HANA component,
    // identified the way this service identifies every component: by the System ID
    // and the component type. An application whose registration named no System ID
    // has nothing to identify that component by.
    let component_id = self
        .sid
        .as_deref()
        .map(|sid| component_id(sid, COMPONENT_TYPE_HANA));
```

But the component set is only what `ComponentsInfo` declared — `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:48-53`:
```
//! An application's COMPONENTS are held the same way: the [`StoredComponent`] records in
//! [`StoredApplication::components`] are the ones the registration's `ComponentsInfo`
//! declared, projected onto the wire `Component` by
//! [`StoredApplication::component_view`] — which is what `GetComponent` reads back. So a
//! component a caller can read is one a registration declared
```
and `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:259-271` builds `components` from `input.components_info` alone (`for component in input.components_info.into_iter().flatten()`), so a registration without `ComponentsInfo` stores none.

`GetComponent` refuses anything outside that set — `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/get_component.rs:171-179`:
```
let Some(component) = application.component(&input.component_id) else {
    return Err(GetComponentErrorModeled::ValidationException(
        crate::state::validation(crate::state::no_component_message(
```

Both halves are pinned by the crate's own tests, which is how the contradiction is observable: `services/service-com-amazonaws-ssmsap-ssmsap/tests/all/list_databases.rs:78-86` expects `expected_component: Some("HDB-HANA")` for application `hdb-single`, whose body comes from `valid_registration` (`tests/all/application_lifecycle.rs:279-288`) which inserts only `ApplicationId`, `ApplicationType`, `Instances` and the conditionally required members — never `ComponentsInfo`; while `services/service-com-amazonaws-ssmsap-ssmsap/tests/all/component_reads.rs:657-662` pins the 400 refusal for "a well-formed component id the registration never declared".

## Steps to Reproduce

1. POST /register-application {"ApplicationId":"hdb-single","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"sap/hdb/admin"}]} — note NO ComponentsInfo
2. POST /list-databases {"ApplicationId":"hdb-single"} — the reported DatabaseSummary carries ComponentId "HDB-HANA"
3. POST /get-application {"ApplicationId":"hdb-single"} — Application.Components is the empty list, i.e. the application has no components
4. POST /get-component {"ApplicationId":"hdb-single","ComponentId":"HDB-HANA"} — 400 ValidationException "No component with ID HDB-HANA exists for application hdb-single", for the component the listing in step 2 just named
