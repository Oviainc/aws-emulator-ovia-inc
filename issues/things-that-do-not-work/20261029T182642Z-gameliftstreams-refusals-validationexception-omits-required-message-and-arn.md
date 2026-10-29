Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-account-wide-reads-and-tag-identifier-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-account-wide-reads-and-tag-identifier-validation
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|ListStreamSessionsByAccount|4xx->4xx|$.Message
Issue-Key: gameliftstreams|ListStreamUrls|4xx->4xx|$.Message
Issue-Key: gameliftstreams|AssociateApplications|4xx->4xx|$.Message
Issue-Key: gameliftstreams|AssociateApplications|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: gameliftstreams|AssociateApplications|4xx->4xx|status
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# gameliftstreams refusals: ValidationException omits required Message and ARN-type validation answers 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStreamSessionsByAccount

## Description
Three net-new causes, all in how the gameliftstreams emulator builds client-error responses: the ValidationException body is emitted without its required Message member, and one identifier-validation path is classified as a not-found instead of a validation failure. All three share one root fault family — the refusal envelope/classification layer — and must all be closed before this issue is submitted.

Cause 1 of 3: ListStreamSessionsByAccount — range-constrained MaxResults refusal carries no Message.
AWS: Smithy model (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json) — ListStreamSessionsByAccount.MaxResults targets MaxResults with @range(min: 1, max: 100) and is bound @httpQuery("MaxResults"); the operation models ValidationException (errorKind client, httpError 400), whose Message member is @required. A MaxResults above the modeled maximum is refused 400 ValidationException with the standard AWS constraint-violation template. Fixed text of that template: "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must have value less than or equal to <max>". Variable parts taken from the request and the model: the error count, the rejected value, the input member name as the service spells it on the wire (lowerCamel, e.g. 'maxResults'), and the modeled maximum (100). The recorded response shows that template instantiated for this operation.
Emulator: answers the correct 400, but the JSON body has no Message member at all ($.Message missing, got None), so the required member of the modeled error shape is absent.
Why: a client that inspects or surfaces the error cannot tell WHICH parameter it got wrong or what the limit is — SDKs render an empty message, and any retry logic or user-facing text keyed on the message gets nothing. The modeled shape says Message is required, so deserializers may also treat the body as malformed.
Fix: the gameliftstreams crate's input-constraint validation / ValidationException serializer — the path that turns a @range violation into the 400 must populate Message with the constraint-violation template.
Traces: gameliftstreams-account-wide-reads-and-tag-identifier-validation#task#2.

Cause 2 of 3: ListStreamUrls — enum-constrained Status refusal carries no Message.
AWS: Smithy model — ListStreamUrls.Status targets the StreamUrlStatus enum (values ACTIVE, EXPIRED, REVOKED, LIMIT_REACHED) bound @httpQuery("Status"); the operation models ValidationException (400) with @required Message. A query value outside the enum is refused 400 ValidationException with the same template, in its enum form. Fixed text: "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy enum value set: [<enum values>]". Variable parts from the request and the model: the rejected value, the wire member name (e.g. 'status'), and the enum value set as the service enumerates it (the recording shows the service emitting it as EXPIRED, ACTIVE, LIMIT_REACHED, REVOKED).
Emulator: returns 400 with no Message member ($.Message missing, got None).
Why: same client-visible consequence as cause 1 — an enum typo is reported as an unexplained 400, and the caller is never told which values are legal.
Fix: same component — the shared constraint-validation/ValidationException serializer for enum-valued query members.
Traces: gameliftstreams-account-wide-reads-and-tag-identifier-validation#task#4.

Cause 3 of 3: AssociateApplications — an Identifier that is not a usable stream-group ARN is answered as a missing resource instead of a validation error.
AWS: Smithy model — AssociateApplications.Identifier is @required, @httpLabel, length 1..128, pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:]*):([0-9]{12}):([^:]*)$)$; the operation models BOTH ValidationException (400) and ResourceNotFoundException (404). AWS validates the identifier's ARN resource type BEFORE resolving the resource, so an ARN that does not name a well-formed 'streamgroup' resource is refused 400 ValidationException. Fixed text of that message: "'<memberName>' is not a valid resource ARN of type '<resourceType>'"; the variable parts are the input member name ('Identifier') and the expected resource type ('streamgroup'), both taken from the operation/request — not from any one run.
Emulator: answers 404 with x-amzn-errortype: ResourceNotFoundException and an emulator-authored message of the form "Resource '<arn>' does not exist (Internal ID: <uuid>)" — a wrong status, a wrong error class, and message text AWS never emits (the parenthesised internal UUID is emulator-local and has no AWS counterpart).
Why: the two errors mean opposite things to a caller. 404 ResourceNotFoundException tells a client the ARN was understood and the group is gone (so it may create it, or stop retrying a specific id), whereas AWS is saying the ARN is malformed for this parameter. Error-class-driven control flow and retry policies therefore take the wrong branch, and the leaked internal id makes the message unusable for assertions.
Fix: the AssociateApplications handler — validate/classify the Identifier label's ARN resource type in the operation's input validation, before the state lookup that produces the not-found, and emit ValidationException with AWS's wording rather than the emulator's resource-missing text.
Traces: gameliftstreams-account-wide-reads-and-tag-identifier-validation#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (fixture gameliftstreams-account-wide-reads-and-tag-identifier-validation):

- task#2 ListStreamSessionsByAccount: status expected 400 / got 400, but $.Message missing — expected "1 validation error detected: Value '9999' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100", got None.
- task#4 ListStreamUrls: status expected 400 / got 400, but $.Message missing — expected "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [EXPIRED, ACTIVE, LIMIT_REACHED, REVOKED]", got None.
- task#8 AssociateApplications: status expected 400, got 404; $.Message expected "'Identifier' is not a valid resource ARN of type 'streamgroup'", got "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/not-a-group' does not exist (Internal ID: 2100614f-3e05-8d7f-597a-37017b3579c2)"; $headers.x-amzn-errortype got ResourceNotFoundException where the recording asserts a ValidationException-shaped refusal.

Model evidence: ValidationException {Message: String @required}, httpError 400; ResourceNotFoundException httpError 404; ListStreamSessionsByAccount.MaxResults @range(1..100); ListStreamUrls.Status -> StreamUrlStatus enum; AssociateApplications errors include both ValidationException and ResourceNotFoundException.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture gameliftstreams-account-wide-reads-and-tag-identifier-validation.
3. Step task#2: GET /streamsessions?MaxResults=9999 — observe HTTP 400 with a JSON body that contains no Message member.
4. Step task#4: GET /streamurls?Status=BOGUS — observe HTTP 400 with a JSON body that contains no Message member.
5. Step task#8: POST /streamgroups/{Identifier}/associations with an Identifier that is not a valid 'streamgroup' resource ARN — observe HTTP 404 with x-amzn-errortype: ResourceNotFoundException and an emulator-authored "does not exist (Internal ID: ...)" message, where AWS answers 400 ValidationException "'Identifier' is not a valid resource ARN of type 'streamgroup'".
