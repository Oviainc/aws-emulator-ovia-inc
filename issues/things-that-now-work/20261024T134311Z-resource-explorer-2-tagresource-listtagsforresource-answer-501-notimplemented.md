Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-explorer-2__sns__sqs.tag-resource-rejects-foreign-service-arn resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results
Issue-Key: resource-explorer-2__sns__sqs.tag-resource-rejects-foreign-service-arn|run#7|AssertionError|assert code == "UnauthorizedException", (
Issue-Key: resource-explorer-2__sns__sqs|GetIndex|NotImplemented
Issue-Commit: 7bcb53c96ea0ea6c32faf644dec1ae2940515247

# resource-explorer-2: TagResource/ListTagsForResource answer 501 NotImplemented, and GetIndex is not routable at all (2 causes)

> _Note: this issue was identified against the code base at commit `7bcb53c96ea0ea6c32faf644dec1ae2940515247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two distinct net-new root causes in the resource-explorer-2 emulator, both surfaced by the cross-service behavioral checks. Neither is covered by the resolved bootstrap issue 20261021T212827Z-bootstrap-service-com-amazonaws-resourceexplorer2-resourceexplorer.md: that issue scoped itself to the CreateIndex/CreateView/ListViews/GetView/Search/DeleteView slice and explicitly deferred "ListResources, Tag/UntagResource, ListTagsForResource" and GetIndex to later gap issues. This is that filing, grounded in the two failing checks below. This is NOT a REOPENS: the fixed bootstrap issue never claimed to implement these operations.

=== Cause 1 of 2: TagResource (and its read-back ListTagsForResource) is an unimplemented scaffold, so the resource-ownership refusal and the tagging happy path are both unreachable. Failing check: resource-explorer-2__sns__sqs.tag-resource-rejects-foreign-service-arn ===

AWS: Smithy model (com-amazonaws-resourceexplorer2-resourceexplorer) binds TagResource to POST /tags/{resourceArn} with resourceArn as a @required @httpLabel and Tags: TagMap (@sensitive), empty output, success 200, and models the errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ThrottlingException(429), UnauthorizedException(401) and ValidationException(400). Resource Explorer only owns index/* and view/* ARNs in its own arnNamespace, so a resourceArn naming a FOREIGN service's resource (here an SNS topic ARN) is outside the resources this operation can authorize against and AWS refuses it with the modeled UnauthorizedException, HTTP 401 — the requirement is that code and status. Recorded message for that refusal is "Unauthorized" (an example of the text; the requirement is the code/status, and the check does compare the text). The refusal must have NO effect: the SNS topic's own tag view (sns ListTagsForResource) must still report an empty Tags list. The identical call against the caller's OWN view ARN must succeed (200, empty body) and persist, so that resource-explorer-2 ListTagsForResource (GET /tags/{resourceArn}, output Tags: TagMap) reads back exactly {"k":"v"}.

Emulator: TagResource answers HTTP 501 with error code NotImplemented for every input. Replay: `AssertionError: expected TagResource against a foreign-service (SNS) ARN to be rejected with UnauthorizedException, got 'NotImplemented'`. Because the operation is a stub, neither the foreign-ARN refusal nor the own-ARN positive control (tag then read back through ListTagsForResource) can be exercised at all.

Why: a client cannot tag or untag any Resource Explorer index or view, and cannot read a resource's tags back, so tag-driven setup and tag-based assertions silently fail. Worse for negative paths: code that relies on AWS refusing a cross-service ARN with UnauthorizedException (401) instead sees NotImplemented (501), which is a server-class code — SDK retry policies and error branches treat 501/NotImplemented completely differently from a 401 client refusal, so an authorization bug in client code is masked as an emulator gap and never routed to its handler.

Fix: the operation's handler plus the service's tagging state — implement TagResource (and the symmetric ListTagsForResource/UntagResource read-back it is graded against) over the per-resource tag maps already described in the bootstrap issue's state model: resolve resourceArn to an index or view held in state, refuse an ARN whose service/namespace is not resource-explorer-2 with UnauthorizedException(401) before any mutation, refuse an unknown but well-formed own-namespace ARN with ResourceNotFoundException(404) on the read path, and otherwise merge the Tags map into the stored resource's tags so ListTagsForResource returns it. Remove the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker once implemented.

=== Cause 2 of 2: GetIndex is not wired into the service at all, so it is answered by the framework's unroutable-operation path and every index-lifecycle wait is unreachable. Failing check: resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results ===

AWS: Smithy model binds GetIndex to POST /GetIndex with a Unit (absent) input and GetIndexOutput { Arn, Type: IndexType, State: IndexState, ReplicatingFrom, ReplicatingTo, CreatedAt (@timestampFormat date-time), LastUpdatedAt, Tags }, success 200, modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). With an index in the account/region the call must return 200 and the index's Arn and State, reaching ACTIVE; with no index in the region it must return the modeled ResourceNotFoundException (HTTP 404). Every Resource Explorer client polls GetIndex to learn when a freshly created index is usable.

Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetIndex operation: operation POST /GetIndex is not implemented in this emulator`. This is not even a scaffolded handler: services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/SKIPPED_STUBS.md records `// SKIPPED: GetIndex — unsupported shape construct: operation 'GetIndex' has a Unit/absent input, which the generator does not emit yet`, so there is no ops/get_index.rs and the operation is absent from the service's operations(...) allow-list; the framework's unroutable-operation path answers it. The check dies on its first GetIndex poll, before it can create the index, index the SNS topic and the SQS queue, or build the resourcetype:sqs:queue view — so the whole cross-service view-scoping behavior behind it is unobserved.

