Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.invoke-guardrail-checks-structural-roundtrip bedrock-runtime.guardrail-checks-offsets-locate-entity-in-source-text
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.invoke-guardrail-checks-structural-roundtrip|run#9|AssertionError|assert scores["VIOLENCE"] > 0.0, (
Issue-Key: bedrock-runtime.guardrail-checks-offsets-locate-entity-in-source-text|run#5|AssertionError|assert {e["type"] for e in found} == {"EMAIL", "PHONE"}, (
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# InvokeGuardrailChecks is over-permissive: VIOLENCE/MISCONDUCT content filters always score 0.0, and the PII recognizer misses a seven-digit local PHONE number

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeGuardrailChecks

## Description
Two net-new root causes in the inline guardrail-checks evaluation path of bedrock-runtime. Both calls return HTTP 200 with a well-formed envelope, so this is NOT the refusal class covered by the fixed issue 20261023T232022Z-bedrock-runtime-invokeguardrailchecks-stub.md, nor the error-taxonomy class covered by the open 20261024T031932Z-bedrock-runtime-inline-guardrail-error-taxonomy-applyguardrail-and.md: the operation answers, and it answers wrongly. Both causes are the same failure mode -- a safety check that reports 'nothing found' for content AWS detects -- so a client that gates on these results admits content AWS blocks.

Cause 1 of 2: the content filter never places VIOLENCE or MISCONDUCT, so an explicit threat scores 0.0.
AWS: Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json) -- InvokeGuardrailChecks input GuardrailChecksContentFilterConfig.categories targets GuardrailChecksContentFilterCategory with values VIOLENCE, HATE, SEXUAL, MISCONDUCT, INSULTS; the result entry GuardrailChecksContentFilterResultEntry.severityScore is a required Double with @range min 0.0 max 1.0. The operation is documented as returning 'per-check results with severity or confidence scores', i.e. the score is a function of the submitted text, and Bedrock Guardrails publishes Violence and Misconduct as first-class content-filter categories. Recorded behavior: a message whose only content is an explicit threat of violence ('I am going to hurt you badly.') comes back with a non-zero VIOLENCE severityScore, strictly greater than the VIOLENCE score of unrelated benign text ('Hello, how has your day been?').
Emulator: both categories are answered at a hardcoded zero. The requested categories are echoed correctly ({VIOLENCE, HATE} in, {VIOLENCE, HATE} out) but every score is 0.0 -- replay shows results {'VIOLENCE': 0.0, 'HATE': 0.0} for the explicit threat. The evaluation maps only three of the five modelled categories onto a detector (HATE->OFFENSIVE, INSULTS->MEAN, SEXUAL->SEXUAL); VIOLENCE and MISCONDUCT have no detector and fall through to `.unwrap_or(0.0)`, so the structural echo is correct while the verdict is constant and text-independent.
Why: a client that uses InvokeGuardrailChecks as a pre-flight gate (the documented use of the operation) sees severityScore 0.0 for the two categories that cover threats and illicit-activity content, so it forwards to the model, logs or displays content real AWS scores above its threshold. The failure is silent and in the over-permissive direction, which is the worst direction for a safety API: nothing in the response distinguishes 'scored and clean' from 'never scored'.
Fix: belongs in the content-filter evaluation of this service's guardrail-checks engine -- the category-to-detector table and the severity derivation it feeds (see Traces for the file). VIOLENCE and MISCONDUCT need real recognizers over the submitted text so the score varies with the text and orders benign below threatening; answering a modelled category with a constant is not an implementation of it.
Traces: failing check bedrock-runtime.invoke-guardrail-checks-structural-roundtrip. Code: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/state/guardrail_checks.rs lines 329-333 (CONTENT_FILTER_DETECTORS, three entries only), 344-356 (category_severity) and 371-398 (content_filter_findings, `.unwrap_or(0.0)` at line 391); handler at src/ops/invoke_guardrail_checks.rs lines 278-290.

