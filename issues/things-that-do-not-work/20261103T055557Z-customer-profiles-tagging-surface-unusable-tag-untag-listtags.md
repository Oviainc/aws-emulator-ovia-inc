Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal customer-profiles-ccp-traj-search-then-create customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling customer-profiles-domain-unset-optional-fields-stay-absent customer-profiles-create-domain-rejects-a-name-outside-the-name-charset
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: customer-profiles|ListTagsForResource|2xx->4xx|status
Issue-Key: customer-profiles|TagResource|2xx->4xx|status
Issue-Key: customer-profiles|UntagResource|2xx->4xx|status
Issue-Key: customer-profiles|GetDomain|2xx->2xx|$.Tags.stage
Issue-Key: customer-profiles|GetDomain|2xx->2xx|$.Tags
Issue-Key: customer-profiles|CreateDomain|2xx->2xx|$.Tags
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# customer-profiles tagging surface unusable: Tag/Untag/ListTags refuse valid domain ARNs and Tags is dropped from domain responses (6 causes)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Six net-new wire causes that together make the customer-profiles tagging surface unusable: the three tagging operations refuse every well-formed domain ARN with a 400, and the domain-projection operations drop the Tags member from their responses.

Cause 1 of 6: ListTagsForResource refuses a valid Customer Profiles domain ARN.
AWS: Smithy: ListTagsForResource is GET /tags/{resourceArn} with resourceArn a required @httpLabel targeting `TagArn` (@length 0..256, @pattern ^arn:[a-z0-9]{1,10}:profile$ as carried in this repo's model), output member `tags` targeting TagMap, errors BadRequestException/400 and ResourceNotFoundException/404. Recorded response: HTTP 200 for the domain ARN 'arn:<partition>:profile:<region>:<accountId>:domains/<DomainName>', with `tags` holding the tag set currently on the domain. The modeled pattern is therefore NOT applied by the service as an anchored full-string match (as written it would match no real ARN, since every real value continues past ':profile').
Emulator: HTTP 400 and no `tags` member - the ARN is rejected before the lookup.
Why: tag-based cost allocation, tag-driven cleanup sweeps and read-after-tag round-trips all fail with an error AWS never returns, and a correct request is reported as malformed.
Fix: the resourceArn check shared by the tagging handlers - validate the value as a Customer Profiles resource ARN ('arn:<partition>:profile:<region>:<account>:domains/<name>') instead of requiring the model pattern to match the whole string.
Traces: customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal#task#3 (cluster also covers #task#6 of the same fixture, the post-untag read).

Cause 2 of 6: TagResource refuses the same ARN, so a domain cannot be tagged after create.
AWS: Smithy: TagResource is POST /tags/{resourceArn} with the same TagArn label and a required `tags` TagMap (@length 1..50); the output structure is empty. Recorded response: HTTP 204 with no body - the success status for this empty-output operation is 204, not the model's default 200 - and the submitted tags are UNIONED onto the tags set at CreateDomain (existing keys preserved, same keys overwritten).
Emulator: HTTP 400 - the call never reaches the tag store.
Why: no caller can add a tag to an existing domain; every tagging workflow (ownership, environment labelling, cleanup selectors) is blocked.
Fix: the TagResource handler's resourceArn validation (shared with Cause 1) plus its success projection - answer 204 with an empty body on success.
Traces: customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal#task#2.

Cause 3 of 6: UntagResource refuses the same ARN, so tags cannot be removed.
AWS: Smithy: UntagResource is DELETE /tags/{resourceArn} with the same TagArn label and a required `tagKeys` @httpQuery list (@length 1..50); empty output. Recorded response: HTTP 204 with no body, and ONLY the named keys are removed - the other keys survive.
Emulator: HTTP 400.
Why: tags accumulate with no way to remove them; a client that untags as part of teardown leaves state behind and sees a spurious 400.
Fix: UntagResource's resourceArn validation (shared with Causes 1-2) plus a 204 empty-body success projection.
Traces: customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal#task#5.

Cause 4 of 6: GetDomain does not report tags that were applied to the domain.
AWS: Smithy: GetDomainResponse$Tags targets TagMap. Recorded response: 200 whose Tags map contains every key currently on the domain - the tags set at CreateDomain unioned with those added by TagResource, each with its current value.
Emulator: 200 with the tagged keys missing from $.Tags, so a key that was successfully applied is not reported.
Why: GetDomain is the read side of the tagging round-trip; a client that writes a tag and reads it back cannot confirm the write, and tag-driven logic keyed off GetDomain sees an untagged domain.
Fix: the GetDomain response projection - serialize the domain's full current tag map.
Traces: customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal#task#4.

Cause 5 of 6: GetDomain omits the Tags member entirely for an untagged domain instead of returning it present and empty.
AWS: recorded behavior only (the model marks Tags optional and says nothing about this): for a domain created with no Tags, GetDomain returns Tags PRESENT and empty ({}), while the genuinely unset optional members (DefaultEncryptionKey, DeadLetterQueueUrl, Matching, RuleBasedMatching) are absent from the response.
Emulator: no Tags key at all in the response body.
Why: SDK callers that index response['Tags'] - safe against AWS - raise KeyError against the emulator, and code that distinguishes 'domain has no tags' from 'tags not reported' loses that distinction.
Fix: the GetDomain response projection - always serialize Tags (empty map when the domain carries none) while continuing to omit the unset optional members.
Traces: customer-profiles-ccp-traj-search-then-create#task#2 (cluster also covers customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling#task#5 and customer-profiles-domain-unset-optional-fields-stay-absent#task#2).

Cause 6 of 6: CreateDomain's own response omits Tags the same way.
AWS: Smithy: CreateDomainResponse$Tags targets TagMap (@length 1..50). Recorded behavior only: creating a domain with no Tags returns Tags present and empty ({}) in the CreateDomain response itself; creating one WITH Tags echoes the map.
Emulator: the CreateDomain response has no Tags key.
Why: a caller that reads Tags straight off the create response (the normal way to confirm what was applied at creation) crashes or concludes nothing was tagged, before it has even issued a GetDomain.
Fix: the CreateDomain response projection - serialize Tags, empty when none were supplied.
Traces: customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling#task#3 (cluster also covers customer-profiles-create-domain-rejects-a-name-outside-the-name-charset#task#1 and customer-profiles-domain-unset-optional-fields-stay-absent#task#1).

Related but not covering: open behavioral issue 20261102T122410Z-customer-profiles-10-net-new-behavioral-causes-refusal.md records the ARN gate and the empty-Tags omission from the BEHAVIORAL suite (its Causes 5 and 6) and carries no wire replay keys; the UntagResource 204 path, the GetDomain tag read-back (Cause 4) and the CreateDomain response omission (Cause 6) are not among its causes.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false). All six clusters are verdict=mismatch.

