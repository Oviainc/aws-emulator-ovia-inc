Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.trail-concurrent-update-last-writer-wins cloudtrail.deploy-trail-cloudformation-behaves cloudtrail.deploy-trail-cloudformation-logging-behaves cloudtrail.op-contract-list-trails cloudtrail.put-insight-selectors-insufficient-s3-policy-fails cloudtrail.start-logging-revoked-bucket-permission-denied cloudtrail.add-tags-unknown-trail-arn-fails cloudtrail.channel-arn-format-validation-vs-notfound cloudtrail.channel-create-conflicting-name-blocked cloudtrail.channel-invalid-source-rejected cloudtrail.channel-lifecycle-create-active-delete-404 cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn cloudtrail.get-query-results-max-results-out-of-range-rejected cloudtrail.import-job-nonexistent-destination-and-notfound-lifecycle cloudtrail.import-lifecycle-start-complete-stop cloudtrail.import-start-invalid-time-range-rejected cloudtrail.list-insights-data-malformed-parameter-rejected cloudtrail.list-insights-data-unsupported-operation-rejected cloudtrail.start-import-malformed-manifest-rejected
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.trail-concurrent-update-last-writer-wins|run#23|AssertionError|assert len(failures) >= 1, (
Issue-Key: cloudtrail|GetTrail|TrailNotFoundException
Issue-Key: cloudtrail.deploy-trail-cloudformation-logging-behaves|run#13|AssertionError|assert len(trails) == 1, f"expected exactly 1 trail named {trail_name!r}, found {trails!r}"
Issue-Key: cloudtrail.op-contract-list-trails|_error_code#13|AssertionError|raise AssertionError(
Issue-Key: cloudtrail|DeleteBucketPolicy|NotImplemented
Issue-Key: cloudtrail.add-tags-unknown-trail-arn-fails|run#6|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: cloudtrail.channel-arn-format-validation-vs-notfound|run#5|AssertionError|assert malformed_code == "ChannelARNInvalidException", (
Issue-Key: cloudtrail.channel-create-conflicting-name-blocked|run#8|AssertionError|assert bad_source_code == "InvalidSourceException", f"expected InvalidSourceException, got {bad_source_code}"
Issue-Key: cloudtrail.channel-invalid-source-rejected|run#6|AssertionError|assert bad_code == "InvalidSourceException", (
Issue-Key: cloudtrail.channel-lifecycle-create-active-delete-404|run#7|AssertionError|assert malformed_code == "ChannelARNInvalidException", (
Issue-Key: cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn|run#3|AssertionError|assert create_code == "InvalidParameterException", (
Issue-Key: cloudtrail.get-query-results-max-results-out-of-range-rejected|run#4|AssertionError|assert code_ok == "QueryIdNotFoundException", f"expected QueryIdNotFoundException, got {code_ok}"
Issue-Key: cloudtrail.import-job-nonexistent-destination-and-notfound-lifecycle|run#13|AssertionError|assert start_code == "EventDataStoreNotFoundException", (
Issue-Key: cloudtrail.import-lifecycle-start-complete-stop|run#8|AssertionError|assert malformed_code == "EventDataStoreARNInvalidException", (
Issue-Key: cloudtrail.import-start-invalid-time-range-rejected|run#11|AssertionError|assert bad_range_code == "EventDataStoreNotFoundException", (
Issue-Key: cloudtrail.list-insights-data-malformed-parameter-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: cloudtrail.list-insights-data-unsupported-operation-rejected|run#6|AssertionError|assert eds_code == "InvalidParameterException", (
Issue-Key: cloudtrail.start-import-malformed-manifest-rejected|run#9|AssertionError|assert malformed_code == "EventDataStoreNotFoundException", (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudTrail has no UpdateTrail concurrency control, CloudFormation never provisions AWS::CloudTrail::Trail, authorization is never evaluated, and the Lake/channel/import surface plus S3 DeleteBucketPolicy answer NotImplemented

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTrail

## Description
Five distinct causes that block whole CloudTrail behavioral areas. There are 5 distinct causes in this issue; all 5 must be closed.

