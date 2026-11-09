Issue-Source: trace-sweep-divergence
Issue-Trace: route53globalresolver-dns-view-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53globalresolver-dns-view-not-found route53globalresolver-global-resolver-not-found-and-validation
Issue-Shape-Id: com.amazonaws.route53globalresolver#EC2DNSGlobalResolverCustomerAPI
Issue-Key: route53globalresolver|CreateDNSView|4xx->4xx|$.message
Issue-Key: route53globalresolver|CreateDNSView|4xx->4xx|$.resourceId
Issue-Key: route53globalresolver|CreateDNSView|4xx->4xx|$.resourceType
Issue-Key: route53globalresolver|GetDNSView|4xx->4xx|$.message
Issue-Key: route53globalresolver|GetDNSView|4xx->4xx|$.resourceId
Issue-Key: route53globalresolver|GetDNSView|4xx->4xx|$.resourceType
Issue-Key: route53globalresolver|ListDNSViews|4xx->4xx|$.message
Issue-Key: route53globalresolver|ListDNSViews|4xx->4xx|$.resourceId
Issue-Key: route53globalresolver|ListDNSViews|4xx->4xx|$.resourceType
Issue-Key: route53globalresolver|GetGlobalResolver|4xx->4xx|$.message
Issue-Key: route53globalresolver|GetGlobalResolver|4xx->4xx|$.resourceId
Issue-Key: route53globalresolver|GetGlobalResolver|4xx->4xx|$.resourceType
Issue-Key: route53globalresolver|DeleteGlobalResolver|4xx->4xx|$.message
Issue-Key: route53globalresolver|DeleteGlobalResolver|4xx->4xx|$.resourceId
Issue-Key: route53globalresolver|DeleteGlobalResolver|4xx->4xx|$.resourceType
Issue-Commit: 7e082a9dfef7650059a714e1b731386462df20dc

# Route 53 Global Resolver refusal bodies are emulator-authored: no [GR-ERRnnnnn] code, wrong not-found wording, extra resourceId/resourceType, and anchored regex in ValidationException (6 causes)