Cause 2 of 2: the sensitive-information check misses a seven-digit local PHONE number, reporting only EMAIL.
AWS: Smithy -- GuardrailChecksSensitiveInformationEntityType includes PHONE, and GuardrailChecksSensitiveInformationResultEntry carries required type/confidenceScore/beginOffset/endOffset/messageIndex/contentIndex. Recorded behavior: for one message whose second content block reads 'My email is jdoe@example.com and my phone is 555-0100.' with entities [EMAIL, PHONE] requested, the response carries one detection per entity -- exactly {EMAIL, PHONE} -- each addressing message 0, content block 1, with offsets that slice the entity out of that block's own text and confidenceScore in (0.0, 1.0]. A seven-digit local subscriber number written NNN-NNNN is a PHONE detection.
Emulator: only the EMAIL detection is returned; results carries 1 entry of type EMAIL and no PHONE entry, with results.sensitiveInformation.truncated=false. Cause: the sensitive-information check delegates to the shared PII recognizer, whose PHONE rules match only the three-part NANP form (optional +CC, then (NNN)/NNN separator, NNN, separator, NNNN) and the E.164 form (+ followed by 10-15 digits). A seven-digit local number matches neither, so no span is placed and the entity is silently absent from a response that is otherwise well-formed.
Why: the response shape is indistinguishable from 'this text contains no phone number', so a client redacting or refusing on PII forwards or stores a phone number AWS detects. Again over-permissive and silent: the per-entity result list has no 'not evaluated' marker, so a missing entry reads as a clean verdict.
Fix: belongs in the shared PII recognizer's PHONE rules -- the entity rule table that the bedrock-runtime sensitive-information check and Comprehend's PII family both read -- which must also place the local seven-digit subscriber form (and keep the offsets/messageIndex/contentIndex addressing the block the match was found in, which it already does for EMAIL).
Traces: failing check bedrock-runtime.guardrail-checks-offsets-locate-entity-in-source-text. Code: services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/contains_pii_entities.rs lines 342-350 (the two PHONE rules in detect_pii_spans); consumed via services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/state/guardrail_checks.rs lines 41 (import of detect_pii_spans), 544-613 (SENSITIVE_INFORMATION_ENTITY_TYPES / modelled_entity_type, which does map PHONE->PHONE, so the gap is in recognition and not in the mapping).

## Evidence
Cause 1 -- bedrock-runtime.invoke-guardrail-checks-structural-roundtrip:
  Request: InvokeGuardrailChecks POST /guardrail-checks/invoke, messages=[{role:'user', content:[{text:'I am going to hurt you badly.'}]}], checks={contentFilter:{categories:[{category:'VIOLENCE'},{category:'HATE'}]}}
  Expected (AWS): HTTP 200, results.contentFilter.results carries one entry per requested category and VIOLENCE.severityScore > 0.0, strictly greater than the VIOLENCE score of the benign control text.
  Actual (emulator): HTTP 200, categories echoed as {VIOLENCE, HATE} but scores are {'VIOLENCE': 0.0, 'HATE': 0.0}.
  Replay assertion: AssertionError: an explicit threat of violence scored zero on VIOLENCE: {'VIOLENCE': 0.0, 'HATE': 0.0}
  No status or error-code divergence: the call is a 200 on both sides; the divergence is the constant, text-independent severityScore for the two categories that have no detector (VIOLENCE, MISCONDUCT).

Cause 2 -- bedrock-runtime.guardrail-checks-offsets-locate-entity-in-source-text:
  Request: InvokeGuardrailChecks with messages=[{role:'user', content:[{text:'Booking confirmed.'},{text:'My email is jdoe@example.com and my phone is 555-0100.'}]},{role:'assistant', content:[{text:'Noted.'}]}], checks={sensitiveInformation:{entities:[{type:'EMAIL'},{type:'PHONE'}]}}
  Expected (AWS): HTTP 200 with results.sensitiveInformation.results carrying two detections, types exactly {EMAIL, PHONE}, each with messageIndex=0, contentIndex=1 and offsets slicing its own entity out of that block, confidenceScore in (0.0, 1.0], truncated=false.
  Actual (emulator): HTTP 200 with a single detection of type EMAIL; no PHONE entry at all.
  Replay assertion: AssertionError: expected exactly EMAIL and PHONE detections, got ['EMAIL'] over 1 entries
  Generalization: any phone number written as a seven-digit local subscriber number (NNN-NNNN) is missed; the literal '555-0100' and 'jdoe@example.com' above are request-supplied values filling the template, not part of the requirement. The requirement is: one detection per entity type the request named and the recognizer placed, addressed to the block it was found in.

## Steps to Reproduce
Boot the emulator and point a botocore bedrock-runtime client at it.

Cause 1:
1. brt.invoke_guardrail_checks(messages=[{'role':'user','content':[{'text':'I am going to hurt you badly.'}]}], checks={'contentFilter':{'categories':[{'category':'VIOLENCE'},{'category':'HATE'}]}})
2. Read {r['category']: r['severityScore'] for r in resp['results']['contentFilter']['results']}.
3. Observe VIOLENCE == 0.0 (emulator); AWS returns a non-zero VIOLENCE score here.
4. Repeat with the benign text 'Hello, how has your day been?' and observe the identical all-zero scores, i.e. the score does not vary with the text. Same for a request naming MISCONDUCT.

Cause 2:
1. brt.invoke_guardrail_checks(messages=[{'role':'user','content':[{'text':'Booking confirmed.'},{'text':'My email is jdoe@example.com and my phone is 555-0100.'}]},{'role':'assistant','content':[{'text':'Noted.'}]}], checks={'sensitiveInformation':{'entities':[{'type':'EMAIL'},{'type':'PHONE'}]}})
2. Read sorted(e['type'] for e in resp['results']['sensitiveInformation']['results']).
3. Observe ['EMAIL'] (emulator); AWS returns ['EMAIL','PHONE'], both addressing messageIndex=0 / contentIndex=1.

Or replay the two checks directly: bedrock-runtime.invoke-guardrail-checks-structural-roundtrip and bedrock-runtime.guardrail-checks-offsets-locate-entity-in-source-text.
