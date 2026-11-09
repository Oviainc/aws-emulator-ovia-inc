Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation.deploy-apigateway-client-certificate-resource-round-trip
Issue-Key: apigateway__cloudformation.deploy-apigateway-client-certificate-resource-round-trip|run#9|AssertionError|assert listed == [ref], f"certificates described {description!r}: {listed!r}, Ref {ref!r}"
Issue-Commit: c99e9fc9078aba345d9d11ad07e2d4c44a175a86

# CloudFormation AWS::ApiGateway::ClientCertificate reports CREATE_COMPLETE without creating a client certificate: Ref is a synthesized CFN name and GetClientCertificates lists nothing

> _Note: this issue was identified against the code base at commit `c99e9fc9078aba345d9d11ad07e2d4c44a175a86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack (AWS::ApiGateway::ClientCertificate) / GenerateClientCertificate

## Description
One net-new root cause.

Cause 1 of 1: a CloudFormation stack whose only resource is AWS::ApiGateway::ClientCertificate reaches CREATE_COMPLETE, but no API Gateway client certificate is created and the resource's physical id is not a client-certificate id. Failing check: apigateway__cloudformation.deploy-apigateway-client-certificate-resource-round-trip.

AWS: provisioning AWS::ApiGateway::ClientCertificate makes CloudFormation perform API Gateway GenerateClientCertificate (Smithy: GenerateClientCertificate, POST /clientcertificates, HTTP 201, input member `description`, output shape ClientCertificate with members clientCertificateId, description, pemEncodedCertificate, createdDate, expirationDate, tags) with the template's Description, and the resource's physical id — the value {"Ref": "<logical id>"} resolves to — is the returned `clientCertificateId` (an API Gateway-minted certificate id, not a name derived from the stack). From that moment the certificate is readable through the service's own doors: GetClientCertificates (Smithy: GET /clientcertificates, paginated inputToken/outputToken `position`, items member `items`, jsonName `item`) lists exactly one item whose `description` is the template's Description and whose `clientCertificateId` equals that Ref, and GetClientCertificate (GET /clientcertificates/{clientCertificateId}) returns that same record with `pemEncodedCertificate` carrying a real PEM (it begins with the fixed text `-----BEGIN CERTIFICATE-----\r\n`) and `expirationDate` strictly after `createdDate`. After the stack is deleted the id is gone: GetClientCertificate on it answers HTTP 404 NotFoundException with message `Invalid client certificate id specified` — that message is entirely fixed text, with no part taken from the request.

Emulator: the stack reaches CREATE_COMPLETE and the Output carrying {"Ref": "ClientCertificate"} resolves to a CloudFormation-synthesized physical name of the form `<stack-name>-<logical-id>-<hex suffix>` instead of an API Gateway `clientCertificateId`, and an independent read of the service shows nothing was created: GetClientCertificates answers 200 with an empty `items` list, so no certificate carries the template's Description. The replayed check fails at its first independent read with `AssertionError: certificates described '<description>': [], Ref '<stack>-ClientCertificate-<suffix>'` (the description, stack name and suffix are per-run values; the shape of the mismatch — empty listing plus a non-id Ref — is the requirement that is violated). Because nothing exists, the later GetClientCertificate read, the PEM/expiration assertions and the post-delete 404 NotFoundException step are never reached.

Why: a client relying on AWS's behavior deploys a stack, is told CREATE_COMPLETE, and then cannot use the resource at all. {"Ref": "ClientCertificate"} — the documented way to pass the certificate to AWS::ApiGateway::Stage ClientCertificateId or to GetClientCertificate/UpdateClientCertificate/DeleteClientCertificate — hands the client a stack-derived name that API Gateway does not know, so every downstream call 404s; and code that discovers the certificate by listing and filtering on its description finds nothing. Backend mutual-TLS wiring provisioned by template is silently absent while the deployment reports success.

Fix: the CloudFormation resource provider for AWS::ApiGateway::ClientCertificate. On CREATE it must perform the API Gateway GenerateClientCertificate behavior (minting the certificate with the template's Description and a real PEM, createdDate/expirationDate) against the apigateway service's client-certificate state, and use the returned clientCertificateId as the resource's physical id so Ref resolves to it; the record must then be visible to GetClientCertificates/GetClientCertificate, and be removed on stack DELETE so the id afterwards answers 404 NotFoundException with AWS's fixed message. Related open issue 20261109T123922Z-api-gateway-getclientcertificates-stub.md carries this same check's replay key for a GetClientCertificates 501 scaffold stub; at the replayed commit GetClientCertificates answers 200 with an empty items list (no refusal), so the divergence reported here is the CFN provisioning/physical-id wiring, not that stub refusal, and it does not cover this.

Traces: apigateway__cloudformation.deploy-apigateway-client-certificate-resource-round-trip (behavioral check, mismatch).

## Evidence
Check: apigateway__cloudformation.deploy-apigateway-client-certificate-resource-round-trip (verdict: mismatch).

Template deployed: {"Resources": {"ClientCertificate": {"Type": "AWS::ApiGateway::ClientCertificate", "Properties": {"Description": "<unique description>"}}}, "Outputs": {"ClientCertificateRef": {"Value": {"Ref": "ClientCertificate"}}}}

Expected (AWS):
- Stack CREATE_COMPLETE.
- Output ClientCertificateRef == the clientCertificateId minted by GenerateClientCertificate.
- GetClientCertificates items filtered on description == [that same id].
- GetClientCertificate(clientCertificateId=<Ref>) returns clientCertificateId/description equal to the Ref and template Description, pemEncodedCertificate starting with the fixed text "-----BEGIN CERTIFICATE-----\r\n", and expirationDate > createdDate.
- After DeleteStack: GetClientCertificate(<Ref>) => HTTP 404 NotFoundException, message "Invalid client certificate id specified" (all fixed text).

Actual (emulator):
- Stack CREATE_COMPLETE.
- Output ClientCertificateRef == a CloudFormation-synthesized name shaped <stack-name>-<logical-id>-<hex suffix> (recorded run: 'sim2real-cfnagwcc-6c6a9f61ba4c-ClientCertificate-A0F07370540F') — not an API Gateway client-certificate id.
- GetClientCertificates => HTTP 200 with items == [] (no certificate with the template's Description).
- Replay failure: AssertionError: certificates described 'sim2real-cfnagwcc-desc-9d925cdb05d6': [], Ref 'sim2real-cfnagwcc-6c6a9f61ba4c-ClientCertificate-A0F07370540F' (names/suffixes are per-run values).
- The GetClientCertificate read, PEM/date assertions and the post-delete 404 step were never reached.

## Steps to Reproduce
1. Boot the emulator (cloudformation + apigateway).
2. CreateStack with TemplateBody: {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"ClientCertificate":{"Type":"AWS::ApiGateway::ClientCertificate","Properties":{"Description":"my-cert-desc"}}},"Outputs":{"ClientCertificateRef":{"Value":{"Ref":"ClientCertificate"}}}}
3. Poll DescribeStacks until CREATE_COMPLETE, then read Outputs: ClientCertificateRef is a <stack>-ClientCertificate-<suffix> name instead of an API Gateway clientCertificateId.
4. Call apigateway GetClientCertificates (paginate on position) and filter items on description == "my-cert-desc": the list is empty, where AWS lists exactly one item whose clientCertificateId equals the Ref.
5. Call apigateway GetClientCertificate(clientCertificateId=<Ref>): AWS returns the certificate (PEM beginning "-----BEGIN CERTIFICATE-----\r\n", expirationDate > createdDate).
6. DeleteStack, then GetClientCertificate(<Ref>): AWS answers 404 NotFoundException "Invalid client certificate id specified".
7. Re-replay apigateway__cloudformation.deploy-apigateway-client-certificate-resource-round-trip to confirm.
