Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2afcd9ff504122666b9644d9cff6b63f96e63ba3

# An `Options` spelling out the modelled defaults counts as a change on an attachment stored with no `Options`, against the rule `same_vpc_options` states

_Filed: 2026-10-29T08:30:28Z_
_Operation: UpdateVpcAttachment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2afcd9ff504122666b9644d9cff6b63f96e63ba3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::same_vpc_options` is the one comparison both VPC-attachment handlers use to decide whether a request asks for the options the attachment already has. Its own documentation states that "an absent `Options` asks for the modelled defaults", but the implementation compares `Option<&VpcOptions>` by mapping each side through `options.map(...)`, so an ABSENT options set (`None`) and one that spells out the modelled defaults (`Some(VpcOptions{ Ipv6Support: false, ApplianceModeSupport: false, DnsSupport: false, SecurityGroupReferencingSupport: false })`) compare as DIFFERENT. Every member of `VpcOptions` carries a modelled `@default` of `false`, which the handler and the test helper both state, so the two spellings ask for exactly the same thing.

Two consequences follow from that one helper:

1. `UpdateVpcAttachment`: for an attachment created without `Options` (stored `None`), an update carrying `Options` with all four flags false — including the empty object `"Options": {}`, which the modelled defaults fill in — falls through the handler's own no-op branch. The record is rewritten, `changed_at`/`available_at` are set, so the attachment is answered `UPDATING` with an advanced `UpdatedAt` and starts reporting an `Options` object where it reported none, for a request that asks for the state it is already in. This is precisely the rule the handler states one line above the branch, and the rule the crate's other attachment updater implements.
2. `CreateVpcAttachment`: the same helper is the `Options` half of the `ClientToken` replay matcher, so a retry under the same token that spells the defaults out (while the stored attachment holds `None`, or the reverse) is not recognised as the same request and is answered `ConflictException` (409) "…already created attachment …, with a different request" instead of replaying the stored attachment.

## Evidence

The helper's own rule, contradicted by its own body — src/state.rs:8304-8318:
```
/// Whether two `VpcOptions` ask for the same thing. See [`same_connect_options`]; an
/// absent `Options` asks for the modelled defaults, which is what an absent one on the
/// other side asks for too, so two absent options are the same request.
pub fn same_vpc_options(left: Option<&VpcOptions>, right: Option<&VpcOptions>) -> bool {
    let members = |options: Option<&VpcOptions>| {
        options.map(|options| {
            (
                options.ipv6_support,
                options.appliance_mode_support,
                options.dns_support,
                options.security_group_referencing_support,
            )
        })
    };
    members(left) == members(right)
}
```
`members(None) == None` while `members(Some(all-false)) == Some((false,false,false,false))`, so the two are unequal.

The model that makes the two spellings identical — `smithy_query query="shape" shape="VpcOptions"`: `"Ipv6Support": {"target": "Boolean", "default": false, ...}`, `"ApplianceModeSupport": {... "default": false ...}`, `"DnsSupport": {... "default": false ...}`, `"SecurityGroupReferencingSupport": {... "default": false ...}`.

The handler stating the same reading, and the no-op rule it breaks — src/ops/update_vpc_attachment.rs:315-331:
```
        // `Options` names the options the attachment is to have, so a request carrying it
        // REPLACES all four flags — every member of `VpcOptions` carries a modelled
        // `@default` of `false` (`smithy_query shape=VpcOptions`), so one the request
        // leaves out is off rather than left alone — and a request carrying none leaves the
        // stored options as they are.
        let options = input.options.or_else(|| held_options.clone());

        // A request that asks for the state the attachment is already in changes nothing:
        // it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and
        // writes nothing, because there is no change for the service to apply.
        if subnet_arns == held_subnets
            && crate::state::same_vpc_options(options.as_ref(), held_options.as_ref())
        {
```
and the write it then performs — src/ops/update_vpc_attachment.rs:336-341: `record.changed_at = Some(now); record.available_at = Some(now + crate::state::SETTLE_DELAY);`.

A stored `None` is reported as an absent member, so a client reading the attachment back cannot tell which spelling it must send — src/state.rs:2981: `options: options.clone(),`.

The crate's own test helper states the equivalence the helper denies — tests/all/vpc_attachment_lifecycle.rs:56-60:
```
/// The four `VpcOptions` flags an attachment reports. An absent member, like an absent
/// `Options`, is the modelled default (`false`), so both spellings of "off" read alike.
```
Test coverage: tests/all/vpc_attachment_lifecycle.rs:443-503 `an_update_that_asks_for_no_change_changes_nothing` pins the no-op rule ("Each row asks for the state the attachment is already in: nothing to apply, so the attachment is not moved to UPDATING and `UpdatedAt` does not advance"), but its "the options it already has" row creates the attachment WITH `Options` (:454 `"Options": options_body([true, false, true, false])`), so no row covers an attachment created without `Options`; no row of `UPDATE_CASES` (:127-198) pairs `created_options: None` with `options: Some([false, false, false, false])` either, so no existing test pins the current behaviour.

The second symptom, the replay matcher — src/ops/create_vpc_attachment.rs:264-268:
```
                            *subnet_arns == input.subnet_arns
                                && crate::state::same_vpc_options(
                                    options.as_ref(),
                                    input.options.as_ref(),
                                )
```
with the refusal it reaches — src/state.rs:8268-8272: `message: format!("client token {token:?} already {effect}, with a different request")`.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document configuring an edge in us-west-2
2. POST /vpc-attachments with {"CoreNetworkId": <id>, "VpcArn": "arn:aws:ec2:us-west-2:<account>:vpc/vpc-0123456789abcdef0", "SubnetArns": ["arn:aws:ec2:us-west-2:<account>:subnet/subnet-0123456789abcdef0"], "ClientToken": "t1"} and NO Options member -> 200; the response carries no `Options`
3. Wait past the settle span, then GET /vpc-attachments/{AttachmentId} -> State AVAILABLE, no `Options` member, UpdatedAt == CreatedAt
4. PATCH /vpc-attachments/{AttachmentId} with {"Options": {"Ipv6Support": false, "ApplianceModeSupport": false, "DnsSupport": false, "SecurityGroupReferencingSupport": false}} (or just {"Options": {}}) -> 200, but State is UPDATING, UpdatedAt has advanced and the attachment now reports an `Options` object — although all four flags are the defaults the absent `Options` already meant
5. Repeat the same PATCH once the attachment has settled -> now a no-op, confirming the first call applied a change nobody asked for
6. Idempotency symptom: repeat step 2 verbatim with the same "ClientToken": "t1" but with the all-false `Options` spelled out -> 409 ConflictException "client token \"t1\" already created attachment …, with a different request", instead of replaying the stored attachment
