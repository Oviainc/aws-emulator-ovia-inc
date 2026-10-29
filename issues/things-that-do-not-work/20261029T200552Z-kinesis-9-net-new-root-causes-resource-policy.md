Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.resource-policy-denies-cross-principal-get-records kinesis.put-resource-policy-invalid-policy-rejected kinesis.resource-policy-invalid-json-rejected kinesis.resource-policy-put-get-delete-lifecycle kinesis.create-duplicate-stream-fails kinesis.create-channel-duplicate-name-blocked kinesis.list-channels-malformed-next-token-rejected kinesis.delete-stream-with-active-consumer-blocked kinesis.tag-resource-invalid-key-fails
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.resource-policy-denies-cross-principal-get-records|_retry_until_denied#26|Timeout|time.sleep(interval_s)
Issue-Key: kinesis.put-resource-policy-invalid-policy-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: kinesis.resource-policy-invalid-json-rejected|run#14|AssertionError|assert message == expected_message, (
Issue-Key: kinesis.resource-policy-put-get-delete-lifecycle|run#20|AssertionError|assert second_delete_message == expected_message, (
Issue-Key: kinesis.create-duplicate-stream-fails|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: kinesis.create-channel-duplicate-name-blocked|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: kinesis.list-channels-malformed-next-token-rejected|run#6|AssertionError|assert code != "ExpiredNextTokenException", (
Issue-Key: kinesis.delete-stream-with-active-consumer-blocked|run#14|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: kinesis.tag-resource-invalid-key-fails|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 954781cef49baf7406aef764139782df619c6255

# kinesis: 9 net-new root causes — resource-policy Deny is never enforced on the data plane, PutResourcePolicy accepts an invalid Effect, and the PutResourcePolicy/DeleteResourcePolicy/CreateStream/CreateChannel/DeleteStream refusal messages plus the ListChannels malformed-NextToken classification and the reserved-'aws:' TagResource rule all diverge

