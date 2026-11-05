Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-catalog-get-container-service-powers
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-catalog-get-container-service-powers
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetContainerServicePowers|2xx->2xx|$.powers[name="large"].ramSizeInGb
Issue-Key: lightsail|GetContainerServicePowers|2xx->2xx|$.powers[name="medium"].ramSizeInGb
Issue-Key: lightsail|GetContainerServicePowers|2xx->2xx|$.powers[name="small"].price
Issue-Key: lightsail|GetContainerServicePowers|2xx->2xx|$.powers[name="small"].ramSizeInGb
Issue-Key: lightsail|GetContainerServicePowers|2xx->2xx|$.powers[name="xlarge"].ramSizeInGb
Issue-Commit: f796db456a5a81ca5d275c1726373159d3dcda31

# Lightsail GetContainerServicePowers serves wrong RAM sizes and a wrong small-tier price

> _Note: this issue was identified against the code base at commit `f796db456a5a81ca5d275c1726373159d3dcda31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetContainerServicePowers

## Description
One net-new root cause in the Lightsail static-catalog family: the container-service power table backing GetContainerServicePowers answers 200 but with the wrong ramSizeInGb on every tier above nano, and the wrong price on the small tier.

Cause 1 of 1: GetContainerServicePowers reports RAM and price values AWS does not serve.
AWS: Smithy (com.amazonaws.lightsail#GetContainerServicePowers -> output member `powers`, a ContainerServicePowerList of com.amazonaws.lightsail#ContainerServicePower) models powerId, name, price ("The monthly price of the power in USD"), cpuCount ("The number of vCPUs included in the power"), ramSizeInGb ("The amount of RAM (in GB) of the power") and isActive. The model fixes the members but not their values, so this is a static reference table and the catalog contents are recorded behavior only: the recorded response reports power name "small" with ramSizeInGb 1.0 and price 15.0, "medium" with ramSizeInGb 2.0, "large" with ramSizeInGb 4.0 and "xlarge" with ramSizeInGb 8.0.
Emulator: the same named powers are served with ramSizeInGb doubled on every tier — small 2.0, medium 4.0, large 8.0, xlarge 16.0 — and small's price is 20.0 instead of 15.0. Status is 200 in both cases and the tier names, powerId set and cpuCount values match; only these value members diverge. Hypothesis: the table was populated by shifting each tier's RAM up one step (each tier carries the next tier's RAM), with the small tier's price taken from that next step too.
Why: a power is chosen from this listing and its `name`/`powerId` is then passed to CreateContainerService, so a client sizing a container service on the advertised RAM provisions a service with half the memory it selected, and any cost estimate or budget check built on the small tier's price is 33% high. Because the divergence is only in the value members, a client has no way to detect it: the listing looks well-formed.
Fix: the crate's static container-service power catalog table backing the GetContainerServicePowers handler — correct ramSizeInGb for the small, medium, large and xlarge tiers and the small tier's price. This is data in that table, not handler logic; cpuCount, name, powerId and isActive are already correct.
Traces: lightsail-catalog-get-container-service-powers#task#1.

Related but NOT covering this: the open issue "Lightsail static catalog listings serve stale, reordered and incomplete entries" (20261029T234852Z-lightsail-static-catalog-listings-serve-stale-reordered-and.md) files eight causes in this same family — GetBlueprints, GetBundles, GetContainerAPIMetadata, GetLoadBalancerTlsPolicies, GetRegions, GetRelationalDatabaseBlueprints and GetRelationalDatabaseBundles. The container-service power table is not among them and that issue carries no replay key for this fixture, so this cause must be closed separately.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. Cluster verdict=mismatch, status 200 -> 200 (the operation answers; the catalog data diverges). Fixture lightsail-catalog-get-container-service-powers, step task#1, affected_count 1, stub_path empty, owned_by empty (no open issue carries this replay key).

Diverging fields, expected (recorded AWS) vs actual (emulator):
  $.powers[name="small"].price         expected 15.0, got 20.0
  $.powers[name="small"].ramSizeInGb   expected 1.0,  got 2.0
  $.powers[name="medium"].ramSizeInGb  expected 2.0,  got 4.0
  $.powers[name="large"].ramSizeInGb   expected 4.0,  got 8.0
  $.powers[name="xlarge"].ramSizeInGb  expected 8.0,  got 16.0

No status, error code or error-message divergence is involved: this is a 200 response whose value members are wrong. The nano tier and every tier's cpuCount, name, powerId and isActive matched, which is what isolates the fault to these two columns of the table rather than to the handler or the serializer. The numbers above are the recorded contents of a static reference table, which is exactly what this operation is required to serve.

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener.

1. Call ls.get_container_service_powers().
2. Index the returned `powers` list by `name`.
3. Assert the small tier: powers[name="small"].ramSizeInGb == 1.0 and .price == 15.0. Observed ramSizeInGb 2.0 and price 20.0.
4. Assert the remaining tiers' RAM: medium == 2.0, large == 4.0, xlarge == 8.0. Observed 4.0, 8.0 and 16.0 respectively — each tier carrying the next tier's RAM.
5. Confirm the rest of the record is already correct: the tier names, powerId values, cpuCount values and isActive flags match the recording, so only ramSizeInGb (small/medium/large/xlarge) and the small tier's price need to change.

Re-replay: lightsail-catalog-get-container-service-powers#task#1.
