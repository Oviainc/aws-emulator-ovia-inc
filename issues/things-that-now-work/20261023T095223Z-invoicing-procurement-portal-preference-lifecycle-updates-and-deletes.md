Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-procurement-portal-preference-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-procurement-portal-preference-lifecycle
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|GetProcurementPortalPreference|2xx->2xx|$.ProcurementPortalPreference.Contacts[*]
Issue-Key: invoicing|GetProcurementPortalPreference|2xx->2xx|$.ProcurementPortalPreference.PurchaseOrderRetrievalEnabled
Issue-Key: invoicing|GetProcurementPortalPreference|2xx->2xx|$.ProcurementPortalPreference.Version
Issue-Key: invoicing|SendProcurementPortalValidation|4xx->2xx|$.Message
Issue-Key: invoicing|SendProcurementPortalValidation|4xx->2xx|$.__type
Issue-Key: invoicing|SendProcurementPortalValidation|4xx->2xx|status
Issue-Key: invoicing|GetProcurementPortalPreference|4xx->2xx|$.Message
Issue-Key: invoicing|GetProcurementPortalPreference|4xx->2xx|$.__type
Issue-Key: invoicing|GetProcurementPortalPreference|4xx->2xx|status
Issue-Commit: 3113486ab2df4ef97dea8b3047048aff3439051f

# Invoicing procurement-portal preference lifecycle: updates and deletes never take effect, and validation send cannot fail

