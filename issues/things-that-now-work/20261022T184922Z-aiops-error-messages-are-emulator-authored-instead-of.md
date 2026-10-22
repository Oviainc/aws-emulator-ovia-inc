Issue-Source: trace-sweep-divergence
Issue-Trace: aiops-error-create-investigation-group-validation
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: aiops-error-create-investigation-group-validation aiops-error-not-found-and-validation aiops-list-investigation-groups-and-validation aiops-error-investigation-group-not-found aiops-error-investigation-group-policy-not-found
Issue-Shape-Id: com.amazonaws.aiops#AIOps
Issue-Key: aiops|CreateInvestigationGroup|4xx->4xx|$.message
Issue-Key: aiops|ListInvestigationGroups|4xx->4xx|$.message
Issue-Key: aiops|GetInvestigationGroup|4xx->4xx|$.message
Issue-Key: aiops|DeleteInvestigationGroup|4xx->4xx|$.message
Issue-Key: aiops|UpdateInvestigationGroup|4xx->4xx|$.message
Issue-Key: aiops|GetInvestigationGroupPolicy|4xx->4xx|$.message
Issue-Key: aiops|PutInvestigationGroupPolicy|4xx->4xx|$.message
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# aiops error messages are emulator-authored instead of matching the service's

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvestigationGroup

## Description
Seven causes, all in aiops error construction: the HTTP status and x-amzn-errortype are right everywhere below, but the $.message the emulator builds follows its own rules rather than the service's. Two rules are violated — (a) aiops validation messages name the member and its constraint but never echo the rejected value, and the constraint they quote is the service's live constraint, and (b) a missing investigation group always yields one fixed literal, not a per-operation sentence built from the caller's identifier. Both are read directly off the recorded responses, which are consistent across five operations and three fixtures. No aiops issue is currently open, so none of these is covered elsewhere.

Cause 1 of 7 - CreateInvestigationGroup name pattern validation message.
AWS: the input member name targets StringWithPatternAndLengthLimits with @length 1..512 and @pattern ^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$ (Smithy: CreateInvestigationGroup.name; error ValidationException -> 400). The recorded 400/ValidationException message is built from the request, so its rule is known: "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — no rejected value is interpolated, and the pattern cited is the live service's, unanchored and wider than the model's ([\-_A-Za-z0-9\[\]\(\){}\.:!?@; ]+ — note it additionally permits ! ? @ ; and carries no ^...$).
Emulator: same status 400 and errortype, but the message interpolates the rejected value ("Value 'invalid!name#with*bad*chars' at 'name' ...") and quotes the model's anchored, narrower pattern (^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$).
Why: two things go wrong for a client. It sees caller data echoed back in an error the service does not echo it in, and it is told a constraint that is stricter than the one the service enforces — so a name containing ! ? @ or ; is reported as pattern-invalid by the emulator's message while AWS accepts those characters, and any client or test that parses the quoted pattern out of the message derives the wrong rule.
Traces: aiops-error-create-investigation-group-validation#task#2 (also task#3; and aiops-error-not-found-and-validation#task#4).

Cause 2 of 7 - ListInvestigationGroups maxResults range validation message.
AWS: maxResults is an @httpQuery Integer with @range min 1 max 50 (Smithy: ListInvestigationGroups.maxResults; error ValidationException -> 400). Recorded 400 message, same request-built rule with no value echoed: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50".
Emulator: status and errortype match, but the message echoes the rejected value: "Value '999' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50".
Why: confirms cause 1 is not member-specific — the emulator's validation formatter interpolates the value for every member, so no aiops validation message it emits matches the service's shape.
Traces: aiops-error-not-found-and-validation#task#5 (also aiops-list-investigation-groups-and-validation#task#2).

Cause 3 of 7 - GetInvestigationGroup on a missing group.
AWS: ResourceNotFoundException is modeled with httpError 404 (Smithy: GetInvestigationGroup errors -> ResourceNotFoundException 404). The recorded 404 message is the fixed literal 'InvestigationGroup does not exist.' (recorded behavior only; identical in every recorded not-found response across operations and fixtures, and it contains nothing drawn from the request).
Emulator: status 404 and errortype match, but the message is emulator-authored and interpolates the caller's identifier, e.g. "no investigation group matches identifier '<identifier>'".
Why: the message is not the service's, and it leaks the requested identifier into an error AWS keeps opaque.
Traces: aiops-error-investigation-group-not-found#task#1 (also aiops-error-not-found-and-validation#task#1).

Cause 4 of 7 - DeleteInvestigationGroup on a missing group. Same AWS literal 'InvestigationGroup does not exist.' at 404/ResourceNotFoundException (Smithy: DeleteInvestigationGroup errors -> ResourceNotFoundException 404); emulator emits the same identifier-interpolating sentence as cause 3.
Traces: aiops-error-investigation-group-not-found#task#2 (also aiops-error-not-found-and-validation#task#2).

Cause 5 of 7 - UpdateInvestigationGroup on a missing group. Same AWS literal at 404; same emulator-authored message.
Traces: aiops-error-investigation-group-not-found#task#3 (also aiops-error-not-found-and-validation#task#3).

