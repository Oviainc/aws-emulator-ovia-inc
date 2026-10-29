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
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# ListDomains and ListObjectives return empty catalog taxonomy lists

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDomains

## Description
The two taxonomy enumerations of ControlCatalog answer 200 with no entries, so the domain/objective tree the rest of the catalog references does not exist on the emulator. Two causes.

Cause 1 of 2 - ListDomains returns no domains (fixture controlcatalog-catalog-list-domains, step task#1).
AWS: Smithy ListDomains (POST /domains, paginated inputToken NextToken / outputToken NextToken / pageSize MaxResults, items Domains; Domains is a required list) returns the catalog's domain summaries, each with Arn, Name, Description, CreateTime and LastUpdateTime. The recorded response lists the catalog's domains (asset management, business continuity and recovery, data protection, identity and access management, and so on).
Emulator: status matches (200) but every expected domain is absent - the report lists $.Domains[Arn="..."] : missing (expected the domain summary, got None) for each domain ARN - i.e. the required Domains list is empty.
Why: a client that enumerates domains to build the catalog taxonomy, or that resolves the Domain.Arn embedded in common-control and objective summaries, finds no domains at all and renders an empty catalog instead of reporting a failure.
Fix: the ListDomains handler's projection and the domain seed state it reads in services/service-com-amazonaws-controlcatalog-controlcatalog.

Cause 2 of 2 - ListObjectives returns no objectives (fixture controlcatalog-catalog-list-objectives, step task#1).
AWS: Smithy ListObjectives (POST /objectives, paginated inputToken NextToken / outputToken NextToken / pageSize MaxResults, items Objectives; Objectives is a required list, ObjectiveFilter optional) returns the catalog's objective summaries, each with Arn, Name, Description, Domain {Arn, Name}, CreateTime and LastUpdateTime.
Emulator: status matches (200) but every expected objective is absent - $.Objectives[Arn="..."] : missing (expected the objective summary, got None) for each objective ARN - the required Objectives list is empty.
Why: a client that lists objectives for a domain, or resolves the Objective.Arn that common-control summaries carry, sees none, so the domain-to-objective-to-common-control hierarchy cannot be walked on the emulator.
Fix: the ListObjectives handler's filter-and-project path and the objective seed state in the same service crate; objectives must be present and selectable by their owning domain.

Traces: controlcatalog-catalog-list-domains#task#1; controlcatalog-catalog-list-objectives#task#1.

## Evidence
Cause 1: status 200 vs 200, but $.Domains[Arn="arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq"]: missing (expected {'Name': 'Asset management', 'Description': ..., 'CreateTime': ..., 'LastUpdateTime': ...}, got None), and the same for domain/33mjpzadrlwo1by3c1012ai5i ('Business continuity and recovery'), domain/884eyhr622uy73h6i1mjb9lfb ('Data protection'), domain/avh1ncrx6oyndrbsv0rfywvat and the remaining domains - the emulator's Domains list is empty.
Cause 2: status 200 vs 200, but $.Objectives[Arn="arn:aws:controlcatalog:::objective/ad11p1961s8erra9m185wa1nn"]: missing (expected {'Name': 'Asset inventory management', 'Domain': {'Arn': 'arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq', 'Name': 'Asset management'}, ...}, got None), and the same for objective/90gifwthorhxhxq7m0rtss98u ('Asset classification'), objective/3frxxgl64u9kzttiuheywykf7 ('Asset maintenance'), objective/5ve4jodybrg8wnky75fp50sbf and the rest - the emulator's Objectives list is empty.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay fixture controlcatalog-catalog-list-domains step task#1: POST /domains -> emulator returns 200 with an empty Domains list; AWS returns the catalog's domain summaries.
3. Replay fixture controlcatalog-catalog-list-objectives step task#1: POST /objectives -> emulator returns 200 with an empty Objectives list; AWS returns the catalog's objective summaries.
