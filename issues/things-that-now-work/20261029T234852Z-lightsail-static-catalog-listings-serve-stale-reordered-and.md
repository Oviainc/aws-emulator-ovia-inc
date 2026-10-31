Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-catalog-get-blueprints
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-catalog-get-blueprints lightsail-catalog-and-operations-lifecycle lightsail-catalog-get-bundles lightsail-catalog-get-container-api-metadata lightsail-catalog-get-load-balancer-tls-policies lightsail-catalog-get-regions lightsail-cloudformation-and-copy-error-paths lightsail-instance-error-paths lightsail-catalog-get-relational-database-blueprints lightsail-catalog-get-relational-database-bundles
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetBlueprints|2xx->2xx|$.blueprints[*]
Issue-Key: lightsail|GetBundles|2xx->2xx|$.bundles[*]
Issue-Key: lightsail|GetContainerAPIMetadata|2xx->2xx|$.metadata[name="lightsailctlVersion"].value
Issue-Key: lightsail|GetLoadBalancerTlsPolicies|2xx->2xx|$.tlsPolicies[name="TLS-2016-08"].description
Issue-Key: lightsail|GetLoadBalancerTlsPolicies|2xx->2xx|$.tlsPolicies[name="TLS-2016-08"].protocols[*]
Issue-Key: lightsail|GetLoadBalancerTlsPolicies|2xx->2xx|$.tlsPolicies[name="TLS-FS-1-1-2019-08"]
Issue-Key: lightsail|GetLoadBalancerTlsPolicies|2xx->2xx|$.tlsPolicies[name="TLS-FS-1-2-2019-08"]
Issue-Key: lightsail|GetLoadBalancerTlsPolicies|2xx->2xx|$.tlsPolicies[name="TLS-FS-1-2-Res-2019-08"].description
Issue-Key: lightsail|GetLoadBalancerTlsPolicies|2xx->2xx|$.tlsPolicies[name="TLS-FS-1-2-Res-2019-08"].protocols[*]
Issue-Key: lightsail|GetLoadBalancerTlsPolicies|2xx->2xx|$.tlsPolicies[name="TLS-FS-2018-06"]
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-east-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-south-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-2"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-3"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ap-southeast-5"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="ca-central-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-central-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-north-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-south-2"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-west-1"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-west-2"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="eu-west-3"].description
Issue-Key: lightsail|GetRegions|2xx->2xx|$.regions[name="sa-east-1"].description
Issue-Key: lightsail|GetRelationalDatabaseBlueprints|2xx->2xx|$.blueprints[*]
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="2Xlarge with High Availability"]
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="2Xlarge"]
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Large with High Availability"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Large"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Medium with High Availability"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Medium"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Micro with High Availability"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Micro with High Availability"].cpuCount
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Micro"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Micro"].cpuCount
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Small with High Availability"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Small with High Availability"].cpuCount
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Small"].bundleId
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Small"].cpuCount
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Xlarge with High Availability"]
Issue-Key: lightsail|GetRelationalDatabaseBundles|2xx->2xx|$.bundles[name="Xlarge"]
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# Lightsail static catalog listings serve stale, reordered and incomplete entries

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBlueprints

## Description
Eight net-new root causes in one family: every one of the Lightsail read-only CATALOG operations answers 200 from a hand-written table in the crate that does not match the catalog AWS serves — wrong element order, stale version/price/power values, missing tiers, missing members and extra entries. These are static reference tables, so the fix is data, per table, but each table is its own cause and must be closed separately.