Cause 6 of 7 - GetInvestigationGroupPolicy when the group does not exist.
AWS: 404/ResourceNotFoundException with the same group-level literal 'InvestigationGroup does not exist.' — the service reports the missing GROUP, not a missing policy (Smithy: GetInvestigationGroupPolicy errors -> ResourceNotFoundException 404).
Emulator: 404 but with "no resource policy is associated with investigation group '<identifier>'", i.e. it describes a different condition (a group that exists with no policy) than the one AWS reports.
Why: a client cannot distinguish "group absent" from "group present, policy absent" the way AWS lets it, because the emulator collapses the former into the latter's wording.
Traces: aiops-error-investigation-group-policy-not-found#task#1.

Cause 7 of 7 - PutInvestigationGroupPolicy when the group does not exist. AWS: 404/ResourceNotFoundException, same literal (Smithy: PutInvestigationGroupPolicy errors -> ResourceNotFoundException 404); emulator emits the identifier-interpolating sentence of cause 3.
Traces: aiops-error-investigation-group-policy-not-found#task#2.

Fix: two shared components of the aiops service crate, not seven call sites. (1) The validation-error message formatter used by the aiops input validators: stop interpolating the rejected value and emit "1 validation error detected: Value at '<member>' failed to satisfy constraint: ..."; for the name pattern, cite the service's live unanchored pattern rather than the model's anchored one (and enforce that wider character set so the two agree). (2) The aiops ResourceNotFoundException constructor for the InvestigationGroup resource, shared by the Get/Delete/Update group handlers and the Get/Put policy handlers: emit the service's fixed group-not-found literal instead of a per-handler sentence containing the caller's identifier, including on the policy paths where the group itself is absent.

## Evidence
Every cause below matched on status and on x-amzn-errortype; only $.message diverged. Expected = recorded AWS, actual = emulator.

Cause 1 - aiops-error-create-investigation-group-validation#task#2, CreateInvestigationGroup, 400 vs 400: $.message value_mismatch, expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\-_A-Za-z0-9\\[\\]\\(\\)\\{\\}\\.:!?@; ]+", got "1 validation error detected: Value 'invalid!name#with*bad*chars' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\-_A-Za-z0-9\\[\\]\\(\\)\\{\\}\\.: ]+$". Cluster affected_count 3: aiops-error-create-investigation-group-validation#task#2, #task#3, aiops-error-not-found-and-validation#task#4.

Cause 2 - aiops-error-not-found-and-validation#task#5, ListInvestigationGroups, 400 vs 400: $.message value_mismatch, expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50", got "1 validation error detected: Value '999' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50". Cluster affected_count 2 (also aiops-list-investigation-groups-and-validation#task#2).

Cause 3 - aiops-error-investigation-group-not-found#task#1, GetInvestigationGroup, 404 vs 404: $.message value_mismatch, expected 'InvestigationGroup does not exist.', got "no investigation group matches identifier 'sim2real-probe-aiops-no-such-group-ba92ca60'". Cluster affected_count 2 (also aiops-error-not-found-and-validation#task#1).

Cause 4 - aiops-error-investigation-group-not-found#task#2, DeleteInvestigationGroup, 404 vs 404: expected 'InvestigationGroup does not exist.', got "no investigation group matches identifier 'sim2real-probe-aiops-no-such-group-ba92ca60'". Cluster affected_count 2 (also aiops-error-not-found-and-validation#task#2).

Cause 5 - aiops-error-investigation-group-not-found#task#3, UpdateInvestigationGroup, 404 vs 404: expected 'InvestigationGroup does not exist.', got "no investigation group matches identifier 'sim2real-probe-aiops-no-such-group-ba92ca60'". Cluster affected_count 2 (also aiops-error-not-found-and-validation#task#3).

Cause 6 - aiops-error-investigation-group-policy-not-found#task#1, GetInvestigationGroupPolicy, 404 vs 404: expected 'InvestigationGroup does not exist.', got "no resource policy is associated with investigation group 'sim2real-probe-aiops-no-such-group-policy-689915a2'". Cluster affected_count 1.

Cause 7 - aiops-error-investigation-group-policy-not-found#task#2, PutInvestigationGroupPolicy, 404 vs 404: expected 'InvestigationGroup does not exist.', got "no investigation group matches identifier 'sim2real-probe-aiops-no-such-group-policy-689915a2'". Cluster affected_count 1.

The quoted identifiers and the quoted rejected value are fixture-specific examples; the requirements are the message RULES stated in the description (member+constraint with no value echoed; the service's unanchored name pattern; the fixed group-not-found literal).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus); all seven steps below are mismatches, not errors.
2. Cause 1: replay aiops-error-create-investigation-group-validation#task#2 - POST /investigationGroups with a name containing characters outside the allowed set; compare $.message with the recorded one (no value echoed, unanchored pattern with ! ? @ ;).
3. Cause 2: replay aiops-error-not-found-and-validation#task#5 - GET /investigationGroups?maxResults=999; compare $.message (recorded one does not echo '999').
4. Causes 3-5: replay aiops-error-investigation-group-not-found#task#1, #task#2 and #task#3 - GET, DELETE and PATCH/PUT of /investigationGroups/{identifier} for an identifier that was never created; each must answer 404/ResourceNotFoundException with the literal 'InvestigationGroup does not exist.'
5. Causes 6-7: replay aiops-error-investigation-group-policy-not-found#task#1 and #task#2 - GET and POST /investigationGroups/{identifier}/policy for a group that does not exist; both must answer 404/ResourceNotFoundException with the same group-level literal, not a policy-level or identifier-quoting sentence.
