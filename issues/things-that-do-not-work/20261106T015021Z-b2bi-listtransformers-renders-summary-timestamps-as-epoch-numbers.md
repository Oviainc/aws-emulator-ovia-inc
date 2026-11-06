Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-transformer-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-transformer-lifecycle
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|ListTransformers|2xx->2xx|$.transformers[name="sim2real-probe-b2bi-transformer-254df7a9"].createdAt
Issue-Key: b2bi|ListTransformers|2xx->2xx|$.transformers[name="sim2real-probe-b2bi-transformer-254df7a9"].modifiedAt
Issue-Commit: a8b0ef515115c35bd51152558bfbd1aa7e5d03e4

# b2bi ListTransformers renders summary timestamps as epoch numbers

> _Note: this issue was identified against the code base at commit `a8b0ef515115c35bd51152558bfbd1aa7e5d03e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTransformers

## Description
One cause: the b2bi ListTransformers list projection serializes each transformer summary's timestamp members as JSON numbers instead of ISO-8601 date-time strings.

Cause 1 of 1: ListTransformers renders every TransformerSummary's createdAt and modifiedAt as a bare JSON float (epoch seconds with a fractional part).

AWS: Smithy services/service-com-amazonaws-b2bi-b2bi (com.amazonaws.b2bi#B2BI, protocol awsJson1_0). TransformerSummary.createdAt targets com.amazonaws.b2bi#CreatedDate and TransformerSummary.modifiedAt targets com.amazonaws.b2bi#ModifiedDate; both members carry @timestampFormat("date-time"), and the member doc states "Returns a timestamp indicating when the transformer was created. For example, 2023-07-20T19:58:44.624Z". So each entry of ListTransformersResponse.transformers (target TransformerList, required) must carry createdAt, and modifiedAt when set, as an ISO-8601 date-time STRING of that shape. createdAt is @required on the summary, modifiedAt is optional but the recording shows it populated. HTTP 200 on success, which the emulator already returns. The literal instant in any recording is a per-run value; only the FORMAT is the requirement.

Emulator: the replayed ListTransformers answers HTTP 200 but renders $.transformers[].createdAt and $.transformers[].modifiedAt as JSON numbers (observed 1793929777.036583 and 1793929777.0456338 for the probe transformer), not quoted date-time strings.

Why: an SDK deserializing CreatedDate/ModifiedDate per @timestampFormat("date-time") expects a string; given a number it either throws a parse error or mis-reads the value, so any client that lists b2bi transformers to display, sort or compare creation/modification times fails or shows garbage. Because the single-resource paths (Get/Create/Update) have their own issue, a client can also see the same resource with two different timestamp encodings depending on which call it made.

Fix: the b2bi ListTransformers response serializer — specifically the TransformerSummary timestamp rendering in the list projection, which must use the same @timestampFormat("date-time") rendering as the single-resource Transformer serializer rather than emitting the stored epoch value.

Related but NOT covering this: open issue 20261029T171003Z-b2bi-mutating-responses-use-http-201-epoch-number.md files the same timestamp fault for CreateProfile/GetProfile/UpdateProfile/ListProfiles and Create/Get/UpdateTransformer; its Issue-Key list includes ListProfiles but has no ListTransformers key, so the ListTransformers list projection is unclaimed and is filed here.

Traces: b2bi-transformer-lifecycle#task#5.

## Evidence
Replay of simparity-wire-aws-v1 (trace_set_id simparity-wire-aws-v1, emulator_up true, score 73.2%). Cluster divergence_key service=b2bi|ListTransformers|status=200->200|keys=$.transformers[name="..."].createdAt,$.transformers[name="..."].modifiedAt, verdict mismatch, owned_by empty, stub_path empty.

Step b2bi-transformer-lifecycle#task#5 ListTransformers: expected_status 200, actual_status 200. Detail:
  $.transformers[name="sim2real-probe-b2bi-transformer-254df7a9"].createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1793929777.036583)
  $.transformers[name="sim2real-probe-b2bi-transformer-254df7a9"].modifiedAt: value_mismatch (expected '<shaped-not-asserted>', got 1793929777.0456338)

Expected per model: an ISO-8601 date-time string, e.g. of the form 2023-07-20T19:58:44.624Z (the model's own documented example). Actual: an unquoted JSON number. The transformer name and the specific epoch instants above are per-run values and are not part of the requirement; the JSON type and format are.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay simparity-wire-aws-v1.
3. Replay fixture b2bi-transformer-lifecycle through step task#5: CreateTransformer, then UpdateTransformer, then ListTransformers.
4. Inspect the ListTransformers 200 body: $.transformers[].createdAt and $.transformers[].modifiedAt come back as bare JSON numbers instead of quoted ISO-8601 date-time strings as required by @timestampFormat("date-time") on com.amazonaws.b2bi#CreatedDate / #ModifiedDate.
