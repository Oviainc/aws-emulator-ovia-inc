Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.config-rule-requires-recorder config.delivery-channel-lifecycle config.delivery-channel-requires-recorder config.start-recorder-without-delivery-channel config.op-delete-delivery-channel-nonexistent config.delivery-channel-delete-blocked-while-recording config.op-start-nonexistent-recorder config.configuration-recorder-max-number-exceeded config.config-rule-lifecycle config.concurrent-delete-config-rule-race config.third-party-service-linked-recorder-creation-validation config.delete-pending-aggregation-request-malformed-identifiers-rejected config.stored-query-invalid-name-rejected
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.config-rule-requires-recorder|run#9|AssertionError|assert message == "You must create a configuration recorder before you can create or update a Config rule.", message
Issue-Key: config.delivery-channel-lifecycle|run#9|AssertionError|assert message == "Configuration recorder is not available to put delivery channel.", message
Issue-Key: config.delivery-channel-requires-recorder|run#10|AssertionError|assert message == "Configuration recorder is not available to put delivery channel.", message
Issue-Key: config.start-recorder-without-delivery-channel|run#9|AssertionError|assert message == "Delivery channel is not available to start configuration recorder.", message
Issue-Key: config.op-delete-delivery-channel-nonexistent|run#18|AssertionError|assert message == f"Cannot find delivery channel with specified name '{missing}'.", message
Issue-Key: config.delivery-channel-delete-blocked-while-recording|run#14|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: config.op-start-nonexistent-recorder|run#5|AssertionError|assert message == (
Issue-Key: config.configuration-recorder-max-number-exceeded|run#11|AssertionError|assert code_message == (
Issue-Key: config.config-rule-lifecycle|run#24|AssertionError|assert code_err.get("Message") == expected_message, f'expected the message {expected_message!r}, got {code_err.get("Message")!r}'
Issue-Key: config.concurrent-delete-config-rule-race|run#20|AssertionError|assert gone.get("Message") == (
Issue-Key: config.third-party-service-linked-recorder-creation-validation|run#9|AssertionError|assert message == expected_message, message
Issue-Key: config.delete-pending-aggregation-request-malformed-identifiers-rejected|run#6|AssertionError|assert message == (
Issue-Key: config.stored-query-invalid-name-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Config error-message rules diverge on ten paths: recorder/delivery-channel preconditions, not-found and limit messages are the emulator's own prose, and the validation formatter renders anchors, echoes the value and mis-names the member path

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliveryChannel

## Description
On these ten paths the emulator returns the correct HTTP status and the correct error CODE, but a different message than AWS produces. Each one is a message whose production rule is known: it is either a fixed constant AWS emits for that error class (measured on real AWS by the check that carries it) or it is built from the request (it interpolates the recorder/channel/rule/query name or renders the violated constraint). The rule is therefore the requirement, not the recorded text. The last two causes are in the shared request-validation formatter and affect every operation that validates a member. The open issue "Config error paths unenforced: delivery-channel validation and absence answer 200 instead of 400" (20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and) already carries the same formatter defect for DeletePendingAggregationRequest as its Cause 4; it does NOT carry the behavioral check keys below, nor the value-echo and member-path renderings. TEN CAUSES must be closed.

Cause 1 of 10: PutConfigRule with no configuration recorder.
AWS: PutConfigRule models NoAvailableConfigurationRecorderException (client, httpError 400; Smithy doc: "There are no customer managed configuration recorders available to record your resources. Use the PutConfigurationRecorder operation to create the customer managed configuration recorder."). Recorded behavior only for the text: AWS emits the fixed message "You must create a configuration recorder before you can create or update a Config rule."
Emulator: same 400 and same code, message "there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one" — its own prose paraphrase of the model doc.
Why: clients and operators that match on AWS's fixed per-class message (log scrapers, user-facing surfacing, test suites) see a string AWS never emits.
Fix: the PutConfigRule handler's recorder-precondition check — raise NoAvailableConfigurationRecorderException with the fixed AWS message for this path (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_config_rule.rs).
Traces: config.config-rule-requires-recorder.

