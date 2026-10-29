Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-catalog-and-instance-types-browsing
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-catalog-and-instance-types-browsing outposts-catalog-list-catalog-items outposts-catalog-list-orderable-instance-types outposts-outpost-lifecycle
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|ListCatalogItems|2xx->2xx|$.CatalogItems[*]
Issue-Key: outposts|GetCatalogItem|2xx->2xx|$.CatalogItem.CurrentType
Issue-Key: outposts|GetCatalogItem|2xx->2xx|$.CatalogItem.Dimensions
Issue-Key: outposts|GetCatalogItem|2xx->2xx|$.CatalogItem.ItemClass
Issue-Key: outposts|GetCatalogItem|2xx->2xx|$.CatalogItem.RackUnitHeight
Issue-Key: outposts|GetCatalogItem|2xx->2xx|$.CatalogItem.SupportedUplinkGiB
Issue-Key: outposts|ListOrderableInstanceTypes|2xx->2xx|$.InstanceTypes[*]
Issue-Key: outposts|GetOutpostSupportedInstanceTypes|2xx->2xx|$.InstanceTypes[*]
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# outposts catalog and instance-type read surfaces serve wrong static data and drop modeled-out members

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCatalogItems

## Description
5 net-new root causes in the read-only catalog surfaces. The emulator ships its own invented catalog and instance-type datasets, and the CatalogItem it serves omits five members AWS returns. IMPORTANT for the fixer: the Smithy model checked in at services/service-com-amazonaws-outposts-outpostsolafservice/smithy-model/com-amazonaws-outposts-outpostsolafservice.json is OLDER than the live API — its CatalogItem structure has only CatalogItemId/ItemStatus/EC2Capacities/PowerKva/WeightLbs/SupportedUplinkGbps/SupportedStorage/RackScalingType, so the five missing members below are "recorded behavior only" and the model must be refreshed or the serializer extended for them to be emittable at all.

