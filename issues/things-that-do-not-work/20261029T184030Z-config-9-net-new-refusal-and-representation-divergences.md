Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.discovered-resource-counts-and-config-history config.get-aggregate-resource-config-not-discovered-rejected config.delete-remediation-exceptions-nonexistent-rejected config.select-aggregate-resource-config-errors config.remediation-configuration-lifecycle config.put-service-linked-recorder-limit-exceeded-rejected config.service-linked-configuration-recorder-duplicate-conflict-rejected config.batch-get-aggregate-resource-config-malformed-batch-rejected config.proactive-resource-evaluation-reaches-terminal-state config.resource-evaluation-summary-reflects-proactive-evaluation
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.discovered-resource-counts-and-config-history|run#9|AssertionError|assert code == "ResourceNotDiscoveredException", (code, message)
Issue-Key: config.get-aggregate-resource-config-not-discovered-rejected|run#12|AssertionError|assert message == _NOT_DISCOVERED, message
Issue-Key: config.delete-remediation-exceptions-nonexistent-rejected|_error_code#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: config.select-aggregate-resource-config-errors|run#13|AssertionError|assert message == "syntax error at line 1, column 19", message
Issue-Key: config-service|DeleteRemediationConfiguration|NoSuchRemediationConfigurationException
Issue-Key: config.put-service-linked-recorder-limit-exceeded-rejected|run#23|AssertionError|assert withdrawn_message == _UNKNOWN_PRINCIPAL_MESSAGE, (
Issue-Key: config.service-linked-configuration-recorder-duplicate-conflict-rejected|run#22|AssertionError|assert gone_message == expected_gone, f"expected {expected_gone!r}, got {gone_message!r}"
Issue-Key: config.batch-get-aggregate-resource-config-malformed-batch-rejected|run#19|AssertionError|assert masked == expected_message, (
Issue-Key: config.proactive-resource-evaluation-reaches-terminal-state|run#21|AssertionError|assert failure_reason == "Resource evaluation timed out.", (
Issue-Key: config.resource-evaluation-summary-reflects-proactive-evaluation|run#15|AssertionError|assert final["EvaluationStatus"].get("FailureReason") == (
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# config: 9 net-new refusal and representation divergences — aggregate reads, remediation, service-linked recorders and resource evaluation

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
This issue bundles 9 DISTINCT net-new root causes in the config service, each grounded in failing behavioral checks from this replay. They are numbered Cause 1 of 9 .. Cause 9 of 9 and each carries its own failing check ids; a fixer must close ALL NINE before submitting. None is covered by an open issue: the closest open ones are 20261023T140058Z-config-refusal-contract-validation-messages-report-one-violation.md and 20261022T190340Z-config-putconfigrule-and-putdeliverychannel-are-overly-permissive-inconsistent.md, which own other checks and other operations; these nine operations are not in them.

=== Cause 1 of 9: GetResourceConfigHistory refuses an undiscovered resource with the recorder gate instead of ResourceNotDiscoveredException ===
AWS: Smithy: GetResourceConfigHistory models BOTH NoAvailableConfigurationRecorderException and ResourceNotDiscoveredException (shape doc: "You have specified a resource that is either unknown or has not been discovered."), plus ValidationException; resourceId is @length 1..768. Recorded behavior: a well-formed resourceType/resourceId pair that was never discovered is refused 400 ResourceNotDiscoveredException whose message is the fixed template "Resource <resourceId> of resourceType:<resourceType> is unknown or has not been discovered" — fixed text "Resource ", " of resourceType:", " is unknown or has not been discovered", with resourceId and resourceType taken verbatim from the request — and this refusal does not depend on whether the account holds a configuration recorder.
Emulator: 400 NoAvailableConfigurationRecorderException, message "there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one". The recorder-existence gate is evaluated before, and instead of, the per-resource discovery check, so the per-resource class is unreachable.
Why: a client cannot tell "this resource was never recorded" from "this account records nothing"; the two classes are modelled separately and drive different remediation, and code-based error handling (botocore config.exceptions.ResourceNotDiscoveredException) never matches.
Fix: the gate ordering in GetResourceConfigHistory's handler — the discovery lookup must decide the refusal for a resource that has no history, and the recorder gate must not pre-empt it.
Traces: config.discovered-resource-counts-and-config-history (asserts code == "ResourceNotDiscoveredException", got NoAvailableConfigurationRecorderException); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs.