Cause 2 of 10: PutDeliveryChannel with no configuration recorder.
AWS: PutDeliveryChannel models NoAvailableConfigurationRecorderException (client/400). Recorded behavior only for the text: the fixed message is "Configuration recorder is not available to put delivery channel."
Emulator: same 400/code, message "there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one" — the same generic string it uses for Cause 1, so the two distinct AWS messages are collapsed into one.
Why: the message no longer identifies which operation's precondition failed, and does not match AWS.
Fix: the PutDeliveryChannel handler's recorder-precondition check (.../src/ops/put_delivery_channel.rs).
Traces: config.delivery-channel-lifecycle, config.delivery-channel-requires-recorder.

Cause 3 of 10: StartConfigurationRecorder with no delivery channel.
AWS: StartConfigurationRecorder models NoAvailableDeliveryChannelException (client/400; Smithy doc: "There is no delivery channel available to record configurations."). Recorded behavior only for the text: the fixed message is "Delivery channel is not available to start configuration recorder."
Emulator: same 400/code, message "there is no delivery channel available to receive configuration items; use PutDeliveryChannel to create one".
Why: as above — a client matching AWS's fixed message for this class does not match.
Fix: the StartConfigurationRecorder handler's delivery-channel precondition (.../src/ops/start_configuration_recorder.rs).
Traces: config.start-recorder-without-delivery-channel.

Cause 4 of 10: DeleteDeliveryChannel for an absent channel.
AWS: DeleteDeliveryChannel models NoSuchDeliveryChannelException (client/400; Smithy doc: "You have specified a delivery channel that does not exist."). The message is built from the request — it quotes the requested channel name: "Cannot find delivery channel with specified name '<name>'."
Emulator: same 400/code, message "the delivery channel '<name>' cannot be deleted ..."-style prose; here "there is no delivery channel named '<name>'".
Why: the rendering rule differs, so clients that parse the AWS message (or assert on it) break even though the class matches.
Fix: the DeleteDeliveryChannel handler's not-found path message (.../src/ops/delete_delivery_channel.rs).
Traces: config.op-delete-delivery-channel-nonexistent.

Cause 5 of 10: DeleteDeliveryChannel while the recorder is running.
AWS: models LastDeliveryChannelDeleteFailedException (client/400; Smithy doc: "You cannot delete the delivery channel you specified because the customer managed configuration recorder is running."). The message is built from the request and quotes the channel name: "Failed to delete last specified delivery channel with name '<name>', because there is a running configuration recorder."
Emulator: same 400/code, message "the delivery channel '<name>' cannot be deleted because the customer managed configuration recorder is running".
Why: same as Cause 4.
Fix: the DeleteDeliveryChannel handler's running-recorder conflict path (.../src/ops/delete_delivery_channel.rs).
Traces: config.delivery-channel-delete-blocked-while-recording.

Cause 6 of 10: StartConfigurationRecorder for a recorder that does not exist.
AWS: models NoSuchConfigurationRecorderException (client/400; Smithy doc: "You have specified a configuration recorder that does not exist."). Recorded behavior only for the text: the fixed message is "The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again."
Emulator: same 400/code, message "there is no configuration recorder named '<name>'".
Why: same as above; also note the emulator correctly does NOT auto-create the recorder, so only the message rule is at fault here.
Fix: the StartConfigurationRecorder handler's not-found path (.../src/ops/start_configuration_recorder.rs).
Traces: config.op-start-nonexistent-recorder.

