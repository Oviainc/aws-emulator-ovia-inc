Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.config-rule-custom-policy-details-validated config.overly-permissive-aggregator-invalid-account-id config.tag-key-and-value-contract-limits-rejected config.tag-resource-lifecycle
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.config-rule-custom-policy-details-validated|_error#17|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: config.overly-permissive-aggregator-invalid-account-id|run#12|AssertionError|assert message == (
Issue-Key: config.tag-key-and-value-contract-limits-rejected|run#16|AssertionError|assert message == expected_message, (
Issue-Key: config.tag-resource-lifecycle|run#21|AssertionError|assert _mask_java_identities(message) == expected_message, (
Issue-Commit: 97b52288e8c9303330ae8e79df94bfb75ff84489

# config: 6 net-new behavioral root causes — PutConfigRule accepts an invalid CUSTOM_POLICY rule, and four ValidationException renderings / two tag refusals diverge (TagResource, PutConfigurationAggregator)

> _Note: this issue was identified against the code base at commit `97b52288e8c9303330ae8e79df94bfb75ff84489`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConfigRule

## Description
SIX distinct root causes on this service's request-validation / refusal paths, grounded in four failing behavioral checks. SIX CAUSES must be closed before submitting.

None of these are covered by an existing open issue. The nearest neighbours were checked and do NOT cover them: 20261030T003924Z-config-recorder-tagging-and-retention-refusals-answer-wrong.md covers only the *not-found* wording of TagResource/ListTagsForResource (ResourceNotFoundException text), not any ValidationException rendering or the aws:/duplicate/50-tag refusals; 20261022T190340Z-config-putconfigrule-and-putdeliverychannel-are-overly-permissive-inconsistent.md covers PutConfigRule's Scope/frequency and managed-SourceIdentifier legs, not the CUSTOM_POLICY legs below.

--- Cause 1 of 6: PutConfigRule accepts a CUSTOM_POLICY rule that carries no CustomPolicyDetails (check config.config-rule-custom-policy-details-validated).
AWS: PutConfigRule models InvalidParameterValueException (client, httpError 400, defaulted) (Smithy: PutConfigRule.errors = InsufficientPermissionsException, InvalidParameterValueException, MaxNumberOfConfigRulesExceededException, NoAvailableConfigurationRecorderException, ResourceInUseException). CustomPolicyDetails is the Source member that carries the runtime and policy document for an Owner=CUSTOM_POLICY rule (Smithy: CustomPolicyDetails.PolicyRuntime required, @length 1..64, @pattern ^guard\-2\.x\.x$; CustomPolicyDetails.PolicyText required, @length 0..10000). Recorded behavior: a PutConfigRule whose ConfigRule.Source has Owner=CUSTOM_POLICY and no CustomPolicyDetails is refused 400 InvalidParameterValueException with the fixed, request-independent sentence "CustomPolicyDetails is required when Owner is CUSTOM_POLICY" (no variable parts), and the rule does not come into existence (DescribeConfigRules for it then answers NoSuchConfigRuleException and GetCustomRulePolicy answers NoSuchConfigRuleException).
Emulator: answers HTTP 200 with an empty JSON body (content-type application/x-amz-json-1.1, content-length 2, i.e. `{}`) and creates the rule. The check's helper asserts a ClientError and reports "expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
Why: a client that relies on Config refusing an incomplete CUSTOM_POLICY definition instead gets a stored rule with no policy document — a rule that can never evaluate anything, and whose GetCustomRulePolicy has nothing to return. The over-permissive accept also means the caller never learns which member is missing.
Fix: PutConfigRule's handler/input validation — add the Owner↔CustomPolicyDetails cross-field requirement (Owner=CUSTOM_POLICY requires CustomPolicyDetails; it is not a single-member constraint the boundary validator can express) and refuse before any state is written, so no rule and no policy document is persisted.

