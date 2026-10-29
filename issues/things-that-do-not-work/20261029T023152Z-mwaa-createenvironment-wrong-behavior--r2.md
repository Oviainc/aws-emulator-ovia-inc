Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# The @idempotent replay comparison is order-sensitive for SubnetIds/SecurityGroupIds, so an identical create is refused 400

_Filed: 2026-10-29T02:31:52Z_
_Operation: CreateEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEnvironment` decides whether a create of an already-stored name is an idempotent REPLAY (answer the stored ARN, write nothing) or a different create of a taken name (refuse with 400 `ValidationException`) by comparing the whole resolved `EnvironmentSpec` with `==`.

That comparison is deliberately normalised for the map-valued member: `EnvironmentSpec.airflow_configuration_options` is stored as a `BTreeMap` with the stated reason "so two equal option sets compare equal". The LIST-valued members are not normalised: `NetworkConfigurationRecord.subnet_ids` and `.security_group_ids` are plain `Vec<String>` filled straight from the request in request order, and `Vec`'s derived `PartialEq` is positional.

Consequence: two creates that describe the SAME environment placement but list the subnets (or the security groups) in a different order compare unequal, so the second one falls through the replay branch into the refusal branch and is answered HTTP 400 `ValidationException` "An environment named <Name> already exists" — even though it is the same create of the same name with the same members, which is exactly the case the handler's own comment says must answer with the stored ARN. The branch taken for one and the same logical request therefore depends on the order of two members whose values are sets (`SubnetList` / `SecurityGroupList` carry no ordering semantics and no `@uniqueItems`), which is the kind of instability the crate already eliminated for the map member of the same comparison.

This is a different defect from the already-filed `Tags`-on-replay gap (that one is about a member absent from the comparison; this one is about the comparison itself being unstable for members that ARE in it).

## Evidence

The comparison that selects replay vs. refusal — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_environment.rs:408-430:
```
        // The operation is `@idempotent`: replaying the SAME create of a name that is
        // already stored IN THIS REGION answers with the stored ARN and writes nothing. A
        // create of that name with different members is a different request, and is
        // refused rather than allowed to clobber the stored record.
        ...
        if let Some(existing) = store.environment(&region, &input.name) {
            if existing.spec == spec && !existing.is_deleting() {
                return Ok(CreateEnvironmentOutput {
                    arn: Some(existing.arn.clone()),
                });
            }
            return Err(validation(format!(
                "An environment named {} already exists",
                input.name
            )));
        }
```

The compared value's stated purpose — services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:498-504:
```
/// Everything about an environment that a caller chose: the create's members, as every
/// later `UpdateEnvironment` merged into them.
///
/// Kept apart from the values the SERVICE minted (ARN, URLs, queue, endpoint service
/// names) so `CreateEnvironment` — which is `@idempotent` — can tell a replay of the same
/// create from a different create of a name that is already taken, by comparing specs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentSpec {
```

The map member IS normalised for that comparison — state.rs:516-518:
```
    /// A `BTreeMap` so two equal option sets compare equal and read back in a stable
    /// order.
    pub airflow_configuration_options: BTreeMap<String, String>,
```

The list members are NOT — state.rs:481-486:
```
/// The VPC networking components an environment was placed in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkConfigurationRecord {
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
}
```
and they are filled in request order — create_environment.rs:270-279, 304-307:
```
        let subnet_ids = input
            .network_configuration
            .subnet_ids
            .clone()
            .unwrap_or_default();
        let security_group_ids = input
            .network_configuration
            .security_group_ids
            .clone()
            .unwrap_or_default();
...
            network_configuration: crate::state::NetworkConfigurationRecord {
                subnet_ids,
                security_group_ids,
            },
```

The model gives these members no ordering semantics — `smithy_query(query="shape", shape="SubnetList,SecurityGroupList")`: `"SubnetList": {"type": "list", "length": {"min": 2, "max": 2}, "member": {"target": "SubnetId", ...}}`, `"SecurityGroupList": {"type": "list", "length": {"min": 1, "max": 5}, ...}` — no `uniqueItems`, no ordering trait, and `CreateEnvironment` carries `@idempotent` (OPERATIONS.md:35 "**Traits:** @idempotent").

Test coverage: the existing replay test re-sends the BYTE-IDENTICAL body and builds its "different create" row by changing `DagS3Path`, so neither assertion varies the order of a list member — tests/all/environment_lifecycle.rs:819-867 (`an_idempotent_replay_returns_the_stored_arn_and_a_different_create_is_refused`), per the quoted coverage note of the already-filed replay issue 20261028T210011Z-mwaa-createenvironment-parameter-ignored.

## Steps to Reproduce

1. PUT /environments/MyMWAAEnvironment with a valid body whose `NetworkConfiguration` is `{"SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"], "SecurityGroupIds": ["sg-00000000000000001"]}` — note the HTTP 200 and the returned `Arn`.
2. Re-send the SAME PUT with the same body: HTTP 200 carrying the same `Arn` (the replay branch at create_environment.rs:421).
3. Re-send the SAME PUT once more, changing nothing but the order of the two subnet ids: `{"SubnetIds": ["subnet-00000000000000002", "subnet-00000000000000001"], "SecurityGroupIds": ["sg-00000000000000001"]}`.
4. Observe HTTP 400 `ValidationException` "An environment named MyMWAAEnvironment already exists" instead of the stored ARN: `existing.spec == spec` is false because `subnet_ids` is a positionally compared `Vec`, even though the request names the same two subnets.
5. The same happens for a create that lists several `SecurityGroupIds` in a different order. Contrast with `AirflowConfigurationOptions`, where re-ordering the pairs still replays, because that member is stored in a `BTreeMap` for exactly this reason (state.rs:516-518).
