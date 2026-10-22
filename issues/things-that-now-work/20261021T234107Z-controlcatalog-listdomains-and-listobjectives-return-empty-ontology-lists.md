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
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# ListDomains and ListObjectives return empty ontology lists

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDomains

## Description
The top of the Control Catalog ontology is not served: the domain and objective listings answer 200 with empty lists. 2 causes, both on service controlcatalog.

Cause 1 of 2 - ListDomains returns no domains (fixture controlcatalog-catalog-list-domains, step task#1).
AWS: Smithy: ListDomains, POST /domains, output ListDomainsResponse.Domains required (DomainSummaryList of Arn, Name, Description, CreateTime, LastUpdateTime), @paginated on Domains, MaxResults 1..100. AWS returns the catalog's control domains (Asset management, Business continuity and recovery, Data protection, Identity and access management, ...).
Emulator: HTTP 200 with the expected DomainSummary entries absent (each $.Domains[Arn=...] reported missing, got None).
Why: domains are the entry point for browsing the catalog; an empty list makes every downstream objective/common-control query unreachable and is indistinguishable from a catalog with no domains.
Fix: the ListDomains handler and the domain data it enumerates.

Cause 2 of 2 - ListObjectives returns no objectives (fixture controlcatalog-catalog-list-objectives, step task#1).
AWS: Smithy: ListObjectives, POST /objectives, input ObjectiveFilter.Domains (documented as one domain ARN at a time), output ListObjectivesResponse.Objectives required (ObjectiveSummaryList of Arn, Name, Description, Domain {Arn, Name}, CreateTime, LastUpdateTime), @paginated on Objectives. AWS returns the objectives of the catalog (optionally of one domain).
Emulator: HTTP 200 with the expected ObjectiveSummary entries absent (each $.Objectives[Arn=...] reported missing, got None).
Why: objectives are the join between a domain and its common controls; with none returned, a client walking domain -> objective -> common control -> control stops at step two and reports the catalog as empty.
Fix: the ListObjectives handler and the objective data it enumerates/filters.

Traces: controlcatalog-catalog-list-domains#task#1 and controlcatalog-catalog-list-objectives#task#1; both handlers and the backing data live in services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/ and src/state.rs.

## Evidence
Cause 1: fixture controlcatalog-catalog-list-domains step task#1 - expected_status 200, actual_status 200; $.Domains[Arn="arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq"] missing (expected {Name: 'Asset management', CreateTime 1710288000.0, LastUpdateTime 1710288000.0, Description ...}, got None), likewise domain/33mjpzadrlwo1by3c1012ai5i ('Business continuity and recovery'), domain/884eyhr622uy73h6i1mjb9lfb ('Data protection'), domain/avh1ncrx6oyndrbsv0rfywvat and the rest.
Cause 2: fixture controlcatalog-catalog-list-objectives step task#1 - expected_status 200, actual_status 200; $.Objectives[Arn="arn:aws:controlcatalog:::objective/ad11p1961s8erra9m185wa1nn"] missing (expected {Name: 'Asset inventory management', Domain: {Arn: .../domain/d4msesd9vvmzmmuvlv06m92uq, Name: 'Asset management'}}, got None), likewise objective/90gifwthorhxhxq7m0rtss98u, objective/3frxxgl64u9kzttiuheywykf7, objective/5ve4jodybrg8wnky75fp50sbf and the rest.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture controlcatalog-catalog-list-domains, step task#1 (POST /domains): 200 with an empty Domains list instead of the catalog's DomainSummary records.
3. Replay fixture controlcatalog-catalog-list-objectives, step task#1 (POST /objectives): 200 with an empty Objectives list instead of the catalog's ObjectiveSummary records.
