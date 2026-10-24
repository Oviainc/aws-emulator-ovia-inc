Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-tagging-and-resource-policy
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-tagging-and-resource-policy
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ListTagsOfResource|2xx->2xx|$.Tags[*]
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# ListTagsOfResource returns tags at a step where AWS reported none

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsOfResource

## Description
1 cause, dynamodb.

Cause 1 of 1: ListTagsOfResource for a table serves the table's full tag set at a point in the recorded sequence where AWS answered with no tags at all.
AWS: recorded behavior only - Smithy: ListTagsOfResource input ResourceArn required (ResourceArnString @length 1..1283); output Tags targets TagList and is optional, and NextToken is optional, so the model does not require tags to be visible on the next call after they are written. The recording for this step is HTTP 200 with no Tags member present for the table ARN.
Emulator: HTTP 200 with Tags populated with the three tags the fixture attached earlier in the sequence (recorded example: env=probe, project=dynamodb-probe, team=platform), i.e. three entries where the recording has none. Status matches; only the body differs, by extra members.
Why: a client that uses ListTagsOfResource as the authority for "which tags are applied" observes a different intermediate state than against AWS: against AWS it must re-poll before the tag set appears, against the emulator the write is immediately visible, so retry/propagation logic and any assertion over the intermediate tag list behave differently in the two environments.
Hypothesis: DynamoDB's tag reads are eventually consistent, so the tags written a step earlier had not yet propagated when the response was recorded, while the emulator serves its tag state synchronously from the same store the write updated. The alternative reading is that the emulator attributes tags to an ARN form AWS treats as untagged. Severity is LOW because, under the first reading, the divergence is a propagation-timing artifact rather than a wrong tag set.
Fix: ListTagsOfResource's handler/tag state - reconcile tag-read visibility with the recorded sequence (and verify the ARN the tags are keyed under).
Traces: dynamodb-tagging-and-resource-policy#task#5.

Related but not covering: open issue 20261023T105249Z-dynamodb-tag-operations-skip-input-validation-and-misclassify-unknown.md owns the tag-operation validation and unknown-resource error divergences in this corpus (divergences 30, 35-37, 42); this cluster is reported unowned by the replay.

## Evidence
Divergence 38 (ListTagsOfResource, status 200 expected, 200 actual, fixture dynamodb-tagging-and-resource-policy#task#5): $.Tags[0] extra (expected None, got {'Key': 'env', 'Value': 'probe'}); $.Tags[1] extra (expected None, got {'Key': 'project', 'Value': 'dynamodb-probe'}); $.Tags[2] extra (expected None, got {'Key': 'team', 'Value': 'platform'}). affected_count 1, owned_by empty, stub_path empty.
Model: smithy_query(operation=ListTagsOfResource) -> input ResourceArn required (ResourceArnString 1..1283); output {Tags: TagList (optional), NextToken: NextTokenString}; errors InternalServerError, InvalidEndpointException, ResourceNotFoundException.

## Steps to Reproduce
1. Replay dynamodb-tagging-and-resource-policy up to task#5: after the fixture's tagging step, call ListTagsOfResource on the table ARN.
2. Observe 200 with three Tags entries (env/project/team) where the recorded response carries no Tags member.
