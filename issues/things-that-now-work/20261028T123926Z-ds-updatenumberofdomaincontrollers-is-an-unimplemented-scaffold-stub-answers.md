Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.update-number-of-domain-controllers-limit-exceeded-blocked
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.update-number-of-domain-controllers-limit-exceeded-blocked|run#7|AssertionError|assert code == "DomainControllerLimitExceededException", f"expected DomainControllerLimitExceededException for DesiredNumber=999, got {code!r}"
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# ds: UpdateNumberOfDomainControllers is an unimplemented scaffold stub — answers 501 NotImplemented instead of enforcing the 20-domain-controller-per-directory limit or scaling the directory

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateNumberOfDomainControllers

## Description
One net-new root cause in the ds (AWS Directory Service) control plane.

Cause 1 of 1: `UpdateNumberOfDomainControllers` is still an unmodified scaffold, so EVERY call to it — valid or invalid — is refused. Neither the domain-controller limit refusal nor the actual scale-out behaviour exists.
Failing check ids: ds.update-number-of-domain-controllers-limit-exceeded-blocked

AWS: Smithy model `com.amazonaws.directoryservice#UpdateNumberOfDomainControllers` (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json). Input: `DirectoryId` (required, pattern `^d-[0-9a-f]{10}$`) and `DesiredNumber` (required, `DesiredNumberOfDomainControllers`, integer @range min 2). Output `UpdateNumberOfDomainControllersResult` is empty, success status 200. Modeled errors: `ClientException` (400), `DirectoryUnavailableException` (400), `DomainControllerLimitExceededException` (400), `EntityDoesNotExistException` (400), `InvalidParameterException` (400), `ServiceException` (500), `UnsupportedOperationException` (400). The model documents the operation as "Adds or removes domain controllers to or from the directory. Based on the difference between current value and new value (provided through this API call), domain controllers will be added or removed", and documents `DomainControllerLimitExceededException` as "The maximum allowed number of domain controllers per directory was exceeded. The default limit per directory is 20 domain controllers." So a `DesiredNumber` above the per-directory limit (default 20) must be refused synchronously with HTTP 400 and error code `DomainControllerLimitExceededException`, leaving the directory's domain-controller count unchanged; the message is the fixed text AWS words for this error class (the limit-exceeded sentence above), with its variable parts — the requested number and the directory limit — taken from the request and the directory's own quota, plus the modeled `RequestId` member (a per-run value, not part of the template). An in-limit `DesiredNumber` must be accepted with 200 and must actually change the directory's domain-controller set, so a subsequent `DescribeDomainControllers` reports the new count.

Emulator: the handler at services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/update_number_of_domain_controllers.rs is the generated stub — its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateNumberOfDomainControllers"))` and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` census marker. Replay shows: `UpdateNumberOfDomainControllers(DirectoryId=d-..., DesiredNumber=999)` returns error code `NotImplemented` (HTTP 501, message "operation UpdateNumberOfDomainControllers is scaffolded but not implemented in this emulator") where AWS returns HTTP 400 `DomainControllerLimitExceededException`. The check asserts `code == "DomainControllerLimitExceededException"` and fails with `got 'NotImplemented'`. The positive control — an in-limit `DesiredNumber = initial_count + 1` followed by waiting for `DescribeDomainControllers` to report the extra controller — is never reached, and cannot pass while the handler returns 501.

Note: this check previously failed on `DescribeDomainControllers`, which issue 20261028T011637Z-ds-describedomaincontrollers-stub (status fixed) addressed; the read path now answers, and the check progresses to this distinct, still-unimplemented write operation. This is a different operation from the one that fixed issue claimed, so it is filed as net-new rather than a reopen.

Why: a client that resizes a Managed Microsoft AD gets a 501 for every request. Code that relies on AWS refusing an over-limit resize with `DomainControllerLimitExceededException` sees an unmodeled `NotImplemented` instead, so quota-handling and retry branches keyed on that error code are never exercised and cannot be tested; code that performs a legitimate in-limit resize can never observe the new domain controller, so directory scaling is untestable end to end against the emulator.

Fix: implement the operation's handler — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/update_number_of_domain_controllers.rs — replacing the `not_implemented` return with a handler that (a) resolves `DirectoryId` in the service state and answers `EntityDoesNotExistException` when absent, (b) gates on directory stage (`DirectoryUnavailableException` when not Active), (c) validates `DesiredNumber` against the directory's domain-controller limit and returns `DomainControllerLimitExceededException` with AWS's wording when exceeded, without mutating state, and (d) on an accepted value mutates the directory's domain-controller collection in state so `DescribeDomainControllers` reflects the new count; then delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from that file.

Traces: ds.update-number-of-domain-controllers-limit-exceeded-blocked

## Evidence
Check ds.update-number-of-domain-controllers-limit-exceeded-blocked [mismatch]:

  File "check_ds_update_number_of_domain_controllers_limit_exceeded_blocked.py", line 41, in run
    assert code == "DomainControllerLimitExceededException", f"expected DomainControllerLimitExceededException for DesiredNumber=999, got {code!r}"
  AssertionError: expected DomainControllerLimitExceededException for DesiredNumber=999, got 'NotImplemented'

Expected (AWS): HTTP 400, Error.Code = DomainControllerLimitExceededException, Error.Message = AWS's limit-exceeded text for this error class (fixed sentence about the maximum allowed number of domain controllers per directory / default limit of 20, with the requested number and the directory's limit filled in from the request and state), plus the modeled RequestId member. Directory's domain-controller count unchanged. A following in-limit UpdateNumberOfDomainControllers(DesiredNumber=initial+1) returns HTTP 200 with an empty body, and DescribeDomainControllers then lists initial+1 controllers.

Actual (emulator): HTTP 501, Error.Code = NotImplemented, Error.Message = "operation UpdateNumberOfDomainControllers is scaffolded but not implemented in this emulator". No state change, and the in-limit positive control is unreachable.

Source evidence: services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/update_number_of_domain_controllers.rs line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 166 is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateNumberOfDomainControllers"))`.

## Steps to Reproduce
1. Boot the ds emulator from this checkout.
2. Create a Managed Microsoft AD (Standard edition) and wait for Stage = Active; confirm DescribeDomainControllers reports at least one domain controller.
3. Call UpdateNumberOfDomainControllers(DirectoryId=<that id>, DesiredNumber=999). Observe HTTP 501 / NotImplemented instead of HTTP 400 / DomainControllerLimitExceededException.
4. Call UpdateNumberOfDomainControllers(DirectoryId=<that id>, DesiredNumber=<current count + 1>). Observe HTTP 501 / NotImplemented instead of HTTP 200, and DescribeDomainControllers never reports the additional controller.
5. Re-replay check ds.update-number-of-domain-controllers-limit-exceeded-blocked to confirm the fix.
