Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-portfolio-principal-association
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-portfolio-principal-association servicecatalog-product-lifecycle
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|AssociatePrincipalWithPortfolio|4xx->4xx|$.Message
Issue-Key: servicecatalog|CreateProduct|4xx->2xx|$.Message
Issue-Key: servicecatalog|CreateProduct|4xx->2xx|$.__type
Issue-Key: servicecatalog|CreateProduct|4xx->2xx|status
Issue-Key: servicecatalog|DescribeProductAsAdmin|2xx->2xx|$.ProvisioningArtifactSummaries[*].ProvisioningArtifactMetadata.SourceProvisioningArtifactId
Issue-Key: servicecatalog|DescribeProductAsAdmin|4xx->4xx|$.Message
Issue-Commit: c99b0435d08cb2f75df4e28f193ee3023a06ec64

# Service Catalog portfolio and product operations diverge from AWS error messages, validation and metadata

> _Note: this issue was identified against the code base at commit `c99b0435d08cb2f75df4e28f193ee3023a06ec64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociatePrincipalWithPortfolio

## Description
Four net-new wire divergences in the emulated AWS Service Catalog (sdkId servicecatalog) service, all on the same underlying fault: the portfolio/product handlers invent their own error text and skip AWS's input validation and response metadata, so the HTTP body a client sees is not the one AWS produces. All four are in this one service and must all be closed. There is no open servicecatalog issue covering any of them (list_existing_issues for 'servicecatalog' returns nothing).

Cause 1 of 4: AssociatePrincipalWithPortfolio accepts a malformed PrincipalARN but reports it with non-AWS message text.
AWS: Smithy: AssociatePrincipalWithPortfolio.PrincipalARN is required, target PrincipalARN @length 1..1000; modeled error InvalidParametersException (errorKind client, httpError 400, member Message targeting ErrorMessage @length 1..1024), documented as "One or more parameters provided to the operation are not valid." The recorded response is HTTP 400 with __type InvalidParametersException and Message "Input principal arn, not-an-arn, is incorrectly formatted: ARNs must start with 'arn:'". The fixed text of that message is "Input principal arn, " ... ", is incorrectly formatted: ARNs must start with 'arn:'"; the only variable part is the PrincipalARN value echoed from the request (the recording happened to send not-an-arn — that literal is a filled-in value, not part of the template).
Emulator: returns the right status (400) and error code, but the body carries Message 'Invalid principal ARN not-an-arn: not an ARN' — emulator-invented wording in a different sentence shape.
Why: clients and test suites that assert on, log, or surface the Service Catalog validation message get text AWS never emits; anything matching AWS's phrasing (for example the "is incorrectly formatted" / "ARNs must start with 'arn:'" fragments) fails against the emulator even though the request was rejected correctly.
Fix: the PrincipalARN validation/error-construction path of the AssociatePrincipalWithPortfolio handler — build the InvalidParametersException message from AWS's template with the request's PrincipalARN interpolated, rather than a locally worded string.
Traces: servicecatalog-portfolio-principal-association#task#5 (cluster also covers servicecatalog-portfolio-principal-association#task#6, the same message defect on a second malformed-principal request).

Cause 2 of 4: CreateProduct does not validate the provisioning artifact template and returns success where AWS rejects the request.
AWS: Smithy: CreateProduct takes ProvisioningArtifactParameters (ProvisioningArtifactProperties, whose Info map documents the accepted keys LoadTemplateFromURL / ImportFromPhysicalId and whose DisableTemplateValidation defaults to false, i.e. Service Catalog DOES validate the template unless told not to), and models InvalidParametersException (client, httpError 400). The recorded response for an invalid template body is HTTP 400 with __type InvalidParametersException and Message "Invalid templateBody. Please make sure that your template is valid" — that message is entirely fixed text with no variable parts.
Emulator: returns HTTP 200 with a populated CreateProductOutput; no __type and no Message at all, so the product is created instead of refused.
Why: the failure mode is inverted. A client that supplies a broken CloudFormation/Terraform template gets a product id back, the error path in its code is never exercised, and subsequent steps operate on a product AWS would never have created — a silently divergent state rather than a reported error.
Fix: CreateProduct's validation step (handler src/ops/create_product.rs) must validate ProvisioningArtifactParameters.Info before mutating state and raise InvalidParametersException(400) with AWS's fixed message when the template body is not valid, honoring DisableTemplateValidation.
Traces: servicecatalog-product-lifecycle#task#4.

Cause 3 of 4: DescribeProductAsAdmin omits the SourceProvisioningArtifactId entry from ProvisioningArtifactMetadata.
AWS: Smithy: DescribeProductAsAdminOutput.ProvisioningArtifactSummaries is a list of ProvisioningArtifactSummary, whose ProvisioningArtifactMetadata member is a ProvisioningArtifactInfo map (@length 1..100) of metadata for the artifact. The recording shows that map containing the key SourceProvisioningArtifactId; its value is a generated artifact id, which the corpus marks shaped-not-asserted, so the requirement is the presence of that key carrying the id of the artifact the version was sourced from, taken from the emulator's own stored state.
Emulator: the key is absent from ProvisioningArtifactMetadata of the returned summary (missing, got None).
Why: a client walking a product's versions to find which source artifact a version came from reads nothing; code that indexes the metadata map by SourceProvisioningArtifactId sees a missing key rather than an id, so version-lineage logic silently degrades.
Fix: the DescribeProductAsAdmin response shaping (handler src/ops/describe_product_as_admin.rs) plus the provisioning-artifact record in the service state it reads from — the artifact's source id must be stored at create time and projected into the ProvisioningArtifactMetadata map.
Traces: servicecatalog-product-lifecycle#task#6.

Cause 4 of 4: DescribeProductAsAdmin not-found message is emulator-worded.
AWS: Smithy: DescribeProductAsAdmin models ResourceNotFoundException (errorKind client, httpError 400, Message targeting ErrorMessage @length 1..1024), documented "The specified resource was not found." The recorded response for an unknown product id is HTTP 400 with Message "There are no local or default products with id prod-s2h7dnp4inzg". Fixed text: "There are no local or default products with id "; variable part: the Id supplied in the request (the prod-... value is that run's request value, not part of the template).
Emulator: same status (400) but Message 'Product prod-s2h7dnp4inzg not found.' — different wording and sentence shape, with a trailing period AWS does not emit.
Why: clients that distinguish "no such product" by message content, and recorded-expectation tests, do not match; the emulator also drops AWS's distinction that the lookup covered local AND default products.
Fix: the not-found error construction in the DescribeProductAsAdmin handler (src/ops/describe_product_as_admin.rs) — emit AWS's message template with the requested Id interpolated.
Traces: servicecatalog-product-lifecycle#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 108 steps, 77 passed, 25 mismatched, score 71.3%). All four clusters are verdict=mismatch with empty stub_path and empty owned_by, i.e. net-new.

Cause 1 — servicecatalog-portfolio-principal-association#task#5, AssociatePrincipalWithPortfolio, expected 400 / actual 400:
  $.Message: value_mismatch (expected "Input principal arn, not-an-arn, is incorrectly formatted: ARNs must start with 'arn:'", got 'Invalid principal ARN not-an-arn: not an ARN')
  cluster also affects servicecatalog-portfolio-principal-association#task#6 (affected_count 2)

Cause 2 — servicecatalog-product-lifecycle#task#4, CreateProduct, expected 400 / actual 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParametersException', got None)
  $.Message: missing (expected 'Invalid templateBody. Please make sure that your template is valid', got None)

Cause 3 — servicecatalog-product-lifecycle#task#6, DescribeProductAsAdmin, expected 200 / actual 200:
  $.ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata.SourceProvisioningArtifactId: missing (expected '<shaped-not-asserted>', got None)

Cause 4 — servicecatalog-product-lifecycle#task#11, DescribeProductAsAdmin, expected 400 / actual 400:
  $.Message: value_mismatch (expected 'There are no local or default products with id prod-s2h7dnp4inzg', got 'Product prod-s2h7dnp4inzg not found.')

Model evidence (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/smithy-model/com-amazonaws-servicecatalog-aws242servicecatalogservice.json): AssociatePrincipalWithPortfolio errors = InvalidParametersException|LimitExceededException|ResourceNotFoundException (all client, httpError 400); CreateProduct errors = InvalidParametersException|LimitExceededException|TagOptionNotMigratedException; DescribeProductAsAdmin errors = InvalidParametersException|ResourceNotFoundException; ProvisioningArtifactSummary.ProvisioningArtifactMetadata is a ProvisioningArtifactInfo map; ProvisioningArtifactProperties.DisableTemplateValidation defaults to false.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it (servicecatalog is bootstrapped: service-com-amazonaws-servicecatalog-aws242servicecatalogservice is in get_bootstrap_status().bootstrapped).
2. Replay fixture servicecatalog-portfolio-principal-association: at step task#5 call AssociatePrincipalWithPortfolio with a valid PortfolioId and a PrincipalARN that is not an ARN (e.g. 'not-an-arn') and PrincipalType IAM. Observe HTTP 400 InvalidParametersException whose Message is 'Invalid principal ARN <value>: not an ARN' instead of AWS's "Input principal arn, <value>, is incorrectly formatted: ARNs must start with 'arn:'". Step task#6 repeats it.
3. Replay fixture servicecatalog-product-lifecycle step task#4: call CreateProduct with ProvisioningArtifactParameters whose Info carries an invalid template body. Observe HTTP 200 with a CreateProductOutput instead of HTTP 400 InvalidParametersException 'Invalid templateBody. Please make sure that your template is valid'.
4. Same fixture step task#6: call DescribeProductAsAdmin for that product and inspect ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata — the SourceProvisioningArtifactId key is absent.
5. Same fixture step task#11: call DescribeProductAsAdmin with a product id that does not exist. Observe HTTP 400 with Message 'Product <id> not found.' instead of 'There are no local or default products with id <id>'.
6. Full sweep: replay_wire_corpus() reproduces all four clusters; re-replay servicecatalog-portfolio-principal-association#task#5 and servicecatalog-product-lifecycle#task#4, #task#6 and #task#11 to confirm a fix.
