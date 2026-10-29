Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# GetDisks reports the disks of EVERY Region, although its own modelled sentence scopes the listing to "your AWS account and region" — the scoping this crate applies to every other listing whose model says so

_Filed: 2026-10-29T12:15:02Z_
_Operation: GetDisks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetDisks` walks the whole `store.disks` map and reports every disk the account holds, whatever Region the disk is in and whatever Region the request was signed for. The operation's own modelled documentation — reproduced verbatim in the handler file's own "Modeled contract" header — says the listing is scoped to a Region: "Returns information about all block storage disks in your AWS account and region."

A disk IS a located record in this crate: `CreateDisk` derives the disk's Region from the `availabilityZone` the request names (`created_in_availability_zone(..)` then `StoredLocation::zonal(..)`), and the stored `location.regionName` is reported on every entry. So the divergence is observable: a caller signed for `us-east-1` is handed disks whose own `location.regionName` is `eu-west-1`, and `nextPageToken` walks them too.

The crate applies exactly the opposite rule wherever a model sentence says a listing is a Region's: buckets are keyed `(region, name)` and `GetBuckets` walks `regional_records_from(&store.buckets, &region, ..)` on the strength of `GetBucketsRequest$bucketName`'s "the response includes all of your buckets in the Amazon Web Services Region where the request is made"; load balancers are keyed by Region because "the modelled `CreateLoadBalancer` documentation caps them 'per AWS Region in your account'". And where the crate deliberately keys a resource by name ALONE it says so with a model quote (distributions: "NOT keyed by region ... a distribution is a global resource") or flags the divergence outright (key pairs: "it is a DEVIATION worth knowing about"). The `disks` map carries neither a model quote nor a flagged deviation — only "Every block storage disk, keyed by its disk name" — and the handler positively asserts the account-wide rule instead.

The same defect stands in the sibling `GetDiskSnapshots`, whose model sentence carries the identical "and region" clause (filed separately); those two are the only listings in this model whose documentation states the Region scope, so the fix is confined to them.

## Evidence

The model, quoted from the operation itself — smithy_query(query="doc", shape="com.amazonaws.lightsail#GetDisks"):
> "Returns information about all block storage disks in your AWS account and region."

the same sentence, in the handler file's own header — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_disks.rs:47:
```
//! Returns information about all block storage disks in your AWS account and region.
```

what the body does instead — src/ops/get_disks.rs:163-183:
```
        // This operation reports what the account holds and takes nothing but a
        // page token, so nothing here reads the request metadata.
        let _ = ctx;
...
            let (disks, next_page_token) = crate::state::resource_page(
                crate::state::records_from(&store.disks, &page_token),
                crate::state::DISK_PAGE_SIZE,
```
(`records_from` walks the whole name-keyed map; `regional_records_from` — the Region-scoped walker this crate already has — is never called here.)

The disk's Region is real and comes from the request's own `availabilityZone`, not from the listing's scope — src/ops/create_disk.rs:188-193 and :226:
```
        let region = crate::state::created_in_availability_zone(
            "availabilityZone",
            &input.availability_zone,
        )
        .map_err(invalid)?
        .name;
...
        let location = crate::state::StoredLocation::zonal(&input.availability_zone, region);
```

The sibling listing that DOES honour the identical kind of sentence — src/ops/get_buckets.rs:223-236:
```
            let (buckets, next_page_token) = crate::state::resource_page(
                // The store keys buckets by `(region, name)`, so the walk takes
                // exactly this region's buckets in name order and leaves the
                // region behind at the first key of the next one.
                crate::state::regional_records_from(&store.buckets, &region, &page_token),
```
against smithy_query(query="doc", shape="GetBucketsRequest", member="bucketName"): "When omitted, the response includes all of your buckets in the Amazon Web Services Region where the request is made."

The crate's own rule for when a map is Region-keyed, with its model quote — src/state.rs:13128-13135:
```
    /// Every load balancer, keyed by the region it was created in and its name.
    ///
    /// A load balancer is a REGIONAL resource — the modelled
    /// `CreateLoadBalancer` documentation caps them "per AWS Region in your
    /// account" — so the region is part of its identity, exactly as it is for a
    /// container service and a bucket.
```
and the two places it deliberately does NOT, each justified — src/state.rs:13171-13180 ("NOT keyed by region, unlike a bucket or a load balancer: a distribution is a global resource and \"all distributions are located in the `us-east-1` Region\"") and src/state.rs:13100-13108 ("it is a DEVIATION worth knowing about — against AWS a key pair belongs to one Region").

The disks map, which carries neither — src/state.rs:13024-13027:
```
    /// Every block storage disk, keyed by its disk name — the identifier every
    /// disk operation takes. A `BTreeMap` so `GetDisks` walks a stable, total
    /// order and a page token can name the next disk.
    pub disks: BTreeMap<String, StoredDisk>,
```

Test coverage, and why it does not settle the question: tests/all/disk_listing.rs:12-26 states the rule it pins as "a listing reports every record the store holds of that resource type", and tests/all/disk_listing.rs:257-267 deliberately creates each row's disk in a different published zone ("each in its own zone out of the published catalog", `zones[index % zones.len()]`, the zones coming from `GetRegions` across ALL Regions) and then expects one listing to report them all. That reason — "every record the store holds" — is exactly the sentence the model contradicts, so the pinned expectation is the defect rather than a justification for it; no comment or test in the crate states a Region-independence rule for disks the way the key-pair and distribution comments state theirs.

## Steps to Reproduce

1. Call CreateDisk signed for us-east-1 with diskName="virginia-disk", availabilityZone="us-east-1a", sizeInGb=8.
2. Call CreateDisk signed for eu-west-1 with diskName="ireland-disk", availabilityZone="eu-west-1a", sizeInGb=8.
3. Call GetDisks signed for us-east-1 with no pageToken.
4. Observe both entries in `disks`: "ireland-disk" is reported with location.regionName "eu-west-1" to a caller whose request was made in us-east-1, although the operation documents itself as "Returns information about all block storage disks in your AWS account and region" (src/ops/get_disks.rs:47).
5. Compare with the same scenario for buckets: CreateBucket in us-east-1 and in eu-west-1, then GetBuckets signed for us-east-1 reports only the us-east-1 bucket (src/ops/get_buckets.rs:223-236) — the crate honours the identical model sentence there and ignores it here.
