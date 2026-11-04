Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicecatalog.copy-product-result-rejected-until-portfolio-access
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog.copy-product-result-rejected-until-portfolio-access|run#28|AssertionError|assert message == expected_message, (
Issue-Commit: c083bea380b29cc5238d586af0774e0d5864cbe3

# DescribeProduct not-found/access-denied error message uses emulator wording instead of AWS's text

> _Note: this issue was identified against the code base at commit `c083bea380b29cc5238d586af0774e0d5864cbe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProduct

## Description
One net-new root cause in the emulated AWS Service Catalog (sdkId servicecatalog) service, grounded in a behavioral check.

Cause 1 of 1: DescribeProduct's ResourceNotFoundException carries an emulator-invented message instead of AWS's message template.

AWS: Smithy (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/smithy-model/com-amazonaws-servicecatalog-aws242servicecatalogservice.json): DescribeProduct input member Id targets Id (@length 1..100, pattern ^[a-zA-Z0-9_\-]*$); DescribeProduct errors = InvalidParametersException | ResourceNotFoundException, both errorKind client with httpError 400 (defaulted); ResourceNotFoundException.Message targets ErrorMessage (@length 1..1024). AWS deliberately conflates "no such product" with "the caller has no portfolio access to it" on this operation and emits the message template '<Id> does not exist or access was denied', where ' does not exist or access was denied' is the FIXED text and the only variable part is the product Id echoed from the request. (The admin variant is worded differently — DescribeProductAsAdmin emits 'There are no local or default products with id <Id>' — so the two templates must not be shared.) The behavioral check recording confirms this: a principal holding servicecatalog:DescribeProduct IAM permission but no portfolio association for the product is refused with ResourceNotFoundException and that exact message text.

Emulator: returns the correct HTTP status (400) and the correct error code (ResourceNotFoundException) — the access scoping itself is right — but the body's Message is 'Product <Id> not found.': a different sentence shape, with a trailing period AWS does not emit and without AWS's 'or access was denied' clause. Replayed verbatim from the check: got 'Product prod-ewk6epkj9bbv not found.', expected 'prod-ewk6epkj9bbv does not exist or access was denied' (the prod- id there is a per-run value filled into the template, not part of it).

Why: clients, SDK wrappers and recorded-expectation suites that branch on, log, or surface this message get text AWS never emits; anything matching AWS's phrasing (e.g. the 'does not exist or access was denied' fragment) fails against the emulator even though status and error code are right. More importantly the emulator's wording asserts the product is absent, destroying AWS's deliberate ambiguity between a product that does not exist and one the caller simply cannot see through any portfolio — the signal a client uses to decide whether to re-try under different credentials or to request a portfolio grant rather than treat the product as gone.

Fix: the not-found/no-access error construction in the DescribeProduct handler's lookup path (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/describe_product.rs), kept distinct from DescribeProductAsAdmin's path — build AWS's ResourceNotFoundException message template with the requested Id interpolated from the request instead of a locally worded string.

Related open issues (neither covers this behavioral check, so this one is filed and names them): 20261104T001234Z-servicecatalog-describeproduct-not-found-error-message-is-emulator-worded.md records the same defect from the WIRE corpus (fixture servicecatalog-product-lifecycle#task#8) and is keyed on that replay unit, not on this behavioral check; 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from.md covers DescribeProductAsAdmin's distinct wording. A fixer closing this issue should re-replay the behavioral check listed below, and may close the wire issue in the same change if the handler is shared.

Traces: behavioral check servicecatalog.copy-product-result-rejected-until-portfolio-access.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1): 2/9 passed, servicecatalog 2/9 (7 failed). Of those 7, 5 are collapsed under two ALREADY FILED unimplemented-stub clusters (DescribeProvisioningParameters, GetProvisionedProductOutputs) and 1 is OWNED by 20261030T010307Z-service-catalog-listlaunchpaths-ignores-the-portfolio-principal-association.md; this issue is grounded in the one remaining failure.

[mismatch] servicecatalog.copy-product-result-rejected-until-portfolio-access
  check_servicecatalog_copy_product_result_rejected_until_portfolio_access.py:168
  AssertionError: the reader's DescribeProduct on the un-granted copy said 'Product prod-ewk6epkj9bbv not found.', expected 'prod-ewk6epkj9bbv does not exist or access was denied'

The check's preceding assertion (line 186) that the error Code is 'ResourceNotFoundException' PASSED, and the HTTP status is 400 as modeled, so status and error class are correct and the divergence is the message text alone.
  Required fixed text: '<Id> does not exist or access was denied'.
  Variable part: the product Id taken from the request (prod-ewk6epkj9bbv is one run's generated id).
  Emulator template instead: 'Product <Id> not found.'

Model evidence (smithy_query, operation=DescribeProduct): errors = InvalidParametersException, ResourceNotFoundException (both errorKind client, httpError 400, defaulted); input Id @length 1..100 pattern ^[a-zA-Z0-9_\-]*$; ResourceNotFoundException has a single Message member targeting ErrorMessage @length 1..1024.

Bootstrap gate: servicecatalog is bootstrapped (service-com-amazonaws-servicecatalog-aws242servicecatalogservice appears in get_bootstrap_status().bootstrapped; only athena and iot are pending), so this fidelity finding is filed rather than deferred.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout (servicecatalog is bootstrapped).
2. Replay behavioral check servicecatalog.copy-product-result-rejected-until-portfolio-access. It creates a reader IAM role whose inline policy allows only servicecatalog:DescribeProduct / DescribeProductView, creates a product and a portfolio, associates both the product and the reader principal with the portfolio (positive control: the reader can DescribeProduct the source), then CopyProduct's the source and waits for SUCCEEDED.
3. As the reader principal, call DescribeProduct(Id=<the copy's TargetProductId>) — the copy is NOT associated with any portfolio the reader can reach, though the reader's IAM permission still allows the API call. Equivalently, call DescribeProduct with any syntactically valid product Id the caller cannot see.
4. Observe the response: HTTP 400, __type ResourceNotFoundException (both correct) with Message 'Product <Id> not found.' instead of AWS's '<Id> does not exist or access was denied'.
5. Re-replay servicecatalog.copy-product-result-rejected-until-portfolio-access to confirm a fix; the check then proceeds to assert the copy mints its own provisioning artifact once associated with the portfolio.
