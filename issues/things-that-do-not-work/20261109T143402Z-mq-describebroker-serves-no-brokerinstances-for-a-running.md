Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order|_rabbitmq_broker#1|AssertionError|assert amqps, f"a RUNNING RabbitMQ broker advertised no amqps:// endpoint: {endpoints!r}"
Issue-Key: mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason|_rabbitmq_broker#1|AssertionError|assert amqps, f"a RUNNING RabbitMQ broker advertised no amqps:// endpoint: {endpoints!r}"
Issue-Commit: 4107fd5c25009167eba32ef42f98137c54de7c7a

# mq DescribeBroker serves no BrokerInstances for a RUNNING RabbitMQ broker, so no amqps:// endpoint is ever advertised

> _Note: this issue was identified against the code base at commit `4107fd5c25009167eba32ef42f98137c54de7c7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBroker

## Description
ONE net-new root cause, grounded in two failing behavioral checks that are NOT carried by any open issue's replay keys.

Cause 1 of 1: once a RABBITMQ broker reports BrokerState RUNNING, DescribeBroker's 200 body still omits brokerInstances entirely, so the broker publishes no wire-level endpoint and no console URL and a client has no address to connect to. Failing checks: mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order, mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason.

AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json) gives DescribeBroker output the member BrokerInstances (jsonName "brokerInstances", list of BrokerInstance), and BrokerInstance declares Endpoints (jsonName "endpoints", "The broker's wire-level protocol endpoints"), ConsoleURL ("The brokers web console URL") and IpAddress ("Does not apply to RabbitMQ brokers"). For a SINGLE_INSTANCE RABBITMQ broker in state RUNNING, AWS returns exactly one BrokerInstance whose Endpoints carry the broker's AMQPS endpoint — scheme amqps://, the RabbitMQ AMQP TLS port 5671 — and whose ConsoleURL addresses the RabbitMQ management console; IpAddress is not reported for RabbitMQ. The host name and broker id in those URIs are environment values; the requirement is that a RUNNING RabbitMQ broker reports one BrokerInstance with at least one amqps:// endpoint and a ConsoleURL.
Emulator: DescribeBroker returns 200 with brokerInstances absent, for RABBITMQ as for ACTIVEMQ. Replay (both checks): AssertionError: a RUNNING RabbitMQ broker advertised no amqps:// endpoint: [] — the flattened BrokerInstances[].Endpoints list is empty because the member is not serialized at all. Source: services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs, the DescribeBrokerOutput construction at lines 232-311, which never sets broker_instances and lets it fall to `..Default::default()`.
Relation to existing issues: Cause 5 of the open issue 20261104T044607Z-mq-broker-lifecycle-rebootbroker-and-promote-are-unimplemented.md describes the same handler gap but is grounded ONLY in mq.native-protocol-stomp-roundtrip-activemq (an ACTIVEMQ broker and its STOMP+SSL endpoint); neither RabbitMQ check above appears in its Issue-Trace-Ids/Issue-Key list, so the RabbitMQ engine's endpoint set (one amqps:// endpoint, no IpAddress) and these two replay keys are not covered by it. Fixing that issue must also make these two checks pass.
Why: every Amazon MQ for RabbitMQ consumer discovers where to connect from BrokerInstances[].Endpoints — AMQP 0-9-1 client libraries, Lambda event source mappings and console links alike. An absent member reads as "a RUNNING broker with no addresses", which AWS never returns, so a client built against the emulator cannot open a connection at all and code that derives its endpoint from the DescribeBroker response gets null.
Fix: the broker state plus DescribeBroker's (and CreateBroker's/ListBrokers') response construction in services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs — derive and store the instance endpoints and console URL when the broker reaches RUNNING, with the endpoint set chosen by EngineType (RABBITMQ: one amqps:// endpoint on 5671 plus the management ConsoleURL and no IpAddress; ACTIVEMQ: one URI per supported wire protocol), and serve them in brokerInstances. Hypothesis (neither the model nor these recordings state it, because both checks stop at endpoint discovery): the AMQP-over-TLS listener the advertised endpoint names is also absent, so after the endpoint is published these checks will additionally need an AMQP 0-9-1 data plane that honours durable queues, publisher confirms, persistent delivery order, and x-dead-letter-exchange routing with the x-death reason.
Traces: mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order, mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 12/28 passed; service mq 12/23 (52%), 11 failed. Nine of those eleven are excluded here as already carried: mq.delete-user-last-admin-accepted is ALREADY FILED (RebootBroker stub), and mq.create-configuration-duplicate-name-conflict-rejected, mq.create-user-weak-password-rejected, mq.delete-broker-removes-it, mq.native-protocol-stomp-roundtrip-activemq, mq.promote-non-replica-broker-rejected, mq.reboot-broker-applies-pending-user-change, mq.reboot-broker-wrong-state-rejected are marked OWNED by open issues.

mq.rabbitmq-durable-queue-holds-confirmed-persistent-messages-across-connections-in-order [mismatch]:
  File "check_mq_rabbitmq_durable_queue_...py", line 28, in run
    endpoint, username, password = _rabbitmq_broker(ctx)
  File "...", line 277, in _rabbitmq_broker
    assert amqps, f"a RUNNING RabbitMQ broker advertised no amqps:// endpoint: {endpoints!r}"
  AssertionError: a RUNNING RabbitMQ broker advertised no amqps:// endpoint: []

mq.rabbitmq-dead-letter-exchange-takes-unrequeued-and-expired-messages-with-their-death-reason [mismatch]:
  same helper, line 267: AssertionError: a RUNNING RabbitMQ broker advertised no amqps:// endpoint: []

Both checks first CreateBroker(EngineType=RABBITMQ, EngineVersion=3.13, DeploymentMode=SINGLE_INSTANCE, HostInstanceType=mq.m5.large, PubliclyAccessible=False, one user) and poll DescribeBroker until BrokerState == RUNNING — which the emulator does report. They then flatten BrokerInstances[].Endpoints and filter for the amqps:// scheme: expected at least one amqps:// URI on port 5671 (plus a ConsoleURL on the same instance); actual, the response has no brokerInstances member, so the flattened list is [].

Source: services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs lines 232-311 — broker_instances is never assigned and is filled by `..Default::default()`.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from the checkout, and replay the behavioral suite (simparity-behavioral-aws-v1), service mq.
2. CreateBroker with BrokerName=<unique>, DeploymentMode=SINGLE_INSTANCE, EngineType=RABBITMQ, EngineVersion=3.13, HostInstanceType=mq.m5.large, PubliclyAccessible=false, one SubnetId, one SecurityGroup and one user.
3. Poll GET /v1/brokers/{BrokerId} until brokerState is RUNNING.
4. Read brokerInstances from that response. Expect one BrokerInstance with consoleURL set and endpoints containing an amqps:// URI on port 5671 (no ipAddress for RabbitMQ). The emulator's 200 body contains no brokerInstances member at all, so endpoints flattens to [].
5. Repeat with EngineType=ACTIVEMQ: brokerInstances is likewise absent (that engine's variant is tracked as Cause 5 of 20261104T044607Z-mq-broker-lifecycle-rebootbroker-and-promote-are-unimplemented.md).
6. With the endpoint published, the two RabbitMQ checks continue on it: connect AMQP 0-9-1 over TLS to the advertised amqps:// endpoint with the created user, declare a durable queue, publish persistent messages with publisher confirms, reconnect and consume them in order; and declare a queue with x-dead-letter-exchange, then nack-without-requeue / let a message expire and read the dead-letter queue's x-death reason.
