Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Elasticsearch Service emulator

_Filed: 2026-10-21T21:08:48Z_
_Operation: service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, while holding zero state. No client workflow works today.

Implement a FIRST, SMALL, stateful slice: the lifecycle of the primary resource, the Elasticsearch **domain** (restJson1, signing name `es`, URI prefix `/2015-01-01`). Scope is exactly these 6 operations, in this order:

1. **CreateElasticsearchDomain** — `POST /2015-01-01/es/domain`. Input: `DomainName` (required, len 3..28, `^[a-z][a-z0-9\-]+$`), `ElasticsearchVersion`, `ElasticsearchClusterConfig`, `EBSOptions`, `AccessPolicies`, `SnapshotOptions`, `AdvancedOptions`, `TagList`, etc. Output: `DomainStatus` (ElasticsearchDomainStatus) — must carry the generated `DomainId`, the `DomainName` as given, a synthesized `ARN` (`arn:aws:es:<region>:<account>:domain/<DomainName>`), `Created=true`, `Deleted=false`, `Processing=true`, `Endpoint`, and the `ElasticsearchClusterConfig` (required member) echoed from the STORED record — not from the request object. Creating a name that already exists must fail with `ResourceAlreadyExistsException` (HTTP 409).
2. **DescribeElasticsearchDomain** — `GET /2015-01-01/es/domain/{DomainName}`. Output: `DomainStatus` (required) read back from the stored record; must reflect everything Create stored plus any later Update. Unknown domain -> `ResourceNotFoundException` (HTTP 409).
3. **DescribeElasticsearchDomainConfig** — `GET /2015-01-01/es/domain/{DomainName}/config`. Output: `DomainConfig` (ElasticsearchDomainConfig, required) built from the stored config, each sub-option wrapped with its `Status` (UpdateDate/UpdateVersion/State). Unknown domain -> `ResourceNotFoundException`.
4. **UpdateElasticsearchDomainConfig** — `POST /2015-01-01/es/domain/{DomainName}/config`. Input: any subset of `ElasticsearchClusterConfig`, `EBSOptions`, `SnapshotOptions`, `AdvancedOptions`, `AccessPolicies`, `LogPublishingOptions`, `DomainEndpointOptions`, plus `DryRun`. Must MERGE the supplied fields into the stored record (fields absent from the request stay unchanged), bump the config version, and return `DomainConfig` reflecting the merged state. With `DryRun=true` nothing may be persisted. Unknown domain -> `ResourceNotFoundException`.
5. **ListDomainNames** — `GET /2015-01-01/domain`, optional `EngineType` query filter. Output: `DomainNames` (list<DomainInfo{DomainName, EngineType}>) enumerated from the store — must include every live domain and must NOT include deleted ones.
6. **DeleteElasticsearchDomain** — `DELETE /2015-01-01/es/domain/{DomainName}`. Output: `DomainStatus` of the removed domain with `Deleted=true`; afterwards the domain disappears from ListDomainNames and DescribeElasticsearchDomain returns `ResourceNotFoundException` (HTTP 409). Deleting a non-existent domain -> `ResourceNotFoundException`.

NEVER STUBS: every one of these six must compute its response from persisted state. Hardcoded `DomainStatus` bodies, echoing the request struct straight back without storing it, or generating a `DomainId`/ARN that is never saved are all failures. A create followed by a describe in a separate request must return what was created.

## State Model

In `src/state.rs`, on the service state struct:

- `domains: BTreeMap<String /* DomainName */, Domain>` — the single source of truth, ordered so ListDomainNames is deterministic.
- `struct Domain {
    domain_id: String,            // "<account-id>/<DomainName>"
    domain_name: String,
    arn: String,                  // arn:aws:es:<region>:<account>:domain/<name>
    engine_type: EngineType,      // derived from elasticsearch_version prefix
    elasticsearch_version: String,
    created: bool,
    deleted: bool,
    processing: bool,
    upgrade_processing: bool,
    endpoint: Option<String>,     // "search-<name>-<suffix>.<region>.es.amazonaws.com"
    config: DomainConfigRecord,
    tags: Vec<Tag>,               // populated from CreateElasticsearchDomain.TagList
    created_at: SystemTime,
    config_version: u64,          // incremented by UpdateElasticsearchDomainConfig
    last_updated: SystemTime,
  }`
