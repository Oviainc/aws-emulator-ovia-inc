Issue-Source: trace-sweep-divergence
Issue-Trace: simpledbv2-list-exports-nonexistent-domain-filter
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: simpledbv2-list-exports-nonexistent-domain-filter simpledbv2-start-export-full-optional-params simpledbv2-start-export-nonexistent-domain
Issue-Shape-Id: com.amazonaws.simpledbv2#SimpleDBv2
Issue-Key: sdb|ListExports|4xx->2xx|$.message
Issue-Key: sdb|ListExports|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: sdb|ListExports|4xx->2xx|status
Issue-Key: sdb|StartDomainExport|4xx->2xx|$.message
Issue-Key: sdb|StartDomainExport|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: sdb|StartDomainExport|4xx->2xx|status
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# SimpleDBv2 export APIs never raise NoSuchDomainException for an unknown domain

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExports

## Description
The emulator's SimpleDBv2 export operations do not check that the requested domain exists: a request naming a domain that was never created is served as a success instead of the modeled client error. Two causes, both the same missing precondition check.

Cause 1 of 2: ListExports with a domainName filter naming a domain that does not exist.
  AWS: ListExports models NoSuchDomainException as a client error at HTTP 400 (Smithy: ListExports errors = InvalidNextTokenException, InvalidParameterValueException, NoSuchDomainException; NoSuchDomainException @error client, @httpError 400, required member `message`, doc "The specified domain does not exist."). domainName is the optional filter member (Smithy: ListExportsRequest.domainName -> DomainName @length min 1, doc "The name of the domain to filter exports"), and the recorded response for an unknown filter value is 400 with header x-amzn-errortype naming NoSuchDomainException (recorded example value: "NoSuchDomainException:http://internal.amazon.com/coral/com.amazonaws.simpledb.v2/") and the documented message as its body `message` (recorded example: "The specified domain does not exist.").
  Emulator: HTTP 200 with a normal ListExports payload — no `message` field and no x-amzn-errortype header at all. Fixture simpledbv2-list-exports-nonexistent-domain-filter, step task#1.
  Why: a client that filters by a domain name gets an empty/partial success instead of the error that tells it the domain is gone, so typo'd or deleted domains look like domains with no exports. SDK error handling for NoSuchDomainException is unreachable, and retry/create-then-export flows that branch on that error take the wrong branch.
  Fix: the ListExports handler's request validation — resolve domainName against domain state before listing and return NoSuchDomainException (400, x-amzn-errortype NoSuchDomainException) when it is absent.

Cause 2 of 2: StartDomainExport naming a domain that does not exist.
  AWS: StartDomainExport.domainName is required (Smithy: StartDomainExportRequest.domainName -> DomainName, required, @length min 1, doc "The name of the domain to export.") and the operation models NoSuchDomainException as a client error at HTTP 400 (Smithy: StartDomainExport errors = ConflictException, InvalidParameterCombinationException, InvalidParameterValueException, NoSuchDomainException, NumberExportsLimitExceeded). The recorded responses are 400 with x-amzn-errortype naming NoSuchDomainException and the documented "The specified domain does not exist." message.
  Emulator: HTTP 200 — an export is started and clientToken/exportArn/requestedAt are returned for a domain that does not exist; no error body or errortype header. Fixtures simpledbv2-start-export-full-optional-params and simpledbv2-start-export-nonexistent-domain, step task#1 each.
  Why: the emulator invents export jobs against nonexistent domains, so a client's export workflow proceeds and later polls GetExport on an ARN that could never exist on AWS; the failure surfaces late (or not at all) instead of as a 400 at submit time. It also means the domain-existence precondition cannot be exercised against the emulator at all.
  Fix: the StartDomainExport handler's precondition/validation step — look the domain up in the service state before allocating an export and return NoSuchDomainException (400) when it is missing, ahead of the optional-parameter handling (the full-optional-params fixture shows the 200 is returned even with s3KeyPrefix/SSE parameters present).

Traces:
  simpledbv2-list-exports-nonexistent-domain-filter#task#1 (Cause 1)
  simpledbv2-start-export-full-optional-params#task#1, simpledbv2-start-export-nonexistent-domain#task#1 (Cause 2)

## Evidence
Cause 1 — ListExports, fixture simpledbv2-list-exports-nonexistent-domain-filter#task#1:
  expected: status 400, $.message = "The specified domain does not exist.", $headers.x-amzn-errortype = "NoSuchDomainException:http://internal.amazon.com/coral/com.amazonaws.simpledb.v2/"
  actual:   status 200, $.message missing, $headers.x-amzn-errortype missing

Cause 2 — StartDomainExport, fixture simpledbv2-start-export-full-optional-params#task#1 (also simpledbv2-start-export-nonexistent-domain#task#1):
  expected: status 400, $.message = "The specified domain does not exist.", $headers.x-amzn-errortype = "NoSuchDomainException:..."
  actual:   status 200, $.message missing, $headers.x-amzn-errortype missing

Requirement being violated: HTTP 400 + error type NoSuchDomainException (Smithy @error client, @httpError 400). The recorded message text is the shape's documented sentence and is cited as an example.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1); 2 of 10 steps pass.
2. Cause 1: POST /v2/ListExports with body {"domainName": "<a domain that was never created>"}. Emulator answers 200 with an exportSummaries payload; AWS answers 400 NoSuchDomainException. Replay simpledbv2-list-exports-nonexistent-domain-filter step task#1.
3. Cause 2: POST /v2/StartDomainExport with body {"domainName": "<a domain that was never created>", "s3Bucket": "<valid-bucket>"} (with or without s3KeyPrefix / s3SseAlgorithm / s3SseKmsKeyId / s3BucketOwner). Emulator answers 200 with clientToken/exportArn/requestedAt; AWS answers 400 NoSuchDomainException. Replay simpledbv2-start-export-full-optional-params step task#1 and simpledbv2-start-export-nonexistent-domain step task#1.