--- Cause 2 of 6: PutConfigRule does not refuse a CUSTOM_POLICY rule whose PolicyRuntime is not guard-2.x.x, nor one whose PolicyText is not valid Guard (same check; these two legs are in the same loop as cause 1 and were NOT reached because the loop aborted on cause 1 — they must be closed for the check to pass).
AWS: PolicyRuntime carries a Smithy @pattern (^guard\-2\.x\.x$), so a bad runtime is caught by the generic request validator and comes back 400 ValidationException naming the member path: fixed text "1 validation error detected: Value '" + the submitted runtime + "' at 'configRule.source.customPolicyDetails.policyRuntime' failed to satisfy constraint: Member must satisfy regular expression pattern: guard\\-2\\.x\\.x" (the quoted value is from the request; the path and the trailing constraint sentence are fixed). An unparseable PolicyText is Config's own domain check: 400 InvalidParameterValueException, fixed text "The PolicyText provided is invalid. Runtime: " + the runtime from the request + ", Reason: Parsing error handling rule, Error = " + the Guard parser diagnostic, then a blank line, "Error occurred " + the same diagnostic, then "\n." — the diagnostic embeds the offending fragment from the request (recorded behavior; the model states only the error class).
Emulator: not exercised past cause 1, but the handler performs no Owner/CustomPolicyDetails semantic validation at all, so neither refusal exists; both legs are expected to answer 200 `{}` as in cause 1.
Why: two of the three CUSTOM_POLICY refusal legs share InvalidParameterValueException, so the message is the only signal that tells a client what to correct; serving 200 instead means a syntactically invalid Guard policy is stored and silently never evaluates.
Fix: PutConfigRule's validation — honour the modeled PolicyRuntime pattern on the nested member (do not skip nested validation for ConfigRule.Source.CustomPolicyDetails), and add a Guard parse step for PolicyText that emits InvalidParameterValueException with AWS's template above.

--- Cause 3 of 6: PutConfigurationAggregator's malformed-account-id ValidationException reports the wrong member path and the wrong constraint form (check config.overly-permissive-aggregator-invalid-account-id).
AWS: AccountAggregationSources[].AccountIds is AccountAggregationSourceAccountList, whose member targets AccountId with @pattern ^\\d{12}$ (Smithy: AccountAggregationSource.AccountIds required, @length min 1; AccountId @pattern ^\\d{12}$). The recording shows 400 ValidationException whose message is reported at the LIST level and aggregates all three failed constraints in one bracketed clause: fixed text "1 validation error detected: Value '[" + the submitted ids + "]' at 'accountAggregationSources.1.member.accountIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: \\d{12}]" — the bracketed value list and the 1-based source index come from the request; the path tail and the three-constraint bracket are fixed.
Emulator: same status (400) and same code (ValidationException) but a different message — it reports the offending ELEMENT path and only one constraint: "1 validation error detected: Value '12345' at 'accountAggregationSources.1.member.accountIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}" (no surrounding brackets on the value, an extra '.1.member' path segment, and the two length constraints missing).
Why: the code alone does not identify which member was rejected, so clients and operators read the member path out of the message; the emulator's path names a shape position AWS never reports, and the dropped length constraints hide that a 12-digit length is the requirement.
Fix: PutConfigurationAggregator's input validation / the shared ValidationException renderer — when every failure is on members of one list, report it once at the list's path with the bracketed value list and the aggregated bracketed constraint set, as AWS does.

--- Cause 4 of 6: TagResource's ValidationException truncates the offending value instead of echoing it in full (check config.tag-key-and-value-contract-limits-rejected).
AWS: Tag$Value targets TagValue with @length 0..256 and Tag$Key targets TagKey with @length 1..128 (Smithy: shape Tag). The recording shows a 257-character value refused 400 ValidationException with fixed text "1 validation error detected: Value '" + the FULL submitted value + "' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256" — the value is echoed whole, however long it is; the path and the constraint sentence are fixed. (The sibling 129-character key leg already matches, so the divergence is specific to the echo, not the path or the constraint sentence.)
Emulator: same status and code, but the echoed value is clipped and a byte count is appended: "...bbbbbbb... [257 bytes]' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256". The marker " ... [N bytes]" is emulator-authored and appears nowhere in AWS's rendering.
Why: the four tag refusal legs share one error code, so the message is the only thing that distinguishes them; a client (or an operator) that reads the rejected value out of the message gets a prefix and a byte count instead of the value it sent, and any assertion on AWS's sentence fails.
Fix: the shared ValidationException value renderer used by this service's boundary validation — do not clip or annotate the echoed value.

