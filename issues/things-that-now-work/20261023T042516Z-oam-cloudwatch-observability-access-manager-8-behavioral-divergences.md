Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: oam.concurrent-delete-sink-race oam.delete-link-twice-not-found-after-deletion oam.create-sink-duplicate-name-conflict-effect-absent oam.create-sink-invalid-name-pattern-effect-absent oam.delete-sink-with-attached-link-conflict-rejected oam.get-sink-policy-absent-before-put oam.update-link-nonexistent-rejected oam.update-link-resource-types-replaces-not-merges oam.put-sink-policy-invalid-json-effect-absent oam.put-sink-policy-wildcard-principal-rejected
Issue-Shape-Id: com.amazonaws.oam#oamservice
Issue-Key: oam.concurrent-delete-sink-race|run#13|AssertionError|assert message == _NOT_FOUND, f"unexpected loser message: {message!r}"
Issue-Key: oam.delete-link-twice-not-found-after-deletion|run#7|AssertionError|assert code1_message == _NOT_FOUND, f"got message {code1_message!r}"
Issue-Key: oam.create-sink-duplicate-name-conflict-effect-absent|run#7|AssertionError|assert same_name_code == "ServiceQuotaExceededException", f"got {same_name_code!r}"
Issue-Key: oam.create-sink-invalid-name-pattern-effect-absent|run#4|AssertionError|assert code == "BadRequestException", f"got {code!r}"
Issue-Key: oam.delete-sink-with-attached-link-conflict-rejected|run#6|AssertionError|assert code == "AccessDeniedException", f"got {code!r}"
Issue-Key: oam.get-sink-policy-absent-before-put|run#8|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: oam.update-link-nonexistent-rejected|run#6|AssertionError|assert code == "AccessDeniedException", f"got {code!r}"
Issue-Key: oam.update-link-resource-types-replaces-not-merges|run#8|AssertionError|assert missing_code == "AccessDeniedException", f"got {missing_code!r}"
Issue-Key: oam.put-sink-policy-invalid-json-effect-absent|run#8|AssertionError|assert code == "InvalidParameterException", f"got {code!r}"
Issue-Key: oam.put-sink-policy-wildcard-principal-rejected|run#9|AssertionError|assert code == "InvalidParameterException", f"got {code!r}"
Issue-Commit: 6288a0cc473439e64f80da5d6e6b2c53557408ab

# OAM (CloudWatch Observability Access Manager): 8 behavioral divergences — sink quota modeled as a name conflict, wrong refusal classes on CreateSink/DeleteSink/UpdateLink, GetSinkPolicy and UpdateLink answer 501, and ResourceNotFoundException carries a synthesized message

