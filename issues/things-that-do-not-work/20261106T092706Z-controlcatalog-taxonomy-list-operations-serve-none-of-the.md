Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-domains
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-domains controlcatalog-catalog-list-objectives
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListDomains|2xx->2xx|$.Domains[Arn="arn
Issue-Key: controlcatalog|ListObjectives|2xx->2xx|$.Objectives[Arn="arn
Issue-Commit: 57b88ffebde7e0555a50fc5146f1edae04c57e49

# ControlCatalog taxonomy list operations serve none of the real domains or objectives

> _Note: this issue was identified against the code base at commit `57b88ffebde7e0555a50fc5146f1edae04c57e49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDomains

## Description
The emulator's ControlCatalog taxonomy enumeration endpoints answer 200 but their result lists contain none of the real catalog taxonomy. Two operations are affected and each needs its own data/handler fix; this bundle carries 2 causes.

Cause 1 of 2: ListDomains returns a Domains list that holds none of the catalog's domains.
AWS: Smithy (services/service-com-amazonaws-controlcatalog-controlcatalog) — ListDomains: POST /domains, success code 200, output shape ListDomainsResponse with `Domains` (DomainSummaryList) @required and optional `NextToken` (@length max 1024); input is MaxResults (httpQuery maxResults, @range 1..100) and NextToken (httpQuery nextToken). DomainSummary requires Arn (DomainArn, @length 33..2048, @pattern ^arn:(aws(?:[-a-z]*)?):controlcatalog:::domain/[0-9a-z]+$), Name, Description, CreateTime and LastUpdateTime. ControlCatalog's domain taxonomy is an AWS-published, account-independent, read-only catalog (the operation is @readonly and the ARNs carry no account or region), so an unfiltered ListDomains must enumerate the whole published domain set, one DomainSummary per domain, each with its real Arn, Name, Description and its create/last-update timestamps. The recording shows that set (domains such as asset management, business continuity and recovery, data protection, identity and access management, each with its published ARN and description).
Emulator: the step returns 200 with a Domains list in which every domain the recording expects is absent — each expected element resolves to None when looked up by its Arn, and no element is matched. The emulator therefore serves no part of the published domain taxonomy, only (at most) entries whose ARNs exist in no real catalog.
Why: a client enumerates ListDomains to discover the domain ARNs it then feeds to ListObjectives/ListCommonControls filters and to render a control-catalog tree. Against the emulator that enumeration comes back carrying none of the real domains, so every downstream lookup keyed on a published domain ARN has nothing to resolve and the client concludes the catalog is empty rather than failing visibly.
Fix: the ListDomains handler and the static ControlCatalog taxonomy seed data it reads — the domain table must be seeded with the published domain set (ARN, name, description, create/last-update time) and the handler must page it per MaxResults/NextToken.
Traces: controlcatalog-catalog-list-domains#task#1.

Cause 2 of 2: ListObjectives returns an Objectives list that holds none of the catalog's objectives.
AWS: Smithy — ListObjectives: POST /objectives, success code 200, output ListObjectivesResponse with `Objectives` (ObjectiveSummaryList) @required plus optional NextToken; input MaxResults (@range 1..100), NextToken and ObjectiveFilter. ObjectiveSummary requires Arn (ObjectiveArn, @length 36..2048, @pattern ^arn:(aws(?:[-a-z]*)?):controlcatalog:::objective/[0-9a-z]+$), Name, Description, Domain (AssociatedDomainSummary = the owning domain's Arn + Name), CreateTime and LastUpdateTime. As with domains this is the AWS-published read-only taxonomy, so an unfiltered ListObjectives must enumerate every published objective, each carrying its real ARN, name, description and the domain it belongs to. The recording shows that set (objectives such as asset inventory management, asset classification and asset maintenance, each nested under its asset-management domain).
Emulator: the step returns 200 with an Objectives list in which every objective the recording expects is absent — each expected element resolves to None when looked up by its Arn.
Why: a client that walks domains -> objectives -> common controls gets an empty objective level, so it can neither build the taxonomy tree nor obtain the objective ARNs that ListCommonControls' ObjectiveFilter requires; combined with the already-open empty-list defects for ListCommonControls and ListControlMappings the whole catalog surface reads as populated-but-empty instead of erroring.
Fix: the ListObjectives handler, its ObjectiveFilter handling and the same static taxonomy seed data — the objective table must be seeded with the published objectives including each one's owning-domain Arn/Name, then filtered by ObjectiveFilter and paged per MaxResults/NextToken.
Traces: controlcatalog-catalog-list-objectives#task#1.

Related open issues, which do NOT cover either cause above (they are about different operations): "ListCommonControls returns an empty CommonControls list for every objective filter" (20261030T021409Z), "ListControlMappings returns an empty ControlMappings list for every control ARN filter" (20261029T195408Z) and "ControlCatalog serves a fabricated placeholder control dataset" (20261101T194508Z). The two causes here are the domain and objective levels of the same taxonomy and are unfiled.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200).

Cause 1 — fixture controlcatalog-catalog-list-domains, step task#1, operation ListDomains, verdict mismatch, expected_status 200 / actual_status 200:
  $.Domains[Arn="arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq"]: missing (expected {'Arn': 'arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq', 'CreateTime': 1710288000.0, 'Description': 'This control domain focuses on asset management ...', 'LastUpdateTime': 1710288000.0, 'Name': 'Asset management'}, got None)
  $.Domains[Arn="arn:aws:controlcatalog:::domain/33mjpzadrlwo1by3c1012ai5i"]: missing (expected {... 'Name': 'Business continuity and recovery'}, got None)
  $.Domains[Arn="arn:aws:controlcatalog:::domain/884eyhr622uy73h6i1mjb9lfb"]: missing (expected {... 'Name': 'Data protection'}, got None)
  $.Domains[Arn="arn:aws:controlcatalog:::domain/avh1ncrx6oyndrbsv0rfywvat"]: missing (... Identity and access management ...)
  ... every expected Domains element reports "got None"; no element matched.
The ARNs, names, descriptions and 1710288000.0 timestamps above are the recording's values filling the DomainSummary template; the requirement is that the published domain set is enumerated with its own real values, not these literals.

Cause 2 — fixture controlcatalog-catalog-list-objectives, step task#1, operation ListObjectives, verdict mismatch, expected_status 200 / actual_status 200:
  $.Objectives[Arn="arn:aws:controlcatalog:::objective/ad11p1961s8erra9m185wa1nn"]: missing (expected {'Arn': ..., 'CreateTime': 1710288000.0, 'Description': 'This control objective focuses on maintaining an accurate and up-to-date inventory of assets ...', 'Domain': {'Arn': 'arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq', 'Name': 'Asset management'}, 'LastUpdateTime': 1710288000.0, 'Name': 'Asset inventory management'}, got None)
  $.Objectives[Arn="arn:aws:controlcatalog:::objective/90gifwthorhxhxq7m0rtss98u"]: missing (... 'Name': 'Asset classification' ...)
  $.Objectives[Arn="arn:aws:controlcatalog:::objective/3frxxgl64u9kzttiuheywykf7"]: missing (... 'Name': 'Asset maintenance' ...)
  ... every expected Objectives element reports "got None".
No status or error-code divergence in either cause: both return the modeled 200, and the defect is entirely the absent required list content.

## Steps to Reproduce
1. Boot the emulator from this checkout (the ControlCatalog service is bootstrapped; /__admin/health answers 200).
2. Cause 1: replay fixture controlcatalog-catalog-list-domains, step task#1 — POST /domains with no filters (ListDomains). Observe 200 whose Domains list contains none of the published domain ARNs (e.g. nothing with Arn arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq); AWS returns one DomainSummary per published domain with Arn/Name/Description/CreateTime/LastUpdateTime.
3. Cause 2: replay fixture controlcatalog-catalog-list-objectives, step task#1 — POST /objectives with no ObjectiveFilter (ListObjectives). Observe 200 whose Objectives list contains none of the published objective ARNs (e.g. nothing with Arn arn:aws:controlcatalog:::objective/ad11p1961s8erra9m185wa1nn); AWS returns one ObjectiveSummary per published objective, each with its owning Domain {Arn, Name}.
4. Re-replay both fixture steps after seeding the taxonomy to confirm; both must match before this issue is closed.
