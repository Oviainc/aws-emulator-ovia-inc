Issue-Source: trace-sweep-divergence
Issue-Trace: acm-pca-put-policy-deny-effect-rejected-and-overwrite-semantics
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-pca-put-policy-deny-effect-rejected-and-overwrite-semantics acm-pca-ca-access-management-guards
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|PutPolicy|4xx->2xx|$
Issue-Key: acm-pca|PutPolicy|4xx->2xx|status
Issue-Key: acm-pca|GetPolicy|2xx->2xx|$.Policy
Issue-Key: acm-pca|PutPolicy|4xx->4xx|$.message
Issue-Key: acm-pca|DeletePolicy|2xx->4xx|$
Issue-Key: acm-pca|DeletePolicy|2xx->4xx|status
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# ACM PCA resource-policy wire contract diverges: a Deny statement is accepted, DeletePolicy is not delete-if-exists, the InvalidPolicy diagnostic is truncated and GetPolicy re-serializes the stored document

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPolicy

## Description
FOUR net-new causes in the acm-pca resource-policy surface (PutPolicy / DeletePolicy / GetPolicy), all reached over the wire. The open behavioral issue 20261025T011542Z-acm-pca-resource-policy-contract-diverges-putpolicy-rejection.md names related symptoms (a generic InvalidPolicy sentence, DeletePolicy non-idempotence, and missing authorization) but none of these four replay steps is marked OWNED by it, and the wire evidence here differs from what that issue describes: the Deny document is now ACCEPTED with HTTP 200 rather than refused with a generic message, and the ARN-mismatch diagnostic is rule-specific but truncated. A fixer must close all four causes below and re-replay the named steps.

Cause 1 of 4: PutPolicy accepts a policy whose statement has Effect Deny instead of refusing it.
AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json) models PutPolicy with ResourceArn (required) and Policy (AWSPolicy, required, @length 1..81920) and InvalidPolicyException among its errors ("The resource policy is invalid or is missing a required statement"). The rule and the text are recorded behavior only: a syntactically valid document whose statement carries Effect Deny is refused with HTTP 400 InvalidPolicyException and the message "InvalidPolicy: Only the Allow effect is supported" — that whole sentence is fixed text, with nothing taken from the request (the offending document is NOT echoed for this class), and no policy is attached to the CA.
Emulator: answers HTTP 200 with an empty body and attaches the Deny policy.
Why: an over-permissive answer on a security-relevant surface. A caller testing that a cross-account Deny cannot be installed on a private CA sees it pass on the emulator and fail in production; worse, any later read of the CA's policy serves a Deny statement that real ACM PCA could never hold, so downstream access-evaluation logic is exercised against an impossible configuration.
Fix: PutPolicy's policy-document validation in the acm-pca service crate — classify the statement's Effect before mutating state and refuse a non-Allow effect with InvalidPolicyException carrying AWS's fixed sentence, leaving the CA's policy untouched.
Traces: acm-pca-put-policy-deny-effect-rejected-and-overwrite-semantics#task#2.

Cause 2 of 4: GetPolicy serves a re-serialized policy document instead of the document that was stored, turning a scalar Action into a one-element array.
AWS: Smithy models GetPolicy.Policy as AWSPolicy (@length 1..81920). Recorded behavior only: GetPolicy returns the policy document as it was supplied to PutPolicy — an IAM policy statement whose Action was submitted as a JSON STRING is read back as a JSON string, not normalized into an array. The document's Sid, Principal, Resource ARN and action name are values from the request; the rule is that the submitted document's JSON shape is preserved verbatim.
Emulator: returns the document with "Action" rewritten as a single-element array ("Action":["acm-pca:ListPermissions"] where the request contained "Action":"acm-pca:ListPermissions"), i.e. the document is parsed and re-emitted through the emulator's own policy model instead of being stored and returned opaquely.
Why: clients diff policy documents as text (or hash them) to detect drift and to implement read-modify-write on a CA's resource policy; a document that comes back structurally different from the one that was written reports permanent spurious drift and makes an overwrite round-trip non-idempotent. Any client that round-trips GetPolicy -> edit -> PutPolicy also silently rewrites the caller's document shape.
Fix: PutPolicy/GetPolicy state and serializer in the acm-pca service crate — persist the submitted policy document byte-for-byte and return it unchanged, doing validation on a parsed copy rather than on the stored representation.
Traces: acm-pca-put-policy-deny-effect-rejected-and-overwrite-semantics#task#6.

Cause 3 of 4: PutPolicy's ARN/resource-mismatch diagnostic is truncated — it names the expected ARN but drops the offending value's type and literal.
AWS: recorded behavior only (the model carries no diagnostic text). When the document's Resource does not match the CA being targeted (or is not a well-formed ARN), AWS refuses with HTTP 400 InvalidPolicyException and a message built FROM THE REQUEST: the fixed prefix "InvalidPolicy: The supplied policy contains different resource or incorrect ARN format: ", then the CA's ARN, then the fixed connector " and type: ", then the JSON type of the offending Resource value (e.g. STRING), then a newline, then the fixed label "string: " followed by the offending value quoted, then a trailing newline. Fixed text: the prefix, " and type: ", "string: " and the newlines; variable: the CA ARN from the request, the type token and the rejected Resource literal.
Emulator: emits only the prefix plus the CA ARN and stops — the " and type: <TYPE>\nstring: \"<value>\"\n" tail is absent, so the diagnostic never says WHICH Resource value was rejected or what type it had.
Why: the whole value of this message is that it identifies the offending element of the submitted document; without it a caller fixing a policy with several statements cannot tell which Resource was wrong, and any test asserting the AWS diagnostic fails on a message that is a prefix of it.
Fix: the InvalidPolicyException message construction in PutPolicy's policy-document validation — carry the rejected Resource value and its JSON type into the message tail.
Traces: acm-pca-ca-access-management-guards#task#6.

