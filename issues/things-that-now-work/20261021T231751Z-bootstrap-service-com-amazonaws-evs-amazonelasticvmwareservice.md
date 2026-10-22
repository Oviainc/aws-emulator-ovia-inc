Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the Amazon EVS (Elastic VMware Service) emulator

_Filed: 2026-10-21T23:17:51Z_
_Operation: service-com-amazonaws-evs-amazonelasticvmwareservice | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 24 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the service holds no state at all. Implement a FIRST, SMALL, stateful slice: the environment lifecycle of the primary resource (`Environment`), so a realistic client script works end to end over awsJson1_0 (`POST /`, `X-Amz-Target: AmazonElasticVMwareService.<Operation>`).

## Happy path to implement (in order)

1. **CreateEnvironment** — input: `vpcId` (required, `^vpc-[a-f0-9]{8}([a-f0-9]{9})?$`), `serviceAccessSubnetId` (required, `^subnet-...$`), `vcfVersion` (required enum SELF_DEPLOYED | VCF-5.2.1 | VCF-5.2.2), `termsAccepted` (required bool), `initialVlans` (required), plus optional `environmentName`, `clientToken` (@idempotencyToken), `kmsKeyId`, `tags`, `connectivityInfo`, `licenseInfo`, `hosts`, `vcfHostnames`, `siteId`, `serviceAccessSecurityGroups`. Output: `environment` (Environment). Must MINT and PERSIST a new environment: `environmentId` matching `^(env-[a-zA-Z0-9]{10})$`, `environmentArn` matching `^arn:aws:evs:<region>:<account>:environment/<name-or-id>$`, `environmentState` = `CREATING` (may advance to `CREATED`), `createdAt`/`modifiedAt` timestamps, and echo back the stored `vpcId`, `serviceAccessSubnetId`, `vcfVersion`, `environmentName`, `termsAccepted`, `siteId`, `licenseInfo`, `kmsKeyId`. Each entry of `initialVlans` must be materialised into a stored `Vlan` record (vlanId, cidr, functionName, subnetId, vlanState, createdAt). Repeating the same `clientToken` must return the SAME environment, not a second one.
2. **GetEnvironment** — input `environmentId` (required). Must look the environment up in state and return exactly what CreateEnvironment stored (same id, arn, name, vpcId, vcfVersion, tags-independent fields, timestamps). Unknown or already-deleted-and-purged id → `ResourceNotFoundException` (404); an id not matching the `env-` pattern → `ValidationException` (400).
3. **ListEnvironments** — input `nextToken`, `maxResults` (1..100), `state` (list<EnvironmentState>). Output `environmentSummaries` (list<EnvironmentSummary>: environmentId, environmentName, vcfVersion, environmentState, environmentStatus, createdAt, modifiedAt, environmentArn) + `nextToken`. Must enumerate persisted environments, honour `maxResults` as a real page bound with a resumable `nextToken`, and filter by `state` when supplied.
4. **ListEnvironmentVlans** — input `environmentId` (required), `maxResults`, `nextToken`. Output `environmentVlans` (list<Vlan>) + `nextToken`. Must return the VLANs derived from the `initialVlans` persisted at create time; unknown environmentId → `ResourceNotFoundException` (404).
5. **DeleteEnvironment** — input `environmentId` (required), `clientToken`. Output `environment` with `environmentState` transitioned to `DELETING`/`DELETED` and `modifiedAt` updated. After deletion, a subsequent GetEnvironment must reflect the deleted state (or 404 once purged) and the environment must no longer appear in ListEnvironments filtered to active states. Deleting an unknown id → `ResourceNotFoundException` (404).

Optionally round out the slice with **TagResource / ListTagsForResource / UntagResource** on the environment ARN (tags stored on the environment record, unknown ARN → `ResourceNotFoundException`), since `CreateEnvironment` already accepts `tags`.

No canned, hardcoded, or echoed-only responses: every one of these must compute its response from persisted state. A random `environmentId` returned without storing the environment, or a `GetEnvironment` that fabricates a response for any id, is a failure of this issue. Likewise ListEnvironments must not return a fixed list.

## State Model

