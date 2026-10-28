Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# The "an AddressId must name a stored address, and a ForwardingAddressId is an address too" rule is written out four times

_Filed: 2026-10-27T22:22:47Z_
_Operation: CreateJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One rule — walk the request's `AddressId` and `ForwardingAddressId`, refuse with InvalidResourceException (resource_type "address") the first one the store does not hold — is implemented four times, once per operation that names an address: create_job.rs, create_cluster.rs, update_job.rs and update_cluster.rs. Each copy re-writes the loop, the `store.addresses.contains_key` test, the message and the resource_type, and each carries its own copy of the justification comment ("A forwarding address is an address too, so the same rule holds for it").

The four copies can drift: a change to which members count as addresses, or to how an unresolvable address is refused, has to be made in four places, and nothing links them. This crate already has the pattern for fixing exactly this shape of duplication — `state::ClusterOrderMembers` was extracted so "the two operations cannot drift into disagreeing about what a cluster's order carries" — so the natural fix is one state-side helper answering the unresolvable address id, with each handler keeping its own error enum and its own refusal ordering (UpdateCluster deliberately takes the answer before the cluster is borrowed, so the cluster-not-found and state refusals still come first). Every response and every existing test stays byte-identical: all four copies already emit the same message and resource_type.

## Evidence

Copy 1 — src/ops/create_job.rs:386-405:

        for address_id in [
            input.address_id.as_deref(),
            input.forwarding_address_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !store.addresses.contains_key(address_id) {
                return Err(CreateJobErrorModeled::InvalidResourceException(
                    InvalidResourceException {
                        message: Some(format!(
                            "The address ID {address_id} can't be found. Check the information \
                             you provided in your last request, and try again."
                        )),
                        resource_type: Some("address".to_string()),

Copy 2 — src/ops/create_cluster.rs:387-406, the same loop over `[Some(input.address_id.as_str()), input.forwarding_address_id.as_deref()]` with the identical message and `resource_type: Some("address".to_string())`, prefaced by "A cluster is built from the address its devices ship to, exactly as a job is".

Copy 3 — src/ops/update_job.rs:313-332, the same loop again:

        for address_id in [
            input.address_id.as_deref(),
            input.forwarding_address_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !store.addresses.contains_key(address_id) {
                return Err(UpdateJobErrorModeled::InvalidResourceException(

Copy 4 — src/ops/update_cluster.rs:215-222 and 257-268, the same rule as a `find` plus a deferred refusal:

        let unknown_address: Option<String> = [
            input.address_id.as_deref(),
            input.forwarding_address_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|address_id| !store.addresses.contains_key(*address_id))
        .map(str::to_string);

The crate's own precedent for extracting a rule shared by these very operations — src/state.rs:1144-1148: "(record, request) pair in each handler: the two operations cannot drift into disagreeing about what a cluster's order carries, and a member added to or removed from the set is changed in this one place."

Tests that pin all four refusals (so the extraction is observable if it changes anything): tests/all/cluster_node_job_members.rs:551 `an_address_a_job_names_has_to_be_stored_whether_or_not_it_joins_a_cluster` (rows at 578-591), tests/all/job_lifecycle.rs:776 ("a ForwardingAddressId that was never created"), tests/all/cluster_creation.rs:563-570, tests/all/job_updates.rs:418-426, tests/all/cluster_updates.rs:460-468.

## Steps to Reproduce

1. Read src/ops/create_job.rs:386-405 — the two-member loop, the contains_key test, the message and resource_type "address".
2. Read src/ops/create_cluster.rs:387-406 — the same rule again for CreateCluster's required AddressId plus ForwardingAddressId.
3. Read src/ops/update_job.rs:313-332 — the same loop a third time.
4. Read src/ops/update_cluster.rs:215-222 — the same rule a fourth time, as a `find` whose refusal is raised at 257-268.
5. Compare with src/state.rs:1144-1148 (ClusterOrderMembers), the helper this crate already extracted so the update operations could not disagree, and note no such helper exists for the address rule.
