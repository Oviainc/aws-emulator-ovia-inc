Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.rule-group-capacity-mismatch-rejected wafv2.delete-apikey-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.rule-group-capacity-mismatch-rejected|run#15|AssertionError|assert code == "WAFInvalidParameterException", code
Issue-Key: wafv2.delete-apikey-nonexistent-rejected|run#5|AssertionError|assert code == "WAFInvalidParameterException", (
Issue-Commit: e7fd84c70d759a793c8e402b7c8f23a100978d7e

# wafv2 rejection semantics: CreateRuleGroup under-declared Capacity raises WAFLimitsExceededException, and DeleteAPIKey answers NotImplemented

> _Note: this issue was identified against the code base at commit `e7fd84c70d759a793c8e402b7c8f23a100978d7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRuleGroup

## Description
This bundle has 2 causes, both net-new (neither check is marked ALREADY FILED or OWNED in the replay report). Both are wafv2 control-plane refusal paths that answer with the wrong modeled error, so an SDK client cannot classify the refusal the way it does against AWS.

Cause 1 of 2 — CreateRuleGroup with a Capacity below the measured WCU cost of its Rules is refused with the wrong error shape. Failing check: wafv2.rule-group-capacity-mismatch-rejected.
AWS: CreateRuleGroup models both WAFInvalidParameterException and WAFLimitsExceededException at httpError 400 (Smithy: CreateRuleGroup errors include WAFInvalidParameterException and WAFLimitsExceededException; CreateRuleGroup.Capacity -> CapacityUnit, required, range min 1). For an under-declared Capacity AWS answers 400 WAFInvalidParameterException, whose modeled members carry the diagnosis (Smithy WAFInvalidParameterException {message, Field: ParameterExceptionField, Parameter (length min 1), Reason: ErrorReason}; ParameterExceptionField includes the value RULE_GROUP). The refusal is request-derived and therefore a requirement: Field = RULE_GROUP, Parameter = the capacity the rules actually cost (the same figure CheckCapacity reports for those Rules, NOT the under-declared value the request sent), Reason = "You exceeded the capacity limit for a rule group or web ACL.", and message is composed from the three as "Error reason: <Reason>, field: <Field>, parameter: <Parameter>". WAFLimitsExceededException carries only {Message, SourceType} and is the account/resource-quota error ("you exceeded your resource limit... the maximum number of WebACL objects that you can create"), not the per-rule-group WCU check.
Emulator: the same request is refused 400 but with error code WAFLimitsExceededException, so Field/Parameter/Reason are absent entirely. The positive control in the same run (identical Rules declared at their measured Capacity) succeeds and GetRuleGroup reports the declared Capacity, so the handler does reach and enforce the capacity comparison — it just classifies the refusal as a quota error.
Why: a client that catches WafV2Exception::WafInvalidParameterException to correct an under-sized Capacity never sees it; it sees a quota exception, which signals "raise an account limit / delete resources" rather than "re-declare Capacity". Code that reads Field/Parameter to learn the required WCU figure gets nothing, so it cannot self-correct from the response.
Fix: the CreateRuleGroup handler's capacity enforcement and the error it constructs — services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/create_rule_group.rs lines 338-358 return WAFLimitsExceededException via crate::state::limits_exceeded_exception(); it should build a WAFInvalidParameterException (crate::state::invalid_parameter_exception with the RULE_GROUP field) carrying the required capacity as Parameter. The same comparison exists for UpdateRuleGroup/CreateWebACL paths and should classify identically.
Traces: wafv2.rule-group-capacity-mismatch-rejected (assertion `code == "WAFInvalidParameterException"` observed 'WAFLimitsExceededException').

Cause 2 of 2 — DeleteAPIKey is an unimplemented scaffold and answers error code 'NotImplemented' where AWS refuses a malformed key token. Failing check: wafv2.delete-apikey-nonexistent-rejected.
AWS: DeleteAPIKey models WAFInvalidParameterException (400), WAFNonexistentItemException (400), WAFInvalidOperationException (400), WAFOptimisticLockException (400) and WAFInternalErrorException (500) (Smithy: DeleteAPIKey input {Scope required, APIKey required, length 1..2048, pattern \S}). A token that CreateAPIKey never issued does not decode to a valid internal key structure, so it is refused 400 WAFInvalidParameterException — distinct from a well-formed-but-absent key, which is WAFNonexistentItemException. The message is request-derived and therefore a requirement: "Error reason: The parameter contains formatting that is not valid., field: null, parameter: <the APIKey value sent>" (Field absent, Parameter echoing the submitted token). The attempt has no side effect: ListAPIKeys is unchanged.
Emulator: DeleteAPIKey answers error code 'NotImplemented' (HTTP 501) for the same request. The handler is still the generated stub: services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/delete_api_key.rs returns Err(crate::scaffolded::Scaffolded::not_implemented("DeleteAPIKey")) and still carries the SCAFFOLDED marker, while ListAPIKeys in the same check answers normally.
Why: 'NotImplemented' is not a shape in the wafv2 model, so no SDK client maps it to a modeled WAFv2 error; a caller cleaning up CAPTCHA/API-key integrations cannot distinguish a malformed token from an absent one, and cannot delete a key at all. Related but NOT covering this: open issue 20261024T065754Z-wafv2-createapikey-stub.md names CreateAPIKey only, and open issue 20261024T114331Z-wafv2-rejection-semantics-missing-on-three-operations-tagresource.md names TagResource, PutPermissionPolicy and the Mobile-SDK family only — DeleteAPIKey is in neither.
Fix: implement the DeleteAPIKey handler (its own file, services/.../src/ops/delete_api_key.rs): decode/validate the APIKey token, refuse an undecodable token with WAFInvalidParameterException (Parameter = the submitted token), refuse a well-formed absent key with WAFNonexistentItemException, delete a real key from the wafv2 store, and remove the SCAFFOLDED marker.
Traces: wafv2.delete-apikey-nonexistent-rejected (assertion `code == "WAFInvalidParameterException"` observed 'NotImplemented').

The two causes live in different handlers and must both be closed: fixing only the capacity classification leaves DeleteAPIKey answering 501, and implementing DeleteAPIKey leaves the capacity refusal misclassified.

## Evidence
Behavioral replay of simparity-behavioral-ga-prod-v1 (44/112 passed), 2 net-new wafv2 clusters:

1. wafv2.rule-group-capacity-mismatch-rejected [mismatch]
   CheckCapacity(Scope=REGIONAL, Rules=[ByteMatch rule with 5 non-NONE text transformations]) -> measured capacity M (> 1).
   Positive control: CreateRuleGroup(Capacity=M, same Rules) succeeds; GetRuleGroup reports Capacity == M.
   Negative: CreateRuleGroup(Capacity=M-1, same Rules)
     expected: 400 error code 'WAFInvalidParameterException', Field 'RULE_GROUP', Parameter = M, Reason 'You exceeded the capacity limit for a rule group or web ACL.', message example "Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: <M>"
     actual:   400 error code 'WAFLimitsExceededException' (no Field/Parameter/Reason members on that shape)
     harness trace: check_wafv2_rule_group_capacity_mismatch_rejected.py line 134: `assert code == "WAFInvalidParameterException", code` -> AssertionError: WAFLimitsExceededException
   Emulator source: services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/create_rule_group.rs:342-358 (`if required_capacity > input.capacity { ... WAFLimitsExceededException ... }`).

2. wafv2.delete-apikey-nonexistent-rejected [mismatch]
   DeleteAPIKey(Scope=REGIONAL, APIKey='<token never issued by CreateAPIKey>')
     expected: 400 error code 'WAFInvalidParameterException', message "Error reason: The parameter contains formatting that is not valid., field: null, parameter: <token>"; ListAPIKeys count unchanged
     actual:   error code 'NotImplemented' (HTTP 501 scaffold)
     harness trace: check_wafv2_delete_apikey_nonexistent_rejected.py line 29: `assert code == "WAFInvalidParameterException"` -> AssertionError: expected WAFInvalidParameterException deleting a never-issued API key token, got 'NotImplemented'
   Emulator source: services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/delete_api_key.rs:173 `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteAPIKey"))`, with the `//! SCAFFOLDED, NOT IMPLEMENTED` marker at line 4.

