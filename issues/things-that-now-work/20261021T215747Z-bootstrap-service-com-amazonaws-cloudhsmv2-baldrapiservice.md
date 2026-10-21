Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the CloudHSM V2 emulator

_Filed: 2026-10-21T21:57:47Z_
_Operation: service-com-amazonaws-cloudhsmv2-baldrapiservice | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 18 operations in `services/service-com-amazonaws-cloudhsmv2-baldrapiservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST SMALL SLICE: the cluster/HSM lifecycle, statefully, so a realistic client script runs end to end.

Wire protocol: `awsJson1_1`, `POST /` with `X-Amz-Target: BaldrApiService.<OperationName>` (per README/OPERATIONS.md), sigv4 signing name `cloudhsm`.

### Happy path to implement (in order)

1. **CreateCluster** — input `HsmType` (required, e.g. `hsm1.medium`), `SubnetIds` (required, 1..10), optional `BackupRetentionPolicy`, `NetworkType`, `Mode`, `SourceBackupId`, `TagList`. Must mint a fresh id matching `^cluster-[2-7a-zA-Z]{11,16}$`, persist the cluster record, and return `Cluster` with at minimum `ClusterId`, `HsmType`, `State=CREATE_IN_PROGRESS` (or `UNINITIALIZED`), `CreateTimestamp`, `SubnetMapping` (derived from the requested `SubnetIds`), `BackupPolicy`, `BackupRetentionPolicy`, `Mode`, `NetworkType`, `Hsms=[]`, `TagList`.
2. **DescribeClusters** — input `Filters` (map, e.g. `clusterIds`), `MaxResults` (1..25), `NextToken`. Must return the clusters actually stored by CreateCluster — including the current `State` and the live `Hsms` list — honouring `MaxResults` and round-tripping `NextToken`. Never a canned/empty list.
3. **CreateHsm** — input `ClusterId` (required), `AvailabilityZone` (required), optional `IpAddress`. Must look the cluster up in state (unknown id → `CloudHsmResourceNotFoundException`, HTTP 400), mint `^hsm-[2-7a-zA-Z]{11,16}$`, synthesize `EniId` (`eni-…`), `EniIp`, `SubnetId` from the cluster's subnet mapping for that AZ, `State=CREATE_IN_PROGRESS`/`ACTIVE`, attach the HSM to the cluster's `Hsms` list, and return `Hsm`.
4. **DeleteHsm** — input `ClusterId` (required) plus exactly one of `HsmId`/`EniId`/`EniIp`. Must resolve the HSM by whichever selector was given, remove it from the cluster's `Hsms`, and return the deleted `HsmId`; unknown cluster/HSM → `CloudHsmResourceNotFoundException`.
5. **DeleteCluster** — input `ClusterId` (required). Must transition the stored cluster to `DELETE_IN_PROGRESS`/`DELETED` and return the `Cluster` as it was, so a following DescribeClusters no longer lists it as active; unknown id → `CloudHsmResourceNotFoundException`.

Every response must be computed from persisted state: no hardcoded ids, no echoing the request back, no "generate an id and forget it". A CreateCluster followed by DescribeClusters must return exactly what was created; CreateHsm must become visible inside that cluster's `Hsms`; after DeleteHsm the HSM must be gone; after DeleteCluster the cluster must be gone.

### State Model

In `src/state.rs` (`ComAmazonawsCloudhsmv2BaldrapiserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `clusters: HashMap<String /*ClusterId*/, ClusterRecord>`
- `ClusterRecord { cluster_id, hsm_type, state: ClusterState, state_message: Option<String>, create_timestamp: SystemTime, subnet_mapping: HashMap<az, subnet_id>, vpc_id: String, security_group: String, backup_policy: BackupPolicy, backup_retention_policy: Option<BackupRetentionPolicy>, mode: Option<ClusterMode>, network_type: Option<NetworkType>, source_backup_id: Option<String>, pre_co_password: Option<String>, certificates: Option<Certificates>, hsms: Vec<HsmRecord>, tags: Vec<Tag> }`
- `HsmRecord { hsm_id, cluster_id, availability_zone, subnet_id, eni_id, eni_ip, hsm_type, state: HsmState, state_message: Option<String> }`
- counters/helpers for id minting that satisfy the modeled patterns (`cluster-`, `hsm-`, `eni-`, `backup-`) and a `next_token` scheme for the paginated describes.

Operations outside this slice (backups, tags, resource policies, InitializeCluster, ModifyCluster, …) stay 501 and are filed as individual gap issues later.

## Evidence

All 18 handler files under `services/service-com-amazonaws-cloudhsmv2-baldrapiservice/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) — confirmed by grep over `src/ops/`, including create_cluster.rs, describe_clusters.rs, create_hsm.rs, delete_hsm.rs, delete_cluster.rs. Contracts cited from the crate's OPERATIONS.md (CreateCluster/CreateHsm/DeleteHsm/DeleteCluster/DescribeClusters sections: input members, required flags, id patterns, MaxResults ranges, error list incl. CloudHsmResourceNotFoundException HTTP 400) and from the Smithy model shapes `Cluster`, `Hsm`, `BackupRetentionPolicy` (output member names and types). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI cloudhsmv2 client at it.
2. aws cloudhsmv2 create-cluster --hsm-type hsm1.medium --subnet-ids subnet-0123456789abcdef0 → expect 200 with a Cluster containing a fresh cluster-XXXXXXXXXXX id and the requested HsmType/SubnetMapping; today: HTTP 501 NotImplemented.
3. aws cloudhsmv2 describe-clusters → expect the just-created cluster listed with its real state and empty Hsms.
4. aws cloudhsmv2 create-hsm --cluster-id <ClusterId> --availability-zone us-east-1a → expect 200 with an Hsm (hsm- id, EniId, EniIp, SubnetId).
5. aws cloudhsmv2 describe-clusters --filters clusterIds=<ClusterId> → expect the cluster's Hsms list to contain that HSM.
6. aws cloudhsmv2 delete-hsm --cluster-id <ClusterId> --hsm-id <HsmId> → expect 200 returning HsmId; re-describe shows Hsms empty.
7. aws cloudhsmv2 delete-cluster --cluster-id <ClusterId> → expect 200 with the cluster in DELETE_IN_PROGRESS/DELETED; a later describe-clusters no longer lists it as active.
8. aws cloudhsmv2 create-hsm --cluster-id cluster-aaaaaaaaaaaa --availability-zone us-east-1a (unknown cluster) → expect CloudHsmResourceNotFoundException with HTTP 400.
