Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehubv2-catalog-list-test-templates
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehubv2-catalog-list-test-templates
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|ListTestTemplates|2xx->5xx|$.testTemplates
Issue-Key: resiliencehubv2|ListTestTemplates|2xx->5xx|$headers.content-type
Issue-Key: resiliencehubv2|ListTestTemplates|2xx->5xx|status
Issue-Commit: 92ad2730926ec3062b662e938e8ed9f3fac68577

# ListTestTemplates answers 500 in an awsJson envelope instead of the restJson1 200 template catalog

> _Note: this issue was identified against the code base at commit `92ad2730926ec3062b662e938e8ed9f3fac68577`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTestTemplates

## Description
One cause.

Cause 1 of 1: GET /v2/list-test-templates returns a server error in the wrong protocol envelope instead of the modeled test-template catalog.

AWS: Smithy model services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json — service NGRHServiceCore, sdkId resiliencehubv2, protocol restJson1, auth sigv4. ListTestTemplates is http GET /v2/list-test-templates with success code 200 (defaulted), input ListTestTemplatesRequest (no members), output ListTestTemplatesResponse whose single member `testTemplates` (TestTemplateSummaryList) is @required. Its modeled errors are the service-common AccessDeniedException (403), InternalServerException (500) and ValidationException (400); 500 is therefore reserved for a genuine internal failure, not for a normal catalog read. Because the protocol is restJson1, a successful response is framed `Content-Type: application/json` — the awsJson media type `application/x-amz-json-1.1` is not a legal framing for this service at all. Per the model's doc, a test template is a pre-configured, AWS-recommended test naming the resilience capability to validate, so this is a read-only listing of a service-owned catalog that requires no prior state and is expected to be non-empty for every caller.

Emulator: the replayed step gets HTTP 500 instead of 200, the required `testTemplates` member is absent from the body entirely (expected a list of template summaries, got None), and the response is framed `Content-Type: application/x-amz-json-1.1` rather than `application/json`. The wrong media type shows the answer is not being rendered by this service's restJson1 renderer at all, so the failure is upstream of a normal modeled-error response. Note that crate's handler is still the scaffold: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/list_test_templates.rs:151 returns `Scaffolded::not_implemented("ListTestTemplates")`, whose documented answer is 501 NotImplemented — the replay reported neither a 501 nor a stub-blocked verdict, so the observed 500 in an awsJson envelope is a second, distinct wire fault layered on the missing implementation rather than the scaffold's own refusal. Hypothesis: the request is answered by a non-restJson1 error renderer (an awsJson-framed fallback) before this service's envelope is applied.

Why: ListTestTemplates is the entry point for discovering which resilience tests can be configured, so a client cannot enumerate templates and every downstream CreateTest/StartTestRun flow that needs a testTemplateArn is unreachable. A 500 is also classified as a retryable server fault by the AWS SDKs, so a caller retries the full retry budget and then surfaces an InternalServerException rather than failing fast — and because the body arrives under an awsJson media type, an SDK built for restJson1 cannot even parse the payload into the modeled error shape, producing an opaque deserialization failure instead of a usable error. A required output member arriving absent breaks strict-typed clients outright.

Fix: implement the operation's handler in services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/list_test_templates.rs so it returns ListTestTemplatesOutput with a populated `testTemplates` list (the service-owned static catalog — each summary carrying its testTemplateArn, name and description) and delete the SCAFFOLDED marker; and fix the response serializer/routing so this service's responses, success and error alike, are rendered through the restJson1 renderer with `Content-Type: application/json` and the model's 200 success status, never an awsJson 1.1 envelope.

Traces: fixture resiliencehubv2-catalog-list-test-templates step task#1.

## Evidence
Fixture resiliencehubv2-catalog-list-test-templates, step task#1, operation ListTestTemplates (GET /v2/list-test-templates), verdict mismatch.

Expected (recorded AWS response): HTTP 200; `$.testTemplates` present as a list of template summaries, e.g. [{name: 'Availability Zone: recovery', testTemplateArn: 'arn:aws:resiliencehub:us-west-2:aws:test-template/aws-az-recovery:rtaz001', description: 'Validate your service can detect and recover from an Availability Zone impairment.'}, {name: 'Dependency validation', testTemplateArn: 'arn:aws:resiliencehub:us-west-2:aws:test-template/aws-dependency-validation:rtdep001'}, {name: 'Multi-Region: isolation', testTemplateArn: 'arn:aws:resiliencehub:us-west-2:aws:test-template/aws-multi-region-isolation:rtmr001'}, {name: 'Multi-Region: recovery', testTemplateArn: 'arn:aws:resiliencehub:us-west-2:aws:test-template/aws-multi-region-recovery:rtmr002'}] (the arns/descriptions are examples of the catalog's content, not a required literal); `$headers.content-type` = 'application/json'.

Actual (emulator): HTTP 500; `$.testTemplates` missing (got None); `$headers.content-type` = 'application/x-amz-json-1.1'.

Requirements diverging: the 200 success status, the presence of the @required `testTemplates` member, and the restJson1 `application/json` framing. Divergence key: service=resiliencehubv2|ListTestTemplates|status=200->500|keys=$.testTemplates,$headers.content-type,status.

Model evidence: smithy_query query=operation operation=ListTestTemplates -> http {method: GET, uri: /v2/list-test-templates, code: 200, codeDefaulted: true}, output {testTemplates: TestTemplateSummaryList, required: true}, errors AccessDeniedException/403, InternalServerException/500, ValidationException/400; smithy_query query=digest -> protocols ['restJson1'].

Code evidence: services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/list_test_templates.rs:3-6 carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 151 returns `Err(crate::scaffolded::Scaffolded::not_implemented("ListTestTemplates"))`; SUCCESS_STATUS is declared 200 at line 132.

## Steps to Reproduce
1. Boot the emulator from this checkout (mainline) and confirm /__admin/health answers 200.
2. Replay the wire corpus simparity-wire-aws-v1, or issue the single step directly: a sigv4-signed GET to the resiliencehubv2 endpoint at path /v2/list-test-templates with an empty request body (ListTestTemplates takes no input members).
3. Observe the response status is 500 with `Content-Type: application/x-amz-json-1.1` and no `testTemplates` member in the body.
4. Expected per the Smithy model and the recording: status 200, `Content-Type: application/json`, and a body whose required `testTemplates` member is a non-empty list of test-template summaries (testTemplateArn, name, description).
5. Re-run fixture resiliencehubv2-catalog-list-test-templates step task#1 to confirm the fix.