Cause 4 of 4: DeletePolicy on an existing CA that has no attached policy is refused as a missing CA instead of succeeding.
AWS: Smithy models DeletePolicy with ResourceArn (required) and the errors ConcurrentModificationException, InvalidArnException, InvalidStateException, LockoutPreventedException, RequestFailedException and ResourceNotFoundException; when each applies is recorded behavior only. The recording shows DeletePolicy against an EXISTING certificate authority that currently holds no resource policy returning HTTP 200 with an empty body — delete-if-exists semantics — with ResourceNotFoundException reserved for the AUTHORITY itself being absent.
Emulator: answers HTTP 400 {"__type":"ResourceNotFoundException","message":"Could not find specified CA"} — it looks the POLICY up and reports the not-found code that means "this CA does not exist", with a message that asserts the CA is missing when it is not.
Why: idempotent delete is the retry and teardown pattern; a client retrying after a timeout, or any teardown that deletes a policy it is not sure exists, gets a hard 400. Because the code and the message both claim the CA is gone, the caller can reasonably conclude the authority was deleted and skip the rest of its cleanup.
Fix: DeletePolicy's handler/state in the acm-pca service crate — resolve the CA from ResourceArn (ResourceNotFoundException only when the authority is absent), then remove any attached policy and return success unconditionally.
Traces: acm-pca-ca-access-management-guards#task#8.

## Evidence
Cause 1 — fixture acm-pca-put-policy-deny-effect-rejected-and-overwrite-semantics, step task#2, PutPolicy:
  expected: HTTP 400, body {"__type": "InvalidPolicyException", "message": "InvalidPolicy: Only the Allow effect is supported"} (whole sentence is fixed text)
  actual:   HTTP 200, empty body ({}) — the Deny policy was attached.

Cause 2 — same fixture, step task#6, GetPolicy, HTTP 200 vs 200, $.Policy diverges:
  expected: {"Version":"2012-10-17","Statement":[{"Sid":"<sid>","Effect":"Allow","Principal":{"AWS":"<principal-arn>"},"Action":"acm-pca:ListPermissions","Resource":"<ca-arn>"}]} — Action a JSON string, exactly as submitted.
  actual:   the same document with "Action":["acm-pca:ListPermissions"] — scalar normalized to a one-element array. (Sid, principal ARN and CA ARN are request/state values, not part of the requirement; the preserved JSON shape is.)

Cause 3 — fixture acm-pca-ca-access-management-guards, step task#6, PutPolicy, HTTP 400 vs 400, $.message diverges:
  expected: "InvalidPolicy: The supplied policy contains different resource or incorrect ARN format: <ca-arn> and type: STRING\nstring: \"<offending Resource value>\"\n"
  actual:   "InvalidPolicy: The supplied policy contains different resource or incorrect ARN format: <ca-arn>"
  diverging part: the fixed tail " and type: " + <JSON type token> + "\nstring: \"" + <rejected value> + "\"\n" is missing. (In the recording the type token is STRING and the rejected Resource value is "*"; those are request-derived values filling the template.)

Cause 4 — same fixture, step task#8, DeletePolicy:
  expected: HTTP 200, empty body {}
  actual:   HTTP 400, body {"__type": "ResourceNotFoundException", "message": "Could not find specified CA"} — wrong status, wrong error class for the situation, and a message asserting the CA is absent while it exists.

## Steps to Reproduce
Setup for all four: CreateCertificateAuthority (ROOT, RSA_2048 / SHA256WITHRSA, a Subject) and keep the returned CertificateAuthorityArn.

Cause 1: PutPolicy(ResourceArn=<ca-arn>, Policy=<a valid IAM document whose single statement has Effect 'Deny' for a principal on an acm-pca action, Resource=<ca-arn>>). It MUST be refused with HTTP 400 InvalidPolicyException and the message "InvalidPolicy: Only the Allow effect is supported", and GetPolicy afterwards must still report no policy.

Cause 2: PutPolicy(ResourceArn=<ca-arn>, Policy=<a valid Allow document whose statement sets "Action" to the JSON STRING "acm-pca:ListPermissions" and Resource=<ca-arn>>), then GetPolicy(ResourceArn=<ca-arn>). The returned Policy must be the submitted document with Action still a JSON string, not an array. (The fixture reaches this after overwriting an earlier policy, so also check an overwrite path: PutPolicy twice and confirm the second document is returned verbatim.)

Cause 3: PutPolicy(ResourceArn=<ca-arn>, Policy=<a valid Allow document whose statement Resource is NOT the CA's ARN, e.g. the string "*">). Expect HTTP 400 InvalidPolicyException whose message, after the ARN, continues " and type: STRING\nstring: \"*\"\n" — i.e. names the rejected value and its type.

Cause 4: on a CA that exists and has NO attached policy (either never attached, or attach-then-DeletePolicy once), call DeletePolicy(ResourceArn=<ca-arn>). It must return HTTP 200 with an empty body. For contrast, DeletePolicy with a well-formed ARN naming a CA that does not exist must be the one returning ResourceNotFoundException.

Replay: acm-pca-put-policy-deny-effect-rejected-and-overwrite-semantics steps task#2 and task#6; acm-pca-ca-access-management-guards steps task#6 and task#8.
