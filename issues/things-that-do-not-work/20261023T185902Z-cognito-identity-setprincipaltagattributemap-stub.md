Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.concurrent-set-principal-tag-attribute-map-consistent cognito-identity.e2e-principal-tags-federation-to-sts-session-tags cognito-identity.e2e-principal-tags-flow-into-sts-session-tags cognito-identity.get-principal-tag-attribute-map-allowlist-mode-filters-claims cognito-identity.get-principal-tag-attribute-map-nonexistent-pool-rejected cognito-identity.get-principal-tag-attribute-map-unknown-provider-not-found cognito-identity.get-principal-tag-attribute-map-use-defaults-flag-round-trip cognito-identity.not-authorized-principal-tag-map-on-foreign-pool cognito-identity.principal-tag-attribute-map-invalid-key-rejected cognito-identity.principal-tag-attribute-map-lifecycle cognito-identity.principal-tag-attribute-map-overwrite-semantics cognito-identity.principal-tag-attribute-map-roundtrip cognito-identity.principal-tag-attribute-map-set-get-lifecycle cognito-identity.principal-tag-attribute-map-special-char-values-preserved cognito-identity.principal-tag-attribute-map-unicode-value-preserved cognito-identity.principal-tag-attribute-map-unicode-values-round-trip cognito-identity.principal-tag-condition-denies-the-other-tenants-object cognito-identity.server-side-principal-tag-attribute-map-enforced cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy cognito-identity.set-and-get-principal-tag-attribute-map-roundtrip cognito-identity.set-principal-tag-attribute-map-basic cognito-identity.set-principal-tag-attribute-map-conflicting-accepted cognito-identity.set-principal-tag-attribute-map-invalid-key-rejected cognito-identity.set-principal-tag-attribute-map-invalid-keys-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|NotImplemented
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# Implement cognito-identity SetPrincipalTagAttributeMap: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetPrincipalTagAttributeMap

## Description
`SetPrincipalTagAttributeMap` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/set_principal_tag_attribute_map.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

24 recorded unit(s) fail on their `SetPrincipalTagAttributeMap` call, so each one unblocks only when `SetPrincipalTagAttributeMap` works: `cognito-identity.concurrent-set-principal-tag-attribute-map-consistent`, `cognito-identity.e2e-principal-tags-federation-to-sts-session-tags`, `cognito-identity.e2e-principal-tags-flow-into-sts-session-tags`, `cognito-identity.get-principal-tag-attribute-map-allowlist-mode-filters-claims`, `cognito-identity.get-principal-tag-attribute-map-nonexistent-pool-rejected`, `cognito-identity.get-principal-tag-attribute-map-unknown-provider-not-found`, `cognito-identity.get-principal-tag-attribute-map-use-defaults-flag-round-trip`, `cognito-identity.not-authorized-principal-tag-map-on-foreign-pool`, `cognito-identity.principal-tag-attribute-map-invalid-key-rejected`, `cognito-identity.principal-tag-attribute-map-lifecycle`, `cognito-identity.principal-tag-attribute-map-overwrite-semantics`, `cognito-identity.principal-tag-attribute-map-roundtrip`, `cognito-identity.principal-tag-attribute-map-set-get-lifecycle`, `cognito-identity.principal-tag-attribute-map-special-char-values-preserved`, `cognito-identity.principal-tag-attribute-map-unicode-value-preserved`, `cognito-identity.principal-tag-attribute-map-unicode-values-round-trip`, `cognito-identity.principal-tag-condition-denies-the-other-tenants-object`, `cognito-identity.server-side-principal-tag-attribute-map-enforced`, `cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy`, `cognito-identity.set-and-get-principal-tag-attribute-map-roundtrip` (+4 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/set_principal_tag_attribute_map.rs`. Replaying the corpus, the emulator refused the `SetPrincipalTagAttributeMap` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SetPrincipalTagAttributeMap operation: operation SetPrincipalTagAttributeMap is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-identity emulator
Call SetPrincipalTagAttributeMap and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
