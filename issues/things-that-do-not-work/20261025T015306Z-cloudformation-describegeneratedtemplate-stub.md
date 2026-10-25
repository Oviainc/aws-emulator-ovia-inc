Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.generated-template-and-scan-not-found-errors
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.generated-template-and-scan-not-found-errors|_expect_code#4|AssertionError|assert code in allowed, f"expected one of {sorted(allowed)}, got {code!r} ({actual!r})"
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# Implement cloudformation DescribeGeneratedTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeGeneratedTemplate

## Description
`DescribeGeneratedTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_generated_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeGeneratedTemplate` call, so each one unblocks only when `DescribeGeneratedTemplate` works: `cloudformation.generated-template-and-scan-not-found-errors`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_generated_template.rs`. Replaying the corpus, the emulator refused the `DescribeGeneratedTemplate` call: File "check_cloudformation_generated_template_and_scan_not_found_errors.py", line 47, in run
    _expect_code(
    ~~~~~~~~~~~~^
        lambda: cf.describe_generated_template(GeneratedTemplateName=missing_gt),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        "GeneratedTemplateNotFound",
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        None,
        ^^^^^
    )
    ^
  File "check_cloudformation_generated_template_and_scan_not_found_errors.py", line 30, in _expect_code
    assert code in allowed, f"expected one of {sorted(allowed)}, got {code!r} ({actual!r})"
           ^^^^^^^^^^^^^^^
AssertionError: expected one of ['GeneratedTemplateNotFound', 'GeneratedTemplateNotFoundException'], got 'NotImplemented' ('operation DescribeGeneratedTemplate is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cloudformation emulator
Call DescribeGeneratedTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
