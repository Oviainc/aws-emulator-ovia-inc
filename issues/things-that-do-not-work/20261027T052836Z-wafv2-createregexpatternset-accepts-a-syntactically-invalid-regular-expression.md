Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.regexpatternset-invalid-syntax-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.regexpatternset-invalid-syntax-rejected|_error#4|AssertionError|raise AssertionError(
Issue-Commit: 3744c0b73d0da990856091a9dde679d904464991

# wafv2 CreateRegexPatternSet accepts a syntactically invalid regular expression (200 + set created) instead of refusing it with WAFInvalidParameterException

> _Note: this issue was identified against the code base at commit `3744c0b73d0da990856091a9dde679d904464991`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRegexPatternSet

## Description
One net-new root cause in the wafv2 regex-pattern-set control plane, grounded in a wafv2-token behavioral check that the suite marked neither ALREADY FILED nor OWNED.

Related issues that do NOT cover it: 20261024T065754Z-wafv2-createregexpatternset-stub.md (now fixed) only asked for the operation to stop answering 501 — the operation now answers, and answers 200 to input AWS refuses, which is a different defect and not a refusal, so this is not a reopen. 20261022T220318Z-wafv2-parameter-validation-returns-validationexception-instead-of-wafinvalidparameterexception.md (fixed) is about the error CLASS chosen for parameters that are already rejected; here no rejection happens at all. 20261026T120000Z-wafv2-two-refusals-carry-the-right-class-but-not-wafs-own-sentence.md (open) covers the Challenge-immunity and managed-rule-set version-key refusal MESSAGES, not regex syntax.

Cause 1 of 1: CreateRegexPatternSet persists a RegularExpressionList entry whose RegexString is not a parseable regular expression.
Failing check: wafv2.regexpatternset-invalid-syntax-rejected
AWS: the validity of each regular expression is a service-side semantic check, because the model deliberately does not constrain the expression's syntax. Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json): CreateRegexPatternSet input members Name (EntityName, required, @length 1..128, @pattern ^[\w\-]+$), Scope (required enum), RegularExpressionList (target RegularExpressionList, required, list of Regex); Regex.RegexString targets RegexPatternString with @length min 1 max 512 and @pattern ".*" — i.e. every byte sequence of legal length passes the modeled constraint, so an unparseable expression reaches the service and the service is what refuses it. Modeled errors on the operation include WAFInvalidParameterException (errorKind client, httpError 400), whose members are Field (enum ParameterExceptionField, whose values include REGEX_PATTERN_SET), Parameter (@length min 1, "The invalid parameter that resulted in the exception") and Reason ("Additional information about the exception"); the shape doc states the operation fails "because WAF didn't recognize a parameter in the request... You specified a parameter name or value that isn't valid." The recorded refusal for this class is HTTP 400 with Error.Code "WAFInvalidParameterException" and Error.Message "Error reason: The parameter contains formatting that is not valid., field: REGEX_PATTERN_SET, parameter: <the offending RegexString>". In that message the FIXED text is "Error reason: ", the Reason sentence "The parameter contains formatting that is not valid.", ", field: ", the Field token REGEX_PATTERN_SET, and ", parameter: " (note the comma-space separators and the full stop inside the Reason clause); the only VARIABLE part is the tail, which is the offending RegularExpressionList[].RegexString echoed verbatim from the request. Because the refusal happens before anything is persisted, ListRegexPatternSets(Scope) must not report a set under that Name afterwards.
Emulator: the call SUCCEEDS. The replay shows CreateRegexPatternSet(Name=<name>, Scope=REGIONAL, RegularExpressionList=[{RegexString: "(unterminated["}]) answering HTTPStatusCode 200 with a full Summary (Name, Id, Description "", LockToken, ARN arn:aws:wafv2:<region>:<account>:regional/regexpatternset/<name>/<id>), so the 400 / WAFInvalidParameterException pair, its Field/Reason/Parameter members and its message are never produced, and a set holding an unparseable pattern is stored under that name. The submitted expression has both an unclosed group and an unclosed character class, so no regex engine accepts it; the check's positive control in the same run creates the SAME name with "^[a-z]+$" and requires it to succeed and round-trip, which pins the difference to the expression's syntax and not to the request shape, the name or the scope.
Why: a client that typos a pattern is told at CreateRegexPatternSet time by AWS and fixes it immediately. Against this emulator the typo is accepted, and the damage surfaces later and elsewhere: a web ACL rule whose RegexPatternReferenceStatement points at the set can never match the traffic the author intended, so requests the author believed were inspected pass unexamined, with no error anywhere to attribute it to. Negative tests that assert the refusal — and tooling that validates user-supplied patterns by round-tripping them through WAF — pass against the emulator and then fail in production.
Fix: the validation stage of the wafv2 CreateRegexPatternSet handler (and the same rule in the UpdateRegexPatternSet handler, which takes the identical RegularExpressionList): compile every RegularExpressionList[].RegexString before any state is written, and on the first expression that does not compile refuse with HTTP 400 WAFInvalidParameterException carrying Field=REGEX_PATTERN_SET, Reason="The parameter contains formatting that is not valid.", Parameter=<that RegexString> and the message template above, so that no set is created and ListRegexPatternSets still does not list the name.
Traces: wafv2.regexpatternset-invalid-syntax-rejected (mismatch).

