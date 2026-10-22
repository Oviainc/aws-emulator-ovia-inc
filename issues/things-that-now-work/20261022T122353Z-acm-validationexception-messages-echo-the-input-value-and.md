Issue-Source: trace-sweep-divergence
Issue-Trace: acm-generic-tag-operations-malformed-arn-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-generic-tag-operations-malformed-arn-errors acm-request-certificate-with-malformed-ca-arn acm-request-public-certificate-dns-validation-lifecycle acm-tagging-and-validation-errors acm-certificate-list-and-search-error-paths acm-multi-certificate-listing-and-search
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|ListTagsForResource|4xx->4xx|$.message
Issue-Key: acm|UntagResource|4xx->4xx|$.message
Issue-Key: acm|RequestCertificate|4xx->4xx|$.message
Issue-Key: acm|PutAccountConfiguration|4xx->4xx|$.message
Issue-Key: acm|ListCertificates|4xx->4xx|$.message
Issue-Commit: 30db2a8818f1221c3278e244a1430bae2ca83b41

# ACM ValidationException messages echo the input value and an anchored pattern

> _Note: this issue was identified against the code base at commit `30db2a8818f1221c3278e244a1430bae2ca83b41`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
The emulator's constraint-validation formatter builds a different sentence from AWS's for every ACM member-constraint failure: it interpolates the caller's submitted value (AWS redacts it as 'Value of the input at') and it echoes the Smithy @pattern verbatim including the ^...$ anchors (AWS reports the pattern unanchored). Status (400) and error type match everywhere; only the formatter output diverges. The message rule here IS known - it is built from the request member and its modeled constraint - so the shape of the sentence is the requirement, not the recorded text of any one case. A fifth cause is a different message with a model-stated rule. Five causes.

Cause 1 of 5 - ListTagsForResource with a malformed resourceArn.
AWS: Smithy ListTagsForResource.ResourceArn targets Arn, required, length 20..2048, pattern ^arn:[\\w+=/,.@-]+:acm:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$; ValidationException (400). Recorded message: "1 validation error detected: Value of the input at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w+=/,.@-]+:acm:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*" - no submitted value, no anchors.
Emulator: 400 with "1 validation error detected: Value 'not-a-valid-arn-at-all' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:...*$".
Why: the emulator leaks the caller's input back into the error text where AWS deliberately redacts it, and advertises a pattern string that differs from AWS's, so clients/tests that assert on or parse the validation message diverge.
Fix: the shared constraint-validation message formatter in the ACM service crate - render 'Value of the input at '<member>'' (no value interpolation) and strip leading ^ / trailing $ from the pattern before interpolating it.

Cause 2 of 5 - UntagResource with the same malformed resourceArn: identical divergence on the same member and formatter.
AWS: UntagResource.ResourceArn, same Arn constraints, ValidationException (400), same redacted/unanchored sentence.
Emulator: same value-echoing, anchored sentence.
Why/Fix: as Cause 1 (same formatter, reached from a second operation - confirms it is not operation-local).

Cause 3 of 5 - RequestCertificate with a malformed certificateAuthorityArn.
AWS: Smithy RequestCertificate.CertificateAuthorityArn targets PcaArn, length 20..2048, pattern ^arn:[\\w+=/,.@-]+:acm-pca:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$. Recorded message reports member 'certificateAuthorityArn' with the acm-pca pattern, unanchored and without the submitted value.
Emulator: echoes "Value 'not-a-valid-certificate-authority-arn-at-all' at 'certificateAuthorityArn'" and the anchored pattern.
Why/Fix: as Cause 1.

Cause 4 of 5 - PutAccountConfiguration with an over-long idempotencyToken trips both constraints.
AWS: Smithy PutAccountConfiguration.IdempotencyToken is required, length 1..32, pattern ^\\w+$. Recorded message: "2 validation errors detected: Value of the input at 'idempotencyToken' failed to satisfy constraint: Member must have length less than or equal to 32; Value of the input at 'idempotencyToken' failed to satisfy constraint: Member must satisfy regular expression pattern: \\w+".
Emulator: same two clauses in the same order, but each interpolates the submitted token and the pattern is rendered '^\\w+$'.
Why/Fix: as Cause 1 - this also shows the multi-error count and ordering are already right, so only the per-clause rendering needs changing.

