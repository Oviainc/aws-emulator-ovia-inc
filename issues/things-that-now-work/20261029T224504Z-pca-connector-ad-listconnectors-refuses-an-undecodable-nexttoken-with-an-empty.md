Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-connector-validation-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-connector-validation-errors
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|ListConnectors|4xx->4xx|$.message
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# ListConnectors refuses an undecodable NextToken with an empty body, omitting the required Message

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConnectors

## Description
One net-new wire cause.

AWS: ListConnectors given a NextToken that satisfies the member's @pattern but is not a token the service ever issued refuses with HTTP 400, code ValidationException (Smithy: ListConnectors models ValidationException @httpError 400, doc "An input validation error occurred. For example, invalid characters in a template name, or if a pagination token is invalid"; ValidationException.Message is @required; ListConnectors.NextToken @httpQuery @length 1..1000 @pattern `^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`) and the body CARRIES a message describing the pagination failure: fixed text `Pagination token exception in operation '` + the operation name + `': ` followed by the decoder's reason for rejecting the token. The recorded reason is of the form `invalid payload wrapping schema <n>`, where the operation name comes from the request and `<n>` is derived from the supplied token's decoded contents, not a constant. The exact reason wording is recorded behavior only; that a Message member is present at all is the model's requirement.
Emulator: answers HTTP 400 with the right error class but an error body that has NO `message` member at all, so the required Message is absent.
Why: the SDK surfaces an empty-string error message for a bad pagination token, so a caller cannot tell an exhausted/garbled cursor from any other 400 — and code that logs or matches on the message, or that distinguishes pagination faults from input faults, has nothing to read. A @required member missing from a modeled error also breaks strict response deserializers.
Fix: the ListConnectors pagination-token decode path in the pca-connector-ad handler (services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/list_connectors.rs) must populate the modeled ValidationException's Message when it rejects a token, with the operation name and the decode reason, instead of raising an error with an empty body.
Traces: fixture pca-connector-ad-connector-validation-errors, step task#3.

Related open issue (does NOT cover this wire step): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md, Cause 2, concerns a NextToken that fails the @pattern (a message-wording/anchor divergence); this step's token satisfies the pattern and fails to DECODE, and the emulator emits no message at all. That issue carries no wire replay key.

## Evidence
pca-connector-ad-connector-validation-errors#task#3 (ListConnectors with a pattern-valid but unissued NextToken):
  status: expected 400, actual 400 (error class matches)
  $.message: missing — expected a message of the form "Pagination token exception in operation 'ListConnectors': invalid payload wrapping schema <n>" (recorded verbatim: "Pagination token exception in operation 'ListConnectors': invalid payload wrapping schema -24949"; fixed text "Pagination token exception in operation '", "': ", with the operation name from the request and the trailing reason/number derived from the supplied token), got None.

## Steps to Reproduce
Boot the emulator and replay simparity-wire-aws-v1, or drive it directly (region us-west-2): call ListConnectors(NextToken=<a string that satisfies ^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$ but was never handed out by the service>). The emulator answers HTTP 400 with an error body that has no `message` member; AWS answers 400 ValidationException whose Message reads "Pagination token exception in operation 'ListConnectors': <decode reason>". (pca-connector-ad-connector-validation-errors#task#3)
