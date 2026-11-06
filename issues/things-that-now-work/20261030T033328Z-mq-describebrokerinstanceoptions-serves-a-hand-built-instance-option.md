Issue-Source: trace-sweep-divergence
Issue-Trace: mq-catalog-describe-broker-instance-options
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-catalog-describe-broker-instance-options
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|DescribeBrokerInstanceOptions|2xx->2xx|$.brokerInstanceOptions[*]
Issue-Commit: 283a38fd1792cd1319725c85b0b39091ba419fd6

# mq DescribeBrokerInstanceOptions serves a hand-built instance-option catalog that diverges from Amazon MQ's published one

> _Note: this issue was identified against the code base at commit `283a38fd1792cd1319725c85b0b39091ba419fd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBrokerInstanceOptions

## Description
Cause 1 of 1: DescribeBrokerInstanceOptions answers 200 but every element of `brokerInstanceOptions` differs from the recorded Amazon MQ response, because the emulator synthesises the catalog from a hard-coded per-engine table plus derivation rules instead of reporting the option set Amazon MQ actually publishes. Fixture mq-catalog-describe-broker-instance-options, step task#1.

AWS: Smithy: DescribeBrokerInstanceOptions GET /v1/broker-instance-options -> DescribeBrokerInstanceOptionsResponse.BrokerInstanceOptions (list of BrokerInstanceOption{availabilityZones, engineType: EngineType[ACTIVEMQ|RABBITMQ], hostInstanceType: __string, storageType: BrokerStorageType[EBS|EFS], supportedDeploymentModes: DeploymentMode[SINGLE_INSTANCE|ACTIVE_STANDBY_MULTI_AZ|CLUSTER_MULTI_AZ], supportedEngineVersions}); errors BadRequestException (400), ForbiddenException (403), InternalServerErrorException (500). The model does not state the catalog's CONTENT or its ORDER, so for those this is recorded behavior only. The recording shows, for the region the request signed for:
  * one element per (host instance type, engine type, storage type) combination Amazon MQ offers, grouped by hostInstanceType and only then by engineType — the recorded first three elements are all the same hostInstanceType (the lexicographically first `mq.m5.*` size) carrying engineType ACTIVEMQ/storageType ebs, ACTIVEMQ/efs, then RABBITMQ/ebs;
  * `storageType` rendered in LOWER case (`ebs`, `efs`) even though the modeled BrokerStorageType enum spells the values EBS/EFS;
  * `supportedEngineVersions` at MAJOR.MINOR granularity and newest-first (ActiveMQ `["5.19", "5.18"]`, RabbitMQ `["3.13"]`) — the versions Amazon MQ currently offers for creation, not historical patch-level versions;
  * `supportedDeploymentModes` carrying the engine's highly-available mode alongside SINGLE_INSTANCE on the `mq.m5` sizes (ACTIVE_STANDBY_MULTI_AZ for ActiveMQ, CLUSTER_MULTI_AZ for RabbitMQ);
  * `availabilityZones` listing every zone of the region (four for the recorded region), the same list on every element.
  The recorded response carries 21 elements; element order is stable across calls, which is what makes a nextToken offset name the same element on a later call. (AZ ORDER inside the list and the concrete zone names/region are environment facts, not requirements — the requirement is that the list is the region's full zone set and is identical on every element.)

Emulator: builds the list in services/service-com-amazonaws-mq-mq/src/ops/describe_broker_instance_options.rs from the `PUBLISHED_ENGINE_OPTIONS` constant and the `offers_storage_type`/`supports_deployment_mode` predicates, yielding 15 elements ordered engine-type-first (all ACTIVEMQ sizes, then all RABBITMQ sizes). Consequently: element[0] is `mq.t2.micro`/ACTIVEMQ/`EFS`, element[1] `mq.t3.micro`/ACTIVEMQ/`EFS`, element[2] `mq.m5.large`/ACTIVEMQ/`EBS`, where the recording has the first `mq.m5.*` size three times over; `storageType` is emitted UPPER case (`EBS`/`EFS`) against the recording's lower case; `supportedEngineVersions` is the same patch-level list on every ActiveMQ element (`["5.15.16", "5.16.7", "5.17.6", "5.18.4"]`) and `["3.11.28", "3.12.13", "3.13.7"]` on every RabbitMQ one, in ascending order; `supportedDeploymentModes` is `["SINGLE_INSTANCE"]` on the elements the recording gives a multi-AZ mode (ActiveMQ EBS `mq.m5.*` and RabbitMQ `mq.m5.*` never get ACTIVE_STANDBY_MULTI_AZ/CLUSTER_MULTI_AZ because `offers_storage_type`/`supports_deployment_mode` make ActiveMQ's pair EFS-only and treat RabbitMQ as EBS-only); `availabilityZones` has three zones instead of the region's full set; and recorded elements 15..20 have no counterpart at all because the synthesised catalog is shorter than the published one.

Why: this operation is the catalog a client consults BEFORE creating a broker, so every element is a decision input. A client that filters it by `storageType=ebs`/`efs` (the casing the recording returns) matches nothing; one that picks an engine version from `supportedEngineVersions` passes a patch-level string Amazon MQ no longer accepts at CreateBroker; one that checks `supportedDeploymentModes` concludes ACTIVE_STANDBY_MULTI_AZ and CLUSTER_MULTI_AZ are unavailable on every size and either refuses to offer HA deployments or sends a CreateBroker the emulator's own catalog says is impossible; and one that pins an instance type by index or iterates the list to its end sees a different, shorter catalog in a different order, so paging with nextToken walks different elements than against AWS. Related but NOT covering this: the open issue 'Implement mq DescribeBrokerEngineTypes: unimplemented scaffold stub' owns the sibling engine/version reporting operation — the version vocabulary fixed here must agree with whatever that stub is implemented to report.

Fix: the catalog data and the derivation rules in this operation's handler — services/service-com-amazonaws-mq-mq/src/ops/describe_broker_instance_options.rs (`PUBLISHED_ENGINE_OPTIONS`, `offers_storage_type`, `supports_deployment_mode`, `published_instance_options`) — plus the serializer's casing for the `storageType` member and the region zone list the handler reads via `state::availability_zones_in_region`. Ordering must become hostInstanceType-then-engineType-then-storageType so the paging offset stays stable, and the element count/combination set must match the published catalog.

Traces: mq-catalog-describe-broker-instance-options#task#1 (DescribeBrokerInstanceOptions, 200 vs 200, all of $.brokerInstanceOptions[0]..[20] diverge).

## Evidence
Replay of fixture mq-catalog-describe-broker-instance-options, step task#1 (verdict mismatch, expected_status 200, actual_status 200). Diverging keys: $.brokerInstanceOptions[0] through $.brokerInstanceOptions[20] (21 elements).

  $.brokerInstanceOptions[0]
    expected {'hostInstanceType': 'mq.m5.2xlarge', 'engineType': 'ACTIVEMQ', 'supportedEngineVersions': ['5.19', '5.18'], 'availabilityZones': [4 zones], 'storageType': 'ebs', 'supportedDeploymentModes': ['SINGLE_INSTANCE']}
    got      {'hostInstanceType': 'mq.t2.micro', 'engineType': 'ACTIVEMQ', 'supportedEngineVersions': ['5.15.16', '5.16.7', '5.17.6', '5.18.4'], 'availabilityZones': [3 zones], 'storageType': 'EFS', 'supportedDeploymentModes': ['SINGLE_INSTANCE']}
  $.brokerInstanceOptions[1]
    expected {'hostInstanceType': 'mq.m5.2xlarge', 'engineType': 'ACTIVEMQ', 'supportedEngineVersions': ['5.19', '5.18'], 'storageType': 'efs', 'supportedDeploymentModes': ['SINGLE_INSTANCE', 'ACTIVE_STANDBY_MULTI_AZ']}
    got      {'hostInstanceType': 'mq.t3.micro', 'engineType': 'ACTIVEMQ', 'supportedEngineVersions': ['5.15.16', '5.16.7', '5.17.6', '5.18.4'], 'storageType': 'EFS', 'supportedDeploymentModes': ['SINGLE_INSTANCE']}
  $.brokerInstanceOptions[2]
    expected {'hostInstanceType': 'mq.m5.2xlarge', 'engineType': 'RABBITMQ', 'supportedEngineVersions': ['3.13'], 'storageType': 'ebs', 'supportedDeploymentModes': ['SINGLE_INSTANCE', 'CLUSTER_MULTI_AZ']}
    got      {'hostInstanceType': 'mq.m5.large', 'engineType': 'ACTIVEMQ', 'supportedEngineVersions': ['5.15.16', '5.16.7', '5.17.6', '5.18.4'], 'storageType': 'EBS', 'supportedDeploymentModes': ['SINGLE_INSTANCE']}

Fixed vs variable parts: the member names, the enum spellings for engineType/supportedDeploymentModes, the lower-case storageType rendering, the per-element grouping order and the presence of the engine's HA deployment mode are the requirement; the concrete region, its zone NAMES and the AZ list's internal order are environment values, not requirements.

Source of the emulator's values: services/service-com-amazonaws-mq-mq/src/ops/describe_broker_instance_options.rs lines 264-288 (PUBLISHED_ENGINE_OPTIONS), 305-311 (offers_storage_type), 320-339 (supports_deployment_mode), 345-388 (published_instance_options: engine-type-outer loop fixes the order and emits the uppercase enum names).

Model checked with smithy_query (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json): operation DescribeBrokerInstanceOptions; shapes BrokerInstanceOption, BrokerStorageType ([EBS, EFS], doc: 'EFS is not supported for RabbitMQ engine type'), EngineType ([ACTIVEMQ, RABBITMQ]), DeploymentMode ([SINGLE_INSTANCE, ACTIVE_STANDBY_MULTI_AZ, CLUSTER_MULTI_AZ]). The model constrains the enums only; it says nothing about catalog content or order, so those are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture mq-catalog-describe-broker-instance-options, step task#1 (GET /v1/broker-instance-options against the mq endpoint, unfiltered).
2. Compare the JSON body's $.brokerInstanceOptions list element by element with the recorded response.
3. Observe: 15 elements instead of the recorded 21; element order grouped by engineType rather than by hostInstanceType; storageType emitted as 'EBS'/'EFS' where the recording has 'ebs'/'efs'; supportedEngineVersions patch-level and ascending ('5.15.16','5.16.7','5.17.6','5.18.4') where the recording has major.minor newest-first ('5.19','5.18') and RabbitMQ '3.13'; supportedDeploymentModes ['SINGLE_INSTANCE'] on every element where the recording adds ACTIVE_STANDBY_MULTI_AZ (ActiveMQ) / CLUSTER_MULTI_AZ (RabbitMQ) on the mq.m5 sizes; availabilityZones carrying three zones instead of the region's full zone set.
4. Re-replay the same fixture step after the fix; every $.brokerInstanceOptions index must match.

## Resolution
Fixed in the handler and the two helpers it shares: the catalog is now grouped by host instance
type, then engine type, then storage type; `storageType` is rendered in the lower-case spelling the
catalog publishes; `supportedEngineVersions` reports the offered versions newest first at
MAJOR.MINOR granularity; `supportedDeploymentModes` lists SINGLE_INSTANCE first and then the
engine's own highly available mode wherever that combination offers it; and `availabilityZones`
reports the signed Region's whole zone set, identical on every element. Replaying the fixture step,
recorded elements 0..11 and 20 now match element for element (0 of 21 matched before).

The catalog's remaining MEMBERSHIP gap — the eight recorded elements at indices 12..19, whose host
instance types the replay withholds — is tracked as its own report,
`20261107T120000Z-mq-describebrokerinstanceoptions-catalog-is-missing-eight-published-instance-option-combinations.md`,
which records what is known about them and the candidate families already ruled out. Acceptance
step 4 above is met for every index that report does not own.
