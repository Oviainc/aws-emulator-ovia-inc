Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcases.related-item-type-content-mismatch-rejected
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases.related-item-type-content-mismatch-rejected|run#9|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 4f251c7985012e87bbf2022fa7122d2715b6d63e

# connectcases CreateRelatedItem type/content mismatch is refused with the emulator's own wording instead of AWS's ValidationException message

> _Note: this issue was identified against the code base at commit `4f251c7985012e87bbf2022fa7122d2715b6d63e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRelatedItem

## Description
One net-new root cause found by replaying the behavioral suite against the connectcases emulator (6/22 checks passed). All other failures in this run are already filed: 11 are blocked by already-filed unimplemented stubs (TagResource, DeleteField, SearchCases, DeleteDomain, SearchRelatedItems, DeleteTemplate) and 4 are OWNED by the open bundle 20261029T183832Z-connectcases-five-response-refusal-fidelity-defects-getcase-unset.md.

=== Cause 1 of 1: CreateRelatedItem's cross-member type/content refusal carries a non-AWS message (failing check: connectcases.related-item-type-content-mismatch-rejected) ===

AWS: Smithy: CreateRelatedItem.type (RelatedItemType enum: Contact | Comment | File | Sla | ConnectCase | Custom) and CreateRelatedItem.content (RelatedItemInputContent union: contact | comment | file | sla | connectCase | custom) are both @required; ValidationException (HTTP 400, single required member `message`) is modeled on the operation (POST /domains/{domainId}/cases/{caseId}/related-items/). The message text itself is recorded behavior only: AWS refuses a request whose declared `type` names a different kind than the union variant set on `content` with ValidationException (HTTP 400) and the message `CONTACT type was specified with a mismatched related-item content.` The template is: the DECLARED type value from the request, UPPER-CASED, followed by the fixed text ` type was specified with a mismatched related-item content.` — nothing else is interpolated (no list of accepted values, no content kind, no domain/case id), and the sentence ends in a period. No related item is left behind.

Emulator: the error CODE and HTTP status are now correct (ValidationException / 400 — the operation is no longer a stub), but the message is the emulator's own prose: `type Contact does not describe the content given, which is the content of a Comment related item; the accepted values are Comment, ConnectCase, Contact, Custom, File, Sla`. It uses the request's `type` in its original casing, names the kind inferred from the content, and appends the whole RelatedItemType value list — none of which appears in AWS's sentence, and AWS's fixed text is absent entirely.

Why: the refusal is correctly classified, so a client that only switches on the error code is fine, but any client that surfaces, logs, localizes or matches the message for this error class (agent-desktop form validation mapping the sentence back to the offending declaration, log-based alerting, contract tests) reads text AWS never sends and its match never fires. The message is part of the error contract for this refusal class.

Fix: the CreateRelatedItem handler's cross-member consistency validation, which already runs before any state write — services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/create_related_item.rs lines 189-202 build the ValidationException message with `format!`; it must emit AWS's fixed sentence with the declared type upper-cased as its leading token. Nothing else about this path needs to change: the pre-lock placement (no related item is stored) and the ValidationException/400 classification already match AWS.

Relation to already-filed work: Cause 5 of the open bundle 20261029T183832Z-connectcases-five-response-refusal-fidelity-defects-getcase-unset.md named this operation and this message, but its recorded replay key for this check is the NotImplemented error-code mismatch (`assert error.get("Code") == "ValidationException"`), which no longer reproduces — the stub issue 20261029T183832Z-connectcases-createrelateditem-stub.md has since been fixed and the operation now validates and refuses with the right code. The surviving divergence is the message text, which the suite does not report as owned, so it is filed here. This is NOT a regression of the stub fix: the call is refused exactly where AWS refuses it.

Traces: connectcases.related-item-type-content-mismatch-rejected

## Evidence
Suite report: connectcases 6/22 (27%), 16 failed. 11 blocked by already-filed unimplemented stubs, 4 OWNED by an open issue, 1 net-new (below).

[mismatch] connectcases.related-item-type-content-mismatch-rejected
  CreateRelatedItem(domainId=<domain>, caseId=<case>, type='Contact', content={'comment': {'body': '...', 'contentType': 'Text/Plain'}})
  expected: ValidationException (HTTP 400), Message == 'CONTACT type was specified with a mismatched related-item content.'
            ('CONTACT' = the request's declared type upper-cased; the rest is fixed text, with no value from the request or state)
  actual:   ValidationException (code and status correct), Message == 'type Contact does not describe the content given, which is the content of a Comment related item; the accepted values are Comment, ConnectCase, Contact, Custom, File, Sla'
  AssertionError: expected message 'CONTACT type was specified with a mismatched related-item content.', got 'type Contact does not describe the content given, which is the content of a Comment related item; the accepted values are Comment, ConnectCase, Contact, Custom, File, Sla'

Source of the diverging text: services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/create_related_item.rs:189-202.
Model citations: CreateRelatedItem.type -> RelatedItemType (values Contact, Comment, File, Sla, ConnectCase, Custom), CreateRelatedItem.content -> RelatedItemInputContent (union), both @required; ValidationException httpError 400 with required `message`.

Not asserted-on in this run but downstream of the failing assertion (the check could not reach it): after the refusal the check calls SearchRelatedItems, which is a separately-filed unimplemented stub.

## Steps to Reproduce
Replay connectcases.related-item-type-content-mismatch-rejected (run_behavioral_suite). Manually:
1. CreateDomain(name=<unique>) and poll GetDomain until domainStatus == 'Active'.
2. CreateField(domainId, name=<unique>, type='Text'); CreateTemplate(domainId, name=<unique>, status='Active').
3. CreateCase(domainId, templateId, fields=[title field + the text field]).
4. CreateRelatedItem(domainId, caseId, type='Contact', content={'comment': {'body': 'wrong content for declared type', 'contentType': 'Text/Plain'}}).
5. Assert the error is ValidationException (HTTP 400) — it is — and that Message equals exactly 'CONTACT type was specified with a mismatched related-item content.' The emulator instead returns 'type Contact does not describe the content given, which is the content of a Comment related item; the accepted values are Comment, ConnectCase, Contact, Custom, File, Sla'.
6. Repeat with another mismatched pair (e.g. type='Comment' with content={'contact': {...}}) and confirm the message is the same fixed sentence with the declared type upper-cased as its leading token ('COMMENT type was specified with a mismatched related-item content.').
