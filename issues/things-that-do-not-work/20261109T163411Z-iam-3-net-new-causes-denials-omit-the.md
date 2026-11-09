Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.get-account-authorization-details iam.session-policy-intersects-the-role-policy iam.policy-documents-are-url-encoded-on-the-query-wire iam.percent-escape-in-policy-value-kept-and-in-policy-name-rejected iam.inline-policy-put-preserves-document-bytes-and-rejects-malformed-shapes
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.get-account-authorization-details|run#16|AssertionError|assert message == _no_policy_denial(unpriv_arn, "GetAccountAuthorizationDetails", message), (
Issue-Key: iam.session-policy-intersects-the-role-policy|run#23|AssertionError|assert narrow_msg == expected_msg, f"expected message {expected_msg!r}, got {narrow_msg!r}"
Issue-Key: iam.policy-documents-are-url-encoded-on-the-query-wire|_assert_encoded_once#6|AssertionError|assert "{" not in text and '"' not in text, (
Issue-Key: iam.percent-escape-in-policy-value-kept-and-in-policy-name-rejected|run#8|AssertionError|assert "%252F" in raw and "%2525" in raw, (
Issue-Key: iam.inline-policy-put-preserves-document-bytes-and-rejects-malformed-shapes|_error#12|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# iam: 3 net-new causes — denials omit the GetRequestAuthorizationDetails pointer, policy documents are served as raw JSON on the Query wire, and a malformed inline policy shape is accepted

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRolePolicy

## Description
Three distinct root causes in the IAM refusal and policy-document surface. (The similar-looking open issues 20261102T091623Z-iam-10-net-new-refusal-message-conflict-precedence.md and 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md do not carry these checks.)

Cause 1 of 3: IAM AccessDenied messages stop after the reason clause and omit AWS's trailing authorization-details pointer.
AWS: an IAM AccessDenied message ends with a pointer to a server-minted, per-request authorization id, given twice: `. Go to https://<partition console region>.console.aws.amazon.com/iam/home?region=<partition console region>#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>`. The fixed text is that whole clause; the only variable is the authorization id, which is minted per request (and the console region, which for IAM is the global partition region). The preceding reason clause is AWS's standard `User: <principal arn> is not authorized to perform: <action> on resource: <resource> because no identity-based policy allows the <action> action` / `... because no session policy allows the <action> action`. Smithy: iam declares the denial as the service's access-denied error; the message template is a service fact — recorded behavior only, and the recording pins every character but the authorization id.
Emulator: the message ends at the reason clause, with no `Go to https://…/authorization-details/…` segment and no GetRequestAuthorizationDetails pointer at all.
Why: a client (or a human reading a log) cannot reach the authorization detail for the refusal; tooling that extracts the authorization id to call GetRequestAuthorizationDetails finds none, so the documented diagnosis path is unavailable, and message-equality checks against AWS fail.
Fix: the denial-message builder in the IAM authorization gate must append the pointer clause and mint the per-request authorization id the pointer names.
Traces: iam.get-account-authorization-details, iam.session-policy-intersects-the-role-policy.

Cause 2 of 3: policy documents are serialized as raw JSON on the Query wire instead of URL-encoded.
AWS: on IAM's Query/XML wire every policy document member (CreateRole's AssumeRolePolicyDocument echo, GetRolePolicy/GetUserPolicy's PolicyDocument, GetPolicyVersion's Document) is URL-encoded exactly once, so the XML element contains no `{` or `"` characters; a client decodes it once with urllib.parse.unquote to recover the JSON. A consequence of that single encoding is that a literal `%2F` or `%25` inside a policy value comes back as `%252F` / `%2525`.
Emulator: the element carries raw JSON (`{"Version": "2012-10-17", "Statement": [...]`), and literal percent escapes in condition values are returned verbatim (`a%2Fb`, `c%25d`) instead of re-encoded.
Why: a client that follows the documented contract and unquotes the value corrupts every document that contains a `%` or a `+`; one that does not unquote works against the emulator and breaks against AWS. Either way the bytes a client reads back are not the bytes AWS would serve.
Fix: the IAM Query/XML serializer's policy-document members must URL-encode the stored document once on the way out (and the inverse on the way in).
Traces: iam.policy-documents-are-url-encoded-on-the-query-wire, iam.percent-escape-in-policy-value-kept-and-in-policy-name-rejected.

Cause 3 of 3: PutRolePolicy/PutUserPolicy accept a malformed policy document shape.
AWS: an inline policy whose document has a structurally invalid shape is refused with HTTP 400 `MalformedPolicyDocument` and a message naming the defect (e.g. `Syntax errors in policy.` / `This policy contains invalid Json`), and no policy is stored; valid documents are stored byte-for-byte. Smithy: iam#PutRolePolicy and iam#PutUserPolicy declare MalformedPolicyDocumentException alongside LimitExceeded/NoSuchEntity/ServiceFailure — so the refusal is modelled.
Emulator: the Put call returns HTTP 200 and stores the malformed document.
Why: the client's authoring error is accepted and becomes a policy that can never evaluate as intended; the mistake surfaces later as an unexplained authorization result rather than as a modelled MalformedPolicyDocument at write time.
Fix: policy-document shape validation in the inline-policy write path (PutRolePolicy/PutUserPolicy/PutGroupPolicy validation), raising MalformedPolicyDocument.
Traces: iam.inline-policy-put-preserves-document-bytes-and-rejects-malformed-shapes.

## Evidence
Cause 1 — Expected: '… because no session policy allows the iam:ListAttachedRolePolicies action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>'. Actual: '… because no session policy allows the iam:ListAttachedRolePolicies action' (pointer clause absent). Same shape for GetAccountAuthorizationDetails.
Cause 2 — Expected: the document URL-encoded once (no '{' or '"' on the wire); a literal %2F/%25 re-encoded as %252F/%2525. Actual: AssertionError: CreateRole response: the document must be URL-encoded on the wire, got raw JSON '{"Version": "2012-10-17", …'; and GetRolePolicy returned 'a%2Fb', 'c%25d' verbatim.
Cause 3 — Expected: MalformedPolicyDocument. Actual: AssertionError: expected a rejection, but the call SUCCEEDED and returned {… 'HTTPStatusCode': 200 …}

## Steps to Reproduce
Cause 1: as a principal with no grant, call iam:GetAccountAuthorizationDetails (and, under a narrowing session policy, iam:ListAttachedRolePolicies); read the AccessDenied message and look for the trailing authorization-details pointer.
Cause 2: CreateRole with a trust policy, then read the raw XML of the CreateRole response and of GetRolePolicy for an inline policy whose condition values contain '%2F' and '%25'.
Cause 3: call PutRolePolicy with a structurally malformed PolicyDocument; observe HTTP 200.
