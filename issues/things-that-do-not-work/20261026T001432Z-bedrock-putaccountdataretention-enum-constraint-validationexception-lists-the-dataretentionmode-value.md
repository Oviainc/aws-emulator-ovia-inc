Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.account-data-retention-ttl-enforced-on-logs bedrock.put-account-data-retention-invalid-period-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|PutAccountDataRetention|ValidationException
Issue-Key: bedrock.put-account-data-retention-invalid-period-out-of-range-rejected|run#6|AssertionError|assert message == "1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, provider_data_share,
Issue-Commit: 7d0541b983b4b8eb6a8ca4ee6daf240e5ee713c5

# PutAccountDataRetention enum-constraint ValidationException lists the DataRetentionMode value set in model-declaration order instead of AWS's order

> _Note: this issue was identified against the code base at commit `7d0541b983b4b8eb6a8ca4ee6daf240e5ee713c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountDataRetention

## Description
Cause 1 of 1: the crate-wide enum-constraint refusal renders the admitted value set in the order the generated schema carries (the model's declaration order) rather than the order Amazon Bedrock lists it in, so the ValidationException MESSAGE for an out-of-enum `mode` on PutAccountDataRetention diverges while status, code, position and requirement clause are all correct.

Failing checks for this cause: bedrock.account-data-retention-ttl-enforced-on-logs, bedrock.put-account-data-retention-invalid-period-out-of-range-rejected.

NOTE — related open issue: `20261024T033000Z-bedrock-enum-validation-message-wording.md` describes this same renderer and the same question (it is the only place the clause is built, `src/service/validation.rs::enum_requirement`). That issue records the emulator emitting the set ALPHABETICALLY (`[aws_review, default, inherit, none, provider_data_share]`); the emulator now emits MODEL-DECLARATION order (`[default, none, aws_review, provider_data_share, inherit]`), so the observed text has moved on from what is filed there and neither order matches AWS. It did not carry these two checks' replay keys, so they are filed here; a fixer should treat the two together.

AWS: Smithy — `PutAccountDataRetention.mode` is required, targets the closed enum `com.amazonaws.bedrock#DataRetentionMode` (declared values: default, none, aws_review, provider_data_share, inherit), `PUT /data-retention`, modeled error `ValidationException` (HTTP 400). The model does not state the wording of the refusal, so for the message this is recorded behavior only: both behavioral checks record Bedrock refusing an out-of-enum mode with HTTP 400 `ValidationException` and the message

  `1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, provider_data_share, none, aws_review, inherit]`

Fixed text of that message: `N validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]`. Variable parts taken from the request/model: the error count (1 here, one per violating member), `<member>` = the input member path from the request (`mode`), and `<values>` = the enum's admitted values in the order the service lists them — for DataRetentionMode the recorded order is `default, provider_data_share, none, aws_review, inherit`. The offending value itself is NOT echoed, which the emulator already gets right. Nothing in the recording or the model explains the ordering rule; Hypothesis: Bedrock lists an enum's values in the order of its own internal/documented value listing, which is neither alphabetical nor the public model's declaration order — so the rule must be established across several of this service's enums rather than by pasting one recording's order into the renderer.

Emulator: the same status (400), the same code (`ValidationException`), the same clause and the same member position, but the bracketed set is ordered `[default, none, aws_review, provider_data_share, inherit]` — the model's declaration order. Replay output for both checks: expected message `... [default, provider_data_share, none, aws_review, inherit]`, got `... [default, none, aws_review, provider_data_share, inherit]`. Everything else both checks assert about this surface passes (the rejected write leaves the stored mode untouched; valid modes echo and persist), so the message order is the whole divergence.

Why: a client that parses the refusal to show a user the permitted values, or that asserts on / diffs the refusal text (as these checks do), sees a message the real service never emits. Because one renderer words every enum-constrained member of every operation this crate serves, the divergence is not specific to data retention: any client checking enum-refusal text against recorded Bedrock behavior disagrees with the emulator.

Fix: the shared validation layer that formats enum-constraint messages — the crate-wide `enum_requirement` renderer in the bedrock control-plane service's validation module (the centralized validation walk that runs BEFORE the operation handler; `put_account_data_retention.rs` itself does not validate, see its module docs lines 15-17 and `handle` at line 128). The ordering fed to that renderer is the generated enum value list on the member schema, so the ordering decision belongs with it, not in the operation handler or state.

Traces: bedrock.account-data-retention-ttl-enforced-on-logs (asserts the message after setting a valid mode first), bedrock.put-account-data-retention-invalid-period-out-of-range-rejected (asserts code + message for `mode="thirty-seven-years"`).

## Evidence
Check bedrock.put-account-data-retention-invalid-period-out-of-range-rejected, line 33:
  AssertionError: unexpected rejection message: "1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, none, aws_review, provider_data_share, inherit]"
  expected: "1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, provider_data_share, none, aws_review, inherit]"

Check bedrock.account-data-retention-ttl-enforced-on-logs, line 55:
  AssertionError: expected message "1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, provider_data_share, none, aws_review, inherit]", got "1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, none, aws_review, provider_data_share, inherit]"

Expected (recorded AWS): HTTP 400, Code `ValidationException`, value set ordered `[default, provider_data_share, none, aws_review, inherit]`.
Actual (emulator): HTTP 400, Code `ValidationException`, value set ordered `[default, none, aws_review, provider_data_share, inherit]` — i.e. the Smithy declaration order reported by smithy_query(query="shape", shape="DataRetentionMode") = [default, none, aws_review, provider_data_share, inherit].

Model citation: smithy_query(query="operation", operation="PutAccountDataRetention", model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json") -> input {mode: DataRetentionMode, required}, errors include ValidationException (httpError 400), http PUT /data-retention.

## Steps to Reproduce
1. Boot the bedrock emulator from this checkout.
2. `PUT /data-retention` with body `{"mode": "thirty-seven-years"}` (any value outside DataRetentionMode), e.g. `aws bedrock put-account-data-retention --mode thirty-seven-years`.
3. Observe HTTP 400 with Code `ValidationException` and Message `1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, none, aws_review, provider_data_share, inherit]`.
4. AWS answers the identical status, code and clause with the set ordered `[default, provider_data_share, none, aws_review, inherit]`.
5. Re-replay bedrock.put-account-data-retention-invalid-period-out-of-range-rejected and bedrock.account-data-retention-ttl-enforced-on-logs to confirm; both also assert that the rejected write leaves the previously stored mode untouched (which already holds).
