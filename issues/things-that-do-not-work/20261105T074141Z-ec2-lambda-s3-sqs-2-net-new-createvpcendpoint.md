Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3__sqs.duplicate-gateway-endpoint-on-route-table-rejected-with-control ec2__lambda__s3__sqs.gateway-endpoint-describe-omits-eni-and-dns-entries
Issue-Key: ec2__lambda__s3__sqs.duplicate-gateway-endpoint-on-route-table-rejected-with-control|run#11|AssertionError|assert got == ("RouteAlreadyExists", expected_message), (
Issue-Key: ec2__lambda__s3__sqs.gateway-endpoint-describe-omits-eni-and-dns-entries|run#21|KeyError|assert policy["Version"] == "2008-10-17", (
Issue-Commit: 33039e6d53cbc4640ae4d958d554409d2dbcb976

# ec2__lambda__s3__sqs: 2 net-new CreateVpcEndpoint causes — the RouteAlreadyExists refusal uses CreateRoute's generic sentence instead of EC2's route-table/prefix-list sentence, and the service-installed default Gateway endpoint policy omits its Version

> _Note: this issue was identified against the code base at commit `33039e6d53cbc4640ae4d958d554409d2dbcb976`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
This issue bundles 2 net-new root causes in the EC2 crate's CreateVpcEndpoint path, both on the Gateway-endpoint leg that now works far enough to be graded. Both surfaced on checks the replay report did NOT mark ALREADY FILED or OWNED. Related open issues in the same area cover a DIFFERENT failure mode (CreateVpcEndpoint answering 501 NotImplemented whenever RouteTableIds is present) and none of them carries either cause or either check id: 20261104T044826Z-ec2-lambda-s3-sqs-createvpcendpoint-refuses-route-table-associations-and-masks-the.md, 20261104T044826Z-ec2-lambda-s3-createvpcendpoint-with-route-table-associations-is-refused-notimplemented.md, 20261104T062058Z-ec2-createvpcendpoint-answers-501-notimplemented-for-a-gateway.md and 20261102T120557Z-appstream-ec2-ec2-createvpcendpoint-leaves-privatednsenabled-false-for.md (PrivateDnsEnabled default, a different member). In this replay the Gateway endpoint IS created and IS refused for the duplicate route, so what diverges now is the refusal's wording and the created endpoint's default policy document.

--- Cause 1 of 2: the duplicate-prefix-list refusal carries CreateRoute's generic 'route identified by' sentence instead of EC2's CreateVpcEndpoint sentence ---
AWS: recorded behavior only — the EC2 model (ec2Query) declares no errors for CreateVpcEndpoint (smithy_query operation=CreateVpcEndpoint: errors: []), so the code and message are recorded behavior. The recording shows HTTP 400 with error code `RouteAlreadyExists` and the message template `route table <route-table-id> already has a route with destination-prefix-list-id <prefix-list-id>`. Fixed text: `route table `, ` already has a route with destination-prefix-list-id `. Variable parts: the route table id, taken from the offending entry of the request's RouteTableIds, and the prefix-list id of the endpoint's service, taken from the emulator's managed-prefix-list state (the same id DescribePrefixLists reports for `com.amazonaws.<region>.s3`). AWS refuses the WHOLE call when one named table already carries that prefix-list route, leaving the clean table in the same request unmodified.
Emulator: the code is right but the sentence is a different operation's. The emulator answered `('RouteAlreadyExists', 'The route identified by pl-fffbb61c already exists.')` — CreateRoute's generic destination-only sentence — where AWS answers `('RouteAlreadyExists', 'route table <rtb-id> already has a route with destination-prefix-list-id <pl-id>')`. The route table id is absent from the emulator's message altogether, and the fixed text differs word for word.
Why: a client that reads the message to learn WHICH of the route tables it named is the occupied one cannot: the emulator names only the prefix list, which the caller already knows, and never names the table. Error-message assertions, operator runbooks and log scrapers keyed on AWS's sentence all break, and the message does not distinguish a CreateVpcEndpoint conflict from a plain CreateRoute conflict.
Fix: CreateVpcEndpoint's validation/refusal path in the EC2 crate (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs) — raise the route-table-scoped RouteAlreadyExists sentence, rendering the conflicting route table id and the service's prefix-list id, instead of reusing the CreateRoute destination-only sentence.
Traces: ec2__lambda__s3__sqs.duplicate-gateway-endpoint-on-route-table-rejected-with-control.

