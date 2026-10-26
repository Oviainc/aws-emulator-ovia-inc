Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d25a8df08abf64a2b119d49e5e317e50f9894e06

# BatchAssociateResource refuses resources the discovery inventory reports as associable (global, account-less and EIP ARNs)

_Filed: 2026-10-26T01:22:49Z_
_Operation: BatchAssociateResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d25a8df08abf64a2b119d49e5e317e50f9894e06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The item classifier `crate::state::resource_set_item_failure_reason` — the one both batch operations use — compares the ARN's Region field for strict equality with the request's Region and requires the ARN's account field to be a known organization account. The discovery inventory this crate reads (and that `ListDiscoveredResources` reports) deliberately admits ARNs that leave those fields EMPTY, and `crate::state::in_region`'s own doc says that is what keeps the two in agreement. The two therefore disagree for three of the ten entries of `DISCOVERABLE_RESOURCE_TYPES`:

* `AWS::CloudFront::Distribution` — a distribution's URI is `arn:aws:cloudfront::<account>:distribution/<Id>` (empty Region), so `arn.region != region` always holds and every discovered distribution comes back in `FailedItems` with `NOT_VALID_REGION`; it can never be a member of any resource set in any Region.
* `AWS::ApiGateway::Stage` — a stage's URI is `arn:aws:apigateway:<region>::/restapis/<id>/stages/<name>` (empty account), so `accounts.contains(arn.account)` is false and the item fails with `NOT_VALID_ACCOUNT_ID`; the resource-type segment also parses as the empty string (`arn_resource_type` splits at the leading `/`), so it could not match `Stage` either.
* `AWS::EC2::EIP` — the inventory mints `arn:aws:ec2:<region>:<account>:elastic-ip/<AllocationId>`, and the classifier normalises that resource type to `elasticip` while the declared type `AWS::EC2::EIP` normalises to `eip`, so a discovered Elastic IP fails with `NOT_VALID_RESOURCE_TYPE` even when the set's `ResourceTypeList` names `AWS::EC2::EIP`.

The effect is a 200 whose `FailedItems` rejects resources the very same crate reports as "available to be associated with a resource set", and nothing is added to the set's membership. `BatchDisassociateResource` inherits the same classifier, so such an item is also reported as a failure there.

## Evidence

The classifier (strict Region equality, account membership), `services/.../src/state.rs:2249-2265`:
```
    if arn.partition != partition {
        return Some("NOT_VALID_PARTITION");
    }
    if arn.region != region {
        return Some("NOT_VALID_REGION");
    }
    if !accounts.contains(arn.account) {
        return Some("NOT_VALID_ACCOUNT_ID");
    }
    let resource_type = normalized_type_name(arn_resource_type(arn.resource));
```
called from the handler at `services/.../src/ops/batch_associate_resource.rs:248-254`: `match crate::state::resource_set_item_failure_reason(uri, &region, &partition, &accounts, &resource_types)`.

The other side, `services/.../src/state.rs:2599-2609`:
```
/// Whether `uri` names a resource of `region`: an ARN whose Region field is
/// `region`, or one that carries no Region at all (a global service's ARN, such
/// as CloudFront's).
...
/// The check is what keeps the inventory and
/// `BatchAssociateResource` in agreement: a resource set holds the resources of
/// the administrator's Region, so a resource reported as discoverable in a
/// Region is one that Region's resource sets can actually take.
fn in_region(uri: &str, region: &str) -> bool {
    parse_arn(uri).is_some_and(|arn| arn.region.is_empty() || arn.region == region)
}
```
and `services/.../src/state.rs:2826-2828`: "CloudFront is global, so a distribution's stored ARN carries no Region and it is discoverable from every Region." (CloudFront mints exactly that form: `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs:4968-4970` `::engine::emulator_arn("cloudfront", "", &format!("distribution/{id}"))`.)

Empty fields are legitimate elsewhere in the same file — `services/.../src/state.rs:2318-2321`: "The region and account fields MAY be empty — a global service's ARN leaves them so", and `services/.../src/state.rs:1772-1773`: "Region and account id are deliberately allowed to be empty — ARNs of global and account-less resources leave them so."

The account invariant the API Gateway rows break, `services/.../src/state.rs:2588-2591`: "the reported `AccountId` and the account segment of the reported `URI` must name the SAME account, or a resource set would be offered a URI its account check refuses" — while `services/.../src/state.rs:2853-2856` records the opposite for stages: "An API Gateway ARN carries an EMPTY account field (`arn:aws:apigateway:<region>::/restapis/<id>/stages/<name>`...), so the owning account is this emulator's rather than the URI's."

The EIP URI, `services/.../src/state.rs:2749-2757`: `uri: arn(&account, &format!("elastic-ip/{id}"))`, against the type match at `services/.../src/state.rs:2258-2264` which compares the last `::` segment of the declared type (`EIP` -> `eip`) with `elastic-ip` -> `elasticip`.

Test coverage: the rule is stated by `tests/all/discovered_resources.rs:1055-1058` — "RULE: what is discoverable is 'available to be associated with a resource set', so `BatchAssociateResource` accepts a discovered resource's URI — the two read the same inventory" — but that test only exercises `AWS::NetworkFirewall::Firewall`. `tests/all/discovered_resources.rs:561-563,608-614,626` assert the inventory DOES report the empty-account stage URI and the `elastic-ip/<allocation>` URI. No test pins the refusal of an empty-Region, empty-account or `elastic-ip` item: the only `NOT_VALID_REGION` row (`tests/all/administrator_gated_operations.rs:916-919`) uses an explicitly different Region, and the only `NOT_VALID_RESOURCE_TYPE` row (lines 924-929) a load balancer against a firewall/instance set.

## Steps to Reproduce

1. Mount the FMS emulator and the CloudFront, API Gateway and EC2 emulators on one shared engine and call AssociateAdminAccount.
2. Create a CloudFront distribution, an API Gateway REST API with a stage, and allocate an EC2 Elastic IP in the calling account.
3. Call ListDiscoveredResources for AWS::CloudFront::Distribution, AWS::ApiGateway::Stage and AWS::EC2::EIP and note each reported URI.
4. PutResourceSet with ResourceTypeList ["AWS::CloudFront::Distribution","AWS::ApiGateway::Stage","AWS::EC2::EIP"].
5. Call BatchAssociateResource with the resource set id and the three discovered URIs as Items.
6. Observed: 200 with every item in FailedItems (NOT_VALID_REGION for the distribution, NOT_VALID_ACCOUNT_ID for the stage, NOT_VALID_RESOURCE_TYPE for the Elastic IP) and ListResourceSetResources reporting an empty membership. Expected (per in_region's and the discovery test's own stated rule): the discovered URIs are accepted and become members.