## Evidence
Cause 1 — check wafv2.regexpatternset-invalid-syntax-rejected [mismatch]:
  File "check_wafv2_regexpatternset_invalid_syntax_rejected.py", line 20, in run
    code, message = _error(lambda: wafv2.create_regex_pattern_set(
        Name=name, Scope=scope, RegularExpressionList=[{"RegexString": "(unterminated["}],
    ))
  File "check_wafv2_regexpatternset_invalid_syntax_rejected.py", line 63, in _error
    raise AssertionError(
        f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}"
    )
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'Summary': {'Name': '<generated name>', 'Id': '<generated uuid>', 'Description': '', 'LockToken': '<generated uuid>', 'ARN': 'arn:aws:wafv2:<region>:<account>:regional/regexpatternset/<name>/<id>'}, 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', ...}, 'RetryAttempts': 0}}

Expected (AWS): HTTP 400, Error.Code = "WAFInvalidParameterException", Error.Message = "Error reason: The parameter contains formatting that is not valid., field: REGEX_PATTERN_SET, parameter: <the RegexString submitted>" — fixed text through "parameter: ", variable tail copied verbatim from RegularExpressionList[].RegexString; the exception also carries Field = "REGEX_PATTERN_SET", Reason = "The parameter contains formatting that is not valid." and Parameter = that same RegexString. Nothing is created, so ListRegexPatternSets(Scope=REGIONAL) reports no set with that Name.
Actual (emulator): HTTP 200 with a populated Summary; no error code, no message, no Field/Reason/Parameter, and a regex pattern set holding the unparseable expression is persisted and listable under that Name. The positive control in the same check — identical call, same Name, RegexString "^[a-z]+$" — is accepted and round-trips through GetRegexPatternSet, so the expression's syntax is the only difference between the call AWS refuses and the call AWS accepts.

Note on per-run values: the Name, Id, LockToken and ARN account/region above are generated per run and are not part of the requirement; the requirement is the 400/WAFInvalidParameterException pair, its Field/Reason/Parameter members, the message template, and the absence of any created set.

## Steps to Reproduce
1. wafv2 CreateRegexPatternSet(Name="sim2real-behav-regex-bad-<suffix>", Scope="REGIONAL", RegularExpressionList=[{"RegexString": "(unterminated["}]).
   Observe HTTP 200 with a Summary (Name/Id/Description/LockToken/ARN) instead of HTTP 400 WAFInvalidParameterException with Error.Message "Error reason: The parameter contains formatting that is not valid., field: REGEX_PATTERN_SET, parameter: (unterminated[" and Field=REGEX_PATTERN_SET / Reason="The parameter contains formatting that is not valid." / Parameter="(unterminated[".
2. wafv2 ListRegexPatternSets(Scope="REGIONAL") — observe a set named "sim2real-behav-regex-bad-<suffix>" present, where a refused create must leave nothing behind.
3. Positive control: wafv2 CreateRegexPatternSet with the SAME Name and RegularExpressionList=[{"RegexString": "^[a-z]+$"}] — must be accepted, and GetRegexPatternSet must round-trip RegularExpressionList as [{"RegexString": "^[a-z]+$"}] verbatim.
4. Same check against UpdateRegexPatternSet: create a valid set, then UpdateRegexPatternSet with RegularExpressionList=[{"RegexString": "(unterminated["}] and a fresh LockToken — the same 400/WAFInvalidParameterException refusal is required, with the stored list and LockToken unchanged.
5. Or replay the check id in trace_ids.