--- Cause 2 of 2: the service-installed default Gateway endpoint policy document has no Version member ---
AWS: recorded behavior only — CreateVpcEndpoint$PolicyDocument is a plain String in the model with no documented default, so the default document's content is recorded behavior. The recording shows that a Gateway endpoint created with NO PolicyDocument is given a full-access document by the service, and that document is dated `"Version": "2008-10-17"` (the service-installed date, not the 2012-10-17 a caller would author) alongside `"Statement": [{"Effect": "Allow", "Principal": "*", "Action": "*", "Resource": "*"}]`. DescribeVpcEndpoints reports that document verbatim in VpcEndpoint.PolicyDocument.
Emulator: the Statement is correct, but the document carries no `Version` key at all — `json.loads(VpcEndpoint.PolicyDocument)['Version']` raises KeyError, i.e. the emulator serves only the Statement array. The replay fails with `KeyError: 'Version'` at the assertion that the default policy is dated 2008-10-17.
Why: a client that round-trips the endpoint policy (read the default, edit it, ModifyVpcEndpoint it back) produces a policy document with no Version, which real IAM-style policy parsers reject or silently reinterpret under the oldest grammar; anything asserting the default document is the service-installed 2008-10-17 one, or diffing a caller-authored 2012-10-17 policy against the installed default, sees a document that cannot exist in AWS.
Fix: CreateVpcEndpoint's state/defaulting in the EC2 crate (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs and the VpcEndpoint record it writes) — when the request omits PolicyDocument, store the complete service-installed full-access document including its Version member, so the DescribeVpcEndpoints projection reports it.
Traces: ec2__lambda__s3__sqs.gateway-endpoint-describe-omits-eni-and-dns-entries.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; per-service pass rate ec2__lambda__s3__sqs 8/17 (47%).

Cause 1 — [mismatch] ec2__lambda__s3__sqs.duplicate-gateway-endpoint-on-route-table-rejected-with-control:
  AssertionError: a CreateVpcEndpoint naming the occupied table alongside a clean one returned ('RouteAlreadyExists', 'The route identified by pl-fffbb61c already exists.'), expected ('RouteAlreadyExists', 'route table rtb-0000000000000003f already has a route with destination-prefix-list-id pl-fffbb61c')
  (the ids shown are this run's generated values; the requirement is the sentence template, with the route table id from the request and the prefix-list id from state.)
  Expected: HTTP 400, code RouteAlreadyExists, message 'route table <rtb-id> already has a route with destination-prefix-list-id <pl-id>'.
  Actual:   code RouteAlreadyExists, message 'The route identified by <pl-id> already exists.' — CreateRoute's sentence; the route table id is never named.

Cause 2 — [error] ec2__lambda__s3__sqs.gateway-endpoint-describe-omits-eni-and-dns-entries:
  File check_ec2__lambda__s3__sqs_gateway_endpoint_describe_omits_eni_and_dns_entries.py, line 88, in run
    assert policy["Version"] == "2008-10-17"
  KeyError: 'Version'
  The preceding assertion on policy['Statement'] == [{'Effect':'Allow','Principal':'*','Action':'*','Resource':'*'}] PASSED, so the emulator does install a full-access default document — it just emits it without the Version member. Expected document: {"Version":"2008-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}]}.

## Steps to Reproduce
Boot the emulator and replay these two checks from suite simparity-behavioral-aws-v1 (report heading ec2__lambda__s3__sqs):
1. Cause 1 — ec2__lambda__s3__sqs.duplicate-gateway-endpoint-on-route-table-rejected-with-control: create a VPC, an isolated subnet with its own route table, and a second, clean route table in the same VPC. CreateVpcEndpoint(VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[<subnet route table>]) so that table now carries the S3 prefix-list route. Resolve the S3 managed prefix-list id with DescribePrefixLists. Then call CreateVpcEndpoint again with RouteTableIds=[<occupied table>, <clean table>]. Expect HTTP 400, code RouteAlreadyExists and the message 'route table <occupied table id> already has a route with destination-prefix-list-id <prefix list id>'; the clean table must stay unmodified and the same request naming only the clean table must then be accepted.
2. Cause 2 — ec2__lambda__s3__sqs.gateway-endpoint-describe-omits-eni-and-dns-entries: in one VPC, create a Gateway endpoint for com.amazonaws.<region>.s3 with RouteTableIds and an Interface endpoint for com.amazonaws.<region>.sqs with SubnetIds/SecurityGroupIds, sending NO PolicyDocument on either. Poll DescribeVpcEndpoints until both are 'available', then json.loads the Gateway endpoint's PolicyDocument. Expect Version '2008-10-17' and the single full-access Allow statement; the emulator's document has the statement but no Version key.
