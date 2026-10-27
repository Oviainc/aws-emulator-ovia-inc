Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.associate-webacl-pricing-plan-feature-denied wafv2.delete-webacl-blocked-while-associated-rejected wafv2.deploy-webacl-cfn-blocks-matching-request wafv2.disassociate-webacl-removes-association-visible-in-getwebaclforresource wafv2.e2e-create-associate-block-disassociate-teardown wafv2.e2e-logging-configuration-capture-flow wafv2.webacl-association-lifecycle-associate-disassociate-reassociate wafv2.webacl-delete-blocked-while-associated-to-resource
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|AssociateWebACL|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Implement wafv2 AssociateWebACL: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
`AssociateWebACL` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/associate_web_acl.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `AssociateWebACL` call, so each one unblocks only when `AssociateWebACL` works: `wafv2.associate-webacl-pricing-plan-feature-denied`, `wafv2.delete-webacl-blocked-while-associated-rejected`, `wafv2.deploy-webacl-cfn-blocks-matching-request`, `wafv2.disassociate-webacl-removes-association-visible-in-getwebaclforresource`, `wafv2.e2e-create-associate-block-disassociate-teardown`, `wafv2.e2e-logging-configuration-capture-flow`, `wafv2.webacl-association-lifecycle-associate-disassociate-reassociate`, `wafv2.webacl-delete-blocked-while-associated-to-resource`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/associate_web_acl.rs`. Replaying the corpus, the emulator refused the `AssociateWebACL` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociateWebACL operation: operation AssociateWebACL is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the wafv2 emulator
Call AssociateWebACL and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
