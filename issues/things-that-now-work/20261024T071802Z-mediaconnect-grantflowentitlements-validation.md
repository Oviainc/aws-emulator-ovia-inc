Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Entitlement Subscribers are stored without checking they are AWS account IDs

_Filed: 2026-10-24T07:18:02Z_
_Operation: GrantFlowEntitlements | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GrantEntitlementRequest$Subscribers` is a required member documented as "The Amazon Web Services account IDs that you want to share your content with. The receiving accounts (subscribers) will be allowed to create their own flows using your content as the source." An entitlement's subscriber list is therefore a list of 12-digit AWS account IDs, and the service refuses a value that cannot be one.

Expected: `{"entitlements":[{"name":"e1","subscribers":["not-an-account"]}]}` is refused with HTTP 400 BadRequestException naming the invalid account ID, and the flow gains no entitlement.

Actual: the handler never looks at `Subscribers`; `crate::state::mint_entitlement` copies the list verbatim (`subscribers: request.subscribers.clone()`), so any string — a name, an ARN, an empty string, a 5-digit number — is persisted and reported by DescribeFlow as an account the content is shared with. The same unchecked list reaches the store through CreateFlow's `Entitlements` and through UpdateFlowEntitlement's `Subscribers`, which replaces the whole list.

## Evidence

Spec: `smithy_query(query="doc",shape="GrantEntitlementRequest",member="Subscribers")` — "The Amazon Web Services account IDs that you want to share your content with…"; the member is required, and an entitlement is the cross-account sharing primitive, so the value is an account ID by contract.

Handler: `src/ops/grant_flow_entitlements.rs:208-306` validates only the entitlement count and name uniqueness, then mints through `crate::state::mint_entitlement` (`src/state.rs:3028-3052`), which stores `subscribers` unchanged; `src/ops/create_flow.rs:438-453` and `src/ops/update_flow_entitlement.rs:280-282` take the same path.

Test coverage: every entitlement test uses well-formed 12-digit IDs (`tests/all/flow_entitlements.rs:81,177`, `tests/all/update_flow_entitlement.rs:44,174`), so no test asserts that a malformed subscriber is accepted or rejected.

## Steps to Reproduce

1. POST /v1/flows with one source — note the flowArn
2. POST /v1/flows/{FlowArn}/entitlements with {"entitlements":[{"name":"e1","subscribers":["not-an-account","12345"]}]} — observe HTTP 200/201 success where AWS answers 400 BadRequestException for the invalid account IDs
3. GET /v1/flows/{FlowArn} — the entitlement is reported with subscribers ["not-an-account","12345"]
4. PUT /v1/flows/{FlowArn}/entitlements/{EntitlementArn} with {"subscribers":["nope"]} — same acceptance through the update path
