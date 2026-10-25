Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.update-extension-parameter-schema-enforced-server-side appconfig.concurrent-update-extension-version-last-writer-wins
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.update-extension-parameter-schema-enforced-server-side|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: appconfig.concurrent-update-extension-version-last-writer-wins|run#13|AssertionError|assert len(successes) == n, f"expected all racers to succeed (UpdateExtension is not version-locked), got {results}"
Issue-Commit: b2b6f2b35a27af8dd637d5ecea8580938de38787

# AppConfig UpdateExtension: Dynamic+Required parameter schema is not enforced and concurrent in-place updates do not land (2 causes)

> _Note: this issue was identified against the code base at commit `b2b6f2b35a27af8dd637d5ecea8580938de38787`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateExtension

## Description
Two net-new root causes on the AppConfig `UpdateExtension` path. Both are observed as the operation refusing every call, and neither requirement is carried by the existing open stub issue `20261024T131534Z-appconfig-updateextension-stub.md` — that issue's replay keys are only `appconfig.extension-full-lifecycle-with-list`, `appconfig.extension-update-server-side-version-increment` and `appconfig.update-extension-version-increments-on-change`, so a fixer who implements a bare happy-path handler closes it while both behaviors below stay wrong and are never re-replayed.

Cause 1 of 2 — UpdateExtension does not enforce the parameter schema rule that a Dynamic parameter cannot be Required.
AWS: Smithy: `UpdateExtension` is PATCH /extensions/{ExtensionIdentifier} with success code 200 and output shape `Extension`; its modeled client errors are `BadRequestException` (httpError 400), `ConflictException` (409) and `ResourceNotFoundException` (404). Smithy documents on `Parameter$Dynamic`: "Indicates whether this parameter's value can be supplied at the extension's action point instead of during extension association. Dynamic parameters can't be marked Required." An update that would leave a parameter both `Required: true` and `Dynamic: true` is therefore refused with HTTP 400 `BadRequestException`, and the refusal message is built from the request — it names the offending parameter key(s) (recorded example: `Extension parameters P1 can not be both required and dynamic`). The rejected update has no effect: the parameter keeps its prior valid state (Required false, Dynamic false), and the same parameter marked `Required: true, Dynamic: false` is accepted.
Emulator: the call is answered with error code `NotImplemented` (HTTP 501, "operation UpdateExtension is scaffolded but not implemented in this emulator") instead of a 400 `BadRequestException`, so neither the refusal nor the accepted positive-control update exists.
Why: a client that relies on the server to reject an impossible parameter definition gets a 501 that is not in the operation's modeled error set — it cannot be matched as `BadRequestException`, retry/validation logic treats it as a transport/feature failure, and a configuration whose extension parameters are self-contradictory can never be detected server-side.
Fix: `UpdateExtension`'s handler and its input validation (services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_extension.rs): implement the handler and add the Dynamic-and-Required cross-member check that raises `BadRequestException` naming the offending parameter key, leaving stored state unchanged.
Traces: appconfig.update-extension-parameter-schema-enforced-server-side

Cause 2 of 2 — UpdateExtension's in-place, non-version-locked update semantics are absent under concurrency.
AWS: Smithy: `UpdateExtension` input carries an optional `VersionNumber` (Integer) and the output `Extension` echoes `Id`, `VersionNumber`, `Description`, `Actions`, `Parameters`; `ConflictException` (409) is modeled but is not raised for this shape of call. Recorded behavior: `UpdateExtension` mutates the named extension version in place and is NOT optimistically locked, so N concurrent calls that all supply the extension's current `VersionNumber` (1) ALL succeed with 200 — none is rejected as stale, no new extension version is forked, `VersionNumber` stays 1, and the extension ends with exactly one racer's `Description` (never a blend of two). A later serial update still lands, and an independent `GetExtension` reflects it.
Emulator: all 6 concurrent `UpdateExtension` calls returned error code `NotImplemented` (HTTP 501); 0 of 6 succeeded, so there is no last-writer-wins behavior, no version-stability guarantee and no post-race state to read back.
Why: a client performing idempotent in-place extension edits (a common reconciler pattern) sees every write refused with an unmodeled status instead of a 200, and cannot distinguish "not supported" from "conflicted"; once implemented, a handler that instead version-locked this operation would wrongly reject concurrent writers with 409.
Fix: `UpdateExtension`'s handler and the extension state it mutates (services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_extension.rs): implement an in-place mutation under the state lock that does not bump or fork `VersionNumber` and does not treat a supplied current `VersionNumber` as an optimistic-lock token.
Traces: appconfig.concurrent-update-extension-version-last-writer-wins

## Evidence
Cause 1 — check appconfig.update-extension-parameter-schema-enforced-server-side:
  Expected: UpdateExtension(ExtensionIdentifier=<ext>, Parameters={"P1": {"Required": true, "Dynamic": true}}) -> HTTP 400, Error.Code = "BadRequestException", message naming the parameter (recorded example: "Extension parameters P1 can not be both required and dynamic"); GetExtension still shows P1 Required=false, Dynamic=false; a follow-up update to {"Required": true, "Dynamic": false} succeeds.
  Actual: AssertionError: expected BadRequestException, got 'NotImplemented' — botocore ClientError (NotImplemented) when calling the UpdateExtension operation: "operation UpdateExtension is scaffolded but not implemented in this emulator" (HTTP 501).

Cause 2 — check appconfig.concurrent-update-extension-version-last-writer-wins:
  Expected: 6 concurrent UpdateExtension(VersionNumber=1, Description="racer-<i>") calls all return 200 with VersionNumber == 1; GetExtension then shows VersionNumber 1 and Description equal to one of the racers' values; a subsequent serial update lands and is visible to an independent read.
  Actual: AssertionError: expected all racers to succeed (UpdateExtension is not version-locked), got [('NotImplemented', None, None), ('NotImplemented', None, None), ('NotImplemented', None, None), ('NotImplemented', None, None), ('NotImplemented', None, None), ('NotImplemented', None, None)] — 0 of 6 succeeded.

## Steps to Reproduce
1. Boot the appconfig emulator and replay the two checks named in trace_ids.
2. Cause 1: CreateExtension(Name=<unique>, Actions={"ON_DEPLOYMENT_COMPLETE": [{"Name": "notify", "Uri": "arn:aws:events:<region>:<account>:event-bus/default"}]}, Parameters={"P1": {"Required": false, "Dynamic": false}}). Then call UpdateExtension(ExtensionIdentifier=<id>, Parameters={"P1": {"Required": true, "Dynamic": true}}) and observe error code NotImplemented (HTTP 501) instead of HTTP 400 BadRequestException; also observe that UpdateExtension(Parameters={"P1": {"Required": true, "Dynamic": false}}) — which AWS accepts — is refused the same way.
3. Cause 2: CreateExtension(Name=<unique>, Actions={"ON_DEPLOYMENT_COMPLETE": [{"Name": "notify", "Uri": "arn:aws:sns:<region>:<account>:<topic>"}]}) and assert VersionNumber == 1. Fire 6 concurrent UpdateExtension(ExtensionIdentifier=<id>, VersionNumber=1, Description="racer-<i>") calls and observe every one refused with NotImplemented instead of all 6 returning 200 with VersionNumber 1, followed by GetExtension showing one racer's Description.