--- Cause 5 of 6: TagResource does not refuse a tag key using the reserved `aws:` prefix (same check; leg NOT reached because the loop aborted on cause 4, but it must be closed for the check to pass).
AWS: 400 ValidationException with the fixed, request-independent sentence "One or more parameter value is invalid" — the offending key is NOT echoed (recorded behavior; the model states only the error class, ValidationException, httpError 400).
Emulator: services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/tag_resource.rs performs only an ARN-shape check, a blank-key check and a per-resource quota check; no reserved-prefix check exists, so `aws:`-prefixed keys are merged into the resource's tag set and the call answers 200.
Why: a caller can write into AWS's reserved tag namespace, and the resource ends up carrying tags real Config would never have stored — the refusal is also required to be atomic (the existing tag set must be unchanged).
Fix: TagResource's validation — refuse any requested key whose prefix is `aws:` with ValidationException and AWS's fixed sentence, before any merge into state.

--- Cause 6 of 6: TagResource silently de-duplicates a key repeated in one call instead of refusing it, and the 50-tag list-length refusal renders the value as a count (checks config.tag-key-and-value-contract-limits-rejected and config.tag-resource-lifecycle).
AWS: a request naming the same tag key twice is refused 400 ValidationException with the fixed sentence "Duplicate tags exists in tags list" (recorded behavior). Exceeding the modeled list maximum is likewise a ValidationException: Tags targets TagList with @length 1..50 (Smithy: TagResource.input.Tags, TagList), and the recording shows 400 with fixed text "1 validation error detected: Value '[" + one echoed element per submitted tag, comma-space separated + "]' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50" — i.e. the value is the BRACKETED LIST of echoed elements, one per submitted tag, not a count. (AWS renders each element as a Java object identity, `com.amazonaws.starling.dove.Tag@<hash>`; the hash is a per-run value and is masked by the check, so the requirement is the bracketed one-element-per-tag list and the surrounding fixed text, not any particular hash.)
Emulator: for the duplicate-key leg it builds a key→value map (crate::state::tag_set) so the last occurrence silently wins and the call answers 200 — there is no duplicate refusal at all. For the 51-tag leg it answers the right status and code but renders the value as a count: "1 validation error detected: Value '51 items' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50" (134 chars) — '51 items' is emulator-authored and the bracketed element list is absent.
Why: silently collapsing duplicates means a client's tag write is partially discarded with a 200; and a client reading the rejected collection out of the list-length message gets a summary string instead of the collection, so message-keyed handling and any parity assertion fail.
Fix: TagResource's validation (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/tag_resource.rs) for the duplicate-key refusal — detect a repeated key on the input list BEFORE the map collapse and refuse with AWS's fixed sentence; and the shared ValidationException renderer for list-length failures — emit the bracketed per-element value list instead of a '<n> items' summary. Both refusals must be atomic (the existing tag set unchanged).

Traces (failing check ids, as examples): config.config-rule-custom-policy-details-validated (causes 1, 2), config.overly-permissive-aggregator-invalid-account-id (cause 3), config.tag-key-and-value-contract-limits-rejected (causes 4, 5, 6), config.tag-resource-lifecycle (cause 6).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (117/176 passed; bootstrap_signal=false, so the emulator was serving). Service `config` is in get_bootstrap_status().bootstrapped.