Cause 7 of 10: PutConfigurationRecorder when the single recorder slot is occupied.
AWS: models MaxNumberOfConfigurationRecordersExceededException (client/400; Smithy doc: "You have reached the limit of the number of configuration recorders you can create."). Recorded behavior only for the text: AWS's message states the limit and reads "Failed to put configuration recorder  because you have reached the limit for the maximum number of customer managed configuration records: (1)" (note AWS's own double space, and that the limit value comes from the service).
Emulator: same 400/code, message "this account already has a customer managed configuration recorder in us-west-2; only one can exist for each account for each Region" — it also bakes a region into the message, which AWS's rendering does not.
Why: the message neither matches AWS nor states the limit in AWS's form, and embedding the region makes it environment-dependent.
Fix: the PutConfigurationRecorder handler's singleton-limit path (.../src/ops/put_configuration_recorder.rs).
Traces: config.configuration-recorder-max-number-exceeded.

Cause 8 of 10: DescribeConfigRules for a rule that does not exist.
AWS: DescribeConfigRules raises NoSuchConfigRuleException (client/400). The message rule is the PLURAL form keyed to the request shape (ConfigRuleNames is a list): "One or more ConfigRules provided in the request are invalid. Please check the configRule names."
Emulator: same 400/code, but renders the SINGULAR per-name form, "The ConfigRule '<name>' provided in the request is invalid. Please check the configRule name."
Why: clients that assert on, or surface, the message AWS returns for a list-shaped lookup get a different string; the divergence also appears on the asynchronous delete path, where the rule's final disappearance is observed through exactly this message.
Fix: the DescribeConfigRules handler's not-found path message rule (.../src/ops/describe_config_rules.rs).
Traces: config.config-rule-lifecycle, config.concurrent-delete-config-rule-race.

Cause 9 of 10: the shared validation-message formatter renders @pattern anchors and echoes the offending value.
AWS: a constraint violation is a 400 whose message is built from the request — it names the member and the constraint it failed. AWS renders the regular expression WITHOUT the ^ / $ anchors, and for this member does NOT echo the offending value: "1 validation error detected: Value at 'servicePrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w+=,.@-]+". Because the construction rule is known (member + constraint, taken from the request), that rendering is the requirement.
Emulator: same 400 and same ValidationException class, but renders the raw model pattern with anchors and echoes the value: "1 validation error detected: Value 'not a valid principal!!' at 'servicePrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w+=,.@-]+$". The same anchored rendering appears on DeletePendingAggregationRequest.RequesterAccountId (pattern ^\d{12}$ rendered with anchors instead of \d{12}).
Why: every client or suite that reads the constraint out of an AWS validation message gets a different pattern string, and a value AWS withholds is leaked into the message.
Fix: the shared request-validation message formatter used by these operations' validation — strip the leading ^ / trailing $ when rendering a @pattern, and follow AWS's rule for when the value is echoed. NOTE: issue 20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and already names the anchor-stripping half of this formatter defect (its Cause 4, DeletePendingAggregationRequest); close them together.
Traces: config.third-party-service-linked-recorder-creation-validation, config.delete-pending-aggregation-request-malformed-identifiers-rejected.

Cause 10 of 10: the validation formatter names the member by its internal path instead of AWS's member name.
AWS: PutStoredQuery with a QueryName violating ^[a-zA-Z0-9-_]+$ is rejected 400 and the message names the member as AWS does: "1 validation error detected: Value 'sim2real invalid name ...' at 'QueryName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$" (message built from the request, so the rendering is the requirement; ValidationException's model doc explicitly lists PutStoredQuery input-validation failures).
Emulator: same 400/class but names the member by its nested path, "at 'storedQuery.queryName'".
Why: a client that maps the member named in the message back to its own request field gets a path AWS never emits.
Fix: the validation-message formatter's member-path rendering for members nested under a wrapper structure (PutStoredQuery's StoredQuery), in that operation's validation (.../src/ops/put_stored_query.rs and the shared formatter).
Traces: config.stored-query-invalid-name-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false). In every cause below the HTTP status and the error code matched; only the message rule diverged.

Cause 1 — config.config-rule-requires-recorder:
  expected "You must create a configuration recorder before you can create or update a Config rule."
  got      "there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one"

