Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-application-provider-describe-round-trip
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-application-provider-describe-round-trip sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListApplicationProviders|2xx->2xx|$.NextToken
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-74443bb3"].State
Issue-Commit: 29f1e00a7002e451c541eb91ee53a740b213bc0d

# Enumeration responses omit output members: ListApplicationProviders returns no NextToken and ListInstances entries carry no State

> _Note: this issue was identified against the code base at commit `29f1e00a7002e451c541eb91ee53a740b213bc0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApplicationProviders

## Description
TWO causes, both the same class of fault in sso-admin enumeration responses: a member the live API serves on the 200 is never emitted, so the response body is structurally short of what a client reads. A fixer must close BOTH before submitting; they sit in two different response serializers.

Cause 1 of 2 - ListApplicationProviders serves the whole application-provider catalog in a single unpaginated 200 and emits no NextToken (fixture sso-admin-application-provider-describe-round-trip, step task#1).
AWS: ListApplicationProviders is a paginated operation. Smithy: ListApplicationProviders is @paginated with inputToken=NextToken, outputToken=NextToken, pageSize=MaxResults, items=ApplicationProviders; input MaxResults is an integer @range min 1 max 100 and NextToken targets Token (@length 0-2048, @pattern ^[-a-zA-Z0-9+=/_]*$); the output shape ListApplicationProvidersResponse carries ApplicationProviders plus NextToken of the same Token shape. The recorded 200 for a call that does not pass MaxResults returns a page of the catalog together with a non-empty NextToken, because the catalog holds more entries than one page: the requirement is that whenever entries remain beyond the page served, the response carries a NextToken whose value is an opaque continuation token matching the Token pattern. The expected value in the recording is shape-asserted only ('<shaped-not-asserted>'), i.e. the concrete token string is a per-run value and NOT a requirement - only its presence and its Token shape are.
Emulator: 200 with the ApplicationProviders list but $.NextToken missing (expected a shaped Token, got None). The companion catalog cluster on this same operation shows the emulator emitting hundreds of entries (indices 0..393) in that single body, confirming the handler never truncates to a page and therefore never has a continuation to report.
Why: a client paginating the catalog the way the live API requires - call, consume ApplicationProviders, follow NextToken until it is absent - sees a first page that claims to be the last. Against AWS it would continue; here it silently stops, and any client that validates the paginated contract (SDK paginators asserting the token member, code that round-trips the token back in as the request's NextToken to resume) is driven down a path the emulator never exercises. Conversely, code written against the emulator's single giant page breaks on AWS.
Fix: the ListApplicationProviders handler and its response serializer - truncate the catalog to the effective page size (MaxResults when supplied, the service default otherwise) and emit NextToken, using the same page-token encoding the crate's other sso-admin enumerations already use, so the token the handler mints is accepted back as this operation's input NextToken.
Traces: sso-admin-application-provider-describe-round-trip#task#1.

Cause 2 of 2 - ListInstances entries omit the State member (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: ListInstances returns Instances, an InstanceList (@length max 10) of com.amazonaws.ssoadmin#InstanceMetadata. The on-disk model declares NO State member on InstanceMetadata (members stop at InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions), so for State this is recorded behavior only: the recorded 200 carries, on each listed instance, an additional State structure whose single member Name holds that instance's lifecycle value, served alongside the flat Status member and agreeing with it (an active instance lists Status "ACTIVE" and State {"Name": "ACTIVE"}). State.Name is read from the listed instance's own stored status; the instance's identity values in any one recording (the generated instance Name, InstanceArn, IdentityStoreId/IdentityStoreArn, CreatedDate) are per-run values of the recording environment and are not requirements.
Emulator: 200 whose listed instance matches on every other member, including Status, but carries no State member at all - $.Instances[Name="<generated instance name>"].State expected {'Name': 'ACTIVE'}, got None. Because Status is emitted correctly, the omission is in the shared InstanceMetadata serialization, not in the stored lifecycle.
Why: a client that reads an instance's lifecycle from the State view the current API serves - filtering a listing on State.Name == "ACTIVE", or polling ListInstances until a newly enabled instance leaves CREATE_IN_PROGRESS - dereferences a member absent on every entry, so it either faults on the missing key or treats every instance as being in an unknown state.
Fix: the InstanceMetadata response serializer shared by ListInstances - emit State from the same stored instance status the flat Status member is written from, so every listed entry carries State.Name.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Related open issues, neither of which covers these clusters:
- 20261029T184546Z-sso-admin-application-provider-catalog-is-wrong-describe-ignores-the.md owns the CONTENT of the application-provider catalog (wrong ARNs, wrong DisplayData, a different catalog in Describe vs List) on this same operation. It says nothing about pagination and its replay keys do not include the $.NextToken cluster, so Cause 1 is reported unowned by the replay report.
- 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md and 20261030T004149Z-sso-admin-listinstances-omits-state-on-the-instance-enumerated-by.md describe the same State omission, but their recorded replay keys pin the ListInstances cluster to instances named "sim2real-wire-idc-c56c0824", "sim2real-wire-idc-04f3bea9" and "IdentityCenter". The instance name in this fixture is generated per run (this replay enumerated a different generated name), so the key does not join this cluster and the replay report reports it unowned. Cause 2 is filed so this cluster is actually re-replayed before the defect is closed; a fixer closing them together should fix the single serializer site once.
- 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken DECODING (case-insensitive hex tokens) on ListInstances, not an unemitted token on ListApplicationProviders.

## Evidence
Cause 1 - ListApplicationProviders, fixture sso-admin-application-provider-describe-round-trip, step task#1, expected HTTP 200 / actual HTTP 200 (verdict: mismatch):
  $.NextToken: missing - expected '<shaped-not-asserted>' (a Token: @length 0-2048, @pattern ^[-a-zA-Z0-9+=/_]*$), got None
The recording asserts the token's SHAPE, not a literal string: the requirement is that a response which does not exhaust the catalog carries a NextToken of that shape, and that the token is accepted back as the operation's input NextToken. The concrete token value is per-run and excluded from the finding. Corroborating signal from the same operation in this replay: the emulator's body enumerated ApplicationProviders[0] .. ApplicationProviders[393] in one response, i.e. no page truncation at all.
Smithy: ListApplicationProviders @paginated {inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults, items: ApplicationProviders}; ListApplicationProvidersResponse.NextToken -> Token; ListApplicationProvidersRequest.MaxResults -> integer @range min 1 max 100.

Cause 2 - ListInstances, fixture sso-admin-describe-list-edge-cases-r2, step task#4, expected HTTP 200 / actual HTTP 200 (verdict: mismatch):
  $.Instances[Name="sim2real-wire-idc-74443bb3"].State: missing - expected {'Name': 'ACTIVE'}, got None
The instance name in the JSONPath selector is a per-run generated value; the requirement is that EVERY entry of Instances carries a State structure whose Name equals that instance's status (here ACTIVE, matching the Status member the emulator did emit). No other member of the entry diverged, and no status or error-code divergence is involved.
Smithy evidence that this is recorded behavior only: com.amazonaws.ssoadmin#InstanceMetadata members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions - no State member is modelled, so the recorded response is the authority for its presence and content.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout.
2. Replay the sso-admin wire corpus of trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture sso-admin-application-provider-describe-round-trip, step task#1 - ListApplicationProviders with no MaxResults. Inspect the 200 body: it lists the entire provider catalog and has no NextToken member. Expected: a page of the catalog plus a NextToken of the modelled Token shape, which can be passed back as the next request's NextToken to retrieve the remaining entries.
4. Cause 2: replay fixture sso-admin-describe-list-edge-cases-r2 up to and including step task#4 (the setup steps enable an Identity Center instance whose name is generated per run, then task#4 calls ListInstances). Inspect the 200 body: the entry for the generated instance carries Status "ACTIVE" but no State member. Expected: that entry also carries State: {"Name": "ACTIVE"}, i.e. a State structure whose Name mirrors the instance's status.