Why: an index whose state can never be read is an index a client can never wait for. Clients that follow the documented create-then-poll pattern (CreateIndex, then GetIndex until State == ACTIVE) hang or abort on the first poll, and clients that branch on ResourceNotFoundException to decide whether to create an index can never take either branch. Because the response is the framework's generic NotImplemented rather than a modeled error, it is also indistinguishable from a transport/routing fault.

Fix: hand-author the operation's handler — add ops/get_index.rs (Unit input, GetIndexOutput) using a rendered sibling as the pattern, register the operation in the service's operations(...) allow-list so it stops falling through the unroutable path, and compute the response from the single-index-per-account+region entry in crate::state::service_state (Arn, Type, State, CreatedAt, LastUpdatedAt, ReplicatingFrom/To, Tags), returning ResourceNotFoundException(404) when the region holds no index. The same Unit-input decline blocks GetDefaultView, GetServiceIndex, GetAccountLevelServiceConfiguration and DisassociateDefaultView, so the wiring fix is shared; only GetIndex is evidenced by a failing check here.

Traces: resource-explorer-2__sns__sqs.tag-resource-rejects-foreign-service-arn (Cause 1); resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results (Cause 2). Stub evidence: services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/tag_resource.rs lines 3-6; services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/SKIPPED_STUBS.md line 24.

## Evidence
Cause 1 — check resource-explorer-2__sns__sqs.tag-resource-rejects-foreign-service-arn, verdict mismatch.
  Expected (AWS): rex.tag_resource(resourceArn=<sns topic arn>, Tags={"k":"v"}) -> error code UnauthorizedException, HTTP 401 (recorded message example: "Unauthorized"); SNS's own list_tags_for_resource on the topic still returns Tags=[]; then rex.tag_resource(resourceArn=<own view arn>, Tags={"k":"v"}) -> 200 and rex.list_tags_for_resource(resourceArn=<own view arn>) -> Tags={"k":"v"}.
  Actual (emulator): HTTP 501, error code NotImplemented, for the foreign-ARN call. Replay assertion: "expected TagResource against a foreign-service (SNS) ARN to be rejected with UnauthorizedException, got 'NotImplemented'". The positive control is never reached. Source: src/ops/tag_resource.rs carries "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`" (lines 3-6).

Cause 2 — check resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results, verdict error.
  Expected (AWS): POST /GetIndex -> 200 with GetIndexOutput { Arn, Type, State (reaching ACTIVE), CreatedAt, LastUpdatedAt, Tags }, or ResourceNotFoundException (404) when the region has no index.
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetIndex operation: operation POST /GetIndex is not implemented in this emulator. Not a scaffolded handler but an unrouted operation — src/ops/SKIPPED_STUBS.md line 24: "// SKIPPED: GetIndex — unsupported shape construct: operation 'GetIndex' has a Unit/absent input, which the generator does not emit yet", and the file's preamble states such an op is "modeled-but-unwired" and "answered by the framework's unroutable-operation path".
  Consequence: the check aborts on its first _create_index_and_wait_active poll, so the downstream cross-service claim (a view with Filters.FilterString='resourcetype:sqs:queue' returns the SQS queue and never the SNS topic indexed alongside it) is never exercised.

Scope note: the resolved issue 20261021T212827Z-bootstrap-service-com-amazonaws-resourceexplorer2-resourceexplorer.md states verbatim that "BatchGetView, UpdateView, UpdateIndexType, ListResources, Tag/UntagResource, ListTagsForResource, the managed/service-view and setup-task families, ..." stay 501 and are to be filed as individual gap issues, and that GetIndex is "not even wired (Unit input), which is why the happy path above avoids them". Both causes here are therefore net-new work, not a regression of that fix.

## Steps to Reproduce
Cause 1 (TagResource / ListTagsForResource):
1. Boot the emulator and point an AWS SDK/CLI client at it with --endpoint-url, services sns and resource-explorer-2.
2. sns create-topic --name rex-tag-foreign-probe -> note TopicArn.
3. aws resource-explorer-2 tag-resource --resource-arn <TopicArn> --tags k=v
   Expect: error code UnauthorizedException, HTTP 401 (recorded message example "Unauthorized").
   Today: HTTP 501, error code NotImplemented.
4. sns list-tags-for-resource --resource-arn <TopicArn> -> expect Tags=[] (the refusal must attach nothing anywhere).
5. Positive control: create an index (create-index), poll get-index until State=ACTIVE, create-view --view-name rex-tag-foreign-probe -> note View.ViewArn.
6. aws resource-explorer-2 tag-resource --resource-arn <ViewArn> --tags k=v -> expect 200.
7. aws resource-explorer-2 list-tags-for-resource --resource-arn <ViewArn> -> expect Tags={"k":"v"}.

Cause 2 (GetIndex):
1. Boot the emulator, service resource-explorer-2.
2. aws resource-explorer-2 get-index
   Expect, with no index in the region: ResourceNotFoundException, HTTP 404.
   Expect, after create-index: HTTP 200 with Arn, Type, State progressing to ACTIVE, CreatedAt, LastUpdatedAt, Tags.
   Today (both cases): error code NotImplemented, "operation POST /GetIndex is not implemented in this emulator".
3. Re-replay resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results after wiring GetIndex, so the downstream resourcetype:sqs:queue view-scoping assertions actually run (they are currently unobserved and may expose further defects in ListResources / CreateView Filters).
