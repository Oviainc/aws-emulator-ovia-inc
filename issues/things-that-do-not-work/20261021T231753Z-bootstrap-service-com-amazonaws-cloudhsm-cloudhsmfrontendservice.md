Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the CloudHSM (CloudHsmFrontendService) emulator

_Filed: 2026-10-21T23:17:53Z_
_Operation: service-com-amazonaws-cloudhsm-cloudhsmfrontendservice | Class: stub_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 20 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client can do anything with this emulator. Implement a FIRST, SMALL slice: the lifecycle of the primary resource, the HSM, plus the list view that clients use to discover it.

Happy path to implement, in order (awsJson1_1, `POST /` with `X-Amz-Target: CloudHsmFrontendService.<Op>`):

1. **CreateHsm** — input `SubnetId` (required, `^subnet-[0-9a-f]{8}$`), `SshKey` (required), `IamRoleArn` (required), `SubscriptionType` (required, enum `PRODUCTION`), optional `EniIp`, `ExternalId`, `ClientToken`, `SyslogIp`. Output `HsmArn` matching `^arn:aws(-iso)?:cloudhsm:<region>:<account12>:hsm-[0-9a-f]{8}$`. Must mint a fresh random 8-hex id, persist an Hsm record carrying EVERY input field, and record creation time. If `ClientToken` is supplied and matches a prior create, return the SAME `HsmArn` instead of a second HSM (idempotency).
2. **DescribeHsm** — input `HsmArn` OR `HsmSerialNumber` (both optional in the model; at least one must resolve). Output must be computed from the stored record: `HsmArn`, `Status` (`HsmStatus`: start `PENDING`, then `RUNNING`), `AvailabilityZone`, `EniId` (`^eni-[0-9a-f]{8}$`, minted at create), `EniIp`, `SubscriptionType`, `SubscriptionStartDate` (epoch-seconds digits), `VpcId` (`^vpc-[0-9a-f]{8}$`), `SubnetId`, `IamRoleArn`, `SerialNumber`, `SshPublicKey` (the `SshKey` given at create), `VendorName`, `HsmType`, `SoftwareVersion`, `Partitions`. An unknown ARN must raise `CloudHsmServiceException` (HTTP 400), not an empty 200.
3. **ListHsms** — output `HsmList` containing exactly the ARNs of the HSMs currently persisted (empty list when none), with `NextToken` echoed/emitted only when the stored set is actually paged. The list must shrink after DeleteHsm.
4. **ModifyHsm** — input `HsmArn` (required) plus optional `SubnetId`, `EniIp`, `IamRoleArn`, `ExternalId`, `SyslogIp`. Must mutate ONLY the supplied members on the stored record (absent members left untouched) and return the same `HsmArn`; unknown ARN → `CloudHsmServiceException` (400). A follow-up DescribeHsm must show the new values.
5. **DeleteHsm** — input `HsmArn` (required). Must remove the record from state and return `Status` (required output member, e.g. `"DELETED"`). A second DeleteHsm or a DescribeHsm on the deleted ARN must fail with `CloudHsmServiceException` (400), and the ARN must be gone from ListHsms.

No canned, echoed or hardcoded responses: each of these five handlers must read/write the shared store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"), so that a create is visible to a later describe/list and a delete is visible as absence. A random ARN returned without a stored record is a failure, as is a DescribeHsm that merely echoes its input.

Input validation that the five must enforce (patterns come from the model): `SubnetId`, `IamRoleArn`, `EniIp`/`SyslogIp` (dotted quad), `SubscriptionType` enum membership, and ARN shape on the Describe/Modify/Delete paths — a malformed value is `InvalidRequestException` (HTTP 400), a well-formed-but-unknown ARN is `CloudHsmServiceException` (HTTP 400).

## State Model

In `src/state.rs`, on the service state struct:

- `hsms: BTreeMap<String /* HsmArn */, Hsm>` — insertion-ordered-by-key store of HSMs.
- `struct Hsm { hsm_arn: String, hsm_id: String /* 8 hex */, serial_number: String /* digits, <=16 */, status: HsmStatus, status_details: Option<String>, availability_zone: String, eni_id: String /* eni-<8hex> */, eni_ip: Option<String>, subnet_id: String, vpc_id: String, iam_role_arn: String, external_id: Option<String>, syslog_ip: Option<String>, ssh_public_key: String, ssh_key_last_updated: String /* epoch secs */, subscription_type: SubscriptionType, subscription_start_date: String /* epoch secs */, subscription_end_date: Option<String>, vendor_name: String, hsm_type: String, software_version: String, server_cert_uri: Option<String>, server_cert_last_updated: Option<String>, partitions: Vec<String> }`
- `hsm_client_tokens: HashMap<String /* ClientToken */, String /* HsmArn */>` — for CreateHsm idempotency.
- `next_hsm_seq: u64` — monotonic counter used to derive deterministic-but-unique hsm/eni ids and serial numbers.

The remaining 15 operations (CreateHapg/DescribeHapg/ModifyHapg/DeleteHapg/ListHapgs, the LunaClient family, GetConfig, ListAvailableZones, and the tagging trio) stay at 501 for now and will be filed as individual gap issues; leave room for `hapgs`, `luna_clients` and `tags` maps in the same state struct when those land.

## Evidence

OPERATIONS.md (crate root) lists all 20 operations with the input/output members and errors quoted above: CreateHsm (required SubnetId/SshKey/IamRoleArn/SubscriptionType → HsmArn), DescribeHsm (HsmArn|HsmSerialNumber → ~22 computed output members incl. Status: HsmStatus DEGRADED|PENDING|RUNNING|SUSPENDED|TERMINATED|TERMINATING|UPDATING), ListHsms (NextToken → HsmList/NextToken), ModifyHsm (HsmArn required + optional mutable fields → HsmArn), DeleteHsm (HsmArn → required Status); all declare CloudHsmInternalException (500), CloudHsmServiceException (400), InvalidRequestException (400). README.md documents the awsJson1_1 wire protocol, the 501 scaffold contract, and the shared-engine state API. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 20 handler files (create_hsm.rs, describe_hsm.rs, list_hsms.rs, modify_hsm.rs, delete_hsm.rs included), proving no handler computes anything from state today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI CloudHSM (classic) client at it.
2. CreateHsm with SubnetId=subnet-0123abcd, SshKey=<ssh-rsa ...>, IamRoleArn=arn:aws:iam::123456789012:role/CloudHsmRole, SubscriptionType=PRODUCTION → expect 200 with an HsmArn of the form arn:aws:cloudhsm:<region>:123456789012:hsm-<8hex>; today 501 NotImplemented.
3. Repeat the same CreateHsm with an identical ClientToken → expect the SAME HsmArn, not a second HSM.
4. DescribeHsm with that HsmArn → expect the stored SubnetId, IamRoleArn, SshPublicKey, EniId/EniIp, SubscriptionType and a Status of PENDING/RUNNING, all from state.
5. ListHsms → expect HsmList to contain exactly that HsmArn.
6. ModifyHsm with that HsmArn and SyslogIp=10.0.0.9 (no other members) → expect 200; DescribeHsm again → SyslogIp updated, every other field unchanged.
7. DeleteHsm with that HsmArn → expect 200 with Status set; ListHsms → empty HsmList; DescribeHsm on the same ARN → CloudHsmServiceException with HTTP 400.
8. DescribeHsm with HsmArn=arn:aws:cloudhsm:us-east-1:123456789012:hsm-deadbeef (never created) → CloudHsmServiceException 400; CreateHsm with SubnetId="not-a-subnet" → InvalidRequestException 400.
