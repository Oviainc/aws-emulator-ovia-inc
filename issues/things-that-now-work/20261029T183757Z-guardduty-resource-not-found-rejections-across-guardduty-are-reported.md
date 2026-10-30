Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.get-nonexistent-ipset-badrequest guardduty.update-ipset-nonexistent-rejected guardduty.filter-lifecycle-create-update-delete guardduty.filter-delete-already-deleted-rejected guardduty.filter-criteria-numeric-round-trip guardduty.deploy-detector-cfn-behaves guardduty.publishing-destination-lifecycle guardduty.malware-protection-plan-lifecycle guardduty.delete-malware-protection-plan-cross-account-denied guardduty.archive-findings-foreign-id-rejected
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.get-nonexistent-ipset-badrequest|run#19|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: guardduty.update-ipset-nonexistent-rejected|run#9|AssertionError|assert error.get("Message") == expected, (
Issue-Key: guardduty.filter-lifecycle-create-update-delete|run#21|AssertionError|assert err["Message"] == "The request is rejected since no such resource found.", f"unexpected message {err['Message']!r}"
Issue-Key: guardduty.filter-delete-already-deleted-rejected|run#10|AssertionError|assert err["Message"] == "The request is rejected because the given filter name is invalid.", (
Issue-Key: guardduty.filter-criteria-numeric-round-trip|run#20|AssertionError|assert err["Message"] == "The request is rejected since no such resource found.", f"unexpected dead filter read message {err['Message']!r}"
Issue-Key: guardduty.deploy-detector-cfn-behaves|run#19|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: guardduty.publishing-destination-lifecycle|run#22|AssertionError|assert err["Message"] == expected_describe, (
Issue-Key: guardduty.malware-protection-plan-lifecycle|run#12|AssertionError|assert err["Message"] == not_found, f"expected message {not_found!r}, got {err['Message']!r}"
Issue-Key: guardduty.delete-malware-protection-plan-cross-account-denied|run#13|AssertionError|assert message == _NOT_FOUND_MESSAGE, f"expected message {_NOT_FOUND_MESSAGE!r}, got {message!r}"
Issue-Key: guardduty.archive-findings-foreign-id-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Resource-not-found rejections across GuardDuty are reported as an account-ownership error instead of AWS's per-family not-found wording

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIPSet

## Description
This issue bundles 5 distinct root causes, all in the guardduty crate: every GuardDuty operation that must refuse an identifier naming no resource answers with one invented, shared message of the form 'The request is rejected because the input <member> is not owned by the current account.' Real GuardDuty never words a plain not-found that way; each resource family has its own fixed text. The error CODE (BadRequestException, HTTP 400) matches in each case, only the message diverges, so every cause below is a message-text defect, not a status defect. Fix all 5 before submitting.

Cause 1 of 5: IPSet not-found.
AWS: Smithy GetIPSet/UpdateIPSet (IpSetId is an httpLabel String; errors BadRequestException httpError 400, InternalServerErrorException 500). Recorded behavior: GetIPSet and UpdateIPSet for an IpSetId that does not exist under the detector return HTTP 400 BadRequestException with the fixed text 'The request is rejected since no such resource found.' (no variable part at all — the id is NOT interpolated).
Emulator: GetIPSet returns 'The request is rejected because the input ipSetId is not owned by the current account.'; UpdateIPSet on a never-existent id returns 'The request is rejected because the parameter ipSetId has an invalid value.' — two further different messages, neither of which AWS emits.
Why: a client matching on the not-found text (or a human reading it) is told the id belongs to another account, or that it is syntactically invalid, when in fact it simply does not exist; cross-account diagnosis and retry logic keyed to the real wording never fire.
Fix: the not-found path of the GetIPSet and UpdateIPSet handlers / the shared error constructor they call in the guardduty crate (services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_ip_set.rs, update_ip_set.rs).
Traces: guardduty.get-nonexistent-ipset-badrequest, guardduty.update-ipset-nonexistent-rejected.

Cause 2 of 5: Filter not-found.
AWS: Smithy GetFilter/DeleteFilter (FilterName httpLabel; BadRequestException 400). Recorded behavior: reading or deleting a filter name that does not exist (including one just deleted) returns HTTP 400 BadRequestException with the same fixed text 'The request is rejected since no such resource found.'
Emulator: 'The request is rejected because the input filterName is not owned by the current account.'
Why: the normal lifecycle probe 'did my delete take effect?' is answered with an ownership claim, so callers that distinguish deleted-vs-foreign resources mis-classify their own deleted filters.
Fix: the not-found path of the GetFilter/DeleteFilter/UpdateFilter handlers in the guardduty crate (src/ops/get_filter.rs, delete_filter.rs).
Traces: guardduty.filter-lifecycle-create-update-delete, guardduty.filter-delete-already-deleted-rejected, guardduty.filter-criteria-numeric-round-trip, guardduty.deploy-detector-cfn-behaves.