Model grounding (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json): CreateRuleGroup errors include WAFInvalidParameterException and WAFLimitsExceededException (both client, httpError 400); WAFInvalidParameterException {message, Field: ParameterExceptionField, Parameter (length min 1), Reason: ErrorReason}; ParameterExceptionField enum contains RULE_GROUP; WAFLimitsExceededException {Message, SourceType} only. DeleteAPIKey input {Scope required, APIKey required len 1..2048 pattern \S}, errors {WAFInternalErrorException 500, WAFInvalidOperationException 400, WAFInvalidParameterException 400, WAFNonexistentItemException 400, WAFOptimisticLockException 400}; 'NotImplemented' is not a shape in the wafv2 model.

## Steps to Reproduce
1. Boot the emulator and replay check wafv2.rule-group-capacity-mismatch-rejected: call CheckCapacity(Scope='REGIONAL', Rules=[one ByteMatchStatement rule on UriPath with 5 non-NONE TextTransformations]) and note the returned Capacity M. Call CreateRuleGroup with Capacity=M and those Rules (succeeds). Then call CreateRuleGroup with a fresh Name, Capacity=M-1 and the identical Rules. Observe error code 'WAFLimitsExceededException' instead of 'WAFInvalidParameterException' with Field 'RULE_GROUP' / Parameter M / Reason 'You exceeded the capacity limit for a rule group or web ACL.'.
2. Replay check wafv2.delete-apikey-nonexistent-rejected: call ListAPIKeys(Scope='REGIONAL'), then DeleteAPIKey(Scope='REGIONAL', APIKey='sim2real-never-issued-api-key-token-000000000000'). Observe error code 'NotImplemented' (HTTP 501) instead of 400 'WAFInvalidParameterException' echoing the submitted token as Parameter, and confirm ListAPIKeys is unchanged.
3. Re-replay both check ids after the fix: wafv2.rule-group-capacity-mismatch-rejected and wafv2.delete-apikey-nonexistent-rejected.
