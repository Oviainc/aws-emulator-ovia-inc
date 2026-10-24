Issue-Source: trace-sweep-divergence
Issue-Trace: directconnect-catalog-describe-locations
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: directconnect-catalog-describe-locations
Issue-Shape-Id: com.amazonaws.directconnect#OvertureService
Issue-Key: directconnect|DescribeLocations|2xx->2xx|$.locations[*]
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# DescribeLocations returns an invented location catalog instead of the AWS Direct Connect catalog

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLocations

## Description
Cause 1 of 1: the seeded Direct Connect location catalog the emulator serves from DescribeLocations does not match the catalog AWS publishes, so every element of the response body diverges even though the status is right.

AWS: Smithy: DescribeLocations has input smithy.api#Unit and output Locations{locations: LocationList of Location{locationCode, locationName, region, availablePortSpeeds, availableProviders, availableMacSecPortSpeeds, availableBillingModes}}; errors DirectConnectClientException (400) / DirectConnectServerException (500). The model documents locationCode as "The code for the location" and locationName as "the name of the colocation partner and the physical site of the building", and the operation doc states these "are the locations that can be selected when calling CreateConnection or CreateInterconnect". The catalog's CONTENT is recorded behavior only: it is not account- or run-specific (the operation takes no input and no caller identity narrows it), it is the global public facility list for the signed Region. The recording for us-west-2 shows, for each facility, a hyphenated per-partner locationCode (e.g. "EqSe2-EQ", "EqSe2-WBE"), a locationName combining both the building and the partner site (e.g. "Westin Building Exchange, Seattle, WA | Equinix, SE2, Seattle, WA"), a full ~30-entry availableProviders list ("Equinix, Inc.", "Megaport", "Zayo Group", "CenturyLink", "AT&T", "PacketFabric", "Arelion", ...), per-facility availablePortSpeeds (some facilities 1G/10G/100G only, others including 400G), availableMacSecPortSpeeds narrowed to the facility's own 10G/100G/400G speeds, and availableBillingModes carrying exactly one entry, billingMode "PayAsYouGo", with no includedRegions. Ten locations were expected on this step.

Emulator: answers 200 with a different, hand-seeded catalog. Every expected element is reported missing ($.locations[0] through $.locations[9]): the codes are unhyphenated short forms ("EqSe2" instead of "EqSe2-EQ"/"EqSe2-WBE"), locationName is a short partner-only string ("Equinix SE2, Seattle, WA"), availableProviders is a 3-entry list ("Equinix", "Megaport", "Zayo") whose names are not the provider names AWS reports, availablePortSpeeds is the same uniform 1G/10G/100G/400G at every facility, and availableBillingModes carries a second, non-AWS mode "FlatRateTier1" with an includedRegions list.

Why: DescribeLocations is the discovery call a client makes before CreateConnection, CreateInterconnect or CreateLag, and locationCode is the value it then passes as the location. A client that reads the emulator's catalog gets codes AWS never issues and never sees the per-partner codes it must choose between, so location selection cannot be exercised; a client that hardcodes a real code ("EqSe2-EQ") finds no matching location at all. The uniform port-speed list hides that only some facilities offer 400G, and the invented FlatRateTier1 billing mode makes a client believe a billing mode exists that AWS does not offer at any location, so billing-mode selection logic is validated against a fiction.

Fix: the seeded location catalog in this service's state (the per-location seed data, plus the derived-speed/billing-mode constants DEDICATED_PORT_SPEEDS, MAC_SEC_PORT_SPEEDS and USAGE_BILLING_MODE), not the handler, which only renders what is stored. The catalog needs per-partner location codes and combined building+partner names, per-facility port-speed lists rather than one uniform list, the real provider names, MACsec speeds derived from the facility's own speeds, and availableBillingModes limited to PayAsYouGo with no includedRegions.

Traces: fixture directconnect-catalog-describe-locations step task#1 (expected 200, got 200; body mismatch on $.locations[0..9]). Emulator components: services/service-com-amazonaws-directconnect-overtureservice/src/ops/describe_locations.rs:160-174 (renders store.locations_in_region / store.location_output) and services/service-com-amazonaws-directconnect-overtureservice/src/state.rs:99-131 (DEDICATED_PORT_SPEEDS, MAC_SEC_PORT_SPEEDS, USAGE_BILLING_MODE, mac_sec_port_speeds).

## Evidence
Step directconnect-catalog-describe-locations#task#1, verdict mismatch, expected_status 200, actual_status 200.

Expected $.locations[0]: {locationCode: 'EqSe2-EQ', locationName: 'Westin Building Exchange, Seattle, WA | Equinix, SE2, Seattle, WA', region: 'us-west-2', availablePortSpeeds: ['100G','1G','10G'], availableMacSecPortSpeeds: ['100G','10G'], availableProviders: ~30 entries including 'Equinix, Inc.', 'Zayo Group', 'CenturyLink', 'AT&T', 'PacketFabric', 'Arelion', availableBillingModes: [{billingMode: 'PayAsYouGo', availablePortSpeeds: ['100G','1G','10G']}]}.
Expected $.locations[1]: locationCode 'EqSe2-WBE', availablePortSpeeds ['100G','1G','10G','400G'], availableMacSecPortSpeeds ['100G','10G','400G'], one availableBillingModes entry 'PayAsYouGo'.

Actual $.locations[0]: {locationCode: 'EqSe2', locationName: 'Equinix SE2, Seattle, WA', region: 'us-west-2', availablePortSpeeds: ['1G','10G','100G','400G'], availableProviders: ['Equinix','Megaport','Zayo'], availableMacSecPortSpeeds: ['10G','100G'], availableBillingModes: [{billingMode: 'PayAsYouGo', availablePortSpeeds: ['1G','10G','100G','400G']}, {billingMode: 'FlatRateTier1', availablePortSpeeds: ['1G','10G','100G','400G'], includedRegions: ['us-east-1','us-east-2','us-west-2']}]}.

Divergence key: service=directconnect|DescribeLocations|status=200->200|keys=$.locations[0],$.locations[1],...,$.locations[9] — i.e. all ten expected locations are absent from the emulator's response.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture directconnect-catalog-describe-locations, step task#1 (a DescribeLocations call signed for us-west-2 with an empty body).
2. Compare the returned locations list against the recorded one: the status matches (200) but no recorded location is present.
3. Equivalently, call DescribeLocations against the emulator in us-west-2 and look for locationCode 'EqSe2-EQ' or 'EqSe2-WBE' (the codes a client would pass to CreateConnection); the emulator reports only 'EqSe2'. Also note availableBillingModes contains 'FlatRateTier1', a mode AWS returns at no location.
