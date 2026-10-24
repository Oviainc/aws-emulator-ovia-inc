Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.deploy-ipset-cfn-referenced-webacl-rule wafv2.deploy-ipset-cloudformation-membership-enforced wafv2.dsw-e2e-list-waf-webacl-findings-lifecycle wafv2.deploy-ipset-cfn-blocks-listed-address wafv2.deploy-webacl-cfn-enforces-default-block-action
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|GetIPSet|ValidationException
Issue-Key: wafv2.dsw-e2e-list-waf-webacl-findings-lifecycle|phase#3|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: wafv2.deploy-ipset-cfn-blocks-listed-address|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Key: wafv2.deploy-webacl-cfn-enforces-default-block-action|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# CloudFormation-deployed WAFv2 resources are not real WAF entities: IPSet Ref/outputs are not an IPSet Id, WebACL GetAtt Arn is malformed, and the protected edge never applies the ACL

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIPSet

## Description
Three distinct root causes in the CloudFormation-provisioned WAFv2 surface. All are grounded in failing behavioral checks of the wafv2 service.

Cause 1 of 3: the identifier a stack publishes for an AWS::WAFv2::IPSet is not the IPSet's Id.
AWS: an AWS::WAFv2::IPSet created by a stack is a real WAFv2 IPSet, and the identifiers the stack publishes (Ref / GetAtt Id / an output built from GetAtt Arn) resolve against the WAFv2 control plane. Smithy (com.amazonaws.wafv2): GetIPSet.Id -> EntityId, @length 1..36, @pattern ^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$ — the Id is always a UUID, and the ARN's last segment is that UUID. Recorded behavior only for the exact ARN layout.
Emulator: the value the stack publishes is the stack/resource NAME, so GetIPSet immediately refuses the emulator's own identifier with 400 ValidationException: "1 validation error detected: Value 'sim2real-waf-ipset-073157b97252' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-..." (same for 'sim2real-wafv2-ipset-83cc21a404eb'). Either no IPSet was created at all, or it was created and its Id is not what the stack reports.
Why: no IaC-deployed WAFv2 IPSet can be read back, updated, or referenced from a rule — every follow-up control-plane call is refused by the emulator's own validator.
Fix: the CloudFormation WAFv2 resource provider (AWS::WAFv2::IPSet create/physical-resource-id and attribute resolution) together with the wafv2 IPSet state it must create.
Traces: wafv2.deploy-ipset-cfn-referenced-webacl-rule, wafv2.deploy-ipset-cloudformation-membership-enforced.

Cause 2 of 3: the Arn attribute of an AWS::WAFv2::WebACL does not have the WAFv2 web-ACL ARN layout.
AWS: a web ACL ARN ends in the entity's name followed by its Id (…/webacl/<name>/<id>), so GetAtt Arn and GetAtt Id agree with each other and with GetWebACL. Recorded behavior only for the ARN text; what is structural is that the ARN carries the entity id.
Emulator: GetAtt Arn is 'arn:aws:wafv2:us-west-2:000000000000:web-acl/sim2real-dsw-acl-34b2b0d108e7' — no '/webacl/<name>/<id>' tail and no id at all, so the ARN cannot be decomposed into the triple the API needs.
Why: a template that passes the web ACL ARN to another resource or to a WAFv2 call (associations, logging configuration, Security-Hub style consumers) hands on an ARN that identifies nothing.
Fix: the CloudFormation WAFv2 resource provider's attribute resolution for AWS::WAFv2::WebACL, which must surface the ARN minted by the wafv2 CreateWebACL state rather than fabricate one.
Traces: wafv2.dsw-e2e-list-waf-webacl-findings-lifecycle (phase 1/4 deploy).

Cause 3 of 3: traffic to the resource protected by a stack-deployed web ACL never gets a WAF decision.
AWS: once AWS::WAFv2::WebACL (DefaultAction Block) and AWS::WAFv2::WebACLAssociation reach CREATE_COMPLETE, requests to the associated resource are answered with the ACL applied — an unmatched request gets 403, and a request from an address inside a referenced IPSet gets 403 while one outside gets 200. Recorded behavior only for the per-case statuses; what is structural is that every request to the protected endpoint produces an HTTP response.
Emulator: no response is ever observed. Both checks poll the protected endpoint and exhaust the harness budget — detail is exactly "Check timed out after 60.0s", so no allow/block decision is observable at all.
Why: the entire IaC-deployed WAF enforcement path is unverifiable; a client sees a hang where AWS serves an allow or a block.
Fix: the data-plane request path for a resource with a web ACL associated (web-ACL evaluation and response generation), which must always produce a response.
Traces: wafv2.deploy-ipset-cfn-blocks-listed-address, wafv2.deploy-webacl-cfn-enforces-default-block-action.

Related open issues (they do NOT cover these causes): 20261023T210711Z-apigateway-wafv2-all-eleven-wafv2-at-the-edge.md and 20261023T210711Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks.md describe the same no-response symptom for apigateway-owned checks.

## Evidence
Cause 1 — botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the GetIPSet operation: 1 validation error detected: Value 'sim2real-waf-ipset-073157b97252' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$ (and the same with 'sim2real-wafv2-ipset-83cc21a404eb'). Expected: a UUID Id for the stack-created IPSet and a successful GetIPSet.
Cause 2 — AssertionError: [phase 1/4 deploy] GetAtt Arn 'arn:aws:wafv2:us-west-2:000000000000:web-acl/sim2real-dsw-acl-34b2b0d108e7' must end in /webacl/<name>/<GetAtt Id>. Expected: an ARN whose tail is the web ACL name and its Id.
Cause 3 — detail for both checks is exactly "Error: Check timed out after 60.0s"; no HTTP status ever arrives from the protected endpoint, where AWS answers 403 for the blocked case and 200 for the allowed control.

## Steps to Reproduce
1. Deploy a stack containing AWS::WAFv2::IPSet (CIDR entries) plus an AWS::WAFv2::WebACL whose rule references the IPSet, and an AWS::WAFv2::WebACLAssociation to a REST API stage.
2. Take the IPSet identifier the stack publishes and call wafv2 GetIPSet with it — the emulator refuses its own identifier with 400 ValidationException (cause 1).
3. Read GetAtt Arn of the AWS::WAFv2::WebACL and observe it has no /webacl/<name>/<id> tail (cause 2).
4. Send a request from an address inside the IPSet, and an unmatched request to a Block-default ACL, to the associated resource — no response is produced (cause 3).
5. Or replay the check ids in trace_ids.
