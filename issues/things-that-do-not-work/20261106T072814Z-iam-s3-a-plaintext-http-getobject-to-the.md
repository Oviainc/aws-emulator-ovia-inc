Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant
Issue-Key: iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant|_http#19|BadStatusLine|with urllib.request.urlopen(req, timeout=60) as resp:
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# iam__s3: a plaintext-HTTP GetObject to the S3 endpoint gets a malformed HTTP status line instead of 403 AccessDenied, so an aws:SecureTransport Deny can never be observed on the wire

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
ONE root cause, grounded in 1 failing behavioral check under the suite heading `iam__s3`. The check is marked neither ALREADY FILED nor OWNED in the replay report.

Related open issues that do NOT carry this replay key: 20261103T082051Z-s3-the-https-front-door-aborts-the-handshake.md (a TLS illegal_parameter abort for a virtual-hosted-style authority — a different leg of the front door; here the request is path-style and PLAINTEXT) and 20261029T192356Z-iam-s3-s3-getobject-is-authorized-with-neither.md. Verify them together.

Cause 1 of 1: a plain `http://` request to the S3 endpoint is answered with bytes that are not an HTTP response, so the client cannot read a status line at all — and the bucket policy's `aws:SecureTransport: false` Deny, which must refuse exactly that request with 403 AccessDenied, is never exercised.
AWS: Smithy (com.amazonaws.s3#GetObject, GET /{Bucket}/{Key+}) models NoSuchKey and InvalidObjectState only, so an authorization refusal rides S3's REST-XML error document; recorded behavior for the text. Recorded behavior: with a bucket policy carrying `{Effect: Deny, Principal: {AWS: "*"}, Action: s3:*, Resource: [<bucket arn>, <bucket arn>/*], Condition: {Bool: {aws:SecureTransport: "false"}}}` plus an Allow for a named user, a SigV4-presigned GetObject URL whose scheme is downgraded from https to http — the signature covers host, path and query but NOT the scheme, so it is the exact plaintext twin of the admitted https request — is answered with HTTP 403 and an XML error body whose `<Code>` is `AccessDenied` and whose `<Message>` names the explicit deny (the body must contain `explicit deny`) and must NOT contain the object's bytes. The HTTPS twin of the same request returns the object, which is what proves the refusal belongs to the transport condition.
Emulator: the plaintext request gets no readable HTTP response. Python's http.client fails while reading the status line: `http.client.BadStatusLine:  2` — the first bytes on the connection are not an HTTP status line (consistent with TLS record bytes being written to a plaintext request), so there is no status code, no error code and no body at all where AWS answers 403 AccessDenied with an XML error document. This is the emulator's own front door answering on the address it published for the client, so it is attributable to the emulator and not to the runner.
Why: a client cannot distinguish "the request was refused by the SecureTransport Deny" from "the endpoint is broken" — it gets a protocol error, not an answer. Enforce-TLS bucket policies are the most widely deployed S3 guardrail, and the only way to verify one is to make a plaintext request and read the 403; that verification is impossible here. The simulator half of this check already passes (SimulatePrincipalPolicy returns allowed for aws:SecureTransport true and explicitDeny for false, both matched to the Resource Policy), so the advisory decision is right and only the real wire refusal is missing. The check's remaining assertions — the 403, the `AccessDenied` code, the `explicit deny` wording and the no-leak check — are never reached.
Fix: the S3 front door's protocol handling on the endpoint it publishes — a request arriving as plaintext HTTP must be served as HTTP (never answered with TLS bytes), and its authorization must evaluate the bucket policy with `aws:SecureTransport` set to false, refusing with HTTP 403 and a REST-XML body whose Code is `AccessDenied` and whose Message names the explicit deny in the resource-based policy, with no object bytes in the body.
Traces: iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant.

## Evidence
[error] iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant
  File "client.py", line 343, in begin: version, status, reason = self._read_status()
  File "client.py", line 325, in _read_status: raise BadStatusLine(line)
  http.client.BadStatusLine:  2
  Raised from the check's own `_http("GET", plain_url)` helper (urllib.request against the presigned URL with its scheme rewritten https -> http by `_as_http`). Everything before it passed in the emulator: CreateBucket, PutObject, PutBucketPolicy with the SecureTransport Deny + named-user Allow, GetBucketPolicy, and both SimulatePrincipalPolicy decisions (EvalDecision 'allowed' for aws:SecureTransport=true, 'explicitDeny' for false, every MatchedStatement of SourcePolicyType 'Resource Policy'), plus the named user's real TLS GetObject returning the object's bytes.
  Expected on the plaintext twin: (status, <Code>) == (403, 'AccessDenied'), the body containing b'explicit deny', and the body NOT containing the object's bytes.
  Actual: no HTTP status line could be parsed from the response, so no status, code or body exists.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. s3:CreateBucket B (all four Block Public Access flags on); s3:PutObject B/test-data.json.
3. iam:CreateUser U and iam:CreateAccessKey for U.
4. s3:PutBucketPolicy on B with two statements: {Sid DenyInsecureTransport, Effect Deny, Principal {AWS: '*'}, Action 's3:*', Resource [arn:aws:s3:::B, arn:aws:s3:::B/*], Condition {Bool: {aws:SecureTransport: 'false'}}} and {Sid AllowDevProfileRead, Effect Allow, Principal {AWS: <U arn>}, Action 's3:GetObject', Resource arn:aws:s3:::B/*}.
5. Confirm the https path works: GetObject B/test-data.json signed with U's key returns the bytes.
6. generate_presigned_url('get_object', Bucket=B, Key='test-data.json') as U with signature_version='s3v4' and addressing_style='path'; rewrite the scheme from https to http and leave host, path and query untouched; send it with a raw HTTP client.
7. AWS answers HTTP 403 with an XML body whose <Code> is AccessDenied and whose <Message> names the explicit deny, and no object bytes. The emulator answers bytes that are not an HTTP response: the client raises http.client.BadStatusLine while reading the status line.
8. Re-replay iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant.
