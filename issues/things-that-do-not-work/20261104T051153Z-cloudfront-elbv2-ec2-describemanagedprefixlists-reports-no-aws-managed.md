Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__elbv2.xsvc-cloudfront-2-alb-origin-protocol-policy-mismatch-rejected cloudfront__elbv2.xsvc-cloudfront-2-alb-origin-secret-header-direct-request-forbidden
Issue-Key: cloudfront__elbv2.xsvc-cloudfront-2-alb-origin-protocol-policy-mismatch-rejected|_public_alb#6|IndexError|origin_facing = ec2.describe_managed_prefix_lists(Filters=[{
Issue-Key: cloudfront__elbv2.xsvc-cloudfront-2-alb-origin-secret-header-direct-request-forbidden|_public_alb#7|IndexError|origin_facing = ec2.describe_managed_prefix_lists(Filters=[{
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudfront__elbv2: EC2 DescribeManagedPrefixLists reports no AWS-managed CloudFront origin-facing prefix list, so an ALB origin cannot be locked to CloudFront

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedPrefixLists

## Description
Cause 1 of 1: DescribeManagedPrefixLists filtered by the AWS-managed CloudFront origin-facing prefix-list name answers 200 with an EMPTY PrefixLists list, so a security group cannot be authorized against CloudFront's origin-facing ranges and every flow that fronts an ALB origin with CloudFront stops before it starts.
Failing checks: cloudfront__elbv2.xsvc-cloudfront-2-alb-origin-protocol-policy-mismatch-rejected, cloudfront__elbv2.xsvc-cloudfront-2-alb-origin-secret-header-direct-request-forbidden.

AWS: DescribeManagedPrefixLists "describes your managed prefix lists and any Amazon Web Services-managed prefix lists" (that operation's own documentation), and the documented `prefix-list-name` filter selects one by name. AWS publishes a managed prefix list named `com.amazonaws.global.cloudfront.origin-facing` — the CloudFront origin-facing list referenced by name in AWS's own "restrict access to your origin" guidance — visible from every Region's EC2 endpoint, so a request with Filters=[{prefix-list-name: com.amazonaws.global.cloudfront.origin-facing}] returns exactly one ManagedPrefixList. Smithy (ec2): ManagedPrefixList carries PrefixListId (a `pl-`-prefixed PrefixListResourceId), PrefixListName, PrefixListArn, AddressFamily, MaxEntries, Version, State (PrefixListState) and OwnerId ("the ID of the owner of the prefix list", which for an AWS-managed list is the literal `AWS`, not an account id). The concrete id, version and entry count are per-environment values and are NOT the requirement; the requirement is that the list exists under that exact name, is reported by both DescribePrefixLists and DescribeManagedPrefixLists with the same id, and its id is accepted by AuthorizeSecurityGroupIngress in IpPermissions[].PrefixListIds.

Emulator: the call is served (HTTP 200, no error) with `PrefixLists` empty. The handler is implemented and does accept the `prefix-list-name` filter, merging the account's stored lists with `crate::state::service_prefix_lists_in_region(&region)` (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_managed_prefix_lists.rs lines 243-256), so the derived published catalog simply does not contain the CloudFront origin-facing list.

Why: locking an origin so that only CloudFront can reach it is the documented, name-based pattern (look the managed prefix list up by name, then authorize tcp/443 or tcp/80 from that PrefixListId). Against the emulator the lookup yields nothing, so a client either crashes on an empty result (as these checks do) or has to open its origin to 0.0.0.0/0 — the exact security property under test becomes untestable and unimplementable.

Fix: the EC2 published service prefix-list catalog — `service_prefix_lists_in_region` in services/service-com-amazonaws-ec2-amazonec2/src/state.rs — must include the global CloudFront origin-facing list (PrefixListName `com.amazonaws.global.cloudfront.origin-facing`, OwnerId `AWS`, AddressFamily IPv4, State `create-complete`) so both services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_managed_prefix_lists.rs and DescribePrefixLists report one id for it, and that id resolves in security-group ingress authorization.

Traces: both failing checks crash on `describe_managed_prefix_lists(...)["PrefixLists"][0]` inside _public_alb (lines 158 and 154 of the two checks respectively).

## Evidence
Expected (AWS): ec2.describe_managed_prefix_lists(Filters=[{"Name":"prefix-list-name","Values":["com.amazonaws.global.cloudfront.origin-facing"]}]) returns PrefixLists with exactly one ManagedPrefixList whose PrefixListName is that name and whose PrefixListId (pl-…) can then be used in AuthorizeSecurityGroupIngress IpPermissions[].PrefixListIds.
Actual (emulator): HTTP 200 with PrefixLists == [], so both checks raise `IndexError: list index out of range` at the `["PrefixLists"][0]` subscript (check_cloudfront__elbv2_xsvc_cloudfront_2_alb_origin_protocol_policy_mismatch_rejected.py line 158 via run line 228; check_cloudfront__elbv2_xsvc_cloudfront_2_alb_origin_secret_header_direct_request_forbidden.py line 154 via run line 202). No error code is returned — the divergence is an empty result set where AWS reports the AWS-managed list.

## Steps to Reproduce
1. Point an EC2 client at the emulator (any Region).
2. Call DescribeManagedPrefixLists with Filters=[{Name:"prefix-list-name",Values:["com.amazonaws.global.cloudfront.origin-facing"]}].
3. Observe PrefixLists is empty; on AWS one ManagedPrefixList is returned.
4. Replay either cloudfront__elbv2 check: both abort in _public_alb while building the ALB's security group, before any CloudFront distribution is created.
