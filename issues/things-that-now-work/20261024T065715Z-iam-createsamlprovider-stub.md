Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-with-saml-expired-assertion-rejected sts.assume-role-with-saml-expired-token-rejected sts.assume-role-with-saml-forged-assertion-rejected sts.assume-role-with-saml-idp-rejected-claim-denied sts.assume-role-with-saml-invalid-identity-token-rejected sts.assume-role-with-saml-non-base64-assertion-rejected sts.assume-role-with-saml-provider-deleted-blocked sts.assume-role-with-saml-rejects-xpath-based-transforms
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: iam|CreateSAMLProvider|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Implement iam CreateSAMLProvider: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSAMLProvider

## Description
`CreateSAMLProvider` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_saml_provider.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `CreateSAMLProvider` call, so each one unblocks only when `CreateSAMLProvider` works: `sts.assume-role-with-saml-expired-assertion-rejected`, `sts.assume-role-with-saml-expired-token-rejected`, `sts.assume-role-with-saml-forged-assertion-rejected`, `sts.assume-role-with-saml-idp-rejected-claim-denied`, `sts.assume-role-with-saml-invalid-identity-token-rejected`, `sts.assume-role-with-saml-non-base64-assertion-rejected`, `sts.assume-role-with-saml-provider-deleted-blocked`, `sts.assume-role-with-saml-rejects-xpath-based-transforms`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_saml_provider.rs`. Replaying the corpus, the emulator refused the `CreateSAMLProvider` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSAMLProvider operation: operation CreateSAMLProvider is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call CreateSAMLProvider and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