=== Cause 2 of 9: GetAggregateResourceConfig's ResourceNotDiscoveredException message is request-derived instead of AWS's fixed sentence ===
AWS: Smithy: GetAggregateResourceConfig models ResourceNotDiscoveredException, doc "You have specified a resource that is either unknown or has not been discovered." Recorded behavior: the message is exactly that one FIXED sentence — "You have specified a resource that is either unknown or has not been discovered." — with NO part of the request interpolated.
Emulator: the code is right but the message is built from the request: "The resource '<resourceId>' of type '<resourceType>' in account '<accountId>' and region '<region>' is either unknown or has not been discovered." Every element after "The resource" is request-derived; AWS interpolates nothing here.
Why: a client that compares or logs the refusal text (and the sibling NoSuchConfigurationAggregatorException sentence this check pairs it with) sees a sentence AWS never emits, and an aggregate read path that leaks the queried account id into the message where AWS does not.
Fix: the refusal construction in GetAggregateResourceConfig's handler — use the fixed service sentence for this error class.
Traces: config.get-aggregate-resource-config-not-discovered-rejected (assert message == "You have specified a resource that is either unknown or has not been discovered."); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_aggregate_resource_config.rs.

=== Cause 3 of 9: DeleteRemediationExceptions accepts a delete of an exception that does not exist ===
AWS: Smithy: DeleteRemediationExceptions takes ConfigRuleName (required) and ResourceKeys (required, len 1..100) and models exactly ONE error, NoSuchRemediationExceptionException (HTTP 400), doc "You tried to delete a remediation exception that does not exist." Recorded behavior: for a (rule, resource) pair with no matching remediation exception — including a rule name that never existed — the call is REFUSED 400 NoSuchRemediationExceptionException with the fixed sentence "Specified RemediationException could not be found."
Emulator: HTTP 200 with body {"RemediationExceptions": []} (content-length 28). The delete's "must already exist" precondition is not enforced at all, and the 200 body carries a member that is not even in the modelled output (the modelled output member is FailedBatches).
Why: a client deleting a non-existent exception is told the delete succeeded, so a cleanup/reconciliation loop treats absent state as deleted state and never surfaces the error AWS raises; the operation also answers a shape its own model does not declare.
Fix: the existence precondition in DeleteRemediationExceptions' handler (refuse with its one modelled error), and its output serialization (FailedBatches, not RemediationExceptions).
Traces: config.delete-remediation-exceptions-nonexistent-rejected ("expected the call to be rejected, but it SUCCEEDED and returned {'RemediationExceptions': [], ...HTTPStatusCode: 200}"); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/delete_remediation_exceptions.rs.

=== Cause 4 of 9: SelectAggregateResourceConfig leaks its internal SQL parser error as the InvalidExpressionException message ===
AWS: Smithy: SelectAggregateResourceConfig models InvalidExpressionException (HTTP 400), doc "The syntax of the query is incorrect." Recorded behavior: a malformed expression is refused with a message of the form "syntax error at line <line>, column <column>" — fixed text "syntax error at line " and ", column ", with line and column being the position in the submitted Expression where parsing failed (for the 18-character expression "SELECT ((( invalid" that is line 1, column 19). AWS reports position only; it never names its parser or echoes the parser's own diagnostic.
Emulator: 400 with message "The expression is not a valid Config query: the expression is not a SQL SELECT command: sql parser error: Expected: ), found: EOF." — the embedded third-party parser's text, with no line/column.
Why: the refusal text is part of the contract clients read and log; a Config query console/tool that points at the offending position in the user's expression has nothing to parse, and the message advertises emulator internals.
Fix: the InvalidExpressionException construction in SelectAggregateResourceConfig's validation — map the parse failure to a (line, column) position and emit AWS's template. The sibling SelectResourceConfig wording was addressed by the fixed issue 20261025T084103Z-config-refusal-messages-aggregator-not-found-selectresourceconfig-invalid.md; the AGGREGATE operation still diverges, with a different message class (position vs parser text).
Traces: config.select-aggregate-resource-config-errors (assert message == "syntax error at line 1, column 19"); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/select_aggregate_resource_config.rs.