Cause 1 of 5 — concurrent UpdateTrail calls all succeed; no writer ever loses.
AWS: Smithy (com-amazonaws-cloudtrail-cloudtrail-20131101.json) binds UpdateTrail error ConflictException with httpError 409 / awsQueryError code ConflictException. Recorded behavior: with several UpdateTrail calls in flight against one trail, at least one loses and is refused with ConflictException 409; losers are rejected outright, never merged into the winner.
Emulator: all 5 concurrent UpdateTrail calls returned 200 and each reported its own value; zero refusals, so the last-writer-wins / reject-the-loser contract is never exercised and partial merges are possible.
Why: a client that handles ConflictException by re-reading and retrying never sees one, and concurrent configuration writes can interleave into a state no single request asked for.
Fix: UpdateTrail's handler/state must take the trail's write lock or version and return ConflictException (409) to the losers.
Traces: cloudtrail.trail-concurrent-update-last-writer-wins.

Cause 2 of 5 — CloudFormation does not provision AWS::CloudTrail::Trail.
AWS: recorded behavior only: a stack declaring AWS::CloudTrail::Trail reaches CREATE_COMPLETE and GetTrail on the declared name returns the trail; ListTrails contains exactly one trail with that name.
Emulator: after the stack operation, GetTrail raises TrailNotFoundException 404 ('Unknown trail: sim2real-cfn-trail-025a87363548 for the user: 000000000000') and the companion check found zero trails named by the template ('expected exactly 1 trail named ..., found []').
Why: CloudTrail resources declared in a template simply do not exist afterwards, so any infrastructure-as-code path that creates a trail is silently a no-op.
Fix: the CloudFormation resource-provider mapping for AWS::CloudTrail::Trail must call CloudTrail's CreateTrail (and StartLogging where the template asks for it) and register the physical resource id.
Traces: cloudtrail.deploy-trail-cloudformation-behaves, cloudtrail.deploy-trail-cloudformation-logging-behaves.

Cause 3 of 5 — no authorization is evaluated on CloudTrail reads.
AWS: recorded behavior only: ListTrails called by an identity with no policy granting cloudtrail:ListTrails is refused with AccessDenied (HTTP 403); the response carries no trail list.
Emulator: ListTrails by an identity with no attached policy returned HTTP 200 and the caller's trails. The open issue 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md records the same absence for IAM-side operations and owns several other checks in this suite; it does not carry this check's key, so this cause is filed here and should be fixed together with it.
Why: every negative authorization path in a client is untestable, and a least-privilege test suite passes against the emulator while failing on AWS.
Fix: the request-authorization layer must evaluate the caller's policy for CloudTrail actions and refuse with AccessDenied 403.
Traces: cloudtrail.op-contract-list-trails.

Cause 4 of 5 — S3 DeleteBucketPolicy is an unimplemented scaffold stub, so CloudTrail's revoked-permission behavior cannot be reached.
AWS: recorded behavior only: DeleteBucketPolicy removes the bucket policy (HTTP 204); CloudTrail then refuses PutInsightSelectors with InsufficientS3BucketPolicyException, and StartLogging/GetTrailStatus report the delivery failure. Both checks revoke the bucket policy as their setup step.
Emulator: ClientError (NotImplemented) when calling the DeleteBucketPolicy operation: 'operation DeleteBucketPolicy is scaffolded but not implemented in this emulator'. NotImplemented is not a modelled S3 error. These checks are CloudTrail checks, but the stub is in the S3 crate and the suite did not mark them as covered by any filed stub issue.
Why: the whole 'permission revoked after the trail was created' class of CloudTrail behavior is unreachable, and any client that tears down a bucket policy gets an error code no SDK models.
Fix: implement S3 DeleteBucketPolicy in services/service-com-amazonaws-s3-amazons3/src/ops (the bucket-policy handler), then the CloudTrail insufficient-policy refusals these checks assert.
Traces: cloudtrail.put-insight-selectors-insufficient-s3-policy-fails, cloudtrail.start-logging-revoked-bucket-permission-denied.

