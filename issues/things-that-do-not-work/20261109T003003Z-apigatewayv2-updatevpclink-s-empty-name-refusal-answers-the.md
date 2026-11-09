Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.update-vpc-link-invalid-security-group-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.update-vpc-link-invalid-security-group-rejected|run#17|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Commit: 7357c0aea0dc68ee0c6ea06203400d848de76641

# apigatewayv2: UpdateVpcLink's empty-name refusal answers the emulator's generic length-constraint sentence instead of AWS's "Vpc link name should not be null or empty."

> _Note: this issue was identified against the code base at commit `7357c0aea0dc68ee0c6ea06203400d848de76641`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateVpcLink

## Description
One net-new root cause in the apigatewayv2 emulator, grounded in a failing behavioral check of this suite. Cause 1 of 1 — it must be closed before submitting.

Cause 1 of 1: UpdateVpcLink called with an empty `Name` is correctly refused with HTTP 400 BadRequestException, but the Error.Message is the emulator's own generic length-constraint sentence rather than the dedicated sentence AWS emits for a null/empty VPC link name.
Failing check id: apigatewayv2.update-vpc-link-invalid-security-group-rejected

AWS:      Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json): UpdateVpcLink, PATCH /v2/vpclinks/{VpcLinkId}, success 200; input member `Name` targets StringWithLengthBetween1And128 (documented "A string with a length between [1-128]" — the shape carries NO @length trait, so the bound is the service's to enforce); modeled errors BadRequestException (client, httpError 400), NotFoundException (404), TooManyRequestsException (429). Recorded behavior for the text: an empty `Name` is refused with error code BadRequestException and the message "Vpc link name should not be null or empty." That whole sentence is FIXED text — it embeds no name, id, value or limit from the request or from state, so there is no variable part to substitute. HTTP status 400 and error code BadRequestException are what the emulator already answers and are not in dispute.
Emulator: 400 BadRequestException (status and code correct) with the message "Invalid value for Name: must have length between 1 and 128, got 0." — the generic member/length-bound sentence this service uses for its own constraint rejections, naming the member, the 1..128 bound and the observed length, none of which AWS mentions for this refusal class.
Why:      a client that branches on the refusal text (or an operator/log scraper matching it) never sees the sentence AWS emits for a null-or-empty VPC link name, so message-based handling of this specific refusal fails even though the code matches; and because the check asserts the message before continuing, the downstream contract it guards — a refused UpdateVpcLink leaving Name and SecurityGroupIds untouched, and a subsequent valid rename still applying — is left unverified behind it.
Fix:      UpdateVpcLink's name validation refusal path — the handler lifts the message straight out of the shared name validator (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/update_vpc_link.rs:156-171, `crate::state::validate_vpc_link_name(name).err()` lifted into BadRequestException). The empty/missing-name case has to produce AWS's dedicated null-or-empty sentence for this operation rather than the generic "Invalid value for <member>: must have length between <lo> and <hi>, got <n>." template. Scope it to the empty case of this refusal; do not blanket-rewrite the generic length message used by other members, and keep the existing ordering (validated before anything is written, so a refused patch leaves the link exactly as it was).
Traces:   apigatewayv2.update-vpc-link-invalid-security-group-rejected (verdict mismatch; the sibling legs of the same check — CreateVpcLink with a malformed security group id answering BadRequestException, the rejected create minting no VpcLink, and the valid create carrying its SecurityGroupIds — all passed, so only this message diverges). Not covered by any open issue: the closest neighbours are 20261106T131339Z-apigatewayv2-2-net-new-root-causes-createvpclink-s.md (CreateVpcLink's subnet-not-found relay clause and the $default-stage invoke URL in StageVariables) and 20261104T121154Z-apigatewayv2-3-net-new-root-causes-a-cfn.md (a CFN-created VpcLink unknown to GetVpcLink); neither names UpdateVpcLink's name refusal. The resolved 20261024T120951Z-apigatewayv2-updatevpclink-stub.md only stood the operation up — this is not a re-refusal of a call AWS accepts (AWS refuses this call too), so it is not a reopen of that issue.

## Evidence
Check apigatewayv2.update-vpc-link-invalid-security-group-rejected, verdict mismatch:

  File "check_apigatewayv2_update_vpc_link_invalid_security_group_rejected.py", line 52, in run
    assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
  AssertionError: expected 'Vpc link name should not be null or empty.', got 'Invalid value for Name: must have length between 1 and 128, got 0.'

Call under test: UpdateVpcLink(VpcLinkId=<an existing, provisioned link>, Name="").
Expected (AWS): HTTP 400, Error.Code BadRequestException, Error.Message "Vpc link name should not be null or empty." (entirely fixed text — no request- or state-derived values).
Actual (emulator): HTTP 400, Error.Code BadRequestException (both correct), Error.Message "Invalid value for Name: must have length between 1 and 128, got 0." — the generic constraint template, whose variable slots (member name `Name`, bound 1/128, observed length 0) AWS does not emit for this refusal.
Preceding assertions in the same check passed: CreateVpcLink with SecurityGroupIds=["not-a-valid-security-group-id"] answered BadRequestException, GetVpcLinks showed no link minted for the rejected name, and the valid CreateVpcLink echoed its SecurityGroupIds. The check aborts at the message assertion, so the "rejected UpdateVpcLink leaves Name/SecurityGroupIds unchanged" and "a valid rename still applies" legs were never reached.

## Steps to Reproduce
1. Pick two subnets in one VPC and that VPC's default security group (ec2 DescribeSubnets / DescribeSecurityGroups).
2. apigatewayv2 CreateVpcLink(Name=<unique>, SubnetIds=<the two subnet ids>, SecurityGroupIds=<the default sg id>) and note the returned VpcLinkId.
3. apigatewayv2 UpdateVpcLink(VpcLinkId=<that id>, Name="").
4. Read the error: Error.Code is BadRequestException with HTTP 400 (correct), but Error.Message must be exactly "Vpc link name should not be null or empty."; the emulator answers "Invalid value for Name: must have length between 1 and 128, got 0."
5. Then confirm the refusal changed nothing (GetVpcLink reports the pre-update Name and SecurityGroupIds) and that a valid rename still applies — both legs are currently unverified because the check stops at step 4.
Replay: run_behavioral_suite and re-run apigatewayv2.update-vpc-link-invalid-security-group-rejected.