> _Note: this issue was identified against the code base at commit `3113486ab2df4ef97dea8b3047048aff3439051f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProcurementPortalPreference

## Description
THREE causes sharing one fault: on the invoicing procurement-portal-preference plane the state a reader reports never moves past what the create wrote, and the validation flow can only succeed. Every read answers the created preference forever — after it was updated, and after it was deleted — and SendProcurementPortalValidation reports success where AWS refuses.

Note on related auto-filed work: PutProcurementPortalPreference and DeleteProcurementPortalPreference are both unimplemented scaffolds that answer 501 (put_procurement_portal_preference.rs:204 and delete_procurement_portal_preference.rs:181 both `Err(Scaffolded::not_implemented(..))`), and the replay filed those stub steps separately. The three causes below are the READ-SIDE and send-side divergences those stubs leave behind, and they are not closed by merely un-stubbing the two writes: the reader has to report the new version, the reader has to stop reporting a deleted preference, and the send path has to be able to refuse. Confirm each by re-replaying the fixture steps named per cause.

Cause 1 of 3: GetProcurementPortalPreference reports the pre-update configuration and Version 1 after the preference was updated.
AWS: Smithy: `ProcurementPortalPreference.Version` is a required Long documented as "the version number of the procurement portal preference configuration", `LastUpdateDate` is required, and `Contacts` / `PurchaseOrderRetrievalEnabled` are the members `PutProcurementPortalPreference` takes (`Contacts` required, `PurchaseOrderRetrievalEnabled` required Boolean) — smithy_query operation=PutProcurementPortalPreference, shape=ProcurementPortalPreference. A successful Put therefore makes the next Get report the submitted Contacts and PurchaseOrderRetrievalEnabled at the next Version. The recording shows Version 2, Contacts[0] = the updated contact, PurchaseOrderRetrievalEnabled = false.
Emulator: the Get after the update still answers 200 with Version 1, the create-time Contacts[0] and PurchaseOrderRetrievalEnabled = true. The read handler itself is faithful to state (get_procurement_portal_preference.rs:167-180 returns `stored.preference.clone()`), so the stale answer is that nothing ever rewrote `stored.preference` — the update path is the 501 stub.
Why: a client that writes a configuration and reads it back sees its own write silently discarded, and any flow gated on the Version advancing (optimistic re-read, "did my change land") loops or proceeds on stale settings.
Fix: PutProcurementPortalPreference's handler and the stored preference in the service's state — the write must replace the stored modeled structure, bump `Version` and set `LastUpdateDate`, so the existing read reports it.
Traces: invoicing-procurement-portal-preference-lifecycle#task#5 (and the later read at #task#9 in the same cluster).

Cause 2 of 3: GetProcurementPortalPreference answers 200 for a preference that was deleted.
AWS: after `DeleteProcurementPortalPreference` succeeds, a Get on that ARN is 404 `ResourceNotFoundException` (modeled error of GetProcurementPortalPreference, httpError 404 — smithy_query operation=GetProcurementPortalPreference). Recorded message example: "Procurement portal preference with arn arn:aws:invoicing::000000000000:procurement-portal-preference/000000000001 does not exist".
Emulator: 200 with the full preference body — `$.__type` and `$.Message` absent because it is a success response, status 404 expected, 200 returned. The preference is still in `procurement_portal_preferences` because the delete never removed it (501 stub).
Why: this is the worst of the three — a client is told a resource it just deleted still exists and is still configured, so cleanup verification passes against a live resource and a recreate path takes the wrong branch.
Fix: DeleteProcurementPortalPreference's handler and the service's state store (remove the entry, so the existing read's not-found branch fires).
Traces: invoicing-procurement-portal-preference-lifecycle#teardown#15.

Cause 3 of 3: SendProcurementPortalValidation always answers 200, never the refusal AWS gives when test-e-invoice delivery cannot be confirmed.
AWS: recorded behavior only — at this point in the lifecycle the recorded response is 400 with `__type` `ValidationException` and a Message explaining that delivery of the test e-invoice could not be confirmed and that the OTP may have been rejected by the portal's business-rule validation (recorded example text: "We could not confirm delivery of the test e-invoice. The OTP may have been sent but rejected by your procurement portal's business-rule validation. Check your portal's rejected-invoice log for the OTP ... If the OTP is not there, verify your portal configuration or contact support."). The model declares `ValidationException` (httpError 400) for this operation, so the refusal is modeled. The requirement is the 400 + `ValidationException`; the prose is an example. Hypothesis: AWS attempts an actual delivery to the configured portal endpoint and reports this 400 when it cannot confirm the OTP was delivered, which is the outcome for a preference whose portal endpoint is not a reachable portal.
Emulator: 200 with the ARN echoed back. send_procurement_portal_validation.rs:202-228 unconditionally mints a code, stores it on the preference and succeeds — there is no path on which this operation refuses, so the recorded delivery-unconfirmed branch is unreachable.
Why: a client exercising the OTP flow never sees the failure mode AWS has, so error handling for "the OTP never arrived" is never exercised; worse, the emulator hands back a code and claims delivery that did not happen, so the subsequent verify succeeds where against AWS it could not.
Fix: the SendProcurementPortalValidation handler — it needs the branch that refuses with the modeled 400 `ValidationException` when delivery of the test e-invoice cannot be confirmed for the stored preference, instead of always issuing a code.
Traces: invoicing-procurement-portal-preference-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, so these are not cross-fixture state bleed):

Cause 1 — invoicing-procurement-portal-preference-lifecycle#task#5, GetProcurementPortalPreference, verdict=mismatch, expected 200 / actual 200, affected_count=2 (task#5, task#9):
  "$.ProcurementPortalPreference.Contacts[0]: expected {'Email': 'updated@example.com', 'Name': 'Updated Contact'}, got {'Name': 'Initial Contact', 'Email': 'initial@example.com'}"
  "$.ProcurementPortalPreference.PurchaseOrderRetrievalEnabled: value_mismatch (expected False, got True)"
  "$.ProcurementPortalPreference.Version: value_mismatch (expected 2, got 1)"

Cause 2 — invoicing-procurement-portal-preference-lifecycle#teardown#15, GetProcurementPortalPreference, verdict=mismatch, expected 404 / actual 200:
  "status: expected 404, got 200"
  "$.__type: missing (expected 'ResourceNotFoundException', got None)"
  "$.Message: missing (expected 'Procurement portal preference with arn arn:aws:invoicing::000000000000:procurement-portal-preference/000000000001 does not exist', got None)"

Cause 3 — invoicing-procurement-portal-preference-lifecycle#task#6, SendProcurementPortalValidation, verdict=mismatch, expected 400 / actual 200:
  "status: expected 400, got 200"
  "$.__type: missing (expected 'ValidationException', got None)"
  "$.Message: missing (expected \"We could not confirm delivery of the test e-invoice. ...\", got None)"

Code, as checked out:
- services/service-com-amazonaws-invoicing-invoicing/src/ops/put_procurement_portal_preference.rs:187-205 — handler body is the scaffold, `Err(crate::scaffolded::Scaffolded::not_implemented("PutProcurementPortalPreference"))`, so no update is ever persisted.
- services/service-com-amazonaws-invoicing-invoicing/src/ops/delete_procurement_portal_preference.rs:164-182 — same scaffold, `not_implemented("DeleteProcurementPortalPreference")`, so the preference is never removed.
- services/service-com-amazonaws-invoicing-invoicing/src/ops/get_procurement_portal_preference.rs:160-181 — returns `stored.preference.clone()` and 404s only when the ARN is absent from `procurement_portal_preferences`, which it never becomes.
- services/service-com-amazonaws-invoicing-invoicing/src/ops/send_procurement_portal_validation.rs:202-228 — issues a code and returns Ok on every call; the only refusal it can produce is the 404 for an unknown ARN.
- Model: smithy_query(query="operation", operation="PutProcurementPortalPreference,GetProcurementPortalPreference,SendProcurementPortalValidation") and query="shape", shape="ProcurementPortalPreference" against services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: CreateProcurementPortalPreference, then PutProcurementPortalPreference on the returned ARN with a different Contacts entry and PurchaseOrderRetrievalEnabled=false, then GetProcurementPortalPreference on that ARN. Observe Version 1, the create-time Contacts and PurchaseOrderRetrievalEnabled=true (the Put itself answers 501). AWS reports Version 2 and the submitted values. Replay fixture invoicing-procurement-portal-preference-lifecycle step task#5 (and task#9).
3. Cause 2: with that preference created, call DeleteProcurementPortalPreference on its ARN and then GetProcurementPortalPreference on the same ARN. Observe HTTP 200 with the full preference; AWS answers 404 `ResourceNotFoundException`. Replay fixture invoicing-procurement-portal-preference-lifecycle step teardown#15.
4. Cause 3: at the lifecycle point where the recording refuses, call SendProcurementPortalValidation on the preference ARN. Observe 200 with the ARN echoed and a code minted into state; AWS answers 400 `ValidationException` reporting that test-e-invoice delivery could not be confirmed. Replay fixture invoicing-procurement-portal-preference-lifecycle step task#6.
