Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-product-view-and-copy-product
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-product-view-and-copy-product servicecatalog-provisioned-product-plan-and-record-not-found
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|DescribeProductAsAdmin|2xx->2xx|$.ProvisioningArtifactSummaries[*].ProvisioningArtifactMetadata.SourceProductId
Issue-Key: servicecatalog|DescribeCopyProductStatus|4xx->4xx|$.Message
Issue-Key: servicecatalog|DescribeProvisionedProduct|4xx->4xx|$.Message
Issue-Related: 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from.md
Issue-Commit: 2bfbb03c7bd3982d98fd2c53f75176784e229027

# ServiceCatalog copy-product and provisioned-product lookups return emulator-invented messages and drop copy metadata

> _Note: this issue was identified against the code base at commit `2bfbb03c7bd3982d98fd2c53f75176784e229027`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProductAsAdmin

## Description
Three net-new wire divergences in the emulated AWS Service Catalog (sdkId servicecatalog, model com.amazonaws.servicecatalog#AWS242ServiceCatalogService), all on the same underlying fault: the copy-product and provisioned-product lookup handlers do not project the state AWS records for a copied product and substitute their own generic 'does not exist or access was denied' sentence for AWS's per-operation error text. The HTTP status and error code are right in every case; the BODY is not the one AWS produces. All three are in this one service and must all be closed. Two open servicecatalog issues exist and cover NEITHER of these clusters: 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from (AssociatePrincipalWithPortfolio message, CreateProduct template validation, DescribeProductAsAdmin metadata/not-found under the product-lifecycle fixture) and 20261030T211928Z-servicecatalog-listprincipalsforportfolio-returns-the-request-arn-instead-of-the. Cause 1 below is a DIFFERENT cluster from that issue's cause 3 (different fixture and a second missing key, SourceProductId); the replay reported owned_by empty for all three clusters here.

Cause 1 of 3: DescribeProductAsAdmin omits the copy-lineage entries SourceProductId and SourceProvisioningArtifactId from ProvisioningArtifactMetadata for a product created by CopyProduct.
AWS: Smithy: DescribeProductAsAdminOutput.ProvisioningArtifactSummaries is a list of ProvisioningArtifactSummary, whose ProvisioningArtifactMetadata member targets ProvisioningArtifactInfo, a string->string map with @length 1..100 holding 'the metadata for the provisioning artifact'. The recording of a product produced by CopyProduct shows that map carrying BOTH SourceProductId, whose value is the id of the product that was copied from (the request's SourceProductId / the id stored by the emulator for the copy), and SourceProvisioningArtifactId, whose value the corpus marks shaped-not-asserted, i.e. the requirement is the presence of the key carrying the id of the source artifact taken from the emulator's own state. Nothing in the model names these keys, so beyond the map's shape this is recorded behavior only: a copied product's artifact metadata reports its source product and source artifact.
Emulator: both keys are absent from ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata (missing, got None) while the call otherwise succeeds with HTTP 200.
Why: a client that copies a product and then reads the copy's versions cannot tell what it was copied from. Code that indexes the metadata map by SourceProductId/SourceProvisioningArtifactId to reconcile a copy against its origin reads a missing key instead of an id, so copy-lineage and idempotency logic silently degrades with no error to notice.
Fix: the DescribeProductAsAdmin response shaping for the provisioning-artifact summaries, plus the provisioning-artifact/product record in the service state it reads from — CopyProduct must persist the source product and source artifact ids on the copied artifact, and the describe path must project them into the ProvisioningArtifactMetadata map.
Traces: servicecatalog-product-view-and-copy-product#task#6.

Cause 2 of 3: DescribeCopyProductStatus reports an unknown CopyProductToken with non-AWS message text.
AWS: Smithy: DescribeCopyProductStatus.CopyProductToken is required (target Id, @length 1..100, pattern ^[a-zA-Z0-9_\-]*$) and the only modeled error is ResourceNotFoundException (errorKind client, httpError 400, member Message targeting ErrorMessage @length 1..1024), documented 'The specified resource was not found.' The recorded response for a token that names no copy job is HTTP 400 whose Message is 'Invalid CopyProductToken copy-zzzzzzzzzzzzz'. Fixed text: 'Invalid CopyProductToken ' followed by the single variable part, the CopyProductToken echoed from the request (the copy-zzzzzzzzzzzzz literal is that run's request value filling the template, not part of it).
Emulator: same status (400) and error code, but Message 'copy-zzzzzzzzzzzzz does not exist or access was denied' — a generic emulator sentence of a different shape that leads with the value instead of naming the CopyProductToken parameter.
Why: clients and recorded-expectation tests that distinguish a bad copy token by message content do not match, and the emulator's wording asserts an authorization dimension ('or access was denied') that AWS does not report for this error, so callers may retry or escalate as a permissions problem rather than treating the token as invalid.
Fix: the not-found error construction in the DescribeCopyProductStatus handler — emit AWS's 'Invalid CopyProductToken <token>' template with the request's CopyProductToken interpolated instead of the shared generic string.
Traces: servicecatalog-product-view-and-copy-product#task#8.

Cause 3 of 3: DescribeProvisionedProduct reports an unknown provisioned product with non-AWS message text.
AWS: Smithy: DescribeProvisionedProduct takes Id (target Id, @length 1..100, pattern ^[a-zA-Z0-9_\-]*$) or Name, and models InvalidParametersException and ResourceNotFoundException (both client, httpError 400; ResourceNotFoundException.Message targets ErrorMessage @length 1..1024, documented 'The specified resource was not found.'). The recorded response for an id that names no provisioned product is HTTP 400 whose Message is 'Provisioned product not found: pp-zzzzzzzzzzzzz'. Fixed text: 'Provisioned product not found: '; the only variable part is the Id supplied in the request (pp-zzzzzzzzzzzzz is that run's value).
Emulator: same status (400) and error code, but Message 'pp-zzzzzzzzzzzzz does not exist or access was denied' — the same generic sentence as cause 2, which is why these travel together.
Why: a client cannot distinguish 'no such provisioned product' from an access failure by message, and anything asserting on AWS's 'Provisioned product not found' phrasing fails even though the lookup was refused correctly.
Fix: the not-found error construction in the DescribeProvisionedProduct handler — build ResourceNotFoundException's message from AWS's per-operation template with the requested Id interpolated. Causes 2 and 3 share one generic message helper; replacing it with per-operation AWS templates closes both.
Traces: servicecatalog-provisioned-product-plan-and-record-not-found#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true, /__admin/health answered 200 and the data listener accepts; 10 fixtures replayed, 108 steps, 81 passed, 24 mismatched, 3 blocked, score_pct 75.0, divergence_count 7). Four of the seven clusters report a non-empty owned_by and are skipped; the three below are verdict=mismatch with empty stub_path AND empty owned_by, i.e. net-new.

Cause 1 — fixture servicecatalog-product-view-and-copy-product, step task#6, DescribeProductAsAdmin, expected 200 / actual 200:
  $.ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata.SourceProductId: missing (expected 'prod-fsnatkpgcyi7', got None)
  $.ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata.SourceProvisioningArtifactId: missing (expected '<shaped-not-asserted>', got None)
  divergence_key servicecatalog|DescribeProductAsAdmin|status=200->200|keys=$.ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata.SourceProductId,$.ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata.SourceProvisioningArtifactId (affected_count 1)
  (the expected prod-... value is the copied-from product id of that run; the requirement is the keys carrying the source ids, not those literals)

Cause 2 — fixture servicecatalog-product-view-and-copy-product, step task#8, DescribeCopyProductStatus, expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Invalid CopyProductToken copy-zzzzzzzzzzzzz', got 'copy-zzzzzzzzzzzzz does not exist or access was denied')
  divergence_key servicecatalog|DescribeCopyProductStatus|status=400->400|keys=$.Message (affected_count 1)

Cause 3 — fixture servicecatalog-provisioned-product-plan-and-record-not-found, step task#5, DescribeProvisionedProduct, expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Provisioned product not found: pp-zzzzzzzzzzzzz', got 'pp-zzzzzzzzzzzzz does not exist or access was denied')
  divergence_key servicecatalog|DescribeProvisionedProduct|status=400->400|keys=$.Message (affected_count 1)

Model evidence (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/smithy-model/com-amazonaws-servicecatalog-aws242servicecatalogservice.json): DescribeProductAsAdmin errors = InvalidParametersException|ResourceNotFoundException (client, httpError 400); output ProvisioningArtifactSummaries is a list of ProvisioningArtifactSummary whose ProvisioningArtifactMetadata targets the ProvisioningArtifactInfo map (@length 1..100). DescribeCopyProductStatus: CopyProductToken required, sole error ResourceNotFoundException (client, 400). DescribeProvisionedProduct errors = InvalidParametersException|ResourceNotFoundException (client, 400). ResourceNotFoundException.Message targets ErrorMessage @length 1..1024, doc 'The specified resource was not found.'

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from that checkout (servicecatalog is bootstrapped: service-com-amazonaws-servicecatalog-aws242servicecatalogservice appears in get_bootstrap_status().bootstrapped and is absent from pending).
2. Cause 1: replay fixture servicecatalog-product-view-and-copy-product up to step task#6 — create a product, CopyProduct it, then call DescribeProductAsAdmin on the copy and inspect ProvisioningArtifactSummaries[0].ProvisioningArtifactMetadata. The SourceProductId and SourceProvisioningArtifactId keys are absent; AWS returns both, carrying the copied-from product id and the source artifact id.
3. Cause 2: same fixture, step task#8 — call DescribeCopyProductStatus with a CopyProductToken that matches the Id pattern but names no copy job (e.g. copy-zzzzzzzzzzzzz). Observe HTTP 400 with Message '<token> does not exist or access was denied' instead of AWS's 'Invalid CopyProductToken <token>'.
4. Cause 3: replay fixture servicecatalog-provisioned-product-plan-and-record-not-found, step task#5 — call DescribeProvisionedProduct with an Id that names no provisioned product (e.g. pp-zzzzzzzzzzzzz). Observe HTTP 400 with Message '<id> does not exist or access was denied' instead of AWS's 'Provisioned product not found: <id>'.
5. Full sweep: replay_wire_corpus() reproduces all three clusters. To confirm a fix, re-replay servicecatalog-product-view-and-copy-product#task#6, servicecatalog-product-view-and-copy-product#task#8 and servicecatalog-provisioned-product-plan-and-record-not-found#task#5 — all three must pass before this issue is closed.
