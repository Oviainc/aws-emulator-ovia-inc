Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-custom-domain-and-base-path-mapping-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-custom-domain-and-base-path-mapping-error-paths apigateway-domain-name-basepath-negative-paths apigateway-domain-access-association-reject-errors apigateway-domain-access-association-reject-flow apigateway-reject-domain-access-association-not-found apigateway-reject-domain-access-association-malformed-arn apigateway-reject-domain-name-access-association-error-path
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateDomainName|4xx->2xx|$.message
Issue-Key: apigateway|CreateDomainName|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateDomainName|4xx->2xx|status
Issue-Key: apigateway|GetDomainName|4xx->2xx|$.message
Issue-Key: apigateway|GetDomainName|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetDomainName|4xx->2xx|status
Issue-Key: apigateway|UpdateDomainName|4xx->2xx|$.message
Issue-Key: apigateway|UpdateDomainName|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateDomainName|4xx->2xx|status
Issue-Key: apigateway|DeleteDomainName|4xx->2xx|$
Issue-Key: apigateway|DeleteDomainName|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|DeleteDomainName|4xx->2xx|status
Issue-Key: apigateway|RejectDomainNameAccessAssociation|4xx->4xx|$.message
Issue-Key: apigateway|RejectDomainNameAccessAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|RejectDomainNameAccessAssociation|4xx->4xx|status
Issue-Key: apigateway|CreateDomainNameAccessAssociation|4xx->4xx|$.message
Issue-Key: apigateway|DeleteDomainNameAccessAssociation|4xx->4xx|$.message
Issue-Key: apigateway|DeleteDomainNameAccessAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigateway|DeleteDomainNameAccessAssociation|4xx->4xx|status
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway custom domain names and access associations (9 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
CreateDomainName accepts a REGIONAL domain with no certificate (so a domain exists that AWS would never have created), and the domain-name-access-association refusals carry the wrong status or emulator-authored text. Smithy: CreateDomainName models BadRequestException @httpError 400; GetDomainName/UpdateDomainName/DeleteDomainName and the *DomainNameAccessAssociation ops model BadRequestException 400 and NotFoundException 404.

Cause 1 of 9 - CreateDomainName for endpointConfiguration REGIONAL without regionalCertificateArn/certificateArn. AWS: 400 BadRequestException `A certificate was not provided for the endpoint type REGIONAL.` (fixed text; the endpoint type comes from the request). Emulator: 201 - the domain is created. Why: a client's negative test passes and the emulator holds a domain with no certificate. Fix: CreateDomainName's certificate validation. Trace: apigateway-custom-domain-and-base-path-mapping-error-paths#task#3 (2 steps).

Cause 2 of 9 - consequence in that fixture: GetDomainName answers 200 instead of 404 `Invalid domain name identifier specified`. Closes with cause 1. Trace: apigateway-custom-domain-and-base-path-mapping-error-paths#task#4.

Cause 3 of 9 - consequence: UpdateDomainName answers 200 instead of 404 `Invalid domain name identifier specified`. Trace: apigateway-custom-domain-and-base-path-mapping-error-paths#task#5.

Cause 4 of 9 - consequence: DeleteDomainName answers 202 instead of 404 `Invalid domain name identifier specified`. Trace: apigateway-custom-domain-and-base-path-mapping-error-paths#task#6.

Cause 5 of 9 - RejectDomainNameAccessAssociation for a private custom domain that has no associations. AWS: 404 NotFoundException `This private custom domain name doesn't have any domain name access associations.` (fixed text). Emulator: `Invalid domain name identifier specified`. Fix: the operation's not-found message. Trace: apigateway-domain-access-association-reject-errors#task#1.

Cause 6 of 9 - the same case reached via a well-formed association ARN: AWS answers 404 NotFoundException with that same message, while the emulator answers 400 BadRequestException complaining the ARN is invalid. Both the status and the error code are wrong. Fix: the ARN-parse-then-lookup order in RejectDomainNameAccessAssociation. Trace: apigateway-domain-access-association-reject-flow#task#1 (2 steps).

Cause 7 of 9 - RejectDomainNameAccessAssociation with a malformed domain-name ARN. AWS: 400 BadRequestException `Invalid domain name ARN specified.` (fixed text, no value interpolated). Emulator: `Invalid ARN specified in the request <arn>`. Fix: that validation message. Trace: apigateway-reject-domain-access-association-malformed-arn#task#1 (3 steps).

Cause 8 of 9 - CreateDomainNameAccessAssociation whose domain-name ARN is not a private custom domain. AWS: 400 BadRequestException `Invalid private custom domain name arn.` (fixed text). Emulator: `Invalid domain name identifier specified`. Fix: CreateDomainNameAccessAssociation's validation message. Trace: apigateway-domain-name-basepath-negative-paths#task#12.

Cause 9 of 9 - DeleteDomainNameAccessAssociation with an unparseable association ARN. AWS: 400 BadRequestException `Invalid domain name access association ARN specified.` Emulator: 404 NotFoundException `Invalid domain name access association identifier specified` - wrong status and code. Fix: validate the ARN before lookup. Trace: apigateway-domain-name-basepath-negative-paths#task#13.

The open issues 'apigateway: the domain-name-access-association surface diverges on 3 net-new causes' and 'apigateway: 4 net-new root causes - RejectDomainNameAccessAssociation's same-account refusal answers 501...' describe neighbouring defects but the replay reported no owner for these nine clusters, so they are filed here.

## Evidence
Cause 1: status expected 400 with $.message 'A certificate was not provided for the endpoint type REGIONAL.', got 201.
Causes 2-4: status expected 404 with 'Invalid domain name identifier specified', got 200/200/202.
Cause 5: 404 both; $.message expected "This private custom domain name doesn't have any domain name access associations.", got 'Invalid domain name identifier specified'.
Cause 6: status expected 404 / x-amzn-errortype NotFoundException with that same message, got 400 / BadRequestException 'Invalid ARN specified in the request <arn>'.
Cause 7: $.message expected 'Invalid domain name ARN specified.', got 'Invalid ARN specified in the request not-an-arn'.
Cause 8: $.message expected 'Invalid private custom domain name arn.', got 'Invalid domain name identifier specified'.
Cause 9: status expected 400 with 'Invalid domain name access association ARN specified.', got 404 NotFoundException 'Invalid domain name access association identifier specified'.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Cause 1: POST /domainnames with endpointConfiguration.types=[REGIONAL] and no certificate ARN (apigateway-custom-domain-and-base-path-mapping-error-paths#task#3); expect 400.
3. Causes 5-7: POST /domainnameaccessassociations/.../reject with (a) a well-formed ARN for a domain with no associations and (b) a malformed ARN.
4. Causes 8/9: create/delete a domain-name access association with a non-private domain ARN and with an unparseable association ARN.
