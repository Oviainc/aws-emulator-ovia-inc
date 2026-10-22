Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.get-account-setting-invalid-name-fails ecr.get-account-setting-invalid-name-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|GetAccountSetting|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ecr GetAccountSetting: unimplemented scaffold stub

> _Update (fix of `20261022T002237Z-ecr-putaccountsetting-stub.md`): the handler now EXISTS. `GetAccountSetting` is the reader the `PutAccountSetting` round-trip is proved through, so it was implemented in that change: `src/ops/get_account_setting.rs` reports the setting from `state::...State::account_settings` (or its documented default), refuses a name this registry does not serve with `InvalidParameterException`, and its `//! SCAFFOLDED, NOT IMPLEMENTED:` census line is gone. What is still open here is only this issue's own two recorded units about invalid names: they were NOT replayed by that fix (it replays only its own unit), so the error shape and wording they expect are unchecked against the implemented handler — check them before changing it._

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSetting

## Description
`GetAccountSetting` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/get_account_setting.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetAccountSetting` call, so each one unblocks only when `GetAccountSetting` works: `ecr.get-account-setting-invalid-name-fails`, `ecr.get-account-setting-invalid-name-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/get_account_setting.rs`. Replaying the corpus, the emulator refused the `GetAccountSetting` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAccountSetting operation: operation GetAccountSetting is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr emulator
Call GetAccountSetting and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
