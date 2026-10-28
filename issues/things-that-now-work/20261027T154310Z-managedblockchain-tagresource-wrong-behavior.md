Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2334cad17af0cf812822af0fde730e954dc43dac

# TagResource writes tags into AWS's standing public-network record, so a Region-scoped write becomes visible to every Region's GetNetwork

_Filed: 2026-10-27T15:43:10Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2334cad17af0cf812822af0fde730e954dc43dac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tagging path resolves a `ResourceArn` to the stored record carrying that ARN and writes into it. The standing, AWS-owned public networks are stored records like any other, so an ARN of one resolves and the write lands in the shared record — the one record the crate says every Region reads and no request creates.

Two consequences the crate's own rules contradict:

1. The write is made under a Region gate (`arn_in_region`), yet it lands in a record that is NOT Region-scoped: after `TagResource` on `arn:aws:managedblockchain:us-east-1:<account>:networks/n-ethereum-mainnet`, `GetNetwork n-ethereum-mainnet` reports those tags from EVERY Region (a public network resolves in every Region and `to_network` reports its tag set), while `ListTagsForResource` for the same ARN from any other Region answers 404 `ResourceNotFoundException`. The same tag set is therefore readable in eu-west-1 through one operation and "not found" there through the operation whose whole job is reporting it.
2. The record mutated is one the crate deliberately keeps out of every other write path (`DeleteMember` refuses a non-Fabric network precisely so the public networks stay "out of every write below"), and whose module doc states it is not account state.

`UntagResource` reaches the same record through the same `tags_for_resource_mut` lookup, so a caller can also remove tags from it.

## Evidence

The crate's stated rule — src/state.rs:13-21 (module doc):

```
//! Managed Blockchain is a REGIONAL service, so every record belongs to the Region the
//! call that created it was signed for, and only a request addressed to that Region
//! reaches it. ...
//! resources. The only records every Region reads are AWS's standing public networks,
//! which no request creates.
```

and src/state.rs:779-790 (`StoredNetwork::in_region`): "AWS's standing public networks ([`PUBLIC_NETWORKS`]) are a different thing: no request created them and they belong to no account, so every Region's endpoint reads the same ones ... What an account creates ON one of them — a member, a node, a proposal — is an account resource again, and is reached only through the Region it was created in." `pub fn in_region(&self, region: &str) -> bool { self.aws_owned || arn_in_region(&self.arn, region) }`

The sibling write path that excludes them — src/ops/delete_member.rs:187-192: "It is also what keeps AWS's standing, AWS-owned public networks (all of them Ethereum) out of every write below, the network deletion included." — and src/state.rs:2332-2334: "One of AWS's standing public networks is not an account resource and no call deletes it".

The write that does reach them — src/ops/tag_resource.rs:210-223 and :250-252:

```
// An ARN of another Region names no resource this Region holds, which is the same
// answer as a resource that was never created ... stated here so it holds for the
// standing public networks too (they are read in every Region, so their stored ARN
// alone does not settle which Region a request addresses).
let Some(tags) = crate::state::arn_in_region(&resource_arn, &region)
    .then(|| guard.tags_for_resource_mut(&resource_arn))
    .flatten()
...
for (key, value) in input.tags {
    tags.insert(key, value);
}
```

The resolution that hands back the AWS-owned record (its stored ARN is minted for `DEFAULT_REGION`, src/state.rs:1770 `arn: network_arn(DEFAULT_REGION, public.id)`) — src/state.rs:1589-1592: `for network in $state.networks.$records() { if network.arn == arn { return Some($record::Network(network)); }`, and the read that then reports the tags in every Region — src/state.rs:898-911 (`to_network`): `tags: Some(map_out(&self.tags))`.

Test coverage: tests/all/public_networks_and_membership.rs:279-352 pins that a public network "must read back identically" across reads and publishes its `arn:aws:managedblockchain:{DEFAULT_REGION}:{ACCOUNT}:networks/{id}` ARN to callers; tests/all/region_isolation.rs:788-791 pins the carve-out ("no request created them and they [are read in every Region]"). No test tags a public network — grep for `ethereum-mainnet` in tests/all/resource_tags.rs, reserved_tag_keys.rs and tagged_record_resolution.rs returns nothing — so the current behaviour is unpinned.

## Steps to Reproduce

1. `GET /networks/n-ethereum-mainnet` signed for us-east-1 -> 200; copy `Network.Arn` (= `arn:aws:managedblockchain:us-east-1:<account>:networks/n-ethereum-mainnet`) and note `Network.Tags` is empty.
2. `POST /tags/<that ARN>` with `{"Tags":{"owner":"me"}}` signed for us-east-1 -> 200.
3. `GET /networks/n-ethereum-mainnet` signed for eu-west-1 -> 200 reporting `Tags: {"owner":"me"}` — a write made in one Region is reported by every Region's read of the shared AWS-owned record (src/state.rs:13-21 says those are the records every Region reads, and that no request creates them).
4. `GET /tags/<that same ARN>` signed for eu-west-1 -> 404 `ResourceNotFoundException`: the tag set GetNetwork just reported in that Region is "not found" there.
5. `DELETE /tags/<that ARN>?tagKeys=owner` signed for us-east-1 -> 200, and the tag disappears from every Region's GetNetwork — the same shared record is writable through UntagResource too.
