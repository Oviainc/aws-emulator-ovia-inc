Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.tag-key-and-value-contract-limits-rejected
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.tag-key-and-value-contract-limits-rejected|_error#16|AssertionError|raise AssertionError(f"expected a rejection, call succeeded with {result!r}")
Issue-Commit: 45bd6d867b1a7177de267d075c8182289448ffad

# config: TagResource accepts tag requests AWS refuses — a reserved aws: key and a key repeated in one call both answer 200 instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `45bd6d867b1a7177de267d075c8182289448ffad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
ONE root cause, grounded in the failing behavioral check config.tag-key-and-value-contract-limits-rejected. ONE CAUSE must be closed before submitting.

This check's replay key is no longer carried by any open issue: the failure has moved from a message-rendering assertion to the check's `_error` helper (the call is no longer refused at all), so the suite did not mark it owned. The nearest neighbour is the open issue 20261030T093351Z-config-6-net-new-behavioral-root-causes-putconfigrule.md, whose causes 5 and 6 describe the same two missing refusals on the same operation but are keyed to the earlier failure frames (`run#16|assert message == expected_message`); it does NOT cover this replay key, and 20261103T034739Z-config-listtagsforresource-tagresource-do-not-resolve-an-aggregationauthorization.md covers only ARN resolution for AggregationAuthorization, not tag-content validation.

--- Cause 1 of 1: TagResource performs no reserved-prefix check and no duplicate-key check, so two refusal classes AWS enforces are answered 200 with an empty body (check config.tag-key-and-value-contract-limits-rejected).
AWS: TagResource models ValidationException (client, httpError 400, defaulted) alongside ResourceNotFoundException and TooManyTagsException (Smithy: TagResource.errors; TagResource.Tags targets TagList @length 1..50 whose member is Tag{Key: TagKey @length 1..128, Value: TagValue @length 0..256}). Two refusals are Config's own domain checks, not single-member constraints: (a) a tag key in the reserved `aws:` namespace is refused 400 ValidationException with the fixed, request-independent sentence "One or more parameter value is invalid" (the offending key is NOT echoed); (b) the same tag key appearing twice in one Tags list is refused 400 ValidationException with the fixed sentence "Duplicate tags exists in tags list" (both recorded behavior; the model states only the error class and the 400). Every such refusal is ATOMIC: the resource's existing tag set is unchanged, and a key from a refused call is not present afterwards (ListTagsForResource still returns exactly the previously stored tags). The sibling length refusals in the same check are ValidationException with the generic "1 validation error detected: Value '<the full submitted value>' at 'tags.1.member.key|value' failed to satisfy constraint: Member must have length less than or equal to 128|256" — fixed text with the value and the member path taken from the request.
Emulator: the first refusal leg the check reaches is answered HTTP 200 with an empty JSON body (content-type application/x-amz-json-1.1, content-length 2, i.e. `{}`), and the check's helper reports `expected a rejection, call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/tag_resource.rs validates only the ARN shape (is_amazon_resource_name, line 183), a blank/whitespace key (line 193), and the per-resource tag quota (line 223) before merging; there is no reserved-prefix check anywhere, and the request list is collapsed into a key→value map by crate::state::tag_set (line 204) BEFORE any duplicate could be detected, so the last occurrence silently wins. Both of those legs therefore cannot be refused as written, and the tag set is mutated by a request AWS would have rejected.
Why: a caller can write into AWS's reserved `aws:` tag namespace on Config resources, so a resource ends up carrying tags real Config would never store (and which the caller cannot distinguish from CloudFormation's own aws: tags); and a duplicate key is silently de-duplicated with a 200, so part of a client's tag write is discarded without any signal. Because all of this operation's refusals share one error code, the message is the only thing that tells the classes apart — a 200 tells the client nothing at all, and any dashboard or policy keyed on reserved tags reads state AWS could not have produced.
Fix: TagResource's validation in services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/tag_resource.rs — detect a repeated key on the input list before the crate::state::tag_set collapse and refuse with ValidationException "Duplicate tags exists in tags list"; reject any requested key whose prefix is `aws:` with ValidationException "One or more parameter value is invalid". Both checks must run before any write to crate::state so the refusals are atomic. While validating this check end to end, confirm the length legs render AWS's full-value message (that rendering is the shared ValidationException value renderer, described as causes 4 and 6 of 20261030T093351Z-config-6-net-new-behavioral-root-causes-putconfigrule.md).
Traces (failing check ids, as examples): config.tag-key-and-value-contract-limits-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 124/179 passed, bootstrap_signal false (the emulator was serving; service `config` passed 115/161 checks). The check is listed under heading `### config` and was NOT marked ALREADY FILED or OWNED.

