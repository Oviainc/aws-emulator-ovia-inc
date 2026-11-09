Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-domain-lifecycle-and-tags
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-domain-lifecycle-and-tags connectcases-field-options-and-select connectcases-layout-lifecycle connectcases-template-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|BatchPutFieldOptions|4xx->4xx|$.message
Issue-Key: connectcases|CreateLayout|4xx->4xx|$.message
Issue-Key: connectcases|CreateTemplate|4xx->4xx|$.message
Issue-Related: 20261109T181134Z-connectcases-validationexception-bodies-use-an-enumerated-constraint-template.md
Issue-Commit: b00bffca1001eed9eb3256fe81ec389ea90933f7

# connectcases conflict and field-reference refusal messages use emulator wording

> _Note: this issue was identified against the code base at commit `b00bffca1001eed9eb3256fe81ec389ea90933f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Four net-new wire divergences in which the emulator reaches the RIGHT refusal (correct HTTP status and error class) but words it in its own idiom instead of AWS's. Each cause is a semantic refusal raised by the handler itself (not the generic validation layer), so each needs its own sentence. THIS ISSUE HAS FOUR CAUSES - close all four before submitting.

=== Cause 1 of 4: CreateDomain for a name that already exists (fixture connectcases-domain-lifecycle-and-tags, step task#8) ===
AWS: Smithy: CreateDomain = POST /domains, errors ConflictException (errorKind client, httpError 409, single required member `message`, doc 'The requested operation would cause a conflict with the current state of a service resource associated with the request...'). Message text is recorded behavior only: AWS answers 409 ConflictException with `Domain name already exists for account: <accountId>` - fixed text `Domain name already exists for account: ` plus ONE variable part, the caller's 12-digit AWS account id taken from the request's credentials/identity. Note what is NOT in it: the domain name itself does not appear.
Emulator: 409 (status and class correct) with message `Domain creation failed: domain resource with the name <name> already exists!` - a different sentence that interpolates the name instead of the account id.
Why: idempotent provisioning code commonly matches this message to decide 'already mine, continue', and account-scoped tooling parses the account id out of it; neither works, so a re-run of a domain bootstrap fails instead of converging.
Fix: CreateDomain's duplicate-name branch (the conflict construction in the operation's handler) - emit the fixed text with the resolved account id.
Traces: connectcases-domain-lifecycle-and-tags#task#8.

=== Cause 2 of 4: BatchPutFieldOptions against a field that is not SingleSelect (fixture connectcases-field-options-and-select, step task#9) ===
AWS: Smithy: BatchPutFieldOptions = PUT /domains/{domainId}/fields/{fieldId}/options, doc 'Creates and updates a set of field options for a single select field in a Cases domain', errors ResourceNotFoundException / ServiceQuotaExceededException / ValidationException (httpError 400). Recorded behavior: 400 ValidationException with message `Field type (<type>) does not support field options. Only SingleSelect field types are supported` - fixed text `Field type (` + `) does not support field options. Only SingleSelect field types are supported`, the single variable part being the target field's TYPE as stored (e.g. Text). The field id does not appear.
Emulator: 400 with message `field <fieldId> is a Text field; field options can only be set on a SingleSelect field` - names the field id, lower-cases the opener, and reads the type into prose.
Why: the caller is told the right thing in the wrong words; message-pinned handling that distinguishes 'wrong field type' from 'unknown field' (both 400-class here) does not fire, and the recorded sentence's type token is what a client echoes to the operator.
Fix: BatchPutFieldOptions' field-type precondition in its handler/validation.
Traces: connectcases-field-options-and-select#task#9.

=== Cause 3 of 4: CreateLayout whose content references field ids that do not exist (fixture connectcases-layout-lifecycle, step task#13) ===
AWS: Smithy: CreateLayout = POST /domains/{domainId}/layouts, errors ConflictException / ResourceNotFoundException / ServiceQuotaExceededException / ValidationException (400). Recorded behavior: 400 ValidationException with `The following field IDs provided in content do not exist: <ids>` - fixed text `The following field IDs provided in content do not exist: ` plus the variable list of the unresolvable field ids from the request. Crucially the sentence names the INPUT MEMBER the bad ids came from (`content`).
Emulator: 400 with the generic `Unknown field Ids: <ids>` - correct ids, no member attribution, different wording.
Why: a client cannot tell which part of its layout payload was rejected, and the message does not match AWS for contract tests or operator-facing output.
Fix: CreateLayout's field-reference resolution in its validation path; the member name must be part of the emitted text.
Traces: connectcases-layout-lifecycle#task#13.

=== Cause 4 of 4: CreateTemplate whose requiredFields reference field ids that do not exist (fixture connectcases-template-lifecycle, step task#12) ===
AWS: Smithy: CreateTemplate = POST /domains/{domainId}/templates, errors ConflictException / ResourceNotFoundException / ServiceQuotaExceededException / ValidationException (400); CreateTemplateRequest carries requiredFields (RequiredFieldList, @length max 100). Recorded behavior: 400 ValidationException with `The following field IDs provided in requiredFields do not exist: <ids>` - same fixed frame as Cause 3 but naming the member `requiredFields`.
Emulator: the SAME generic `Unknown field Ids: <ids>` it emits for CreateLayout.
Why: because AWS's text is parameterised by the input member, one shared helper cannot satisfy both this cause and Cause 3 - a fix that only retextes the shared helper will make one of the two pass and leave the other diverging. A client validating a template payload gets no indication that requiredFields (rather than, say, a layout reference) was at fault.
Fix: CreateTemplate's field-reference resolution - the refusal must be raised per input member, with the member name threaded into the message.
Traces: connectcases-template-lifecycle#task#12.

## Evidence
Replay summary: simparity-wire-aws-v1, emulator_up=true, 182 steps, 56 mismatched, score 64.3%, 22 clusters (all connectcases). All four clusters below have empty stub_path and empty owned_by.

Cause 1 - connectcases-domain-lifecycle-and-tags#task#8, CreateDomain, expected 409 / actual 409 [mismatch]:
  $.message: expected 'Domain name already exists for account: <accountId>' (recorded with account 000000000000), got 'Domain creation failed: domain resource with the name <name> already exists!' (recorded with the per-run name sim2real-probe-cc-domain-8c6f35fd)

Cause 2 - connectcases-field-options-and-select#task#9, BatchPutFieldOptions, expected 400 / actual 400 [mismatch]:
  $.message: expected 'Field type (Text) does not support field options. Only SingleSelect field types are supported', got 'field <fieldId> is a Text field; field options can only be set on a SingleSelect field' (recorded fieldId 3b5850f7-ecbb-4474-aa89-0aeab23e0ae6 is a per-run value)

Cause 3 - connectcases-layout-lifecycle#task#13, CreateLayout, expected 400 / actual 400 [mismatch]:
  $.message: expected 'The following field IDs provided in content do not exist: <ids>', got 'Unknown field Ids: <ids>' (recorded id sim2real-probe-cc-missing-field-90966db6 is a per-run value and is the variable part)

Cause 4 - connectcases-template-lifecycle#task#12, CreateTemplate, expected 400 / actual 400 [mismatch]:
  $.message: expected 'The following field IDs provided in requiredFields do not exist: <ids>', got 'Unknown field Ids: <ids>' (recorded id sim2real-probe-cc-missing-field-e7ca8e0d)

In each quote the ids/names/account are per-run values filling the template; the requirement is the fixed text and which request/state value is interpolated. Causes 3 and 4 show the member name ('content' vs 'requiredFields') is part of the fixed frame.

Smithy: ConflictException httpError 409 (required member message); ValidationException httpError 400 (required member message); bound as above on CreateDomain / BatchPutFieldOptions / CreateLayout / CreateTemplate.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the connectcases emulator and read the four clusters named under Traces. Manually:
1. CreateDomain(name=N) -> poll Active; CreateDomain(name=N) again -> expect 409 ConflictException 'Domain name already exists for account: <your 12-digit account id>'; emulator returns 'Domain creation failed: domain resource with the name N already exists!' (Cause 1).
2. CreateField(domainId, name=<unique>, type='Text') -> fieldId; BatchPutFieldOptions(domainId, fieldId, options=[{name:'a', value:'a', active:true}]) -> expect 400 'Field type (Text) does not support field options. Only SingleSelect field types are supported'; emulator returns 'field <fieldId> is a Text field; ...' (Cause 2).
3. CreateLayout(domainId, name=<unique>, content referencing fieldId '<never-created-id>') -> expect 400 'The following field IDs provided in content do not exist: <never-created-id>'; emulator returns 'Unknown field Ids: <never-created-id>' (Cause 3).
4. CreateTemplate(domainId, name=<unique>, status='Active', requiredFields=[{fieldId:'<never-created-id>'}]) -> expect 400 'The following field IDs provided in requiredFields do not exist: <never-created-id>'; emulator returns the same 'Unknown field Ids: ...' sentence (Cause 4).
