Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-scep-error-create-connector-bad-ca
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-scep-error-create-connector-bad-ca pca-connector-scep-error-validation-params
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep|CreateConnector|4xx->4xx|$.message
Issue-Key: pca-connector-scep|CreateChallenge|4xx->4xx|$.message
Issue-Key: pca-connector-scep|ListConnectors|4xx->4xx|$.message
Issue-Key: pca-connector-scep|GetConnector|4xx->4xx|$.message
Issue-Related: 20261029T190804Z-pca-connector-scep-missing-resource-handling-returns-the-wrong-status-class.md
Issue-Commit: ef775f68482e6d25f63f0fc1f052163f266c1a25

# Pattern-constraint ValidationException messages use the wrong form and keep the regex anchors

> _Note: this issue was identified against the code base at commit `ef775f68482e6d25f63f0fc1f052163f266c1a25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Shared fault: the emulator renders every input-constraint failure through one hard-coded message builder — the `"N validation error detected: ..."` form, emitting the @pattern regex verbatim with its ^ and $ anchors. AWS uses two different forms depending on where the member is bound, strips the anchors in the form it does use, and on the untyped tagging ARN does not raise ValidationException at all. 5 causes, all pca-connector-scep; statuses are already 400 everywhere.

Smithy: pca-connector-scep (restJson1); ValidationException and BadRequestException are both errorKind=client, httpError=400, body member `message` (lowercase) required. CreateConnectorRequest.CertificateAuthorityArn and CreateChallengeRequest.ConnectorArn are BODY members with @pattern; ListConnectorsRequest.NextToken is httpQuery-bound with @pattern; GetConnectorRequest.ConnectorArn is an httpLabel with @pattern; ListTagsForResourceRequest.ResourceArn targets plain String with NO pattern at all.

Cause 1 of 5: @pattern failure on a BODY member must use the ECMA-262 form, not the 'validation error detected' form.
  AWS: 400 ValidationException whose `message` is `[ECMA 262 regex "<pattern>" does not match input string "<value>"]` — fixed text `[ECMA 262 regex `, ` does not match input string `, the bracket/quote punctuation and the closing `]`; variable parts are the member's @pattern (WITH its ^ and $, as modeled) and the rejected value from the request. Recorded for CreateConnector with a malformed CertificateAuthorityArn in the JSON payload.
  Emulator: 400 ValidationException with `message` = "1 validation error detected: Value '<value>' at 'certificateAuthorityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — the wrong template entirely for a body member.
  Why: the two forms are structurally different (one names the member, one quotes the regex), so any client or test asserting on validation text for a payload member sees a message AWS never produces.
  Fix: the request-validation/serializer layer — select the message form by the member's HTTP binding (body => ECMA-262 form), rather than always using the query/label form.
  Traces: pca-connector-scep-error-create-connector-bad-ca#task#2.

Cause 2 of 5: same wrong form on CreateChallenge's body member ConnectorArn.
  AWS: 400 ValidationException, `message` = `[ECMA 262 regex "<ConnectorArn pattern>" does not match input string "<value>"]`.
  Emulator: the "1 validation error detected: Value '<value>' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" form.
  Why: as cause 1, on the challenge-creation path.
  Fix: same binding-aware selection in the shared validation layer, exercised from CreateChallenge.
  Traces: pca-connector-scep-error-validation-params#task#4.

Cause 3 of 5: for a QUERY-bound member AWS uses the 'validation error detected' form but WITHOUT the regex anchors.
  AWS: 400 ValidationException, `message` = "1 validation error detected: Value '<value>' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern with leading ^ and trailing $ removed>". Fixed text is the whole sentence scaffold; variable parts are the rejected value, the lowerCamel member name and the anchor-stripped pattern.
  Emulator: identical sentence but the pattern is emitted verbatim including ^ ... $.
  Why: a client (or doc/UX layer) that echoes the server-provided pattern to a user, or a test that compares it against the documented constraint, mismatches on every pattern-validated parameter.
  Fix: the validation message builder — strip a leading ^ and trailing $ from the @pattern before interpolation.
  Traces: pca-connector-scep-error-validation-params#task#1.

Cause 4 of 5: same anchor leak for an httpLabel (path) member.
  AWS: 400 ValidationException, "1 validation error detected: Value '<value>' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <anchor-stripped pattern>" for GetConnector's path ARN.
  Emulator: the same sentence with ^...$ retained.
  Why: as cause 3, on every path-bound ARN.
  Fix: same message builder.
  Traces: pca-connector-scep-error-validation-params#task#2.

Cause 5 of 5: a syntactically bad tagging ARN is a BadRequestException, not a ValidationException.
  AWS: 400 with x-amzn-errortype `BadRequestException` and `message` = "Invalid input resource arn: bad syntax" — a fixed string with no value interpolated. ListTagsForResourceRequest.ResourceArn is an unconstrained String, so there is no @pattern to report; the tagging layer rejects malformed ARNs with its own error.
  Emulator: 400 with x-amzn-errortype `ValidationException` and no `message` member matching AWS's text — it synthesizes a pattern-constraint message for a member that has no pattern.
  Why: the error code is wrong, so a client's catch on BadRequestException never fires and a caller cannot distinguish "this string is not an ARN" from a modeled constraint violation.
  Fix: the ListTagsForResource handler's ARN parsing — raise BadRequestException with the fixed message on a syntax failure instead of routing through pattern validation.
  Traces: pca-connector-scep-error-validation-params#task#3.

## Evidence
Replay against trace set simparity-wire-aws-v1, 5 mismatch clusters, HTTP status 400 == 400 in all five:

- pca-connector-scep-error-create-connector-bad-ca#task#2 CreateConnector: $.message expected '[ECMA 262 regex "^arn:aws(-[a-z]+)*:acm-pca:[a-z]+(-[a-z]+)+-[1-9]\\d*:\\d{12}:certificate-authority\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$" does not match input string "not-an-arn"]', got "1 validation error detected: Value 'not-an-arn' at 'certificateAuthorityArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-[a-z]+)*:acm-pca:...$"
- pca-connector-scep-error-validation-params#task#4 CreateChallenge: $.message expected '[ECMA 262 regex "^arn:aws(-[a-z]+)*:pca-connector-scep:...$" does not match input string "not-an-arn"]', got the "1 validation error detected: ... at 'connectorArn' ..." form
- pca-connector-scep-error-validation-params#task#1 ListConnectors: $.message expected "... at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: (?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?", got the same sentence with the pattern wrapped in ^...$
- pca-connector-scep-error-validation-params#task#2 GetConnector: $.message expected "... at 'connectorArn' ... pattern: arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\\d*:\\d{12}:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}", got the same with leading ^ and trailing $
- pca-connector-scep-error-validation-params#task#3 ListTagsForResource: $headers.x-amzn-errortype expected 'BadRequestException', got 'ValidationException'; $.message expected 'Invalid input resource arn: bad syntax', got None

Fixed text per form: '[ECMA 262 regex "' / '" does not match input string "' / '"]'; 'N validation error detected: Value ' / ' at ' / ' failed to satisfy constraint: Member must satisfy regular expression pattern: '; 'Invalid input resource arn: bad syntax'. Variable parts: the rejected value and member name from the request, and the member's modeled pattern (anchors stripped in the second form). 'not-an-arn', 'short' and 'not-valid-base64!!' are this recording's inputs, not requirements.

## Steps to Reproduce
1. Boot the emulator and replay fixtures pca-connector-scep-error-create-connector-bad-ca and pca-connector-scep-error-validation-params from trace set simparity-wire-aws-v1.
2. POST /connectors with a CertificateAuthorityArn that violates its @pattern (task#2) and POST /challenges with a bad ConnectorArn (validation-params task#4): expect 400 ValidationException with the '[ECMA 262 regex "..." does not match input string "..."]' message; observe the 'N validation error detected: ...' message.
3. GET /connectors?NextToken=<non-base64> (task#1) and GET /connectors/{bad-arn} (task#2): expect the 'N validation error detected' message with the pattern stripped of ^ and $; observe the anchors retained.
4. GET /tags/{syntactically-invalid-arn} (task#3): expect 400 x-amzn-errortype BadRequestException with message 'Invalid input resource arn: bad syntax'; observe ValidationException with a synthesized pattern message.