Cause 5 of 5 — the CloudTrail Lake, channel, import, query and tagging operations are scaffold stubs, so their modelled refusals are replaced by NotImplemented.
AWS: Smithy binds specific client errors on each of these operations, e.g. ChannelARNInvalidException and ChannelNotFoundException on the channel operations, InvalidSourceException on CreateChannel, EventDataStoreARNInvalidException and EventDataStoreNotFoundException on the event-data-store and import operations (httpError 400/404 with awsQueryError codes EventDataStoreARNInvalid / EventDataStoreNotFound), InvalidParameterException on CreateEventDataStore, QueryIdNotFoundException on GetQueryResults, ValidationException on the Insights data operations.
Emulator: every one of these calls answers error code 'NotImplemented' with 'operation <Op> is scaffolded but not implemented in this emulator'. Thirteen checks in this suite reach such a stub without being marked as covered by the per-operation stub issues (20261022T230533Z-cloudtrail-*-stub.md and 20261022T215433Z-cloudtrail-data-*), so they are grouped here; the per-operation stub issues cover the implementation, and this cause is the modelled refusal semantics each handler must then produce.
Why: NotImplemented is not in any CloudTrail error model, so no SDK branch matches it; a client cannot distinguish a bad argument from an unbuilt API, and every negative class on the Lake/channel/import surface is dead.
Fix: implement the named handlers under services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops and have them raise the modelled identifier-format, not-found and source-validation errors.
Traces: cloudtrail.add-tags-unknown-trail-arn-fails, cloudtrail.channel-arn-format-validation-vs-notfound, cloudtrail.channel-create-conflicting-name-blocked, cloudtrail.channel-invalid-source-rejected, cloudtrail.channel-lifecycle-create-active-delete-404, cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn, cloudtrail.get-query-results-max-results-out-of-range-rejected, cloudtrail.import-job-nonexistent-destination-and-notfound-lifecycle, cloudtrail.import-lifecycle-start-complete-stop, cloudtrail.import-start-invalid-time-range-rejected, cloudtrail.list-insights-data-malformed-parameter-rejected, cloudtrail.list-insights-data-unsupported-operation-rejected, cloudtrail.start-import-malformed-manifest-rejected.

## Evidence
Cause 1: "expected at least one racer to lose the race with ConflictException -- got zero failures among 5 concurrent UpdateTrail calls, so the contract under test (losers are rejected, never merged) was never exercised: [('ok', ('sim2real-racer-0', True)), ('ok', ('sim2real-racer-1', False)), ('ok', ('sim2real-racer-2', True)), ('ok', ('sim2real-racer-3', False)), ('ok', ('sim2real-racer-4', True))]".
Cause 2: botocore.errorfactory.TrailNotFoundException: An error occurred (TrailNotFoundException) when calling the GetTrail operation: Unknown trail: sim2real-cfn-trail-025a87363548 for the user: 000000000000; and "expected exactly 1 trail named 'sim2real-ct-cfn-t-2245d362bdd4', found []".
Cause 3: "ListTrails by an identity with no attached policy succeeded and returned {'Trails': [{'TrailARN': 'arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-ct-trail-539ce9a32c61', ...}], 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}" — expected AccessDenied 403.
Cause 4: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteBucketPolicy operation: operation DeleteBucketPolicy is scaffolded but not implemented in this emulator (both checks).
Cause 5: representative details — "expected GetChannel on a malformed identifier to fail with ChannelARNInvalidException, got 'NotImplemented'"; "expected InvalidSourceException, got NotImplemented"; "expected an unregistered Source to be rejected with InvalidSourceException, got 'NotImplemented'"; "expected ChannelARNInvalidException, got NotImplemented"; "positive control failed: expected CreateEventDataStore to reject with InvalidParameterException (account ineligible for new EDS), got NotImplemented"; "expected QueryIdNotFoundException, got NotImplemented"; "expected StartImport against a nonexistent event data store to fail with EventDataStoreNotFoundException, got 'NotImplemented'"; "expected EventDataStoreARNInvalidException, got NotImplemented"; "expected EventDataStoreNotFoundException, got NotImplemented"; "expected ValidationException, got NotImplemented"; "expected the destination-existence check to dominate a malformed S3 source, got 'NotImplemented'"; AddTags: "AssertionError: NotImplemented".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay the checks in trace_ids.
3. Manually: (a) fire 5 concurrent UpdateTrail calls at one trail and count refusals -> expect >=1 ConflictException 409, observe 0; (b) CreateStack with a template declaring AWS::CloudTrail::Trail, then GetTrail on the declared name -> expect the trail, observe TrailNotFoundException 404; (c) call ListTrails with credentials carrying no CloudTrail permission -> expect AccessDenied 403, observe 200 with the trail list; (d) s3 DeleteBucketPolicy on any bucket -> expect 204, observe NotImplemented; (e) call GetChannel/CreateChannel/CreateEventDataStore/StartImport/GetQueryResults/ListInsightsData/AddTags with malformed or unknown identifiers -> expect the modelled 400/404 codes, observe NotImplemented.
