Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the AmazonOpenSearchService emulator

_Filed: 2026-10-21T23:13:05Z_
_Operation: service-com-amazonaws-opensearch-amazonopensearchservice | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 96 operations in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Scaffolded::not_implemented(..)` (HTTP 501). No state is held, so no client workflow works at all.

Implement a FIRST minimal slice: the stateful domain lifecycle, which is the primary resource of OpenSearch Service (restJson1, signing name `es`, API version path prefix `/2021-01-01`). Scope is exactly these six operations, in this order:

1. **CreateDomain** — `POST /2021-01-01/opensearch/domain`. Input: `DomainName` (required, len 3..28, `^[a-z][a-z0-9\-]+$`), `EngineVersion`, `ClusterConfig`, `EBSOptions`, `AccessPolicies`, `IPAddressType`, `SnapshotOptions`, `EncryptionAtRestOptions`, `NodeToNodeEncryptionOptions`, `AdvancedOptions`, `DomainEndpointOptions`, `AdvancedSecurityOptions`, `TagList`, `AutoTuneOptions`, `SoftwareUpdateOptions`, `UseCase`, `EngineMode`. Must persist a new domain record keyed by name and return `DomainStatus` built from it (required members `DomainId`, `DomainName`, `ARN`, `ClusterConfig`; plus `Created=true`, `Deleted=false`, `Processing=true`, `EngineVersion`, `Endpoint`). Creating a name that already exists must fail with `ResourceAlreadyExistsException` (HTTP 409); a name violating the length/pattern constraint must fail with `ValidationException` (HTTP 400).
2. **DescribeDomain** — `GET /2021-01-01/opensearch/domain/{DomainName}`. Must return the *stored* `DomainStatus` (required output member), echoing back exactly what CreateDomain persisted (same `DomainId`, `ARN`, `ClusterConfig`, `EngineVersion`, tags). Unknown name → `ResourceNotFoundException` (HTTP 409).
3. **DescribeDomainConfig** — `GET /2021-01-01/opensearch/domain/{DomainName}/config`. Must return `DomainConfig` (required) derived from the same stored record, with each `*Status` wrapper (`EngineVersion`, `ClusterConfig`, `EBSOptions`, `AccessPolicies`, `AdvancedOptions`, `SnapshotOptions`, ...) carrying the persisted value and an `OptionStatus` whose `CreationDate`/`UpdateDate`/`UpdateVersion`/`State` reflect the record's real create/update history. Unknown name → `ResourceNotFoundException`.
4. **ListDomainNames** — `GET /2021-01-01/domain`. Must return `DomainNames` as one `DomainInfo{DomainName, EngineType}` per *persisted, not-yet-deleted* domain, with `EngineType` derived from the stored `EngineVersion` prefix (`OpenSearch_*` → `OpenSearch`, `Elasticsearch_*` → `Elasticsearch`), and must honour the `engineType` query filter.
5. **UpdateDomainConfig** — `POST /2021-01-01/opensearch/domain/{DomainName}/config`. Must merge the supplied option containers (`ClusterConfig`, `EBSOptions`, `AccessPolicies`, `AdvancedOptions`, `LogPublishingOptions`, `SnapshotOptions`, `IPAddressType`, ...) into the stored record, bump `UpdateVersion`/`UpdateDate` on the touched options, and return the updated `DomainConfig`. A later DescribeDomain/DescribeDomainConfig must observe the change. `DryRun=true` must NOT mutate stored state. Unknown name → `ResourceNotFoundException`.
6. **DeleteDomain** — `DELETE /2021-01-01/opensearch/domain/{DomainName}`. Must remove the record and return the final `DomainStatus` with `Deleted=true`; afterwards DescribeDomain must return `ResourceNotFoundException` and the name must be gone from ListDomainNames. Deleting an unknown name → `ResourceNotFoundException`.

NEVER STUBS: each of these six must compute its response entirely from the persisted record. Hardcoded `DomainStatus` literals, echoing the input back without storing it, or minting an ARN/DomainId that is never saved are failures of this issue — a create followed by a describe must return what was created, and a delete must be observable by the two read paths.

## State Model

In `src/state.rs`, on `ComAmazonawsOpensearchAmazonopensearchserviceState`:

- `domains: BTreeMap<String /* DomainName */, Domain>` — insertion-stable registry of domains for the emulated account/region.
- `struct Domain {
    domain_id: String,        // "<account-id>/<domain-name>"
    domain_name: String,
    arn: String,              // arn:aws:es:<region>:<account>:domain/<domain-name>
    engine_version: String,   // default "OpenSearch_2.11" when input omits it
    engine_mode: Option<EngineMode>,
    use_case: Option<DomainUseCase>,
    endpoint: String,         // "<domain-name>-<suffix>.<region>.es.amazonaws.com"
    created: bool,
    deleted: bool,
    processing: bool,
    cluster_config: ClusterConfig,
    ebs_options: Option<EBSOptions>,
    access_policies: Option<String>,
    ip_address_type: Option<IPAddressType>,
    snapshot_options: Option<SnapshotOptions>,
    encryption_at_rest_options: Option<EncryptionAtRestOptions>,
    node_to_node_encryption_options: Option<NodeToNodeEncryptionOptions>,
    advanced_options: BTreeMap<String, String>,
    log_publishing_options: BTreeMap<LogType, LogPublishingOption>,
    domain_endpoint_options: Option<DomainEndpointOptions>,
    advanced_security_options: Option<AdvancedSecurityOptions>,
    auto_tune_options: Option<AutoTuneOptionsOutput>,
    software_update_options: Option<SoftwareUpdateOptions>,
    tags: Vec<Tag>,
    created_at: SystemTime,
    updated_at: SystemTime,
    option_versions: BTreeMap<String /* option name */, u64 /* UpdateVersion */>,
  }`

`option_versions` plus `created_at`/`updated_at` are what let DescribeDomainConfig/UpdateDomainConfig produce truthful `OptionStatus` values instead of constants. All of this lives in the single shared engine reached via `crate::state::service_state(&*self.engine)` and must be mutated through the lock guard (never a freshly constructed engine).

The other 90 operations (packages, VPC endpoints, cross-cluster connections, data sources, applications, reserved instances, insights, maintenance, migrations, upgrades, tagging) stay at HTTP 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) documents the wire contract used above: `## CreateDomain` (`POST /2021-01-01/opensearch/domain`, required `DomainName` len 3..28 pattern `^[a-z][a-z0-9\-]+$`, output `DomainStatus`, errors incl. `ResourceAlreadyExistsException (HTTP 409)`/`ValidationException (HTTP 400)`); `## DescribeDomain` (`GET .../domain/{DomainName}`, required output `DomainStatus`, `ResourceNotFoundException (HTTP 409)`); `## DescribeDomainConfig` (`GET .../domain/{DomainName}/config`, required output `DomainConfig`); `## ListDomainNames` (`GET /2021-01-01/domain`, `EngineType` @httpQuery(engineType), output `DomainNames` list<DomainInfo>); `## UpdateDomainConfig` (`POST .../domain/{DomainName}/config`, option containers + `DryRun`, required output `DomainConfig`); `## DeleteDomain` (`DELETE .../domain/{DomainName}`, output `DomainStatus`). smithy_query on `DomainStatus` confirms `DomainId`, `DomainName`, `ARN`, `ClusterConfig` are @required and that `Created`/`Deleted`/`Processing` carry creation/deletion state; `DomainInfo` confirms `{DomainName, EngineType}`; `DomainConfig` confirms every option is wrapped in a `*Status` container. All handlers are scaffolds: `src/ops/create_domain.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented``, and README.md documents that marker as the stub census for this crate. No tests exist beyond `tests/state_roundtrip.rs`, the generated state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `opensearch` client at it.
2. `CreateDomain` with DomainName=`my-domain`, EngineVersion=`OpenSearch_2.11`, ClusterConfig={InstanceType: r6g.large.search, InstanceCount: 2}, EBSOptions={EBSEnabled: true, VolumeType: gp3, VolumeSize: 20} — expect 200 with DomainStatus{DomainName: my-domain, non-empty DomainId and ARN, Created: true, the ClusterConfig just sent}; today: HTTP 501 NotImplemented.
3. `CreateDomain` again with the same name — expect HTTP 409 ResourceAlreadyExistsException.
4. `DescribeDomain` DomainName=`my-domain` — expect the same DomainId/ARN/ClusterConfig/EngineVersion that CreateDomain returned.
5. `ListDomainNames` — expect DomainNames to contain {DomainName: my-domain, EngineType: OpenSearch}; with engineType=Elasticsearch expect it to be absent.
6. `UpdateDomainConfig` DomainName=`my-domain`, ClusterConfig={InstanceCount: 4} — expect 200 and DomainConfig.ClusterConfig.Options.InstanceCount == 4 with a bumped Status.UpdateVersion.
7. `DescribeDomainConfig` DomainName=`my-domain` — expect InstanceCount 4 persisted from the previous step (and re-running UpdateDomainConfig with DryRun=true must leave it at 4).
8. `DeleteDomain` DomainName=`my-domain` — expect 200 with DomainStatus.Deleted == true.
9. `DescribeDomain` DomainName=`my-domain` — expect HTTP 409 ResourceNotFoundException; `ListDomainNames` must no longer list it.
10. `DescribeDomain` DomainName=`never-created` — expect HTTP 409 ResourceNotFoundException, not 501.
