Issue-Source: trace-sweep-divergence
Issue-Trace: endusermessaging-brand-profile-deletion-protection
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: endusermessaging-brand-profile-deletion-protection endusermessaging-brand-profile-idempotent-create endusermessaging-brand-profile-lifecycle
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging|CreateBrandProfile|2xx->2xx|$.attributesCreated
Issue-Key: endusermessaging|CreateBrandProfile|2xx->2xx|$.createdAt
Issue-Key: endusermessaging|CreateBrandProfile|2xx->2xx|$.updatedAt
Issue-Key: endusermessaging|UpdateBrandProfile|2xx->2xx|$.createdAt
Issue-Key: endusermessaging|UpdateBrandProfile|2xx->2xx|$.updatedAt
Issue-Key: endusermessaging|GetBrandProfile|2xx->2xx|$.createdAt
Issue-Key: endusermessaging|GetBrandProfile|2xx->2xx|$.updatedAt
Issue-Commit: e5b2e36fc94a41937ab22fe33196324420972c76

# endusermessaging brand-profile success bodies diverge: nanosecond-precision epoch timestamps and an empty default attribute template

> _Note: this issue was identified against the code base at commit `e5b2e36fc94a41937ab22fe33196324420972c76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBrandProfile

## Description
Two root causes in the endusermessaging brand-profile success (200) projection, both observed while replaying the wire corpus simparity-wire-aws-v1 against the running emulator (the service is bootstrapped and answering). The only open endusermessaging issues are the two scaffold-stub issues (CreateBrandProfileAttributes, CreateNotifyCodeConfiguration), which cover other operations, and the behavioral issue '20261104T173700Z-endusermessaging-brand-profiles-2-net-new-causes-createbrandprofile.md'. That behavioral issue describes the attributesCreated gap from a behavioral check but carries NO wire replay key (Issue-Source is the behavioral corpus, with no Issue-Trace/Issue-Step), so the wire steps below are not covered by it and the replay report reports them unowned; Cause 1 here is not described anywhere.

=== Cause 1 of 2: every modelled Timestamp output member is serialized as an epoch-seconds double carrying sub-millisecond (nanosecond) resolution, so createdAt/updatedAt do not have the shape AWS's responses carry ===
AWS: Smithy model com.amazonaws.endusermessaging#EndUserMessaging, protocol restJson1 (service_info: protocols [restJson1], no protocol modifiers). CreateBrandProfile/GetBrandProfile/UpdateBrandProfile each declare output members createdAt and updatedAt targeting smithy.api#Timestamp, both @required, with no @timestampFormat override, so they are carried in the JSON body in the protocol's default epoch-seconds form. The recorded responses do not assert the clock value (it differs per run) but do assert the member's shape; AWS's epoch-seconds timestamps are emitted at the resolution the service reports (recorded behavior only: at most millisecond fractional precision), never at nanosecond resolution. Hypothesis: the shape assertion is what a 7-fractional-digit double fails.
Emulator: every timestamp output on this surface is built by the service's shared helper, which deliberately preserves the full resolution of SystemTime::now() and emits values such as 1793816917.8705125 and 1793816919.0737119 (seven fractional digits). The replay flags createdAt and updatedAt on CreateBrandProfile (expected '<shaped-not-asserted>', got 1793816917.8705125), on UpdateBrandProfile (createdAt 1793816917.8705125, updatedAt 1793816917.881114) and on GetBrandProfile (both 1793816919.0737119) — i.e. on every operation that projects a brand profile, which is why this is one serialization cause rather than three per-operation ones.
Why: a client that round-trips, hashes, compares or persists these timestamps, or that renders them with a fixed-width formatter, sees a value whose precision AWS never produces; equality against a value fetched back from the same resource and anything that reformats the timestamp (e.g. to AWS's millisecond granularity) diverges from the real service, and conditional/If-Unmodified style comparisons on sub-millisecond digits behave differently than against AWS.
Fix: the service's timestamp projection — the shared Timestamp helper in the brand-profile state module used by the Create/Get/Update brand-profile output builders — must emit epoch-seconds at AWS's resolution rather than carrying the host clock's nanoseconds into the wire value.
Traces: endusermessaging-brand-profile-deletion-protection#task#1 (CreateBrandProfile, also covers the clustered idempotent-create and lifecycle creates), endusermessaging-brand-profile-deletion-protection#task#3 (UpdateBrandProfile), endusermessaging-brand-profile-lifecycle#task#2 (GetBrandProfile).

=== Cause 2 of 2: CreateBrandProfile seeds no default attribute template, so the @required attributesCreated is always 0 ===
AWS: Smithy model CreateBrandProfile output member attributesCreated (Integer, @required) reports the number of default attributes created for the new profile. The recorded response for a plain create carries a positive count (recorded behavior only: 25); the requirement is that the count is positive and equals the number of attributes the profile actually has after create (the ones ListBrandProfileAttributes then pages), not one recording's literal 25.
Emulator: the CreateBrandProfile handler mints the profile with an empty attribute map and derives the count from it, so attributesCreated is 0 on every create — the replay reports '$.attributesCreated: value_mismatch (expected 25, got 0)' on four clustered create steps.
Why: a client that reads attributesCreated to learn how many attributes it must fetch, reconcile or page sees 0 and concludes the profile carries no attribute template; any subsequent read or update of a default attribute then fails against an attribute AWS would have seeded.
Fix: CreateBrandProfile's handler and the brand-profile state — the create path must seed the service's default attribute template into the profile's attribute map and report the number actually seeded.
Traces: endusermessaging-brand-profile-deletion-protection#task#1, plus the clustered endusermessaging-brand-profile-idempotent-create#task#1, endusermessaging-brand-profile-idempotent-create#task#3, endusermessaging-brand-profile-lifecycle#task#1.

## Evidence
Cause 1 — fixture endusermessaging-brand-profile-deletion-protection step task#1 (CreateBrandProfile, 200 vs 200):
  $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1793816917.8705125)
  $.updatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1793816917.8705125)
Same cause, fixture endusermessaging-brand-profile-deletion-protection step task#3 (UpdateBrandProfile, 200 vs 200):
  $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1793816917.8705125)
  $.updatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1793816917.881114)
Same cause, fixture endusermessaging-brand-profile-lifecycle step task#2 (GetBrandProfile, 200 vs 200):
  $.createdAt / $.updatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1793816919.0737119)
  The quoted epoch values are this run's clock and are NOT the requirement; the divergence is that the emulator serializes seven fractional digits of epoch seconds for every Timestamp member.
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/state.rs:200-211 — timestamp(SystemTime) is built 'from whole seconds plus sub-second nanoseconds so a stored instant keeps the resolution SystemTime::now() gave it'; services/.../src/ops/create_brand_profile.rs:255-256 projects created_at/updated_at through it.

Cause 2 — same fixture/step (CreateBrandProfile, 200 vs 200):
  $.attributesCreated: value_mismatch (expected 25, got 0)
  Clustered members: endusermessaging-brand-profile-idempotent-create#task#1, endusermessaging-brand-profile-idempotent-create#task#3, endusermessaging-brand-profile-lifecycle#task#1 (same divergence_key service=endusermessaging|CreateBrandProfile|status=200->200).
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/create_brand_profile.rs:235-236 ('This call creates no default attributes; `attributesCreated` counts this map.' / attributes: BTreeMap::new()) and :257 (attributes_created from profile.attributes.len()).

## Steps to Reproduce
Cause 1:
1. POST /v1/brand-profiles with {"brandProfileName": "<any name matching ^[A-Za-z0-9_ -]*[A-Za-z0-9_-][A-Za-z0-9_ -]*$"}.
2. Read createdAt/updatedAt from the 200 body: the emulator returns an epoch-seconds double with seven fractional digits (e.g. 1793816917.8705125).
3. GET /v1/brand-profiles/{brandProfileId} and PUT /v1/brand-profiles/{brandProfileId} with {"deletionProtectionEnabled": false} — the same nanosecond-resolution values appear in both bodies.

Cause 2:
1. POST /v1/brand-profiles with any valid brandProfileName.
2. Read attributesCreated from the 200 body — the emulator reports 0; AWS reports the positive number of seeded default attributes (25 in the recording).

Replay: replay the wire corpus simparity-wire-aws-v1; fixtures endusermessaging-brand-profile-deletion-protection (task#1, task#3) and endusermessaging-brand-profile-lifecycle (task#2).
