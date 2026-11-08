Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.get-schema-reflects-configured-table-association cleanrooms.protected-query-cancel-lifecycle cleanrooms.protected-query-list-rule-round-trip cleanrooms.update-configured-table-association-analysis-rule-round-trip
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|CreateConfiguredTableAssociationAnalysisRule|NotImplemented
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# Implement cleanrooms CreateConfiguredTableAssociationAnalysisRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguredTableAssociationAnalysisRule

## Description
`CreateConfiguredTableAssociationAnalysisRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_configured_table_association_analysis_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `CreateConfiguredTableAssociationAnalysisRule` call, so each one unblocks only when `CreateConfiguredTableAssociationAnalysisRule` works: `cleanrooms.get-schema-reflects-configured-table-association`, `cleanrooms.protected-query-cancel-lifecycle`, `cleanrooms.protected-query-list-rule-round-trip`, `cleanrooms.update-configured-table-association-analysis-rule-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_configured_table_association_analysis_rule.rs`. Replaying the corpus, the emulator refused the `CreateConfiguredTableAssociationAnalysisRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfiguredTableAssociationAnalysisRule operation: operation CreateConfiguredTableAssociationAnalysisRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cleanrooms emulator
Call CreateConfiguredTableAssociationAnalysisRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
