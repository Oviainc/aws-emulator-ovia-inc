Issue-Source: trace-sweep-divergence
Issue-Trace: route53resolver-resolver-endpoint-missing-resource-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53resolver-resolver-endpoint-missing-resource-errors
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateResolverEndpoint|4xx->2xx|$.Message
Issue-Key: route53resolver|CreateResolverEndpoint|4xx->2xx|$.__type
Issue-Key: route53resolver|CreateResolverEndpoint|4xx->2xx|status
Issue-Key: route53resolver|ListResolverEndpoints|2xx->2xx|$.MaxResults
Issue-Commit: eb649b8e6cc19057378cc7d140b29e6e37a84f5c

# Route 53 Resolver endpoint ops skip resource validation and omit paginator echo

> _Note: this issue was identified against the code base at commit `eb649b8e6cc19057378cc7d140b29e6e37a84f5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolverEndpoint

## Description
Two independent wire divergences in the route53resolver resolver-endpoint surface, both found in fixture route53resolver-resolver-endpoint-missing-resource-errors. A fixer must close BOTH causes before submitting.

Cause 1 of 2: CreateResolverEndpoint accepts an IpAddresses entry whose SubnetId does not exist and reports success (fixture route53resolver-resolver-endpoint-missing-resource-errors, step task#2).
AWS: CreateResolverEndpoint takes IpAddresses (Smithy: CreateResolverEndpoint.IpAddresses, required, target IpAddressesRequest, @length min 2 max 20) whose members reference an existing VPC subnet, and the operation models InvalidParameterException (Smithy: CreateResolverEndpoint errors include InvalidParameterException, errorKind client, httpError 400; InvalidParameterException.Message required, plus FieldName documented as "the name of the parameter that's invalid"). The recorded response for a request naming a subnet that does not exist is HTTP 400 with __type InvalidParameterException; the recorded message, as an example, is "The subnet ID 'subnet-0123456789abcdef0' does not exist". The requirement is the status 400 and the error code InvalidParameterException for an unresolvable subnet reference, with the message naming the offending subnet id taken from the request; the exact recorded wording is an example.
Emulator: returns HTTP 200 with a CreateResolverEndpointResponse body. No __type and no Message are emitted at all (replay detail: "status: expected 400, got 200; $.__type: missing (expected 'InvalidParameterException', got None); $.Message: missing"). The emulator therefore creates a resolver endpoint against a subnet that was never created in the fixture's setup.
Why: a client that provisions a resolver endpoint with a typo'd or already-deleted subnet gets a success response and a resolver-endpoint id that cannot exist on AWS. Error-path tests and retry/compensation logic that depend on InvalidParameterException never fire, and subsequent steps operate on a phantom endpoint, so an emulator-backed test suite passes a configuration AWS rejects outright.
Fix: CreateResolverEndpoint's handler/validation must resolve every IpAddresses[].SubnetId against the emulator's VPC/subnet state before creating the endpoint and, when a referenced subnet is absent, fail with InvalidParameterException at HTTP 400 instead of proceeding to the create path.
Traces: route53resolver-resolver-endpoint-missing-resource-errors#task#2.

Cause 2 of 2: ListResolverEndpoints does not echo MaxResults in its response (fixture route53resolver-resolver-endpoint-missing-resource-errors, step task#1).
AWS: ListResolverEndpointsResponse carries a MaxResults member (Smithy: ListResolverEndpointsResponse.MaxResults, target MaxResults, @range min 1 max 100, documented as "The value that you specified for MaxResults in the request"), and the operation is @paginated with pageSize MaxResults. So a request that sets MaxResults must get that same value back in the response; the recording shows MaxResults 100 echoed on a 200.
Emulator: answers HTTP 200 but the body omits MaxResults entirely (replay detail: "$.MaxResults: missing (expected 100, got None)"). The status is correct; the modeled output member is simply absent.
Why: clients and paginators that read the response's MaxResults to size the next page, or that assert the service honoured the requested page size, see null where AWS returns an integer. Strongly-typed SDK consumers get an unset optional and page-size-sensitive code paths behave differently against the emulator than against AWS.
Fix: ListResolverEndpoints' handler/serializer must populate the response's MaxResults from the effective request value (the supplied MaxResults, or the service default when unset) rather than leaving the member unserialized.
Traces: route53resolver-resolver-endpoint-missing-resource-errors#task#1.

## Evidence
Cause 1 (step task#2, CreateResolverEndpoint): expected HTTP 400 with $.__type = InvalidParameterException and a Message naming the missing subnet (recorded example: "The subnet ID 'subnet-0123456789abcdef0' does not exist"); actual HTTP 200 with neither $.__type nor $.Message present. Replay divergence_key service=route53resolver|CreateResolverEndpoint|status=400->200|keys=$.Message,$.__type,status.
Cause 2 (step task#1, ListResolverEndpoints): expected HTTP 200 with $.MaxResults = 100; actual HTTP 200 with $.MaxResults missing (None). Replay divergence_key service=route53resolver|ListResolverEndpoints|status=200->200|keys=$.MaxResults.
Model citations: Smithy ListResolverEndpointsResponse.MaxResults (@range 1-100, "The value that you specified for MaxResults in the request"); Smithy CreateResolverEndpoint errors include InvalidParameterException (client, httpError 400) and IpAddresses is required.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay wire fixture route53resolver-resolver-endpoint-missing-resource-errors from trace set simparity-wire-aws-v1.
3. Step task#1 calls ListResolverEndpoints with a MaxResults of 100: the 200 response body has no MaxResults member (AWS echoes 100).
4. Step task#2 calls CreateResolverEndpoint with an IpAddresses entry whose SubnetId (subnet-0123456789abcdef0 in the recording) was never created: the emulator returns 200 and creates the endpoint, where AWS returns 400 with __type InvalidParameterException.
5. Both steps are reported as mismatches by replay_wire_corpus.