Cause 1 of 5: ListCatalogItems entries carry the wrong catalog identities and shapes.
AWS: 200, CatalogItems entries whose CatalogItemId is a real published SKU and whose EC2Capacities MaxSize is the bare size suffix (e.g. MaxSize '24xlarge', not 'c5.24xlarge'), with per-item PowerKva/WeightLbs taken from the published rack specification, and each item carrying CurrentType, Dimensions, ItemClass, RackUnitHeight and SupportedUplinkGiB (recorded behavior only — the checked-in model lacks these five members).
Emulator: 200 with an invented catalog: different CatalogItemIds, EC2Capacities whose MaxSize repeats the family prefix ('c5.24xlarge'), invented Quantity/PowerKva/WeightLbs, and no CurrentType/Dimensions/ItemClass/RackUnitHeight/SupportedUplinkGiB at all.
Why: a client cannot order from the emulator using any SKU AWS publishes, and code that reads ItemClass (rack vs server), Dimensions or RackUnitHeight to pick hardware sees nothing; MaxSize parsing that assumes AWS's bare-suffix form breaks on the prefixed form.
Fix: the catalog dataset and CatalogItem serializer behind src/ops/list_catalog_items.rs (the shared catalog table in the crate's state/catalog data) — publish AWS's SKUs and emit all CatalogItem members.
Traces: outposts-catalog-and-instance-types-browsing#task#1.

Cause 2 of 5: GetCatalogItem omits five CatalogItem members.
AWS: 200, CatalogItem including CurrentType (e.g. 'AC'), Dimensions (e.g. 'OUTPOST_RACK'), ItemClass (e.g. 'RACK'), RackUnitHeight (e.g. 'HEIGHT_42U') and SupportedUplinkGiB (list) in addition to the members the checked-in model declares (recorded behavior only for those five).
Emulator: 200 with those five members absent.
Why: the single-item read is the call a client makes to decide whether a SKU fits its site (power form, rack dimensions, unit height, uplink), and all of that is missing, so the decision cannot be made.
Fix: the CatalogItem shape/serializer plus the catalog dataset behind src/ops/get_catalog_item.rs.
Traces: outposts-catalog-and-instance-types-browsing#task#2.

Cause 3 of 5: the full ListCatalogItems listing diverges item-for-item.
AWS: 200 with the published catalog — every entry in the recorded listing (37 positions compared) differs from what the emulator serves, including Gen-2 rack SKUs whose EC2Capacities name m7i/r7i families, and DISCONTINUED items at the positions AWS has them.
Emulator: 200 with its own list at every index — wrong ids, wrong families, wrong ItemStatus distribution, wrong PowerKva/WeightLbs, and the five members of cause 1/2 absent throughout.
Why: catalog browsing and any ordering flow keyed on a published SKU (CreateOrder line items, quote requirements) cannot be reproduced against the emulator at all.
Fix: same catalog dataset as cause 1; this cause is the unfiltered listing (no ItemClassFilter/EC2FamilyFilter), so it must be re-verified separately from cause 1's filtered read.
Traces: outposts-catalog-list-catalog-items#task#1.

Cause 4 of 5: ListOrderableInstanceTypes serves a different instance-type catalog.
AWS: 200, DetailedInstanceTypeItem entries (Smithy: InstanceType, VCPUs, MemoryInMib, NetworkPerformance, FormFactorConfigs) covering both Outpost generations — the recording includes d-suffix Gen-1 types (c5d.24xlarge, m5d.24xlarge) and Gen-2 types (c7i.48xlarge, c8i.96xlarge, m7i.48xlarge) with OutpostGeneration GENERATION_2 and their real MemoryInMib/NetworkPerformance.
Emulator: 200 with a list that omits every d-suffix and Gen-2 type and substitutes other c5/c6id/g4dn entries, so 68 of 72 compared positions differ.
Why: a client choosing an orderable instance type for a Gen-2 Outpost finds none, and MemoryInMib/NetworkPerformance read back for the wrong type.
Fix: the orderable-instance-type dataset behind src/ops/list_orderable_instance_types.rs, including its FormFactorConfigs/OutpostGeneration mapping.
Traces: outposts-catalog-list-orderable-instance-types#task#1.

Cause 5 of 5: GetOutpostSupportedInstanceTypes answers the global instance-type catalog instead of what the Outpost supports.
AWS: 200 with InstanceTypes scoped to the capacity actually installed on that Outpost — for the Outpost in this fixture the recorded response carries no instance-type entries at all.
Emulator: 200 with 64 entries (the whole c5/c6id/g4dn/... table) marked extra against an expected-empty list — the per-Outpost scope is ignored.
Why: a client reads supported types to size instances on a specific Outpost; the emulator claims every type is supported on an Outpost with no such capacity, so launches that AWS rejects look valid.
Fix: src/ops/get_outpost_supported_instance_types.rs — derive the list from the Outpost's installed assets/capacity-task configuration rather than the static catalog. Related open issue 20261029T125602Z-outposts-getoutpostsupportedinstancetypes-parameter-ignored covers the DROPPED OrderId parameter, not this unscoped result set.
Traces: outposts-outpost-lifecycle#task#8.

## Evidence
Replay of simparity-wire-aws-v1 (all five clusters 200 vs 200, body-divergent):
- ListCatalogItems (outposts-catalog-and-instance-types-browsing#task#1): '$.CatalogItems[0]' expected {CatalogItemId 'OR-FWTFFXJ', CurrentType 'AC', Dimensions 'OUTPOST_RACK', EC2Capacities [{Family 'c5', MaxSize '24xlarge', Quantity '1'}], ItemClass 'RACK', ItemStatus 'AVAILABLE', PowerKva 6.39, RackScalingType 'MULTI_RACK', RackUnitHeight 'HEIGHT_42U', SupportedStorage ['EBS','S3'], SupportedUplinkGbps [10,40,100], SupportedUplinkGiB [10,40,100], WeightLbs 1328}; got {CatalogItemId 'OR-7Q7BLB6', ItemStatus 'AVAILABLE', EC2Capacities [{Family 'c5', MaxSize 'c5.24xlarge', Quantity '15'}, ...], PowerKva 15.0, WeightLbs 1800, SupportedUplinkGbps [10,40,100], SupportedStorage ['EBS','S3'], RackScalingType 'MULTI_RACK'} — five members absent, ids/sizes/figures different.
- GetCatalogItem (#task#2): '$.CatalogItem.CurrentType' missing (expected 'AC'), '.Dimensions' missing (expected 'OUTPOST_RACK'), '.ItemClass' missing (expected 'RACK'), '.RackUnitHeight' missing (expected 'HEIGHT_42U'), '.SupportedUplinkGiB' missing (expected [10,40,100]).
- ListCatalogItems full listing (outposts-catalog-list-catalog-items#task#1): divergence key names $.CatalogItems[0]..[36] — every compared index differs (e.g. expected [1] 'OR-G4Q39QN' with m7i/r7i capacities, got 'OR-8RC2ZKS' DISCONTINUED with c5/g4dn/i3en/m5/r5).
- ListOrderableInstanceTypes (outposts-catalog-list-orderable-instance-types#task#1): divergence key names 68 of $.InstanceTypes[0..71]; e.g. expected [1] c5d.24xlarge (196608 MiB), got c5.18xlarge; expected [2] c7i.48xlarge GENERATION_2, got c5.24xlarge GENERATION_1.
- GetOutpostSupportedInstanceTypes (outposts-outpost-lifecycle#task#8): '$.InstanceTypes[0..63]' all reported 'extra (expected None, got ...)' — e.g. got {'InstanceType': 'c5.12xlarge', 'VCPUs': 48} where the recording has no entry.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Representative: fixture outposts-catalog-and-instance-types-browsing, step task#1 — GET /catalog/items and compare CatalogItems[0] field-by-field with the recorded response (ids, MaxSize form, PowerKva/WeightLbs, and the five absent members).
3. Same fixture task#2: GET /catalog/item/{CatalogItemId} and check CurrentType, Dimensions, ItemClass, RackUnitHeight, SupportedUplinkGiB are present.
4. outposts-catalog-list-catalog-items#task#1: GET /catalog/items unfiltered and diff the whole list against the recording.
5. outposts-catalog-list-orderable-instance-types#task#1: GET /instanceTypes and look for the d-suffix Gen-1 and Gen-2 (c7i/c8i/m7i) entries.
6. outposts-outpost-lifecycle#task#8: GET /outposts/{OutpostIdentifier}/supportedInstanceTypes for the fixture's Outpost — the emulator returns the global catalog where the recording returns no entries.
