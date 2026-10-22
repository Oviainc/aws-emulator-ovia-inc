Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-outpost-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-outpost-lifecycle outposts-site-lifecycle outposts-update-site-rack-physical-properties-persists
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|GetOutpost|2xx->2xx|$.Outpost.Description
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.Description
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.Name
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.FiberOpticCableType
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.MaximumSupportedWeightLbs
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.OpticalStandard
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.PowerConnector
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.PowerDrawKva
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.PowerFeedDrop
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.PowerPhase
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.UplinkCount
Issue-Key: outposts|GetSite|2xx->2xx|$.Site.RackPhysicalProperties.UplinkGbps
Issue-Related: 20261021T235115Z-outposts-site-and-outpost-response-projection-omits-tags.md
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# Outposts update operations do not persist mutated fields

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOutpost

## Description
The emulator accepts the outposts PATCH update operations with 200 but does not write the mutated members into state: a read-back after the update still returns the pre-update values. Three causes, one per update operation.

Cause 1 of 3: UpdateOutpost does not persist Description.
AWS: Smithy: UpdateOutpost (PATCH /outposts/{OutpostId}) input Name/Description/SupportedHardwareType, output Outpost. The recording shows a subsequent GetOutpost returning the Description supplied to UpdateOutpost ('Updated outpost desc' in this fixture).
Emulator: GetOutpost after the update returns 200 with $.Outpost.Description still the creation-time value ('Probe test outpost'). The same response also shows the Tags/AvailabilityZoneId/LifeCycleStatus projection defects filed separately under the Site/Outpost response-projection issue; the update-persistence defect is this cause.
Why: a client that renames or re-describes an Outpost and then reads it back sees its write silently lost — the 200 reports success that never happened.
Fix: the UpdateOutpost handler's write path into the outposts state store (apply present input members to the stored Outpost).
Traces: outposts-outpost-lifecycle#task#6.

Cause 2 of 3: UpdateSite does not persist Name or Description.
AWS: Smithy: UpdateSite (PATCH /sites/{SiteId}) input Name/Description/Notes, output Site. The recording shows a subsequent GetSite returning the updated Name and Description.
Emulator: GetSite after the update returns 200 with $.Site.Name and $.Site.Description still the creation-time values.
Why: site renames are lost, so any workflow that updates then reads (or lists by name) diverges from AWS.
Fix: the UpdateSite handler's write path into the stored Site.
Traces: outposts-site-lifecycle#task#5.

Cause 3 of 3: UpdateSiteRackPhysicalProperties does not persist any rack property.
AWS: Smithy: UpdateSiteRackPhysicalProperties (PATCH /sites/{SiteId}/rackPhysicalProperties) input PowerDrawKva, PowerPhase, PowerConnector, PowerFeedDrop, UplinkGbps, UplinkCount, FiberOpticCableType, OpticalStandard, MaximumSupportedWeightLbs (all enums), output Site. Site.RackPhysicalProperties is documented as "Information about the physical and logistical details for a rack at the site". The recording shows a subsequent GetSite returning every property as submitted, including ones that were unset at create time.
Emulator: GetSite after the update returns 200 with the submitted enum members either still at their creation values (PowerConnector, PowerDrawKva, PowerFeedDrop, PowerPhase, UplinkCount, UplinkGbps) or entirely absent (FiberOpticCableType, MaximumSupportedWeightLbs, OpticalStandard).
Why: rack-property configuration is unobservable, so a client cannot confirm a rack order's physical requirements were recorded; the absent members additionally break clients that read RackPhysicalProperties sub-fields without null checks.
Fix: the UpdateSiteRackPhysicalProperties handler's write path into the stored Site.RackPhysicalProperties, plus the RackPhysicalProperties serializer for members set only by this op.
Traces: outposts-update-site-rack-physical-properties-persists#task#3.

## Evidence
Cause 1 — GetOutpost (after UpdateOutpost), expected 200 / actual 200: $.Outpost.Description expected 'Updated outpost desc', got 'Probe test outpost'.
Cause 2 — GetSite (after UpdateSite), 200/200: $.Site.Description expected 'Updated description', got 'Probe site for lifecycle test'; $.Site.Name expected the '-updated' name, got the original name.
Cause 3 — GetSite (after UpdateSiteRackPhysicalProperties), 200/200: $.Site.RackPhysicalProperties.FiberOpticCableType missing (expected 'MULTI_MODE'), MaximumSupportedWeightLbs missing (expected 'MAX_2000_LBS'), OpticalStandard missing (expected 'OPTIC_10GBASE_SR'), PowerConnector expected 'CS8365C' got 'L6_30P', PowerDrawKva expected 'POWER_30_KVA' got 'POWER_5_KVA', PowerFeedDrop expected 'BELOW_RACK' got 'ABOVE_RACK', PowerPhase expected 'THREE_PHASE' got 'SINGLE_PHASE', UplinkCount expected 'UPLINK_COUNT_2' got 'UPLINK_COUNT_1', UplinkGbps expected 'UPLINK_10G' got 'UPLINK_1G'.
The specific strings are the fixtures' request values; the requirement is that a read-back returns whatever the update submitted. state_reset_failures was 0 for this replay, so these are not cross-fixture state leaks.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay outposts-outpost-lifecycle through step task#6 — UpdateOutpost sets a new Description, then GetOutpost at task#6 still returns the old one.
3. Cause 2: replay outposts-site-lifecycle through step task#5 — UpdateSite sets a new Name/Description, then GetSite at task#5 still returns the old ones.
4. Cause 3: replay outposts-update-site-rack-physical-properties-persists through step task#3 — UpdateSiteRackPhysicalProperties submits nine enum properties, then GetSite at task#3 returns the pre-update values or omits the members.