config.tag-key-and-value-contract-limits-rejected [mismatch]:
  Controls PASSED first: PutConfigurationAggregator created the taggable resource, and TagResource with a 128-character key and a 256-character value round-tripped byte-exact through ListTagsForResource (so ARN resolution and the happy path are fine).
  File "check_config_tag_key_and_value_contract_limits_rejected.py", line 89, in run -> line 15, in _error
  AssertionError: expected a rejection, call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', 'date': '...'}, 'RetryAttempts': 0}}
  The check drives four refusal legs in order against the same aggregator ARN, each requiring code ValidationException, the exact message, and an UNCHANGED tag set afterwards:
    leg 1  Tags=[{Key: 'k'+'a'*128 (129 chars), Value:'v'}]            -> "1 validation error detected: Value '<the full 129-char key>' at 'tags.1.member.key' failed to satisfy constraint: Member must have length less than or equal to 128"
    leg 2  Tags=[{Key:'k', Value:'v'+'b'*256 (257 chars)}]              -> "1 validation error detected: Value '<the full 257-char value>' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256"
    leg 3  Tags=[{Key:'aws:<suffix>', Value:'v'}]                        -> "One or more parameter value is invalid"
    leg 4  Tags=[{Key:'dupe',Value:'first'},{Key:'dupe',Value:'second'}] -> "Duplicate tags exists in tags list"
  One of these legs answered 200 '{}' instead of a 400 ValidationException, which aborted the loop at the helper. Legs 3 and 4 provably cannot be refused by the current handler: tag_resource.rs:183-237 contains an ARN-shape check, a blank-key check and a quota check only, and collapses the request list through crate::state::tag_set (line 204) before any duplicate detection, so an aws:-prefixed key is merged into state and a repeated key silently keeps its last value, both with 200.
  After the loop the check also requires UntagResource on the 128-char key to leave an empty tag set and UntagResource(['dupe']) to be a no-op — i.e. no tag from a refused call may be present.

Per-run values (the aggregator name from ctx.unique_name, the resource ARN, the account id, the `aws:` key suffix, the response date) are not part of the requirement; the requirements are the 400 status, the ValidationException code, the two fixed sentences above, and the unchanged tag set.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): TagResource.Tags -> TagList @length 1..50, member Tag{Key: TagKey @length 1..128, Value: TagValue @length 0..256}; TagResource errors ResourceNotFoundException, TooManyTagsException, ValidationException (all client, httpError 400 defaulted).

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator; replay simparity-behavioral-aws-v1.
2. PutConfigurationAggregator with one AccountAggregationSource for this account/region and keep the returned ConfigurationAggregatorArn.
3. TagResource(ResourceArn=<that arn>, Tags=[{Key:'aws:anything', Value:'v'}]) — expect 400 ValidationException with message exactly "One or more parameter value is invalid"; observe 200 with body '{}'. Then ListTagsForResource and confirm the aws: key was stored (AWS would leave the tag set unchanged).
4. TagResource(ResourceArn=<that arn>, Tags=[{Key:'dupe',Value:'first'},{Key:'dupe',Value:'second'}]) — expect 400 ValidationException with message exactly "Duplicate tags exists in tags list"; observe 200 with body '{}' and 'dupe'='second' stored.
5. Also drive the two length legs (a 129-character key, a 257-character value) and confirm each is 400 ValidationException whose message echoes the submitted value IN FULL at 'tags.1.member.key' / 'tags.1.member.value'.
6. Re-replay the check id in trace_ids to confirm the fix: config.tag-key-and-value-contract-limits-rejected.
