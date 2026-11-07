Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__resourcegroupstaggingapi.blueprint-tag-applied-after-create-surfaces-in-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-deleted-function-leaves-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-key-filter-is-case-sensitive lambda__resourcegroupstaggingapi.blueprint-tag-key-filter-selects-only-the-tagged-function lambda__resourcegroupstaggingapi.blueprint-tag-keys-and-values-summaries-report-the-colon-key lambda__resourcegroupstaggingapi.blueprint-tag-query-is-region-scoped lambda__resourcegroupstaggingapi.blueprint-tag-query-paginates-one-function-per-page lambda__resourcegroupstaggingapi.blueprint-tag-resource-type-filter-absence-widens-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-untag-removes-the-function-from-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-value-filter-matches-exact-strings-as-a-union lambda__resourcegroupstaggingapi.blueprint-tag-versions-and-aliases-do-not-surface-as-tagged-resources
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-applied-after-create-surfaces-in-the-tag-query|_poll#5|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-deleted-function-leaves-the-tag-query|_poll#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-key-filter-is-case-sensitive|_poll#3|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-key-filter-selects-only-the-tagged-function|_poll#5|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-keys-and-values-summaries-report-the-colon-key|_poll#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-query-is-region-scoped|_poll#5|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-query-paginates-one-function-per-page|_poll#5|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-resource-type-filter-absence-widens-the-tag-query|_poll#13|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-untag-removes-the-function-from-the-tag-query|_poll#5|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-value-filter-matches-exact-strings-as-a-union|_poll#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-versions-and-aliases-do-not-surface-as-tagged-resources|_poll#7|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# Lambda functions are never indexed by the Resource Groups Tagging API — GetResources/GetTagKeys/GetTagValues see no tagged function

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Cause 1 of 1: a tag written on a Lambda function — whether at CreateFunction time, through lambda:TagResource, or through the tagging API itself — never appears in the Resource Groups Tagging API's index, so every tag query over functions returns nothing.

AWS: recorded behavior only. Tags on an `arn:aws:lambda:...:function:<name>` resource are indexed by the Resource Groups Tagging API within the service's indexing delay: GetResources with TagFilters (key-only, key+values union, with or without ResourceTypeFilters) returns the function ARN and its tags, GetTagKeys/GetTagValues report the key and its values, the index is region-scoped, it paginates, untagging removes the resource from the result, deleting the function removes it, and function *versions*/aliases do not surface as separately tagged resources. All eleven recordings poll GetResources/GetTagValues for up to 120 s before asserting.

Emulator: nothing is ever indexed. Every one of the eleven checks fails on the same empty result after the full polling window — `AssertionError: ... not met within 120.0s. Last result: {}` / `Last result: []`. Where a second resource type is in the same query, only the non-Lambda resource is returned (`Last result: ['arn:aws:sqs:us-west-2:000000000000:sim2real-scn-bp-q-<hex>']`), which shows the index exists but has no Lambda entries. The CloudFormation leg additionally shows no system tags on the deployed function (`CloudFormation must add exactly its three system tags to the tagged function; got []`), and `Lambda ListTags must see the tag; got {}`.

Why: a client that uses tags to find, bill, govern or sweep its functions finds none of them. Cost allocation by tag reports zero Lambda spend, tag-based cleanup scripts delete nothing (or, if they treat an empty result as "all gone", delete the wrong thing), tag-policy compliance scans pass vacuously, and because the result is an empty success rather than an error the client has no signal that the index is simply not populated for Lambda.

Fix: tag propagation from the Lambda tag plane into the Resource Groups Tagging API index — CreateFunction Tags, TagResource/UntagResource and DeleteFunction must add/update/remove the function ARN's entry, scoped to the region, and function versions/aliases must not be indexed as separate resources.

Traces: the eleven blueprint-tag-* checks listed in trace_ids.

## Evidence
Expected: GetResources with TagFilters [{Key: 'lambda-console:blueprint'}] returns the tagged function's ARN with its tags; GetTagValues('lambda-console:blueprint') lists the value; untagging/deleting removes the entry; versions and aliases do not appear.
Actual: `Last result: {}` / `Last result: []` after 120 s on every leg; in a mixed query only the SQS queue is returned (`['arn:aws:sqs:us-west-2:000000000000:sim2real-scn-bp-q-<hex>']`); `Lambda ListTags must see the tag; got {}`.

## Steps to Reproduce
1. CreateFunction with Tags={'lambda-console:blueprint':'<value>'} (and a second function tagged via lambda:TagResource after create).
2. Call tagging-api GetResources with TagFilters=[{Key:'lambda-console:blueprint'}] and GetTagKeys / GetTagValues(Key='lambda-console:blueprint').
3. The function ARN never appears and the value is never listed, for at least 120 s.