Cause 5 of 5 - ListCertificates with SortBy but no SortOrder reports the wrong sentence.
AWS: Smithy ListCertificatesRequest.SortBy doc states 'If you specify SortBy, you must also specify SortOrder'; the recorded 400 message for that rule is 'Request contains either SortBy or SortOrder. Both SortBy and SortOrder parameters need to specified in the request.'
Emulator: 400 with 'SortBy and SortOrder must be specified together; SortOrder is missing'.
Why: the status and the rule match, but the text is the emulator's own wording for a message whose rule the model states, so clients asserting AWS's sentence fail.
Fix: the ListCertificates handler's sort-parameter validation - emit AWS's sentence for the paired-parameter rule.

Traces: acm-generic-tag-operations-malformed-arn-errors#task#1 (representative) and #task#2, acm-request-certificate-with-malformed-ca-arn#task#1 (cluster also covers acm-request-public-certificate-dns-validation-lifecycle#task#5), acm-tagging-and-validation-errors#task#4, acm-certificate-list-and-search-error-paths#task#3 (cluster also covers acm-multi-certificate-listing-and-search#task#4).

## Evidence
All five: HTTP 400 expected and returned; only $.message diverges.
acm-generic-tag-operations-malformed-arn-errors#task#1 (ListTagsForResource) and #task#2 (UntagResource): expected "1 validation error detected: Value of the input at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w+=/,.@-]+:acm:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*", got "1 validation error detected: Value 'not-a-valid-arn-at-all' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[\\w+=/,.@-]+:acm:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$".
acm-request-certificate-with-malformed-ca-arn#task#1 (RequestCertificate): expected "...Value of the input at 'certificateAuthorityArn' ... pattern: arn:[\\w+=/,.@-]+:acm-pca:...(/[\\w+=,.@-]+)*", got "...Value 'not-a-valid-certificate-authority-arn-at-all' at 'certificateAuthorityArn' ... pattern: ^arn:...$".
acm-tagging-and-validation-errors#task#4 (PutAccountConfiguration): expected "2 validation errors detected: Value of the input at 'idempotencyToken' failed to satisfy constraint: Member must have length less than or equal to 32; Value of the input at 'idempotencyToken' ... pattern: \\w+", got the same two clauses with "Value 'edcf2492-b054-4720-b528-4e3096c39531' at 'idempotencyToken'" and pattern "^\\w+$".
acm-certificate-list-and-search-error-paths#task#3 (ListCertificates): expected 'Request contains either SortBy or SortOrder. Both SortBy and SortOrder parameters need to specified in the request.', got 'SortBy and SortOrder must be specified together; SortOrder is missing'.
Smithy: Arn pattern ^arn:...:acm:...$ (length 20..2048); PcaArn pattern ^arn:...:acm-pca:...$; IdempotencyToken length 1..32 pattern ^\\w+$; ValidationException httpError 400; ListCertificatesRequest.SortBy doc 'If you specify SortBy, you must also specify SortOrder'.

## Steps to Reproduce
1. Replay the ACM wire corpus against the emulator.
2. Call ListTagsForResource and UntagResource with ResourceArn='not-a-valid-arn-at-all' -> 400, but the message interpolates the value and the anchored pattern.
3. Call RequestCertificate with CertificateAuthorityArn='not-a-valid-certificate-authority-arn-at-all' -> same formatter divergence on 'certificateAuthorityArn'.
4. Call PutAccountConfiguration with a 36-char UUID idempotency token -> both clauses echo the value and render '^\\w+$'.
5. Call ListCertificates with SortBy set and SortOrder omitted -> 400 with the emulator's own sentence instead of AWS's.
6. Named steps: acm-generic-tag-operations-malformed-arn-errors#task#1 and #task#2, acm-request-certificate-with-malformed-ca-arn#task#1, acm-tagging-and-validation-errors#task#4, acm-certificate-list-and-search-error-paths#task#3.
