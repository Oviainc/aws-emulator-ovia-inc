Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.op-contract-listEntitiesForPolicy-role-and-user iam.saml-provider-invalid-metadata-document-rejected
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.op-contract-listEntitiesForPolicy-role-and-user|run#25|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.saml-provider-invalid-metadata-document-rejected|run#11|AssertionError|assert message == want, f"InvalidInput message: expected {want!r}, got {message!r}"
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# iam: 2 net-new root causes — ListEntitiesForPolicy's NoSuchEntity message drops AWS's 'or is not attachable' clause, and CreateSAMLProvider's InvalidInput message leaks an internal XML-parser diagnostic

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEntitiesForPolicy

## Description
Two further distinct net-new root causes in the bootstrapped iam service, disjoint from the causes carried by 20261025T000319Z-iam-9-net-new-behavioral-root-causes-cloudformation.md and 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md (neither names these two operations or these two check ids). Neither failing check is marked ALREADY FILED or OWNED in the replay report. Both are refusals where the HTTP status and the error code are already correct and ONLY the message body diverges; in both cases the diverging text is AWS's fixed, request-independent wording for that operation, so the rule producing it is known. A fixer must close both before submitting.

Cause 1 of 2: ListEntitiesForPolicy on a well-formed but unregistered PolicyArn uses the generic 'does not exist' message instead of the attachment-family wording.
AWS: ListEntitiesForPolicy models NoSuchEntityException -> awsQueryError code 'NoSuchEntity', HTTP 404, and InvalidInputException -> 'InvalidInput', HTTP 400 (Smithy: ListEntitiesForPolicy.PolicyArn @required target arnType, @length 20-2048). For the policy-attachment family of operations the recorded message is the fixed template 'Policy <PolicyArn> does not exist or is not attachable.' — the only request-derived part is the ARN echoed from the input; the trailing 'or is not attachable' clause is constant and is what distinguishes this family from the plain 'does not exist.' wording IAM uses elsewhere (e.g. GetPolicy).
Emulator: code NoSuchEntity and status 404 are right, but the message is 'Policy arn:aws:iam::<account>:policy/<name> does not exist.' — the attachability clause is dropped, i.e. the handler reuses a generic not-found template rather than this operation's.
Why: a client cannot distinguish 'this ARN names nothing' from 'this ARN names something that cannot participate in attachment', which is exactly the branch runbooks and SDK wrappers key on when deciding whether to create the policy or to stop; matchers on AWS's exact string fail.
Fix: the NoSuchEntity message constant in the ListEntitiesForPolicy handler, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_entities_for_policy.rs (note the companion issue's cause 5 reports the inverse mix-up on PutRole/UserPermissionsBoundary, so the two templates should be sited per operation rather than shared blindly).
Traces: iam.op-contract-listEntitiesForPolicy-role-and-user.

Cause 2 of 2: CreateSAMLProvider's InvalidInput refusal for a non-XML metadata document returns an internal parser diagnostic instead of AWS's fixed text.
AWS: CreateSAMLProvider models InvalidInputException -> awsQueryError code 'InvalidInput', HTTP 400 (Smithy: CreateSAMLProvider.SAMLMetadataDocument @required, @length min 1000 max 10000000; Name @required @length 1-128 @pattern ^[\w._-]+$). A document that satisfies the length constraint but is not SAML metadata XML is refused with the constant message 'Invalid SAML metadata document' — request-independent text, with no member name, no parser detail and no position.
Emulator: code InvalidInput and status 400 are right, but the message is 'Invalid SAMLMetadataDocument: the document is not well-formed XML (unknown token at 1:1)' — the input member name plus an XML-parser error and a byte position from the emulator's own parser.
Why: the message is not AWS-shaped at all — clients matching AWS's string do not match, and the response exposes emulator-internal parser diagnostics (token kind and offset) that AWS never returns, which is also an input-echo surface.
Fix: the invalid-document message constant in the CreateSAMLProvider handler, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_saml_provider.rs — keep the validation (it correctly refuses the garbage document and correctly creates nothing) and replace the propagated parser error with AWS's constant.
Traces: iam.saml-provider-invalid-metadata-document-rejected.

## Evidence
Cause 1 — iam.op-contract-listEntitiesForPolicy-role-and-user:
AssertionError: expected message 'Policy arn:aws:iam::000000000000:policy/does-not-exist-9b5761a9ee63 does not exist or is not attachable.', got 'Policy arn:aws:iam::000000000000:policy/does-not-exist-9b5761a9ee63 does not exist.'
The error code asserted immediately before this (NoSuchEntity) passed, so only the message body diverges. The account id and random suffix in the ARN are environment values; the requirement is the fixed 'does not exist or is not attachable.' clause after the echoed ARN.

Cause 2 — iam.saml-provider-invalid-metadata-document-rejected:
AssertionError: InvalidInput message: expected 'Invalid SAML metadata document', got 'Invalid SAMLMetadataDocument: the document is not well-formed XML (unknown token at 1:1)'
The error code asserted immediately before this (InvalidInput) passed, and the check's subsequent 'no provider was created' assertion was never reached, so the effect-absence claim is untested here; only the message body is shown to diverge.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: iam CreatePolicy, attach it to a role and a user, then call iam ListEntitiesForPolicy with a well-formed but unregistered PolicyArn (same account/path, a name that was never created). Observe NoSuchEntity/404 with message 'Policy <arn> does not exist.' instead of 'Policy <arn> does not exist or is not attachable.'
3. Cause 2: call iam CreateSAMLProvider with Name matching ^[\w._-]+$ and SAMLMetadataDocument set to >=1000 characters of non-XML text. Observe InvalidInput/400 with message 'Invalid SAMLMetadataDocument: the document is not well-formed XML (unknown token at 1:1)' instead of 'Invalid SAML metadata document'.
4. Re-replay iam.op-contract-listEntitiesForPolicy-role-and-user and iam.saml-provider-invalid-metadata-document-rejected to confirm both are closed.