Cause 1 — config.config-rule-custom-policy-details-validated [mismatch]:
  The check's control legs PASSED: a well-formed CUSTOM_POLICY rule reached ACTIVE, kept Source.Owner=CUSTOM_POLICY, acquired no SourceIdentifier, and GetCustomRulePolicy returned the Guard document byte-for-byte. The failure is on the first refusal leg.
  File "check_config_config_rule_custom_policy_details_validated.py", line 355, in run -> line 69, in _error
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2'}, 'RetryAttempts': 0}}
  Request: PutConfigRule ConfigRule={Scope:{TagKey:<inert>}, Source:{Owner:'CUSTOM_POLICY', SourceDetails:[...]}}  (no CustomPolicyDetails)
  expected: 400 InvalidParameterValueException, message 'CustomPolicyDetails is required when Owner is CUSTOM_POLICY'
  got:      200, body '{}'

Cause 2 — same check, legs 2 and 3 of the same loop (not reached, aborted on cause 1):
  leg 2 request: Source.CustomPolicyDetails={PolicyRuntime:'guard-1.x.x', PolicyText:<valid guard>}
    expected: 400 ValidationException "1 validation error detected: Value 'guard-1.x.x' at 'configRule.source.customPolicyDetails.policyRuntime' failed to satisfy constraint: Member must satisfy regular expression pattern: guard\\-2\\.x\\.x"
  leg 3 request: Source.CustomPolicyDetails={PolicyRuntime:'guard-2.x.x', PolicyText:'this is not guard syntax {{{ <<< '}
    expected: 400 InvalidParameterValueException "The PolicyText provided is invalid. Runtime: guard-2.x.x, Reason: Parsing error handling rule, Error = <guard parser diagnostic>\n\nError occurred <same diagnostic>\n."
  After each refusal the check also requires DescribeConfigRules -> NoSuchConfigRuleException "One or more ConfigRules provided in the request are invalid. Please check the configRule names." and GetCustomRulePolicy -> NoSuchConfigRuleException "The ConfigRule '<name>' provided in the request is invalid. Please check the configRule name." (rule name from the request).

Cause 3 — config.overly-permissive-aggregator-invalid-account-id [mismatch]:
  File "check_config_overly_permissive_aggregator_invalid_account_id.py", line 66, in run
  Request: PutConfigurationAggregator ConfigurationAggregatorName=<name>, AccountAggregationSources=[{AccountIds:['12345'], AllAwsRegions:true}]
  code: ValidationException on both sides (that assertion passed).
  expected message: "1 validation error detected: Value '[12345]' at 'accountAggregationSources.1.member.accountIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: \\d{12}]"
  got message:      "1 validation error detected: Value '12345' at 'accountAggregationSources.1.member.accountIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}"
  (A well-formed aggregator was created first as a positive control and succeeded, so the divergence is specific to the malformed-id rendering.)

Cause 4 — config.tag-key-and-value-contract-limits-rejected [mismatch]:
  Controls PASSED: a 128-char key + 256-char value round-tripped byte-exact; leg 1 (129-char key) matched AWS exactly.
  File "check_config_tag_key_and_value_contract_limits_rejected.py", line 96, in run
  Request: TagResource ResourceArn=<aggregator arn>, Tags=[{Key:'k', Value:<257 chars: 'v' + 'b'*256>}]
  expected: "1 validation error detected: Value '<the full 257-character value>' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256"
  got:      "1 validation error detected: Value 'vbbbb…bbb... [257 bytes]' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256"
  (code ValidationException on both sides; the sole difference is the clipped echo plus the emulator-authored ' ... [257 bytes]' marker.)

Cause 5 — same check, leg 3 (not reached):
  Request: TagResource Tags=[{Key:'aws:<suffix>', Value:'v'}]
  expected: 400 ValidationException, message 'One or more parameter value is invalid'
  emulator: no reserved-prefix check in services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/tag_resource.rs (handle() validates only the ARN shape, a blank key, and the per-resource tag quota), so the key is merged and the call answers 200.

