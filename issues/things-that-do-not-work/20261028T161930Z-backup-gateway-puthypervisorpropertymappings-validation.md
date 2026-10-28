Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ef3dfae520d72d664802e5111b62d3d016960f9c

# PutHypervisorPropertyMappings stores AwsTagKey/AwsTagValue without the tag rules every other tag path in this crate enforces

_Filed: 2026-10-28T16:19:30Z_
_Operation: PutHypervisorPropertyMappings | Class: validation_

> _Note: this issue was identified against the code base at commit `ef3dfae520d72d664802e5111b62d3d016960f9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate funnels every AWS tag key/value it is asked to persist through one validator, `crate::state::merged_tags`, which refuses the two rules the model states in prose and therefore the boundary validator cannot refuse: a key with the reserved `aws:` prefix, and a value containing spaces. `CreateGateway`, `ImportHypervisorConfiguration` and `TagResource` all call it (through `tag_map` / `merge_resource_tags`) before anything is written.

`PutHypervisorPropertyMappings` persists AWS tag keys and values too — `VmwareToAwsTagMapping$AwsTagKey` targets the very same `TagKey` shape as `Tag$Key`, and `AwsTagValue` targets the same `TagValue` shape as `Tag$Value` — but its handler builds `StoredPropertyMappings` straight out of the owned input and hands it to `set_property_mappings` without calling `merged_tags`, `tag_map` or any other check. So a mapping whose `AwsTagKey` is `aws:created-by`, or whose `AwsTagValue` contains spaces, is accepted with HTTP 200, persisted on the hypervisor record, and reported verbatim by `GetHypervisorPropertyMappings` — while the identical key/value pair sent to `TagResource` on the same hypervisor is refused with `ValidationException`. One crate, one pair of modelled shapes, two answers.

The defect is the skipped helper, not a new feature: the validator already exists in this crate's own state module and three sibling handlers call it.

## Evidence

The rule, stated once in this crate — `src/state.rs:2011-2026` (doc of `merged_tags`):

    /// Merge a request's `Tags` into the tags a resource already carries, refusing what the
    /// model's own traits cannot express.
    ///
    /// Three rules, all from the model's docs rather than its traits — so the boundary
    /// validator cannot have refused them already:
    ///
    /// * `Tag$Key` "can't start with `aws:`" (that prefix is reserved for AWS-generated
    ///   tags), which the member's `@pattern` allows;
    /// * "Spaces are not allowed in tag values" (the `Tag` shape's own doc), which
    ///   `TagValue`'s `@pattern` of `^[^\x00]*$` admits …

and its enforcement, `src/state.rs:2032-2046`:

    for tag in requested {
        if tag.key.to_ascii_lowercase().starts_with("aws:") {
            return Err(format!(
                "Value '{}' at 'Tags' failed to satisfy constraint: a tag key must not \
                 start with the reserved prefix 'aws:'",

Sides that APPLY it — `src/ops/tag_resource.rs:166`:

        let Some(merged) = state.merge_resource_tags(&input.resource_arn, &input.tags) else {

(`merge_resource_tags` delegates to `merged_tags`, `src/state.rs:1922`), `src/ops/create_gateway.rs:167`:

        let tags = crate::state::tag_map(input.tags.as_deref().unwrap_or(&[])).map_err(

and `src/ops/import_hypervisor_configuration.rs:167` (identical call, with the comment at `:165-166` "What the model does NOT express is the tag rules, which `tag_map` states and refuses").

Side that SKIPS it — `src/ops/put_hypervisor_property_mappings.rs:228-232`:

        let mappings = crate::state::StoredPropertyMappings {
            iam_role_arn: input.iam_role_arn.clone(),
            vmware_to_aws_tag_mappings: input.vmware_to_aws_tag_mappings.clone(),
        };
        let Some(updated) = store.set_property_mappings(&input.hypervisor_arn, mappings) else {

The only validation claim the handler makes is the modelled one, `src/ops/put_hypervisor_property_mappings.rs:200-203`:

        // `HypervisorArn`'s and `IamRoleArn`'s lengths and patterns, and every member of
        // every `VmwareToAwsTagMapping`, are refused by the boundary validator before
        // `handle` runs, so reaching here means the input satisfies all of them. What is
        // left is a fact of the STORE: which appliance would apply these mappings.

That the two members are the same modelled shapes — `smithy_query(query="shape",shape="VmwareToAwsTagMapping,Tag")`:

    "AwsTagKey": {"target": "TagKey", "required": true, "length": {"min": 1, "max": 128}, "pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$", "doc": "<p>The key part of the Amazon Web Services tag's key-value pair.</p>"},
    "AwsTagValue": {"target": "TagValue", "required": true, "length": {"min": 0, "max": 256}, "pattern": "^[^\\x00]*$", …}

    "Key": {"target": "TagKey", …, "doc": "<p>The key part of a tag's key-value pair. The key can't start with <code>aws:</code>.</p>"}

Test coverage: the `aws:` rule is pinned only for `Tag` lists — `tests/all/gateway_lifecycle.rs:844-854` ("reserved aws: tag key on create" / "reserved aws: tag key on tag") and `tests/all/hypervisor_reads.rs:913-922` ("A tag key with the reserved `aws:` prefix is refused"). The mapping suites exercise only the modelled bounds — `tests/all/put_hypervisor_property_mappings.rs:499` and `:574-587` ("an AwsTagKey one character over its 128-character maximum", "an AwsTagValue one character over its 256-character maximum") — so no test pins the current permissive behaviour for `AwsTagKey`/`AwsTagValue`.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted.
2. Perform the activation handshake (GET /?activationRegion=us-east-1&no_redirect) and CreateGateway with the issued key; note the GatewayArn.
3. Call ImportHypervisorConfiguration (Name, Host) and note the HypervisorArn.
4. Call AssociateGatewayToServer with that GatewayArn and the HypervisorArn as ServerArn, so the PutHypervisorPropertyMappings gateway gate passes.
5. Call TagResource with ResourceARN = the HypervisorArn and Tags = [{Key: "aws:created-by", Value: "me"}] -> HTTP 400 ValidationException, "a tag key must not start with the reserved prefix 'aws:'".
6. Call PutHypervisorPropertyMappings with that HypervisorArn, a valid IamRoleArn, and VmwareToAwsTagMappings = [{VmwareCategory: "env", VmwareTagName: "prod", AwsTagKey: "aws:created-by", AwsTagValue: "two words"}] -> HTTP 200, and the mapping is persisted.
7. Call GetHypervisorPropertyMappings for the same HypervisorArn -> the response reports AwsTagKey "aws:created-by" and AwsTagValue "two words", the exact key and value TagResource refused in step 5.