> _Note: this issue was identified against the code base at commit `954781cef49baf7406aef764139782df619c6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Nine distinct net-new root causes in the kinesis emulator (Cause 1 of 9 … Cause 9 of 9), each grounded in its own failing behavioral check(s) of the `kinesis` service. None of these nine checks is marked ALREADY FILED or OWNED in the replay report. Where a cause is the same defect an already-open issue describes but whose replay key that issue does not carry, the cause says so explicitly and must be closed by the same single fix.

=== Cause 1 of 9: a resource-policy Deny is never enforced on the data plane ===
Failing check: kinesis.resource-policy-denies-cross-principal-get-records.
AWS: Smithy models GetRecords with AccessDeniedException (HTTP 400) among its errors, and PutResourcePolicy as the operation that attaches a resource-based policy to a stream/channel ARN. Recorded behavior: with a resource policy on the stream carrying { "Effect": "Deny", "Principal": {"AWS": [<role arn>]}, "Action": "kinesis:GetRecords", "Resource": <stream arn> }, a caller using credentials assumed into that role — whose IDENTITY policy allows kinesis:GetRecords/GetShardIterator/ListShards/DescribeStreamSummary — is refused on GetRecords with HTTP 400 `AccessDeniedException` and the message `User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: kinesis:GetRecords on resource: <stream-arn> with an explicit deny in a resource-based policy`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` with an explicit deny in a resource-based policy`; variable parts: the assumed-role ARN (account id, role name, session name) of the caller, the action, and the stream ARN — all taken from the request and state. The stream owner's own principal, not targeted by the Deny, keeps reading successfully, and removing the Deny restores the role's access.
Emulator: the Deny has no effect. PutResourcePolicy accepts the document, the owner reads normally, and the assumed-role client's GetShardIterator + GetRecords keep SUCCEEDING; the check polls for the refusal and never observes one, so it is reported as "Check timed out after 60.0s" at the harness cap, where a service that enforced the policy would refuse on the first attempt. A stored resource policy is inert: it is persisted and readable but never consulted by the data plane.
Why: this is the over-permissive direction and it is a security contract. A client validating that a Deny actually blocks a principal gets a pass against the emulator and a different answer in production; conversely, code written against the emulator can be built on the assumption that a resource policy is decorative.
Fix: resource-policy evaluation in the kinesis authorization path — the data-plane handlers (get_records / get_shard_iterator / put_record / put_records) must consult the policy stored for the stream ARN for the calling principal and raise the modeled AccessDeniedException with AWS's explicit-deny wording before serving records.
Traces: kinesis.resource-policy-denies-cross-principal-get-records.

=== Cause 2 of 9: PutResourcePolicy accepts a document that is valid JSON but not a valid policy ===
Failing check: kinesis.put-resource-policy-invalid-policy-rejected.
AWS: Smithy models PutResourcePolicy input { ResourceARN (required, @length 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$), StreamId, Policy (required) } with errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException (all HTTP 400). Recorded behavior: the service parses AND VALIDATES the policy document — a statement whose `Effect` is neither `Allow` nor `Deny` is refused with HTTP 400 `InvalidArgumentException`, and the previously stored policy is left untouched so a later valid PutResourcePolicy still succeeds.
Emulator: the call SUCCEEDS — HTTP 200 with an empty body (content-length 2) — for a syntactically well-formed JSON document whose statement Effect is outside {Allow, Deny}. Only JSON well-formedness is checked; policy semantics are not.
Why: the emulator stores a policy no real account would accept, so the invalid document is readable back through GetResourcePolicy and any code that round-trips it looks correct while the same deploy fails in production at the write.
Fix: policy-document validation in the PutResourcePolicy handler (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/put_resource_policy.rs) — validate statement structure (Effect in {Allow, Deny}, Principal/Action/Resource presence) after JSON parsing and refuse with the modeled InvalidArgumentException before mutating stored state.
Traces: kinesis.put-resource-policy-invalid-policy-rejected.

=== Cause 3 of 9: PutResourcePolicy's malformed-JSON refusal message is emulator-authored ===
Failing check: kinesis.resource-policy-invalid-json-rejected.
AWS: PutResourcePolicy with a `Policy` that is not parseable JSON is refused HTTP 400 `InvalidArgumentException` with the message `Policy validation error: This policy contains invalid Json` — entirely fixed text, no variable parts — and the previously stored policy is left in place.
Emulator: the code and status match, but the message is `Policy must be a JSON policy document: the value sent is not valid JSON.` — a sentence AWS never emits.
Why: a client or test that matches on the refusal text (the only way to separate "unparseable document" from the other InvalidArgumentException causes of PutResourcePolicy, which all share one code) does not match, and the emulator teaches wording that does not exist.
Fix: the refusal message in the PutResourcePolicy handler's policy-parse path. Related open issue 20261029T174012Z-kinesis-five-error-identities-and-messages-are-invented.md covers other invented kinesis messages but not this one.
Traces: kinesis.resource-policy-invalid-json-rejected.

=== Cause 4 of 9: DeleteResourcePolicy's no-policy refusal message is emulator-authored ===
Failing check: kinesis.resource-policy-put-get-delete-lifecycle.
AWS: Smithy models DeleteResourcePolicy input { ResourceARN (required), StreamId } with errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException (all HTTP 400). Recorded behavior: a second DeleteResourcePolicy on a resource whose policy was already removed is refused with the message `No resource policy found for resource ARN <resource-arn>.` — fixed text `No resource policy found for resource ARN ` plus a trailing period, the one variable part being the ResourceARN from the request.
Emulator: the message is `Policy for resource <resource-arn> under account <account-id> not found.` — a different sentence that additionally interpolates the caller's account id, which AWS's wording for this error does not mention.
Why: idempotent teardown code keys on this text to decide "already gone" versus a real failure; it does not match, and the emulator teaches a template with an extra variable AWS does not use.
Fix: the refusal message in the DeleteResourcePolicy handler (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/delete_resource_policy.rs), or the shared policy-not-found message helper it uses — note the same emulator-authored sentence is what GetResourcePolicy wrongly returns, which is filed separately as the GetResourcePolicy no-policy refusal.
Traces: kinesis.resource-policy-put-get-delete-lifecycle.

=== Cause 5 of 9: CreateStream's duplicate-name ResourceInUseException message is emulator-authored ===
Failing check: kinesis.create-duplicate-stream-fails.
AWS: Smithy models CreateStream { StreamName (required, @length 1..128, pattern ^[a-zA-Z0-9_.-]+$), ShardCount, StreamModeDetails, Tags, … } with ResourceInUseException (HTTP 400) among its errors. Recorded behavior: a second CreateStream for a name already in use is refused HTTP 400 `ResourceInUseException` with the message `Stream <stream-name> under account <account-id> already exists.` — fixed text `Stream `, ` under account `, ` already exists.`; variable parts the StreamName from the request and the caller's 12-digit account id.
Emulator: the code and status match, but the message is `stream <stream-name> already exists in this account and region; delete it before creating another under that name` — lowercase lead, no account id, an invented advisory clause and no terminating period.
Why: "create if absent" logic and idempotent provisioning match on this text to tell an already-exists from the other ResourceInUseException causes CreateStream can raise; the match fails.
Fix: the duplicate-name refusal message in the CreateStream handler (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/create_stream.rs) — render AWS's fixed template with the request's stream name and the caller's account id.
Traces: kinesis.create-duplicate-stream-fails.

=== Cause 6 of 9: CreateChannel's duplicate-name ResourceInUseException message is emulator-authored ===
Failing check: kinesis.create-channel-duplicate-name-blocked.
AWS: Smithy models CreateChannel { ChannelName (required, @length 1..128, pattern ^[a-zA-Z0-9_.-]+$), ServiceExecutionRoleARN (required), StreamConfigurationList (required), S3DestinationConfiguration, … } with ResourceInUseException (HTTP 400) among its errors. Recorded behavior: creating a second channel with a ChannelName already in use (even while the first is still CREATING) is refused HTTP 400 `ResourceInUseException` with `Channel <channel-name> under account <account-id> already exists.` — the same fixed template as the stream case with `Channel` as the lead noun; variable parts the ChannelName from the request and the caller's account id.
Emulator: the message is `channel <channel-name> already exists in this account and region; a channel name is unique within your account and region` — lowercase lead, no account id, an invented explanatory clause, no terminating period.
Why: same as Cause 5, on the channel surface; the two templates are parallel on AWS and should come from one shared renderer.
Fix: the duplicate-name refusal message in the CreateChannel handler (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/create_channel.rs). Related open issue 20261029T174012Z-kinesis-stream-state-gating-wrongly-refuses-deletestream-and.md touches CreateChannel validation ordering but not this message.
Traces: kinesis.create-channel-duplicate-name-blocked.

=== Cause 7 of 9: ListChannels misclassifies a syntactically invalid NextToken as ExpiredNextTokenException ===
Failing check: kinesis.list-channels-malformed-next-token-rejected.
AWS: Smithy models ListChannels input { StreamFilter, MaxResults (1..10000), NextToken (@length 1..1048576) } with errors AccessDeniedException, ExpiredNextTokenException, InvalidArgumentException, LimitExceededException and ValidationException (all HTTP 400). Recorded behavior: a NextToken that is not a well-formed service token is refused HTTP 400 `ValidationException` with the message `Invalid NextToken.` (entirely fixed text); `ExpiredNextTokenException` is reserved for a WELL-FORMED token whose ~300 s TTL has genuinely elapsed. The two are distinct error identities for distinct conditions.
Emulator: a garbage token (`sim2real-not-a-real-next-token-deadbeef`) is refused with code `ExpiredNextTokenException`, collapsing "you sent a token I cannot parse" into "your token aged out".
Why: expiry is the retryable condition — a paginator seeing ExpiredNextTokenException legitimately restarts the listing from the first page, so a client handed a corrupted token loops forever instead of surfacing a validation failure.
Fix: NextToken decoding/validation in the ListChannels handler (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/list_channels.rs) — or in the shared pagination-token helper — must distinguish an undecodable token (ValidationException "Invalid NextToken.") from a decodable but expired one (ExpiredNextTokenException). Open issue 20261029T140704Z-kinesis-liststreamconsumers-misclassifies-a-malformed-nexttoken-as-expirednexttokenexception.md reports the identical misclassification on ListStreamConsumers; if both operations share one token helper, one fix closes both, but that issue's replay keys do not include this ListChannels check.
Traces: kinesis.list-channels-malformed-next-token-rejected.

=== Cause 8 of 9: DeleteStream's consumers-present refusal message is reworded in its tail ===
Failing check: kinesis.delete-stream-with-active-consumer-blocked.
AWS: Smithy models DeleteStream { StreamName, StreamARN, EnforceConsumerDeletion, StreamId } with ResourceInUseException (HTTP 400). Recorded behavior for a stream that still has a registered ACTIVE enhanced-fan-out consumer and EnforceConsumerDeletion unset: HTTP 400 `ResourceInUseException` with `DeleteStream can’t delete <account-id>:<stream-name> because there are consumers associated with this data stream. To delete <stream-name> anyway, set the EnforceConsumerDeletion parameter to true to force the deletion of all the consumers that are associated with it.` — note the typographic apostrophe in "can’t"; the only variable parts are the 12-digit account id and the stream name, both from the request and state.
Emulator: same status and code, but the final clause is rewritten to `...to force the deletion of all the consumers associated with this data stream.` instead of AWS's `...to force the deletion of all the consumers that are associated with it.`
Why: this text is how a client distinguishes "needs EnforceConsumerDeletion" from DeleteStream's other ResourceInUseException causes, which all share one code; the match fails.
Fix: the DeleteStream handler's consumers-present refusal message (or the shared stream-message helper that composes it). This is the SAME root cause as Cause 2 of open issue 20261027T201859Z-kinesis-4-net-new-root-causes-subscribetoshard-is.md, whose Issue-Trace-Ids carry only kinesis.delete-stream-with-active-consumer-requires-enforce-flag; this cause exists to carry the replay key of kinesis.delete-stream-with-active-consumer-blocked, which that fixer would otherwise never re-replay. One fix closes both.
Traces: kinesis.delete-stream-with-active-consumer-blocked.

=== Cause 9 of 9: TagResource accepts a reserved 'aws:'-prefixed tag key ===
Failing check: kinesis.tag-resource-invalid-key-fails.
AWS: Smithy models TagResource { ResourceARN (required), Tags (required, TagMap, @length 1..200), StreamId } with InvalidArgumentException (HTTP 400). Recorded behavior: a tag key beginning with the reserved `aws:` prefix is refused HTTP 400 `InvalidArgumentException` with `Some tag keys in the request started with 'aws:'. System tags cannot be modified.` (entirely fixed text, no variable parts), and the resource's tag set is left unchanged.
Emulator: the call SUCCEEDS — HTTP 200 with an empty body (content-length 2) — and the reserved key is stored as an ordinary user tag.
Why: the over-permissive direction: tooling that production refuses silently writes into AWS's reserved namespace against the emulator, and tag-based cost allocation/automation keyed on system tags reads values no real account can produce.
Fix: a reserved-prefix check in the tag-validation path used by the TagResource handler, refusing with the modeled InvalidArgumentException before any state mutation. This is the SAME root cause as Cause 9 of open issue 20261027T011654Z-kinesis-9-net-new-root-causes-cloudformation-aws.md, whose replay keys carry kinesis.tag-resource-reserved-aws-prefix-rejected and kinesis.add-tags-to-stream-reserved-aws-prefix-rejected but NOT kinesis.tag-resource-invalid-key-fails; this cause exists to carry that third key. One fix closes all three.
Traces: kinesis.tag-resource-invalid-key-fails.

## Evidence
All evidence is from the replayed behavioral suite (simparity-behavioral-aws-v1), 148/284 passed. Stream/channel/role names, account ids and timestamps below are per-run values filling the templates; the requirements are the statuses, codes and message templates.

Cause 1 — expected: the assumed-role client's GetRecords is refused HTTP 400 AccessDeniedException "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: kinesis:GetRecords on resource: <stream-arn> with an explicit deny in a resource-based policy". Actual: PutResourcePolicy(Deny kinesis:GetRecords for the role) returns 200, the owner reads normally, and the role's repeated GetShardIterator+GetRecords attempts all SUCCEED; the poll for the refusal never fires and the check is reported "Check timed out after 60.0s".

Cause 2 — expected: PutResourcePolicy with a statement Effect outside {Allow, Deny} is refused 400 InvalidArgumentException and the stored policy is unchanged. Actual:
  AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Cause 3 — AssertionError: malformed-JSON PutResourcePolicy message 'Policy must be a JSON policy document: the value sent is not valid JSON.', expected 'Policy validation error: This policy contains invalid Json'

Cause 4 — AssertionError: double DeleteResourcePolicy message 'Policy for resource arn:aws:kinesis:us-west-2:000000000000:stream/<stream> under account 000000000000 not found.', expected 'No resource policy found for resource ARN arn:aws:kinesis:us-west-2:000000000000:stream/<stream>.'

Cause 5 — AssertionError: expected message 'Stream <stream> under account 000000000000 already exists.', got 'stream <stream> already exists in this account and region; delete it before creating another under that name'

Cause 6 — AssertionError: expected message 'Channel <channel> under account 000000000000 already exists.', got 'channel <channel> already exists in this account and region; a channel name is unique within your account and region'

Cause 7 — AssertionError: a syntactically-invalid token must not be classified the same as a well-formed-but-expired one, got 'ExpiredNextTokenException'. Expected code ValidationException with message 'Invalid NextToken.'; the surrounding plain list_channels(MaxResults=1) positive controls both succeeded.

Cause 8 — AssertionError: expected the message 'DeleteStream can’t delete 000000000000:<stream> because there are consumers associated with this data stream. To delete <stream> anyway, set the EnforceConsumerDeletion parameter to true to force the deletion of all the consumers that are associated with it.', got '...to force the deletion of all the consumers associated with this data stream.'

Cause 9 — AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}} for tag_resource(ResourceARN=<stream arn>, Tags={'aws:reserved-behav-tag': 'x'}); expected 400 InvalidArgumentException "Some tag keys in the request started with 'aws:'. System tags cannot be modified."

## Steps to Reproduce
Cause 1: create a 1-shard ACTIVE stream; iam.create_role with a root-principal trust policy and an inline identity policy allowing kinesis:GetRecords/GetShardIterator/ListShards/DescribeStreamSummary on *; kinesis.put_resource_policy(ResourceARN=<stream arn>, Policy={Effect: Deny, Principal: {AWS: [<role arn>]}, Action: kinesis:GetRecords, Resource: <stream arn>}); put_record; read once as the owner (succeeds, as it must); sts.assume_role into the role and, with those credentials, get_shard_iterator + get_records -> observe repeated 200s with the record instead of 400 AccessDeniedException with the explicit-deny message.

Cause 2: on the same stream, put_resource_policy with a valid policy (accepted), then put_resource_policy with {"Version":"2012-10-17","Statement":[{"Sid":"X","Effect":"Permit","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"kinesis:DescribeStreamSummary","Resource":"<stream arn>"}]} -> observe 200 instead of 400 InvalidArgumentException, then get_resource_policy and see the invalid document stored.

Cause 3: put_resource_policy(ResourceARN=<stream arn>, Policy='{not json') -> read Error.Message of the 400 InvalidArgumentException and compare with 'Policy validation error: This policy contains invalid Json'.

Cause 4: put_resource_policy with a valid policy, delete_resource_policy, then delete_resource_policy again -> read Error.Message and compare with 'No resource policy found for resource ARN <arn>.'

Cause 5: create_stream(StreamName=<n>, ShardCount=1), wait ACTIVE, create_stream(StreamName=<n>, ShardCount=1) again -> read Error.Message of the 400 ResourceInUseException and compare with 'Stream <n> under account <account> already exists.'

Cause 6: create_channel(ChannelName=<c>, ServiceExecutionRoleARN=<role>, S3DestinationConfiguration=..., StreamConfigurationList=[{StreamARN: <stream arn>, ...}]); immediately create_channel again with the same ChannelName -> read Error.Message of the 400 ResourceInUseException and compare with 'Channel <c> under account <account> already exists.'

Cause 7: list_channels(MaxResults=1) (succeeds), then list_channels(NextToken='sim2real-not-a-real-next-token-deadbeef') -> observe Error.Code 'ExpiredNextTokenException' instead of 'ValidationException' with Message 'Invalid NextToken.'

Cause 8: create_stream (ON_DEMAND), wait ACTIVE, register_stream_consumer, wait consumer ACTIVE, delete_stream(StreamName=<n>) with EnforceConsumerDeletion unset -> read Error.Message of the 400 ResourceInUseException and compare its final clause with AWS's '...to force the deletion of all the consumers that are associated with it.'

Cause 9: tag_resource(ResourceARN=<stream arn>, Tags={'aws:anything': 'x'}) -> observe 200; list_tags_for_resource shows the reserved key stored. Expected 400 InvalidArgumentException "Some tag keys in the request started with 'aws:'. System tags cannot be modified." leaving the tag set unchanged.
