Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-catalog-get-compatible-kafka-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-catalog-get-compatible-kafka-versions kafka-catalog-list-kafka-versions
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|GetCompatibleKafkaVersions|2xx->2xx|$.compatibleKafkaVersions[*]
Issue-Key: kafka|ListKafkaVersions|2xx->2xx|$.kafkaVersions[*]
Issue-Commit: 2294e811bd9f401401d7efd231d6d04e23ec8f30

# MSK version catalog is stale: ListKafkaVersions and GetCompatibleKafkaVersions diverge from the published catalog

> _Note: this issue was identified against the code base at commit `2294e811bd9f401401d7efd231d6d04e23ec8f30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCompatibleKafkaVersions

## Description
Both of MSK's catalog read operations serve a hard-coded Apache Kafka version table (crate::state::KAFKA_VERSION_CATALOG, consumed by the ListKafkaVersions and GetCompatibleKafkaVersions handlers) whose contents no longer match the catalog MSK publishes. The table is the single source for both responses, so the same staleness shows up twice: as missing/mis-statused versions in the version list, and as wrong upgrade-target sets in the compatibility table. Both steps answer HTTP 200 with a well-formed body — the divergence is purely the catalog data. 2 causes.

Cause 1 of 2: GetCompatibleKafkaVersions (fixture kafka-catalog-get-compatible-kafka-versions, step task#1) returns a compatibility table that is neither the same length nor the same content as MSK's.
AWS: recorded behavior only — the Smithy model (GetCompatibleKafkaVersions -> GetCompatibleKafkaVersionsResponse.CompatibleKafkaVersions, jsonName compatibleKafkaVersions, list of CompatibleKafkaVersion{sourceVersion,targetVersions}) constrains neither the entries nor their order, and no member carries an enum of versions. The recording (HTTP 200) shows one entry per source version MSK currently offers, each carrying only the versions that source can actually be upgraded to, including the KRaft variants: sourceVersion '3.9.x.kraft' -> ['4.1.x.kraft','4.0.x.kraft'], '3.8.x' -> ['3.9.x'], '3.7.x.kraft' -> ['4.1.x.kraft','3.8.x.kraft','3.9.x.kraft','4.0.x.kraft'], '3.6.0' -> ['3.9.x','3.8.x'], '3.8.x.kraft' -> ['4.1.x.kraft','3.9.x.kraft','4.0.x.kraft'], '3.7.x' -> ['3.8.x','3.9.x'], '3.9.x' -> ['3.9.x.kraft'], and terminal versions with an empty list ('4.2.x.kraft' -> [], '1.1.1' -> []). The requirement is the rule, not these literal strings: every version the service lists must appear as a sourceVersion exactly once, and its targetVersions must be that version's real upgrade set (empty for a terminal version).
Emulator: serves a shorter table built from an older version set and a uniform target list. Legacy sources the recording does not place at those positions appear instead ('2.1.0', '2.2.1', '2.4.1', '2.8.2.tiered'), each with the identical targetVersions ['3.6.0','3.7.x','3.8.x','3.9.x'] regardless of source, and the tail entries the recording ends with ('3.7.x', '3.8.x', '3.9.x', '1.1.1') are absent entirely (got None at those indexes). KRaft upgrade paths such as '3.9.x.kraft' -> ['4.1.x.kraft','4.0.x.kraft'] are not represented.
Why: a client that calls GetCompatibleKafkaVersions to decide whether an upgrade is legal before calling UpdateClusterKafkaVersion gets the wrong answer in both directions — it is told a KRaft target is unavailable when AWS allows it, and offered targets (a blanket ['3.6.0','3.7.x','3.8.x','3.9.x'] for any source) that AWS would refuse. Upgrade-planning tooling and version pickers built against the emulator cannot be trusted.
Fix: the shared Kafka version/compatibility catalog in the service's state module (crate::state::KAFKA_VERSION_CATALOG) and the GetCompatibleKafkaVersions handler that projects it, which must emit a per-source upgrade set rather than one shared target list.
Traces: kafka-catalog-get-compatible-kafka-versions#task#1.

Cause 2 of 2: ListKafkaVersions (fixture kafka-catalog-list-kafka-versions, step task#1) omits a published version and reports another version's support status wrongly.
AWS: recorded behavior only — the model (ListKafkaVersions -> ListKafkaVersionsResponse.KafkaVersions, jsonName kafkaVersions, list of KafkaVersion{version,status}) does not enumerate versions or statuses. The recording (HTTP 200) carries the whole published catalog in one fixed order, with each entry's real support state, including {'version': '3.9.x', 'status': 'DEPRECATED'} and a final {'version': '4.2.x.kraft', 'status': 'ACTIVE'}. The requirement is that the catalog served covers every version MSK publishes, with that version's current ACTIVE/DEPRECATED status — not these particular strings.
Emulator: serves a catalog that is one entry short and mis-states a status. At the position the recording holds the DEPRECATED '3.9.x' entry the emulator returns {'version': '4.0.x.kraft', 'status': 'ACTIVE'}, and the recording's trailing {'version': '4.2.x.kraft', 'status': 'ACTIVE'} entry has no counterpart at all (got None). Because the two handlers share the table, this is the same staleness as Cause 1 seen through the list projection.
Why: a client that enumerates ListKafkaVersions to populate a version choice cannot select the newest version MSK offers (it is absent), and is not warned off a version MSK has deprecated — so it will happily create or upgrade clusters onto a version AWS flags as DEPRECATED while believing it is ACTIVE.
Fix: the same shared version catalog in the service's state module, plus the ListKafkaVersions handler's status projection; the catalog must carry per-version status rather than defaulting entries to ACTIVE.
Traces: kafka-catalog-list-kafka-versions#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1, both steps HTTP 200 expected and HTTP 200 actual (body-only divergence):

Cause 1 — kafka-catalog-get-compatible-kafka-versions#task#1, GetCompatibleKafkaVersions:
  $.compatibleKafkaVersions[1]: expected {'sourceVersion': '3.9.x.kraft', 'targetVersions': ['4.1.x.kraft','4.0.x.kraft']}, got {'sourceVersion': '2.1.0', 'targetVersions': ['3.6.0','3.7.x','3.8.x','3.9.x']}
  $.compatibleKafkaVersions[2]: expected {'sourceVersion': '3.8.x', 'targetVersions': ['3.9.x']}, got {'sourceVersion': '2.2.1', 'targetVersions': ['3.6.0','3.7.x','3.8.x','3.9.x']}
  $.compatibleKafkaVersions[4]: expected {'sourceVersion': '3.7.x.kraft', 'targetVersions': ['4.1.x.kraft','3.8.x.kraft','3.9.x.kraft','4.0.x.kraft']}, got {'sourceVersion': '2.4.1', 'targetVersions': ['3.6.0','3.7.x','3.8.x','3.9.x']}
  $.compatibleKafkaVersions[16]: expected {'sourceVersion': '3.9.x.kraft', 'targetVersions': ['4.2.x.kraft']}, got {'sourceVersion': '2.8.2.tiered', 'targetVersions': ['3.6.0','3.7.x','3.8.x','3.9.x']}
  $.compatibleKafkaVersions[27]: expected {'sourceVersion': '3.6.0', 'targetVersions': ['3.9.x','3.8.x']}, got {'sourceVersion': '3.8.x', 'targetVersions': ['3.8.x.kraft','3.9.x']}
  $.compatibleKafkaVersions[34]: expected {'sourceVersion': '3.8.x.kraft', 'targetVersions': ['4.1.x.kraft','3.9.x.kraft','4.0.x.kraft']}, got {'sourceVersion': '4.2.x.kraft', 'targetVersions': []}
  $.compatibleKafkaVersions[35..38]: expected {'3.7.x' -> ['3.8.x','3.9.x']}, {'3.8.x' -> ['3.9.x']}, {'3.9.x' -> ['3.9.x.kraft']}, {'1.1.1' -> []}, got None (list ends early)

Cause 2 — kafka-catalog-list-kafka-versions#task#1, ListKafkaVersions:
  $.kafkaVersions[32]: expected {'version': '3.9.x', 'status': 'DEPRECATED'}, got {'version': '4.0.x.kraft', 'status': 'ACTIVE'}
  $.kafkaVersions[36]: expected {'version': '4.2.x.kraft', 'status': 'ACTIVE'}, got None

Neither cluster is blocked by an unimplemented stub and neither is owned by an open issue (owned_by: []). Related but non-covering open issues: 20261029T202809Z-kafka-msk-error-payloads-name-the-smithy-member-instead.md and 20261029T175822Z-kafka-msk-error-messages-are-the-emulator-s-own.md concern error payload wording, not the success-path version catalog.

## Steps to Reproduce
1. Boot the emulator from this checkout (the kafka service answers; bootstrap-kafka is resolved).
2. GET /v1/compatible-kafka-versions with no clusterArn query parameter (GetCompatibleKafkaVersions catalog mode). Compare the full compatibleKafkaVersions list against fixture kafka-catalog-get-compatible-kafka-versions step task#1: the emulator returns legacy sourceVersions ('2.1.0','2.2.1','2.4.1','2.8.2.tiered') each with the same targetVersions ['3.6.0','3.7.x','3.8.x','3.9.x'], and is missing the recording's trailing '3.7.x', '3.8.x', '3.9.x' and '1.1.1' entries plus the KRaft upgrade paths.
3. GET /v1/kafka-versions (ListKafkaVersions). Compare kafkaVersions against fixture kafka-catalog-list-kafka-versions step task#1: '3.9.x' is not reported DEPRECATED and the final '4.2.x.kraft' ACTIVE entry is absent.
4. Re-replay both fixture steps after correcting the shared catalog in services/service-com-amazonaws-kafka-kafka/src/state.rs (KAFKA_VERSION_CATALOG) and the two handlers' projections; both must pass.
