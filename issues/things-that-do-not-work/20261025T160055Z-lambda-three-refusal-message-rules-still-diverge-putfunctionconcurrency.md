Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.get-account-settings-enforced-limits lambda.remove-permission lambda.remove-permission-unknown-statement-not-found lambda.resource-not-found-error-consistency
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.get-account-settings-enforced-limits|run#13|AssertionError|assert message == expected_message, (
Issue-Key: lambda.remove-permission|run#12|AssertionError|assert msg == "No policy is associated with the given resource.", f"expected the exact gone-statement message, got {msg!r}"
Issue-Key: lambda.remove-permission-unknown-statement-not-found|run#10|AssertionError|assert message == _NO_POLICY_MESSAGE, (
Issue-Key: lambda.resource-not-found-error-consistency|run#30|AssertionError|assert layer_msg == expected, f"GetLayerVersion: expected message {expected!r}, got {layer_msg!r}"
Issue-Commit: ee5f79e932caffa01c897fc87f80cd65e8d793a6

# lambda: three refusal-message rules still diverge — PutFunctionConcurrency's account-floor sentence loses its terminating period, RemovePermission answers GetPolicy's not-found sentence, and GetLayerVersion names the layer instead of Lambda's canonical not-found sentence (3 causes)

> _Note: this issue was identified against the code base at commit `ee5f79e932caffa01c897fc87f80cd65e8d793a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemovePermission

## Description
Three distinct net-new root causes in the Lambda control plane, all of them message-RULE divergences: in every one the HTTP status and the error CODE match AWS, but the refusal's `Message` is not the sentence AWS emits for that condition. These are not cosmetic text differences — AWS's sentence for each of these conditions is fixed (or built by a fixed rule), so "emit this sentence" is the behaviour the emulator does not implement.

Relationship to what is already filed: open issue 20261025T035202Z-lambda-eight-refusal-message-rules-diverge-resourcenotfoundexception-preconditionfailedexception.md carries the replay keys of lambda.remove-permission, lambda.remove-permission-unknown-statement-not-found and lambda.resource-not-found-error-consistency, but only for OTHER assertions in those checks (its Cause 4 = GetPolicy's emptied-policy sentence, its Cause 5 = GetAlias not-found). Those two now answer correctly — the GetPolicy sentence was corrected by fixed issue 20261025T073446Z-ses-receipt-rule-lambdaaction-check-cannot-start-lambda.md and the GetAlias sentence by fixed issue 20261025T052458Z-codedeploy-and-lambda-getalias-refusal-messages-use-emulator.md — so the three checks now advance past those assertions and fail on the three assertions below, which no open issue covers and whose replay keys no open issue carries. The suite marked none of them OWNED or ALREADY FILED. lambda.get-account-settings-enforced-limits appears in no open issue at all.

Cause 1 of 3: PutFunctionConcurrency's "reservation drops the account below its unreserved floor" refusal omits the terminating period.
  AWS: Smithy (com.amazonaws.lambda#AWSGirApiService): `PutFunctionConcurrency` takes required `ReservedConcurrentExecutions` (@range min 0) and models `InvalidParameterValueException` at httpError 400. When the requested reservation would leave the account's UnreservedConcurrentExecutions below its floor, AWS answers 400 `InvalidParameterValueException` with the sentence `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [<floor>].` — the fixed text is everything outside the brackets, including the final period; the only variable part is the account's minimum-unreserved floor inside the brackets, taken from the account's limits (100 in the replayed account).
  Emulator: the same status and code, and the same sentence with the final `.` missing: `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [100]`.
  Why: the message is the only part of this refusal that tells a client WHICH limit it hit, and the check (like real client code and log-matchers) compares it for equality. A one-character divergence makes a correct-looking emulator fail every string-equality or regex-anchored match while a substring match would pass, so the break is silent.
  Fix: the account-limit validation in the PutFunctionConcurrency handler of the lambda service crate — emit AWS's sentence including the terminating period.
  Traces: lambda.get-account-settings-enforced-limits

Cause 2 of 3: RemovePermission for a StatementId that is not in the policy returns GetPolicy's not-found sentence instead of RemovePermission's.
  AWS: Smithy: `RemovePermission` (DELETE /2015-03-31/functions/{FunctionName}/policy/{StatementId}, success code 204) models `ResourceNotFoundException` at httpError 404. When the function exists but carries no policy statement with that Sid — whether the Sid was never added, or it was the only statement and has already been removed so the policy is now empty — AWS answers 404 `ResourceNotFoundException` with the constant sentence `No policy is associated with the given resource.` It is a true constant: no function name, ARN or statement id is interpolated. AWS deliberately uses a DIFFERENT sentence here from `GetPolicy`, which answers `The resource you requested does not exist.` for a policy-less function.
  Emulator: 404 `ResourceNotFoundException` with `The resource you requested does not exist.` — GetPolicy's sentence reused on the RemovePermission path.
  Why: a client cannot tell "there is no policy to remove this statement from" (safe to treat as already-removed) from "the resource you addressed does not exist" (the function name or qualifier is wrong). Idempotent teardown code and permission-reconciliation loops branch on exactly this distinction, and with both conditions collapsed onto one sentence they either retry forever or swallow a genuinely bad target.
  Fix: the not-found refusal construction in the RemovePermission handler of the lambda service crate — it must use RemovePermission's own constant, not the shared GetPolicy/resource-absent sentence.
  Traces: lambda.remove-permission, lambda.remove-permission-unknown-statement-not-found

Cause 3 of 3: GetLayerVersion's not-found message is generated from the request instead of Lambda's canonical not-found sentence.
  AWS: Smithy: `GetLayerVersion` (GET /2018-10-31/layers/{LayerName}/versions/{VersionNumber}) models `ResourceNotFoundException` at httpError 404. For a layer or layer version that does not exist AWS answers the constant `The resource you requested does not exist.` — no layer name and no version number appear in it.
  Emulator: 404 with the right code but `Layer version <layerName>:<versionNumber> does not exist`, built from the request (e.g. `Layer version layer-missing-4a56b79b567e:1 does not exist`; that name and the `:1` are per-run request values, not part of any requirement).
  Why: this check exists to prove Lambda's not-found refusals are consistent across resource kinds — a client that recognises Lambda's single canonical 404 sentence sees an unrecognised message here and must fall back to code-only handling, losing the ability to distinguish a Lambda not-found from arbitrary prose.
  Fix: the layer-version lookup error construction behind the GetLayerVersion handler of the lambda service crate.
  Traces: lambda.resource-not-found-error-consistency
  Note: this is the same message rule as Cause 2 of open issue 20261025T035202Z-lambda-eight-refusal-message-rules-diverge-resourcenotfoundexception-preconditionfailedexception.md (which lists lambda.delete-layer-version-idempotent-on-missing and lambda.layer-get-rejected-for-an-unknown-layer-while-list-returns-empty), reached here through a third check whose replay key that issue does not carry; fixing the shared construction closes both.

## Evidence
All three causes replayed against the booted emulator. In each the HTTP status and the error Code matched AWS; only Message diverged.

Cause 1 — lambda.get-account-settings-enforced-limits, PutFunctionConcurrency reserving 2000 against an account whose unreserved floor is 100:
  expected: `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [100].`
  actual:   `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [100]`
  (identical but for the missing terminating period; the bracketed 100 is the account's floor, the fixed text is the rest.)

Cause 2 — lambda.remove-permission (RemovePermission repeated on the statement it had just removed, i.e. the policy is now empty) and lambda.remove-permission-unknown-statement-not-found (RemovePermission on StatementId `never-existed`):
  expected: ResourceNotFoundException / `No policy is associated with the given resource.`
  actual:   ResourceNotFoundException / `The resource you requested does not exist.`
  Both checks pass their positive controls first — AddPermission then RemovePermission of a real Sid succeeds and the Sid is independently confirmed gone, and GetPolicy on the emptied policy now correctly answers `The resource you requested does not exist.` — so the divergence is isolated to the RemovePermission not-found sentence. The expected sentence is a constant: no function name, ARN or Sid belongs in it.

Cause 3 — lambda.resource-not-found-error-consistency, GetLayerVersion on a layer that was never published:
  expected: ResourceNotFoundException / `The resource you requested does not exist.`
  actual:   ResourceNotFoundException / `Layer version layer-missing-4a56b79b567e:1 does not exist`
  The layer name and `:1` in the actual message are this run's request values; the requirement is the constant sentence, with no request data in it. The check's earlier GetFunction/GetAlias legs now pass, so the failure is isolated to the layer-version path.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run exactly the checks in trace_ids:
   - lambda.get-account-settings-enforced-limits — CreateFunction, wait Active, read GetAccountSettings for the account's ConcurrentExecutions limit, then PutFunctionConcurrency(FunctionName=<fn>, ReservedConcurrentExecutions=2000). Assert the 400 InvalidParameterValueException Message equals `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [<floor>].`, period included, where <floor> is the account's minimum unreserved concurrency.
   - lambda.remove-permission — CreateFunction, wait Active, AddPermission(StatementId='to-remove', Action='lambda:InvokeFunction', Principal='*'), confirm the Sid via GetPolicy, RemovePermission it (succeeds), then RemovePermission the SAME StatementId again. Assert 404 ResourceNotFoundException with Message exactly `No policy is associated with the given resource.`
   - lambda.remove-permission-unknown-statement-not-found — same setup; after removing the one statement, call RemovePermission(StatementId='never-existed'). Assert 404 ResourceNotFoundException with Message exactly `No policy is associated with the given resource.`
   - lambda.resource-not-found-error-consistency — call GetLayerVersion(LayerName=<a layer name never published>, VersionNumber=1). Assert 404 ResourceNotFoundException with Message exactly `The resource you requested does not exist.`, with no layer name or version number in the text.
3. In every case the status and Code already match; assert on Message.
