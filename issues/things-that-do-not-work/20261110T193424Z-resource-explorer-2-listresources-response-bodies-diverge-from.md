Issue-Source: trace-sweep-divergence
Issue-Trace: resource-explorer-2-tagged-queue-found-by-list-resources
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-explorer-2-tagged-queue-found-by-list-resources resource-explorer-2-view-scoped-list-resources
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2|ListResources|2xx->2xx|$.Resources[*].LastReportedAt
Issue-Key: resource-explorer-2|ListResources|2xx->2xx|$.Resources[*].Properties[*].Data
Issue-Key: resource-explorer-2|ListResources|2xx->2xx|$.Resources[*].Properties[*].LastReportedAt
Issue-Key: resource-explorer-2|ListResources|4xx->4xx|$.message
Issue-Commit: 7bca59ca3b47061319fdbc89b07cd36d64f78b10

# resource-explorer-2 ListResources response bodies diverge from the modeled wire shape

> _Note: this issue was identified against the code base at commit `7bca59ca3b47061319fdbc89b07cd36d64f78b10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResources

## Description
ListResources serializes its response body in shapes no AWS SDK client can consume: timestamps are emitted as numbers rather than date-time strings, the tags resource property is emitted as a JSON object rather than the key/value list AWS returns, and the 401 UnauthorizedException body carries no message member at all. Two independent causes, both in the resource-explorer-2 ListResources response path.

Cause 1 of 2: ListResources success body — LastReportedAt timestamp format and Properties[].Data document shape.
AWS: Smithy com.amazonaws.resourceexplorer2 — Resource.LastReportedAt and ResourceProperty.LastReportedAt both target Timestamp with @timestampFormat("date-time"), so on the wire they are ISO-8601 date-time STRINGS, never epoch numbers. ResourceProperty.Data is a @document whose content varies by resource type; for the tags property, recorded behavior only: AWS returns a JSON ARRAY of objects each having the members Key and Value (one element per tag on the resource), i.e. [{"Key": <tag key from the resource's tag set>, "Value": <that tag's value>}, ...]. Status 200, ListResourcesOutput.Resources per the model.
Emulator: status 200, but $.Resources[0].LastReportedAt and $.Resources[0].Properties[0].LastReportedAt are emitted as a floating-point epoch number instead of a date-time string, and $.Resources[0].Properties[0].Data is emitted as a flat single-level JSON OBJECT mapping tag key directly to tag value ({<tag key>: <tag value>}) instead of the list of {Key, Value} structures. (Replay reports the Data divergence as type_mismatch: expected a list of Key/Value objects, got an object; and both LastReportedAt members as value_mismatch against the modeled date-time shape.)
Why: a client deserializing ListResourcesOutput with any AWS SDK gets a parse failure or a wrong-typed field for LastReportedAt (SDKs expect a date-time string for a @timestampFormat date-time member and will not coerce a bare float), and any client walking a resource's tags via Properties[].Data iterates a list of {Key, Value} entries — against the emulator that loop sees object keys, not structures, so tag enumeration and tag-based filtering silently return nothing or crash.
Fix: the ListResources response serializer / its output projection for the resource-explorer-2 service — render both LastReportedAt members with the date-time timestamp format, and build the tags property Data document as a list of {Key, Value} structures from the indexed resource's tag set.
Traces: resource-explorer-2-tagged-queue-found-by-list-resources#task#5.

Cause 2 of 2: ListResources 401 UnauthorizedException body omits the error message.
AWS: Smithy com.amazonaws.resourceexplorer2 — ListResources declares the error UnauthorizedException (errorKind client, httpError 401) with a Message member, documented as "The principal making the request isn't permitted to perform the operation." The recorded response is HTTP 401 with the error message body member (serialized lowercase as `message`) set to the fixed text `Unauthorized` — that text is wholly fixed for this error class, with no variable part taken from the request or state.
Emulator: status 401 is correct, but the body omits the message member entirely ($.message missing; expected `Unauthorized`, got none), so the error carries a code and status with no message.
Why: a client relying on AWS's behavior reads the exception message to report and to classify the authorization failure; against the emulator every 401 from ListResources surfaces as an empty-message error, so logging, user-facing messages and any message-based handling lose the only human-readable signal the error has.
Fix: the UnauthorizedException serialization in the resource-explorer-2 ListResources error path — populate the error structure's Message member (wire name `message`) with `Unauthorized` when refusing the view-scoped call.
Traces: resource-explorer-2-view-scoped-list-resources#task#3.

Not covered by anything already open: the open issue "Implement resource-explorer-2 ListResources: unimplemented scaffold stub" (20261108T081111Z-resource-explorer-2-listresources-stub.md) is keyed on resource-explorer-2|ListResources|NotImplemented and carries only the behavioral unit resource-explorer-2.list-resources-without-view-or-default-succeeds — the operation now answers 200/401 rather than 501, and that issue carries neither of the wire replay keys above. The open issue 20261029T191823Z-resource-explorer-2-createview-refuses-with-400-validationexception-instead-of-creating.md already owns the CreateView 400 divergence in the same view-scoped fixture; it is deliberately not re-filed here.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up: true, 14 steps, 7 passed, score 50%).

Cause 1 — fixture resource-explorer-2-tagged-queue-found-by-list-resources, step task#5, ListResources, verdict mismatch, expected status 200 / actual status 200:
  $.Resources[0].LastReportedAt: value_mismatch (expected a date-time-shaped value, got 1794339185.3482478)
  $.Resources[0].Properties[0].Data: type_mismatch (expected [{'Key': <tag key>, 'Value': <tag value>}], got {<tag key>: <tag value>})
  $.Resources[0].Properties[0].LastReportedAt: value_mismatch (expected a date-time-shaped value, got 1794339185.3482478)
The epoch value and the concrete tag key/value in that line are this run's recorded data, not requirements; the requirement is the TYPE: ISO-8601 date-time string, and a list of Key/Value structures.
Model evidence: Resource.LastReportedAt -> Timestamp, timestampFormat date-time; ResourceProperty.LastReportedAt -> Timestamp, timestampFormat date-time; ResourceProperty.Data -> Document.

Cause 2 — fixture resource-explorer-2-view-scoped-list-resources, step task#3, ListResources, verdict mismatch, expected status 401 / actual status 401:
  $.message: missing (expected 'Unauthorized', got None)
Model evidence: ListResources errors include UnauthorizedException (client, httpError 401) with member Message.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture resource-explorer-2-tagged-queue-found-by-list-resources up to step task#5 — index a resource that carries tags, then call ListResources and inspect the returned Resources[0]. Observe LastReportedAt (on the resource and on its property) is a bare epoch number instead of an ISO-8601 date-time string, and Properties[0].Data is a flat {tagKey: tagValue} object instead of a list of {"Key": ..., "Value": ...} structures.
3. Cause 2: replay fixture resource-explorer-2-view-scoped-list-resources up to step task#3 — call ListResources with a ViewArn the caller is not permitted to use. Observe HTTP 401 whose JSON body has no `message` member, where AWS returns `message`: `Unauthorized`.
