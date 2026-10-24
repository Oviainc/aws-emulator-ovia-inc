Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# DescribeInstance omits EncryptionConfigurationDetails until UpdateInstance sets one, instead of reporting the AWS owned key default

_Filed: 2026-10-24T15:19:22Z_
_Operation: DescribeInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: every IAM Identity Center instance is encrypted at rest, and the default is an AWS owned KMS key — `KmsKeyType` is `AWS_OWNED_KMS_KEY | CUSTOMER_MANAGED_KEY` and `EncryptionConfigurationDetails` is documented as "The encryption configuration of your IAM Identity Center instance, including the key type, KMS key ARN, and current encryption status". An instance nobody has called `UpdateInstance` on therefore describes as `EncryptionConfigurationDetails: {"KeyType":"AWS_OWNED_KMS_KEY","EncryptionStatus":"ENABLED"}` (no `KmsKeyArn`, since no customer ARN names an AWS owned key, and no `EncryptionStatusReason`, since nothing failed). There is no such thing as an Identity Center instance with no encryption configuration.

Actual: the member is reported ONLY when a prior `UpdateInstance` carried an `EncryptionConfiguration`; on the account's ambient instance, and on every freshly created instance, `DescribeInstance` omits `EncryptionConfigurationDetails` entirely. A client that reads the instance's key type to decide whether to migrate to a customer managed key (or that asserts encryption is enabled) sees a null where AWS reports the AWS owned default, and after one `UpdateInstance` with `KeyType: AWS_OWNED_KMS_KEY` the same instance suddenly starts reporting the member — a state transition AWS does not have, because the default was already that.

## Evidence

Spec/model: `smithy_query(query="shape", shape="EncryptionConfigurationDetails")` — members `KeyType` (`KmsKeyType`), `KmsKeyArn`, `EncryptionStatus` (`KmsKeyStatus`: `UPDATING | ENABLED | UPDATE_FAILED`), `EncryptionStatusReason`; shape doc: "The encryption configuration of your IAM Identity Center instance...". `EncryptionConfiguration$KmsKeyArn` doc: "Cannot be specified when KeyType is AWS_OWNED_KMS_KEY" — i.e. the AWS owned key is a complete configuration without an ARN, which is what the default state is. Well-established service behaviour: IAM Identity Center encrypts customer data at rest by default with an AWS owned key; the customer managed key option is an opt-in on top of that default, so the default configuration is always present on the instance.

Handler: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/describe_instance.rs lines 241-248 — `encryption_configuration_details: instance.encryption.as_ref().map(|encryption| encryption.to_shape(now))`, with the comment "An instance nothing has configured reports no details rather than an invented key type or ARN." The record is only ever populated by `UpdateInstance` (src/ops/update_instance.rs lines 303-309), and `crate::state::Instance::active` / `::creating` leave `encryption` as `None`; `EncryptionConfigurationRecord` (src/state.rs lines 545-598) derives `UPDATING` -> `ENABLED` from the stored due instant, so an `ENABLED` AWS-owned default is expressible with the structures already there.

Test coverage: the current (omitting) behaviour is pinned rather than the AWS default — tests/all/instance_describe.rs lines 221-228 assert `body["EncryptionConfigurationDetails"] == Value::Null` for the account's instance, so that expectation changes with the handler. tests/all/instance_update.rs lines 320-352 already cover the post-update `UPDATING` -> `ENABLED` reporting and are unaffected.

## Steps to Reproduce

1. Start the emulator and POST / with `X-Amz-Target: SWBExternalService.ListInstances` to read the account's instance ARN.
2. POST / with `X-Amz-Target: SWBExternalService.DescribeInstance` and body `{"InstanceArn":"<instance>"}`.
3. Observe the 200 response carries no `EncryptionConfigurationDetails`, where AWS reports `{"KeyType":"AWS_OWNED_KMS_KEY","EncryptionStatus":"ENABLED"}` for an instance that has never been given a customer managed key.
4. Call `X-Amz-Target: SWBExternalService.CreateInstance` with `{"Name":"Fresh"}` in a fresh process and describe the created instance: the member is likewise absent, so no instance in this emulator ever reports its encryption state until a caller writes one.
5. POST `UpdateInstance` with `{"InstanceArn":"<instance>","EncryptionConfiguration":{"KeyType":"AWS_OWNED_KMS_KEY"}}` and describe again: the member now appears, showing the member's presence is driven by the write rather than by the instance's real (always-encrypted) state.
