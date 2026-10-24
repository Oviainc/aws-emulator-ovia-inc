Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# StartInstanceOnboardingJob onboards any instance id, never checking that the Connect instance exists

_Filed: 2026-10-24T07:51:25Z_
_Operation: StartInstanceOnboardingJob | Class: validation_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler takes the `connectInstanceId` straight from the request and writes an onboarding record for it, so any string inside `InstanceId`'s pattern becomes an "onboarded" Connect instance: `GetInstanceOnboardingJobStatus` then reports IN_PROGRESS/SUCCEEDED for it, `GetConnectInstanceConfig` projects a config with a service-linked role for it, and `CreateCampaign` happily mints campaigns against an instance that exists in no Amazon Connect account.

AWS onboards an EXISTING Connect instance (onboarding provisions the Campaigns service-linked role and the encryption config for that instance). A `connectInstanceId` that names no instance in the caller's account is refused with the modeled `ResourceNotFoundException` (HTTP 404) and nothing is persisted. Expected: HTTP 404 `ResourceNotFoundException` for an unknown instance id, with no onboarding record created (so the follow-up GetInstanceOnboardingJobStatus is also a 404 and CreateCampaign stays refused). Actual: HTTP 200 with `connectInstanceOnboardingJobStatus.status` = IN_PROGRESS for an arbitrary id.

The emulator already has the state needed: the Amazon Connect crate keeps the instance registry (`services/service-com-amazonaws-connect-amazonconnectservice/src/state.rs`, `instances: BTreeMap<String, InstanceRecord>` written by CreateInstance), and cross-crate state reads of exactly this kind are an established pattern in this repo (e.g. amplifybackend's list_s3_buckets reads `service_com_amazonaws_s3_amazons3::state`, bcmdataexports reads the IAM crate's state).

## Evidence

Spec: `StartInstanceOnboardingJob` (PUT /v2/connect-instance/{connectInstanceId}/onboarding) models `ResourceNotFoundException` (HTTP 404, "The specified resource was not found") — the only resource the request names is the Connect instance, so that error is the instance-existence refusal. The same rule is already recorded for the neighbouring operation: the fixed issue 20261023T034742Z-connectcampaignsv2-createcampaign-skips-the-connect-instance-existence-check quotes AWS's recorded 404 "Connect Instance does not exist or is not enabled to use Campaigns." for a nonexistent instance id, and the emulator's own CreateCampaign now gates on onboarding — but onboarding itself accepts anything, so the gate can be satisfied for an instance that does not exist.

Handler: src/ops/start_instance_onboarding_job.rs:197-270 — validates only `encryptionConfig` (check_encryption_config), then calls `store.start_instance_onboarding(&input.connect_instance_id, ...)`; the only other refusal is the store-ceiling ConflictException. src/state.rs `start_instance_onboarding` inserts a record for whatever id it is handed.

Test coverage: no test asserts an existence check — tests/all/instance_onboarding.rs and instance_onboarding_tags_and_outbound_requests.rs onboard bare ids such as "instance-1" / "pending-1" and assert the 200, so the missing 404 class is untested.

## Steps to Reproduce

1. Boot the emulator with the connectcampaignsv2 crate loaded (no Connect instance created).
2. PUT /v2/connect-instance/00000000-0000-0000-0000-000000000000/onboarding with {"encryptionConfig":{"enabled":false}} -> observe HTTP 200 and connectInstanceOnboardingJobStatus.status=IN_PROGRESS; expected HTTP 404 ResourceNotFoundException because no Connect instance with that id exists.
3. GET /v2/connect-instance/00000000-0000-0000-0000-000000000000/onboarding -> observe a stored job (SUCCEEDED after the job span); expected HTTP 404 because step 2 must persist nothing.
4. PUT /v2/campaigns with {"name":"x","connectInstanceId":"00000000-0000-0000-0000-000000000000"} -> observe HTTP 200 and a minted campaign for a nonexistent instance; expected HTTP 404 ResourceNotFoundException.
