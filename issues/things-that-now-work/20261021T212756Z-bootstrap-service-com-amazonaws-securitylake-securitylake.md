Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the securitylake emulator

_Filed: 2026-10-21T21:27:56Z_
_Operation: service-com-amazonaws-securitylake-securitylake | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 31 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Security Lake client workflow can run. Implement a FIRST SLICE of 6 operations covering the data lake + subscriber lifecycle, all computed from persisted state (no canned, hardcoded or echoed responses):

1. **CreateDataLake** (`POST /v1/datalake`) — input `configurations` (list<DataLakeConfiguration>: `region` required, optional `encryptionConfiguration`, `lifecycleConfiguration`, `replicationConfiguration`), `metaStoreManagerRoleArn` (required), optional `tags`. For each configuration, store one data lake keyed by region and return `dataLakes` (list<DataLakeResource>) with a generated `dataLakeArn` (`arn:aws:securitylake:<region>:<account>:data-lake/default`), the `region`, a derived `s3BucketArn` (e.g. `arn:aws:s3:::aws-security-data-lake-<region>-<suffix>`), the configuration echoed back from what was STORED, and `createStatus` = `INITIALIZED`/`COMPLETED`. A second CreateDataLake for a region that already exists must fail with `ConflictException` (409).
2. **ListDataLakes** (`GET /v1/datalakes`, query `regions`) — return `dataLakes` built from the stored map, filtered by `regions` when supplied; empty list (not an error) when nothing was created.
3. **CreateSubscriber** (`POST /v1/subscribers`) — input `subscriberIdentity` (AwsIdentity: `principal`, `externalId` both required), `subscriberName` (required), `sources` (list<LogSourceResource> union of `awsLogSource`/`customLogSource`, required), optional `subscriberDescription`, `accessTypes`, `tags`. Must require an existing data lake (else `ResourceNotFoundException` 404), allocate a UUID `subscriberId` matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, derive `subscriberArn` (`arn:aws:securitylake:<region>:<account>:subscriber/<subscriberId>`), set `subscriberStatus` (`ACTIVE`), `createdAt`/`updatedAt`, persist the record, and return it as `subscriber`.
4. **GetSubscriber** (`GET /v1/subscribers/{subscriberId}`) — look the stored subscriber up by id and return the SAME `SubscriberResource` fields that CreateSubscriber returned (identity, name, description, sources, accessTypes, arn, status, timestamps). Unknown id ⇒ `ResourceNotFoundException` (404).
5. **ListSubscribers** (`GET /v1/subscribers`, query `maxResults` 1..100 default 50, `nextToken` ≤2048) — page over the stored subscribers in a stable order, truncate at `maxResults`, and return an opaque `nextToken` only when more remain; a token round trip must resume exactly after the last returned item.
6. **DeleteSubscriber** (`DELETE /v1/subscribers/{subscriberId}`) — remove the stored subscriber so a following GetSubscriber returns 404 and ListSubscribers no longer includes it. Unknown id ⇒ `ResourceNotFoundException` (404).

Optionally close the loop with **DeleteDataLake** (`POST /v1/datalake/delete`, input `regions` required) removing the stored data lake per region so ListDataLakes no longer reports it.

Every response field above must be read back out of the store — a create followed by a read must return what was created, and a delete must be observable by the next call.

## State Model

In `src/state.rs` (`ComAmazonawsSecuritylakeSecuritylakeState`, reached via `crate::state::service_state(&*self.engine)`):

- `data_lakes: HashMap<String /* region */, DataLake>` where `DataLake { data_lake_arn: String, region: String, s3_bucket_arn: String, encryption_configuration: Option<DataLakeEncryptionConfiguration>, lifecycle_configuration: Option<DataLakeLifecycleConfiguration>, replication_configuration: Option<DataLakeReplicationConfiguration>, meta_store_manager_role_arn: String, create_status: DataLakeStatus, update_status: Option<DataLakeUpdateStatus>, tags: Vec<Tag> }`
- `subscribers: IndexMap<String /* subscriberId (UUID) */, Subscriber>` (insertion-ordered, for stable pagination) where `Subscriber { subscriber_id: String, subscriber_arn: String, subscriber_identity: AwsIdentity /* principal, external_id */, subscriber_name: String, subscriber_description: Option<String>, sources: Vec<LogSourceResource>, access_types: Vec<AccessType>, role_arn: Option<String>, s3_bucket_arn: Option<String>, subscriber_endpoint: Option<String>, subscriber_status: SubscriberStatus, resource_share_arn: Option<String>, resource_share_name: Option<String>, created_at: SystemTime, updated_at: SystemTime, tags: Vec<Tag> }`
- `account_id: String` and `region: String` used to mint ARNs consistently across operations.
- Pagination tokens for ListSubscribers: opaque base64 of the index/key of the next item, validated on input (malformed ⇒ `BadRequestException` 400).

All other operations (log sources, organization configuration, delegated administrator, exception subscription, tagging, notifications, Update*) stay 501 for now and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-securitylake-securitylake/OPERATIONS.md` lists all 31 operations with their routes, inputs, outputs and errors — CreateDataLake (`POST /v1/datalake` -> `dataLakes` list<DataLakeResource>), ListDataLakes (`GET /v1/datalakes`), CreateSubscriber (`POST /v1/subscribers` -> `subscriber` SubscriberResource), GetSubscriber (`GET /v1/subscribers/{subscriberId}`), ListSubscribers (`GET /v1/subscribers`, @paginated items=subscribers), DeleteSubscriber (`DELETE /v1/subscribers/{subscriberId}`), DeleteDataLake (`POST /v1/datalake/delete`). Field lists for `DataLakeResource`, `SubscriberResource`, `DataLakeConfiguration`, `AwsIdentity` and `LogSourceResource` come from the Smithy model (`smithy_query shape`), including the required-ness of `dataLakeArn`/`region` and of `subscriberId`/`subscriberArn`/`subscriberIdentity`/`subscriberName`/`sources`. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-securitylake-securitylake/src/ops` matches all 31 handler files (e.g. `src/ops/create_data_lake.rs`, `src/ops/create_subscriber.rs`, `src/ops/get_subscriber.rs`, `src/ops/list_subscribers.rs`, `src/ops/delete_subscriber.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI securitylake client at it.
2. `CreateDataLake` with configurations=[{region: "us-east-1"}] and metaStoreManagerRoleArn="arn:aws:iam::123456789012:role/AmazonSecurityLakeMetaStoreManager" — expect 200 with dataLakes[0].dataLakeArn, region=us-east-1, s3BucketArn and a createStatus; today: HTTP 501 NotImplemented.
3. `ListDataLakes` — expect the data lake just created to appear with the same ARN and region.
4. `CreateSubscriber` with subscriberIdentity={principal:"123456789012", externalId:"ext-1"}, subscriberName="test-sub", sources=[{awsLogSource:{sourceName:"ROUTE53", sourceVersion:"2.0"}}] — expect 200 with a UUID subscriberId and subscriberArn.
5. `GetSubscriber` with that subscriberId — expect the stored subscriber with identical subscriberName, subscriberIdentity and sources.
6. `ListSubscribers` with maxResults=1 after creating two subscribers — expect one subscriber plus a nextToken, and the second subscriber on the follow-up call with that token.
7. `DeleteSubscriber` with the subscriberId, then `GetSubscriber` again — expect 404 ResourceNotFoundException and absence from ListSubscribers.
8. `DeleteDataLake` with regions=["us-east-1"], then `ListDataLakes` — expect an empty dataLakes list.
