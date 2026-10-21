Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-assets-and-connection
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-assets-and-connection outposts-outpost-lifecycle outposts-quote-and-order-lifecycle outposts-renewal-and-billing outposts-site-lifecycle outposts-capacity-task-lifecycle
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|CreateSite|2xx->2xx|$.Site.Tags
Issue-Key: outposts|CreateSite|2xx->2xx|$.SiteId
Issue-Key: outposts|CreateOutpost|2xx->2xx|$.Outpost.AvailabilityZoneId
Issue-Key: outposts|CreateOutpost|2xx->2xx|$.Outpost.LifeCycleStatus
Issue-Key: outposts|CreateOutpost|2xx->2xx|$.Outpost.Tags
Issue-Key: outposts|GetOutpost|2xx->2xx|$.Outpost.AvailabilityZoneId
Issue-Key: outposts|GetOutpost|2xx->2xx|$.Outpost.LifeCycleStatus
Issue-Key: outposts|GetOutpost|2xx->2xx|$.Outpost.Tags
Issue-Key: outposts|ListOutposts|2xx->2xx|$.Outposts[Name="sim2real-probe-outposts-olc-outpost-fb09a8a0"].AvailabilityZoneId
Issue-Key: outposts|ListOutposts|2xx->2xx|$.Outposts[Name="sim2real-probe-outposts-olc-outpost-fb09a8a0"].LifeCycleStatus
Issue-Key: outposts|ListOutposts|2xx->2xx|$.Outposts[Name="sim2real-probe-outposts-olc-outpost-fb09a8a0"].Tags
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.Tags
Issue-Key: outposts|ListSites|2xx->2xx|$.Sites[Name="sim2real-probe-outposts-site-1ff0ec35"].Tags
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# Outposts Site and Outpost response projection omits Tags, SiteId and AvailabilityZoneId

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSite

## Description
The emulator's Site and Outpost serializers drop members that AWS always returns, and it reports a freshly created Outpost as already ACTIVE. Seven causes, all in the outposts Site/Outpost response projection and the CreateOutpost lifecycle model.

Cause 1 of 7: CreateSite omits Site.SiteId and Site.Tags.
AWS: Smithy: CreateSite -> CreateSiteOutput.Site : Site; Site.SiteId : SiteId (pattern ^(arn:...:site/)?(os-[a-f0-9]{17})$) and Site.Tags : TagMap ("The site tags."). The recording shows a 200 whose $.Site.SiteId is the newly minted site id (the same id the fixture then reuses as a request label) and whose $.Site.Tags is present as an empty map {} when no tags were supplied.
Emulator: 200, but $.SiteId is None and $.Site.Tags is missing (absent key, not {}).
Why: a client that reads SiteId out of CreateSite's response has nothing to address the new site with, so every follow-on GetSite/CreateOutpost/DeleteSite call fails or has to be fed an id obtained some other way; a client that iterates response Tags gets a null-dereference instead of an empty map.
Fix: the CreateSite handler's output projection / Site serializer in the outposts service crate.
Traces: outposts-assets-and-connection#setup#1 (also outposts-outpost-lifecycle#task#1, outposts-quote-and-order-lifecycle#setup#1, outposts-renewal-and-billing#setup#1, outposts-site-lifecycle#setup#1).

Cause 2 of 7: CreateOutpost omits Outpost.AvailabilityZoneId and Outpost.Tags and returns the wrong LifeCycleStatus.
AWS: Smithy: CreateOutpost -> CreateOutpostOutput.Outpost : Outpost; Outpost.AvailabilityZoneId : AvailabilityZoneId (pattern ^[a-zA-Z]+\d-[a-zA-Z]+\d$), Outpost.Tags : TagMap ("The Outpost tags."), Outpost.LifeCycleStatus : LifeCycleStatus. The recording shows AWS resolving AvailabilityZoneId from the requested AvailabilityZone and returning it on create, returning Tags as {} when none were supplied, and returning LifeCycleStatus 'PENDING' for a just-created Outpost.
Emulator: 200, but $.Outpost.AvailabilityZoneId is None, $.Outpost.Tags is missing, and $.Outpost.LifeCycleStatus is 'ACTIVE' immediately.
Why: a client that waits for an Outpost to leave PENDING never observes the pending state and cannot exercise/verify provisioning logic, and one that keys capacity placement off AvailabilityZoneId gets nothing.
Fix: the CreateOutpost handler — AZ-to-AZ-id resolution, initial lifecycle state in the stored Outpost, and the Outpost serializer.
Traces: outposts-assets-and-connection#setup#2 (also outposts-capacity-task-lifecycle#setup#2, outposts-outpost-lifecycle#task#2, outposts-quote-and-order-lifecycle#setup#2, outposts-renewal-and-billing#setup#2).

