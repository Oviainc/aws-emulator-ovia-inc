Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 889b7a0ea20193cdb3c9577846ef2b90647eb8e8

# A ListDiscoveredResources NextToken is not scoped to the account/resource-type/Region selection that minted it, so a token from one selection silently empties another's page

_Filed: 2026-10-29T08:03:07Z_
_Operation: ListDiscoveredResources | Class: pagination_

> _Note: this issue was identified against the code base at commit `889b7a0ea20193cdb3c9577846ef2b90647eb8e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDiscoveredResources`' page set is fully SELECTED by request members: the handler builds the inventory from `crate::state::discovered_resources(&*self.engine, &region, account, &input.resource_type)`, i.e. from the request's Region, its single `MemberAccountIds` entry and its `ResourceType`. Every one of those is a different collection of keys.

Its `NextToken`, however, is a BARE resource URI: it is minted with `crate::state::encode_key_token(uri)` and decoded with `crate::state::decode_uri_page_token`, which checks only that the decoded value parses as an ARN. Nothing in the token records which selection it is a boundary of, so a token minted for (account A, `AWS::EC2::VPC`, us-east-1) is accepted verbatim as the lower bound of the (account A, `AWS::EC2::Subnet`, us-east-1) page — or of any other account/type/Region page.

Because the boundary is used straight as `inventory.range(lower..)`, every discovered resource of the new selection whose URI sorts BELOW that foreign key is dropped, and the walk then runs to the end so the response carries no `NextToken` at all: a truncated or completely empty page is indistinguishable from a complete one. Concretely, a VPC boundary `arn:aws:ec2:us-east-1:<acct>:vpc/vpc-...` sorts after every `.../subnet/...` ARN, so replaying it under `ResourceType: AWS::EC2::Subnet` answers 200 with `Items: []` and no token, although the account holds subnets.

This crate already states the rule and applies it in three sibling listings; `ListDiscoveredResources` is the one narrowed listing that does not, and it is also absent from the suite that was written to pin exactly this rule.

## Evidence

The crate's own statement of the rule, on the helper it built for it — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:3174-3192:
```
/// Encode a page boundary of a listing whose page set is SELECTED by a request
/// member — the accounts one policy applies to, the members of one resource set,
/// the lists of one namespace — as an opaque `PaginationToken` carrying the
/// selecting value beside the boundary.
///
/// A boundary of such a page names nothing in another selection: the key may be
/// absent there, or sort after items that selection does hold, so resuming from
/// it would silently drop them. Carrying the selecting value is what lets
/// [`decode_scoped_page_token`] tell a token minted for THIS page from one
/// minted for another
...
pub fn encode_scoped_page_token(scope: &str, key: &str) -> String {
    encode_key_token(&scoped_page_key(scope, key))
}
```

What `ListDiscoveredResources` does instead — src/ops/list_discovered_resources.rs:216-238:
```
        // A token this service did not mint for an inventory page names no page
        // boundary: this page is keyed by a resource URI, so a token whose
        // decoded key is not one is refused as a bad request rather than used as
        // an arbitrary lower bound ...
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_uri_page_token(token) {
```
and src/ops/list_discovered_resources.rs:260-264:
```
        // A token only when a resource is left AFTER this page, so the last page
        // carries none.
        let next_token = remaining
            .next()
            .map(|(uri, _)| crate::state::encode_key_token(uri));
```
while the page it is a boundary of is selected by three request-derived values — src/ops/list_discovered_resources.rs:240-250:
```
        // What the services that own this resource type hold for this account in
        // this Region, keyed by resource URI ...
        let inventory = crate::state::discovered_resources(
            &*self.engine,
            &region,
            account,
            &input.resource_type,
        );
        let lower = start.unwrap_or_default();
        let mut remaining = inventory.range(lower..);
```

The decoder it uses weighs only the ARN shape, and its own doc is already stale about who calls it — src/state.rs:3132-3148:
```
/// Decode a URI-keyed `PaginationToken` produced by [`encode_key_token`], for
/// the paginated operations whose store key is a resource's URI — a resource
/// set's membership and the organisation's discoverable inventory
/// ([`discovered_resources`]), which is keyed the same way. `None` when the
/// token was not produced by this service for such a page.
...
pub fn decode_uri_page_token(token: &str) -> Option<String> {
    let decoded = decode_key_token(token)?;
    parse_arn(&decoded).is_some().then_some(decoded)
}
```
(the other named user, the resource-set membership listing, has since been scoped and no longer calls this.)

The sibling that DOES scope the identical URI-keyed page — src/ops/list_resource_set_resources.rs:224-227:
```
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_scoped_page_token(&set_id, token)
                .filter(|uri| crate::state::parse_arn(uri).is_some())
```
and src/ops/list_resource_set_resources.rs:266-268:
```
        let next_token = remaining
            .next()
            .map(|uri| crate::state::encode_scoped_page_token(&set_id, uri));
```

The modelled member the token stands for — `smithy_query(query="doc", shape="com.amazonaws.fms#ListDiscoveredResourcesRequest", member="NextToken")`: "When you request a list of objects with a `MaxResults` setting, if the number of objects that are still available for retrieval exceeds the maximum you requested, Firewall Manager returns a `NextToken` value in the response. To retrieve the next batch of objects, use the token returned from the prior request in your next request." — the next batch of the SAME request's objects.

Test coverage today: tests/all/narrowed_page_token_scoping.rs:3-21 exists for exactly this rule — "such a listing's `NextToken` names a page boundary only under the selection the request that minted it named, so a token minted under one selection is refused under another rather than resumed from as an arbitrary key" — and enumerates its table as "`ListComplianceStatus` ... `ListResourceSetResources` ... `ListAppsLists` and `ListProtocolsLists`". `ListDiscoveredResources` is not a row, and tests/all/discovered_resources.rs / tests/all/associable_discovered_resources.rs only walk one selection's own pages and refuse a non-ARN token.

## Steps to Reproduce

1. Boot the emulator and call `AssociateAdminAccount` with the emulator's own account id so the administrator gate is open.
2. Through the EC2 emulator, create (in us-east-1, owned by that account) one VPC and one subnet, so `ListDiscoveredResources` has something to report for two resource types.
3. Call `ListDiscoveredResources` with `{"MemberAccountIds": ["<account>"], "ResourceType": "AWS::EC2::VPC", "MaxResults": 1}` repeatedly until the response carries a `NextToken` (seed more than one VPC if needed). Keep that token.
4. Call `ListDiscoveredResources` with `{"MemberAccountIds": ["<account>"], "ResourceType": "AWS::EC2::Subnet", "MaxResults": 100, "NextToken": "<the VPC token>"}`.
5. Expected (the rule state.rs:3174-3186 states and list_resource_set_resources.rs:224-227 applies): the token names no boundary of the subnet page, so it is refused with `InvalidInputException`.
6. Actual: 200, `Items: []` and no `NextToken` — the subnet whose URI sorts before `.../vpc/...` is silently dropped and the empty page is indistinguishable from "this account owns no subnets". The same token is likewise accepted for another `MemberAccountIds` entry and for a request signed for another Region.