Cause 1 of 8: GetBlueprints serves a short, differently ordered instance-blueprint table, so no index matches.
AWS: Smithy (com.amazonaws.lightsail#Blueprint) models blueprintId, name, group, type, description, isActive, minPower, version, versionCode, productUrl, licenseUrl, platform, appCategory. Recorded behavior: the listing has 39 entries, begins with the WINDOWS blueprints (blueprintId "windows_server_2025", group "windows_2025", platform "WINDOWS", minPower 500, with version, versionCode, productUrl and licenseUrl all populated), then "windows_server_2022", and so on.
Emulator: entry 0 is "amazon_linux_2" (platform LINUX_UNIX, minPower 0, no versionCode, no productUrl, no licenseUrl) and the whole list is a shorter Linux-first set, so all 39 recorded positions mismatch.
Why: a client that selects a blueprint by list position, or filters on platform/minPower/versionCode, picks the wrong image or finds none; the Windows catalog is absent altogether, so no Windows instance workflow can be driven.
Fix: the crate's static blueprint catalog table backing the GetBlueprints handler — populate the full AWS blueprint set, in AWS's order, with every modeled member including versionCode/productUrl/licenseUrl.
Traces: lightsail-catalog-get-blueprints#task#1.

Cause 2 of 8: the same GetBlueprints table is also wrong when read inside the catalog+operations lifecycle fixture (entry 0 expected "windows_server_2025", got "amazon_linux_2"), confirming it is the shared table and not a per-fixture filter.
AWS: as cause 1.
Emulator: as cause 1.
Why: as cause 1.
Fix: as cause 1 — one table, but this trace must be re-replayed too, because it exercises GetBlueprints through a different fixture path.
Traces: lightsail-catalog-and-operations-lifecycle#task#4.

Cause 3 of 8: GetBundles serves instance bundles with wrong cpuCount/power/price and a different ordering.
AWS: recorded behavior — the bundle list carries 100+ entries; bundle "nano_3_0" has cpuCount 2, power 298, price 5.0, diskSizeInGb 20, ramSizeInGb 0.5, supportedPlatforms [LINUX_UNIX]; "micro_3_0" has cpuCount 2, diskSizeInGb 40, power 500, price 7.0; "small_3_0" cpuCount 2, diskSizeInGb 60, power 1000, price 12.0; LINUX_UNIX tiers come before the WINDOWS variants.
Emulator: "nano_3_0" has cpuCount 1, power 300, price 3.5; position 1 is "nano_win_3_0" (the WINDOWS variant interleaved early) and position 2 is "micro_3_0" with cpuCount 1 and price 5.0 — every recorded position mismatches.
Why: bundle power is the value the blueprint minPower is compared against and price/cpuCount are what sizing and cost logic read, so a client choosing a bundle from this listing provisions and bills against values AWS does not serve.
Fix: the crate's static instance-bundle catalog table backing GetBundles.
Traces: lightsail-catalog-get-bundles#task#1.

Cause 4 of 8: GetContainerAPIMetadata reports a stale lightsailctl version.
AWS: recorded behavior — the metadata list contains the entry name "lightsailctlVersion" with value "1.0.7".
Emulator: the same entry with value "1.0.6".
Why: this operation exists so a client can discover the lightsailctl plugin version it must have; a stale value makes a correctly updated client believe it is ahead of, or mismatched with, the service.
Fix: the static container-API metadata table backing GetContainerAPIMetadata.
Traces: lightsail-catalog-get-container-api-metadata#task#1.

Cause 5 of 8: GetLoadBalancerTlsPolicies serves the wrong policy set, non-AWS protocol tokens and invented descriptions.
AWS: recorded behavior — policy "TLS-2016-08" has description "Enables TLS versions 1, 1.1, and 1.2" and protocols ["Protocol-TLSv1", "Protocol-TLSV1.1", "Protocol-TLSv1.2"]; policy "TLS-FS-1-2-Res-2019-08" has description "Enables only TLS version 1.2" and protocols ["Protocol-TLSv1.2"]. The protocol tokens are the "Protocol-" prefixed form (note the capitalisation AWS uses in "Protocol-TLSV1.1"), and the served set does not include TLS-FS-2018-06, TLS-FS-1-1-2019-08 or TLS-FS-1-2-2019-08.
Emulator: the descriptions are cipher-suite prose ("Negotiates TLSv1, TLSv1.1 and TLSv1.2 with 18 cipher suites"), the protocols are bare "TLSv1"/"TLSv1.1"/"TLSv1.2" without the "Protocol-" prefix, and three extra policies (TLS-FS-2018-06, TLS-FS-1-1-2019-08, TLS-FS-1-2-2019-08) are present.
Why: the protocol token is a wire value a client echoes back when configuring a load balancer, so the unprefixed form is not interchangeable; and a client offering the extra policies configures policies the service does not have.
Fix: the static TLS-policy table backing GetLoadBalancerTlsPolicies — AWS's policy set, AWS's "Protocol-" tokens, AWS's descriptions.
Traces: lightsail-catalog-get-load-balancer-tls-policies#task#1.

Cause 6 of 8: GetRegions serves hand-written region descriptions.
AWS: recorded behavior — each region's `description` is AWS's own sentence, e.g. eu-west-1 and eu-west-2 both "This region is recommended to serve users in Ireland, the United Kingdom, and Iceland", eu-central-1 "This region is recommended to serve users in Europe, the Middle East, and Africa", eu-north-1 "This region is recommended to serve users in Stockholm and north Europe", and opt-in regions are prefixed "This opt-in Region (Region that is disabled by default) is recommended to serve users in ...".
Emulator: a different sentence per region (eu-west-1 "... in Ireland and western Europe", eu-central-1 "... in Germany and central Europe", eu-north-1 "... in the Nordic countries", eu-south-2 "... in Spain and Portugal" with no opt-in prefix), diverging on 14 regions.
Why: the description is the string a console or picker shows, and the opt-in prefix is the only signal in this response that a region is disabled by default, so a client cannot tell opt-in regions apart.
Fix: the static region table backing GetRegions, including the opt-in prefix rule.
Traces: lightsail-catalog-get-regions#task#1 (the same cluster also covers lightsail-cloudformation-and-copy-error-paths#task#1 and lightsail-instance-error-paths#task#7).

Cause 7 of 8: GetRelationalDatabaseBlueprints serves stale engine versions and drops AWS's version-description suffix.
AWS: recorded behavior — e.g. blueprintId "postgres_13" has engineVersion "13.23-rds.20260514" and engineVersionDescription "PostgreSQL 13.23-rds.20260514-R1"; "postgres_14" 14.24 / "PostgreSQL 14.24-R1"; "postgres_15" 15.19 / "-R1"; "postgres_16" 16.15 / "-R1"; "postgres_17" 17.11 / "-R1". The engineVersionDescription is "<engineDescription> <engineVersion>-R1".
Emulator: the same blueprintIds carry older versions (postgres_13 -> 13.21, postgres_14 -> 14.18, postgres_15 -> 15.13, postgres_16 -> 16.9) and engineVersionDescription without the "-R1" suffix.
Why: a client that creates a database by blueprintId and then asserts the engine version it got, or that pins a minor version from this catalog, reads a version the service does not offer.
Fix: the static relational-database blueprint table backing GetRelationalDatabaseBlueprints, including the "-R1" engineVersionDescription form.
Traces: lightsail-catalog-get-relational-database-blueprints#task#1.

Cause 8 of 8: GetRelationalDatabaseBundles serves the superseded 1_0 bundle generation and omits the largest tiers.
AWS: recorded behavior — bundle ids are the 2_0 generation ("micro_2_0", "micro_ha_2_0", "small_2_0", "small_ha_2_0", "medium_2_0", "medium_ha_2_0", "large_2_0", "large_ha_2_0") with cpuCount 2 on the micro and small tiers, and the catalog additionally contains "xlarge_2_0", "xlarge_ha_2_0", "2xlarge_2_0" and "2xlarge_ha_2_0" (isEncrypted true, with their own cpuCount/ram/disk/price/transfer values).
Emulator: the ids are the 1_0 generation ("micro_1_0", "micro_ha_1_0", "small_1_0", ... "large_ha_1_0") with cpuCount 1 on micro/small, and the Xlarge and 2Xlarge tiers (plus their HA variants) are absent entirely.
Why: bundle id is the value passed to CreateRelationalDatabase, so every id from this listing is rejected or creates the wrong class; and the two largest tiers cannot be selected at all.
Fix: the static relational-database bundle table backing GetRelationalDatabaseBundles — the 2_0 generation ids with their cpuCount and the missing xlarge/2xlarge tiers.
Traces: lightsail-catalog-get-relational-database-bundles#task#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. All eight clusters are verdict=mismatch with status 200 -> 200, i.e. the operations answer but with diverging catalog data.

Cause 1 — lightsail-catalog-get-blueprints#task#1 (GetBlueprints): $.blueprints[0]..$.blueprints[38] all mismatch. Expected [0] blueprintId 'windows_server_2025' (group 'windows_2025', platform 'WINDOWS', minPower 500, versionCode '1', productUrl and licenseUrl present); got blueprintId 'amazon_linux_2' (group 'amazon-linux', platform 'LINUX_UNIX', minPower 0, no versionCode/productUrl/licenseUrl). Expected [1] 'windows_server_2022'.
Cause 2 — lightsail-catalog-and-operations-lifecycle#task#4 (GetBlueprints): $.blueprints[0] expected 'windows_server_2025', got 'amazon_linux_2'.
Cause 3 — lightsail-catalog-get-bundles#task#1 (GetBundles): $.bundles[0]..$.bundles[99] all mismatch. Expected [0] nano_3_0 cpuCount 2 / power 298 / price 5.0; got nano_3_0 cpuCount 1 / power 300 / price 3.5. Expected [1] micro_3_0 (cpuCount 2, disk 40, power 500, price 7.0); got nano_win_3_0 (WINDOWS). Expected [2] small_3_0; got micro_3_0 cpuCount 1 price 5.0.
Cause 4 — lightsail-catalog-get-container-api-metadata#task#1 (GetContainerAPIMetadata): $.metadata[name="lightsailctlVersion"].value expected '1.0.7', got '1.0.6'.
Cause 5 — lightsail-catalog-get-load-balancer-tls-policies#task#1 (GetLoadBalancerTlsPolicies): $.tlsPolicies[name="TLS-2016-08"].description expected 'Enables TLS versions 1, 1.1, and 1.2', got 'Negotiates TLSv1, TLSv1.1 and TLSv1.2 with 18 cipher suites'; .protocols[0..2] expected 'Protocol-TLSv1' / 'Protocol-TLSV1.1' / 'Protocol-TLSv1.2', got 'TLSv1' / 'TLSv1.1' / 'TLSv1.2'; $.tlsPolicies[name="TLS-FS-1-2-Res-2019-08"].description expected 'Enables only TLS version 1.2', got 'Negotiates TLSv1.2 with 8 cipher suites, every one of which provides forward secrecy'; .protocols[0] expected 'Protocol-TLSv1.2', got 'TLSv1.2'; extra policies served: TLS-FS-2018-06, TLS-FS-1-1-2019-08, TLS-FS-1-2-2019-08.
Cause 6 — lightsail-catalog-get-regions#task#1 (GetRegions, cluster also covers lightsail-cloudformation-and-copy-error-paths#task#1 and lightsail-instance-error-paths#task#7): 14 region descriptions mismatch, e.g. eu-west-1 expected 'This region is recommended to serve users in Ireland, the United Kingdom, and Iceland', got 'This region is recommended to serve users in Ireland and western Europe'; eu-south-2 expected the 'This opt-in Region (Region that is disabled by default) ...' form, got 'This region is recommended to serve users in Spain and Portugal'.
Cause 7 — lightsail-catalog-get-relational-database-blueprints#task#1: $.blueprints[1..6] mismatch, e.g. postgres_13 expected engineVersion '13.23-rds.20260514' / engineVersionDescription 'PostgreSQL 13.23-rds.20260514-R1', got '13.21' / 'PostgreSQL 13.21'; postgres_14 expected 14.24 / 'PostgreSQL 14.24-R1', got 14.18 / 'PostgreSQL 14.18'.
Cause 8 — lightsail-catalog-get-relational-database-bundles#task#1: $.bundles[name="Micro"].bundleId expected 'micro_2_0', got 'micro_1_0' and .cpuCount expected 2, got 1 (same for Micro/Small HA and non-HA); Medium/Large and their HA variants expected '*_2_0', got '*_1_0'; $.bundles[name="Xlarge"], ["Xlarge with High Availability"], ["2Xlarge"] and ["2Xlarge with High Availability"] expected present, got None.

(Version strings, prices and powers above are the recorded catalog contents of these reference tables, which is exactly what this family of operations is required to serve.)

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener.

1. ls.get_blueprints() -> blueprints[0].blueprintId must be 'windows_server_2025' with platform 'WINDOWS', minPower 500 and versionCode/productUrl/licenseUrl populated, and the list must hold 39 entries in AWS's order. Observed 'amazon_linux_2' first and a shorter Linux-only set. Repeat inside the catalog/operations lifecycle fixture path for cause 2.
2. ls.get_bundles() -> bundles[0] must be nano_3_0 with cpuCount 2, power 298, price 5.0, and the LINUX_UNIX tiers must precede the WINDOWS variants. Observed cpuCount 1, power 300, price 3.5 and nano_win_3_0 at index 1.
3. ls.get_container_api_metadata() -> the 'lightsailctlVersion' entry must be '1.0.7'. Observed '1.0.6'.
4. ls.get_load_balancer_tls_policies() -> TLS-2016-08.protocols must be ['Protocol-TLSv1','Protocol-TLSV1.1','Protocol-TLSv1.2'] with description 'Enables TLS versions 1, 1.1, and 1.2', and TLS-FS-2018-06 / TLS-FS-1-1-2019-08 / TLS-FS-1-2-2019-08 must not be served. Observed bare 'TLSv1...' tokens, cipher-suite descriptions and the three extra policies.
5. ls.get_regions() -> each region's description must be AWS's sentence, with opt-in regions prefixed 'This opt-in Region (Region that is disabled by default) ...'. Observed different sentences and no opt-in prefix.
6. ls.get_relational_database_blueprints() -> postgres_13 must report engineVersion '13.23-rds.20260514' and engineVersionDescription ending '-R1'. Observed '13.21' and no '-R1'.
7. ls.get_relational_database_bundles() -> bundle ids must be the '*_2_0' generation with cpuCount 2 on micro/small, and xlarge_2_0 / xlarge_ha_2_0 / 2xlarge_2_0 / 2xlarge_ha_2_0 must be present. Observed '*_1_0' ids, cpuCount 1, and the xlarge/2xlarge tiers missing.

Re-replay: lightsail-catalog-get-blueprints#task#1, lightsail-catalog-and-operations-lifecycle#task#4, lightsail-catalog-get-bundles#task#1, lightsail-catalog-get-container-api-metadata#task#1, lightsail-catalog-get-load-balancer-tls-policies#task#1, lightsail-catalog-get-regions#task#1, lightsail-catalog-get-relational-database-blueprints#task#1, lightsail-catalog-get-relational-database-bundles#task#1.
