Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-product-lifecycle
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-product-lifecycle
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|DescribeProduct|4xx->4xx|$.Message
Issue-Commit: 1f99f06a1022112563c3b4688c6a90b00e72bda1

# DescribeProduct not-found error message is emulator-worded instead of AWS's text

> _Note: this issue was identified against the code base at commit `1f99f06a1022112563c3b4688c6a90b00e72bda1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProduct

## Description
One net-new wire divergence in the emulated AWS Service Catalog (sdkId servicecatalog) service: the DescribeProduct handler builds its own not-found sentence instead of the message AWS emits for ResourceNotFoundException, so the HTTP body a client sees on an unknown product id is not the one AWS produces.

Cause 1 of 1: DescribeProduct returns a non-AWS ResourceNotFoundException message for an unknown product id.
AWS: Smithy (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/smithy-model/com-amazonaws-servicecatalog-aws242servicecatalogservice.json): DescribeProduct input member Id targets Id (@length 1..100, pattern ^[a-zA-Z0-9_\-]*$); DescribeProduct errors = InvalidParametersException | ResourceNotFoundException, both errorKind client with httpError 400; ResourceNotFoundException.Message targets ErrorMessage (@length 1..1024) and is documented "The specified resource was not found." The recorded response for an unknown product id is HTTP 400 with Message '<Id> does not exist or access was denied'. The fixed text of that message is ' does not exist or access was denied'; the only variable part is the product Id echoed from the request (the recording's prod-zzzzzzzzzzzzz is a filled-in request value, not part of the template). Note that AWS words this differently from the admin variant: DescribeProductAsAdmin emits 'There are no local or default products with id <Id>', so the two operations are not interchangeable — DescribeProduct deliberately conflates not-found with access-denied.
Emulator: returns the correct status (400) and error class, but the body carries Message 'Product <Id> not found.' — emulator-invented wording in a different sentence shape, with a trailing period AWS does not emit, and without AWS's "or access was denied" clause.
Why: clients and recorded-expectation test suites that distinguish "no such product" by message content, log it, or surface it to a user get text AWS never emits; anything matching AWS's phrasing (for example the 'does not exist or access was denied' fragment) fails against the emulator even though the request was rejected with the right status and error code. The emulator also drops AWS's deliberate ambiguity between a missing product and one the caller cannot see, which is the signal a client uses to decide whether to retry with different credentials rather than treat the product as absent.
Fix: the not-found error construction in the DescribeProduct handler (its lookup/error path, distinct from DescribeProductAsAdmin's) — emit AWS's ResourceNotFoundException message template with the requested Id interpolated from the request, instead of a locally worded string. This is a sibling of Cause 4 of open issue 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from.md (which covers DescribeProductAsAdmin's not-found wording); that issue does NOT cover DescribeProduct, which has its own distinct AWS message, so both must be closed and the two templates must not be shared.
Traces: servicecatalog-product-lifecycle#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health answered 200 and the data listener accepts; 108 steps total, 83 passed, 22 mismatched, 3 blocked, score 76.9%, 5 divergence clusters). This cluster is verdict=mismatch with empty stub_path and empty owned_by, i.e. net-new; the other four clusters in this sweep are already owned by open issues and were skipped.

Cause 1 — fixture servicecatalog-product-lifecycle, step task#8, operation DescribeProduct, expected status 400 / actual status 400, affected_count 1:
  $.Message: value_mismatch (expected 'prod-zzzzzzzzzzzzz does not exist or access was denied', got 'Product prod-zzzzzzzzzzzzz not found.')
  divergence_key: service=servicecatalog|DescribeProduct|status=400->400|keys=$.Message

Status (400) and error class (ResourceNotFoundException) match; the divergence is the message text alone. Fixed text required: '<Id> does not exist or access was denied'. Variable part: the Id from the request. Emulator template instead: 'Product <Id> not found.'

Model evidence: DescribeProduct errors = InvalidParametersException, ResourceNotFoundException (both client, httpError 400); ResourceNotFoundException has a single Message member targeting ErrorMessage @length 1..1024.

Bootstrap gate: servicecatalog is bootstrapped (service-com-amazonaws-servicecatalog-aws242servicecatalogservice appears in get_bootstrap_status().bootstrapped; only athena and iot are pending), so this fidelity finding is filed rather than deferred.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout (servicecatalog is bootstrapped).
2. Replay fixture servicecatalog-product-lifecycle. Its earlier steps create a portfolio and a product; at step task#8 the fixture calls DescribeProduct with a product Id that does not exist in the emulator's state (the recording uses prod-zzzzzzzzzzzzz, any syntactically valid but unknown Id reproduces it).
3. Observe the response: HTTP 400 with __type ResourceNotFoundException and Message 'Product <Id> not found.' instead of AWS's '<Id> does not exist or access was denied'.
4. Full sweep: replay_wire_corpus() reproduces this cluster as service=servicecatalog|DescribeProduct|status=400->400|keys=$.Message. Re-replay servicecatalog-product-lifecycle#task#8 to confirm a fix.
