Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-decommission-and-delete-conflicts
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-decommission-and-delete-conflicts outposts-error-paths-not-found-and-validation
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|DeleteSite|4xx->4xx|$.Message
Issue-Key: outposts|DeleteSite|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: outposts|DeleteSite|4xx->4xx|status
Issue-Key: outposts|ListTagsForResource|4xx->4xx|$.message
Issue-Key: outposts|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: outposts|CreateOutpost|4xx->4xx|$.Message
Issue-Key: outposts|CreateOutpost|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: outposts|CreateOutpost|4xx->4xx|status
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# Outposts error responses return wrong HTTP status and error code

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSite

## Description
Three outposts failure paths are classified into the wrong modeled error, so the status line and x-amzn-errortype a client sees do not match AWS. Each is a status and/or error-code divergence, not a wording difference.

Cause 1 of 3: DeleteSite on a site that still has Outposts answers 409 ConflictException instead of 400.
AWS: Smithy: DeleteSite errors are AccessDeniedException(403), ConflictException(409), InternalServerException(500), NotFoundException(404), ValidationException(400). The recording shows AWS rejecting the delete-with-children case with HTTP 400 (the fixture does not assert the errortype header), message e.g. 'Site os-... cannot be deleted because it has one or more outposts.'
Emulator: HTTP 409 with x-amzn-errortype: ConflictException.
Why: 409 and 400 are different retry/handling classes — SDK retry policies and client error branches that key on the status code or on ConflictException take a different path against the emulator than against AWS, so a teardown that must fail cleanly on 400 is mis-handled.
Fix: the DeleteSite handler's error classification for the has-dependent-outposts precondition (should raise the 400 error the recording shows, not ConflictException).
Traces: outposts-decommission-and-delete-conflicts#task#3.

Cause 2 of 3: ListTagsForResource on a malformed ARN answers ValidationException instead of BadRequestException.
AWS: Smithy: ListTagsForResource (GET /tags/{ResourceArn}) with ResourceArn pattern ^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$; modeled errors are InternalServerException(500), NotFoundException(404), ValidationException(400). Recorded behavior only: AWS answers HTTP 400 with x-amzn-errortype: BadRequestException and an ARN-syntax message (e.g. 'Invalid input resource arn: bad syntax'), i.e. this op does not surface the generic framework validation error for a bad ARN.
Emulator: HTTP 400 (status matches) with x-amzn-errortype: ValidationException and the generic '1 validation error detected: ... failed to satisfy constraint: Member must satisfy regular expression pattern: ...' body.
Why: the error code, not just the text, differs — a client that branches on BadRequestException (as the AWS response names) falls through to its unknown-error path.
Fix: the ListTagsForResource handler's ARN parsing/validation, which must emit the BadRequestException code the recording shows rather than deferring to the shared pattern-validation path.
Traces: outposts-error-paths-not-found-and-validation#task#10.

Cause 3 of 3: CreateOutpost with an availability zone outside the region answers 404 NotFoundException instead of 400 ValidationException.
AWS: Smithy: CreateOutpost input AvailabilityZone/AvailabilityZoneId, SiteId required; modeled errors include NotFoundException(404) and ValidationException(400). The recording shows AWS validating the requested availability zone against the request's region and rejecting it with HTTP 400 / x-amzn-errortype ValidationException, message e.g. 'use1-az1 does not match your region.'
Emulator: HTTP 404 with x-amzn-errortype: NotFoundException, reporting the site as nonexistent instead of rejecting the cross-region AZ.
Why: the client is told the wrong thing about its request — a retry with a corrected AZ is indicated by a 400 ValidationException, whereas a 404 NotFoundException sends it looking for a missing site. Checking site existence before AZ/region validation also masks AZ errors generally.
Fix: the CreateOutpost handler's input validation ordering — validate AvailabilityZone/AvailabilityZoneId against the region and raise ValidationException(400) before (or independently of) the site lookup.
Traces: outposts-error-paths-not-found-and-validation#task#12.

## Evidence
Cause 1 — DeleteSite: status expected 400, got 409; $headers.x-amzn-errortype got 'ConflictException' (recorded response did not assert the header); $.Message example expected 'Site os-bf70f261ff7a5dbbe cannot be deleted because it has one or more outposts.', got 'the site os-bf70f261ff7a5dbbe cannot be deleted while Outpost op-e77c84b180a57e0a0 is at it'.
Cause 2 — ListTagsForResource: status 400 both sides; $headers.x-amzn-errortype expected 'BadRequestException', got 'ValidationException'; $.message example expected 'Invalid input resource arn: bad syntax', got "1 validation error detected: Value 'not-a-valid-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ...".
Cause 3 — CreateOutpost: status expected 400, got 404; $headers.x-amzn-errortype expected 'ValidationException', got 'NotFoundException'; $.Message example expected 'use1-az1 does not match your region.', got 'the site os-00000000000000000 does not exist'.
Statuses and error codes above are the requirement; the quoted messages are examples from the recordings.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay outposts-decommission-and-delete-conflicts step task#3 — DeleteSite on a site that still holds an Outpost; observe 409/ConflictException where the recording has 400.
3. Cause 2: replay outposts-error-paths-not-found-and-validation step task#10 — ListTagsForResource with resourceArn 'not-a-valid-arn'; observe x-amzn-errortype ValidationException where the recording has BadRequestException.
4. Cause 3: replay outposts-error-paths-not-found-and-validation step task#12 — CreateOutpost with an availability zone from another region; observe 404/NotFoundException where the recording has 400/ValidationException.
