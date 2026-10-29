Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results
Issue-Key: resource-explorer-2__sns__sqs|ListResources|NotImplemented
Issue-Commit: 5289dd65a4144c77c241afe8c5587a00323c4392

# ListResources is a scaffold stub: every Resource Explorer resource query answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `5289dd65a4144c77c241afe8c5587a00323c4392`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResources

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: `ListResources` on Resource Explorer 2 is still the generated scaffold and refuses every call, so no resource can be read out of an index or through a view.

AWS: Smithy model `com.amazonaws.resourceexplorer2#ListResources` — HTTP POST /ListResources, success status 200, input `ListResourcesInput{Filters: SearchFilter (sensitive; SearchFilter.FilterString @required @length 0..2048), MaxResults: Integer @range 1..1000, ViewArn: String @length 0..1000, NextToken: String @length 1..2048}`, output `ListResourcesOutput{Resources: ResourceList, NextToken: String, ViewArn: String}`, `@paginated(inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults, items: Resources)`, `@readonly`. Each `Resource` carries `Arn`, `OwningAccountId`, `Region`, `ResourceType`, `Service`, `CfnResourceType`, `LastReportedAt` (date-time) and `Properties`. Per the model's own documentation for the operation, the query must use a view: when `ViewArn` is omitted the operation uses the Region's default view, and when the Region has no default view (or the caller may not use it) the operation fails with a 401 `UnauthorizedException`. The only modeled refusals are `AccessDeniedException` (403), `InternalServerException` (500), `ResourceNotFoundException` (404), `ThrottlingException` (429), `UnauthorizedException` (401) and `ValidationException` (400); `NotImplemented`/501 is not among them. The operation's documented filtering contract is that `Filters.FilterString` supports filter prefixes with optional operators and no free-form text (e.g. `service:sqs`, `resourcetype:sqs:queue`, `region:us* service:ec2 -tag:stage=prod`), and that a view's own filter string scopes what the view can return — so a view created with `resourcetype:sqs:queue` must return the account's SQS queues and must never return resources of another type/service (an SNS topic) indexed in the same index, with every returned `Resource.ResourceType` equal to `sqs:queue` and `Resource.Service` equal to `sqs`.

Emulator: every `ListResources` call is refused with HTTP 501 and error code `NotImplemented`, message `operation ListResources is scaffolded but not implemented in this emulator`. Replay shows the very first probe fail: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListResources operation: operation ListResources is scaffolded but not implemented in this emulator`. The handler body still returns `Scaffolded::not_implemented("ListResources")` and the file still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker. In that message, the fixed text is AWS's error-class wording for the modeled error that actually applies (there is none for a successful query — the success path must return 200 with `Resources`); the operation name is the only request-derived part of the emulator's string.

Why: a client gets a hard, non-modeled 501 for the service's primary read path. Resource Explorer is only usable through `ListResources`/`Search`, so index content cannot be enumerated, view scoping cannot be observed, and pagination over `NextToken` can never be driven. `NotImplemented` is not in the operation's modeled error set, so SDK error handling (retry classification, `ResourceNotFoundException`/`UnauthorizedException`/`ValidationException` branches) never matches and callers surface an unrecoverable client error instead of either results or a modeled refusal.

Fix: implement the operation's handler — `ListResources::handle` in services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/list_resources.rs — reading the indexed resources from this service's state (`crate::state::service_state`), resolving `ViewArn` (or the Region default view, else 401 `UnauthorizedException`), intersecting the view's filter string with the request `Filters.FilterString`, projecting each hit into `Resource` with `Arn`/`ResourceType`/`Service`/`Region`/`OwningAccountId`/`LastReportedAt` populated consistently, paginating by `MaxResults` with an opaque `NextToken`, and echoing the used `ViewArn`; then delete the SCAFFOLDED marker line so the stub inventory stays accurate. Filter-string parsing (prefix tokens `service:`, `resourcetype:`, `region:`, `tag:`, negation with `-`, `*` suffix wildcards) belongs in that handler's validation/filter component, and refusals for a malformed filter string belong there as 400 `ValidationException` — never 501.

Traces: resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results (the check creates an index, waits for it to go ACTIVE, creates an SNS topic and an SQS queue, then calls ListResources to confirm a `resourcetype:sqs:queue` view returns the queue and never the topic). Note: no open issue names this operation; the replay report did not mark this failure as already filed or owned.

## Evidence
Failing check: resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results (verdict: error).

Expected (AWS / Smithy): HTTP 200 with ListResourcesOutput{Resources:[...], ViewArn, optional NextToken}; refusals only from the modeled set AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), UnauthorizedException(401), ValidationException(400). For a view whose filter string is `resourcetype:sqs:queue`, the returned Resources must include the account's SQS queue ARN, must exclude the SNS topic ARN indexed in the same index, and every element must report ResourceType='sqs:queue' and Service='sqs'.

Actual (emulator): HTTP 501, Error.Code='NotImplemented', Error.Message='operation ListResources is scaffolded but not implemented in this emulator'. Raised on the first ListResources probe:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListResources operation: operation ListResources is scaffolded but not implemented in this emulator

Code evidence: services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/list_resources.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`) and line 204 (`Err(crate::scaffolded::Scaffolded::not_implemented("ListResources"))`).

Message-part breakdown of the diverging string: fixed text = 'operation ... is scaffolded but not implemented in this emulator' (emulator-invented, not AWS wording for any modeled error); variable part = the operation name 'ListResources'. No AWS error class uses this text, so the whole message is a divergence, not a per-run value difference.

## Steps to Reproduce
1. Boot the emulator and point a botocore/SDK client at it with service name `resource-explorer-2` in the check's region.
2. CreateIndex (Tags={'sim2real': ...}) and poll GetIndex until State == 'ACTIVE' (this succeeds today).
3. Create an SNS topic and an SQS queue in the same account/region so both are indexed; read the queue ARN via sqs.GetQueueAttributes(AttributeNames=['QueueArn']).
4. Call resource-explorer-2 ListResources(MaxResults=100, Filters={'FilterString': 'service:sqs'}).
   Observed: HTTP 501 ClientError NotImplemented — 'operation ListResources is scaffolded but not implemented in this emulator'.
   Expected: HTTP 200 with Resources containing the queue ARN (ResourceType='sqs:queue', Service='sqs') and a ViewArn naming the default view.
5. Call CreateView(ViewName=..., Filters={'FilterString': 'resourcetype:sqs:queue'}) then ListResources(ViewArn=<that view>, MaxResults=100).
   Expected: the queue ARN present, the SNS topic ARN absent, and every Resource reporting ResourceType='sqs:queue' and Service='sqs'.
6. Re-replay check resource-explorer-2__sns__sqs.view-filter-string-scopes-search-results to confirm.
