Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka__lambda.msk-lambda-esm-filter-criteria-delivers-only-matching-records-and-a-cleared-filter-releases-the-rest kafka__lambda.msk-lambda-event-source-mapping-disabled-delivers-nothing-then-reenabled-delivers-the-missed-records
Issue-Key: kafka__lambda.msk-lambda-esm-filter-criteria-delivers-only-matching-records-and-a-cleared-filter-releases-the-rest|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka__lambda.msk-lambda-event-source-mapping-disabled-delivers-nothing-then-reenabled-delivers-the-missed-records|<genexpr>#3|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# MSK bootstrap brokers are published on a kafka-serverless.amazonaws.com host the emulator does not serve

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBootstrapBrokers

## Description
Cause 1 of 1: the MSK bootstrap-broker endpoint the emulator publishes is a real AWS hostname, so no producer or consumer can reach the broker the emulator serves, and every MSK event-source-mapping check dies before exercising Lambda's Kafka integration.

AWS: recorded behavior only for the host — the Kafka Smithy model types the bootstrap-broker strings without constraining the host. The behavioral requirement is that the address the service announces is the address it answers on: in AWS the published `<cluster>.c<n>.kafka-serverless.<region>.amazonaws.com:9098` resolves to the broker that cluster runs. The `host:port` shape is the contract; the host must be one the emulator answers on.

Emulator: publishes `<cluster-id>.c1.kafka-serverless.<region>.amazonaws.com:9098`, a host it does not serve and which does not resolve to the emulator. A client that produces to or consumes from the published bootstrap servers never reaches the emulator.

Why: the bootstrap-broker list is the only way a client finds the cluster. Announcing an unreachable AWS host makes the Kafka data plane unusable: ESM filter-criteria behavior, disable/re-enable catch-up and offset commits can never be observed, and a customer's Kafka client fails with a DNS/connect error rather than a broker response.

Fix: the MSK bootstrap-broker endpoint serializer/state (GetBootstrapBrokers and the cluster's published endpoints) — publish the emulator's own host and port for the broker listener it actually serves, keeping AWS's `host:port` shape.

Traces: kafka__lambda.msk-lambda-esm-filter-criteria-delivers-only-matching-records-and-a-cleared-filter-releases-the-rest, kafka__lambda.msk-lambda-event-source-mapping-disabled-delivers-nothing-then-reenabled-delivers-the-missed-records.

## Evidence
Both failing checks die on the published endpoint, before any ESM behavior is exercised:
simparity.published.PublishedUrlRejected: endpoint_host was given boot-<id>.c1.kafka-serverless.<region>.amazonaws.com:9098 (published by the emulator): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]
(cluster id and port are per-run values; the defect is announcing a host other than the one the emulator is reached on.)

## Steps to Reproduce
1. Create an MSK (serverless) cluster in the emulator.
2. Call GetBootstrapBrokers (or read the cluster's published endpoints).
3. Connect a Kafka client to the returned bootstrap servers — the host does not resolve to the emulator, so the connection never reaches it.
