Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-catalog-list-schemas
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-catalog-list-schemas
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListSchemas|2xx->2xx|$.Schemas[*]
Issue-Commit: 81630e6703d155728070b64f1f7397104fda7330

# ListSchemas serves a fabricated AWS-managed catalogue: digest-derived LastModified and a wrong entry set

> _Note: this issue was identified against the code base at commit `81630e6703d155728070b64f1f7397104fda7330`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas

## Description
Paging the service-owned (AWS-managed) schema registry with ListSchemas answers 200 with a body whose `Schemas` list does not match the catalogue AWS publishes. Every one of the 51 net-new divergence clusters on fixture `schemas-catalog-list-schemas` (221 replayed steps) is an instance of the TWO causes below; both live in the catalogue seed, not in the pagination codec, so they are filed together. This is NOT the already-open `20261029T113448Z-schemas-listschemas-pagination.md` (that issue is about `decode_next_token` accepting NextToken spellings `encode_next_token` never mints) and NOT `20261029T203553Z-schemas-aws-managed-schema-content-is-wrong-for-both.md` (that one is about the `Content` document DescribeSchema/ExportSchema return). Neither of those changes which entries ListSchemas lists or what `LastModified` it reports.

Cause 1 of 2: `SchemaSummary.LastModified` of every AWS-managed catalogue entry is fabricated from a digest of the schema name instead of the entry's published instant.
AWS: Smithy: `ListSchemas` -> `__listOfSchemaSummary` of `SchemaSummary`; member `LastModified` targets `__timestampIso8601` with `@timestampFormat("date-time")` and is documented "The date and time that schema was modified." The model does not state the value, so this is recorded behavior only: across the recording, every entry of the AWS-managed catalogue reports a publication instant drawn from the small set of instants at which the catalogue was published — the overwhelming majority fall inside one narrow publication window (whole-second `date-time` values a few minutes apart), with a handful of entries carrying their own older publication instant. Entries published together report the SAME instant; the instant is a property of the published schema, identical on every request.
Emulator: for any name absent from its own `MANAGED_PUBLICATION_INSTANTS` seed table, it computes `SHA-256(schema_name)`, takes the first 8 bytes modulo a six-year span and adds that to a catalogue epoch — so each entry gets its own pseudo-random instant scattered uniformly over a six-year window. Replay shows, for the same `SchemaName`, an expected instant inside the catalogue's publication window against an emulator instant years earlier and unrelated to any sibling entry's; this diverges for essentially every entry of every page (clusters at `task#1`..`task#206`).
Why: `LastModified` is the field a client compares against its cached copy of a schema to decide whether to re-fetch. A client sees schemas of one source as having been modified at a dozen unrelated moments years apart, so cache-staleness logic, "changed since" filtering and any ordering or reporting by modification time all come out wrong — and the values bear no relation to the ones the same call returns against AWS.
Fix: the AWS-managed catalogue seed in the schemas service's state module — `managed_last_modified` and the per-entry seed data `MANAGED_PUBLICATION_INSTANTS` it falls back from. The digest-derived fallback must go: an entry's instant is catalogue DATA and has to be seeded per entry (and shared by entries published together), not synthesized from the name. Nothing in the ListSchemas handler or the serializer needs to change.
Traces: `schemas-catalog-list-schemas#task#1` (representative), and `#task#2`, `#task#3`, `#task#4`, `#task#5`, `#task#6`, `#task#206`.