- `struct DomainConfigRecord { cluster_config, ebs_options, snapshot_options, vpc_options, cognito_options, encryption_at_rest_options, node_to_node_encryption_options, advanced_options: BTreeMap<String,String>, access_policies: Option<String>, log_publishing_options, domain_endpoint_options, advanced_security_options, auto_tune_options }` — the merge target for Update and the source for both Describe* responses. Each field keeps its own `update_date`/`update_version`/`state` so `ElasticsearchDomainConfig`'s `*Status` wrappers are generated from stored data rather than fabricated per response.
- `next_id_seq: u64` — monotonic counter feeding DomainId/endpoint suffix generation so IDs are stable and unique within a run.

All other operations (packages, VPC endpoints, cross-cluster search connections, reserved instances, tags, upgrades, Auto-Tune, instance-type limits) stay scaffolded at 501 and will be filed as individual gap issues later.

## Evidence

1) `services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/src/ops/` contains 50 operation files, every one of which still has the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and a `handle` body whose only statement is `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))` (verified in full in `create_elasticsearch_domain.rs`, lines 4 and 202-220) — HTTP 501 for every call, zero persisted state.
2) `OPERATIONS.md` (crate root) grounds the slice: protocol `restJson1`, sdkId `Elasticsearch Service`, signing name `es`; CreateElasticsearchDomain `POST /2015-01-01/es/domain` -> `DomainStatus`, errors incl. `ResourceAlreadyExistsException (409)`; DescribeElasticsearchDomain `GET /2015-01-01/es/domain/{DomainName}` -> `DomainStatus` (required), `ResourceNotFoundException (409)`; DescribeElasticsearchDomainConfig `GET .../config` -> `DomainConfig` (required); UpdateElasticsearchDomainConfig `POST .../config` with `DryRun` -> `DomainConfig` + `DryRunResults`; ListDomainNames `GET /2015-01-01/domain` -> `DomainNames` (list<DomainInfo>); DeleteElasticsearchDomain `DELETE /2015-01-01/es/domain/{DomainName}` -> `DomainStatus`.
3) Smithy model shapes: `ElasticsearchDomainStatus` requires `DomainId`, `DomainName`, `ARN`, `ElasticsearchClusterConfig` and models `Created`/`Deleted`/`Processing`/`Endpoint`; `DomainInfo` carries `DomainName` + `EngineType` — these are the fields the state model must persist to answer truthfully.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. `aws es create-elasticsearch-domain --domain-name my-test-domain --elasticsearch-version 7.10 --elasticsearch-cluster-config InstanceType=t3.small.elasticsearch,InstanceCount=2` — expect 200 with DomainStatus.DomainName=my-test-domain, a non-empty DomainId and ARN ending in `domain/my-test-domain`, Created=true. Today: HTTP 501 NotImplemented.
3. `aws es describe-elasticsearch-domain --domain-name my-test-domain` — expect the same DomainName/DomainId/ARN and the cluster config stored at create time (InstanceCount=2).
4. `aws es list-domain-names` — expect DomainNames to contain {DomainName: my-test-domain, EngineType: Elasticsearch}.
5. `aws es update-elasticsearch-domain-config --domain-name my-test-domain --elasticsearch-cluster-config InstanceType=t3.small.elasticsearch,InstanceCount=3` — expect 200 and DomainConfig.ElasticsearchClusterConfig.Options.InstanceCount=3, with previously set options (EBS, snapshot hour, access policies) unchanged.
6. `aws es describe-elasticsearch-domain-config --domain-name my-test-domain` — expect InstanceCount=3 persisted, and an incremented UpdateVersion on the changed option.
7. `aws es create-elasticsearch-domain --domain-name my-test-domain ...` again — expect HTTP 409 ResourceAlreadyExistsException.
8. `aws es delete-elasticsearch-domain --domain-name my-test-domain` — expect 200 with DomainStatus.Deleted=true.
9. `aws es describe-elasticsearch-domain --domain-name my-test-domain` — expect HTTP 409 ResourceNotFoundException; `aws es list-domain-names` must no longer list it.
10. `aws es describe-elasticsearch-domain --domain-name never-created` — expect HTTP 409 ResourceNotFoundException, not 501 and not an empty 200.
