Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# A ResourceARN that is not an ARN at all answers 404 ResourceNotFoundException instead of 400 ValidationException

_Filed: 2026-10-24T15:57:57Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ResourceARN` is modelled as an `AmazonResourceName` with only a length bound (1..1011) and NO pattern, so a syntactically invalid value — "my-contact", "arn:aws:ssm-contacts", a bare alias — reaches the handler and the service itself must reject it. AWS answers the modelled ValidationException (HTTP 400, "Invalid value provided - Invalid resource Arn <value>"), because it cannot even parse the string into a resource identity; ResourceNotFoundException is reserved for a WELL-FORMED ARN that names nothing.

The emulator conflates the two: the handler does one map lookup (`state.tags_of(&input.resource_arn)`) and renders every miss as 404 ResourceNotFoundException with ResourceId set to the raw garbage string. A caller that mistypes an ARN is told the resource is missing rather than that the input is malformed, and SDK/CLI retry and error-handling paths that branch on ValidationException vs ResourceNotFoundException take the wrong branch.

The same hole exists on the two sibling tagging writes (TagResource, UntagResource), which resolve the ARN through the same `tags_mut` lookup — the crate even already carries the right message helper (`crate::state::invalid_resource_arn_message`, used only by the replication-set gate).

Expected: a ResourceARN that is not a parseable `arn:<partition>:ssm-contacts:<region>:<account>:<resource>` answers 400 ValidationException; a well-formed ssm-contacts ARN that names no stored contact/rotation keeps answering 404 ResourceNotFoundException (today's behaviour, which is correct).

## Evidence

Spec: smithy_query operation=ListTagsForResource — input `ResourceARN` targets `AmazonResourceName` (length 1..1011, no pattern), and the operation declares BOTH ValidationException (client, httpError 400) and ResourceNotFoundException (client, httpError 404); the model gives ValidationException as "The input fails to satisfy the constraints specified by an Amazon Web Services service", which is exactly an unparseable ARN. Every other member of this service that is meant to be an ARN uses `SsmContactsArn` with the ^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:... pattern, so for those the framework answers 400 — ResourceARN is the one ARN member the boundary validator cannot screen, which is why the handler must.

Handler: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/list_tags_for_resource.rs lines 181-190 — `let Some(tags) = state.tags_of(&input.resource_arn) else { return Err(... ResourceNotFoundException { resource_id: input.resource_arn.clone(), resource_type: "resource" }) }` with no syntactic check beforehand. src/ops/tag_resource.rs lines 197-207 and src/ops/untag_resource.rs lines 181-191 do the same via `tags_mut`. src/state.rs line 887 defines `invalid_resource_arn_message` ("Invalid value provided - Invalid resource Arn {arn}") but it is only reached from `require_active_replication_set_for`.

Tests: tests/all/data_plane_precondition.rs lines 1735-1766 assert 404/ResourceNotFoundException for all three tagging operations, but ONLY for well-formed ARNs that name nothing (an absent contact ARN, an absent rotation ARN, a contact-channel ARN, an engagement ARN). No test passes a value that is not an ARN, so nothing asserts the current behaviour for malformed input.

## Steps to Reproduce

1. Onboard the account (seed an ACTIVE Incident Manager replication set) so the tagging operations are past the account gate.
2. Call ListTagsForResource with ResourceARN="my-contact" (not an ARN). Observed: HTTP 404 ResourceNotFoundException echoing the string as ResourceId. Expected: HTTP 400 ValidationException ("Invalid value provided - Invalid resource Arn my-contact").
3. Repeat with ResourceARN="arn:aws:ssm-contacts" (truncated ARN) and with ResourceARN="arn:aws:s3:::some-bucket" (an ARN of another service): both answer 404 where AWS answers 400.
4. Call TagResource with ResourceARN="my-contact", Tags=[{Key: team, Value: sre}] and UntagResource with ResourceARN="my-contact", TagKeys=[team]: same 404-for-malformed divergence.
5. Confirm the correct case still holds: ListTagsForResource with a well-formed but never-created contact ARN must stay 404 ResourceNotFoundException.
