Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: dbfb16c0807859a588c77add63fc25981d8f50b1

# The resource-set item classifier never weighs the ARN's service field, so NOT_VALID_SERVICE is a modelled reason the service can never report and a foreign-service ARN is associated

_Filed: 2026-10-28T22:37:18Z_
_Operation: BatchAssociateResource | Class: validation_

> _Note: this issue was identified against the code base at commit `dbfb16c0807859a588c77add63fc25981d8f50b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::resource_set_item_failure_reason` is the one classifier both batch membership writers use to decide whether an item may become a member of a resource set. It parses the item's ARN into all five fields (`ParsedArn { partition, service, region, account, resource }`) and then weighs the partition, the Region, the account and the resource field's type keyword — but never the SERVICE field. The parsed `service` is used nowhere except an emptiness check inside `parse_arn_form`.

Two consequences:

1. `FailedItemReason` declares six values and the crate's own enum list carries all six, but only five can ever be produced. `NOT_VALID_SERVICE` is dead: no code path in this crate emits it, so an item that disagrees with the set on exactly that field is reported under no reason at all.
2. Because the type is read only off the resource field's keywords, an ARN whose service field names a service that does not own the declared resource type is classified as associable and is written into the set's membership. `arn:aws:iam:us-east-1:<account>:instance/i-0123456789abcdef0` has resource-field keyword `instance`, which matches the declared `AWS::EC2::Instance`, so it is associated and afterwards reported by `ListResourceSetResources` and `GetResourceSet` as a member — although no emulated service holds it and `ListDiscoveredResources` can never report it (`held_resources` reads only the store of the service that OWNS each type, so every URI the inventory yields carries that service in its ARN). The classifier's own doc claims the opposite: that its readings are "the same readings the discoverable inventory makes".

The check is refused-item classification, not a cap: the model names the field and gives it its own reason value.

## Evidence

The model declares the reason the crate never produces — `smithy_query(query="shape", shape="FailedItemReason")`:
```
"FailedItemReason": {"type": "enum", "values": ["NOT_VALID_ARN", "NOT_VALID_PARTITION", "NOT_VALID_REGION", "NOT_VALID_SERVICE", "NOT_VALID_RESOURCE_TYPE", "NOT_VALID_ACCOUNT_ID"]}
```
and the crate carries all six — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/types/shared_types.rs:273`:
```
pub const FAILED_ITEM_REASON_VALUES: &[&str] = &["NOT_VALID_ACCOUNT_ID", "NOT_VALID_ARN", "NOT_VALID_PARTITION", "NOT_VALID_REGION", "NOT_VALID_RESOURCE_TYPE", "NOT_VALID_SERVICE"];
```

The classifier's own doc enumerates the fields it weighs "in the order the ARN is built" and silently omits the service field — `src/state.rs:2711-2726`:
```
/// Every reason is a value of the model's `FailedItemReason`
/// (`crate::ops::types::FAILED_ITEM_REASON_VALUES`), and the checks run in the
/// order the ARN is built, so an item is reported against the FIRST field that
/// disagrees:
///
/// * not an ARN at all -> `NOT_VALID_ARN`;
/// * another partition than the one this emulator's resources live in
///   (`partition`) -> `NOT_VALID_PARTITION`;
/// * another Region than the one the request addresses -> `NOT_VALID_REGION`,
/// * an account that is not in this organization (`accounts`) ->
///   `NOT_VALID_ACCOUNT_ID`;
/// * a resource whose type no entry of the set's `ResourceTypeList` names ->
///   `NOT_VALID_RESOURCE_TYPE`
```

The code, which matches that list exactly — `src/state.rs:2749-2769`:
```
) -> Option<&'static str> {
    let Some(arn) = parse_arn(uri) else {
        return Some("NOT_VALID_ARN");
    };
    if arn.partition != partition {
        return Some("NOT_VALID_PARTITION");
    }
    if !arn_in_region(&arn, region) {
        return Some("NOT_VALID_REGION");
    }
    if !arn_in_accounts(&arn, accounts) {
        return Some("NOT_VALID_ACCOUNT_ID");
    }
    let keywords = arn_type_keywords(arn.resource);
    let in_scope = resource_types
        .iter()
        .any(|declared| keywords_name_resource_type(&keywords, declared));
    (!in_scope).then_some("NOT_VALID_RESOURCE_TYPE")
}
```

The field is parsed and then unused — `src/state.rs:2834-2840` declares `pub service: &'a str,` and the ONLY read of it in the crate is the emptiness check at `src/state.rs:2909-2912`:
```
    match (parsed.partition, parsed.service, parsed.resource) {
        ("", _, _) => Err(ArnFormDefect::EmptyField("partition")),
        (_, "", _) => Err(ArnFormDefect::EmptyField("service")),
```
(`grep -n '\.service' src/` returns only state.rs:2909.)

The claim the loose classification breaks — `src/state.rs:2734-2737`:
```
/// Those are the same readings the discoverable inventory makes
/// ([`in_region`], [`discovered_resources`]), which is what keeps a resource
/// reported as "available to be associated with a resource set" one these
/// operations actually take.
```
while the inventory only ever yields ARNs minted by the owning service — `src/state.rs:3506-3519` dispatches `held_resources` per resource type to that type's owning service store (EC2, ELBv2, CloudFront, API Gateway, Network Firewall).

The caller — `src/ops/batch_associate_resource.rs:286-299` feeds every `input.items` entry through it and pushes the unclassified ones straight into the membership at `batch_associate_resource.rs:330-334`.

Test coverage: `tests/all/resource_set_disassociation_membership_invariants.rs:73-79` names the full six-value enum but generates only three variants — `NOT_VALID_ARN`, `NOT_VALID_REGION`, `NOT_VALID_RESOURCE_TYPE` — "one per ARN field a generated set can disagree with"; no test sends an item whose service field disagrees, and `tests/all/associable_discovered_resources.rs` only asserts the inventory-reported direction.

## Steps to Reproduce

1. POST / with X-Amz-Target AWSFMS_20180101.AssociateAdminAccount and body {"AdminAccount": "<the emulator account id>"} — 200.
2. POST / AWSFMS_20180101.PutResourceSet with {"ResourceSet": {"Name": "svc", "ResourceTypeList": ["AWS::EC2::Instance"]}} and note the minted Id.
3. POST / AWSFMS_20180101.BatchAssociateResource with {"ResourceSetIdentifier": "<Id>", "Items": ["arn:aws:iam:us-east-1:<the emulator account id>:instance/i-0123456789abcdef0"]}.
4. Observed: HTTP 200 with FailedItems: [] — the item was associated, although its service field is `iam` and nothing in the emulator (nor in AWS IAM) owns an `AWS::EC2::Instance` under that service.
5. POST / AWSFMS_20180101.ListResourceSetResources with {"Identifier": "<Id>"} — the foreign-service ARN is reported as a member of the set.
6. POST / AWSFMS_20180101.ListDiscoveredResources with {"MemberAccountIds": ["<account>"], "ResourceType": "AWS::EC2::Instance"} — that URI is never reported, so the set holds a member the discovery inventory can never produce.
7. Expected (the modelled reason the crate lists but never emits): the item comes back in FailedItems with Reason NOT_VALID_SERVICE and is not associated.