In `src/state.rs` (`ComAmazonawsEvsAmazonelasticvmwareserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `environments: BTreeMap<String /* environmentId */, StoredEnvironment>`
- `StoredEnvironment { environment_id: String, environment_arn: String, environment_name: Option<String>, state: EnvironmentState, state_details: Option<String>, created_at: Timestamp, modified_at: Timestamp, vpc_id: String, service_access_subnet_id: String, vcf_version: VcfVersion, terms_accepted: bool, kms_key_id: Option<String>, site_id: Option<String>, license_info: Vec<LicenseInfo>, connectivity_info: Option<ConnectivityInfo>, vcf_hostnames: Option<VcfHostnames>, service_access_security_groups: Option<ServiceAccessSecurityGroups>, tags: BTreeMap<String,String>, vlans: Vec<StoredVlan>, hosts: Vec<StoredHost>, connectors: BTreeMap<String /* cnctr-... */, StoredConnector> }`
- `StoredVlan { vlan_id: i32, cidr: String, function_name: String, subnet_id: String, availability_zone: String, vlan_state: VlanState, is_public: bool, created_at: Timestamp, modified_at: Timestamp, eip_associations: Vec<EipAssociation> }` (populated from `initialVlans` at create time; `hosts`/`connectors` reserved for the later host/connector slices)
- `client_tokens: BTreeMap<String /* clientToken */, String /* environmentId */>` for @idempotencyToken replay on CreateEnvironment/DeleteEnvironment
- `next_id_seed: u64` (or equivalent) so `env-XXXXXXXXXX` ids are unique per create

All other operations (CreateEnvironmentHost, DeleteEnvironmentHost, ListEnvironmentHosts, Create/Update/Delete/ListEnvironmentConnectors, Create/DeleteEntitlement, ListVmEntitlements, AssociateEipToVlan, DisassociateEipFromVlan, GetDepotUrl, GetVersions, Get/PutAccountSettings) stay stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md (crate root) documents all 24 bound operations with their input/output members and errors: CreateEnvironment (required vpcId/serviceAccessSubnetId/vcfVersion/termsAccepted/initialVlans → `environment`), GetEnvironment (`environmentId` → `environment`, ResourceNotFoundException 404), ListEnvironments (@paginated inputToken=nextToken, items=environmentSummaries), ListEnvironmentVlans (@paginated, items=environmentVlans, ResourceNotFoundException 404), DeleteEnvironment (`environmentId` → `environment`). README.md documents the wire protocol (awsJson1_0, X-Amz-Target AmazonElasticVMwareService.<Op>) and that each generated stub answers 501 until written. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 24 operation files, confirming zero implemented behavior. Smithy model shapes Environment (environmentId `^(env-[a-zA-Z0-9]{10})$`, environmentArn, environmentState, createdAt/modifiedAt), EnvironmentSummary, EnvironmentState (CREATING|CREATED|DELETING|DELETED|CREATE_FAILED) and Vlan ground the field names and state values above. No tests exercise these operations (only the scaffold's tests/state_roundtrip.rs).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `evs` client at it.
2. CreateEnvironment with vpcId=vpc-0123456789abcdef0, serviceAccessSubnetId=subnet-0123456789abcdef0, vcfVersion=VCF-5.2.1, termsAccepted=true, environmentName=demo-env, initialVlans={...}, clientToken=tok-1 — expect 200 with environment.environmentId matching ^env-[a-zA-Z0-9]{10}$, environmentArn set, environmentState=CREATING/CREATED (today: HTTP 501 NotImplemented).
3. Repeat the identical CreateEnvironment with clientToken=tok-1 — expect the same environmentId returned, not a new environment.
4. GetEnvironment with the returned environmentId — expect 200 and the same environmentName/vpcId/serviceAccessSubnetId/vcfVersion/arn/createdAt that create returned.
5. GetEnvironment with environmentId=env-0000000000 (never created) — expect ResourceNotFoundException (HTTP 404).
6. ListEnvironments with maxResults=1 after creating two environments — expect one summary plus a nextToken, and the second page returns the other environment with no duplicates.
7. ListEnvironmentVlans for the environmentId — expect the VLANs derived from the initialVlans supplied at create time.
8. DeleteEnvironment with the environmentId — expect 200 with environmentState=DELETING/DELETED and modifiedAt updated.
9. GetEnvironment / ListEnvironments again — expect the environment to be gone (404) or reported as DELETED, proving state is real and persistent across calls.