> _Note: this issue was identified against the code base at commit `7e082a9dfef7650059a714e1b731386462df20dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGlobalResolver

## Description
Every refusal body route53globalresolver serves is written by the emulator rather than reproduced as AWS words it. The HTTP status and the error class are already right on all six causes (404 ResourceNotFoundException, 400 ValidationException); what diverges is the body: the `message` member carries emulator prose with no `[GR-ERRnnnnn]` code, the not-found bodies carry two members AWS does not send, and the ValidationException message quotes the Smithy @pattern with its regex anchors still attached. Six distinct causes, all in this service, all on the refusal path. THIS IS SIX CAUSES — close all six before submitting.

Cause 1 of 6: CreateDNSView against a globalResolverId that does not exist.
AWS: Smithy CreateDNSView, http POST /dns-views/{globalResolverId}, globalResolverId is an @httpLabel of ResourceId (@length 1..64, @pattern ^[-.a-zA-Z0-9]+$); modeled error ResourceNotFoundException @error(client) @httpError(404). Recorded behavior for the message template: the body is exactly {"message": "[GR-ERR00309] No GlobalResolver found with the ID: <globalResolverId>"}. Fixed text = "[GR-ERR00309] No GlobalResolver found with the ID: "; the only variable part is the globalResolverId taken verbatim from the request path. Recorded behavior only for the members: although the model declares `resourceId` and a @required `resourceType` on ResourceNotFoundException, the recorded 404 body omits BOTH — the wire shape is message-only.
Emulator: 404, correct class, but message "global-resolver <id> was not found" and it additionally serializes resourceId="<id>" and resourceType="global-resolver".
Why: a client that matches on the GR-ERR00309 code (the documented way to tell this refusal apart from the other not-found classes) sees no code at all and falls through its handler; a client that round-trips the error body sees two members real AWS never sends, so golden-body comparisons and SDK-shape assertions fail against the emulator and pass against AWS.
Fix: the CreateDNSView handler's not-found construction and this service's ResourceNotFoundException serializer — emit the GR-ERR00309 template and stop populating resourceId/resourceType.
Traces: route53globalresolver-dns-view-not-found#task#1.

Cause 2 of 6: GetDNSView against a dnsViewId that does not exist.
AWS: Smithy GetDNSView, http GET /dns-views/{dnsViewId}, dnsViewId an @httpLabel ResourceId; error ResourceNotFoundException @httpError(404). Recorded body: {"message": "[GR-ERR00309] No DnsView found with the ID: <dnsViewId>"} — same GR-ERR00309 code as cause 1 but the resource noun is "DnsView", so the fixed text is "[GR-ERR00309] No DnsView found with the ID: " with the request's dnsViewId as the only variable part. No resourceId/resourceType members.
Emulator: message "dns-view <id> was not found", plus resourceId and resourceType="dns-view".
Why: as cause 1 — no code to branch on, and the emulator's noun ("dns-view") is its own internal resource-type token rather than the API's spelling, so a client cannot recover the resource kind from the message either.
Fix: the GetDNSView handler's not-found construction.
Traces: route53globalresolver-dns-view-not-found#task#2.

Cause 3 of 6: ListDNSViews against a globalResolverId that does not exist.
AWS: Smithy ListDNSViews, http GET /dns-views/resolver/{globalResolverId}, @paginated; error ResourceNotFoundException @httpError(404). Recorded body: {"message": "[GR-ERR02300] No GlobalResolver found with ID <globalResolverId>"}. Note this is a DIFFERENT template from cause 1 even though the missing resource is the same kind: the code is GR-ERR02300, not GR-ERR00309, and the fixed text is "[GR-ERR02300] No GlobalResolver found with ID " — no colon before the id. Variable part: the request's globalResolverId. No resourceId/resourceType.
Emulator: message "global-resolver <id> was not found", plus resourceId and resourceType="global-resolver" — i.e. the same single generic string it uses for every not-found, which cannot be right for both GR-ERR00309 and GR-ERR02300.
Why: the list path and the read path are distinguishable on AWS by their error codes; collapsing them loses the distinction, and any client keying retry/branch logic on GR-ERR02300 never matches.
Fix: the ListDNSViews handler's not-found construction — it needs its own GR-ERR02300 template, not a shared helper.
Traces: route53globalresolver-dns-view-not-found#task#6.

Cause 4 of 6: GetGlobalResolver against a globalResolverId that does not exist.
AWS: Smithy GetGlobalResolver, http GET /global-resolver/{globalResolverId}; error ResourceNotFoundException @httpError(404). Recorded body: {"message": "[GR-ERR00309] No GlobalResolver found with the ID: <globalResolverId>"} — fixed text "[GR-ERR00309] No GlobalResolver found with the ID: ", variable part the request's globalResolverId; no resourceId/resourceType.
Emulator: message "global-resolver <id> was not found", plus resourceId and resourceType="global-resolver".
Why: as cause 1.
Fix: the GetGlobalResolver handler's not-found construction.
Traces: route53globalresolver-global-resolver-not-found-and-validation#task#1.

Cause 5 of 6: DeleteGlobalResolver against a globalResolverId that does not exist.
AWS: Smithy DeleteGlobalResolver, http DELETE /global-resolver/{globalResolverId}, @idempotent; error ResourceNotFoundException @httpError(404). Recorded body: {"message": "[GR-ERR00309] No GlobalResolver found with the ID: <globalResolverId>"}, message-only, same template as cause 4.
Emulator: message "global-resolver <id> was not found", plus resourceId and resourceType="global-resolver".
Why: as cause 1; deletion-is-idempotent probes that distinguish "already gone" from other refusals by the GR code cannot do so.
Fix: the DeleteGlobalResolver handler's not-found construction.
Traces: route53globalresolver-global-resolver-not-found-and-validation#task#4.

Cause 6 of 6: GetGlobalResolver with a globalResolverId that violates the member @pattern — the ValidationException message reports the regex with its anchors.
AWS: Smithy GetGlobalResolver.globalResolverId targets ResourceId with @pattern ^[-.a-zA-Z0-9]+$; error ValidationException @error(client) @httpError(400). Recorded body message: "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: [-.a-zA-Z0-9]+". Fixed text = "1 validation error detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts = the rejected value from the request, the member name, and the pattern. The rule the recording establishes is that AWS renders the pattern with the leading ^ and trailing $ STRIPPED, i.e. the bare body of the regex.
Emulator: status 400 and class are correct, and the whole template is correct, but it prints the pattern verbatim from the model — "...regular expression pattern: ^[-.a-zA-Z0-9]+$" — so the message differs from AWS's by those two anchor characters on every pattern-constrained member of this service, not just this one.
Why: validation-message assertions and user-facing surfaces that echo the message show a regex AWS never shows; a client that parses the pattern out of the message and re-applies it gets a subtly different (anchored) expression.
Fix: the shared ValidationException message formatter for this service's @pattern constraint — strip ^ / $ when rendering the pattern.
Traces: route53globalresolver-global-resolver-not-found-and-validation#task#2.

No existing open issue covers route53globalresolver; the many open route53resolver / route53profiles message-wording issues are a different service and do not cover these steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 fixtures, 35 steps, 2 passed). All six clusters match on status and error class and diverge only in the body.

Cause 1 — route53globalresolver-dns-view-not-found#task#1, CreateDNSView, expected 404 / actual 404:
  $.message: expected '[GR-ERR00309] No GlobalResolver found with the ID: rslvr-sim2real-probe-missing-f16676d3', got 'global-resolver rslvr-sim2real-probe-missing-f16676d3 was not found'
  $.resourceId: expected absent, got 'rslvr-sim2real-probe-missing-f16676d3'
  $.resourceType: expected absent, got 'global-resolver'

Cause 2 — route53globalresolver-dns-view-not-found#task#2, GetDNSView, expected 404 / actual 404:
  $.message: expected '[GR-ERR00309] No DnsView found with the ID: rslvr-dnsview-sim2real-probe-missing-ade11460', got 'dns-view rslvr-dnsview-sim2real-probe-missing-ade11460 was not found'
  $.resourceId: expected absent, got 'rslvr-dnsview-sim2real-probe-missing-ade11460'
  $.resourceType: expected absent, got 'dns-view'

Cause 3 — route53globalresolver-dns-view-not-found#task#6, ListDNSViews, expected 404 / actual 404:
  $.message: expected '[GR-ERR02300] No GlobalResolver found with ID rslvr-sim2real-probe-missing-f16676d3', got 'global-resolver rslvr-sim2real-probe-missing-f16676d3 was not found'
  $.resourceId: expected absent, got 'rslvr-sim2real-probe-missing-f16676d3'
  $.resourceType: expected absent, got 'global-resolver'

Cause 4 — route53globalresolver-global-resolver-not-found-and-validation#task#1, GetGlobalResolver, expected 404 / actual 404:
  $.message: expected '[GR-ERR00309] No GlobalResolver found with the ID: rslvr-sim2real-probe-missing-a458f829', got 'global-resolver rslvr-sim2real-probe-missing-a458f829 was not found'
  $.resourceId / $.resourceType: expected absent, got 'rslvr-sim2real-probe-missing-a458f829' / 'global-resolver'

Cause 5 — route53globalresolver-global-resolver-not-found-and-validation#task#4, DeleteGlobalResolver, expected 404 / actual 404:
  $.message: expected '[GR-ERR00309] No GlobalResolver found with the ID: rslvr-sim2real-probe-missing-a458f829', got 'global-resolver rslvr-sim2real-probe-missing-a458f829 was not found'
  $.resourceId / $.resourceType: expected absent, got 'rslvr-sim2real-probe-missing-a458f829' / 'global-resolver'

Cause 6 — route53globalresolver-global-resolver-not-found-and-validation#task#2, GetGlobalResolver, expected 400 / actual 400:
  $.message: expected "1 validation error detected: Value 'bad id!' at 'globalResolverId' failed to satisfy constraint: Member must satisfy regular expression pattern: [-.a-zA-Z0-9]+", got "1 validation error detected: Value 'bad id!' at 'globalResolverId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-.a-zA-Z0-9]+$"

The ids above are one recording's values and only fill the templates; the requirement is the fixed text plus the request-derived substitutions described per cause. Model evidence: smithy_query operation CreateDNSView/GetDNSView/ListDNSViews/GetGlobalResolver/DeleteGlobalResolver and shape ResourceNotFoundException (members message @required, resourceId, resourceType @required; @error client, @httpError 404) / ValidationException (@httpError 400) on services/service-com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi/smithy-model/com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi.json.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1, or drive the steps directly against the route53globalresolver endpoint:

1. Cause 1: POST /dns-views/{globalResolverId} with a globalResolverId that was never created (e.g. rslvr-sim2real-probe-missing-f16676d3) and a valid body. Expect 404 with body {"message":"[GR-ERR00309] No GlobalResolver found with the ID: <that id>"} and no other members. Fixture route53globalresolver-dns-view-not-found step task#1.
2. Cause 2: GET /dns-views/{dnsViewId} with a dns-view id that does not exist. Expect 404 {"message":"[GR-ERR00309] No DnsView found with the ID: <that id>"}. Fixture route53globalresolver-dns-view-not-found step task#2.
3. Cause 3: GET /dns-views/resolver/{globalResolverId} with a global-resolver id that does not exist. Expect 404 {"message":"[GR-ERR02300] No GlobalResolver found with ID <that id>"} — code GR-ERR02300, no colon. Fixture route53globalresolver-dns-view-not-found step task#6.
4. Cause 4: GET /global-resolver/{globalResolverId} with an id that does not exist. Expect 404 {"message":"[GR-ERR00309] No GlobalResolver found with the ID: <that id>"}. Fixture route53globalresolver-global-resolver-not-found-and-validation step task#1.
5. Cause 5: DELETE /global-resolver/{globalResolverId} with an id that does not exist. Expect the same GR-ERR00309 404 body. Fixture route53globalresolver-global-resolver-not-found-and-validation step task#4.
6. Cause 6: GET /global-resolver/{globalResolverId} with a label that violates @pattern ^[-.a-zA-Z0-9]+$ (e.g. the value `bad id!`). Expect 400 ValidationException whose message ends "...Member must satisfy regular expression pattern: [-.a-zA-Z0-9]+" with NO ^ or $. Fixture route53globalresolver-global-resolver-not-found-and-validation step task#2.

In every case assert the body members too, not just the status: the 404 bodies must contain `message` only.
