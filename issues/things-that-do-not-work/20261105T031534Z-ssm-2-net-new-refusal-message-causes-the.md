Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.parameter-tier-upgrade-is-one-way-and-intelligent-tiering-derives-the-tier ssm.update-document-rejects-duplicate-content
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.parameter-tier-upgrade-is-one-way-and-intelligent-tiering-derives-the-tier|run#26|AssertionError|assert huge_message == (
Issue-Key: ssm.update-document-rejects-duplicate-content|run#12|AssertionError|assert message == (
Issue-Commit: 8de6682f074d8bd25eca3d1022f82b7ad69da0d3

# ssm: 2 net-new refusal-message causes — the Advanced-tier value-size ceiling and UpdateDocument's DuplicateDocumentContent are emulator-authored

> _Note: this issue was identified against the code base at commit `8de6682f074d8bd25eca3d1022f82b7ad69da0d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutParameter

## Description
Two net-new root causes, both in SSM refusal MESSAGES. In both cases the HTTP status (400) and the error code are already right; only the Message text diverges from the sentence AWS writes. Each AWS message below is a TEMPLATE: the quoted prose is fixed text, and any variable part is named explicitly and comes from the request or the emulator's own state — no value from one recording is itself the requirement.

Context: the fixed issue 20261103T215115Z-ssm-parameter-store-refusal-messages-and-error-classes.md corrected the STANDARD-tier ceiling sentence (its cause 9); that assertion now passes, and the same check advances to the ADVANCED-tier ceiling below, which that issue did not cover. The open issues 20261103T173026Z-ssm-parameter-store-error-classes-and-messages-are-emulator.md and 20261103T215115Z-ssm-parameter-store-semantics-diverge-selector-echo-filter.md describe the same formatter family but carry neither cause and neither of these check ids, so they do not cover this work.

Cause 1 of 2: PutParameter with a value over the 8 KB advanced-tier ceiling is refused with the wrong sentence.
AWS: 400 ValidationException (Smithy: PutParameter models no ValidationException and puts no @length on PSParameterValue, so the ceiling and its wording are recorded behavior only) with the wholly fixed, two-sentence message `The specified parameter value is too large. Advanced-tier parameters support a maximum parameter value of 8192 characters.` There is no variable part: the message never echoes the request's byte count or the parameter name. The refusal must also leave no parameter behind (DescribeParameters on the name returns nothing).
Emulator: 400 ValidationException with `Advanced tier parameters support a maximum parameter value of 8192 characters.` — the leading "The specified parameter value is too large." sentence is missing and the hyphen in AWS's "Advanced-tier" is dropped.
Why: a client that matches or surfaces AWS's refusal text (log scrapers, retry logic that distinguishes a too-large value from other validation failures, a user-facing message) never sees AWS's sentence, and an operator reading the emulator's text is not told that the VALUE is what was too large.
Fix: the PutParameter handler's tier/value-size validation — the advanced-tier ceiling refusal message (the sibling standard-tier ceiling message in the same validation path is already correct).
Traces: ssm.parameter-tier-upgrade-is-one-way-and-intelligent-tiering-derives-the-tier.

Cause 2 of 2: UpdateDocument's DuplicateDocumentContent message is a different error's sentence.
AWS: 400 DuplicateDocumentContent (Smithy: UpdateDocument error DuplicateDocumentContent, httpError 400) with `Document to be updated with name <Name> has the same metadata and content as document version provided in request` — fixed text around one variable part, <Name>, taken verbatim from the request's Name member (no trailing full stop). The refusal must mint no new version: ListDocumentVersions still returns the versions that existed before and DescribeDocument's LatestVersion is unchanged.
Emulator: 400 DuplicateDocumentContent with `The content of the association document matches another document. Change the content of the document and try again.` — AWS's wording for a DIFFERENT condition (a duplicate association document), naming no document and describing a cross-document collision rather than "same content as the version provided in request".
Why: the caller is told its content collides with some other document when in fact it re-submitted the content already on this document's own latest version, and a client that matches AWS's sentence (or parses the document name out of it) cannot identify the document.
Fix: the UpdateDocument handler's duplicate-content precondition — construct DuplicateDocumentContent with the UpdateDocument sentence and the request's Name.
Traces: ssm.update-document-rejects-duplicate-content.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout; both checks are listed under '### ssm' in the suite report.

Cause 1 — [mismatch] ssm.parameter-tier-upgrade-is-one-way-and-intelligent-tiering-derives-the-tier
check_ssm_parameter_tier_upgrade_is_one_way_and_intelligent_tiering_derives_the_tier.py line 219:
  assert huge_message == ("The specified parameter value is too large. Advanced-tier parameters support a "
                          "maximum parameter value of 8192 characters.")
AssertionError: Advanced tier parameters support a maximum parameter value of 8192 characters.
So: expected Message 'The specified parameter value is too large. Advanced-tier parameters support a maximum parameter value of 8192 characters.'; got 'Advanced tier parameters support a maximum parameter value of 8192 characters.'. The code assertion (ValidationException) passed, as did the earlier explicit-Standard-tier ceiling message assertion, so the divergence is confined to the advanced-tier sentence. The failing call is PutParameter(Name=<unique>, Value=9000 'h' characters, Type=String, Tier=Advanced).

Cause 2 — [mismatch] ssm.update-document-rejects-duplicate-content
check_ssm_update_document_rejects_duplicate_content.py line 94:
  assert message == (f"Document to be updated with name {name} has the same metadata and content as document "
                     "version provided in request")
AssertionError: The content of the association document matches another document. Change the content of the document and try again.
So: expected Message 'Document to be updated with name <Name> has the same metadata and content as document version provided in request'; got 'The content of the association document matches another document. Change the content of the document and try again.'. The code assertion (DuplicateDocumentContent) passed, and the positive control — an UpdateDocument with genuinely different content minting version "2" — also passed, so only the duplicate-content message diverges.

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (PutParameter: Value targets PSParameterValue, required, sensitive, no @length; errors include IncompatiblePolicyException/ParameterPatternMismatchException but no ValidationException — the tier ceilings are recorded behavior. UpdateDocument: Name required with pattern ^[a-zA-Z0-9_\-.]{3,128}$; errors include DuplicateDocumentContent (client, httpError 400), DuplicateDocumentVersionName, InvalidDocumentContent).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: PutParameter(Name=/<unique>, Value=<9000 characters>, Type="String", Tier="Advanced"). Expect 400 ValidationException with Message 'The specified parameter value is too large. Advanced-tier parameters support a maximum parameter value of 8192 characters.' and DescribeParameters on that name returning no row.
3. Cause 2: CreateDocument(Name=<unique>, DocumentType="Command", DocumentFormat="JSON", Content=<a schemaVersion 2.2 aws:runShellScript document>); wait for Status Active. UpdateDocument with genuinely different Content and DocumentVersion="$LATEST" (mints version "2"), then repeat the IDENTICAL UpdateDocument. Expect 400 DuplicateDocumentContent with Message 'Document to be updated with name <Name> has the same metadata and content as document version provided in request', ListDocumentVersions still 2 versions and DescribeDocument LatestVersion "2".
4. Compare Error.Code and Error.Message against the templates in the description.
5. Re-replay exactly the check ids in trace_ids to confirm both causes are closed.
