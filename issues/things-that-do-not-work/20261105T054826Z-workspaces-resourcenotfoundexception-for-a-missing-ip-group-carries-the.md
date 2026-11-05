Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-ip-group-tagging
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-ip-group-tagging
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|DescribeTags|4xx->4xx|$.message
Issue-Key: workspaces|CreateTags|4xx->4xx|$.message
Issue-Commit: b350b7ce5eb4140cadaa8c09dcbaa452e8ffad74

# ResourceNotFoundException for a missing IP group carries the generic resource message instead of AWS's IP-group text

> _Note: this issue was identified against the code base at commit `b350b7ce5eb4140cadaa8c09dcbaa452e8ffad74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTags

## Description
The tagging operations resolve a `ResourceId` that names a WorkSpaces IP group. When that IP group does not exist, the emulator raises its ResourceNotFoundException with the shape's generic default message text rather than the resource-specific message AWS returns for an IP group, on every tagging operation that takes an IP group id. Two causes, one shared root: the IP-group lookup used by the tagging handlers maps "not found" onto a generic error message instead of the IP-group-specific one.

Cause 1 of 2 — DescribeTags on an unknown IP group id.
  AWS: Smithy: DescribeTags input ResourceId (NonEmptyString, required, @length min 1); modeled error ResourceNotFoundException (errorKind client, httpError 400) with members `message` and `ResourceId`. The model's own doc string for the shape is the generic "The resource could not be found."; the recorded response shows that for an IP-group ResourceId AWS words the message for the resource class instead: HTTP 400, error code ResourceNotFoundException, message "The specified Ip Group could not be found." That whole sentence is fixed text for this error class — it has no variable parts; the IP group id travels in the exception's `ResourceId` member, not in the message.
  Emulator: HTTP 400 and error code ResourceNotFoundException are correct, but `$.message` is "The resource could not be found." (value_mismatch: expected "The specified Ip Group could not be found.").
  Why: a client that branches on the error text — or that surfaces it to an operator — cannot tell a missing IP group from any other missing WorkSpaces resource, so IP-group-specific retry/remediation paths and log assertions keyed to AWS's wording never fire.
  Fix: the DescribeTags handler's resource resolution / error construction for the IP-group resource type — it must emit the IP-group-specific ResourceNotFoundException message rather than the shape's default.
  Traces: workspaces-ip-group-tagging#task#7.

Cause 2 of 2 — CreateTags on an unknown IP group id.
  AWS: Smithy: CreateTags input ResourceId (NonEmptyString, required) and Tags (TagList, required); modeled errors InvalidParameterValuesException, ResourceLimitExceededException, ResourceNotFoundException (all client, httpError 400). Recorded response for a tag write against a non-existent IP group: HTTP 400, code ResourceNotFoundException, message "The specified Ip Group could not be found." — same fixed text, no variable parts.
  Emulator: HTTP 400 and code ResourceNotFoundException are correct, `$.message` is "The resource could not be found." (value_mismatch against the expected IP-group text).
  Why: same loss of resource discrimination on the write path; a caller tagging an IP group it believes it just created gets a message that does not identify what was missing, so cleanup/creation-retry logic keyed to AWS's wording is not triggered.
  Fix: the CreateTags handler's resource resolution / error construction for the IP-group resource type (same code path as Cause 1; the two should share one IP-group-aware not-found error).
  Traces: workspaces-ip-group-tagging#task#8.

Neither cause is covered by any open issue. The open issue 20261103T034302Z-workspaces-createipgroup-duplicate-name-refusal-under-concurrency-carries-the.md is also about WorkSpaces IP-group error wording, but it concerns CreateIpGroup's duplicate-name refusal, not the tagging operations' not-found message, and carries neither of these replay keys. 20261029T185818Z-workspaces-createworkspaces-failedrequests-entries-diverge-request-echo-drops-workspaceproperties.md is unrelated (CreateWorkspaces FailedRequests).

## Evidence
Replay of fixture workspaces-ip-group-tagging (service workspaces), verdict mismatch in both clusters:

Step task#7 — DescribeTags, expected_status 400, actual_status 400:
  $.message: value_mismatch (expected 'The specified Ip Group could not be found.', got 'The resource could not be found.')

Step task#8 — CreateTags, expected_status 400, actual_status 400:
  $.message: value_mismatch (expected 'The specified Ip Group could not be found.', got 'The resource could not be found.')

Status (400) and error code (ResourceNotFoundException) match in both; only the message text diverges. Smithy (com.amazonaws.workspaces): ResourceNotFoundException is a client error, httpError 400, members message/ResourceId, and is a modeled error of both DescribeTags and CreateTags.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `workspaces-ip-group-tagging`.
2. At step `task#7` the fixture calls DescribeTags with a ResourceId naming a WorkSpaces IP group that does not exist. Observe HTTP 400 / ResourceNotFoundException with message 'The resource could not be found.' instead of 'The specified Ip Group could not be found.'
3. At step `task#8` the fixture calls CreateTags with the same non-existent IP group ResourceId and a tag list. Observe the same generic message instead of the IP-group-specific one.
4. Equivalently, by hand: call DescribeTags (then CreateTags) against the emulator with ResourceId set to any wsipg-* id that was never created, and compare `message` in the 400 body to AWS's IP-group wording.