Cause 2 — config.delivery-channel-lifecycle and config.delivery-channel-requires-recorder (identical failure):
  expected "Configuration recorder is not available to put delivery channel."
  got      "there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one"

Cause 3 — config.start-recorder-without-delivery-channel:
  expected "Delivery channel is not available to start configuration recorder."
  got      "there is no delivery channel available to receive configuration items; use PutDeliveryChannel to create one"

Cause 4 — config.op-delete-delivery-channel-nonexistent:
  expected "Cannot find delivery channel with specified name '<channel>'."
  got      "there is no delivery channel named '<channel>'"

Cause 5 — config.delivery-channel-delete-blocked-while-recording:
  expected "Failed to delete last specified delivery channel with name '<channel>', because there is a running configuration recorder."
  got      "the delivery channel '<channel>' cannot be deleted because the customer managed configuration recorder is running"

Cause 6 — config.op-start-nonexistent-recorder:
  expected "The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again."
  got      "there is no configuration recorder named '<recorder>'"

Cause 7 — config.configuration-recorder-max-number-exceeded:
  expected "Failed to put configuration recorder  because you have reached the limit for the maximum number of customer managed configuration records: (1)"
  got      "this account already has a customer managed configuration recorder in us-west-2; only one can exist for each account for each Region"

Cause 8 — config.config-rule-lifecycle:
  expected "One or more ConfigRules provided in the request are invalid. Please check the configRule names."
  got      "The ConfigRule '<rule>' provided in the request is invalid. Please check the configRule name."
  config.concurrent-delete-config-rule-race: same divergence, observed on the post-delete lookup — AssertionError: {'Message': "The ConfigRule '<rule>' provided in the request is invalid. Please check the configRule name.", 'Code': 'NoSuchConfigRuleException'}.

Cause 9 — config.third-party-service-linked-recorder-creation-validation:
  expected "1 validation error detected: Value at 'servicePrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w+=,.@-]+"
  got      "1 validation error detected: Value 'not a valid principal!!' at 'servicePrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w+=,.@-]+$"
  config.delete-pending-aggregation-request-malformed-identifiers-rejected:
  got      "1 validation error detected: Value 'not-an-acct1' at 'requesterAccountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$" (AWS renders the pattern as \d{12})

Cause 10 — config.stored-query-invalid-name-rejected:
  expected "... Value 'sim2real invalid name ...' at 'QueryName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$"
  got      "... at 'storedQuery.queryName' ..."

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): NoAvailableConfigurationRecorderException, NoAvailableDeliveryChannelException, NoSuchDeliveryChannelException, LastDeliveryChannelDeleteFailedException, NoSuchConfigurationRecorderException, MaxNumberOfConfigurationRecordersExceededException, NoSuchConfigRuleException and ValidationException are all errorKind client with httpError 400 (defaulted); each carries a single `message` member.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
3. Cause 1: with no configuration recorder in the account, call PutConfigRule — 400 NoAvailableConfigurationRecorderException with the emulator's own prose.
4. Cause 2: with no recorder, call PutDeliveryChannel — same generic message as step 3 instead of "Configuration recorder is not available to put delivery channel."
5. Cause 3: with a recorder but no delivery channel, call StartConfigurationRecorder.
6. Causes 4/5: call DeleteDeliveryChannel for an absent channel, then for the only channel while the recorder is running.
7. Cause 6: call StartConfigurationRecorder with a recorder name that does not exist.
8. Cause 7: create one configuration recorder, then PutConfigurationRecorder a second one.
9. Cause 8: call DescribeConfigRules with ConfigRuleNames naming a rule that does not exist.
10. Cause 9: PutThirdPartyServiceLinkedConfigurationRecorder with ServicePrincipal='not a valid principal!!', and DeletePendingAggregationRequest with RequesterAccountId='not-an-acct1' — compare the rendered pattern and the echoed value.
11. Cause 10: PutStoredQuery with StoredQuery.QueryName='sim2real invalid name' — compare the member path named in the message.
