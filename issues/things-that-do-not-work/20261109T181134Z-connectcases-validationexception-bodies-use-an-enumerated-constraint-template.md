Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-domain-lifecycle-and-tags
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-domain-lifecycle-and-tags connectcases-malformed-requests-raw-wire connectcases-field-and-options-lifecycle connectcases-template-and-case-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|CreateDomain|4xx->4xx|$.message
Issue-Key: connectcases|CreateDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connectcases|CreateField|4xx->4xx|$.message
Issue-Key: connectcases|CreateField|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connectcases|BatchGetField|4xx->4xx|$.message
Issue-Key: connectcases|BatchGetField|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261109T181134Z-connectcases-resourcenotfoundexception-refusals-carry-a-generic-message-and.md
Issue-Commit: b00bffca1001eed9eb3256fe81ec389ea90933f7

# connectcases ValidationException bodies use an enumerated constraint template instead of AWS wording

> _Note: this issue was identified against the code base at commit `b00bffca1001eed9eb3256fe81ec389ea90933f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Four net-new wire divergences, all the same fault: when a connectcases request body violates a modeled input constraint the emulator answers with the Query/EC2-style enumerated validation template ("N validation error(s) detected: Value '<v>' at '<member>' failed to satisfy constraint: ...") that other AWS services use, whereas connectcases answers 400 ValidationException with its own, much terser text. The status (400) is right in all four; the message - and for three of them the x-amzn-errortype header's shape - diverges. THIS ISSUE HAS FOUR CAUSES - close all four before submitting.

=== Cause 1 of 4: CreateDomain with a `name` that violates its constraints (fixture connectcases-domain-lifecycle-and-tags, step task#9) ===
AWS: Smithy: CreateDomain = POST /domains, errors ConflictException / ServiceQuotaExceededException / ValidationException; ValidationException is errorKind client, httpError 400, with one required member `message`, shape doc 'The request isn't valid. Check the syntax and try again.'. CreateDomainRequest.name targets DomainName with @length min 1 max 100 and @pattern ^.*[\S]$, so an empty-string name violates BOTH. The message TEXT is recorded behavior only: AWS answers HTTP 400 with the single fixed sentence `Invalid request body` - entirely fixed text, with NO variable parts: it does not name the member, the rejected value, the constraint or how many constraints failed. The recorded response's x-amzn-errortype header is also shape-asserted and the emulator's bare token does not satisfy it. Hypothesis: the refusal is produced by the service frontend's body deserializer rather than by per-member constraint evaluation, which is why no member name appears.
Emulator: 400 with message "2 validation errors detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*[\S]$" and x-amzn-errortype: `ValidationException`, which fails the recorded header's shape assertion.
Why: a client pinned to connectcases' refusal text (contract tests, log matchers, UI copy) never matches, and one that parses the enumerated template to recover a member name is reading a structure AWS never sends for this service - so input-validation handling built against AWS misfires.
Fix: the service's request-validation layer / error serializer for connectcases - constraint violations on the parsed body must collapse to the single fixed sentence and to the recorded errortype header shape, instead of reusing the cross-service enumerated validation formatter.
Traces: connectcases-domain-lifecycle-and-tags#task#9 (cluster also holds connectcases-malformed-requests-raw-wire#task#3).

=== Cause 2 of 4: CreateField with a missing/null required member (fixture connectcases-field-and-options-lifecycle, step task#11) ===
AWS: Smithy: CreateField = POST /domains/{domainId}/fields, errors include ValidationException httpError 400; CreateFieldRequest.name is required (FieldName, @length 1-100, @pattern ^.*[\S]$). Recorded behavior: 400 ValidationException with the same single fixed sentence `Invalid request body`, no member name and no value, plus the same shape-asserted x-amzn-errortype.
Emulator: 400 with "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null" and x-amzn-errortype: ValidationException.
Why: same as Cause 1; the divergence is uniform across operations, so a client sees AWS's wording from no connectcases operation at all.
Fix: same validation/serialization layer; the null/required-member path must route to the same fixed sentence.
Traces: connectcases-field-and-options-lifecycle#task#11 (cluster also holds connectcases-field-and-options-lifecycle#task#12, connectcases-malformed-requests-raw-wire#task#4 and #task#5).

=== Cause 3 of 4: BatchGetField with an empty `fields` list (fixture connectcases-malformed-requests-raw-wire, step task#7) ===
AWS: Smithy: BatchGetField = POST /domains/{domainId}/fields-batch, errors ResourceNotFoundException / ValidationException; the request's field-identifier list is length-constrained (min 1), so an empty list is invalid. Recorded behavior: 400 ValidationException, message `Invalid request body` (fixed text, no variable parts), same shape-asserted errortype header.
Emulator: 400 with "1 validation error detected: Value '0 items' at 'fields' failed to satisfy constraint: Member must have length greater than or equal to 1" and x-amzn-errortype: ValidationException.
Why: same as above - and it shows the divergence covers list-length constraints, not just scalar members, so fixing only the scalar path leaves it.
Fix: same layer, list-length branch included.
Traces: connectcases-malformed-requests-raw-wire#task#7.

=== Cause 4 of 4: CreateCase that omits the template's required fields (fixture connectcases-template-and-case-lifecycle, step task#13) ===
AWS: Smithy: CreateCase = POST /domains/{domainId}/cases, errors ConflictException / ResourceNotFoundException / ValidationException (httpError 400). Recorded behavior: when the submitted case does not supply the field values the template requires, AWS does NOT answer with a member-level complaint about `fields`; it evaluates the TEMPLATE's required fields and answers 400 ValidationException with message `Field(s) <names> are required` - fixed text `Field(s) ` + ` are required`, the variable part being the list of missing required field ids/names taken from the template (recorded as the single id `title`). So this is a semantic check against stored template state, not a shape check.
Emulator: 400 with "1 validation error detected: Value null at 'fields' failed to satisfy constraint: Member must not be null" - it never reaches the template's required-field evaluation.
Why: a client that reads the missing field names out of this message to prompt the user (the whole point of template required fields) gets nothing usable, and a case-creation form cannot tell 'you forgot title' from 'your request was malformed'.
Fix: CreateCase's handler/validation - after the body is accepted, compare the supplied field ids against the resolved template's requiredFields and emit this message naming the missing ones.
Traces: connectcases-template-and-case-lifecycle#task#13.

## Evidence
Replay summary: simparity-wire-aws-v1, emulator_up=true, 182 steps, 56 mismatched, score 64.3%, 22 clusters (all connectcases). All four clusters below have empty stub_path and empty owned_by.

Cause 1 - connectcases-domain-lifecycle-and-tags#task#9, CreateDomain, expected 400 / actual 400 [mismatch]:
  $.message: expected 'Invalid request body', got "2 validation errors detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.*[\S]$"
  $headers.x-amzn-errortype: shape_mismatch (recorded header shape not satisfied by the emulator's 'ValidationException')
  affected: connectcases-malformed-requests-raw-wire#task#3

Cause 2 - connectcases-field-and-options-lifecycle#task#11, CreateField, expected 400 / actual 400 [mismatch]:
  $.message: expected 'Invalid request body', got "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null"
  $headers.x-amzn-errortype: shape_mismatch (got 'ValidationException')
  affected: connectcases-field-and-options-lifecycle#task#12, connectcases-malformed-requests-raw-wire#task#4, connectcases-malformed-requests-raw-wire#task#5

Cause 3 - connectcases-malformed-requests-raw-wire#task#7, BatchGetField, expected 400 / actual 400 [mismatch]:
  $.message: expected 'Invalid request body', got "1 validation error detected: Value '0 items' at 'fields' failed to satisfy constraint: Member must have length greater than or equal to 1"
  $headers.x-amzn-errortype: shape_mismatch (got 'ValidationException')

Cause 4 - connectcases-template-and-case-lifecycle#task#13, CreateCase, expected 400 / actual 400 [mismatch]:
  $.message: expected 'Field(s) title are required', got "1 validation error detected: Value null at 'fields' failed to satisfy constraint: Member must not be null"
  ('title' is the template's required field id for this recording and is the message's variable part; the fixed text is 'Field(s) ' ... ' are required')

Smithy: ValidationException = structure, errorKind client, httpError 400, single required member message, doc "The request isn't valid. Check the syntax and try again."; bound on CreateDomain, CreateField, BatchGetField and CreateCase. CreateDomainRequest.name -> DomainName @length{min:1,max:100} @pattern ^.*[\S]$.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the connectcases emulator and read the four clusters named under Traces. Manually:
1. POST /domains with body {"name": ""} -> expect 400 ValidationException with message exactly 'Invalid request body'; emulator returns the '2 validation errors detected: ...' template (Cause 1).
2. CreateDomain(name=<unique>), poll GetDomain until Active, then POST /domains/{domainId}/fields with body {"type": "Text"} (no name) -> expect 400 / 'Invalid request body'; emulator returns "1 validation error detected: Value null at 'name' ..." (Cause 2).
3. POST /domains/{domainId}/fields-batch with body {"fields": []} -> expect 400 / 'Invalid request body'; emulator returns the '0 items' template (Cause 3).
4. CreateTemplate(domainId, name=<unique>, status='Active', requiredFields=[{fieldId:'title'}]) then POST /domains/{domainId}/cases with body {"templateId": <templateId>} (no fields) -> expect 400 ValidationException 'Field(s) title are required'; emulator returns "1 validation error detected: Value null at 'fields' ..." (Cause 4).
