Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.default-application-setting-managed opensearch.representation-default-application-setting-empty-vs-absent-preserved
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|GetDefaultApplicationSetting|NotImplemented
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Implement opensearch GetDefaultApplicationSetting: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDefaultApplicationSetting

## Description
`GetDefaultApplicationSetting` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/get_default_application_setting.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetDefaultApplicationSetting` call, so each one unblocks only when `GetDefaultApplicationSetting` works: `opensearch.default-application-setting-managed`, `opensearch.representation-default-application-setting-empty-vs-absent-preserved`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/get_default_application_setting.rs`. Replaying the corpus, the emulator refused the `GetDefaultApplicationSetting` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetDefaultApplicationSetting operation: operation GetDefaultApplicationSetting is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the opensearch emulator
Call GetDefaultApplicationSetting and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution
`GetDefaultApplicationSetting` is implemented: it reports the account-scoped
default-application setting the service's state now holds
(`default_application_arn`), and refuses with the modelled
`ResourceNotFoundException` (409) while no default is set, as the operation
documents. It is a pure read.

Its writer, `PutDefaultApplicationSetting`, was a stub with no issue of its own and
is implemented in the same change: `setAsDefault` true points the setting at the
ARN in the request, false clears it when that ARN is the one in effect, and either
way the answer reports the setting in effect after the call — so a cleared setting
answers with no `applicationArn` member at all. The ARN is checked for ARN syntax
only; resolving it to a stored application needs the application registry that
`CreateApplication` (`20261024T055421Z-opensearch-createapplication-stub.md`) owns.

Both recorded units get past this operation and now stop at that `CreateApplication`
stub.
