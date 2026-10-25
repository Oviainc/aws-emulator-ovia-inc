Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1547d46e2415f486cf41cf952181485c961e01df

# DeleteSecurityConfiguration removes a configuration a live virtual cluster references, creating exactly the dangling reference CreateVirtualCluster refuses to persist

_Filed: 2026-10-25T18:40:48Z_
_Operation: DeleteSecurityConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1547d46e2415f486cf41cf952181485c961e01df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteSecurityConfiguration` removes the record unconditionally: it never looks at whether a stored virtual cluster still names that id in its `securityConfigurationId`. After the delete, `DescribeVirtualCluster`/`ListVirtualClusters` keep reporting `securityConfigurationId: <id>` for a RUNNING cluster while `DescribeSecurityConfiguration` of that same id answers `ResourceNotFoundException`.

That is the exact state this crate says it must never reach: `CreateVirtualCluster` resolves `securityConfigurationId` out of the same store before minting anything, and its comment gives the reason — a reference that is not stored must not be "persisted as a dangling reference that every later read would report as live". The delete path reaches the same end state through the back door, so the referential rule holds on one side of the resource's life and not the other. The crate already has the lookup helpers needed to see the reference (`region_virtual_clusters`, `VirtualClusterRecord::security_configuration_id`), and `DeleteVirtualCluster`/`UpdateVirtualCluster` show the shape of a state-based refusal for a delete that cannot be honoured.

## Evidence

The rule and its reason, src/ops/create_virtual_cluster.rs:181-202:
```
        // Every resource this request REFERENCES is resolved out of state before anything
        // is minted: `securityConfigurationId` is the id of a security configuration this
        // service stores, and an id that names none is refused with the modelled
        // `ResourceNotFoundException` rather than persisted as a dangling reference that
        // every later read would report as live.
        if let Some(configuration_id) = input.security_configuration_id.as_deref() {
            if state
                .security_configuration(&region, configuration_id)
                .is_none()
            {
                return Err(CreateVirtualClusterErrorModeled::ResourceNotFoundException(
```

The delete that undoes it, src/ops/delete_security_configuration.rs:173-194:
```
        // The delete must actually take effect: the record leaves the store here, so the
        // next `DescribeSecurityConfiguration` of this id finds nothing and answers the
        // modelled `ResourceNotFoundException` instead of serving a deleted resource.
        …
        let Some(record) = state.remove_security_configuration(&region, &input.id) else {
```
and `state::remove_security_configuration` (src/state.rs:1835-1848) only removes the record and its idempotency entry — it consults no virtual cluster.

The reference the clusters keep reporting: src/state.rs:498 `pub security_configuration_id: Option<String>,` on `VirtualClusterRecord`, rendered on every read at src/state.rs:558 `security_configuration_id: self.security_configuration_id.clone(),`.

Test coverage: the rules tests/all/security_configuration_delete.rs:14-25 enumerate are (1) a deleted configuration stops resolving, (2) a delete names exactly one resource, (3) an unknown id is refused, (4) a delete frees one slot — none of them creates a virtual cluster that references the configuration, so no test pins the in-use delete; tests/all/virtual_cluster_references.rs only covers the create-time refusal ("a `securityConfigurationId` naming no stored" configuration, line 14).

## Steps to Reproduce

1. POST /securityconfigurations {name, clientToken, securityConfigurationData} -> 200, note the id S.
2. POST /virtualclusters {name, clientToken, containerProvider, "securityConfigurationId": S} -> 200, note the cluster id V.
3. DELETE /securityconfigurations/S -> actual: 200 reporting S deleted, with no regard for the cluster that references it.
4. GET /virtualclusters/V -> 200, still reporting "securityConfigurationId": S for a RUNNING cluster.
5. GET /securityconfigurations/S -> 400 ResourceNotFoundException "Security Configuration S doesn't exist." — the cluster now reports a reference that resolves to nothing, the state create_virtual_cluster.rs:181-185 says must never be persisted.
6. Compare: POST /virtualclusters with that same (now deleted) securityConfigurationId is refused with ResourceNotFoundException.
