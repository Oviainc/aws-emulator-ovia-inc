Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-ecc-traj-get-products-request-mistakes
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-ecc-traj-get-products-request-mistakes
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing|GetProducts|4xx->4xx|$.Message
Issue-Commit: 8a15654a3e32a09451431e59a712f0f10794d652

# pricing GetProducts 400 validation refusals carry no error Message

> _Note: this issue was identified against the code base at commit `8a15654a3e32a09451431e59a712f0f10794d652`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProducts

## Description
One net-new root cause in the Pricing (AWS Price List Service) GetProducts input-validation path. The emulator refuses malformed GetProducts requests with the right outcome — HTTP 400, client-fault error — but the refusal body carries NO Message member at all, so every one of these refusals reaches the client with an empty reason. Six consecutive steps of the request-mistakes fixture cluster on exactly this key (status 400 both ways, $.Message missing), which is why this is one renderer-level cause rather than six handler bugs.

Cause 1 of 1: a constraint-violation refusal of GetProducts is serialized without its Message.
AWS: Smithy GetProducts (model com.amazonaws.pricing#AWSPriceListService, protocol awsJson1_1) takes required ServiceCode, Filters (list 0..50 of Filter{Type required, targeting the FilterType enum whose modeled values are TERM_MATCH, EQUALS, CONTAINS, ANY_OF, NONE_OF; Field required len 1..1024; Value required len 1..1024}), FormatVersion (len 1..32) and MaxResults (range 1..100), and models InvalidParameterException (errorKind client, httpError 400) for "one or more parameters had an invalid value". AWS answers a constraint violation with the standard single-error validation sentence in the body's Message member. For the recorded enum violation that sentence is:
  `1 validation error detected: Value 'EXACT' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [TERM_CONTAIN, ANY_OF, TERM_MATCH, NONE_OF, CONTAINS, EQUALS]`
Its FIXED text is `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy enum value set: [`, `]`, and the enum value set itself, which the service renders from its own server-side FilterType set (the recorded set includes TERM_CONTAIN in addition to the five values this model file lists, in the service's own order). Its VARIABLE parts come from the request: the rejected value (recorded run: 'EXACT') and the wire path of the offending member (recorded run: 'filters.1.member.type' — the lowerCamel member path including the list index of the bad element). Only the fixed text and the rule are requirements; the recorded value and index are that request's data.
Emulator: status 400 and the error classification match (the replay flags nothing but the body member), but `$.Message` is absent entirely — got None, not a differently worded sentence. The same thing happens for all six malformed GetProducts requests in the fixture, so it is the shared constraint-validation refusal serializer on the pre-handler path, not any one check: the GetProducts handler's own refusals do build messages (services/service-com-amazonaws-pricing-awspricelistservice/src/ops/get_products.rs lines 170-210 set InvalidParameterException.message for an unsupported FormatVersion and for an unknown filter Type), and the handler comments note that framework constraint validation refuses an unmodeled enum value BEFORE the handler runs — which is the path that emits no message.
Why: the message is the only part of a 400 that tells a client WHICH member it got wrong. With it absent, botocore surfaces `An error occurred (InvalidParameterException) when calling the GetProducts operation:` with an empty reason, a retry/validation wrapper that greps the message for the member path or the legal enum set finds nothing, and a human debugging a 50-filter request is told only that something in it was invalid. Code that only checks the error code still passes, so the gap is invisible until someone reads the text.
Fix: the shared input-validation refusal path for this crate — the constraint-violation arm that builds the InvalidParameterException body for GetProducts input (the pre-handler validate step, not the handler), which must populate Message with the `1 validation error detected: ...` sentence for each constraint kind (enum value set, required, length, range) instead of emitting a body with no Message. Fixing it inside the GetProducts handler would not help: these requests never reach the handler.
Traces: pricing-ecc-traj-get-products-request-mistakes#task#1, and the same cluster at #task#2, #task#3, #task#4, #task#5 and #task#6.

Related but distinct open issue: 20261029T205548Z-pricing-renders-pattern-validation-messages-with-the-regex.md reports that PATTERN-constraint messages are rendered with the regex anchors included — i.e. that path does emit a message and only its text is wrong. This issue is the different failure of the same family: for GetProducts constraint violations no Message is emitted at all, so fixing the anchor trimming does not fix it.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200): 40 steps, 15 passed, 24 mismatched, score 37.5%.

pricing-ecc-traj-get-products-request-mistakes#task#1 (GetProducts, expected 400, got 400), divergence key pricing|GetProducts|status=400->400|keys=$.Message:
  $.Message: missing. Expected "1 validation error detected: Value 'EXACT' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [TERM_CONTAIN, ANY_OF, TERM_MATCH, NONE_OF, CONTAINS, EQUALS]"; got None.
  Status and error classification matched (the replay flags no status or type difference) — the body simply has no Message member.
  Affected (same cluster, 6 members): pricing-ecc-traj-get-products-request-mistakes#task#1, #task#2, #task#3, #task#4, #task#5, #task#6 — i.e. every malformed GetProducts request in that fixture is refused messagelessly.

Code corroboration that the messageless path is the pre-handler validation rather than the handler: services/service-com-amazonaws-pricing-awspricelistservice/src/ops/get_products.rs lines 193-196 state that "the model declares Filter.Type as an enum, so the framework's constraint validation refuses an unknown type before this handler runs", and the handler's own defensive refusal (lines 197-210) does set InvalidParameterException.message. The emulator's answer carries no message, so the framework arm is the one that served these six steps.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or point a botocore 'pricing' client at the emulator with region='us-east-1' (the Price List Query API is served from us-east-1, eu-central-1 and ap-south-1).
3. Fixture pricing-ecc-traj-get-products-request-mistakes, step task#1 — send GetProducts with a Filters list whose SECOND element carries an unmodeled Type, e.g. Filters=[{'Type':'TERM_MATCH','Field':'ServiceCode','Value':'AmazonEC2'},{'Type':'EXACT','Field':'instanceType','Value':'t3.nano'}] (bypass client-side validation by posting the awsJson1_1 body directly, target AWSPriceListService.GetProducts).
4. Read the response: HTTP 400 with the client-fault error type, but the JSON body has no 'Message' member, where AWS returns Message = "1 validation error detected: Value 'EXACT' at 'filters.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [...]".
5. Steps task#2..task#6 of the same fixture send the other GetProducts request mistakes and are all refused 400 with no Message — re-replay them to confirm the fix covers the whole constraint-validation renderer, not just the enum arm.
