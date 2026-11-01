Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# GetDiskSnapshots reports the snapshots of EVERY Region and says so in a comment, against its own modelled sentence "all block storage disk snapshots in your AWS account and region"

_Filed: 2026-10-29T12:15:02Z_
_Operation: GetDiskSnapshots | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetDiskSnapshots` walks the whole `store.disk_snapshots` map and reports every snapshot the account holds, whatever Region the snapshot belongs to and whatever Region the request was signed for. Its own modelled documentation — reproduced verbatim in the handler file's own "Modeled contract" header — scopes the listing to a Region: "Returns information about all block storage disk snapshots in your AWS account and region."

The handler's body states the opposite rule in so many words, three lines of the same file below that header: "A snapshot the singular `GetDiskSnapshot` answers for is one this listing reports too, whatever zone its source was in." So the file contradicts itself, and the stored record makes the divergence observable: a disk snapshot keeps the `location` of its source (`CreateDiskSnapshot` copies `disk.location` / `instance.location` onto it), and `CopySnapshot` exists precisely because a snapshot "belongs to the Region it was taken in" — a rule `CreateRelationalDatabaseFromSnapshot` enforces for its own snapshots ("a snapshot belongs to the Region it was taken in, and only that Region's Lightsail endpoint can restore it"). A caller signed for us-east-1 is nevertheless handed snapshots whose `location.regionName` is eu-west-1.

Everywhere else in this crate a listing whose model sentence names the Region is scoped to it (`GetBuckets` → `regional_records_from`), and a resource deliberately held account-wide is justified with a model quote (distributions, certificates) or flagged as a known deviation (key pairs). The `disk_snapshots` map carries neither.

## Evidence

The model — smithy_query(query="doc", shape="com.amazonaws.lightsail#GetDiskSnapshots"):
> "Returns information about all block storage disk snapshots in your AWS account and region."

the same sentence in the handler's own header — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_disk_snapshots.rs:47-48:
```
//! Returns information about all block storage disk snapshots in your AWS account and
//! region.
```

the body of the SAME file stating the opposite rule — src/ops/get_disk_snapshots.rs:165-184:
```
        // This operation reports what the account holds; it takes no location, so
        // nothing here reads the request's credential scope. A snapshot the
        // singular `GetDiskSnapshot` answers for is one this listing reports too,
        // whatever zone its source was in.
        let _ = ctx;
...
            let (disk_snapshots, next_page_token) = crate::state::resource_page(
                crate::state::records_from(&store.disk_snapshots, &page_token),
                crate::state::DISK_SNAPSHOT_PAGE_SIZE,
```

The snapshot really does carry one Region — src/ops/create_disk_snapshot.rs:252-262 and :296-302:
```
            // The snapshotted volume's size and location come from the SOURCE
            // record as it stands now, so the snapshot keeps them even if the
            // source later changes or is deleted.
            let (size_in_gb, location, stored_source) = match &source {
                RequestedSource::Disk(disk_name) => {
...
                        disk.location.clone(),
...
                location: location.clone(),
```
and the crate's own statement that a snapshot's Region is binding — src/ops/create_relational_database_from_snapshot.rs:328-337:
```
            // A snapshot belongs to the Region it was taken in, and only that
            // Region's Lightsail endpoint can restore it: moving one between
            // Regions is what `CopySnapshot` is for.
            if snapshot.location.region_name != region {
```

The sibling listing that honours the identical kind of sentence — src/ops/get_buckets.rs:223-227 (`regional_records_from(&store.buckets, &region, &page_token)`), against GetBucketsRequest$bucketName: "the response includes all of your buckets in the Amazon Web Services Region where the request is made".

Test coverage, and why it does not settle the question: tests/all/disk_snapshot_listing.rs:144-158 gives the rows "the region the LIST call is signed for (this operation reports what the account holds, so it changes nothing)", and tests/all/disk_snapshot_listing.rs:355-377 asserts it for five Regions:
```
    // Every created snapshot, once each, in the store's own order by name — and
    // the same answer whatever scope the walk is signed for, because the
    // operation reports what the account holds.
...
    for region in LIST_REGIONS {
        let body = emulator.ok_at(None, *region, "GetDiskSnapshots", json!({})).await;
        assert_eq!(body["diskSnapshots"], Value::Array(first.clone()), "case listing from {region:?}: expected the same records the account holds, got {body:?}");
```
The stated reason — "the operation reports what the account holds" — is precisely the claim the modelled sentence contradicts ("in your AWS account and region"), so the pinned expectation is the defect, not a justification for it.

## Steps to Reproduce

1. Call CreateDisk signed for us-east-1 with diskName="virginia-disk", availabilityZone="us-east-1a", sizeInGb=8, and CreateDiskSnapshot with diskSnapshotName="virginia-snap", diskName="virginia-disk".
2. Call CreateDisk signed for eu-west-1 with diskName="ireland-disk", availabilityZone="eu-west-1a", sizeInGb=8, and CreateDiskSnapshot with diskSnapshotName="ireland-snap", diskName="ireland-disk".
3. Call GetDiskSnapshots signed for us-east-1 with no pageToken.
4. Observe both entries: "ireland-snap" is reported with location.regionName "eu-west-1", although the operation documents itself as "Returns information about all block storage disk snapshots in your AWS account and region" (src/ops/get_disk_snapshots.rs:47-48).
5. Read src/ops/get_disk_snapshots.rs:47-48 next to :165-168: the file's own modelled-contract header says "account and region" while its handler comment says the listing reports a snapshot "whatever zone its source was in".
6. Compare with src/ops/get_buckets.rs:223-227, where the crate scopes a listing to the request's Region on the strength of the same kind of sentence, and with src/ops/create_relational_database_from_snapshot.rs:328-337, where it refuses to cross a snapshot's Region at all.
