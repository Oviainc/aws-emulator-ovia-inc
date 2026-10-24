Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# UpdateAccountConfiguration answers 404 until the account has created an upload URL or a scan

_Filed: 2026-10-24T02:28:10Z_
_Operation: UpdateAccountConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: UpdateAccountConfiguration sets the account's encryption configuration and answers 200 with the resulting encryptionConfig. Configuring a customer-managed KMS key is the documented FIRST step a new account takes (so that subsequent scans are encrypted with its own key); it does not require any pre-existing scan or upload.

Actual: the handler refuses with ResourceNotFoundException (HTTP 404) whenever `store.account_configuration` is None (src/ops/update_account_configuration.rs lines 193-210). That record is created only as a side effect of CreateUploadUrl or CreateScan (Store::onboard_account), an "onboarding" gate the model does not express. So on a fresh emulator, PUT /updateAccountConfiguration with a valid kmsKeyArn returns 404 and stores nothing, and the only way to make it work is to mint an upload URL first. The behavior is also internally inconsistent: GET /accountConfiguration/get answers 200 with an empty encryptionConfig for exactly the same account state, so there is nothing "not found" about the resource being updated.

## Evidence

Smithy model: UpdateAccountConfiguration input/output are just the required encryptionConfig; nothing in the model or in UpdateAccountConfigurationRequest$encryptionConfig docs conditions success on a pre-existing resource, and GetAccountConfiguration is documented to report the AWS-managed-key default for an account that has configured nothing. Handler: src/ops/update_account_configuration.rs lines 185-211 plus src/state.rs Store::onboard_account (called only from create_upload_url.rs:246 and create_scan.rs:405). Tests ASSERT the divergent 404 (tests/all/account_configuration.rs:88 updating_the_configuration_of_an_account_that_has_none_is_not_found), so correct behavior is uncovered.

## Steps to Reproduce

1. Start a fresh emulator (no CodeGuru Security calls yet)
2. GET /accountConfiguration/get -> 200 with an empty encryptionConfig
3. PUT /updateAccountConfiguration with {"encryptionConfig":{"kmsKeyArn":"arn:aws:kms:us-east-1:123456789012:key/<uuid>"}}
4. Observe 404 ResourceNotFoundException instead of 200 with the key applied; re-run after POST /uploadUrl and the same request now succeeds
