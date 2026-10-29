Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7

# A CustomerGatewayArns entry that is no customer gateway ARN at all is silently matched as a filter, although the crate single-homes that refusal and its prefix-list sibling applies it to its own ARN FILTER

_Filed: 2026-10-29T15:11:19Z_
_Operation: GetCustomerGatewayAssociations | Class: validation_

> _Note: this issue was identified against the code base at commit `7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetCustomerGatewayAssociations` applies exactly one check to its `CustomerGatewayArns` filter — the documented 10-entry maximum via `crate::state::check_filter_ids` — and then matches every entry literally against stored records. A string that names no customer gateway at all (not an ARN, another service's namespace, another EC2 resource type, a `cgx-` id, a non-hex id) is therefore accepted and quietly selects nothing, so the caller is answered 200 with an empty page instead of the field failure the crate already builds for exactly that input class.

The crate has the helper (`crate::state::parse_customer_gateway_arn`) and the refusal (`crate::state::malformed_customer_gateway_arn`), whose own doc says it is the answer "a customer gateway association operation" gives when the member "is not a customer gateway ARN at all", explicitly distinguishing it from "a well-formed ARN that names no association this global network holds". Both customer-gateway write handlers call it, and the crate's other ARN-FILTER listing — `ListCoreNetworkPrefixListAssociations` — makes the same distinction for its own filter member and refuses a malformed one. This read is the only customer-gateway operation that skips it, so one malformed ARN gets two different answers depending on which customer-gateway operation it arrives on.

## Evidence

The rule, single-homed in this crate's state module — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5771-5777:
```
/// The modelled `ValidationException` (HTTP 400) a customer gateway association operation
/// answers when the string in its member `member` is not a customer gateway ARN at all
/// (see [`parse_customer_gateway_arn`]).
///
/// Like [`malformed_prefix_list_arn`]: a string that names no customer gateway is a FIELD
/// failure on the member that carried it, a different answer from a well-formed ARN that
/// names no association this global network holds.
pub fn malformed_customer_gateway_arn(member: &str) -> ValidationException {
```

The sibling ARN-FILTER listing applies exactly that distinction to a FILTER member — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/list_core_network_prefix_list_associations.rs:187-203:
```
// `PrefixListArn` is a FILTER here, not a resource reference ("The ARN of a
// specific prefix list to filter the associations"), so a string that names no
// managed prefix list at all is a field failure on the member that carried it —
// the same refusal the create and the delete answer for the same input class. A
// well-formed ARN naming no stored association is NOT an error: it selects nothing.
if let Some(filter) = input.prefix_list_arn.as_deref() {
    if crate::state::parse_prefix_list_arn(filter).is_none() {
        return Err(
            ListCoreNetworkPrefixListAssociationsErrorModeled::ValidationException(
                crate::state::malformed_prefix_list_arn(
                    crate::state::FIELD_PREFIX_LIST_ARN,
                ),
            )
            .into(),
        );
```

The two customer-gateway writers apply it — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/associate_customer_gateway.rs:164-166:
```
if crate::state::parse_customer_gateway_arn(&input.customer_gateway_arn).is_none() {
    return Err(AssociateCustomerGatewayErrorModeled::ValidationException(
        crate::state::malformed_customer_gateway_arn(
```
and services/service-com-amazonaws-networkmanager-networkmanager/src/ops/disassociate_customer_gateway.rs:148-160:
```
// `CustomerGatewayArn` is modelled free-form (it arrives in a path label, which
// the pattern `^[\s\S]*$` does not constrain), so the form of the ARN is judged
// here, the same way the associate judges it: a string that names no customer
// gateway at all is a field failure on the member that carried it, a different
// answer from a well-formed ARN this global network holds no association for.
if crate::state::parse_customer_gateway_arn(&input.customer_gateway_arn).is_none() {
```

The read skips it — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_customer_gateway_associations.rs:180-207 (the ONLY check on the member is the count, then the entries are matched literally):
```
// `CustomerGatewayArns` documents a maximum of 10 values that the modelled
// `CustomerGatewayArnList` does not express as `@length`, so a longer filter is
// refused ...
crate::state::check_filter_ids("CustomerGatewayArns", &requested)
    .map_err(GetCustomerGatewayAssociationsErrorModeled::ValidationException)?;
...
    .filter(|(_, record)| {
        requested.is_empty() || requested.contains(&record.customer_gateway_arn)
    })
```

Test coverage: tests/all/customer_gateway_association_lifecycle.rs:493-558 (`a_request_naming_nothing_stored_or_no_customer_gateway_is_refused_and_stores_nothing`) pins the 400 for the eight malformed spellings on `AssociateCustomerGateway` and `DisassociateCustomerGateway` only — it never sends one through the listing. The listing's own filter test (same file, lines 766-799) uses only well-formed `plain_cgw_arn(..)` values at and one past the 10 maximum, so adding the form check leaves it passing.

## Steps to Reproduce

1. Create a global network, a device and a link, and associate a customer gateway with the device (`POST /global-networks/{id}/customer-gateway-associations`).
2. Call `GET /global-networks/{id}/customer-gateway-associations?customerGatewayArns=cgw-0123456789abcdef1` (a bare id, one of the malformed spellings tests/all/customer_gateway_association_lifecycle.rs:511 feeds the associate).
3. Observe HTTP 200 with an empty `CustomerGatewayAssociations` list.
4. Send the same string to `DisassociateCustomerGateway` and observe HTTP 400 `ValidationException` with `Fields[0].Name = customerGatewayArn` — the same input class, answered two different ways.
5. Compare `GET /core-networks/{id}/prefix-list-associations?prefixListArn=pl-123` which IS refused 400 by src/ops/list_core_network_prefix_list_associations.rs:193.
