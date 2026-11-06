Issue-Source: trace-sweep-divergence
Issue-Trace: mq-catalog-describe-broker-instance-options
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-catalog-describe-broker-instance-options
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|DescribeBrokerInstanceOptions|2xx->2xx|$.brokerInstanceOptions[*]
Issue-Related: 20261030T033328Z-mq-describebrokerinstanceoptions-serves-a-hand-built-instance-option.md

# mq DescribeBrokerInstanceOptions is missing eight of the published instance-option combinations

## Operation
DescribeBrokerInstanceOptions

## Description
Cause 1 of 1: the catalog's RULES are now the service's (see the related issue, which fixed the
grouping, the `storageType` rendering, the engine-version list, the deployment modes and the
Availability Zone set), but its MEMBERSHIP is short: the emulator reports 13 of the 21 combinations
the recording carries, so elements 12 through 19 of `brokerInstanceOptions` have no counterpart.

AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json)
constrains only the enums of `BrokerInstanceOption` — `EngineType` [ACTIVEMQ, RABBITMQ],
`BrokerStorageType` [EBS, EFS] ("EFS is not supported for RabbitMQ engine type"), `DeploymentMode`
[SINGLE_INSTANCE, ACTIVE_STANDBY_MULTI_AZ, CLUSTER_MULTI_AZ] — so WHICH (host instance type, engine
type, storage type) combinations the service offers is recorded behaviour. The recording carries 21
elements, sorted by host instance type, then engine type, then storage type.

Emulator: `PUBLISHED_HOST_INSTANCE_TYPES` in
services/service-com-amazonaws-mq-mq/src/ops/describe_broker_instance_options.rs holds the
combinations the recording confirms element for element — the four `mq.m5` sizes on ActiveMQ (ebs
and efs) and RabbitMQ (ebs), and the burstable `micro` size on ActiveMQ (efs) — which the replay
matches at recorded indices 0..11 and 20. The eight recorded elements in between are served by
nothing.

Why: this operation is the catalog a client consults BEFORE creating a broker. A client that sizes a
broker from it, or iterates it to the end, sees a shorter offer set than AWS publishes, and an
instance type AWS offers looks unavailable.

What is known about the missing eight, from the replay: their host instance types sort strictly
between `mq.m5.xlarge` and `mq.t3.micro`, because the element before them is the last `mq.m5` entry
and the element after them is the burstable `micro` entry. Their members follow the rules the
related issue established for every other element (lower-case `storageType`, the engine's offered
versions newest first, SINGLE_INSTANCE first plus the engine's highly available mode where it is
offered, the signed Region's whole zone set), unless one of those rules does not hold for them. The
replay reports only WHICH element indices differ and withholds every recorded value, so the host
instance types cannot be read out of it; these candidate families and forms were probed and match no
recorded element: the `mq.m5a`/`mq.m5d`/`mq.m5n` variants, `mq.m6a`/`mq.m6g`/`mq.m6i`,
`mq.m7a`/`mq.m7g`/`mq.m7i`, `mq.m8g` and `mq.r5`/`mq.r6g`/`mq.r7g`/`mq.r8g` (every published size of
the Graviton families, for RabbitMQ on ebs and for ActiveMQ on ebs and efs, with and without the
highly available mode, with the Region's whole zone set and with a three-zone subset, and with a
newer RabbitMQ major version), the previous-generation `mq.m4` sizes, the `mq.t2` burstable size in
every storage/mode/zone form, and a RabbitMQ entry for the `mq.t3` burstable size.

Fix: name the eight combinations and add one row each to `PUBLISHED_HOST_INSTANCE_TYPES` (host
instance type plus the engines offered on it); the per-entry members are already derived by
`offers_storage_type`, `highly_available_deployment_mode`, `offered_engine_versions` and
`catalog_storage_type`, so no handler logic should need to change unless one of those rules does not
hold for the missing hosts. A source that states the host instance type a recorded element carries —
the recorded response body itself, or Amazon MQ's published instance-type tables for the Region the
fixture signs for — is what this needs.

## Evidence
Replay of fixture mq-catalog-describe-broker-instance-options, step task#1 (expected_status 200,
actual_status 200): `$.brokerInstanceOptions[12]` through `$.brokerInstanceOptions[19]` differ;
indices 0..11 and 20 match element for element.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture
   mq-catalog-describe-broker-instance-options, step task#1 (GET /v1/broker-instance-options,
   unfiltered).
2. Compare the JSON body's `$.brokerInstanceOptions` list element by element with the recorded
   response: 13 of the recorded 21 elements match, and the eight at indices 12..19 are absent.
3. After adding the missing combinations, every `$.brokerInstanceOptions` index must match.
