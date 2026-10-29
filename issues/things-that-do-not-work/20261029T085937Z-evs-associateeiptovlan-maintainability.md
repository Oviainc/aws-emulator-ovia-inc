Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# The three-step "addressed VLAN must be the public hcx VLAN" gate is written out twice, once in each Elastic IP handler

_Filed: 2026-10-29T08:59:37Z_
_Operation: AssociateEipToVlan | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateEipToVlan` and `DisassociateEipFromVlan` each resolve the VLAN the request addresses with the same three branches in the same order, spelled out independently:

1. refuse unless `vlanName == crate::state::HCX_FUNCTION` ("hcx is the only accepted VLAN name at this time");
2. find the environment's VLAN whose `function_name` is `HCX_FUNCTION`, refusing with "names no VLAN of this environment" when there is none;
3. refuse unless that VLAN's `is_public` is set ("This operation is only allowed for public HCX VLANs at this time").

Both copies build the refusal through `crate::state::field_validation_error("vlanName", …)`; the only difference is the verb in two of the sentences ("associated with" / "disassociated from", "an Elastic IP address can only be associated with" / "disassociated from"). Nothing in `crate::state` owns this resolution, so a change to any of the three rules — which name is accepted, what a missing HCX VLAN answers, what counts as public — has to be made in two places and the copies can drift. They already differ in which lookup they use (`environment_in_region` vs `environment_in_region_mut`).

A shared helper in `crate::state` returning the addressed VLAN (or the refusal), parameterised by the verb the two operations word their sentences with, leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy one — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/associate_eip_to_vlan.rs:276-315:

        // "hcx is the only accepted VLAN name at this time" (`vlanName` doc) ...
        if input.vlan_name != crate::state::HCX_FUNCTION {
            return Err(AssociateEipToVlanErrorModeled::ValidationException(
                crate::state::field_validation_error(
                    "vlanName",
                    &format!(
                        "must be {}, the only VLAN an Elastic IP address can be associated \
                         with at this time", crate::state::HCX_FUNCTION),
        ...
        let hcx = environment.vlans.iter()
            .find(|vlan| vlan.function_name == crate::state::HCX_FUNCTION)
            .ok_or_else(|| ... "names no VLAN of this environment" ...)?;
        // "This operation is only allowed for public HCX VLANs at this time" ...
        if !hcx.is_public {
            ... "names a private VLAN; an Elastic IP address can only be associated with a public HCX VLAN" ...

Copy two — src/ops/disassociate_eip_from_vlan.rs:216-264:

        // "hcx is the only accepted VLAN name at this time" (`vlanName` doc).
        let vlan_name_is_hcx = input.vlan_name == crate::state::HCX_FUNCTION;
        ...
        if !vlan_name_is_hcx {
            ... "must be {}, the only VLAN an Elastic IP address can be disassociated from at this time" ...
        let vlan = environment.vlans.iter_mut()
            .find(|vlan| vlan.function_name == crate::state::HCX_FUNCTION)
            .ok_or_else(|| ... "names no VLAN of this environment" ...)?;
        // "This operation is only allowed for public HCX VLANs at this time."
        if !vlan.is_public {
            ... "names a private VLAN; an Elastic IP address can only be disassociated from a public HCX VLAN" ...

For contrast, every other rule these two handlers share IS stated once in `crate::state`: `HCX_FUNCTION` (state.rs:173), `field_validation_error` (state.rs:415), `note_modified_at` (state.rs:360), `eip_association_not_found` (state.rs:629), `release_elastic_ips`.

Existing tests that cover both copies and must stay green: tests/all/hcx_eip_associations.rs, tests/all/public_vlan_netmask.rs, tests/all/eip_association_existence.rs, tests/all/eip_allocation_existence.rs, tests/all/eip_ec2_propagation.rs.

## Steps to Reproduce

1. Open src/ops/associate_eip_to_vlan.rs and read lines 276-315.
2. Open src/ops/disassociate_eip_from_vlan.rs and read lines 216-264.
3. Compare: the same three branches in the same order, with the same member name and the same helper, differ only in the verb inside two sentences — and in whether the environment is borrowed immutably or mutably.
4. Change any one of the three rules (for example, make a second VLAN name acceptable) and observe that both files must be edited for the two operations to agree.