=== Cause 5 of 9: DeleteRemediationConfiguration's not-found message quotes the rule name and adds a trailing period ===
AWS: Smithy: DeleteRemediationConfiguration models NoSuchRemediationConfigurationException (HTTP 400), doc "You specified an Config rule without a remediation configuration." Recorded behavior: the message is "No RemediationConfiguration for rule <ConfigRuleName> exists" — fixed text "No RemediationConfiguration for rule " and " exists", with ConfigRuleName taken from the request UNQUOTED and NO terminating period.
Emulator: "No RemediationConfiguration for rule '<ConfigRuleName>' exists." — the rule name is wrapped in single quotes and a period is appended.
Why: the message is a requirement, and this one differs from AWS's for every rule name; a client matching or asserting on the refusal text (including the idempotent re-delete leg of a remediation lifecycle) diverges on punctuation the service does not emit.
Fix: the NoSuchRemediationConfigurationException message template used by DeleteRemediationConfiguration's handler (shared state message helper).
Traces: config.remediation-configuration-lifecycle (expected 'No RemediationConfiguration for rule <rule> exists', got "No RemediationConfiguration for rule '<rule>' exists."); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/delete_remediation_configuration.rs and the message helper in src/state.rs.

=== Cause 6 of 9: PutServiceLinkedConfigurationRecorder's unsupported-principal ValidationException message is emulator prose ===
AWS: Smithy: PutServiceLinkedConfigurationRecorder models ValidationException (HTTP 400) and takes ServicePrincipal (required, len 1..128). Recorded behavior: for a service principal Config will not create a service-linked recorder for, the refusal is 400 ValidationException with the FIXED sentence "The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again." — no part of the request is interpolated.
Emulator: the code is right, the message is not: "The ServicePrincipal '<principal>' provided in the request is not a valid service principal for a service-linked configuration recorder."
Why: an error message is a requirement; a client or operator reading the refusal gets a sentence AWS never produces, and the one AWS produces (which names the verification step) is lost.
Fix: the ValidationException message in PutServiceLinkedConfigurationRecorder's validation — use the fixed service sentence for the unknown/unsupported-principal class.
Traces: config.put-service-linked-recorder-limit-exceeded-rejected (expected the fixed sentence, got the request-derived one; the check itself is tagged sweep:never because the capability was withdrawn by AWS — the divergence filed here is ONLY the refusal wording, both sides refuse with ValidationException); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_service_linked_configuration_recorder.rs.

=== Cause 7 of 9: DeleteServiceLinkedConfigurationRecorder's not-found message spells "ServicePrincipal" where AWS writes "Service Principal" ===
AWS: Smithy: DeleteServiceLinkedConfigurationRecorder models NoSuchConfigurationRecorderException (HTTP 400), doc "You have specified a configuration recorder that does not exist." Recorded behavior: deleting an already-deleted service-linked recorder is refused with "Configuration recorder (AccountId: <accountId>, Service Principal: <servicePrincipal>) cannot be found" — fixed text "Configuration recorder (AccountId: ", ", Service Principal: " (two words, with a space) and ") cannot be found", with accountId from the caller's account and servicePrincipal from the request.
Emulator: "Configuration recorder (AccountId: <accountId>, ServicePrincipal: <servicePrincipal>) cannot be found" — the label is the API member spelling, not AWS's prose spelling.
Why: the message text is a requirement and this one never matches AWS for any principal; this is the terminal "proves gone and unusable" read of the service-linked recorder lifecycle, so it is the leg clients assert on.
Fix: the NoSuchConfigurationRecorderException message template for the service-linked delete path (handler/state message helper).
Traces: config.service-linked-configuration-recorder-duplicate-conflict-rejected (expected '... Service Principal: ...', got '... ServicePrincipal: ...'); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/delete_service_linked_configuration_recorder.rs.