Cause 3 of 5: Publishing destination not-found.
AWS: Smithy DescribePublishingDestination (DestinationId httpLabel, required; errors BadRequestException 400, InternalServerErrorException 500). Recorded behavior: describing a destination id that no longer exists returns HTTP 400 BadRequestException with the fixed text 'The request is rejected because the one or more input parameters have invalid values.' (AWS treats a dead DestinationId as a bad parameter, with no interpolated id).
Emulator: 'The request is rejected because the input destinationId is not owned by the current account.'
Why: a client confirming DeletePublishingDestination took effect gets wording AWS never emits, so parity assertions and operator runbooks keyed to the real text fail.
Fix: the not-found path of the DescribePublishingDestination handler in the guardduty crate (src/ops/describe_publishing_destination.rs).
Traces: guardduty.publishing-destination-lifecycle.

Cause 4 of 5: Malware protection plan not-found.
AWS: Smithy DeleteMalwareProtectionPlan/GetMalwareProtectionPlan (MalwareProtectionPlanId httpLabel; modeled errors include ResourceNotFoundException 404 and BadRequestException 400). Recorded behavior: an id that is absent (or owned elsewhere) is refused with the fixed text 'The request was rejected because the specified malware protection plan was not found.'
Emulator: 'The request is rejected because the input malwareProtectionPlanId is not owned by the current account.'
Why: AWS deliberately collapses foreign and absent plans into one not-found wording so plan ids are not probeable; the emulator's message discloses an ownership distinction AWS hides and breaks clients matching the documented text.
Fix: the not-found/foreign-owner path of the malware-protection-plan handlers in the guardduty crate (src/ops/delete_malware_protection_plan.rs, get_malware_protection_plan.rs).
Traces: guardduty.malware-protection-plan-lifecycle, guardduty.delete-malware-protection-plan-cross-account-denied.

Cause 5 of 5: ArchiveFindings with a finding id that is not the detector's.
AWS: Smithy ArchiveFindings (FindingIds, list length 0..50, required; errors BadRequestException 400, InternalServerErrorException 500). Recorded behavior: a well-formed 32-hex FindingId that does not belong to the detector is refused with HTTP 400 BadRequestException and the fixed text 'The request is rejected because an invalid finding identifiers is specified.' (AWS's wording, grammatical oddity included; the ids are not interpolated), and no finding of the caller is archived as a side effect.
Emulator: 'The request is rejected because the input findingIds is not owned by the current account.'
Why: the finding-id rejection class is the one AWS clients retry/alert on; the substituted text makes the refusal indistinguishable from the emulator's generic ownership error, and the same wrong text is reused for every unrelated family (causes 1-4), so no client can tell these refusals apart.
Fix: the FindingIds validation path of the ArchiveFindings handler (and the sibling findings-metadata ops that share it) in the guardduty crate (src/ops/archive_findings.rs). Note: open issue 20261029T131547Z-guardduty-archivefindings-wrong-behavior.md concerns only the monotonic clock on a no-op write and does not cover this message.
Traces: guardduty.archive-findings-foreign-id-rejected.

## Evidence
Replayed check assertions (expected = real AWS, got = emulator):
- guardduty.get-nonexistent-ipset-badrequest: expected message 'The request is rejected since no such resource found.', got 'The request is rejected because the input ipSetId is not owned by the current account.'
- guardduty.update-ipset-nonexistent-rejected: expected message 'The request is rejected since no such resource found.', got 'The request is rejected because the parameter ipSetId has an invalid value.'
- guardduty.filter-lifecycle-create-update-delete / filter-delete-already-deleted-rejected / filter-criteria-numeric-round-trip / deploy-detector-cfn-behaves: expected 'The request is rejected since no such resource found.', got 'The request is rejected because the input filterName is not owned by the current account.'
- guardduty.publishing-destination-lifecycle: expected 'The request is rejected because the one or more input parameters have invalid values.', got 'The request is rejected because the input destinationId is not owned by the current account.'
- guardduty.malware-protection-plan-lifecycle / delete-malware-protection-plan-cross-account-denied: expected 'The request was rejected because the specified malware protection plan was not found.', got 'The request is rejected because the input malwareProtectionPlanId is not owned by the current account.'
- guardduty.archive-findings-foreign-id-rejected: code BadRequestException matched; expected 'The request is rejected because an invalid finding identifiers is specified.', got 'The request is rejected because the input findingIds is not owned by the current account.'
In every case HTTP 400 / BadRequestException matched; only the message text diverged.

## Steps to Reproduce
1. Boot the emulator and run the behavioral suite's guardduty checks listed in trace_ids.
2. Minimal manual repro per cause: (a) CreateDetector, then GetIPSet(DetectorId, IpSetId='0'*32) and UpdateIPSet on the same id; (b) CreateFilter then DeleteFilter then GetFilter on the same Name; (c) CreatePublishingDestination, DeletePublishingDestination, then DescribePublishingDestination on the returned DestinationId; (d) DeleteMalwareProtectionPlan with an id that was never created; (e) CreateSampleFindings, then ArchiveFindings(DetectorId, FindingIds=['f'*32]).
3. Compare each Error.Message against the AWS texts quoted in the evidence; Error.Code and HTTP 400 already match.
