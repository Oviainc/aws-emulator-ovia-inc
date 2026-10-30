Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 15a7913299c0ebfe5f22c92412d933bd37150a6d

# GetCostEstimate estimates a distribution or DNS zone from any Region, skipping the us-east-1 restriction every other door onto those records applies

_Filed: 2026-10-29T16:16:42Z_
_Operation: GetCostEstimate | Class: validation_

> _Note: this issue was identified against the code base at commit `15a7913299c0ebfe5f22c92412d933bd37150a6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetCostEstimate` resolves the name it is given against every record map of the store in turn — including the distributions and the DNS zones, which are exactly the kinds of record this crate holds under a name alone *because* their API is served from the N. Virginia endpoint alone. It therefore answers a 200 cost estimate for a distribution (or a domain) when the request's SigV4 credential scope names eu-west-1, while every other door onto the same stored record — `GetDistributions`, `GetDistributionLatestCacheReset`, `GetDomain`, `CreateDistribution`, `DeleteDistribution`, and the two tag doors — refuses the same request with the modelled 400 `InvalidInputException`.

The crate already owns this rule twice over: `crate::state::domain_region_refusal` for a door that knows up front which kind of record it touches, and `crate::state::domain_region_refusal_for_resource_type` for "a door that admits every kind of record and so learns WHICH kind a name addresses only once it has resolved it". `GetCostEstimate` is precisely the second kind of door (its `billed_resources` walk tries instance, bucket, container service, distribution, load balancer, static IP, domain, alarm, export record in turn), and it calls neither helper. `GetCostEstimate` declares `InvalidInputException`, so the refusal has a modelled channel.

The handler's own doc section discusses only WHICH map a name is resolved in (name-keyed vs `(region, name)`-keyed); it says nothing about the documented endpoint restriction, so nothing in the file justifies skipping it.

## Evidence

The helper this door should ask, and what the crate says it is for — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:16720-16738:
```
/// The refusal for a call onto a record of `resource_type` this request cannot
/// be served by, and `None` when the kind is not one the restriction covers or
/// the request is made in the Region those APIs are served from.
///
/// For a door that admits every kind of record and so learns WHICH kind a name
/// addresses only once it has resolved it — a tag change, which reaches every
/// record whose modelled shape carries a `tags` member ...
pub fn domain_region_refusal_for_resource_type(
```
and state.rs:16714-16718 lists the kinds it covers: `DOMAIN_REGION_ONLY_RESOURCE_TYPES: [&str; 3] = [RESOURCE_TYPE_DOMAIN, RESOURCE_TYPE_DISTRIBUTION, RESOURCE_TYPE_CERTIFICATE]`.

The sibling door that DOES ask it, and states the rule covers reads — src/ops/tag_resource.rs:224-232:
```
// A record of a kind the documented restriction covers is
// edited only through the Region its own API is served in, so
// the tag write is refused here exactly as that record's own
// create, read and delete refuse — before a single tag moves.
if let Some(message) =
    crate::state::domain_region_refusal_for_resource_type(parts, resource_type)
```
(the same call in src/ops/untag_resource.rs:208; `crate::state::domain_region_refusal(ctx.parts())` in get_distributions.rs:167, get_domain.rs:161, get_distribution_latest_cache_reset.rs:158, create_distribution.rs:203, delete_distribution.rs:157, …).

The side that skips it — src/ops/get_cost_estimate.rs:191-213 reads only `let region = crate::state::request_region(ctx.parts());` and then calls `billed_resources(store, &region, &input.resource_name)`; grep for `domain_region_refusal` across `src/ops` returns 24 handlers and `get_cost_estimate.rs` is not among them. The distribution and domain arms of that walk, src/ops/get_cost_estimate.rs:609-628 and :666-673:
```
if let Some(distribution) = store.distribution(name) { … resource_type: crate::state::RESOURCE_TYPE_DISTRIBUTION, … }
…
if let Some(domain) = store.domain(name) { … resource_type: crate::state::RESOURCE_TYPE_DOMAIN, … }
```

The handler's own doc says only this about Regions (get_cost_estimate.rs:28-37): "The remaining records (an instance, a disk, a snapshot, a managed database, a distribution, a static IP, a DNS zone, an alarm, an export record) are keyed by name ALONE … exactly as the operations that report those resources resolve them" — but those operations ALSO refuse the request outright outside us-east-1, which this one does not.

Precedent in this crate: the fixed issue 20261028T212440Z-lightsail-tagresource-validation ("TagResource edits a domain, distribution or certificate from any Region, skipping the us-east-1 restriction every other door onto those records applies") is the same gap on the tag door; test coverage of the rule lives in tests/all/domain_region_restriction.rs and tests/all/tag_region_restriction.rs, neither of which exercises GetCostEstimate.

## Steps to Reproduce

1. Sign a request with a SigV4 credential scope naming us-east-1 and call CreateDistribution to create a distribution (say `cdn-1`) over a bucket or instance origin.
2. Sign a request with a credential scope naming eu-west-1 and call GetDistributions: the call is refused 400 InvalidInputException ("Domain and distribution APIs are only available in the N. Virginia (us-east-1) …"), via src/ops/get_distributions.rs:167.
3. Sign a request with the same eu-west-1 scope and call GetCostEstimate with resourceName=`cdn-1` and a forward startTime/endTime window.
4. Observe HTTP 200 with resourcesBudgetEstimate naming resourceType `Distribution` and the distribution's bundle charge — the same record the previous step refused to show from that Region.
5. Repeat with a DNS zone created by CreateDomain: GetDomain from eu-west-1 is refused, GetCostEstimate from eu-west-1 reports it with resourceType `Domain`.
