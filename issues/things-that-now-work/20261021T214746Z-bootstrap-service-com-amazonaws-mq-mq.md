Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the mq emulator

_Filed: 2026-10-21T21:47:46Z_
_Operation: service-com-amazonaws-mq-mq | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `services/service-com-amazonaws-mq-mq/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Amazon MQ client workflow can run at all. Implement a FIRST minimal, stateful slice covering the broker lifecycle (restJson1, sigv4 signing name `mq`, endpoint prefix `mq`).

### Happy path to implement (in order)

1. **CreateBroker** — `POST /v1/brokers`. Required input: `BrokerName`, `EngineType` (ACTIVEMQ|RABBITMQ), `DeploymentMode` (SINGLE_INSTANCE|ACTIVE_STANDBY_MULTI_AZ|CLUSTER_MULTI_AZ), `HostInstanceType`, `PubliclyAccessible`; also persist `EngineVersion`, `AutoMinorVersionUpgrade`, `SecurityGroups`, `SubnetIds`, `StorageType`, `Users`, `Tags`, `CreatorRequestId` (@idempotencyToken). Must mint a fresh `BrokerId` (e.g. `b-<uuid>`) and a `BrokerArn` of the form `arn:aws:mq:<region>:<account>:broker:<BrokerName>:<BrokerId>`, store the broker record, and return both. A second CreateBroker with the same `BrokerName` must fail with `ConflictException` (HTTP 409); missing/invalid required members must fail with `BadRequestException` (HTTP 400).
2. **DescribeBroker** — `GET /v1/brokers/{BrokerId}`. Must look the broker up by the id minted in step 1 and return the persisted values: `BrokerId`, `BrokerArn`, `BrokerName`, `EngineType`, `EngineVersion`, `DeploymentMode`, `HostInstanceType`, `PubliclyAccessible`, `AutoMinorVersionUpgrade`, `SecurityGroups`, `SubnetIds`, `StorageType`, `Created`, `Tags`, `Users` (list<UserSummary>) and `BrokerState`. Unknown id -> `NotFoundException` (HTTP 404).
3. **ListBrokers** — `GET /v1/brokers`. Must return one `BrokerSummary` per stored broker (`BrokerId`, `BrokerArn`, `BrokerName`, `BrokerState`, `EngineType`, `DeploymentMode`, `HostInstanceType`, `Created`) — i.e. exactly the brokers created so far and not the deleted ones — honouring `MaxResults` (1..100) and round-tripping `NextToken`.
4. **CreateUser** — `POST /v1/brokers/{BrokerId}/users/{Username}`. Required `Password`; also persist `ConsoleAccess`, `Groups`, `ReplicationUser`. Empty 200 body. Unknown `BrokerId` -> `NotFoundException`; an already-existing username on that broker -> `ConflictException` (409).
5. **DescribeUser** — `GET /v1/brokers/{BrokerId}/users/{Username}`. Must return the user created in step 4 from state: `BrokerId`, `Username`, `ConsoleAccess`, `Groups`, `ReplicationUser`. Unknown broker or username -> `NotFoundException` (404).
6. **DeleteBroker** — `DELETE /v1/brokers/{BrokerId}`. Must remove the broker (and its users/tags) from state and return `BrokerId`. After it, DescribeBroker on the same id must be `NotFoundException` (404) and ListBrokers must no longer include it. (A faithful emulator may first transition `BrokerState` to `DELETION_IN_PROGRESS`; either way the record must actually disappear so the lifecycle verifies.)

Every one of these must compute its response from persisted state — no canned bodies, no echoing the request back, and no minting an id that is never stored. A create followed by a describe must return what was created.

## State Model

Store in `ComAmazonawsMqMqState` (`src/state.rs`, reached via `crate::state::service_state(&*self.engine)`):

- `brokers: HashMap<String /* BrokerId */, Broker>`
- `broker_names: HashMap<String /* BrokerName */, String /* BrokerId */>` — or enforce name uniqueness by scanning `brokers` — to make the duplicate-name `ConflictException` possible
- `Broker { broker_id: String, broker_arn: String, broker_name: String, engine_type: EngineType, engine_version: Option<String>, deployment_mode: DeploymentMode, host_instance_type: String, publicly_accessible: bool, auto_minor_version_upgrade: Option<bool>, authentication_strategy: Option<AuthenticationStrategy>, storage_type: Option<BrokerStorageType>, security_groups: Vec<String>, subnet_ids: Vec<String>, state: BrokerState, created: SystemTime, creator_request_id: Option<String>, users: BTreeMap<String /* Username */, BrokerUser>, tags: BTreeMap<String, String> }`
- `BrokerUser { username: String, password: String (never echoed back), console_access: bool, groups: Vec<String>, replication_user: bool }`
- `next_id` / uuid source used to mint `b-<uuid>` broker ids, plus the region/account used to build the ARN, so `BrokerArn` is stable across calls.

A single shared `ListBrokers` pagination helper over the `brokers` map (stable ordering by `created` then `broker_id`, opaque `NextToken` encoding the offset) keeps step 3 honest.

The remaining 19 operations (configurations, tags, update/reboot/promote, engine-type and instance-option catalogs, shared resources) stay 501 for now and are tracked as individual gap issues.

## Evidence

`services/service-com-amazonaws-mq-mq/OPERATIONS.md` lists all 25 operations with their routes, members and errors: CreateBroker `POST /v1/brokers` returning `BrokerArn`/`BrokerId` with `ConflictException (409)`/`BadRequestException (400)`; DescribeBroker `GET /v1/brokers/{BrokerId}` with `NotFoundException (404)`; ListBrokers `GET /v1/brokers` @paginated(items=BrokerSummaries, MaxResults 1..100); CreateUser/DescribeUser `/v1/brokers/{BrokerId}/users/{Username}`; DeleteBroker `DELETE /v1/brokers/{BrokerId}` returning `BrokerId`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-mq-mq/src/ops/` matches all 25 handler files (create_broker.rs, describe_broker.rs, list_brokers.rs, create_user.rs, describe_user.rs, delete_broker.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, and the crate README confirms a scaffold holds zero state.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `mq` client at it.
2. `aws mq create-broker --broker-name demo-broker --engine-type ACTIVEMQ --deployment-mode SINGLE_INSTANCE --host-instance-type mq.t3.micro --no-publicly-accessible --users Username=admin,Password=SuperSecret123` — expect 200 with a minted BrokerId and BrokerArn; currently HTTP 501 NotImplemented.
3. `aws mq describe-broker --broker-id <BrokerId>` — expect the persisted BrokerName/EngineType/DeploymentMode/HostInstanceType/Created back.
4. `aws mq list-brokers` — expect exactly one BrokerSummary with that BrokerId and BrokerName.
5. `aws mq create-user --broker-id <BrokerId> --username alice --password AlicePass12345 --console-access` then `aws mq describe-user --broker-id <BrokerId> --username alice` — expect ConsoleAccess=true and the stored groups.
6. `aws mq create-broker --broker-name demo-broker ...` again — expect ConflictException (HTTP 409).
7. `aws mq delete-broker --broker-id <BrokerId>` — expect 200 echoing BrokerId; then `describe-broker` on it must be NotFoundException (404) and `list-brokers` must return an empty list.
8. `aws mq describe-broker --broker-id b-does-not-exist` — expect NotFoundException (404), not 501.
