Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.get-sagemaker-servicecatalog-portfolio-status-consistent-read sagemaker.get-sagemaker-servicecatalog-portfolio-status-idempotent-read sagemaker.get-sagemaker-servicecatalog-portfolio-status-reflects-enablement sagemaker.get-sagemaker-servicecatalog-portfolio-status-valid-enum sagemaker.servicecatalog-portfolio-status-reflects-account-state sagemaker.servicecatalog-portfolio-status-reflects-enablement sagemaker.servicecatalog-portfolio-status-reflects-managed-state sagemaker.servicecatalog-portfolio-status-reflects-toggle sagemaker.servicecatalog-portfolio-status-toggle-reflected
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|GetSagemakerServicecatalogPortfolioStatus|NotImplemented
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker GetSagemakerServicecatalogPortfolioStatus: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSagemakerServicecatalogPortfolioStatus

## Description
`GetSagemakerServicecatalogPortfolioStatus` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/get_sagemaker_servicecatalog_portfolio_status.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `GetSagemakerServicecatalogPortfolioStatus` call, so each one unblocks only when `GetSagemakerServicecatalogPortfolioStatus` works: `sagemaker.get-sagemaker-servicecatalog-portfolio-status-consistent-read`, `sagemaker.get-sagemaker-servicecatalog-portfolio-status-idempotent-read`, `sagemaker.get-sagemaker-servicecatalog-portfolio-status-reflects-enablement`, `sagemaker.get-sagemaker-servicecatalog-portfolio-status-valid-enum`, `sagemaker.servicecatalog-portfolio-status-reflects-account-state`, `sagemaker.servicecatalog-portfolio-status-reflects-enablement`, `sagemaker.servicecatalog-portfolio-status-reflects-managed-state`, `sagemaker.servicecatalog-portfolio-status-reflects-toggle`, `sagemaker.servicecatalog-portfolio-status-toggle-reflected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/get_sagemaker_servicecatalog_portfolio_status.rs`. Replaying the corpus, the emulator refused the `GetSagemakerServicecatalogPortfolioStatus` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetSagemakerServicecatalogPortfolioStatus operation: operation GetSagemakerServicecatalogPortfolioStatus is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sagemaker emulator
Call GetSagemakerServicecatalogPortfolioStatus and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