=== Cause 8 of 9: BatchGetAggregateResourceConfig's oversized-batch ValidationException summarises the rejected value as an item count ===
AWS: Smithy: BatchGetAggregateResourceConfig.ResourceIdentifiers is @length 1..100 and the operation models ValidationException (HTTP 400). Recorded behavior: a batch of 101 identifiers is refused with the framework constraint template "1 validation error detected: Value '<rendered value>' at 'resourceIdentifiers' failed to satisfy constraint: Member must have length less than or equal to 100", where <rendered value> is the LIST ITSELF rendered element by element — "[<elem>, <elem>, ...]" with one entry per submitted identifier, each element rendered as the service's object identity (com.amazonaws.starling.dove.AggregateResourceIdentifier@<hash>; the hash is a per-run value the check masks, the per-element rendering is not).
Emulator: "1 validation error detected: Value '101 items' at 'resourceIdentifiers' failed to satisfy constraint: Member must have length less than or equal to 100" (151 chars) — the Value slot carries a count summary instead of the rendered list, so the message is a different length and shape for every request size.
Why: the Value slot of this template is how AWS reports WHAT was rejected; a client that parses the template's Value (or just asserts the refusal text) sees a value AWS never emits, and the emulator's list-constraint renderer will differ the same way for every other list member it refuses.
Fix: the constraint-violation message renderer used for list members at the request boundary (render the member value element-wise, not as "<n> items"), reached from BatchGetAggregateResourceConfig's validation.
Traces: config.batch-get-aggregate-resource-config-malformed-batch-rejected; handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/batch_get_aggregate_resource_config.rs.

=== Cause 9 of 9: GetResourceEvaluationSummary reports emulator prose as the terminal FailureReason ===
AWS: Smithy: GetResourceEvaluationSummary returns EvaluationStatus (ResourceEvaluationStatus enum IN_PROGRESS|FAILED|SUCCEEDED) with a FailureReason, Compliance, EvaluationMode and the echoed ResourceDetails. Recorded behavior: a PROACTIVE StartResourceEvaluation terminates FAILED with one of AWS's two FIXED reasons — "No Config rule is enabled in proactive mode for the given resource type." when the account has no PROACTIVE-mode rule for the resource type (Compliance is NOT_APPLICABLE even then), and "Resource evaluation timed out." when a PROACTIVE rule IS active for the type but the on-demand evaluation is never invoked and idles out the submitted EvaluationTimeout. Neither sentence names the implementation or the rule.
Emulator: the terminal status is reached, but FailureReason is emulator-authored prose that describes the emulator: "the proactive evaluation could not be completed: this emulator does not execute the evaluation logic of the Config rule(s) <rule>, so no verdict was produced for the resource" (rule present case) and "the PROACTIVE evaluation could not be run: this account has no Config rule that evaluates in PROACTIVE mode, so there was no rule configuration to evaluate the resource details against" (no-rule case).
Why: FailureReason is the only machine-readable explanation of a FAILED evaluation; a client branching on AWS's two sentences cannot distinguish "no proactive rule" from "timed out", and the served text advertises that the responder is an emulator.
Fix: the terminal-state writer behind StartResourceEvaluation / GetResourceEvaluationSummary — emit the service's two fixed reasons for these two classes (and keep the timeout class gated on the submitted EvaluationTimeout).
Traces: config.proactive-resource-evaluation-reaches-terminal-state (expected "Resource evaluation timed out."), config.resource-evaluation-summary-reflects-proactive-evaluation (expected "No Config rule is enabled in proactive mode for the given resource type."); handlers services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_resource_evaluation.rs and src/ops/get_resource_evaluation_summary.rs.

## Evidence
Replay output, config service (each line is the failing assertion from the named check):

1) config.discovered-resource-counts-and-config-history — AssertionError: ('NoAvailableConfigurationRecorderException', 'there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one'); expected code ResourceNotDiscoveredException with message "Resource <id> of resourceType:AWS::S3::Bucket is unknown or has not been discovered".

2) config.get-aggregate-resource-config-not-discovered-rejected — AssertionError: got "The resource 'i-cf8b3c9faa1f42598' of type 'AWS::EC2::Instance' in account '000000000000' and region 'us-west-2' is either unknown or has not been discovered."; expected the fixed "You have specified a resource that is either unknown or has not been discovered."