> _Note: this issue was identified against the code base at commit `6288a0cc473439e64f80da5d6e6b2c53557408ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSink

## Description
Eight distinct root causes in the oam emulator (service-com-amazonaws-oam-oamservice), all found by replaying the behavioral suite (0/22 checks passed). A fixer must close ALL EIGHT numbered causes below before submitting. Note: the separately auto-filed scaffold-stub issues for PutSinkPolicy, ListSinks, ListLinks, TagResource and ListTagsForResource cover only 'implement the operation'; Cause 8 below records the request-validation semantics those stub issues do not state.

=== Cause 1 of 8: ResourceNotFoundException message is synthesized from the request instead of OAM's constant text ===
AWS: recorded behavior only (the Smithy model declares only `ResourceNotFoundException` {Message, x-amzn-ErrorType} at HTTP 404 with doc 'The request references a resource that does not exist.'; it states no message text). Across every OAM not-found path the suite records — DeleteSink, DeleteLink, GetSink, GetSinkPolicy, ListAttachedLinks — AWS answers HTTP 404, code `ResourceNotFoundException`, and the message is the fixed, identifier-independent string `Resource not found.`. The rule is therefore known: the message is a service-wide constant and never embeds the identifier.
Emulator: returns the correct 404/`ResourceNotFoundException` but builds a per-request message, e.g. `no sink matching 'arn:aws:oam:us-west-2:<acct>:sink/<uuid>' exists in this account and Region` (DeleteSink) and `no link matching 'arn:aws:oam:us-west-2:<acct>:link/<uuid>' exists in this account and Region` (DeleteLink).
Why: clients and test harnesses that match OAM's constant not-found text (and log scrubbers that assume the message carries no ARN) see an unexpected, identifier-bearing message.
Fix: the shared not-found error construction used by the OAM operation handlers/state layer — emit the constant message for ResourceNotFoundException rather than formatting the identifier into it.
Traces: oam.concurrent-delete-sink-race (DeleteSink losers of a 6-way race: exactly one win and 5 x ResourceNotFoundException is correct, only the message diverges), oam.delete-link-twice-not-found-after-deletion.

=== Cause 2 of 8: the one-sink-per-account/Region quota is implemented as a sink-name collision, with the wrong error code ===
AWS: Smithy: CreateSink errors include `ServiceQuotaExceededException` (errorKind client, httpError 429, doc 'The request would cause a service quota to be exceeded.') and `ConflictException` (409). Recorded behavior: once an account/Region has a sink, EVERY further CreateSink is refused with HTTP 429 `ServiceQuotaExceededException`, message of the form `Reached account sink max(1) for account(<accountId>).` — including a CreateSink with a DIFFERENT, unused Name. Deleting the existing sink frees the slot and a differently-named CreateSink then succeeds.
Emulator: the second CreateSink with the SAME name is refused with `ConflictException` (409), i.e. the rule enforced is name uniqueness, not the account sink quota. A differently-named CreateSink is therefore accepted where AWS refuses it (overly permissive, and the emulator can hold several sinks that cannot exist in one account/Region).
Why: clients that retry/branch on `ServiceQuotaExceededException` (429, throttling-class in several SDK retry policies) never see it, and code written against the single-sink invariant is silently allowed to create multiple sinks, so monitoring-account topology that is impossible on AWS passes against the emulator.
Fix: CreateSink's handler/validation plus the sink collection in the service state — enforce an account/Region cardinality of 1 and raise ServiceQuotaExceededException with the account-scoped quota message, independent of the requested Name.
Traces: oam.create-sink-duplicate-name-conflict-effect-absent (asserts got 'ConflictException' where ServiceQuotaExceededException was expected).

=== Cause 3 of 8: CreateSink Name pattern violation is refused with the wrong error class and message rule ===
AWS: Smithy: CreateSink.Name targets `SinkName` with @pattern `^[a-zA-Z0-9_\.\-]{1,255}$`, required. Recorded behavior only for the refusal class: a Name that violates the pattern (e.g. `bad name!`) is rejected by the REST front end with HTTP 400 and error code `BadRequestException` (not one of the operation's modeled errors), and the message follows a request-derived rule — `Invalid request body: [ECMA 262 regex "<the member's pattern>" does not match input string "<the supplied value>"]`. No sink is created.
Emulator: answers code `InvalidParameterException` (400) instead of `BadRequestException`; the message does not follow the ECMA-262-regex rule above.
Why: a client that distinguishes a malformed request (BadRequestException, never retryable, surfaced from the framework) from a semantic parameter error (InvalidParameterException) mis-classifies constraint violations, and SDK/CLI error handling keyed on the code branches wrongly.
Fix: the constraint-violation path of the shared request validator for this service (pre-handler @pattern enforcement) — classify modeled-constraint violations as BadRequestException with the regex/input message rule, rather than mapping them onto InvalidParameterException.
Traces: oam.create-sink-invalid-name-pattern-effect-absent (asserts got 'InvalidParameterException').

=== Cause 4 of 8: DeleteSink on a well-formed non-sink (link-shaped) ARN answers not-found instead of the resource-policy explicit deny ===
AWS: Smithy: DeleteSink.Identifier targets `ResourceIdentifier` @pattern `^[a-zA-Z0-9][a-zA-Z0-9_:\.\-\/]{0,2047}$`; modeled errors are ConflictException/InternalServiceFault/InvalidParameterException/MissingRequiredParameterException/ResourceNotFoundException. Recorded behavior only: an Identifier that satisfies the pattern but is not a sink ARN in this account (here an `arn:aws:oam:<region>:<account>:link/<uuid>`) is refused by access control BEFORE any sink lookup — HTTP 403 `AccessDeniedException` (a code the model does not declare), message built from the request as `User: <caller ARN> is not authorized to perform: oam:DeleteSink on resource: <the identifier supplied> with an explicit deny in a resource-based policy`.
Emulator: answers `ResourceNotFoundException`, i.e. it treats any oam-shaped ARN as an acceptable sink identifier and reports absence.
Why: the emulator leaks existence information AWS withholds and loses the authorization/ not-found distinction, so clients that branch on AccessDeniedException (permission problem, do not retry, surface to the operator) versus ResourceNotFoundException (resource genuinely gone, safe to treat as deleted) take the wrong branch — a delete-idempotency path will silently swallow an authorization failure.
Fix: DeleteSink's handler — gate on the identifier naming a sink resource of this account under the resource-policy evaluation path and return the explicit-deny AccessDeniedException, before the sink-table lookup that produces not-found.
Traces: oam.delete-sink-with-attached-link-conflict-rejected (asserts got 'ResourceNotFoundException' where AccessDeniedException was expected).

=== Cause 5 of 8: GetSinkPolicy is an unimplemented scaffold stub (HTTP 501 NotImplemented) ===
AWS: Smithy: GetSinkPolicy (POST /GetSinkPolicy, readonly) returns {SinkArn, SinkId, Policy} on 200 and models InternalServiceFault/InvalidParameterException/MissingRequiredParameterException/`ResourceNotFoundException` (404). Recorded behavior: on a real sink with NO policy yet attached, GetSinkPolicy is refused with 404 `ResourceNotFoundException` (message `Resource not found.`, per Cause 1); after a PutSinkPolicy it returns the stored document in `Policy` (statement contents preserved modulo service-assigned Sids) together with the sink's ARN.
Emulator: every GetSinkPolicy answers HTTP 501 with code `NotImplemented` ('operation GetSinkPolicy is scaffolded but not implemented in this emulator'). Not covered by any auto-filed stub issue in this suite run.
Why: the policy read-back half of OAM's whole cross-account story is unusable: a client cannot tell 'no policy yet' (404) from 'service error', and cannot read a policy it just wrote.
Fix: implement the handler in services/service-com-amazonaws-oam-oamservice/src/ops/get_sink_policy.rs (still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker, handler body returns `Scaffolded::not_implemented("GetSinkPolicy")`), reading the policy from the sink's state.
Traces: oam.get-sink-policy-absent-before-put (asserts want=ResourceNotFoundException have='NotImplemented').

=== Cause 6 of 8: UpdateLink is an unimplemented scaffold stub (HTTP 501 NotImplemented) ===
AWS: Smithy: UpdateLink (POST /UpdateLink) takes Identifier (required, ResourceIdentifier pattern) and ResourceTypes (required, @length 1..50, uniqueItems) and models InternalServiceFault/InvalidParameterException/MissingRequiredParameterException/ResourceNotFoundException. Recorded behavior only for the nonexistent-link path: UpdateLink against a well-formed but never-created link ARN is refused with HTTP 403 `AccessDeniedException`, message `User: <caller ARN> is not authorized to perform: oam:UpdateLink on resource: arn:aws:oam:<region>:<account>:link/* with an explicit deny in a resource-based policy` — while the read-only GetLink/DeleteLink on the same identifier report `ResourceNotFoundException`.
Emulator: every UpdateLink answers HTTP 501 `NotImplemented`. Not covered by any auto-filed stub issue in this suite run.
Why: the mutating half of the link lifecycle does not exist, and no refusal class (403 vs 404) can be observed for it at all.
Fix: implement the handler in services/service-com-amazonaws-oam-oamservice/src/ops/update_link.rs (carries the `SCAFFOLDED, NOT IMPLEMENTED` marker, body returns `Scaffolded::not_implemented("UpdateLink")`), including the explicit-deny refusal for an identifier that is not an existing link of this account.
Traces: oam.update-link-nonexistent-rejected (asserts got 'NotImplemented' where AccessDeniedException was expected).

=== Cause 7 of 8: on UpdateLink, pre-handler shape validation runs BEFORE the authorization gate ===
AWS: Smithy: UpdateLink.ResourceTypes is required with @length min 1, and MissingRequiredParameterException (400) is modeled. Recorded behavior only for the ORDERING: with the identifier naming a link that does not exist in this account, AWS answers the identical HTTP 403 `AccessDeniedException` explicit-deny (same message rule as Cause 6) whether ResourceTypes is omitted entirely, sent as an empty list, or sent well-formed — the access-control decision precedes any required-member/length check, so no shape error is ever observed on that identifier.
Emulator: with botocore parameter validation disabled so the request reaches the wire, omitting ResourceTypes yields `MissingRequiredParameterException` (and an empty list would likewise yield the length-constraint refusal), i.e. the centralized deserialize->validate step answers before the handler's authorization decision.
Why: a client probing permissions learns about its request shape instead of the authorization outcome; the refusal class and HTTP status for the same call differ from AWS (400 vs 403), so retry/alerting logic keyed on 403 never fires.
Fix: UpdateLink's handler/validation ordering for this service — the identifier-ownership (resource-policy) decision must be evaluated before required-member and list-length constraints are reported for this operation.
Traces: oam.update-link-resource-types-replaces-not-merges (asserts got 'MissingRequiredParameterException' where AccessDeniedException was expected).

=== Cause 8 of 8: PutSinkPolicy performs no policy-document validation, and the invalid-policy paths are unreachable ===
AWS: Smithy: PutSinkPolicy (POST /PutSinkPolicy) takes SinkIdentifier (required) + Policy (required, SinkPolicy) and models InvalidParameterException (400), MissingRequiredParameterException (400), ResourceNotFoundException (404), InternalServiceFault (500). Recorded behavior: a Policy body that is not valid JSON (`{not valid json`) and a syntactically valid policy that omits `Principal` are both refused with 400 `InvalidParameterException`, message `Policy validation failed`, and NO policy is attached (a following GetSinkPolicy still answers 404 ResourceNotFoundException); by contrast a correctly scoped statement with an unconditioned wildcard `Principal: "*"` IS accepted and is read back by GetSinkPolicy. PutSinkPolicy against a sink that has been deleted answers 404 ResourceNotFoundException.
Emulator: every PutSinkPolicy answers HTTP 501 `NotImplemented`, so neither the accept nor the refuse path exists. The scaffold-stub issue auto-filed for PutSinkPolicy covers only 'implement the operation'; this cause records the validation contract the implementation must satisfy (which document shapes are refused with InvalidParameterException/'Policy validation failed' and that a refused Put leaves no policy behind), so making the happy path work alone does not close it.
Why: an emulator that accepts any document would let a client ship a sink policy AWS rejects (or believe a rejected policy was attached), and the current 501 makes every cross-account linking scenario untestable.
Fix: services/service-com-amazonaws-oam-oamservice/src/ops/put_sink_policy.rs — parse/validate the policy document and store it on the sink atomically, refusing with InvalidParameterException and leaving state untouched.
Traces: oam.put-sink-policy-invalid-json-effect-absent, oam.put-sink-policy-wildcard-principal-rejected (both assert got 'NotImplemented' where InvalidParameterException was expected).

## Evidence
Suite: simparity-behavioral-aws-v1 — oam 0/22 (0%), 10 mismatched + 12 errored (the errored ones are blocked by already-filed scaffold stubs PutSinkPolicy/ListTagsForResource/TagResource/ListLinks/ListSinks and are not re-filed here).

Cause 1 — expected code ResourceNotFoundException + message 'Resource not found.'; got ResourceNotFoundException with message "no sink matching 'arn:aws:oam:us-west-2:000000000000:sink/beb1f886-...' exists in this account and Region" (DeleteSink) and "no link matching 'arn:aws:oam:us-west-2:000000000000:link/4750bd83-...' exists in this account and Region" (DeleteLink).
Cause 2 — expected ServiceQuotaExceededException (HTTP 429) + 'Reached account sink max(1) for account(<acct>).' on the 2nd CreateSink (same name AND different name); got 'ConflictException' (HTTP 409); a differently-named sink is accepted instead of refused.
Cause 3 — CreateSink(Name='bad name!'): expected BadRequestException (HTTP 400) + 'Invalid request body: [ECMA 262 regex "^[a-zA-Z0-9_\.\-]{1,255}$" does not match input string "bad name!"]'; got 'InvalidParameterException'.
Cause 4 — DeleteSink(Identifier='arn:aws:oam:<region>:<acct>:link/<uuid>'): expected AccessDeniedException (HTTP 403) + 'User: <caller> is not authorized to perform: oam:DeleteSink on resource: <identifier> with an explicit deny in a resource-based policy'; got 'ResourceNotFoundException'.
Cause 5 — GetSinkPolicy on a fresh sink: expected ResourceNotFoundException (HTTP 404); got 'NotImplemented' (HTTP 501, 'operation GetSinkPolicy is scaffolded but not implemented in this emulator'). File still carries '//! SCAFFOLDED, NOT IMPLEMENTED' (src/ops/get_sink_policy.rs:4) and returns Scaffolded::not_implemented at src/ops/get_sink_policy.rs:164.
Cause 6 — UpdateLink(Identifier=<nonexistent link ARN>, ResourceTypes=['AWS::CloudWatch::Metric']): expected AccessDeniedException (HTTP 403) + oam:UpdateLink explicit-deny message on 'arn:aws:oam:<region>:<acct>:link/*'; got 'NotImplemented' (HTTP 501). src/ops/update_link.rs:4 marker, Scaffolded::not_implemented at src/ops/update_link.rs:187.
Cause 7 — UpdateLink(Identifier=<nonexistent link ARN>) with ResourceTypes omitted (parameter_validation=False): expected AccessDeniedException (HTTP 403); got 'MissingRequiredParameterException' (HTTP 400).
Cause 8 — PutSinkPolicy(Policy='{not valid json') and PutSinkPolicy(policy without Principal): expected InvalidParameterException (HTTP 400) + 'Policy validation failed' with no policy attached (GetSinkPolicy still 404); got 'NotImplemented' (HTTP 501).

## Steps to Reproduce
Boot the emulator and point a botocore 'oam' client at it (region us-west-2), then:
1. Cause 2: oam.create_sink(Name='sink-a') -> 200; oam.create_sink(Name='sink-a') -> expect 429 ServiceQuotaExceededException 'Reached account sink max(1) for account(<acct>).', observe 409 ConflictException; oam.create_sink(Name='sink-b') -> expect the same 429, observe success.
2. Cause 3: oam.create_sink(Name='bad name!') -> expect 400 BadRequestException with the ECMA-262 regex message, observe InvalidParameterException.
3. Cause 1: create a sink, then run 6 concurrent oam.delete_sink(Identifier=<sink arn>) — one wins, the 5 losers must be 404 ResourceNotFoundException with message exactly 'Resource not found.'; observe the synthesized "no sink matching '<arn>' ..." message. Same for oam.delete_link(Identifier='arn:aws:oam:us-west-2:<acct>:link/<uuid>') called twice.
4. Cause 4: oam.delete_sink(Identifier='arn:aws:oam:us-west-2:<acct>:link/<uuid>') -> expect 403 AccessDeniedException explicit-deny, observe 404 ResourceNotFoundException.
5. Cause 5: create a sink, then oam.get_sink_policy(SinkIdentifier=<sink arn>) -> expect 404 ResourceNotFoundException, observe 501 NotImplemented.
6. Cause 8: oam.put_sink_policy(SinkIdentifier=<sink arn>, Policy='{not valid json') and a policy document with no Principal -> expect 400 InvalidParameterException 'Policy validation failed' with GetSinkPolicy still 404; a correct statement with Principal '*' must be ACCEPTED and read back. Observe 501 NotImplemented for all of them.
7. Cause 6/7: oam.update_link(Identifier='arn:aws:oam:us-west-2:<acct>:link/<uuid>', ResourceTypes=['AWS::CloudWatch::Metric']) -> expect 403 AccessDeniedException, observe 501 NotImplemented; repeat with a client built with botocore Config(parameter_validation=False) and ResourceTypes omitted (and with ResourceTypes=[]) -> expect the identical 403, observe 400 MissingRequiredParameterException.
Replay shortcut: re-run the oam checks listed in trace_ids.