Cause 1 - fixture customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal step task#3, ListTagsForResource:
  status: expected 200, got 400
  $.tags: missing (expected the domain's current tag map - {'sim2real': 'behav-cp', 'stage': 'probe'} in this recording - got None). affected_count=2 (#task#6).

Cause 2 - same fixture step task#2, TagResource: status: expected 204, got 400.

Cause 3 - same fixture step task#5, UntagResource: status: expected 204, got 400.

Cause 4 - same fixture step task#4, GetDomain, 200 -> 200:
  $.Tags.stage: missing (expected the value applied by the preceding TagResource, got None).

Cause 5 - fixture customer-profiles-ccp-traj-search-then-create step task#2, GetDomain, 200 -> 200:
  $.Tags: missing (expected {}, got None). affected_count=3.

Cause 6 - fixture customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling step task#3, CreateDomain, 200 -> 200:
  $.Tags: missing (expected {}, got None). affected_count=3.

The tag keys/values and the ARN in the recording are that run's values; the requirement is the shape described above - 204 empty-body success for TagResource/UntagResource, 200 with a `tags` map for ListTagsForResource, and Tags always present on the domain projections.

## Steps to Reproduce
Let ARN = 'arn:aws:profile:<region>:<account>:domains/D'.

Cause 6: CreateDomain(DomainName=D, DefaultExpirationDays=366, Tags={'sim2real':'behav-cp'}) -> response must carry Tags; repeat with no Tags -> response must carry Tags == {}. Observe no Tags key either way.
Cause 2: TagResource(resourceArn=ARN, tags={'stage':'probe'}) -> expect HTTP 204, empty body; observe 400.
Cause 1: ListTagsForResource(resourceArn=ARN) -> expect 200 with tags == {'sim2real':'behav-cp','stage':'probe'} (union of create-time and added tags); observe 400 and no tags member.
Cause 4: GetDomain(D) -> expect Tags to contain 'stage':'probe'; observe the key missing.
Cause 3: UntagResource(resourceArn=ARN, tagKeys=['stage']) -> expect HTTP 204, empty body, and a following ListTagsForResource to show only 'sim2real'; observe 400.
Cause 5: CreateDomain(D2, DefaultExpirationDays=366) with NO Tags; GetDomain(D2) -> expect Tags present and == {} with DefaultEncryptionKey/DeadLetterQueueUrl/Matching/RuleBasedMatching absent; observe no Tags key.

Re-replay fixtures customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal, customer-profiles-ccp-traj-search-then-create, customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling, customer-profiles-create-domain-rejects-a-name-outside-the-name-charset and customer-profiles-domain-unset-optional-fields-stay-absent to confirm.