Cause 3 of 7: CreateSite omits Site.Tags on the capacity-task fixture's site.
AWS: as Cause 1 — Site.Tags is present and empty when no tags were supplied.
Emulator: 200 with $.Site.Tags missing.
Why: same null-vs-empty-map break for tag-reading clients.
Fix: Site serializer (shared with Cause 1).
Traces: outposts-capacity-task-lifecycle#setup#1.

Cause 4 of 7: GetOutpost omits AvailabilityZoneId and Tags and reports ACTIVE for a PENDING Outpost.
AWS: Smithy: GetOutpost (GET /outposts/{OutpostId}) -> GetOutpostOutput.Outpost : Outpost, same members as Cause 2. The recording shows AvailabilityZoneId present, Tags {} and LifeCycleStatus 'PENDING' on a read-back of a just-created Outpost.
Emulator: 200 with AvailabilityZoneId None, Tags missing, LifeCycleStatus 'ACTIVE'.
Why: read-back does not agree with AWS, so poll-until-ready and AZ-placement clients behave differently against the emulator than against AWS.
Fix: the GetOutpost handler's Outpost projection plus the stored lifecycle state.
Traces: outposts-outpost-lifecycle#task#3.

Cause 5 of 7: ListOutposts omits the same Outpost members and reports ACTIVE.
AWS: Smithy: ListOutposts -> Outposts : outpostListDefinition of Outpost; list entries carry the same members as the single-resource reads.
Emulator: 200, but the matched list entry has AvailabilityZoneId None, Tags missing and LifeCycleStatus 'ACTIVE' instead of 'PENDING'.
Why: list and get disagree with AWS identically, and LifeCycleStatusFilter / AvailabilityZoneIdFilter (modeled query params on this op) cannot work correctly on records whose status and AZ id are wrong or absent.
Fix: the ListOutposts handler's per-item Outpost projection (shared serializer with Cause 4).
Traces: outposts-outpost-lifecycle#task#4.

Cause 6 of 7: GetSite omits Site.Tags.
AWS: Smithy: GetSite (GET /sites/{SiteId}) -> Site.Tags : TagMap; recording shows {} for an untagged site.
Emulator: 200 with $.Site.Tags missing.
Fix: GetSite handler's Site projection.
Why: tag-reading clients see absent instead of empty.
Traces: outposts-site-lifecycle#task#2.

Cause 7 of 7: ListSites omits Tags on list entries.
AWS: Smithy: ListSites -> Sites : siteListDefinition of Site, entries carry Tags.
Emulator: 200 with the matched entry's Tags missing.
Why: same as Cause 6, on the list path.
Fix: ListSites handler's per-item Site projection.
Traces: outposts-site-lifecycle#task#3.

## Evidence
Cause 1 — CreateSite, expected 200 / actual 200: $.Site.Tags missing (expected {}, got None); $.SiteId missing (expected the created site id, got None).
Cause 2 — CreateOutpost, 200/200: $.Outpost.AvailabilityZoneId missing (expected an AZ id such as 'usw2-az2', got None); $.Outpost.LifeCycleStatus expected 'PENDING', got 'ACTIVE'; $.Outpost.Tags missing (expected {}, got None).
Cause 3 — CreateSite, 200/200: $.Site.Tags missing (expected {}, got None).
Cause 4 — GetOutpost, 200/200: $.Outpost.AvailabilityZoneId missing (got None); LifeCycleStatus expected 'PENDING', got 'ACTIVE'; Tags missing (expected {}, got None).
Cause 5 — ListOutposts, 200/200: matched entry AvailabilityZoneId missing, LifeCycleStatus expected 'PENDING' got 'ACTIVE', Tags missing (expected {}, got None).
Cause 6 — GetSite, 200/200: $.Site.Tags missing (expected {}, got None).
Cause 7 — ListSites, 200/200: matched entry .Tags missing (expected {}, got None).
Generated ids, names and AZ strings in the recordings are examples; the requirement is presence of the member and the PENDING-on-create lifecycle value.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay outposts-assets-and-connection step setup#1 (CreateSite) — inspect $.SiteId and $.Site.Tags in the 200 body.
3. Cause 2: replay outposts-assets-and-connection step setup#2 (CreateOutpost) — inspect $.Outpost.AvailabilityZoneId, $.Outpost.Tags, $.Outpost.LifeCycleStatus.
4. Cause 3: replay outposts-capacity-task-lifecycle step setup#1 (CreateSite).
5. Cause 4: replay outposts-outpost-lifecycle step task#3 (GetOutpost).
6. Cause 5: replay outposts-outpost-lifecycle step task#4 (ListOutposts).
7. Cause 6: replay outposts-site-lifecycle step task#2 (GetSite).
8. Cause 7: replay outposts-site-lifecycle step task#3 (ListSites).
