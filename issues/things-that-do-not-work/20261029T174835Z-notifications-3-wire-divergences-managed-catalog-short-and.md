Issue-Source: trace-sweep-divergence
Issue-Trace: notifications-catalog-list-managed-notification-configurations
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notifications-catalog-list-managed-notification-configurations notifications-tag-resource-list-tags-untag-roundtrip
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn
Issue-Key: notifications|TagResource|2xx->2xx|status
Issue-Key: notifications|UntagResource|2xx->2xx|status
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# notifications: 3 wire divergences — managed catalog short and tag mutations answer 200 not 204

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedNotificationConfigurations

## Description
Replaying the notifications wire corpus against the emulator produced 3 mismatches, all in this service. They are THREE separate causes and all three must be closed:

Cause 1 of 3: ListManagedNotificationConfigurations publishes a catalog that is missing most of the managed notification configurations AWS publishes (fixture notifications-catalog-list-managed-notification-configurations, step task#1).
AWS: Smithy: ListManagedNotificationConfigurations GET /managed-notification-configurations, output member managedNotificationConfigurations (@required, list of ManagedNotificationConfigurationStructure with @required arn/name/description; arn pattern ^arn:[a-z-]{3,10}:notifications::[0-9]{12}:managed-notification-configuration/category/[a-zA-Z0-9\-]{3,64}/sub-category/[a-zA-Z0-9\-]{3,64}$). The model does NOT enumerate the catalog, so the published set is recorded behavior only. The recording shows AWS publishing managed configurations for categories beyond AWS-Health — among them AWS-Partner-Central sub-categories PCProductListings, PCPaymentsAndDisbursements and PCOffersAndAgreements — each carrying a human-readable display `name` ("Product listings", "Payments and disbursements", "Offers and Agreements") and a prose `description` of the events that category covers, both distinct from the arn's own category/sub-category tokens.
Emulator: the handler reports whatever the store was seeded with, and the seed is a four-entry AWS-Health-only catalog (category AWS-Health, sub-categories Issue, AccountNotification, ScheduledChange, Investigation) whose `name` is synthesized as "{category}-{sub_category}". The replay reports every other expected entry as missing (expected the structure, got None) — e.g. $.managedNotificationConfigurations[arn="...managed-notification-configuration/category/AWS-Partner-Central/sub-category/PCProductListings"]: missing. The status matches (200); only the body is short.
Why: a client enumerating the account's managed notifications sees only AWS-Health. Every managed configuration AWS publishes for other services (Partner Central, etc.) is undiscoverable, so a caller cannot read it with GetManagedNotificationConfiguration or associate a channel with it, and a reconciliation loop that diffs the catalog concludes those categories do not exist. The synthesized name scheme also means a UI that renders `name` shows an arn token instead of the display name AWS returns.
Fix: the service's seeded managed-notification catalog in the crate's state (the constant the store builds its managed_configurations map from), which must publish the full catalog AWS serves with each entry's own display name and description, rather than deriving name from the arn segments. The listing handler itself (ordering, channelIdentifier filter, paging) is not at fault.
Traces: notifications-catalog-list-managed-notification-configurations#task#1; see services/service-com-amazonaws-notifications-notifications/src/state.rs (MANAGED_NOTIFICATION_CATALOG, ~line 546; seeded_managed_configurations, ~line 613) and src/ops/list_managed_notification_configurations.rs.

Cause 2 of 3: TagResource answers HTTP 200 where AWS answers HTTP 204 (fixture notifications-tag-resource-list-tags-untag-roundtrip, step task#5).
AWS: Smithy: TagResource POST /tags/{arn}, output shape TagResourceResponse has NO members and the model carries no explicit @http code (code is defaulted), so the success status is recorded behavior only: the recording shows 204 with an empty body for a successful tag write.
Emulator: returns 204 -> 200 (replay detail "status: expected 204, got 200") with the same empty body.
Why: a client or proxy that branches on the success status — treating 204 as "no content, nothing to parse" and 200 as "a body is present" — mis-handles the response; conformance and contract tests asserting the documented 204 fail against the emulator even though the mutation itself succeeded.
Fix: the operation's success-status declaration in its handler module (const SUCCESS_STATUS, currently 200u16, in src/ops/tag_resource.rs ~line 140) so the empty-output success serializes as 204.
Traces: notifications-tag-resource-list-tags-untag-roundtrip#task#5.

Cause 3 of 3: UntagResource answers HTTP 200 where AWS answers HTTP 204 (same fixture, step task#7).
AWS: Smithy: UntagResource DELETE /tags/{arn}, output shape UntagResourceResponse has NO members and its @http code is defaulted, so again recorded behavior only: the recording shows 204 with an empty body for a successful untag.
Emulator: returns 204 -> 200 (replay detail "status: expected 204, got 200").
Why: same as cause 2 — the status is part of the response contract for an empty-bodied mutation, and a client that distinguishes 200 from 204 diverges.
Fix: the operation's success-status declaration in src/ops/untag_resource.rs (const SUCCESS_STATUS), matching the fix in cause 2; the two share the fault but are separate declarations and both must change.
Traces: notifications-tag-resource-list-tags-untag-roundtrip#task#7.

No open issue covers these. The nearest neighbours are all resolved ("notifications: 4 fidelity divergences — pattern-validation message rendering, new-configuration status, TagResource quota message, hub not-found message" and the ListManagedNotificationConfigurations / TagResource stub issues), and a resolved issue covers nothing; the open notificationscontacts issues are a different service.

## Evidence
Replay of trace set simparity-wire-aws-v1: 12 steps, 9 passed, 3 mismatched, score 75.0%, emulator_up=true, bootstrap_signal=false.

1) fixture notifications-catalog-list-managed-notification-configurations step task#1, ListManagedNotificationConfigurations, verdict mismatch, expected_status 200 / actual_status 200. Detail (truncated by the reporter):
   $.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-Partner-Central/sub-category/PCProductListings"]: missing (expected {'arn': '...PCProductListings', 'description': 'Marketplace events related to your published AMI, container, and SaaS product catalog, including listing change set status, vulnerability scan findings, open product tasks, and Vendor Insights security profile snapshots.', 'name': 'Product listings'}, got None)
   $.managedNotificationConfigurations[arn="...sub-category/PCPaymentsAndDisbursements"]: missing (expected name 'Payments and disbursements', got None)
   $.managedNotificationConfigurations[arn="...sub-category/PCOffersAndAgreements"]: missing (expected name 'Offers and Agreements', got None)
   ... further entries missing.
   Emulator-side cause: MANAGED_NOTIFICATION_CATALOG in src/state.rs holds exactly 4 tuples, all category "AWS-Health", and names each entry format!("{category}-{sub_category}").

2) fixture notifications-tag-resource-list-tags-untag-roundtrip step task#5, TagResource, verdict mismatch: "status: expected 204, got 200". Model: TagResourceResponse has no members; http {method POST, uri /tags/{arn}, code 200, codeDefaulted true} -> emulator const SUCCESS_STATUS = 200u16.

3) fixture notifications-tag-resource-list-tags-untag-roundtrip step task#7, UntagResource, verdict mismatch: "status: expected 204, got 200". Model: UntagResourceResponse has no members; http {method DELETE, uri /tags/{arn}, code 200, codeDefaulted true}.

None of the three carries a stub_path or an owned_by, so none is already filed. The affected clusters are single-step (affected_count 1 each).

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay fixture notifications-catalog-list-managed-notification-configurations: step task#1 issues GET /managed-notification-configurations with no maxResults. Expect the managed catalog AWS publishes (including category AWS-Partner-Central sub-categories PCProductListings, PCPaymentsAndDisbursements, PCOffersAndAgreements with their display names and descriptions); observe only the four AWS-Health entries, each named "AWS-Health-<SubCategory>", so every other expected arn is absent from managedNotificationConfigurations.
3. Replay fixture notifications-tag-resource-list-tags-untag-roundtrip. Step task#5 POSTs /tags/{configurationArn} with a tag map: expect HTTP 204 with an empty body, observe HTTP 200.
4. Step task#7 of the same fixture DELETEs /tags/{configurationArn}?tagKeys=...: expect HTTP 204 with an empty body, observe HTTP 200.
5. Re-replay both fixtures after the fix; all 12 steps of the corpus must pass.
