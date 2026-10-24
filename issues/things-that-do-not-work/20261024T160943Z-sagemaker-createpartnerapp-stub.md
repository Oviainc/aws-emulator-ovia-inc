Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.partner-app-presigned-url-expires-after-ttl
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.partner-app-presigned-url-expires-after-ttl|_create_partner_app_expect_marketplace_rejection#5|AssertionError|raise AssertionError(
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Implement sagemaker CreatePartnerApp: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePartnerApp

## Description
`CreatePartnerApp` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_partner_app.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreatePartnerApp` call, so each one unblocks only when `CreatePartnerApp` works: `sagemaker.partner-app-presigned-url-expires-after-ttl`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_partner_app.rs`. Replaying the corpus, the emulator refused the `CreatePartnerApp` call: File "check_sagemaker_partner_app_presigned_url_expires_after_ttl.py", line 36, in run
    code, message = _create_partner_app_expect_marketplace_rejection(sm, role_arn, app_name)
                    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_sagemaker_partner_app_presigned_url_expires_after_ttl.py", line 71, in _create_partner_app_expect_marketplace_rejection
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: CreatePartnerApp never reached the expected marketplace-subscription ValidationException after 4 attempts (last: 'NotImplemented' 'operation CreatePartnerApp is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sagemaker emulator
Call CreatePartnerApp and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