3) config.delete-remediation-exceptions-nonexistent-rejected — AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'RemediationExceptions': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}; expected 400 NoSuchRemediationExceptionException "Specified RemediationException could not be found."

4) config.select-aggregate-resource-config-errors — AssertionError: "The expression is not a valid Config query: the expression is not a SQL SELECT command: sql parser error: Expected: ), found: EOF."; expected InvalidExpressionException "syntax error at line 1, column 19" for Expression "SELECT ((( invalid".

5) config.remediation-configuration-lifecycle — AssertionError: expected 'No RemediationConfiguration for rule <rule> exists', got "No RemediationConfiguration for rule '<rule>' exists."

6) config.put-service-linked-recorder-limit-exceeded-rejected — AssertionError: expected 'The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again.', got "The ServicePrincipal 'securityhub.amazonaws.com' provided in the request is not a valid service principal for a service-linked configuration recorder."

7) config.service-linked-configuration-recorder-duplicate-conflict-rejected — AssertionError: expected 'Configuration recorder (AccountId: <acct>, Service Principal: telemetry-enablement.observabilityadmin.amazonaws.com) cannot be found', got 'Configuration recorder (AccountId: <acct>, ServicePrincipal: telemetry-enablement.observabilityadmin.amazonaws.com) cannot be found'.

8) config.batch-get-aggregate-resource-config-malformed-batch-rejected — AssertionError: the refusal said "1 validation error detected: Value '101 items' at 'resourceIdentifiers' failed to satisfy constraint: Member must have length less than or equal to 100" (151 chars), expected "1 validation error detected: Value '[com.amazonaws.starling.dove.AggregateResourceIdentifier@<hash>, ... x101]' at 'resourceIdentifiers' failed to satisfy constraint: Member must have length less than or equal to 100".

9) config.proactive-resource-evaluation-reaches-terminal-state — AssertionError: the terminal FailureReason must be the evaluation timeout; got 'the proactive evaluation could not be completed: this emulator does not execute the evaluation logic of the Config rule(s) <rule>, so no verdict was produced for the resource'. config.resource-evaluation-summary-reflects-proactive-evaluation — AssertionError: unexpected FailureReason: 'the PROACTIVE evaluation could not be run: this account has no Config rule that evaluates in PROACTIVE mode, so there was no rule configuration to evaluate the resource details against'; expected "No Config rule is enabled in proactive mode for the given resource type."

## Steps to Reproduce
Boot the emulator and replay the behavioral suite; the ten checks listed in trace_ids all fail. Per cause, against the emulator endpoint with any credentials:

Cause 1: config get-resource-config-history --resource-type AWS::S3::Bucket --resource-id never-discovered-xyz -> observe NoAvailableConfigurationRecorderException instead of ResourceNotDiscoveredException.
Cause 2: put-configuration-aggregator (self account/region), then get-aggregate-resource-config with a made-up AWS::EC2::Instance id -> observe the request-derived message.
Cause 3: config delete-remediation-exceptions --config-rule-name never-existed --resource-keys ResourceType=AWS::EC2::Instance,ResourceId=nope -> observe HTTP 200 {'RemediationExceptions': []}.
Cause 4: select-aggregate-resource-config --configuration-aggregator-name <agg> --expression 'SELECT ((( invalid' -> observe the SQL-parser text.
Cause 5: delete-remediation-configuration --config-rule-name <rule with no remediation> -> observe the quoted/period message.
Cause 6: put-service-linked-configuration-recorder --service-principal securityhub.amazonaws.com -> observe the request-derived ValidationException message.
Cause 7: put-service-linked-configuration-recorder --service-principal telemetry-enablement.observabilityadmin.amazonaws.com, delete it, then delete it again -> observe 'ServicePrincipal:' in the not-found message.
Cause 8: batch-get-aggregate-resource-config with 101 ResourceIdentifiers -> observe Value '101 items'.
Cause 9: start-resource-evaluation --evaluation-mode PROACTIVE --resource-details ... (once with no PROACTIVE rule in the account, once with an ACTIVE CUSTOM_POLICY PROACTIVE rule for the type and EvaluationTimeout 60), then get-resource-evaluation-summary -> observe the emulator-authored FailureReason in both.
