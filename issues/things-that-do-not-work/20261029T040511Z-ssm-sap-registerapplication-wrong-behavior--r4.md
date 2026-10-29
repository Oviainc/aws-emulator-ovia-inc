Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fb2d49e8bd3d09cec7ea7562ca3d7e59e4fee21f

# REGISTER_APPLICATION operation record files empty Properties although the call carries parameters

_Filed: 2026-10-29T04:05:11Z_
_Operation: RegisterApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fb2d49e8bd3d09cec7ea7562ca3d7e59e4fee21f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for an operation record's `Properties`: they are the parameters of the call that filed the record, so a client reading the record back learns how the work was asked for. `StopApplication` and `UpdateApplicationSettings` implement that rule. `RegisterApplication` files its record with an EMPTY property map, justified by an inline comment claiming the call carries no parameters beyond the resource it names — which is false for this operation: the model gives `RegisterApplication` `ApplicationType` and `Instances` (both `@required`) plus `Sid`, `SapInstanceNumber`, `Credentials`, `DatabaseArn` and `ComponentsInfo`. The result is that a later `GetOperation`/`ListOperations` on the registration's own `OperationId` reports no `Properties` at all, while every other recorded call reports the parameters it was asked with.

## Evidence

Premise that is false, and the empty map it justifies — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:365-372:
```
        let operation_id = region_state.record_operation(
            crate::state::OPERATION_TYPE_REGISTER_APPLICATION,
            &application,
            // This call carries no parameters beyond the resource it names, which the
            // record already reports as `ResourceId`/`ResourceArn`.
            ::std::collections::HashMap::new(),
            now,
        );
```
The rule a sibling handler of the SAME crate states and implements — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs:312-317:
```
    // The operation's `Properties` are the parameters of the call that filed it, so
    // a client reading the record back learns how the stop was asked for.
    let mut properties = ::std::collections::HashMap::new();
    if let Some(entity) = input.stop_connected_entity {
        properties.insert("StopConnectedEntity".to_string(), entity);
    }
```
and services/service-com-amazonaws-ssmsap-ssmsap/src/ops/update_application_settings.rs:262-272:
```
        // The operation's `Properties` are the parameters of the call that filed it, so
        // a client reading the record back learns what the update asked for. A
        // credential's `SecretId` is `@sensitive`, so what the record names of a
        // credential is the SAP HANA database it reaches and its type, never the secret.
        let mut properties = ::std::collections::HashMap::new();
        if !credentials_to_add_or_update.is_empty() {
            properties.insert(
                "CredentialsToAddOrUpdate".to_string(),
                named_credentials(credentials_to_add_or_update),
            );
```
That the premise is false comes from the model itself — `smithy_query(query="operation", operation="RegisterApplication")` input: `ApplicationId` (required), `ApplicationType` (required), `Instances` (required), `SapInstanceNumber`, `Sid`, `Tags`, `Credentials`, `DatabaseArn`, `ComponentsInfo` — mirrored in services/service-com-amazonaws-ssmsap-ssmsap/OPERATIONS.md:177.

Test coverage: no test pins the registration record's `Properties`. tests/all/application_lifecycle.rs:1229-1242 asserts "the call's own parameters are what the record reports as `Properties`" only for the start/stop steps, and tests/all/list_operations.rs:123-156 deliberately gives the register row `no_extra()`, so nothing pins the empty map for `REGISTER_APPLICATION`.

## Steps to Reproduce

1. RegisterApplication with ApplicationId="app1", ApplicationType="HANA", Instances=["i-01"], Sid="PRD", SapInstanceNumber="00", Credentials=[{DatabaseName:"SYSTEMDB",CredentialType:"ADMIN",SecretId:"sap/prd/system"}] and note the returned OperationId
2. Wait out the transition span, then call GetOperation with that OperationId
3. Observe the returned Operation carries NO `Properties` member, even though the call carried ApplicationType, Instances, Sid, SapInstanceNumber and Credentials
4. For contrast, call StopApplication with StopConnectedEntity="DBMS" and GetOperation on its OperationId: that record reports Properties {"StopConnectedEntity": "DBMS"}