Cause 6 — same check leg 4, plus config.tag-resource-lifecycle [mismatch]:
  leg 4 request: TagResource Tags=[{Key:'dupe', Value:'first'}, {Key:'dupe', Value:'second'}]
    expected: 400 ValidationException, message 'Duplicate tags exists in tags list'
    emulator: crate::state::tag_set(&input.tags) collapses the list into a key->value map (tag_resource.rs:204), so 'second' wins and the call answers 200.
  config.tag-resource-lifecycle: File "check_config_tag_resource_lifecycle.py", line 152, in run — tag/untag round-trip on a config-rule ARN PASSED; the 51-tag refusal diverges.
    Request: TagResource ResourceArn=<config rule arn>, Tags=[{Key:'k0'..'k50', Value:'v'}]  (51 tags)
    code ValidationException on both sides.
    expected: "1 validation error detected: Value '[<51 comma-separated echoed elements>]' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50"  (elements rendered by AWS as com.amazonaws.starling.dove.Tag@<hash>; hashes are per-run and masked by the check)
    got:      "1 validation error detected: Value '51 items' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50"  (134 chars)

Per-run values (aggregator/rule/recorder names from ctx.unique_name, resource ARNs, account id, Java object hashes, timestamps, request ids) are not part of any requirement — the requirements are the statuses, the error codes and the message templates above, with the named variable parts taken from the request.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): PutConfigRule models InvalidParameterValueException (400) among its errors; CustomPolicyDetails.PolicyRuntime is required with @length 1..64 and @pattern ^guard\\-2\\.x\\.x$, CustomPolicyDetails.PolicyText required @length 0..10000; AccountAggregationSource.AccountIds targets AccountAggregationSourceAccountList whose member is AccountId @pattern ^\\d{12}$; TagResource.Tags targets TagList @length 1..50 with member Tag{Key: TagKey @length 1..128, Value: TagValue @length 0..256} and models ResourceNotFoundException, TooManyTagsException and ValidationException (all 400).

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1.
3. Cause 1: run check config.config-rule-custom-policy-details-validated — PutConfigRule with ConfigRule.Source.Owner='CUSTOM_POLICY' and no CustomPolicyDetails; observe 200 '{}' instead of 400 InvalidParameterValueException 'CustomPolicyDetails is required when Owner is CUSTOM_POLICY', and observe the rule afterwards resolves in DescribeConfigRules.
4. Cause 2: in the same check, the two following legs — PutConfigRule with CustomPolicyDetails.PolicyRuntime='guard-1.x.x' (expect 400 ValidationException naming 'configRule.source.customPolicyDetails.policyRuntime'), and with PolicyText='this is not guard syntax {{{ <<< ' (expect 400 InvalidParameterValueException with the Guard-parse template). After each, DescribeConfigRules and GetCustomRulePolicy for that name must answer NoSuchConfigRuleException.
5. Cause 3: run check config.overly-permissive-aggregator-invalid-account-id — PutConfigurationAggregator with AccountAggregationSources=[{AccountIds:['12345'],AllAwsRegions:true}]; compare the ValidationException message against the list-level, three-constraint form.
6. Cause 4: run check config.tag-key-and-value-contract-limits-rejected — TagResource with a 257-character tag value; confirm the message echoes the value in full with no ' ... [N bytes]' marker.
7. Cause 5: same check — TagResource with Tags=[{Key:'aws:<anything>',Value:'v'}]; expect 400 ValidationException 'One or more parameter value is invalid' and an unchanged tag set.
8. Cause 6: same check — TagResource with the key 'dupe' twice in one call; expect 400 ValidationException 'Duplicate tags exists in tags list' and an unchanged tag set. Then run check config.tag-resource-lifecycle and submit 51 tags in one TagResource call; confirm the message's Value is the bracketed per-element list, not '51 items'.
9. Re-replay all four check ids in trace_ids to confirm the fix: config.config-rule-custom-policy-details-validated, config.overly-permissive-aggregator-invalid-account-id, config.tag-key-and-value-contract-limits-rejected, config.tag-resource-lifecycle.
