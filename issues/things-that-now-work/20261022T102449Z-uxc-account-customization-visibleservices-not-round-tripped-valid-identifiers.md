Issue-Source: trace-sweep-divergence
Issue-Trace: uxc-configure-visible-services-roundtrip
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: uxc-configure-visible-services-roundtrip uxc-partial-update-preserves-other-fields
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc|UpdateAccountCustomizations|2xx->4xx|$.visibleServices
Issue-Key: uxc|UpdateAccountCustomizations|2xx->4xx|status
Issue-Key: uxc|GetAccountCustomizations|2xx->2xx|$.visibleServices[*]
Issue-Key: uxc|UpdateAccountCustomizations|2xx->2xx|$.visibleServices[*]
Issue-Commit: beffe59d77453c22c05c3018349d1d31ffa3bd08

# Account customization visibleServices not round-tripped: valid identifiers refused, read back null, order rewritten

> _Note: this issue was identified against the code base at commit `beffe59d77453c22c05c3018349d1d31ffa3bd08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountCustomizations

## Description
FOUR causes, all on uxc account-customization read/write. Causes 1-3 are one write path that never happens (the update is refused, so both read-backs report an unconfigured setting); Cause 4 is an independent defect in the order a list setting is reported in. All four must be closed before submitting, and each names its own fixture and step so each can be re-replayed.

Cause 1 of 4: UpdateAccountCustomizations refuses a valid visibleServices with HTTP 400 (fixture uxc-configure-visible-services-roundtrip, step task#3; the cluster also covers the same fixture's task#5).
AWS: Smithy: UpdateAccountCustomizations PATCH /v1/account-customizations, success code 200 (defaulted), @idempotent; input visibleServices targets ServiceList (@length max 500, member Service @length 1..64 @pattern ^[a-z0-9]+(-[a-z0-9]+)*$); modelled errors AccessDeniedException (403), InternalServerException (500), ThrottlingException (429), ValidationException (400). The operation doc states only that visibleServices holds "Amazon Web Services service identifiers" and "for valid service identifiers, call ListServices" -- it states no refusal for an identifier the catalog omits. The recording shows AWS answering HTTP 200 for a request setting visibleServices to a two-element list of ordinary console service identifiers (e.g. ['account-access', 'acm']) and echoing that setting back in $.visibleServices.
Emulator: HTTP 400 with the modelled ValidationException (message "Invalid services provided", fieldList naming visibleServices.<n>.member) and no $.visibleServices member at all, because the handler refuses any identifier that is not a member of its seeded ListServices catalog -- and that seeded catalog is itself a 45-entry endpoint-prefix list that omits most of the identifiers AWS's catalog holds (already filed, open issue 20261022T030944Z-uxc-listservices-returns-wrong-service-catalog-and-validation-message.md, which covers the CATALOG CONTENTS on ListServices only and does NOT carry this step's replay key or this refusal).
Why: a client does exactly what the model tells it to -- enumerate ListServices, then set visibility -- and gets a 400 ValidationException for identifiers AWS accepts. The whole configure workflow terminates at the first write, nothing is persisted, and a caller that retries sees the same refusal; against AWS the same request succeeds with 200.
Fix: the UpdateAccountCustomizations handler's catalog-membership guard (the pre-write filter over input.visible_services in services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs) together with the catalog seed it decides against in src/state.rs; a membership rule the model does not state must not turn a modelled-valid request into a 400.

Cause 2 of 4: GetAccountCustomizations reports the setting as unconfigured after it was configured (fixture uxc-configure-visible-services-roundtrip, step task#4).
AWS: Smithy: GetAccountCustomizations GET /v1/account-customizations, @readonly, success 200; output visibleServices targets ServiceList. The doc states a null visibleServices means the feature was never configured and ALL services are visible. The recording shows HTTP 200 with $.visibleServices = ['account-access', 'acm'] -- the two identifiers the preceding update set.
Emulator: HTTP 200 but $.visibleServices is absent (replay: expected 'account-access'/'acm' at indices 0 and 1, got None), because the write in Cause 1 was refused and the account's record is still the default one.
Why: the response asserts the exact opposite of the account's configuration -- null means "unrestricted, all services visible", so a console client reads back "no restriction" immediately after successfully restricting visibility on AWS. Read-after-write on this setting is unobservable.
Fix: follows from Cause 1 (the state write in the update handler never runs); re-replay this step after Cause 1 to confirm the record persists and the read reports it. Nothing in the GetAccountCustomizations handler itself needs to change if the write lands.

Cause 3 of 4: the same read-back failure after a partial update (fixture uxc-configure-visible-services-roundtrip, step task#6).
AWS: HTTP 200 with $.visibleServices[0] = 'account-access' -- the second update changed another setting and, per the doc "Omitted settings remain unchanged", visibleServices survives.
Emulator: HTTP 200 with $.visibleServices absent (expected 'account-access', got None).
Why/Fix: as Cause 2 -- it is the preservation-across-partial-update read, so it must be re-replayed separately from task#4 to show that an omitted setting is both persisted and preserved.

Cause 4 of 4: UpdateAccountCustomizations rewrites the order of visibleServices (fixture uxc-partial-update-preserves-other-fields, step task#2).
AWS: Smithy: UpdateAccountCustomizations output visibleServices (ServiceList, @length max 500), documented as "The current list of visible service identifiers after the update"; neither the model nor the doc states an ordering, so: recorded behavior only -- the recording shows HTTP 200 with $.visibleServices in the sequence the request listed its members ('s3' then 'ec2').
Emulator: HTTP 200 with the same members in lexicographic ascending order ('ec2' then 's3'): crate::state::canonical_members sorts a list setting's members on write (src/state.rs, set_visible_services/canonical_members), so the stored and reported sequence is the emulator's own rather than the caller's.
Why: a client that compares the returned list with the list it sent, or indexes it positionally (visibleServices[0] as "the first service I made visible"), sees different content than AWS returns for the same request; the ordering of a JSON array is part of the response document.
Hypothesis: the emulator's own source comment (src/state.rs, the canonical_members doc) argues the corpus contains two responses describing the same members in opposite sequences and that per-response order is therefore not reproducible. That claim is not what this step shows -- this step's recorded response lists the members in request order and the emulator's does not -- so a fixer should decide the rule against the corpus (preserving the request sequence reproduces this step) rather than against that comment.
Fix: crate::state::canonical_members / AccountUxCustomization::set_visible_services in src/state.rs -- the state layer's list-setting write, not the handler's validation or the serializer.

Traces: Cause 1 uxc-configure-visible-services-roundtrip#task#3 (cluster also covers #task#5); Cause 2 uxc-configure-visible-services-roundtrip#task#4; Cause 3 uxc-configure-visible-services-roundtrip#task#6; Cause 4 uxc-partial-update-preserves-other-fields#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout: emulator_up true, 9 fixtures replayed, 36 steps, 24 passed, 12 mismatched, 0 errored, 0 blocked, state_reset_failures 0, score 66.7%. Ten divergence clusters; six are owned by the open ListServices issue, the four below are net-new.

Cause 1 -- uxc-configure-visible-services-roundtrip#task#3, UpdateAccountCustomizations, expected status 200 / actual status 400: "status: expected 200, got 400", "$.visibleServices: missing (expected ['account-access', 'acm'], got None)". affected_count 2: the cluster also contains uxc-configure-visible-services-roundtrip#task#5. Emulator source: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs lines 249-262 refuse any visibleServices member for which state::is_catalog_service is false, returning the ValidationException built at lines 143-159; src/state.rs lines 81-127 seed that catalog with 45 endpoint-prefix identifiers that do not include 'account-access', and lines 179-187 decide membership by binary search over it.

Cause 2 -- uxc-configure-visible-services-roundtrip#task#4, GetAccountCustomizations, expected 200 / actual 200: "$.visibleServices[0]: missing (expected 'account-access', got None)", "$.visibleServices[1]: missing (expected 'acm', got None)". Source: src/ops/get_account_customizations.rs lines 168-184 report the stored record, which is still AccountUxCustomization::default() (src/state.rs lines 14-22) because the write was refused.

Cause 3 -- uxc-configure-visible-services-roundtrip#task#6, GetAccountCustomizations, expected 200 / actual 200: "$.visibleServices[0]: missing (expected 'account-access', got None)".

Cause 4 -- uxc-partial-update-preserves-other-fields#task#2, UpdateAccountCustomizations, expected 200 / actual 200, status agrees and only the sequence diverges: "$.visibleServices[0]: value_mismatch (expected 's3', got 'ec2')", "$.visibleServices[1]: value_mismatch (expected 'ec2', got 's3')". Source: src/state.rs lines 27-29 and 66-70 sort a list setting's members on write; src/ops/update_account_customizations.rs lines 272-284 answer from that stored, re-ordered record.

## Steps to Reproduce
1. Clone this repo, boot the emulator built from it, and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture uxc-configure-visible-services-roundtrip, step task#3 -- PATCH /v1/account-customizations with a body setting visibleServices to ['account-access', 'acm']. The emulator answers 400 ValidationException ("Invalid services provided"); the recording answers 200 with $.visibleServices echoing both identifiers. Step task#5 of the same fixture (a second PATCH on the same account) fails the same way.
3. Cause 2: continue that fixture to step task#4 -- GET /v1/account-customizations and read $.visibleServices. The emulator omits the member; the recording returns ['account-access', 'acm'].
4. Cause 3: continue to step task#6 -- GET /v1/account-customizations after the partial update of task#5 and read $.visibleServices[0]. The emulator omits the member; the recording returns 'account-access', i.e. the setting omitted from the second update is preserved.
5. Cause 4: replay fixture uxc-partial-update-preserves-other-fields, step task#2 -- PATCH /v1/account-customizations with visibleServices listed as 's3' then 'ec2' and read $.visibleServices positionally. The emulator returns ['ec2', 's3'] (sorted); the recording returns ['s3', 'ec2'], the request's sequence.
6. All four steps must pass on re-replay before this issue is closed; the six ListServices-catalog clusters in the same run belong to open issue 20261022T030944Z-uxc-listservices-returns-wrong-service-catalog-and-validation-message.md.

## Resolution

Fixed in `services/service-com-amazonaws-uxc-awsaccountuxsetting`: the catalog-membership guard is gone from `src/ops/update_account_customizations.rs` (with the `invalid_services` refusal it built), and `src/state.rs` no longer sorts a list setting on write (`canonical_members` deleted, `is_catalog_service` removed as unused). Two rules, both from the model: the modelled shape constraints are the WHOLE admission rule for `visibleServices` / `visibleRegions` — prose pointing a caller at the operation that enumerates values ("For valid service identifiers, call ListServices") is discovery, not an admission list — and a list setting's value is the caller's document, so the supplied sequence is stored, answered and read back verbatim. Pinned by `tests/all/visible_services_round_trip.rs` (a deterministic generated table built from the model's own `@pattern`s by `wire::legal_service`/`wire::legal_region` and checked against hand-written transcriptions of those patterns, plus a non-catalog-identifier table, a catalog walk and the configure/partial-update workflow) and by the rewritten `tests/all/list_setting_order.rs` (which enumerates all 24 permutations of a four-member list).

- **Cause 1 (a modelled-valid `visibleServices` refused with 400) — FIXED.** Replay keys `uxc|UpdateAccountCustomizations|2xx->4xx|status` and `...|2xx->4xx|$.visibleServices` are both `fixed`; `uxc-configure-visible-services-roundtrip` task#3 and task#5 now answer 200 and echo the setting.
- **Cause 2 (`GetAccountCustomizations` reports the setting unconfigured) — FIXED.** Key `uxc|GetAccountCustomizations|2xx->2xx|$.visibleServices[*]` is `fixed` (task#4).
- **Cause 3 (same read-back after a partial update) — FIXED.** Same key, same fixture, task#6 passes: an omitted setting is persisted and preserved.
- **Cause 4 (`uxc-partial-update-preserves-other-fields#task#2`, the reported sequence) — DECLINED as per-response variation.** Decided against the corpus, as this issue asks, by probing the write rule: storing the supplied sequence makes that fixture's later `GetAccountCustomizations` (task#4) agree and task#2 differ; storing it REVERSED makes task#2 agree and task#4 differ, at the same two indices. So the fixture records the same two members in opposite sequences in two responses with no write between them, and no deterministic rule satisfies both — only keying the write on one recorded response would. Every other recorded response for this setting (both update echoes and both reads of `uxc-configure-visible-services-roundtrip`) agrees with the supplied sequence, so that is the rule the emulator keeps; note the earlier close of 20261022T030944Z-uxc-updateaccountcustomizations-diverges-on-list-order-and-invalid-member.md by SORTING never reproduced this step either (sorted order equals the reverse for these two members). The key `uxc|UpdateAccountCustomizations|2xx->2xx|$.visibleServices[*]` therefore still mismatches on that one step's sequence.
- The `$.services[*]` divergences in the same replay remain with open issue 20261022T030944Z-uxc-listservices-returns-wrong-service-catalog-and-validation-message.md; this change does not touch `ListServices` or the catalog seed, and a caller is no longer blocked by them, since an identifier the catalog omits is now accepted.