Cause 2 of 2: the catalogue's entry SET is wrong — generic CloudTrail/ServiceEvent schemas are synthesized for sources that publish none, and a case-variant duplicate entry is seeded — so pages carry the wrong schemas and the page boundaries land in the wrong place.
AWS: recorded behavior only (`ListSchemas` is `@paginated` with `inputToken=NextToken`/`outputToken=NextToken`/`items=Schemas`/`pageSize=Limit`, and the registry's entry set is service data the model does not describe). The recording shows the AWS-managed registry publishes the six generic `AWSAPICallviaCloudTrail` / `AWSConsoleActionviaCloudTrail` / `AWSConsoleSignInviaCloudTrail` / `AWSInsightviaCloudTrail` / `AWSNetworkActivityEventviaCloudTrail` / `AWSServiceEventviaCloudTrail` schemas only for SOME sources: `aws.a4b`, `aws.abuse`, `aws.account` and the like have them, while e.g. `aws.aidevops` publishes only its `Investigation*` event schemas and `aws.application-autoscaling` only `ApplicationAutoScalingScalingActivityStateChange`. The catalogue also publishes exactly one spelling of a given event name per source. Because entries come back in name order, the entry set fixes both each page's contents and where each page ends.
Emulator: it synthesizes the six generic schemas for every source in its seed, so sources that publish none of them gain six extra entries that sort ahead of the source's real ones, and it seeds a case-variant duplicate (`...@AWSAPICallViaCloudTrail`, capital V) alongside the published `...@AWSAPICallviaCloudTrail` for a source. Replay: at `task#10` the page is expected to open with that source's `Investigation*` entries but opens with six synthesized generic CloudTrail entries; at `task#22` the page is expected to continue into the next source's entries but is still emitting the previous source's synthesized generic entries; at `task#32` the expected entries for a source are displaced and the emulator emits a different source's entries plus the capital-V case variant. Each insertion shifts every following page, so the divergence is not confined to the affected sources.
Why: a client enumerating the AWS-managed registry gets schemas AWS does not publish, misses or mis-orders the ones it does, and its `NextToken` walk visits a different sequence of pages than against AWS — so code generated or validated from the enumeration is built against the wrong schema list, and a paged crawl can both see duplicates and skip real entries.
Fix: the AWS-managed catalogue seed data in the schemas service's state module — `aws_managed_event_types` / `aws_managed_schemas` and the registries `aws_managed_registries` seeds from them. The generic CloudTrail/ServiceEvent set must be seeded per source rather than fanned out over all sources, and the case-variant duplicate removed so one event name per source is published.
Traces: `schemas-catalog-list-schemas#task#10`, `#task#22`, `#task#32`.

## Evidence
Fixture `schemas-catalog-list-schemas`, operation ListSchemas, expected status 200 / actual status 200 — 51 net-new divergence clusters covering 221 steps, none carrying `stub_path` or `owned_by`.

Cause 1 — `$.Schemas[i]` differs only in `LastModified` for an identically named entry:
- `task#1`: `$.Schemas[0]` expected `{'SchemaName': 'aws.a4b@AWSAPICallviaCloudTrail', 'LastModified': '2026-07-08T18:12:15Z', 'VersionCount': 1, 'tags': {}}`, got `{'SchemaName': 'aws.a4b@AWSAPICallviaCloudTrail', 'LastModified': '2020-11-26T01:30:32Z', 'VersionCount': 1, 'tags': {}}`.
- `task#2`: `aws.abuse@AWSConsoleSignInviaCloudTrail` expected `2026-07-08T18:06:55Z`, got `2021-08-29T02:58:21Z`.
- `task#4`: `aws.account@AWSInsightviaCloudTrail` expected `2026-07-08T18:25:05Z`, got `2021-11-06T05:19:47Z`.
- `task#6`: `aws.acm@ACMCertificateApproachingExpiration` expected `2026-07-08T18:19:48Z`, got `2020-12-10T08:41:55Z`.
- `task#206`: `aws.waf-regional@AWSAPICallviaCloudTrail` expected `2026-07-08T18:12:16Z`, got `2023-09-28T12:07:33Z`.
The expected instants of entries published together coincide (many distinct names share `2026-07-08T18:12:15Z`), while the emulator's are uniformly scattered across 2019..2025 — the signature of a name digest modulo a six-year span. Source: `managed_last_modified` falls back to `SHA-256(schema_name)[..8] % CATALOGUE_SPAN_SECONDS` added to `CATALOGUE_EPOCH_SECONDS` whenever the name is not in `MANAGED_PUBLICATION_INSTANTS`.

Cause 2 — `$.Schemas[i]` carries a different `SchemaName` than expected at the same index:
- `task#10`: `$.Schemas[0]` expected `aws.aidevops@InvestigationCancelled` (VersionCount 2), got `aws.aidevops@AWSAPICallviaCloudTrail` (VersionCount 1); `$.Schemas[1]` expected `aws.aidevops@InvestigationCompleted`, got `aws.aidevops@AWSConsoleActionviaCloudTrail`; the whole page is displaced by six synthesized generic entries.
- `task#22`: `$.Schemas[1]` expected `aws.application-insights@AWSAPICallviaCloudTrail`, got `aws.application-autoscaling@AWSConsoleActionviaCloudTrail` — the page should have moved on to the next source but is still emitting synthesized entries for the previous one.
- `task#32`: `$.Schemas[1]` expected `aws.autoscaling@AWSAPICallviaCloudTrail`, got `aws.autoscaling-plans@AWSConsoleActionviaCloudTrail`; `$.Schemas[6]` expected `aws.autoscaling@AWSServiceEventviaCloudTrail`, got `aws.autoscaling@AWSAPICallViaCloudTrail` (capital V) with `LastModified 2019-12-01T00:59:43Z` — exactly the case-variant entry seeded in `MANAGED_PUBLICATION_INSTANTS`.

Replay summary for the run: steps_total 289, steps_passed 66, steps_mismatched 223, score_pct 22.8, emulator_up true, bootstrap_signal false.

## Steps to Reproduce
1. Boot the emulator built from this checkout (data listener up, `/__admin/health` 200).
2. Replay fixture `schemas-catalog-list-schemas` — or issue the ListSchemas calls it records directly: `GET /v1/registries/name/{RegistryName}/schemas?limit=<n>[&nextToken=<token from the previous page>]` against the service-owned AWS-managed registry, walking the pages as the fixture does.
3. Cause 1 — compare any returned `Schemas[i].LastModified` against the recording for the same `SchemaName` (step `task#1`: `aws.a4b@AWSAPICallviaCloudTrail`). The emulator returns an instant years away from the catalogue's publication window, and entries the catalogue published together no longer share an instant.
4. Cause 2 — walk to the pages recorded as `task#10`, `task#22` and `task#32` and compare `Schemas[i].SchemaName` index by index. The emulator emits six synthesized generic CloudTrail/ServiceEvent entries for sources that publish none (`aws.aidevops`, `aws.application-autoscaling`), plus the case-variant `aws.autoscaling@AWSAPICallViaCloudTrail`, which displaces the real entries and moves every subsequent page boundary.
5. Both reproduce on every boot: nothing here reads the clock, so the wrong values are stable and no per-run value is involved.
