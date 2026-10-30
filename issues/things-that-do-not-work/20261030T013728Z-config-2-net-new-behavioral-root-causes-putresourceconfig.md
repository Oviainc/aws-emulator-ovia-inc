Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.put-resource-config-rejects-unregistered-type config.put-resource-config-and-list-discovered config.resource-config-history-chronological-ordering config.resource-config-unicode-and-binary-blob-round-trip config.select-and-batch-get-resource-config
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.put-resource-config-rejects-unregistered-type|run#12|AssertionError|assert code == "ValidationException", code
Issue-Key: config.put-resource-config-and-list-discovered|run#17|AssertionError|assert code == "ValidationException", f"unexpected code: {code}"
Issue-Key: config-service|PutResourceConfig|InsufficientPermissionsException
Issue-Key: config.resource-config-unicode-and-binary-blob-round-trip|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: config.select-and-batch-get-resource-config|run#4|AssertionError|assert message == "syntax error at line 1, column 19", (
Issue-Commit: 68c23b152149323c5b4a39e6e1afab7c6a9b73aa

# config: 2 net-new behavioral root causes — PutResourceConfig misclassifies the unregistered-resource-type refusal as InsufficientPermissionsException, and SelectResourceConfig reports the wrong syntax-error column

> _Note: this issue was identified against the code base at commit `68c23b152149323c5b4a39e6e1afab7c6a9b73aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourceConfig

## Description
Two distinct, net-new root causes on the AWS Config resource-config surface. Neither is covered by an open issue: the open Config issues (20261030T003924Z-config-read-paths-over-refuse-fabricate-aggregator-rows.md, 20261030T003924Z-config-recorder-tagging-and-retention-refusals-answer-wrong.md, 20261023T140058Z-config-refusal-contract-validation-messages-report-one-violation.md, 20261029T184030Z-config-9-net-new-refusal-and-representation-divergences.md) cover other operations and carry neither of the replay keys below. A fixer must close BOTH causes before submitting.

=== Cause 1 of 2: PutResourceConfig refuses an unregistered ResourceType with the wrong error class ===
AWS: PutResourceConfig records the configuration state only of a resource type the CloudFormation registry holds a registration for (Smithy doc, PutResourceConfig: "The custom resource type must be registered with CloudFormation"). When the registry holds no such type, AWS refuses the request as an INPUT validation failure: HTTP 400, error code `ValidationException` (Smithy: PutResourceConfig models ValidationException, InsufficientPermissionsException, MaxActiveResourcesExceededException, NoRunningConfigurationRecorderException, all client/400; ValidationException doc: "The requested operation is not valid. You will see this exception if there are missing required fields or if the input value fails the validation."). Recorded behavior shows the message is a fixed two-sentence template whose only variable part is the ResourceType taken from the request: fixed text `Cannot find registered Resource Type with name ` + <the request's ResourceType, verbatim> + `. Please register the Resource Type with Cloudformation and try again.` AWS's `InsufficientPermissionsException` is reserved for the IAM/role-permission failures its own doc enumerates (PutConfigRule, PutOrganizationConfigRule, PutConformancePack, PutServiceLinkedConfigurationRecorder), not for an unregistered type.
Emulator: with a running recorder and a role that has been granted cloudformation:DescribeType/ListTypes, every PutResourceConfig naming a type outside the published registry list is refused with error code `InsufficientPermissionsException` (HTTP 400). The message text itself is correct — the emulator builds exactly the template above — but the error CODE is wrong, and it is wrong unconditionally and permanently: one check polls for up to its deadline waiting for the permission error to clear and it never does. The choice is deliberate in the handler (it classifies the registry lookup as a permission failure of the recorder's role rather than as validation of the submitted member).
Why: a client cannot distinguish "my type is not registered with CloudFormation — register it and retry" from "the Config recorder's role is missing an IAM permission — fix the role". The two have completely different remediations, and botocore/SDK retry and error-handling branches keyed on the error code take the permissions branch. Any caller probing type registration via PutResourceConfig, or asserting on ValidationException, breaks against the emulator.
Fix: the PutResourceConfig handler's resource-type registration gate — it must raise the modeled ValidationException variant (keeping the existing message template, including the request's ResourceType as its one variable part) instead of InsufficientPermissionsException. The ordering relative to the recorder check (NoRunningConfigurationRecorderException first) is correct and should be preserved.
Traces: config.put-resource-config-rejects-unregistered-type (AssertionError: InsufficientPermissionsException, expected ValidationException); config.put-resource-config-and-list-discovered (AssertionError: unexpected code: InsufficientPermissionsException); config.resource-config-history-chronological-ordering (AssertionError: expected ValidationException for an unregistered resource type, got 'InsufficientPermissionsException'); config.resource-config-unicode-and-binary-blob-round-trip (AssertionError: expected ValidationException, got 'InsufficientPermissionsException'). Handler: services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_resource_config.rs lines 299-316 (and the module doc at lines 11-13 that states the wrong rule).

=== Cause 2 of 2: SelectResourceConfig's InvalidExpressionException reports the wrong syntax-error column ===
AWS: SelectResourceConfig refuses a malformed SELECT expression with HTTP 400 and error code `InvalidExpressionException` (Smithy: InvalidExpressionException, client/400, doc "The syntax of the query is incorrect"). The model does not specify the message; recorded behavior only: the message is the parser's position report, fixed text `syntax error at line ` + <line> + `, column ` + <column>, where line/column are 1-based positions in the SUBMITTED expression at which parsing stopped. For the expression `SELECT ((( invalid` (18 characters) AWS reports `syntax error at line 1, column 19` — i.e. the position one past the end of the input, where the parser ran out of tokens while still needing to close the open parentheses. The numbers are values derived from the request's expression, not constants.
Emulator: the error code and HTTP status are right, but the position is computed differently — for the same expression the emulator answers `syntax error at line 1, column 8`, the 1-based offset of the first offending `(` rather than the offset where the read terminated. Hypothesis: the emulator's expression reader surfaces the position of the token it could not accept, while AWS surfaces the position the parse advanced to before failing (end-of-input for an unterminated construct).
Why: the message is the only machine-usable detail on this error class — one error code stands for every rule of the Config query language — so tools and query editors that highlight the offending position point at the wrong character, and any client asserting on the exact message text diverges.
Fix: the shared Config query-expression reader and its refusal wording — crate::state::read_expression / ExpressionRefusal::message in services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs, reached from services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/select_resource_config.rs lines 199-210. The position carried in the refusal must be where the parse stopped (end-of-input for an unterminated expression), not the start of the rejected token.
Traces: config.select-and-batch-get-resource-config (AssertionError: expected the parser's exact position report, got 'syntax error at line 1, column 8').

## Evidence
Cause 1 — expected (AWS): HTTP 400, Error.Code="ValidationException", Error.Message="Cannot find registered Resource Type with name <ResourceType from request>. Please register the Resource Type with Cloudformation and try again."
Actual (emulator): HTTP 400, Error.Code="InsufficientPermissionsException" with that same message body.
Replay output:
  config.put-resource-config-rejects-unregistered-type -> check_config_put_resource_config_rejects_unregistered_type.py:289 `assert code == "ValidationException", code` -> AssertionError: InsufficientPermissionsException
  config.put-resource-config-and-list-discovered -> check_...:368 -> AssertionError: unexpected code: InsufficientPermissionsException
  config.resource-config-history-chronological-ordering -> check_...:211 -> AssertionError: expected ValidationException for an unregistered resource type, got 'InsufficientPermissionsException'
  config.resource-config-unicode-and-binary-blob-round-trip -> check_...:40 -> AssertionError: expected ValidationException, got 'InsufficientPermissionsException'
The checks pre-grant cloudformation:DescribeType/ListTypes to the recorder role and ensure a RUNNING recorder precisely so that no permission or recorder precondition can mask the type rejection, and one of them polls past InsufficientPermissionsException until its deadline — the emulator never stops returning it. Source confirms the classification is unconditional: put_resource_config.rs:307-316 returns PutResourceConfigErrorModeled::InsufficientPermissionsException for `!is_registered_resource_type(..)`, with the correct message built by `unregistered_resource_type_message` (put_resource_config.rs:161-166).

Cause 2 — expression submitted: `SELECT ((( invalid` (18 chars).
Expected (AWS): HTTP 400, Error.Code="InvalidExpressionException", Error.Message="syntax error at line 1, column 19" — fixed text "syntax error at line " / ", column "; the line and column are values derived from the submitted expression (here, one past its end).
Actual (emulator): HTTP 400, Error.Code="InvalidExpressionException", Error.Message="syntax error at line 1, column 8".
Replay output: config.select-and-batch-get-resource-config -> check_config_select_and_batch_get_resource_config.py:40 `assert message == "syntax error at line 1, column 19"` -> AssertionError: expected the parser's exact position report, got 'syntax error at line 1, column 8'. The error code and the subsequent well-formed-expression success path both match; only the position differs.

## Steps to Reproduce
Cause 1:
1. Create/adopt an IAM role trusted by config.amazonaws.com with AWS_ConfigRole attached, plus an inline grant of cloudformation:DescribeType and cloudformation:ListTypes.
2. Create an S3 delivery bucket + delivery channel and a configuration recorder using that role, and StartConfigurationRecorder so a recorder is RUNNING.
3. Call PutResourceConfig(ResourceType="Sim2RealTest::Widget::Resource", SchemaVersionId="1-0-0", ResourceId=<any unique id>, Configuration="{\"greeting\":\"hello\"}").
4. Observe Error.Code: AWS answers ValidationException; the emulator answers InsufficientPermissionsException (same Message text).
5. Replay: config.put-resource-config-rejects-unregistered-type, config.put-resource-config-and-list-discovered, config.resource-config-history-chronological-ordering, config.resource-config-unicode-and-binary-blob-round-trip.

Cause 2:
1. Call SelectResourceConfig(Expression="SELECT ((( invalid").
2. Observe Error.Message: AWS answers "syntax error at line 1, column 19"; the emulator answers "syntax error at line 1, column 8".
3. Replay: config.select-and-batch-get-resource-config.
