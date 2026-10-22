Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconnect-flow-list-pagination
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconnect-flow-list-pagination mediaconnect-flow-lifecycle-basic mediaconnect-flow-sources-entitlements-update
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect|CreateFlow|2xx->4xx|$.flow
Issue-Key: mediaconnect|CreateFlow|2xx->4xx|status
Issue-Key: mediaconnect|ListFlows|2xx->2xx|$.flows[name="sim2real-probe-mediaconnect-flow-b1799d85"].description
Issue-Key: mediaconnect|ListFlows|2xx->2xx|$.flows[*]
Issue-Key: mediaconnect|GrantFlowEntitlements|2xx->2xx|status
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# MediaConnect flow description dropped, duplicate flow name refused, GrantFlowEntitlements success status 200

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlow

## Description
Three distinct root causes in the MediaConnect flow store and its success-status wiring, all in service `mediaconnect`, carried by four replayed divergence clusters. Every cause must be closed before this issue is submitted.

=== Cause 1 of 3: CreateFlow refuses a flow whose name an existing flow already uses ===
AWS: Smithy CreateFlow (POST /v1/flows, success 201) models `Name` as required and documents no uniqueness rule for it; the flow's identity is the generated `flowArn`, whose resource segment carries a per-flow generated id (`flow:1-<id>:<name>`), so two flows with the same name are distinct resources. Recorded behavior: creating a second flow with the SAME name as a live flow succeeds with HTTP 201 and returns a `flow` whose `flowArn` differs from the first flow's only in that generated id segment. The operation's modeled client errors are BadRequestException 400, CreateFlow420Exception 420, ForbiddenException 403, TooManyRequestsException 429.
Emulator: the create is refused with HTTP 400 BadRequestException. `src/ops/create_flow.rs` keeps a `flow_arns_by_name` index and rejects at lines 348-353 with "A flow named ... already exists in this account and Region: <arn>.", on the stated assumption that "The flow name is part of the flow's ARN, so two flows cannot share one name" — but the ARN also carries the generated id, so the name is not the identity.
Why: any client that creates more than one flow from the same template or name prefix (the normal pattern for a redundant A/B pair, and for pagination/fan-out tests) gets a hard 400 on the second create. Downstream state then diverges too: the second flow never exists, so ListFlows returns one page entry where AWS returns two (see the cascade in mediaconnect-flow-list-pagination#task#3, where `flows[1]` is absent entirely).
Fix: CreateFlow's name handling in `src/ops/create_flow.rs` — drop the by-name uniqueness rejection and let the store key flows by their generated ARN, keeping any name index one-to-many.
Traces: mediaconnect-flow-list-pagination#setup#2 (the refused create), mediaconnect-flow-list-pagination#task#3 (the resulting single-entry ListFlows page).

=== Cause 2 of 3: a flow's `description` is never populated ===
AWS: Smithy `Flow.Description` (jsonName `description`) and `ListedFlow.Description` (jsonName `description`, required on the listed shape). CreateFlow's input models no `Description` member, so the value is service-derived. Recorded behavior: AWS always reports a non-empty description for an owned flow — the recordings show both an echo of the source's description ('probe source') and, for a flow whose source request carried none, the generated sentence '<flow name> using source <source name>'. Hypothesis: the flow description is taken from the request's source description when one is given and otherwise generated from the flow and source names. The firm requirement is that `flow.description` is present on DescribeFlow and that `flows[*].description` on ListFlows is the same non-empty value, not an empty string.
Emulator: DescribeFlow omits `flow.description` entirely and ListFlows reports `description: ''`. `src/ops/create_flow.rs` stores `description: None` with the comment "CreateFlow models no `Description` member, so a new flow has none", and the listed projection then renders that as the empty string.
Why: `ListedFlow.Description` is a required member, so a client deserializing a ListFlows page gets a required field that is present-but-empty and a DescribeFlow body where it is missing altogether; consoles and inventory tooling that label flows by description show blanks, and round-tripping a listed flow back into config loses it.
Fix: derive and store the flow description when the flow is created (`src/ops/create_flow.rs`) and project it on both the `Flow` and `ListedFlow` shapes.
Traces: mediaconnect-flow-lifecycle-basic#task#3 (ListFlows, description ''), mediaconnect-flow-list-pagination#task#3 (ListFlows, description '' where the generated sentence is expected), and the `$.flow.description` facet of mediaconnect-flow-lifecycle-basic#task#2.

=== Cause 3 of 3: GrantFlowEntitlements answers HTTP 200 where AWS answers HTTP 201 ===
AWS: recorded behavior — a successful GrantFlowEntitlements (POST /v1/flows/{FlowArn}/entitlements) returns HTTP 201 with `{flowArn, entitlements}`. Note the on-disk Smithy model declares `http.code` 200 for this operation while the recording shows 201; the recorded wire response is what a client observes, so 201 is the requirement and the model's code appears stale. Response body members matched on this step — only the status diverges.
Emulator: returns HTTP 200 (the handler is wired to the model's 200 success status).
Why: a client or gateway that branches on the exact 2xx status (201-means-created handling, strict status assertions, SDK/HTTP middleware that treats 200 vs 201 differently, and any proxy that re-signs or caches by status) takes the wrong branch for a grant that did create entitlements.
Fix: GrantFlowEntitlements' success status wiring in its handler (`SUCCESS_STATUS` for `src/ops/grant_flow_entitlements.rs`).
Traces: mediaconnect-flow-sources-entitlements-update#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1: 16/50 steps passed (32%), 18 divergence clusters, all service=mediaconnect. None of the clusters below carried a stub_path or an owned_by.

Cause 1 — mediaconnect-flow-list-pagination#setup#2 (CreateFlow):
  expected: HTTP 201 with flow name 'sim2real-probe-mediaconnect-flow-d9b429c3' and a flowArn whose generated id differs from the flow created in setup#1
  actual:   HTTP 400, no `flow` member in the body ($.flow: missing)

Cause 1 cascade / Cause 2 — mediaconnect-flow-list-pagination#task#3 (ListFlows, 200/200):
  $.flows[0]: expected description '<name> using source <name>-src', got description ''
  $.flows[1]: missing entirely (expected the second flow's listed entry) — the flow setup#2 was refused

Cause 2 — mediaconnect-flow-lifecycle-basic#task#3 (ListFlows, 200/200):
  $.flows[name="sim2real-probe-mediaconnect-flow-b1799d85"].description: value_mismatch (expected 'probe source', got '')
  and in mediaconnect-flow-lifecycle-basic#task#2 (DescribeFlow, 200/200): $.flow.description: missing (expected 'probe source', got None)

Cause 3 — mediaconnect-flow-sources-entitlements-update#task#4 (GrantFlowEntitlements):
  status: expected 201, got 200 (body members all matched)

## Steps to Reproduce
Boot the emulator and replay the wire corpus (replay_wire_corpus), or drive a botocore mediaconnect client at the emulator endpoint:

Cause 1: mc.create_flow(Name='dup', Source={'Name':'dup-src','Protocol':'rtp','WhitelistCidr':'10.24.43.0/24'}) twice -> the second call raises BadRequestException (HTTP 400) "A flow named \"dup\" already exists..." instead of returning HTTP 201 with a second flowArn. Then mc.list_flows() -> only one entry where two are expected.

Cause 2: create a flow whose source carries Description='probe source', then mc.describe_flow(FlowArn=...) -> `flow` has no `description` key; mc.list_flows() -> that flow's `description` is ''. Create a flow whose source carries no description -> AWS reports '<flow name> using source <source name>', the emulator reports ''.

Cause 3: mc.grant_flow_entitlements(FlowArn=..., Entitlements=[{'Subscribers':['123456789012'],'Name':'e1'}]) and inspect ResponseMetadata.HTTPStatusCode -> 200 instead of 201.
